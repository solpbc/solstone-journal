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
        GpuProfile, NvattestVerdict, StatusExpectation, StatusMode, build_gpu_appraisal,
        build_nvattest_attest_command, build_nvattest_offline_attest_command,
        classify_nvattest_result, parse_nvattest_stdout,
    },
    snp::AppraisalStep,
    tlv::GpuEnvelope,
};

/// Maximum wall-clock duration for the nvattest subprocess.
pub const NVATTEST_TIMEOUT: Duration = Duration::from_secs(60);
/// The helper's JSON result is a few kilobytes; anything near this is refused.
const MAX_NVATTEST_STDOUT_BYTES: usize = 4 * 1024 * 1024;
/// Diagnostics are drained so the helper never blocks, but kept only to here.
const MAX_NVATTEST_STDERR_BYTES: usize = 1024 * 1024;

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

/// The locally selected status inputs for one GPU appraisal.
#[derive(Debug, Clone, Copy)]
pub struct GpuStatusInput<'a> {
    /// The profile the verified CPU fingerprint selected.
    pub profile: &'a GpuProfile,
    /// The engine's raw status-proof bundle, when its certificate carried one.
    /// Only an offline profile reads it; its presence never selects a mode.
    pub proofs: Option<&'a [u8]>,
    /// This device's clock when the appraisal starts.
    pub verification_time: SystemTime,
}

/// Appraises one GPU evidence envelope against an owner nonce.
pub trait GpuAppraiser {
    fn appraise(
        &self,
        envelope: &GpuEnvelope,
        owner_nonce: &[u8; 32],
        nvattest_dir: &Path,
        status: &GpuStatusInput<'_>,
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
        status: &GpuStatusInput<'_>,
    ) -> Result<crate::nvgpu::GpuAppraisal, GpuAppraisalReason> {
        self.appraise_with_timeout(
            envelope,
            owner_nonce,
            nvattest_dir,
            status,
            NVATTEST_TIMEOUT,
        )
    }
}

/// Appraises a GPU leg with the production nvattest implementation.
pub fn appraise_gpu_leg(
    envelope: &GpuEnvelope,
    owner_nonce: &[u8; 32],
    nvattest_dir: &Path,
    status: &GpuStatusInput<'_>,
) -> Result<crate::nvgpu::GpuAppraisal, GpuAppraisalReason> {
    NvattestGpuAppraiser.appraise(envelope, owner_nonce, nvattest_dir, status)
}

impl NvattestGpuAppraiser {
    fn appraise_with_timeout(
        &self,
        envelope: &GpuEnvelope,
        owner_nonce: &[u8; 32],
        nvattest_dir: &Path,
        status: &GpuStatusInput<'_>,
        timeout: Duration,
    ) -> Result<crate::nvgpu::GpuAppraisal, GpuAppraisalReason> {
        // An offline profile without proofs stops here, before the helper is
        // found or launched: there is no online fallback to try.
        let offline_proofs = match status.profile.status() {
            StatusMode::OnlineNonce => None,
            StatusMode::OfflineSignedAge => Some(
                status
                    .proofs
                    .ok_or(GpuAppraisalReason::StatusProofsMissing)?,
            ),
        };
        crate::nvgpu::locate_nvattest(nvattest_dir)?;
        let rims = super::rims::TempRimDir::write(status.profile)?;
        let evidence_file = TempEvidenceFile::write(envelope, owner_nonce)?;
        let (command, expectation, _proof_file) = match offline_proofs {
            None => (
                build_nvattest_attest_command(
                    nvattest_dir,
                    evidence_file.path(),
                    owner_nonce,
                    "dir",
                    Some(rims.path()),
                )?,
                StatusExpectation::OnlineNonce,
                None,
            ),
            Some(proofs) => {
                let verification_time_unix = status
                    .verification_time
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
                    .filter(|seconds| *seconds > 0)
                    .ok_or(GpuAppraisalReason::GpuAppraisalFailed)?;
                let proof_file = TempFile::write("proofs", "der", proofs)?;
                (
                    build_nvattest_offline_attest_command(
                        nvattest_dir,
                        evidence_file.path(),
                        owner_nonce,
                        rims.path(),
                        proof_file.path(),
                        verification_time_unix,
                    )?,
                    StatusExpectation::OfflineSignedAge {
                        verification_time_unix,
                    },
                    Some(proof_file),
                )
            }
        };
        let output = run_nvattest(command, timeout)?;
        let stdout =
            String::from_utf8(output.stdout).map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
        let stdout =
            parse_nvattest_stdout(&stdout).map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;

        let verdict = classify_nvattest_result(
            output.status.code().unwrap_or(-1),
            &stdout,
            owner_nonce,
            expectation,
        );
        let acceptance = match verdict {
            NvattestVerdict::Accepted(acceptance) => acceptance,
            NvattestVerdict::Rejected(rejection) => return Err(rejection.reason),
        };

        build_gpu_appraisal(
            &acceptance.claim,
            envelope,
            appraisal_steps(expectation),
            acceptance.status,
        )
        .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)
    }
}

fn appraisal_steps(expectation: StatusExpectation) -> Vec<AppraisalStep> {
    let status = match expectation {
        StatusExpectation::OnlineNonce => "nonce-bearing OCSP",
        StatusExpectation::OfflineSignedAge { .. } => "signed-age OCSP proofs",
    };
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
            detail: format!(
                "claims-version=3.0 report, driver-RIM, vbios-RIM checks passed; status={status}"
            ),
        },
    ]
}

fn run_nvattest(
    invocation: crate::nvgpu::NvattestCommand,
    timeout: Duration,
) -> Result<Output, GpuAppraisalReason> {
    // The released Linux binary's RUNPATH ends in an empty entry, which the
    // loader reads as the working directory: never inherit the caller's.
    let mut process = Command::new(&invocation.executable)
        .args(invocation.argv.iter().skip(1))
        .envs(invocation.env)
        .current_dir("/")
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
                .spawn_scoped(scope, move || read_pipe(stdout, MAX_NVATTEST_STDOUT_BYTES))
                .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
            let stderr = thread::Builder::new()
                .name("nvattest-stderr".to_owned())
                .spawn_scoped(scope, move || read_pipe(stderr, MAX_NVATTEST_STDERR_BYTES))
                .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
            let status = wait_nvattest(&mut process, deadline)?;
            let (stdout, stdout_overflowed) = stdout
                .join()
                .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?
                .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
            if stdout_overflowed {
                return Err(GpuAppraisalReason::GpuAppraisalFailed);
            }
            let (stderr, _) = stderr
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

/// Reads a pipe to its end, keeping at most `limit` bytes. Bytes past the
/// limit are still drained so the helper never blocks on a full pipe.
fn read_pipe(mut pipe: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut overflowed = false;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = match pipe.read(&mut buffer) {
            Ok(0) => return Ok((bytes, overflowed)),
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let room = limit.saturating_sub(bytes.len());
        if read > room {
            overflowed = true;
        }
        bytes.extend_from_slice(&buffer[..read.min(room)]);
    }
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

/// A private, create-new temporary file removed on drop.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn write(label: &str, extension: &str, bytes: &[u8]) -> Result<Self, GpuAppraisalReason> {
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-nvattest-{label}-{}-{timestamp}-{sequence}.{extension}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
        let temp = Self { path };
        let written = file.write_all(bytes);
        drop(file);
        written.map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
        Ok(temp)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
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
    use std::{fs, path::Path, path::PathBuf, sync::atomic::AtomicU64, sync::atomic::Ordering};
    #[cfg(all(test, feature = "full-tests"))]
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    use super::{GpuAppraisalReason, GpuAppraiser, GpuStatusInput, NvattestGpuAppraiser};
    use crate::{test_support::fixture_bytes, tlv::decode_gpu_envelope};

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    /// Room for a stand-in helper to finish under a fully parallel test run on
    /// a loaded host; the timeout test keeps its own short deadline.
    #[cfg(all(test, feature = "full-tests"))]
    const HELPER_DEADLINE: Duration = Duration::from_secs(10);

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

    #[cfg(all(test, feature = "full-tests"))]
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

    fn production_profile() -> crate::nvgpu::GpuProfile {
        crate::nvgpu::GpuProfiles::production()
            .select(crate::pins::PRODUCTION_PCR_SHA256_PINS[0])
            .expect("production profile")
            .clone()
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn offline_profile() -> crate::nvgpu::GpuProfile {
        crate::nvgpu::GpuProfile::new(
            "33".repeat(32),
            crate::nvgpu::ManifestSet::QUALIFIED_595_71_05,
            crate::nvgpu::StatusMode::OfflineSignedAge,
        )
    }

    #[cfg(all(test, feature = "full-tests"))]
    const VERIFIED_AT: u64 = 1_790_996_648;

    #[cfg(all(test, feature = "full-tests"))]
    fn appraise(
        root: &Path,
        timeout: Duration,
    ) -> Result<crate::nvgpu::GpuAppraisal, GpuAppraisalReason> {
        let profile = production_profile();
        appraise_with(root, timeout, &profile, None)
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn appraise_with(
        root: &Path,
        timeout: Duration,
        profile: &crate::nvgpu::GpuProfile,
        proofs: Option<&[u8]>,
    ) -> Result<crate::nvgpu::GpuAppraisal, GpuAppraisalReason> {
        let envelope = decode_gpu_envelope(&fixture_bytes("gpu-envelope.tlv")).expect("envelope");
        NvattestGpuAppraiser.appraise_with_timeout(
            &envelope,
            &owner_nonce(),
            root,
            &GpuStatusInput {
                profile,
                proofs,
                verification_time: std::time::UNIX_EPOCH + Duration::from_secs(VERIFIED_AT),
            },
            timeout,
        )
    }

    /// The positive helper output, rewritten the way an offline helper reports
    /// each certificate chain.
    #[cfg(all(test, feature = "full-tests"))]
    fn offline_stdout(verification_time: u64, deadline: u64) -> Vec<u8> {
        let mut body: serde_json::Value =
            serde_json::from_slice(&fixture_bytes("nvattest/positive.stdout")).expect("stdout");
        let claim = body["claims"][0].as_object_mut().expect("claim");
        for key in [
            "x-nvidia-gpu-attestation-report-cert-chain",
            "x-nvidia-gpu-driver-rim-cert-chain",
            "x-nvidia-gpu-vbios-rim-cert-chain",
        ] {
            let chain = claim[key].as_object_mut().expect("chain");
            chain.insert("x-nvidia-cert-ocsp-nonce-matches".to_owned(), false.into());
            chain.insert(
                "x-sol-cert-ocsp-signed-age".to_owned(),
                serde_json::json!({
                    "version": 1,
                    "mode": "signed-age",
                    "verification_time_unix": verification_time,
                    "status_deadline_unix": deadline,
                    "oldest_this_update_unix": deadline - 86_400,
                    "covered_certificates": 3,
                }),
            );
        }
        serde_json::to_vec(&body).expect("offline stdout")
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn appraiser_builds_a_gpu_appraisal_from_green_stdout() {
        let root = TempDir::new();
        let output = root.path().join("positive.stdout");
        fs::write(&output, fixture_bytes("nvattest/positive.stdout")).expect("write stdout");
        install_script(
            root.path(),
            "#!/bin/sh\ncat \"$(dirname \"$0\")/../positive.stdout\"\n",
        );

        let appraisal = appraise(root.path(), HELPER_DEADLINE).expect("green appraisal");
        assert_eq!(appraisal.hwmodel, "GH100 A01 GSP BROM");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn appraiser_only_supplies_packaged_local_manifests() {
        let root = TempDir::new();
        fs::write(
            root.path().join("positive.stdout"),
            fixture_bytes("nvattest/positive.stdout"),
        )
        .expect("write stdout");
        install_script(
            root.path(),
            r#"#!/bin/sh
store=; rims=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --rim-store) shift; store=$1 ;;
        --rim-dir) shift; rims=$1 ;;
    esac
    shift
done
[ "$store" = dir ] || exit 7
[ -d "$rims" ] || exit 8
set -- "$rims"/*.xml
[ -f "$1" ] || exit 9
exec cat "$(dirname "$0")/../positive.stdout"
"#,
        );
        appraise(root.path(), HELPER_DEADLINE).expect("local manifests supplied");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn offline_profile_without_proofs_does_not_launch_the_verifier() {
        let root = TempDir::new();
        install_script(
            root.path(),
            "#!/bin/sh\ntouch \"$(dirname \"$0\")/../launched\"\n",
        );
        assert_eq!(
            appraise_with(root.path(), HELPER_DEADLINE, &offline_profile(), None),
            Err(GpuAppraisalReason::StatusProofsMissing)
        );
        assert!(!root.path().join("launched").exists());
    }

    /// A stand-in helper that records its arguments and the proof file it was
    /// handed, then prints a prepared result.
    #[cfg(all(test, feature = "full-tests"))]
    fn install_recording_script(root: &Path, stdout: &[u8]) {
        fs::write(root.join("result.stdout"), stdout).expect("write stdout");
        install_script(
            root,
            r#"#!/bin/sh
base=$(dirname "$0")/..
printf '%s\n' "$@" > "$base/argv"
while [ "$#" -gt 0 ]; do
    if [ "$1" = --ocsp-proof-bundle ]; then cp "$2" "$base/proofs.seen"; fi
    shift
done
exec cat "$base/result.stdout"
"#,
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn offline_profile_hands_the_helper_only_local_proofs_and_this_devices_time() {
        let root = TempDir::new();
        let deadline = VERIFIED_AT + 3_600;
        install_recording_script(root.path(), &offline_stdout(VERIFIED_AT, deadline));
        let proofs = b"\x30\x03\x02\x01\x01 raw proof bytes";
        let appraisal = appraise_with(
            root.path(),
            HELPER_DEADLINE,
            &offline_profile(),
            Some(proofs),
        )
        .expect("offline appraisal");
        assert_eq!(
            appraisal.status,
            crate::nvgpu::GpuStatusAuthorization::OfflineSignedAge {
                verified_at: std::time::UNIX_EPOCH + Duration::from_secs(VERIFIED_AT),
                deadline: std::time::UNIX_EPOCH + Duration::from_secs(deadline),
            }
        );
        assert_eq!(
            fs::read(root.path().join("proofs.seen")).expect("proof file seen"),
            proofs
        );
        let argv = fs::read_to_string(root.path().join("argv")).expect("argv");
        let argv = argv.lines().collect::<Vec<_>>();
        let time_at = argv
            .iter()
            .position(|argument| *argument == "--ocsp-verification-time")
            .expect("verification time");
        assert_eq!(argv[time_at + 1], VERIFIED_AT.to_string());
        for refused in [
            "--ca-bundle",
            "--ocsp-url",
            "--rim-url",
            "--nras-url",
            "--service-key",
        ] {
            assert!(!argv.contains(&refused), "{refused}");
        }
        assert_eq!(
            argv[argv.iter().position(|a| *a == "--rim-store").unwrap() + 1],
            "dir"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn legacy_helper_output_cannot_satisfy_an_offline_profile() {
        let root = TempDir::new();
        // A helper that never saw the proofs, or an older one that ignored the
        // flag, still reports a nonce match and no signed-age result.
        install_recording_script(root.path(), &fixture_bytes("nvattest/positive.stdout"));
        assert_eq!(
            appraise_with(
                root.path(),
                HELPER_DEADLINE,
                &offline_profile(),
                Some(b"proof")
            ),
            Err(GpuAppraisalReason::GpuAppraisalFailed)
        );
        // An older helper refuses the unknown flag and prints usage.
        install_script(
            root.path(),
            "#!/bin/sh\necho 'The following argument was not expected' >&2\nexit 109\n",
        );
        assert_eq!(
            appraise_with(
                root.path(),
                HELPER_DEADLINE,
                &offline_profile(),
                Some(b"proof")
            ),
            Err(GpuAppraisalReason::GpuAppraisalFailed)
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn offline_output_cannot_satisfy_the_online_profile() {
        let root = TempDir::new();
        install_recording_script(
            root.path(),
            &offline_stdout(VERIFIED_AT, VERIFIED_AT + 3_600),
        );
        assert_eq!(
            appraise(root.path(), HELPER_DEADLINE),
            Err(GpuAppraisalReason::GpuAppraisalFailed)
        );
        // The online profile ignores proofs: it neither reads nor passes them.
        let root = TempDir::new();
        install_recording_script(root.path(), &fixture_bytes("nvattest/positive.stdout"));
        let profile = production_profile();
        appraise_with(root.path(), HELPER_DEADLINE, &profile, Some(b"ignored"))
            .expect("online appraisal");
        let argv = fs::read_to_string(root.path().join("argv")).expect("argv");
        assert!(!argv.contains("--ocsp-proof-bundle"));
        assert!(!root.path().join("proofs.seen").exists());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn oversized_helper_stdout_is_refused_after_draining() {
        let root = TempDir::new();
        let mut output = vec![b' '; super::MAX_NVATTEST_STDOUT_BYTES + 1];
        output.extend(fixture_bytes("nvattest/positive.stdout"));
        fs::write(root.path().join("positive.stdout"), output).expect("write stdout");
        install_script(
            root.path(),
            "#!/bin/sh\nexec cat \"$(dirname \"$0\")/../positive.stdout\"\n",
        );
        assert_eq!(
            appraise(root.path(), Duration::from_secs(5)),
            Err(GpuAppraisalReason::GpuAppraisalFailed)
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
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

        let appraisal = appraise(root.path(), HELPER_DEADLINE).expect("green appraisal");
        assert_eq!(appraisal.hwmodel, "GH100 A01 GSP BROM");
    }

    #[cfg(all(test, feature = "full-tests"))]
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

        let appraisal = appraise(root.path(), HELPER_DEADLINE).expect("green appraisal");
        assert_eq!(appraisal.hwmodel, "GH100 A01 GSP BROM");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn appraiser_runs_nvattest_from_the_filesystem_root() {
        let root = TempDir::new();
        fs::write(
            root.path().join("positive.stdout"),
            fixture_bytes("nvattest/positive.stdout"),
        )
        .expect("write stdout");
        install_script(
            root.path(),
            "#!/bin/sh\n[ \"$(pwd -P)\" = / ] || exit 7\nexec cat \"$(dirname \"$0\")/../positive.stdout\"\n",
        );

        let appraisal = appraise(root.path(), HELPER_DEADLINE).expect("green appraisal");
        assert_eq!(appraisal.hwmodel, "GH100 A01 GSP BROM");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn appraiser_maps_malformed_stdout_to_gpu_appraisal_failed() {
        let root = TempDir::new();
        install_script(root.path(), "#!/bin/sh\nprintf 'not json\\n'\n");

        assert_eq!(
            appraise(root.path(), HELPER_DEADLINE),
            Err(GpuAppraisalReason::GpuAppraisalFailed)
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
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
        let profile = production_profile();
        assert_eq!(
            NvattestGpuAppraiser.appraise(
                &envelope,
                &owner_nonce(),
                root.path(),
                &GpuStatusInput {
                    profile: &profile,
                    proofs: None,
                    verification_time: std::time::SystemTime::now(),
                }
            ),
            Err(GpuAppraisalReason::NvattestUnavailable)
        );
    }
}
