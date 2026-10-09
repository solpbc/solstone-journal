// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed Windows package binding for llama-server engine, Vulkan loader, and Vulkan probe.

#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(any(windows, test))]
use solstone_core_distribution::windows_payload::{
    WINDOWS_LLAMA_SERVER, WINDOWS_VULKAN_LOADER, WINDOWS_VULKAN_PROBE, WindowsPayloadError,
    WindowsPayloadRefusal, verify_windows_payload,
};
#[cfg(any(windows, test))]
use std::ffi::OsStr;

/// The verified package locations and digests required for Windows local inference operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsLlamaPackage {
    pub package_root: PathBuf,
    pub engine: PathBuf,
    pub loader: PathBuf,
    pub probe: PathBuf,
    pub engine_sha256: String,
    pub loader_sha256: String,
    pub probe_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WindowsLlamaPackageError {
    #[error("{0}")]
    Missing(String),
    #[error("{0}")]
    Invalid(String),
}

#[cfg(any(windows, test))]
impl From<WindowsPayloadError> for WindowsLlamaPackageError {
    fn from(error: WindowsPayloadError) -> Self {
        match error.kind {
            WindowsPayloadRefusal::Missing | WindowsPayloadRefusal::MissingMember => {
                Self::Missing(error.to_string())
            }
            _ => Self::Invalid(error.to_string()),
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static TEST_PACKAGE: std::cell::RefCell<Option<WindowsLlamaPackage>> = const { std::cell::RefCell::new(None) };
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn set_test_windows_llama_package(pkg: Option<WindowsLlamaPackage>) {
    TEST_PACKAGE.with(|current| *current.borrow_mut() = pkg);
}

#[cfg(any(test, feature = "test-hooks"))]
impl WindowsLlamaPackage {
    pub fn mock() -> Self {
        Self {
            package_root: PathBuf::from("C:\\test"),
            engine: PathBuf::from("C:\\test\\bin\\llama-server.exe"),
            loader: PathBuf::from("C:\\test\\bin\\vulkan-1.dll"),
            probe: PathBuf::from("C:\\test\\bin\\solstone-core-vulkan-probe.exe"),
            engine_sha256: "a".repeat(64),
            loader_sha256: "b".repeat(64),
            probe_sha256: "c".repeat(64),
        }
    }
}

/// Resolve the llama-server engine and Vulkan components from the complete signed package
/// containing the running journal executable. Every invocation verifies the current bytes.
#[cfg(windows)]
pub fn verified_windows_llama_package() -> Result<WindowsLlamaPackage, WindowsLlamaPackageError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(pkg) = TEST_PACKAGE.with(|current| current.borrow().clone()) {
        return Ok(pkg);
    }
    let executable = std::env::current_exe().map_err(|error| {
        WindowsLlamaPackageError::Missing(format!(
            "could not determine the running journal executable: {error}"
        ))
    })?;
    verified_windows_llama_package_at(&executable)
}

#[cfg(not(windows))]
pub fn verified_windows_llama_package() -> Result<WindowsLlamaPackage, WindowsLlamaPackageError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(pkg) = TEST_PACKAGE.with(|current| current.borrow().clone()) {
        return Ok(pkg);
    }
    Err(WindowsLlamaPackageError::Missing(
        "Windows local thinking package verification requires a Windows runtime".to_owned(),
    ))
}

#[cfg(any(windows, test))]
pub fn verified_windows_llama_package_at(
    executable: &Path,
) -> Result<WindowsLlamaPackage, WindowsLlamaPackageError> {
    use WindowsLlamaPackageError::{Invalid, Missing};
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

    let engine = payload.llama_server_path()?;
    let loader = payload.vulkan_loader_path()?;
    let probe = payload.vulkan_probe_path()?;

    let find_digest = |path: &str| {
        payload
            .manifest()
            .files
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.sha256.clone())
            .ok_or_else(|| {
                Missing(format!(
                    "signed payload manifest does not declare digest for {path}"
                ))
            })
    };

    let engine_sha256 = find_digest(WINDOWS_LLAMA_SERVER)?;
    let loader_sha256 = find_digest(WINDOWS_VULKAN_LOADER)?;
    let probe_sha256 = find_digest(WINDOWS_VULKAN_PROBE)?;

    Ok(WindowsLlamaPackage {
        package_root: package_root.to_path_buf(),
        engine,
        loader,
        probe,
        engine_sha256,
        loader_sha256,
        probe_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_refuses_wrong_layout_and_missing_manifest() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            verified_windows_llama_package_at(&root.path().join("journal.exe")),
            Err(WindowsLlamaPackageError::Invalid(_))
        ));
        std::fs::create_dir(root.path().join("bin")).unwrap();
        assert!(matches!(
            verified_windows_llama_package_at(&root.path().join("bin/journal.exe")),
            Err(WindowsLlamaPackageError::Missing(_))
        ));
    }

    #[cfg(all(feature = "full-tests", feature = "test-fixture-pin"))]
    #[test]
    fn signed_package_readiness_rechecks_tamper_before_probe() {
        use solstone_core_distribution::windows_payload::{
            WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE, render_windows_payload_manifest,
        };
        use std::io::Cursor;
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("bin/journal.exe");
        for (path, bytes) in [
            ("bin/journal.exe", b"journal".as_slice()),
            (WINDOWS_LLAMA_SERVER, b"engine".as_slice()),
            (WINDOWS_VULKAN_LOADER, b"loader".as_slice()),
            (WINDOWS_VULKAN_PROBE, b"probe".as_slice()),
        ] {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let manifest =
            render_windows_payload_manifest(root.path(), &"a".repeat(40), &"b".repeat(64)).unwrap();
        let minisign::KeyPair { pk, sk } = super::super::windows_payload_test_keys();
        let signature = minisign::sign(
            Some(pk),
            sk,
            Cursor::new(manifest.as_slice()),
            None,
            Some("Llama consumer fixture"),
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

        assert!(verified_windows_llama_package_at(&executable).is_ok());

        // Tamper with engine
        std::fs::write(root.path().join(WINDOWS_LLAMA_SERVER), b"tampered").unwrap();
        assert!(matches!(
            verified_windows_llama_package_at(&executable),
            Err(WindowsLlamaPackageError::Invalid(_))
        ));

        // Restore engine
        std::fs::write(root.path().join(WINDOWS_LLAMA_SERVER), b"engine").unwrap();
        assert!(verified_windows_llama_package_at(&executable).is_ok());

        // Remove probe
        std::fs::remove_file(root.path().join(WINDOWS_VULKAN_PROBE)).unwrap();
        assert!(matches!(
            verified_windows_llama_package_at(&executable),
            Err(WindowsLlamaPackageError::Missing(_))
        ));
    }
}
