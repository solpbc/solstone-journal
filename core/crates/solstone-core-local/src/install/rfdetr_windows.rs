// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed Windows package binding for RF-DETR helper executables, runtime, and model.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

#[cfg(any(windows, test))]
use solstone_core_distribution::windows_payload::{
    WINDOWS_RFDETR_MODEL, WINDOWS_RFDETR_WORKER, WindowsPayloadError, WindowsPayloadRefusal,
    verify_windows_payload,
};
#[cfg(any(windows, test))]
use std::ffi::OsStr;

pub const RFDETR_HELP_TIMEOUT: Duration = Duration::from_secs(10);
pub const RFDETR_DETECT_TIMEOUT: Duration = Duration::from_secs(120);
pub const RFDETR_STDIN_LIMIT_BYTES: usize = 1;
pub const RFDETR_STDOUT_LIMIT_BYTES: usize = 64 * 1024;
pub const RFDETR_STDERR_LIMIT_BYTES: usize = 64 * 1024;
pub const RFDETR_RESULT_LIMIT_BYTES: u64 = 1024 * 1024;
pub const RFDETR_COMMITTED_MEMORY_BYTES: usize = 2 * 1024 * 1024 * 1024;
pub const RFDETR_CPU_RATE_PER_10_000: u32 = 10000;
pub const RFDETR_THRESHOLD: &str = "0.25";
pub const RFDETR_THREADS: &str = "4";

pub const RFDETR_PACKAGE_UNAVAILABLE_GUIDANCE: &str =
    "Object detection is unavailable. Repair or reinstall the solstone app.";

pub fn rfdetr_degraded_guidance(os_name: &str, arch: &str) -> &'static str {
    if super::rfdetr_install::rfdetr_uses_package_payload(os_name, arch) {
        RFDETR_PACKAGE_UNAVAILABLE_GUIDANCE
    } else {
        super::rfdetr_readiness::RFDETR_UNAVAILABLE_GUIDANCE
    }
}

/// The verified package locations required for Windows RF-DETR operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsRfdetrPackage {
    pub package_root: PathBuf,
    pub binary: PathBuf,
    pub model: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum WindowsRfdetrPackageError {
    #[error("{0}")]
    Missing(String),
    #[error("{0}")]
    Invalid(String),
}

#[cfg(any(windows, test))]
impl From<WindowsPayloadError> for WindowsRfdetrPackageError {
    fn from(error: WindowsPayloadError) -> Self {
        match error.kind {
            WindowsPayloadRefusal::Missing | WindowsPayloadRefusal::MissingMember => {
                Self::Missing(error.to_string())
            }
            _ => Self::Invalid(error.to_string()),
        }
    }
}

/// Resolve RF-DETR from the complete signed package containing the running
/// journal executable. Every invocation verifies the current bytes.
#[cfg(windows)]
pub fn verified_windows_rfdetr_package() -> Result<WindowsRfdetrPackage, WindowsRfdetrPackageError>
{
    let executable = std::env::current_exe().map_err(|error| {
        WindowsRfdetrPackageError::Missing(format!(
            "could not determine the running journal executable: {error}"
        ))
    })?;
    verified_windows_rfdetr_package_at(&executable)
}

#[cfg(not(windows))]
pub fn verified_windows_rfdetr_package() -> Result<WindowsRfdetrPackage, WindowsRfdetrPackageError>
{
    Err(WindowsRfdetrPackageError::Missing(
        "Windows RF-DETR package verification requires a Windows runtime".to_owned(),
    ))
}

#[cfg(any(windows, test))]
fn verified_windows_rfdetr_package_at(
    executable: &Path,
) -> Result<WindowsRfdetrPackage, WindowsRfdetrPackageError> {
    use WindowsRfdetrPackageError::{Invalid, Missing};
    let bin = executable.parent().ok_or_else(|| {
        Missing("running journal executable has no containing directory".to_owned())
    })?;
    if bin.file_name() != Some(OsStr::new("bin")) {
        return Err(Invalid(format!(
            "running journal executable is not in the package bin directory: {}",
            executable.display()
        )));
    }
    let package_root = bin
        .parent()
        .ok_or_else(|| Missing("package bin directory has no package root".to_owned()))?;
    let payload = verify_windows_payload(package_root)?;
    let member = |path: &str| {
        payload.declared_path(path).ok_or_else(|| {
            Missing(format!(
                "signed RF-DETR app payload does not declare {path}"
            ))
        })
    };
    Ok(WindowsRfdetrPackage {
        package_root: package_root.to_path_buf(),
        binary: member(WINDOWS_RFDETR_WORKER)?,
        model: member(WINDOWS_RFDETR_MODEL)?,
    })
}

/// Host-independent launch specification for Windows bounded helper invocations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RfdetrWindowsLaunchSpec {
    pub package_root: PathBuf,
    pub executable: PathBuf,
    pub current_directory: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<OsString, OsString>,
    pub stdin: Vec<u8>,
    pub timeout: Duration,
    pub stdin_limit_bytes: usize,
    pub stdout_limit_bytes: usize,
    pub stderr_limit_bytes: usize,
    pub cpu_rate_per_10_000: u32,
    pub committed_memory_bytes: usize,
}

pub fn rfdetr_windows_help_launch(
    package: &WindowsRfdetrPackage,
    system_root: OsString,
) -> RfdetrWindowsLaunchSpec {
    let current_directory = package.package_root.join("bin");
    RfdetrWindowsLaunchSpec {
        package_root: package.package_root.clone(),
        executable: package.binary.clone(),
        current_directory,
        arguments: vec!["--help".to_owned()],
        environment: BTreeMap::from([(OsString::from("SystemRoot"), system_root)]),
        stdin: Vec::new(),
        timeout: RFDETR_HELP_TIMEOUT,
        stdin_limit_bytes: RFDETR_STDIN_LIMIT_BYTES,
        stdout_limit_bytes: RFDETR_STDOUT_LIMIT_BYTES,
        stderr_limit_bytes: RFDETR_STDERR_LIMIT_BYTES,
        cpu_rate_per_10_000: RFDETR_CPU_RATE_PER_10_000,
        committed_memory_bytes: RFDETR_COMMITTED_MEMORY_BYTES,
    }
}

pub fn rfdetr_windows_detect_launch(
    package: &WindowsRfdetrPackage,
    system_root: OsString,
    input: &Path,
    output: &Path,
) -> RfdetrWindowsLaunchSpec {
    let current_directory = package.package_root.join("bin");
    RfdetrWindowsLaunchSpec {
        package_root: package.package_root.clone(),
        executable: package.binary.clone(),
        current_directory,
        arguments: vec![
            "detect".to_owned(),
            "--model".to_owned(),
            package.model.to_string_lossy().into_owned(),
            "--input".to_owned(),
            input.to_string_lossy().into_owned(),
            "--output".to_owned(),
            output.to_string_lossy().into_owned(),
            "--threshold".to_owned(),
            RFDETR_THRESHOLD.to_owned(),
            "--threads".to_owned(),
            RFDETR_THREADS.to_owned(),
        ],
        environment: BTreeMap::from([(OsString::from("SystemRoot"), system_root)]),
        stdin: Vec::new(),
        timeout: RFDETR_DETECT_TIMEOUT,
        stdin_limit_bytes: RFDETR_STDIN_LIMIT_BYTES,
        stdout_limit_bytes: RFDETR_STDOUT_LIMIT_BYTES,
        stderr_limit_bytes: RFDETR_STDERR_LIMIT_BYTES,
        cpu_rate_per_10_000: RFDETR_CPU_RATE_PER_10_000,
        committed_memory_bytes: RFDETR_COMMITTED_MEMORY_BYTES,
    }
}

pub fn map_rfdetr_detect_completion(exit_ok: bool, output_path: &Path) -> Result<Value, String> {
    if !exit_ok {
        return Err("rfdetr-cli detect failed".to_owned());
    }
    let file = std::fs::File::open(output_path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(RFDETR_RESULT_LIMIT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > RFDETR_RESULT_LIMIT_BYTES {
        return Err("RF-DETR result exceeded its byte limit".to_owned());
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_refuses_wrong_layout_and_missing_manifest() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            verified_windows_rfdetr_package_at(&root.path().join("journal.exe")),
            Err(WindowsRfdetrPackageError::Invalid(_))
        ));
        std::fs::create_dir(root.path().join("bin")).unwrap();
        assert!(matches!(
            verified_windows_rfdetr_package_at(&root.path().join("bin/journal.exe")),
            Err(WindowsRfdetrPackageError::Missing(_))
        ));
    }

    #[test]
    fn detection_result_is_bounded_and_child_failure_precedes_file_read() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("output.json");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(RFDETR_RESULT_LIMIT_BYTES + 1).unwrap();
        assert!(
            map_rfdetr_detect_completion(true, &path)
                .unwrap_err()
                .contains("byte limit")
        );
        assert_eq!(
            map_rfdetr_detect_completion(false, &path).unwrap_err(),
            "rfdetr-cli detect failed"
        );
        std::fs::write(&path, b"{bad").unwrap();
        assert!(map_rfdetr_detect_completion(true, &path).is_err());
        std::fs::write(&path, br#"{"image":{},"detections":[]}"#).unwrap();
        assert_eq!(
            map_rfdetr_detect_completion(true, &path).unwrap()["detections"],
            serde_json::json!([])
        );
    }

    #[cfg(feature = "test-fixture-pin")]
    #[test]
    fn signed_package_readiness_rechecks_tamper_before_probe() {
        use super::super::rfdetr_readiness::{
            RfdetrDegradedCause, RfdetrReadiness, evaluate_windows_rfdetr_readiness_from,
        };
        use solstone_core_distribution::manifest_verify::install_test_fixture_pin;
        use solstone_core_distribution::windows_payload::{
            WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE, render_windows_payload_manifest,
        };
        use std::cell::Cell;
        use std::io::Cursor;
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("bin/journal.exe");
        for (path, bytes) in [
            ("bin/journal.exe", b"journal".as_slice()),
            (WINDOWS_RFDETR_WORKER, b"engine".as_slice()),
            (WINDOWS_RFDETR_MODEL, b"model".as_slice()),
        ] {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let manifest =
            render_windows_payload_manifest(root.path(), &"a".repeat(40), &"b".repeat(64)).unwrap();
        let minisign::KeyPair { pk, sk } =
            minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pin_dir = tempfile::tempdir().unwrap();
        let pin = pin_dir.path().join("fixture.pub");
        std::fs::write(&pin, pk.to_box().unwrap().to_bytes()).unwrap();
        install_test_fixture_pin(&pin).unwrap();
        let signature = minisign::sign(
            Some(&pk),
            &sk,
            Cursor::new(manifest.as_slice()),
            None,
            Some("RF-DETR consumer fixture"),
        )
        .unwrap();
        let manifest_path = root.path().join(WINDOWS_PAYLOAD_MANIFEST);
        std::fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        std::fs::write(&manifest_path, manifest).unwrap();
        std::fs::write(
            root.path().join(WINDOWS_PAYLOAD_SIGNATURE),
            signature.into_string(),
        )
        .unwrap();
        let calls = Cell::new(0);
        let evaluate = || {
            evaluate_windows_rfdetr_readiness_from(
                verified_windows_rfdetr_package_at(&executable),
                |package| {
                    calls.set(calls.get() + 1);
                    let spec = rfdetr_windows_help_launch(package, OsString::from("system-root"));
                    assert_eq!(spec.executable, root.path().join(WINDOWS_RFDETR_WORKER));
                    assert_eq!(spec.current_directory, root.path().join("bin"));
                    assert_eq!(spec.arguments, ["--help"]);
                    serde_json::json!({"runnable": true})
                },
            )
        };
        assert!(matches!(evaluate(), RfdetrReadiness::Ready { .. }));
        std::fs::write(root.path().join(WINDOWS_RFDETR_MODEL), b"wrong").unwrap();
        assert!(matches!(
            evaluate(),
            RfdetrReadiness::Degraded {
                cause: RfdetrDegradedCause::IntegrityInvalid,
                ..
            }
        ));
        assert_eq!(calls.get(), 1);
        std::fs::write(root.path().join(WINDOWS_RFDETR_MODEL), b"model").unwrap();
        assert!(matches!(evaluate(), RfdetrReadiness::Ready { .. }));
        std::fs::remove_file(root.path().join(WINDOWS_RFDETR_WORKER)).unwrap();
        assert!(matches!(
            evaluate(),
            RfdetrReadiness::Degraded {
                cause: RfdetrDegradedCause::Absent,
                ..
            }
        ));
        assert_eq!(calls.get(), 2);
    }
}
