// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host verdict for the pinned ced.cpp sound-tagging assets.

use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::ffi::OsStr;

use serde_json::{Value, json};
#[cfg(any(windows, test))]
use solstone_core_installed_payload::guidance;
#[cfg(windows)]
use solstone_core_installed_payload::windows_payload::{
    WINDOWS_CED_LIBRARY, WINDOWS_CED_MODEL, WINDOWS_CED_WORKER, WindowsPayloadRefusal,
    verify_windows_payload,
};
use solstone_core_installed_payload::{
    COMPILED_VERSION, InstalledPackage, InstalledPayloadRefusal, code, host_executable_platform,
    locate_installed_package,
};

use super::capability_status::CapabilityStatus;
use super::ced_runtime::{
    CED_ANALYZE_TIMEOUT, CED_PROBE_COMMAND, CedAnalyzeError, CedAnalyzeProgram,
    invoke_ced_analyze_with_args,
};

/// Short ready detail for `journal check` and `journal health`.
pub const CED_READY_DETAIL: &str = "ced.cpp sound-tag engine and model are ready";

/// Capability identifier carried on every CED-constructed non-ready status.
pub const CED_CAPABILITY: &str = "ced";

pub const POSIX_CED_MODEL: &str = "lib/solstone-ced/ced-tiny-q8_0.gguf";
pub const LINUX_CED_LIBRARY: &str = "lib/solstone-ced/libced.so";
pub const MACOS_CED_LIBRARY: &str = "lib/solstone-ced/libced.dylib";

/// Result of probing CED assets on a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CedVerdict {
    Ready { library: PathBuf, model: PathBuf },
    Unsupported { os: String, arch: String },
    Degraded(CapabilityStatus),
}

/// Named signed members for one CED helper invocation on Windows.
#[derive(Debug, Clone)]
pub struct WindowsCedPackage {
    pub root: PathBuf,
    pub helper: PathBuf,
    pub library: PathBuf,
    pub model: PathBuf,
}

#[cfg(windows)]
pub fn verified_windows_ced_package() -> Result<WindowsCedPackage, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let bin = executable
        .parent()
        .ok_or("running executable has no containing directory")?;
    if bin.file_name() != Some(OsStr::new("bin")) {
        return Err("running executable is not in the package bin directory".to_owned());
    }
    let root = bin
        .parent()
        .ok_or("package bin directory has no package root")?;
    let payload = verify_windows_payload(root).map_err(|error| error.to_string())?;
    let member = |path| {
        payload
            .declared_path(path)
            .map_err(|error| format!("signed CED app payload refuses {path}: {error}"))
    };
    Ok(WindowsCedPackage {
        root: root.to_path_buf(),
        helper: member(WINDOWS_CED_WORKER)?,
        library: member(WINDOWS_CED_LIBRARY)?,
        model: member(WINDOWS_CED_MODEL)?,
    })
}

#[cfg(not(windows))]
pub fn verified_windows_ced_package() -> Result<WindowsCedPackage, String> {
    Err("Windows CED package verification requires a Windows runtime".to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PosixReason {
    RestartToFinishUpdate,
    ManifestMissing,
    ManifestInvalid,
    WrongProduct,
    WrongTarget,
    UnexpectedFile,
    MemberUnreadable,
    UnsupportedLocation,
    MemberMissing,
    MemberChanged,
    UnsafePath,
}

impl PosixReason {
    fn from_code(c: &str) -> Self {
        match c {
            code::RESTART_TO_FINISH_UPDATE => PosixReason::RestartToFinishUpdate,
            code::MANIFEST_MISSING => PosixReason::ManifestMissing,
            code::MANIFEST_INVALID => PosixReason::ManifestInvalid,
            code::WRONG_PRODUCT => PosixReason::WrongProduct,
            code::WRONG_TARGET => PosixReason::WrongTarget,
            code::UNEXPECTED_FILE => PosixReason::UnexpectedFile,
            code::MEMBER_UNREADABLE => PosixReason::MemberUnreadable,
            code::UNSUPPORTED_LOCATION => PosixReason::UnsupportedLocation,
            code::MEMBER_MISSING => PosixReason::MemberMissing,
            code::MEMBER_CHANGED => PosixReason::MemberChanged,
            code::UNSAFE_PATH => PosixReason::UnsafePath,
            other => panic!("unknown installed payload refusal code: {other}"),
        }
    }
}

pub fn map_posix_refusal(refusal: InstalledPayloadRefusal) -> CapabilityStatus {
    let reason = PosixReason::from_code(refusal.code);
    let detail = format!("{refusal} {}", refusal.guidance);
    match reason {
        PosixReason::RestartToFinishUpdate | PosixReason::ManifestMissing => {
            CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            }
        }
        PosixReason::ManifestInvalid | PosixReason::UnexpectedFile => {
            CapabilityStatus::IntegrityInvalid {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            }
        }
        PosixReason::WrongProduct | PosixReason::WrongTarget => {
            CapabilityStatus::WrongAbiOrProtocol {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            }
        }
        PosixReason::MemberUnreadable | PosixReason::UnsupportedLocation => {
            CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            }
        }
        PosixReason::MemberMissing => CapabilityStatus::Absent {
            capability: CED_CAPABILITY.to_owned(),
            detail,
        },
        PosixReason::MemberChanged | PosixReason::UnsafePath => {
            CapabilityStatus::IntegrityInvalid {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            }
        }
    }
}

#[cfg(any(windows, test))]
pub fn map_windows_refusal(
    error: solstone_core_installed_payload::windows_payload::WindowsPayloadError,
) -> CapabilityStatus {
    let kind = error.kind;
    let guidance_text = match kind {
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::MissingMember => {
            guidance::PACKAGE_MISMATCH
        }
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Digest
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Bytes => {
            guidance::PACKAGE_MISMATCH
        }
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::UnexpectedMember => {
            guidance::PACKAGE_MISMATCH
        }
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::UnsafePath
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::ReparseOrSymlink => {
            guidance::UNSAFE_PATH
        }
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Target => {
            guidance::WRONG_TARGET
        }
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Missing => {
            guidance::MANIFEST_MISSING
        }
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Manifest
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Signature
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Schema
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::SourceCommit
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::LockDigest
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Empty
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::UnsortedOrDuplicate
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::CaseCollision
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::NotRegular => {
            guidance::MANIFEST_INVALID
        }
    };
    let detail = format!("{}: {} {}", kind.as_str(), error.detail, guidance_text);
    match kind {
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::MissingMember => {
            CapabilityStatus::Absent {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            }
        }
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Digest
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Bytes
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::UnexpectedMember
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::UnsafePath
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::ReparseOrSymlink => {
            CapabilityStatus::IntegrityInvalid {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            }
        }
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Target => {
            CapabilityStatus::WrongAbiOrProtocol {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            }
        }
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Missing => {
            CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            }
        }
        solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Manifest
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Signature
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Schema
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::SourceCommit
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::LockDigest
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::Empty
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::UnsortedOrDuplicate
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::CaseCollision
        | solstone_core_installed_payload::windows_payload::WindowsPayloadRefusal::NotRegular => {
            CapabilityStatus::IntegrityInvalid {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            }
        }
    }
}

pub fn ced_target_and_library(os: &str, arch: &str) -> Option<(&'static str, &'static str)> {
    match (os, arch) {
        ("linux", "x86_64") => Some((
            solstone_core_installed_payload::TARGET_LINUX_X86_64,
            LINUX_CED_LIBRARY,
        )),
        ("linux", "aarch64") | ("linux", "arm64") => Some((
            solstone_core_installed_payload::TARGET_LINUX_AARCH64,
            LINUX_CED_LIBRARY,
        )),
        ("darwin", "arm64") | ("darwin", "aarch64") | ("macos", "arm64") | ("macos", "aarch64") => {
            Some((
                solstone_core_installed_payload::TARGET_MACOS_ARM64,
                MACOS_CED_LIBRARY,
            ))
        }
        ("windows", "x86_64") => Some((solstone_core_installed_payload::TARGET_WINDOWS_X86_64, "")),
        _ => None,
    }
}

#[allow(dead_code)]
enum ResolvedMembers {
    Posix { library: PathBuf, model: PathBuf },
    Windows { library: PathBuf, model: PathBuf },
}

fn resolve_in_package_or_exe(
    package_root_or_exe: &Path,
    os: &str,
    arch: &str,
) -> Result<ResolvedMembers, CapabilityStatus> {
    let (target, lib_rel) = match ced_target_and_library(os, arch) {
        Some(t) => t,
        None => unreachable!("caller checks supported"),
    };

    if os == "windows" {
        #[cfg(windows)]
        {
            let root = if package_root_or_exe.is_file() {
                package_root_or_exe
                    .parent()
                    .and_then(|bin| bin.parent())
                    .unwrap_or(package_root_or_exe)
            } else {
                package_root_or_exe
            };
            let payload = verify_windows_payload(root).map_err(map_windows_refusal)?;
            let library = payload
                .declared_path(WINDOWS_CED_LIBRARY)
                .map_err(map_windows_refusal)?;
            let model = payload
                .declared_path(WINDOWS_CED_MODEL)
                .map_err(map_windows_refusal)?;
            Ok(ResolvedMembers::Windows { library, model })
        }
        #[cfg(not(windows))]
        {
            let _ = package_root_or_exe;
            Err(CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                capability: CED_CAPABILITY.to_owned(),
                detail: "Windows CED package verification requires a Windows runtime".to_owned(),
            })
        }
    } else {
        let root = if package_root_or_exe.is_file() {
            locate_installed_package(package_root_or_exe, host_executable_platform())
                .map_err(map_posix_refusal)?
        } else {
            package_root_or_exe.to_path_buf()
        };
        let package =
            InstalledPackage::admit(&root, COMPILED_VERSION, target).map_err(map_posix_refusal)?;
        let library = package.member(lib_rel).map_err(map_posix_refusal)?;
        let model = package.member(POSIX_CED_MODEL).map_err(map_posix_refusal)?;
        Ok(ResolvedMembers::Posix { library, model })
    }
}

fn resolve_in_current_exe(os: &str, arch: &str) -> Result<ResolvedMembers, CapabilityStatus> {
    let current_exe = std::env::current_exe().map_err(|err| {
        CapabilityStatus::ResourceOrOwnerScopeUnavailable {
            capability: CED_CAPABILITY.to_owned(),
            detail: err.to_string(),
        }
    })?;
    resolve_in_package_or_exe(&current_exe, os, arch)
}

/// Production verdict: admits from `current_exe()`.
pub fn evaluate_ced_readiness(_journal: &Path, os: &str, arch: &str) -> CedVerdict {
    evaluate_ced_readiness_with_probe(_journal, os, arch, |library, model| {
        probe_ced_engine(&CedAnalyzeProgram::SiblingHelper, library, model)
    })
}

/// Production verdict with caller-supplied probe.
pub fn evaluate_ced_readiness_with_probe(
    _journal: &Path,
    os: &str,
    arch: &str,
    probe: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> CedVerdict {
    if ced_target_and_library(os, arch).is_none() {
        return CedVerdict::Unsupported {
            os: os.to_owned(),
            arch: arch.to_owned(),
        };
    }
    match resolve_in_current_exe(os, arch) {
        Err(status) => CedVerdict::Degraded(status),
        Ok(
            ResolvedMembers::Posix { library, model } | ResolvedMembers::Windows { library, model },
        ) => {
            if let Err(detail) = probe(&library, &model) {
                CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable {
                    capability: CED_CAPABILITY.to_owned(),
                    detail,
                })
            } else {
                CedVerdict::Ready { library, model }
            }
        }
    }
}

/// Package-based verdict for tests or explicit package paths.
pub fn evaluate_ced_readiness_in_package(
    package_root_or_exe: &Path,
    os: &str,
    arch: &str,
) -> CedVerdict {
    evaluate_ced_readiness_in_package_with_probe(package_root_or_exe, os, arch, |library, model| {
        probe_ced_engine(&CedAnalyzeProgram::SiblingHelper, library, model)
    })
}

/// Package-based verdict with caller-supplied probe.
pub fn evaluate_ced_readiness_in_package_with_probe(
    package_root_or_exe: &Path,
    os: &str,
    arch: &str,
    probe: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> CedVerdict {
    if ced_target_and_library(os, arch).is_none() {
        return CedVerdict::Unsupported {
            os: os.to_owned(),
            arch: arch.to_owned(),
        };
    }
    match resolve_in_package_or_exe(package_root_or_exe, os, arch) {
        Err(status) => CedVerdict::Degraded(status),
        Ok(
            ResolvedMembers::Posix { library, model } | ResolvedMembers::Windows { library, model },
        ) => {
            if let Err(detail) = probe(&library, &model) {
                CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable {
                    capability: CED_CAPABILITY.to_owned(),
                    detail,
                })
            } else {
                CedVerdict::Ready { library, model }
            }
        }
    }
}

/// Fresh model resolution immediately before spawn.
pub fn fresh_model_check(os: &str, arch: &str) -> Result<PathBuf, CapabilityStatus> {
    let current_exe = std::env::current_exe().map_err(|err| {
        CapabilityStatus::ResourceOrOwnerScopeUnavailable {
            capability: CED_CAPABILITY.to_owned(),
            detail: err.to_string(),
        }
    })?;
    fresh_model_check_in_package(&current_exe, os, arch)
}

/// Fresh model resolution in explicit package root or executable.
pub fn fresh_model_check_in_package(
    package_root_or_exe: &Path,
    os: &str,
    arch: &str,
) -> Result<PathBuf, CapabilityStatus> {
    let (target, _) = ced_target_and_library(os, arch).ok_or_else(|| {
        CapabilityStatus::ResourceOrOwnerScopeUnavailable {
            capability: CED_CAPABILITY.to_owned(),
            detail: format!("unsupported CED platform {os}/{arch}"),
        }
    })?;

    if os == "windows" {
        #[cfg(windows)]
        {
            let root = if package_root_or_exe.is_file() {
                package_root_or_exe
                    .parent()
                    .and_then(|bin| bin.parent())
                    .unwrap_or(package_root_or_exe)
            } else {
                package_root_or_exe
            };
            let payload = verify_windows_payload(root).map_err(map_windows_refusal)?;
            payload
                .declared_path(WINDOWS_CED_MODEL)
                .map_err(map_windows_refusal)
        }
        #[cfg(not(windows))]
        {
            let _ = package_root_or_exe;
            Err(CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                capability: CED_CAPABILITY.to_owned(),
                detail: "Windows CED package verification requires a Windows runtime".to_owned(),
            })
        }
    } else {
        let root = if package_root_or_exe.is_file() {
            locate_installed_package(package_root_or_exe, host_executable_platform())
                .map_err(map_posix_refusal)?
        } else {
            package_root_or_exe.to_path_buf()
        };
        let package =
            InstalledPackage::admit(&root, COMPILED_VERSION, target).map_err(map_posix_refusal)?;
        package.member(POSIX_CED_MODEL).map_err(map_posix_refusal)
    }
}

const PROBE_REQUEST_SCHEMA: &str = "solstone-ced-probe-request-v1";
const HELPER_ERROR_SCHEMA: &str = "solstone-ced-error-v1";

pub fn probe_ced_engine(
    program: &CedAnalyzeProgram,
    library: &Path,
    model: &Path,
) -> Result<(), String> {
    let request = json!({
        "schema": PROBE_REQUEST_SCHEMA,
        "models": {
            "ced_library_path": library,
            "ced_model_path": model,
        },
    });
    match invoke_ced_analyze_with_args(program, &[CED_PROBE_COMMAND], &request, CED_ANALYZE_TIMEOUT)
    {
        Ok(response) if response.get("ok") == Some(&Value::Bool(true)) => Ok(()),
        Ok(response) => Err(format!(
            "ced probe helper returned an unexpected response: {response}"
        )),
        Err(CedAnalyzeError::Exit { stderr, code }) => Err(helper_error_detail(&stderr)
            .unwrap_or_else(|| format!("ced probe helper exited {code:?}: {stderr}"))),
        Err(error) => Err(error.to_string()),
    }
}

fn helper_error_detail(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        (value.get("schema").and_then(Value::as_str) == Some(HELPER_ERROR_SCHEMA))
            .then(|| {
                value
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_installed_payload::render_installed_payload;
    use std::fs;

    fn write_rendered_manifest(root: &Path, target: &str) {
        let manifest_bytes = render_installed_payload(
            root,
            solstone_core_installed_payload::PRODUCT,
            COMPILED_VERSION,
            target,
            "commit_aaa",
        )
        .unwrap();
        let manifest_path = root.join(solstone_core_installed_payload::INSTALLED_PAYLOAD_MANIFEST);
        fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        fs::write(manifest_path, manifest_bytes).unwrap();
    }

    #[test]
    fn unsupported_platform_is_unsupported() {
        let journal = tempfile::tempdir().unwrap();
        match evaluate_ced_readiness(journal.path(), "freebsd", "x86_64") {
            CedVerdict::Unsupported { os, arch } => {
                assert_eq!(os, "freebsd");
                assert_eq!(arch, "x86_64");
            }
            other => panic!("expected unsupported, got {other:?}"),
        }
        match evaluate_ced_readiness(journal.path(), "linux", "mips") {
            CedVerdict::Unsupported { os, arch } => {
                assert_eq!(os, "linux");
                assert_eq!(arch, "mips");
            }
            other => panic!("expected unsupported, got {other:?}"),
        }
    }

    #[test]
    fn checkout_on_supported_platform_is_degraded_unsupported_location() {
        let journal = tempfile::tempdir().unwrap();
        match evaluate_ced_readiness(journal.path(), "linux", "x86_64") {
            CedVerdict::Degraded(CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                detail,
                ..
            }) => {
                assert!(
                    detail.contains(guidance::UNSUPPORTED_LOCATION),
                    "detail must contain unsupported-location guidance: {detail}"
                );
                assert!(
                    !detail.contains("reinstall"),
                    "guidance must not say reinstall: {detail}"
                );
                assert!(
                    !detail.contains("install-models"),
                    "guidance must not say install-models: {detail}"
                );
            }
            other => panic!("expected unsupported location degraded, got {other:?}"),
        }
    }

    #[test]
    fn posix_refusals_map_faithfully_to_capability_status() {
        // Table row checks:
        // 1. deleted executable with no manifest -> ResourceOrOwnerScopeUnavailable, RESTART_UPDATE
        let ref_deleted = InstalledPayloadRefusal {
            code: code::MANIFEST_MISSING,
            guidance: guidance::RESTART_UPDATE,
            path: Some("/opt/solstone/bin/solstone (deleted)".to_owned()),
            versions: None,
            targets: None,
            io: None,
        };
        match map_posix_refusal(ref_deleted) {
            CapabilityStatus::ResourceOrOwnerScopeUnavailable { detail, .. } => {
                assert!(detail.contains(guidance::RESTART_UPDATE));
                assert!(detail.contains(code::MANIFEST_MISSING));
            }
            other => panic!("expected ResourceOrOwnerScopeUnavailable, got {other:?}"),
        }

        // 2. wrong product -> WrongAbiOrProtocol
        let ref_prod = InstalledPayloadRefusal {
            code: code::WRONG_PRODUCT,
            guidance: guidance::WRONG_PRODUCT,
            path: None,
            versions: None,
            targets: None,
            io: None,
        };
        match map_posix_refusal(ref_prod) {
            CapabilityStatus::WrongAbiOrProtocol { detail, .. } => {
                assert!(detail.contains(guidance::WRONG_PRODUCT));
                assert!(detail.contains(code::WRONG_PRODUCT));
            }
            other => panic!("expected WrongAbiOrProtocol, got {other:?}"),
        }

        // 3. wrong target -> WrongAbiOrProtocol
        let ref_target = InstalledPayloadRefusal {
            code: code::WRONG_TARGET,
            guidance: guidance::WRONG_TARGET,
            path: None,
            versions: None,
            targets: None,
            io: None,
        };
        match map_posix_refusal(ref_target) {
            CapabilityStatus::WrongAbiOrProtocol { detail, .. } => {
                assert!(detail.contains(guidance::WRONG_TARGET));
            }
            other => panic!("expected WrongAbiOrProtocol, got {other:?}"),
        }

        // 4. member missing -> Absent, PACKAGE_MISMATCH
        let ref_missing = InstalledPayloadRefusal {
            code: code::MEMBER_MISSING,
            guidance: guidance::PACKAGE_MISMATCH,
            path: Some(LINUX_CED_LIBRARY.to_owned()),
            versions: None,
            targets: None,
            io: None,
        };
        match map_posix_refusal(ref_missing) {
            CapabilityStatus::Absent { detail, .. } => {
                assert!(detail.contains(guidance::PACKAGE_MISMATCH));
                assert!(detail.contains(code::MEMBER_MISSING));
            }
            other => panic!("expected Absent, got {other:?}"),
        }

        // 5. member changed -> IntegrityInvalid, PACKAGE_MISMATCH
        let ref_changed = InstalledPayloadRefusal {
            code: code::MEMBER_CHANGED,
            guidance: guidance::PACKAGE_MISMATCH,
            path: Some(POSIX_CED_MODEL.to_owned()),
            versions: None,
            targets: None,
            io: None,
        };
        match map_posix_refusal(ref_changed) {
            CapabilityStatus::IntegrityInvalid { detail, .. } => {
                assert!(detail.contains(guidance::PACKAGE_MISMATCH));
                assert!(detail.contains(code::MEMBER_CHANGED));
                assert!(!detail.contains("tamper"));
            }
            other => panic!("expected IntegrityInvalid, got {other:?}"),
        }

        // 6. unsafe path -> IntegrityInvalid, UNSAFE_PATH
        let ref_unsafe = InstalledPayloadRefusal {
            code: code::UNSAFE_PATH,
            guidance: guidance::UNSAFE_PATH,
            path: Some("lib/evil".to_owned()),
            versions: None,
            targets: None,
            io: None,
        };
        match map_posix_refusal(ref_unsafe) {
            CapabilityStatus::IntegrityInvalid { detail, .. } => {
                assert!(detail.contains(guidance::UNSAFE_PATH));
                assert!(detail.contains(code::UNSAFE_PATH));
            }
            other => panic!("expected IntegrityInvalid, got {other:?}"),
        }

        // 7. manifest invalid -> IntegrityInvalid, MANIFEST_INVALID
        let ref_minv = InstalledPayloadRefusal {
            code: code::MANIFEST_INVALID,
            guidance: guidance::MANIFEST_INVALID,
            path: None,
            versions: None,
            targets: None,
            io: None,
        };
        match map_posix_refusal(ref_minv) {
            CapabilityStatus::IntegrityInvalid { detail, .. } => {
                assert!(detail.contains(guidance::MANIFEST_INVALID));
            }
            other => panic!("expected IntegrityInvalid, got {other:?}"),
        }

        // 8. restart to finish update -> ResourceOrOwnerScopeUnavailable, RESTART_UPDATE
        let ref_restart = InstalledPayloadRefusal {
            code: code::RESTART_TO_FINISH_UPDATE,
            guidance: guidance::RESTART_UPDATE,
            path: None,
            versions: None,
            targets: None,
            io: None,
        };
        match map_posix_refusal(ref_restart) {
            CapabilityStatus::ResourceOrOwnerScopeUnavailable { detail, .. } => {
                assert!(detail.contains(guidance::RESTART_UPDATE));
            }
            other => panic!("expected ResourceOrOwnerScopeUnavailable, got {other:?}"),
        }

        // 9. unexpected file -> IntegrityInvalid, PACKAGE_MISMATCH
        let ref_unexp = InstalledPayloadRefusal {
            code: code::UNEXPECTED_FILE,
            guidance: guidance::PACKAGE_MISMATCH,
            path: Some("lib/unexpected".to_owned()),
            versions: None,
            targets: None,
            io: None,
        };
        match map_posix_refusal(ref_unexp) {
            CapabilityStatus::IntegrityInvalid { detail, .. } => {
                assert!(detail.contains(guidance::PACKAGE_MISMATCH));
                assert!(detail.contains(code::UNEXPECTED_FILE));
            }
            other => panic!("expected IntegrityInvalid, got {other:?}"),
        }

        // 10. member unreadable -> ResourceOrOwnerScopeUnavailable, MEMBER_UNREADABLE
        let ref_unread = InstalledPayloadRefusal {
            code: code::MEMBER_UNREADABLE,
            guidance: guidance::MEMBER_UNREADABLE,
            path: Some("lib/unreadable".to_owned()),
            versions: None,
            targets: None,
            io: None,
        };
        match map_posix_refusal(ref_unread) {
            CapabilityStatus::ResourceOrOwnerScopeUnavailable { detail, .. } => {
                assert!(detail.contains(guidance::MEMBER_UNREADABLE));
                assert!(detail.contains(code::MEMBER_UNREADABLE));
            }
            other => panic!("expected ResourceOrOwnerScopeUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn worker_failure_maps_to_unloadable_or_unrunnable_without_install_models() {
        let root = tempfile::tempdir().unwrap();
        let target = solstone_core_installed_payload::TARGET_LINUX_X86_64;
        let lib_path = root.path().join(LINUX_CED_LIBRARY);
        let model_path = root.path().join(POSIX_CED_MODEL);
        fs::create_dir_all(lib_path.parent().unwrap()).unwrap();
        fs::write(&lib_path, b"so_content").unwrap();
        fs::write(&model_path, b"gguf_content").unwrap();

        write_rendered_manifest(root.path(), target);

        let verdict = evaluate_ced_readiness_in_package_with_probe(
            root.path(),
            "linux",
            "x86_64",
            |_library, _model| Err("signal 9".to_owned()),
        );
        match verdict {
            CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable {
                capability,
                detail,
            }) => {
                assert_eq!(capability, CED_CAPABILITY);
                assert!(detail.contains("signal 9"));
                assert!(!detail.contains("install-models"));
            }
            other => panic!("expected UnloadableOrUnrunnable, got {other:?}"),
        }
    }

    #[test]
    fn helper_probe_failure_is_unloadable_without_install_models() {
        let err = helper_error_detail(
            "{\"schema\":\"solstone-ced-error-v1\",\"reason\":\"library-unloadable\",\"detail\":\"libgomp.so.1 missing\"}\n",
        )
        .unwrap();
        assert_eq!(err, "libgomp.so.1 missing");
        assert!(!err.contains("install-models"));
        assert!(!err.contains("reinstall"));
    }

    #[test]
    fn ready_in_package_when_members_admitted_and_probe_passes() {
        let root = tempfile::tempdir().unwrap();
        let target = solstone_core_installed_payload::TARGET_LINUX_X86_64;
        let lib_path = root.path().join(LINUX_CED_LIBRARY);
        let model_path = root.path().join(POSIX_CED_MODEL);
        fs::create_dir_all(lib_path.parent().unwrap()).unwrap();
        fs::write(&lib_path, b"so_content").unwrap();
        fs::write(&model_path, b"gguf_content").unwrap();

        write_rendered_manifest(root.path(), target);

        let verdict = evaluate_ced_readiness_in_package_with_probe(
            root.path(),
            "linux",
            "x86_64",
            |library, model| {
                assert_eq!(library, lib_path);
                assert_eq!(model, model_path);
                Ok(())
            },
        );
        match verdict {
            CedVerdict::Ready { library, model } => {
                assert_eq!(library, lib_path);
                assert_eq!(model, model_path);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn fallback_negative_ignores_legacy_cache_locations() {
        use std::io::Read;

        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("repo root");
        let archive_path =
            repo_root.join("core/models/assets/ced/ced-v0.1.0-lib-linux-cpu-x64.tar.gz");
        let model_source_path = repo_root.join("core/models/assets/ced/ced-tiny-q8_0.gguf");

        let tar_gz = fs::File::open(&archive_path).expect("open archive");
        let decoder = flate2::read::GzDecoder::new(tar_gz);
        let mut archive = tar::Archive::new(decoder);
        let mut real_lib_bytes: Option<Vec<u8>> = None;
        for entry in archive.entries().expect("entries") {
            let mut entry = entry.expect("entry");
            let path = entry.path().expect("entry path");
            if path.ends_with("libced.so") {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).expect("read libced.so");
                real_lib_bytes = Some(bytes);
                break;
            }
        }
        let real_engine_bytes = real_lib_bytes.expect("extracted libced.so bytes");
        let real_model_bytes = fs::read(&model_source_path).expect("read real gguf");

        let journal = tempfile::tempdir().unwrap();
        let legacy_engine = journal
            .path()
            .join("cache/providers/ced/v0.1.0/engine/linux-cpu-x64/libced.so");
        let legacy_model = journal.path().join(
            "cache/providers/ced/v0.1.0/models/mudler__ced-gguf/b5e9a4aad6438763c8da16079d77563fbed35c65/ced-tiny-q8_0.gguf",
        );
        let legacy_sidecar_install = journal
            .path()
            .join("cache/providers/ced/v0.1.0/.ced-install.json");
        let legacy_sidecar_model = journal
            .path()
            .join("cache/providers/ced/v0.1.0/.ced-model.json");

        fs::create_dir_all(legacy_engine.parent().unwrap()).unwrap();
        fs::create_dir_all(legacy_model.parent().unwrap()).unwrap();

        fs::write(&legacy_engine, &real_engine_bytes).unwrap();
        fs::write(&legacy_model, &real_model_bytes).unwrap();
        fs::write(&legacy_sidecar_install, b"{}").unwrap();
        fs::write(&legacy_sidecar_model, b"{}").unwrap();

        let root = tempfile::tempdir().unwrap();
        let target = solstone_core_installed_payload::TARGET_LINUX_X86_64;
        let lib_path = root.path().join(LINUX_CED_LIBRARY);
        let model_path = root.path().join(POSIX_CED_MODEL);
        fs::create_dir_all(lib_path.parent().unwrap()).unwrap();
        fs::create_dir_all(model_path.parent().unwrap()).unwrap();
        fs::write(&lib_path, &real_engine_bytes).unwrap();
        fs::write(&model_path, &real_model_bytes).unwrap();

        write_rendered_manifest(root.path(), target);

        // 1. With package member present, package paths are used, probe never receives legacy path
        let verdict = evaluate_ced_readiness_in_package_with_probe(
            root.path(),
            "linux",
            "x86_64",
            |library, model| {
                assert_eq!(library, lib_path);
                assert_eq!(model, model_path);
                assert!(!library.starts_with(journal.path()));
                assert!(!model.starts_with(journal.path()));
                Ok(())
            },
        );
        assert!(matches!(verdict, CedVerdict::Ready { .. }));
        assert_eq!(fs::read(&legacy_engine).unwrap(), real_engine_bytes);
        assert_eq!(fs::read(&legacy_model).unwrap(), real_model_bytes);
        assert_eq!(fs::read(&legacy_sidecar_install).unwrap(), b"{}");
        assert_eq!(fs::read(&legacy_sidecar_model).unwrap(), b"{}");

        // 2. With package member missing, readiness is not Ready, legacy path is not used
        fs::remove_file(&lib_path).unwrap();
        let verdict_missing = evaluate_ced_readiness_in_package_with_probe(
            root.path(),
            "linux",
            "x86_64",
            |_library, _model| Ok(()),
        );
        assert!(matches!(
            verdict_missing,
            CedVerdict::Degraded(CapabilityStatus::Absent { .. })
        ));
        assert_eq!(fs::read(&legacy_engine).unwrap(), real_engine_bytes);
        assert_eq!(fs::read(&legacy_model).unwrap(), real_model_bytes);
        assert_eq!(fs::read(&legacy_sidecar_install).unwrap(), b"{}");
        assert_eq!(fs::read(&legacy_sidecar_model).unwrap(), b"{}");

        // 3. With package model changed, readiness is not Ready, legacy path is not used
        fs::write(&lib_path, &real_engine_bytes).unwrap();
        fs::write(&model_path, b"altered_model_bytes").unwrap();
        let verdict_changed = evaluate_ced_readiness_in_package_with_probe(
            root.path(),
            "linux",
            "x86_64",
            |_library, _model| Ok(()),
        );
        assert!(matches!(
            verdict_changed,
            CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid { .. })
        ));
        assert_eq!(fs::read(&legacy_engine).unwrap(), real_engine_bytes);
        assert_eq!(fs::read(&legacy_model).unwrap(), real_model_bytes);
        assert_eq!(fs::read(&legacy_sidecar_install).unwrap(), b"{}");
        assert_eq!(fs::read(&legacy_sidecar_model).unwrap(), b"{}");
    }
}
