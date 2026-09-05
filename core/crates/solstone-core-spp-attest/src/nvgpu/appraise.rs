// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shell-out GPU appraisal through the locally provisioned nvattest binary.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;

use crate::{
    error::GpuAppraisalReason,
    nvgpu::{
        NvattestVerdict, build_gpu_appraisal, build_nvattest_attest_command,
        classify_nvattest_result, parse_nvattest_stdout,
    },
    snp::AppraisalStep,
    tlv::GpuEnvelope,
};

/// Maximum wall-clock duration for the nvattest subprocess.
pub const NVATTEST_TIMEOUT: Duration = Duration::from_secs(60);

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

/// Appraises one GPU evidence envelope against an owner nonce.
pub trait GpuAppraiser {
    fn appraise(
        &self,
        envelope: &GpuEnvelope,
        owner_nonce: &[u8; 32],
        nvattest_dir: &Path,
    ) -> Result<crate::nvgpu::GpuAppraisal, GpuAppraisalReason>;
}

/// Production GPU appraiser backed by the local nvattest executable.
#[derive(Debug, Default, Clone, Copy)]
pub struct NvattestGpuAppraiser;

impl GpuAppraiser for NvattestGpuAppraiser {
    fn appraise(
        &self,
        envelope: &GpuEnvelope,
        owner_nonce: &[u8; 32],
        nvattest_dir: &Path,
    ) -> Result<crate::nvgpu::GpuAppraisal, GpuAppraisalReason> {
        self.appraise_with_timeout(envelope, owner_nonce, nvattest_dir, NVATTEST_TIMEOUT)
    }
}

/// Appraises a GPU leg with the production nvattest implementation.
pub fn appraise_gpu_leg(
    envelope: &GpuEnvelope,
    owner_nonce: &[u8; 32],
    nvattest_dir: &Path,
) -> Result<crate::nvgpu::GpuAppraisal, GpuAppraisalReason> {
    NvattestGpuAppraiser.appraise(envelope, owner_nonce, nvattest_dir)
}

impl NvattestGpuAppraiser {
    fn appraise_with_timeout(
        &self,
        envelope: &GpuEnvelope,
        owner_nonce: &[u8; 32],
        nvattest_dir: &Path,
        timeout: Duration,
    ) -> Result<crate::nvgpu::GpuAppraisal, GpuAppraisalReason> {
        let evidence_file = TempEvidenceFile::write(envelope, owner_nonce)?;
        let command = build_nvattest_attest_command(
            nvattest_dir,
            evidence_file.path(),
            owner_nonce,
            "remote",
            None,
        )?;
        let output = run_nvattest(command, timeout)?;
        let stdout =
            String::from_utf8(output.stdout).map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
        let stdout =
            parse_nvattest_stdout(&stdout).map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;

        let verdict =
            classify_nvattest_result(output.status.code().unwrap_or(-1), &stdout, owner_nonce);
        let acceptance = match verdict {
            NvattestVerdict::Accepted(acceptance) => acceptance,
            NvattestVerdict::Rejected(rejection) => return Err(rejection.reason),
        };

        build_gpu_appraisal(&acceptance.claim, envelope, appraisal_steps())
            .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)
    }
}

fn appraisal_steps() -> Vec<AppraisalStep> {
    vec![
        AppraisalStep {
            name: "nvattest",
            status: "ok",
            detail: "returncode=0 result_code=0 result_message=Ok".to_owned(),
        },
        AppraisalStep {
            name: "overall-eat",
            status: "ok",
            detail: "alg=none iss=NVAT-LOCAL-VERIFIER overall_att_result=True".to_owned(),
        },
        AppraisalStep {
            name: "gpu-claims",
            status: "ok",
            detail: "claims-version=3.0 report, driver-RIM, vbios-RIM checks passed".to_owned(),
        },
    ]
}

fn run_nvattest(
    invocation: crate::nvgpu::NvattestCommand,
    timeout: Duration,
) -> Result<Output, GpuAppraisalReason> {
    let mut process = Command::new(&invocation.executable)
        .args(invocation.argv.iter().skip(1))
        .envs(invocation.env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| GpuAppraisalReason::NvattestUnavailable)?;
    let deadline = Instant::now() + timeout;
    let stdout = process.stdout.take().expect("nvattest stdout is piped");
    let stderr = process.stderr.take().expect("nvattest stderr is piped");

    thread::scope(|scope| {
        let result = (|| {
            let stdout = thread::Builder::new()
                .name("nvattest-stdout".to_owned())
                .spawn_scoped(scope, move || read_pipe(stdout))
                .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
            let stderr = thread::Builder::new()
                .name("nvattest-stderr".to_owned())
                .spawn_scoped(scope, move || read_pipe(stderr))
                .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
            let status = wait_nvattest(&mut process, deadline)?;
            let stdout = stdout
                .join()
                .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?
                .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
            let stderr = stderr
                .join()
                .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?
                .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
            Ok(Output {
                status,
                stdout,
                stderr,
            })
        })();
        if result.is_err() {
            // Stop the producer before the scope joins any remaining pipe reader.
            let _ = process.kill();
            let _ = process.wait();
        }
        result
    })
}

fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn wait_nvattest(process: &mut Child, deadline: Instant) -> Result<ExitStatus, GpuAppraisalReason> {
    loop {
        if let Some(status) = process
            .try_wait()
            .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(GpuAppraisalReason::GpuAppraisalFailed);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

struct TempEvidenceFile {
    path: PathBuf,
}

impl TempEvidenceFile {
    fn write(envelope: &GpuEnvelope, owner_nonce: &[u8; 32]) -> Result<Self, GpuAppraisalReason> {
        let evidence = evidence_json(envelope, owner_nonce)?;
        let mut path = std::env::temp_dir();
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?
            .as_nanos();
        path.push(format!(
            "solstone-nvattest-{}-{timestamp}-{sequence}.json",
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
        let evidence_file = Self { path };
        let write_result = write_evidence(&mut file, &evidence);
        drop(file);
        write_result?;
        Ok(evidence_file)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempEvidenceFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn evidence_json(
    envelope: &GpuEnvelope,
    owner_nonce: &[u8; 32],
) -> Result<serde_json::Value, GpuAppraisalReason> {
    let arch = std::str::from_utf8(
        envelope
            .field(7)
            .ok_or(GpuAppraisalReason::GpuAppraisalFailed)?,
    )
    .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?
    .to_uppercase();
    let certificate = STANDARD.encode(
        envelope
            .field(3)
            .ok_or(GpuAppraisalReason::GpuAppraisalFailed)?,
    );
    let evidence = STANDARD.encode(
        envelope
            .field(2)
            .ok_or(GpuAppraisalReason::GpuAppraisalFailed)?,
    );
    Ok(json!([{
        "arch": arch,
        "certificate": certificate,
        "evidence": evidence,
        "nonce": hex_lower(owner_nonce),
    }]))
}

fn write_evidence(file: &mut File, evidence: &serde_json::Value) -> Result<(), GpuAppraisalReason> {
    serde_json::to_writer(&mut *file, evidence)
        .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
    file.write_all(b"\n")
        .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[usize::from(byte >> 4)] as char);
        result.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    use super::{GpuAppraisalReason, GpuAppraiser, NvattestGpuAppraiser};
    use crate::{test_support::fixture_bytes, tlv::decode_gpu_envelope};

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-spp-attest-appraise-test-{}-{}",
                std::process::id(),
                NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn owner_nonce() -> [u8; 32] {
        let hex = String::from_utf8(fixture_bytes("nonce.hex")).expect("nonce is UTF-8");
        let bytes = hex
            .split_whitespace()
            .flat_map(|line| line.as_bytes().chunks_exact(2))
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                    .expect("valid hex")
            })
            .collect::<Vec<_>>();
        bytes.try_into().expect("fixture nonce is 32 bytes")
    }

    fn install_script(root: &Path, script: &str) {
        fs::create_dir_all(root.join("bin")).expect("create bin");
        fs::create_dir_all(root.join("lib")).expect("create lib");
        fs::create_dir_all(root.join("share/ca")).expect("create CA directory");
        let binary = root.join("bin/nvattest");
        fs::write(&binary, script).expect("write fake nvattest");
        let mut permissions = fs::metadata(&binary)
            .expect("binary metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(binary, permissions).expect("make fake nvattest executable");
        fs::write(root.join("share/ca/ca-bundle.pem"), "CA").expect("write CA bundle");
    }

    fn appraise(
        root: &Path,
        timeout: Duration,
    ) -> Result<crate::nvgpu::GpuAppraisal, GpuAppraisalReason> {
        let envelope = decode_gpu_envelope(&fixture_bytes("gpu-envelope.tlv")).expect("envelope");
        NvattestGpuAppraiser.appraise_with_timeout(&envelope, &owner_nonce(), root, timeout)
    }

    #[test]
    fn appraiser_builds_a_gpu_appraisal_from_green_stdout() {
        let root = TempDir::new();
        let output = root.path().join("positive.stdout");
        fs::write(&output, fixture_bytes("nvattest/positive.stdout")).expect("write stdout");
        install_script(
            root.path(),
            "#!/bin/sh\ncat \"$(dirname \"$0\")/../positive.stdout\"\n",
        );

        let appraisal = appraise(root.path(), Duration::from_secs(1)).expect("green appraisal");
        assert_eq!(appraisal.hwmodel, "GH100 A01 GSP BROM");
    }

    #[test]
    fn appraiser_drains_large_valid_stdout_before_waiting_for_exit() {
        let root = TempDir::new();
        let mut output = vec![b' '; 256 * 1024];
        output.extend(fixture_bytes("nvattest/positive.stdout"));
        fs::write(root.path().join("positive.stdout"), output).expect("write stdout");
        install_script(
            root.path(),
            "#!/bin/sh\nexec cat \"$(dirname \"$0\")/../positive.stdout\"\n",
        );

        let appraisal = appraise(root.path(), Duration::from_secs(1)).expect("green appraisal");
        assert_eq!(appraisal.hwmodel, "GH100 A01 GSP BROM");
    }

    #[test]
    fn appraiser_drains_large_stderr_while_preserving_valid_stdout() {
        let root = TempDir::new();
        fs::write(
            root.path().join("diagnostic.stderr"),
            vec![b'x'; 256 * 1024],
        )
        .expect("write stderr");
        fs::write(
            root.path().join("positive.stdout"),
            fixture_bytes("nvattest/positive.stdout"),
        )
        .expect("write stdout");
        install_script(
            root.path(),
            "#!/bin/sh\nbase=$(dirname \"$0\")/..\ncat \"$base/diagnostic.stderr\" >&2\nexec cat \"$base/positive.stdout\"\n",
        );

        let appraisal = appraise(root.path(), Duration::from_secs(1)).expect("green appraisal");
        assert_eq!(appraisal.hwmodel, "GH100 A01 GSP BROM");
    }

    #[test]
    fn appraiser_maps_malformed_stdout_to_gpu_appraisal_failed() {
        let root = TempDir::new();
        install_script(root.path(), "#!/bin/sh\nprintf 'not json\\n'\n");

        assert_eq!(
            appraise(root.path(), Duration::from_secs(1)),
            Err(GpuAppraisalReason::GpuAppraisalFailed)
        );
    }

    #[test]
    fn appraiser_maps_timeout_to_gpu_appraisal_failed() {
        let root = TempDir::new();
        install_script(root.path(), "#!/bin/sh\nexec sleep 10\n");

        assert_eq!(
            appraise(root.path(), Duration::from_millis(20)),
            Err(GpuAppraisalReason::GpuAppraisalFailed)
        );
    }

    #[test]
    fn appraiser_maps_missing_binary_to_nvattest_unavailable() {
        let root = TempDir::new();
        fs::create_dir_all(root.path()).expect("create root");

        let envelope = decode_gpu_envelope(&fixture_bytes("gpu-envelope.tlv")).expect("envelope");
        assert_eq!(
            NvattestGpuAppraiser.appraise(&envelope, &owner_nonce(), root.path()),
            Err(GpuAppraisalReason::NvattestUnavailable)
        );
    }
}
