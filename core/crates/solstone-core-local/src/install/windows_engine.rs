// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed Windows package binding for llama-server engine, Vulkan loader, and Vulkan probe.

use solstone_core_cli_copy::MessageDetail;
use solstone_core_installed_payload::WindowsArchitecture;
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
    pub architecture: WindowsArchitecture,
    pub package_root: PathBuf,
    pub engine: PathBuf,
    pub loader: PathBuf,
    pub probe: PathBuf,
    pub engine_sha256: String,
    pub loader_sha256: String,
    pub probe_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WindowsLlamaPackageError<T = String> {
    #[error("{0}")]
    Missing(T),
    #[error("{0}")]
    Invalid(T),
}

impl WindowsLlamaPackageError<MessageDetail> {
    fn into_source(self) -> WindowsLlamaPackageError {
        match self {
            Self::Missing(detail) => WindowsLlamaPackageError::Missing(detail.to_string()),
            Self::Invalid(detail) => WindowsLlamaPackageError::Invalid(detail.to_string()),
        }
    }
}

impl WindowsLlamaPackage {
    pub fn require_runtime_key(self, key: &str) -> Result<Self, WindowsLlamaPackageError> {
        self.require_runtime_key_detail(key)
            .map_err(WindowsLlamaPackageError::into_source)
    }

    pub(super) fn require_runtime_key_detail(
        self,
        key: &str,
    ) -> Result<Self, WindowsLlamaPackageError<MessageDetail>> {
        if self.architecture.runtime_key() != key {
            return Err(WindowsLlamaPackageError::Invalid(MessageDetail::message(
                "journal.cli.install.windows.target.different",
                [("key", &key)],
            )));
        }
        Ok(self)
    }
}

#[cfg(any(windows, test))]
pub(super) fn payload_error_detail(error: &WindowsPayloadError) -> MessageDetail {
    let id = match error.kind {
        WindowsPayloadRefusal::Missing => "journal.cli.install.windows.payload.missing",
        WindowsPayloadRefusal::NotRegular => "journal.cli.install.windows.payload.not.regular",
        WindowsPayloadRefusal::ReparseOrSymlink => {
            "journal.cli.install.windows.payload.reparse.or.symlink"
        }
        WindowsPayloadRefusal::UnsafePath => "journal.cli.install.windows.payload.unsafe.path",
        WindowsPayloadRefusal::Manifest => "journal.cli.install.windows.payload.manifest",
        WindowsPayloadRefusal::Signature => "journal.cli.install.windows.payload.signature",
        WindowsPayloadRefusal::Schema => "journal.cli.install.windows.payload.schema",
        WindowsPayloadRefusal::Target => "journal.cli.install.windows.payload.target",
        WindowsPayloadRefusal::SourceCommit => "journal.cli.install.windows.payload.source.commit",
        WindowsPayloadRefusal::LockDigest => "journal.cli.install.windows.payload.lock.digest",
        WindowsPayloadRefusal::Empty => "journal.cli.install.windows.payload.empty",
        WindowsPayloadRefusal::UnsortedOrDuplicate => {
            "journal.cli.install.windows.payload.unsorted.or.duplicate"
        }
        WindowsPayloadRefusal::CaseCollision => {
            "journal.cli.install.windows.payload.case.collision"
        }
        WindowsPayloadRefusal::Digest => "journal.cli.install.windows.payload.digest",
        WindowsPayloadRefusal::Bytes => "journal.cli.install.windows.payload.bytes",
        WindowsPayloadRefusal::MissingMember => {
            "journal.cli.install.windows.payload.missing.member"
        }
        WindowsPayloadRefusal::UnexpectedMember => {
            "journal.cli.install.windows.payload.unexpected.member"
        }
    };
    MessageDetail::message(id, [("detail", &error.detail), ("line_break", &"\n")])
}

#[cfg(any(windows, test))]
impl From<WindowsPayloadError> for WindowsLlamaPackageError<MessageDetail> {
    fn from(error: WindowsPayloadError) -> Self {
        let detail = payload_error_detail(&error);
        match error.kind {
            WindowsPayloadRefusal::Missing | WindowsPayloadRefusal::MissingMember => {
                Self::Missing(detail)
            }
            _ => Self::Invalid(detail),
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
            architecture: solstone_core_installed_payload::compiled_windows_architecture(),
            package_root: PathBuf::from("C:\\test"),
            engine: PathBuf::from("C:\\test\\lib\\solstone-native\\llama-server.exe"),
            loader: PathBuf::from("C:\\test\\lib\\solstone-native\\vulkan-1.dll"),
            probe: PathBuf::from("C:\\test\\bin\\solstone-core-vulkan-probe.exe"),
            engine_sha256: "a".repeat(64),
            loader_sha256: "b".repeat(64),
            probe_sha256: "c".repeat(64),
        }
    }
}

#[cfg(any(windows, test))]
impl From<WindowsPayloadError> for WindowsLlamaPackageError {
    fn from(error: WindowsPayloadError) -> Self {
        WindowsLlamaPackageError::<MessageDetail>::from(error).into_source()
    }
}

/// Resolve the llama-server engine and Vulkan components from the complete signed package
/// containing the running journal executable. Every invocation verifies the current bytes.
pub fn verified_windows_llama_package() -> Result<WindowsLlamaPackage, WindowsLlamaPackageError> {
    verified_windows_llama_package_detail().map_err(WindowsLlamaPackageError::into_source)
}

#[cfg(windows)]
pub(super) fn verified_windows_llama_package_detail()
-> Result<WindowsLlamaPackage, WindowsLlamaPackageError<MessageDetail>> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(pkg) = TEST_PACKAGE.with(|current| current.borrow().clone()) {
        return Ok(pkg);
    }
    let executable = std::env::current_exe().map_err(|error| {
        WindowsLlamaPackageError::Missing(MessageDetail::message(
            "journal.cli.install.windows.executable.read",
            [("error", &error)],
        ))
    })?;
    verified_windows_llama_package_at_detail(&executable)
}

#[cfg(not(windows))]
pub(super) fn verified_windows_llama_package_detail()
-> Result<WindowsLlamaPackage, WindowsLlamaPackageError<MessageDetail>> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(pkg) = TEST_PACKAGE.with(|current| current.borrow().clone()) {
        return Ok(pkg);
    }
    Err(WindowsLlamaPackageError::Missing(MessageDetail::message(
        "journal.cli.install.windows.package.runtime",
        [],
    )))
}

#[cfg(any(windows, test))]
pub fn verified_windows_llama_package_at(
    executable: &Path,
) -> Result<WindowsLlamaPackage, WindowsLlamaPackageError> {
    verified_windows_llama_package_at_detail(executable)
        .map_err(WindowsLlamaPackageError::into_source)
}

#[cfg(any(windows, test))]
fn verified_windows_llama_package_at_detail(
    executable: &Path,
) -> Result<WindowsLlamaPackage, WindowsLlamaPackageError<MessageDetail>> {
    use WindowsLlamaPackageError::{Invalid, Missing};
    let bin = executable.parent().ok_or_else(|| {
        Missing(MessageDetail::message(
            "journal.cli.install.windows.executable.parent",
            [],
        ))
    })?;
    if bin.file_name() != Some(OsStr::new("bin")) {
        return Err(Invalid(MessageDetail::message(
            "journal.cli.install.windows.executable.location",
            [("path", &executable.display())],
        )));
    }
    let package_root = bin.parent().ok_or_else(|| {
        Missing(MessageDetail::message(
            "journal.cli.install.windows.package.root",
            [],
        ))
    })?;
    let payload = verify_windows_payload(package_root)?;
    let architecture =
        WindowsArchitecture::from_target_id(&payload.manifest().target).ok_or_else(|| {
            Invalid(MessageDetail::message(
                "journal.cli.install.windows.target.unsupported",
                [],
            ))
        })?;

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
                Missing(MessageDetail::message(
                    "journal.cli.install.windows.digest.absent",
                    [("path", &path)],
                ))
            })
    };

    let engine_sha256 = find_digest(WINDOWS_LLAMA_SERVER)?;
    let loader_sha256 = find_digest(WINDOWS_VULKAN_LOADER)?;
    let probe_sha256 = find_digest(WINDOWS_VULKAN_PROBE)?;

    Ok(WindowsLlamaPackage {
        architecture,
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
