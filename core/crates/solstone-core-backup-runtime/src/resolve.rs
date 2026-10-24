// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_backup::get_backup_config;

#[cfg(all(not(windows), any(test, feature = "test-hooks")))]
use crate::engine::run_backup_tool_resolution_started_hook;
use crate::engine::{AdmittedBackupMode, AdmittedCapability, ClosedToolError};
use crate::runner::ToolRunner;

use solstone_core_spp_attest::android_service_tools::{
    AndroidServiceToolRole, AndroidServiceToolsRefusal, resolve_admitted_android_service_tool,
    revalidate_installed_android_service_tools,
};

#[cfg(not(windows))]
const RESTIC_MEMBER: &str = "lib/solstone-restic/restic";
#[cfg(not(windows))]
const RCLONE_MEMBER: &str = "lib/solstone-rclone/rclone";

/// Captured pinned executable or admitted linked iOS implementation.
#[derive(Debug)]
pub enum ResolvedResticInvocation {
    Executable(PathBuf),
    #[cfg(target_os = "ios")]
    Callable(std::sync::Arc<crate::ios_callable::AdmittedIosBackupAuthority>),
}

impl ResolvedResticInvocation {
    pub fn invocation(&self) -> crate::runner::ResticInvocation<'_> {
        match self {
            Self::Executable(path) => path.into(),
            #[cfg(target_os = "ios")]
            Self::Callable(authority) => {
                crate::runner::ResticInvocation::Callable(authority.as_ref())
            }
        }
    }

    pub fn executable_path(&self) -> Option<&Path> {
        self.invocation().executable_path()
    }
}

impl From<PathBuf> for ResolvedResticInvocation {
    fn from(path: PathBuf) -> Self {
        Self::Executable(path)
    }
}

/// Explicit execution and, when needed, append-only transport capabilities.
#[derive(Debug)]
pub struct ResolvedTools {
    pub restic: ResolvedResticInvocation,
    pub rclone_path: Option<PathBuf>,
}

#[cfg(target_os = "ios")]
// Operated storage needs no rclone here: the linked transport keeps an operated
// destination's non-lock writes create-only.
fn resolve_ios_tools(
    runner: &dyn ToolRunner,
    journal: &Path,
) -> Result<ResolvedTools, ClosedToolError> {
    let unavailable = |detail: &str| ClosedToolError::ResticUnavailable {
        detail: detail.into(),
        guidance: solstone_core_installed_payload::guidance::PACKAGE_MISMATCH.into(),
    };
    let root = runner
        .ios_journal_root()
        .ok_or_else(|| unavailable("ios_journal_root_unavailable"))?;
    if root.canonical_path() != journal || root.revalidate_canonical_binding().is_err() {
        return Err(unavailable("ios_journal_root_changed"));
    }
    let authority = crate::ios_callable::admitted_ios_backup(std::sync::Arc::clone(root))?;
    Ok(ResolvedTools {
        restic: ResolvedResticInvocation::Callable(authority),
        rclone_path: None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPlatform {
    Desktop,
    AndroidAarch64,
    #[cfg(target_os = "ios")]
    IosCallable,
}

#[must_use]
pub fn host_tool_platform() -> ToolPlatform {
    #[cfg(target_os = "android")]
    {
        ToolPlatform::AndroidAarch64
    }
    #[cfg(target_os = "ios")]
    {
        ToolPlatform::IosCallable
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        ToolPlatform::Desktop
    }
}

pub trait AndroidAuthority: Send + Sync {
    fn resolve_tool(
        &self,
        role: AndroidServiceToolRole,
    ) -> Result<PathBuf, AndroidServiceToolsRefusal>;
    fn revalidate(&self) -> Result<(), AndroidServiceToolsRefusal>;
}

pub struct LiveAndroidAuthority;

impl AndroidAuthority for LiveAndroidAuthority {
    fn resolve_tool(
        &self,
        role: AndroidServiceToolRole,
    ) -> Result<PathBuf, AndroidServiceToolsRefusal> {
        resolve_admitted_android_service_tool(role)
    }

    fn revalidate(&self) -> Result<(), AndroidServiceToolsRefusal> {
        revalidate_installed_android_service_tools()
    }
}

#[cfg(not(windows))]
fn map_android_refusal(
    refusal: &AndroidServiceToolsRefusal,
    needs_rclone: bool,
) -> ClosedToolError {
    let is_rclone = refusal.role() == Some(AndroidServiceToolRole::Rclone);
    let detail = refusal.code().to_owned();
    let guidance =
        if refusal.code() == solstone_core_installed_payload::code::RESTART_TO_FINISH_UPDATE {
            solstone_core_installed_payload::guidance::RESTART_UPDATE.to_owned()
        } else {
            solstone_core_installed_payload::guidance::PACKAGE_MISMATCH.to_owned()
        };
    if needs_rclone && is_rclone {
        ClosedToolError::RcloneUnavailable { detail, guidance }
    } else {
        ClosedToolError::ResticUnavailable { detail, guidance }
    }
}

#[cfg(not(windows))]
fn resolve_android_tools(
    authority: &dyn AndroidAuthority,
    needs_rclone: bool,
) -> Result<ResolvedTools, ClosedToolError> {
    if let Err(refusal) = authority.revalidate() {
        return Err(map_android_refusal(&refusal, needs_rclone));
    }
    let restic_path = authority
        .resolve_tool(AndroidServiceToolRole::Restic)
        .map_err(|refusal| map_android_refusal(&refusal, needs_rclone))?;
    let rclone_path = if needs_rclone {
        Some(
            authority
                .resolve_tool(AndroidServiceToolRole::Rclone)
                .map_err(|refusal| map_android_refusal(&refusal, needs_rclone))?,
        )
    } else {
        None
    };
    Ok(ResolvedTools {
        restic: restic_path.into(),
        rclone_path,
    })
}

#[cfg(not(windows))]
fn resolve_android_restic_member(
    authority: &dyn AndroidAuthority,
) -> Result<PathBuf, ClosedToolError> {
    if let Err(refusal) = authority.revalidate() {
        return Err(map_android_refusal(&refusal, false));
    }
    authority
        .resolve_tool(AndroidServiceToolRole::Restic)
        .map_err(|refusal| map_android_refusal(&refusal, false))
}

#[cfg(not(windows))]
fn admit_package(
    executable: &Path,
    needs_rclone: bool,
) -> Result<solstone_core_installed_payload::InstalledPackage, ClosedToolError> {
    use solstone_core_installed_payload::{
        COMPILED_VERSION, InstalledPackage, compiled_target, host_executable_platform,
        locate_installed_package,
    };
    let root =
        locate_installed_package(executable, host_executable_platform()).map_err(|refusal| {
            ClosedToolError::ResticUnavailable {
                detail: refusal.code.to_owned(),
                guidance: refusal.guidance.to_owned(),
            }
        })?;
    let package =
        InstalledPackage::admit(&root, COMPILED_VERSION, compiled_target()).map_err(|refusal| {
            if needs_rclone && refusal.path.as_deref() == Some(RCLONE_MEMBER) {
                ClosedToolError::RcloneUnavailable {
                    detail: refusal.code.to_owned(),
                    guidance: refusal.guidance.to_owned(),
                }
            } else {
                ClosedToolError::ResticUnavailable {
                    detail: refusal.code.to_owned(),
                    guidance: refusal.guidance.to_owned(),
                }
            }
        })?;
    Ok(package)
}

/// Resolve the pinned tools required by an already-admitted backup capability.
pub fn resolve_tools(
    capability: &AdmittedCapability,
    runner: &dyn ToolRunner,
    executable: &Path,
) -> Result<ResolvedTools, ClosedToolError> {
    #[cfg(not(windows))]
    {
        resolve_tools_for_platform(
            capability,
            runner,
            executable,
            host_tool_platform(),
            &LiveAndroidAuthority,
        )
    }

    #[cfg(windows)]
    {
        let _ = (runner, executable);
        resolve_tools_windows(capability)
    }
}

#[cfg(not(windows))]
pub fn resolve_tools_for_platform(
    capability: &AdmittedCapability,
    _runner: &dyn ToolRunner,
    executable: &Path,
    platform: ToolPlatform,
    android_authority: &dyn AndroidAuthority,
) -> Result<ResolvedTools, ClosedToolError> {
    #[cfg(any(test, feature = "test-hooks"))]
    run_backup_tool_resolution_started_hook(&capability.resolved_journal);

    let needs_rclone = matches!(capability.mode, AdmittedBackupMode::Operated { .. });

    match platform {
        #[cfg(target_os = "ios")]
        ToolPlatform::IosCallable => resolve_ios_tools(_runner, &capability.resolved_journal),
        ToolPlatform::Desktop => {
            let package = admit_package(executable, needs_rclone)?;
            let restic_path = package.member(RESTIC_MEMBER).map_err(|refusal| {
                ClosedToolError::ResticUnavailable {
                    detail: refusal.code.to_owned(),
                    guidance: refusal.guidance.to_owned(),
                }
            })?;
            let rclone_path = if needs_rclone {
                Some(package.member(RCLONE_MEMBER).map_err(|refusal| {
                    ClosedToolError::RcloneUnavailable {
                        detail: refusal.code.to_owned(),
                        guidance: refusal.guidance.to_owned(),
                    }
                })?)
            } else {
                None
            };
            Ok(ResolvedTools {
                restic: restic_path.into(),
                rclone_path,
            })
        }
        ToolPlatform::AndroidAarch64 => {
            let _ = executable;
            resolve_android_tools(android_authority, needs_rclone)
        }
    }
}

#[cfg(windows)]
fn resolve_tools_windows(
    capability: &AdmittedCapability,
) -> Result<ResolvedTools, ClosedToolError> {
    let needs_rclone = matches!(capability.mode, AdmittedBackupMode::Operated { .. });
    let restic_path = crate::windows_tool::verify_package_and_get_tool(
        solstone_core_installed_payload::windows_payload::WINDOWS_RESTIC_WORKER,
    )
    .map_err(|err| {
        let msg = err.to_string();
        ClosedToolError::ResticUnavailable {
            detail: msg.clone(),
            guidance: msg,
        }
    })?;
    let rclone_path = if needs_rclone {
        Some(
            crate::windows_tool::verify_package_and_get_tool(
                solstone_core_installed_payload::windows_payload::WINDOWS_RCLONE_WORKER,
            )
            .map_err(|err| {
                let msg = err.to_string();
                ClosedToolError::RcloneUnavailable {
                    detail: msg.clone(),
                    guidance: msg,
                }
            })?,
        )
    } else {
        None
    };
    Ok(ResolvedTools {
        restic: restic_path.into(),
        rclone_path,
    })
}

/// Resolve the pinned restic binary, and rclone when this journal needs an
/// operated append-only session.
pub fn resolve_operational_tools(
    runner: &dyn ToolRunner,
    journal: &Path,
    append_only: bool,
    executable: &Path,
) -> Result<ResolvedTools, ClosedToolError> {
    #[cfg(not(windows))]
    {
        resolve_operational_tools_for_platform(
            runner,
            journal,
            append_only,
            executable,
            host_tool_platform(),
            &LiveAndroidAuthority,
        )
    }

    #[cfg(windows)]
    {
        let _ = (runner, executable);
        resolve_operational_tools_windows(journal, append_only)
    }
}

#[cfg(not(windows))]
pub fn resolve_operational_tools_for_platform(
    _runner: &dyn ToolRunner,
    journal: &Path,
    append_only: bool,
    executable: &Path,
    platform: ToolPlatform,
    android_authority: &dyn AndroidAuthority,
) -> Result<ResolvedTools, ClosedToolError> {
    let needs_rclone = append_only && journal_is_operated(journal);
    match platform {
        #[cfg(target_os = "ios")]
        ToolPlatform::IosCallable => resolve_ios_tools(_runner, journal),
        ToolPlatform::Desktop => {
            let package = admit_package(executable, needs_rclone)?;
            let restic_path = package.member(RESTIC_MEMBER).map_err(|refusal| {
                ClosedToolError::ResticUnavailable {
                    detail: refusal.code.to_owned(),
                    guidance: refusal.guidance.to_owned(),
                }
            })?;
            let rclone_path = if needs_rclone {
                Some(package.member(RCLONE_MEMBER).map_err(|refusal| {
                    ClosedToolError::RcloneUnavailable {
                        detail: refusal.code.to_owned(),
                        guidance: refusal.guidance.to_owned(),
                    }
                })?)
            } else {
                None
            };
            Ok(ResolvedTools {
                restic: restic_path.into(),
                rclone_path,
            })
        }
        ToolPlatform::AndroidAarch64 => {
            let _ = executable;
            resolve_android_tools(android_authority, needs_rclone)
        }
    }
}

#[cfg(windows)]
fn resolve_operational_tools_windows(
    journal: &Path,
    append_only: bool,
) -> Result<ResolvedTools, ClosedToolError> {
    let needs_rclone = append_only && journal_is_operated(journal);
    let restic_path = crate::windows_tool::verify_package_and_get_tool(
        solstone_core_installed_payload::windows_payload::WINDOWS_RESTIC_WORKER,
    )
    .map_err(|err| {
        let msg = err.to_string();
        ClosedToolError::ResticUnavailable {
            detail: msg.clone(),
            guidance: msg,
        }
    })?;
    let rclone_path = if needs_rclone {
        Some(
            crate::windows_tool::verify_package_and_get_tool(
                solstone_core_installed_payload::windows_payload::WINDOWS_RCLONE_WORKER,
            )
            .map_err(|err| {
                let msg = err.to_string();
                ClosedToolError::RcloneUnavailable {
                    detail: msg.clone(),
                    guidance: msg,
                }
            })?,
        )
    } else {
        None
    };
    Ok(ResolvedTools {
        restic: restic_path.into(),
        rclone_path,
    })
}

/// Resolve the pinned restic binary only.
pub fn resolve_restic_member(executable: &Path) -> Result<PathBuf, ClosedToolError> {
    #[cfg(not(windows))]
    {
        resolve_restic_member_for_platform(executable, host_tool_platform(), &LiveAndroidAuthority)
    }

    #[cfg(windows)]
    {
        let _ = executable;
        crate::windows_tool::verify_package_and_get_tool(
            solstone_core_installed_payload::windows_payload::WINDOWS_RESTIC_WORKER,
        )
        .map_err(|err| {
            let msg = err.to_string();
            ClosedToolError::ResticUnavailable {
                detail: msg.clone(),
                guidance: msg,
            }
        })
    }
}

/// Resolve the pinned rclone binary, the operated append-only transport.
pub fn resolve_rclone_member(executable: &Path) -> Result<PathBuf, ClosedToolError> {
    #[cfg(not(windows))]
    {
        let package = admit_package(executable, true)?;
        package
            .member(RCLONE_MEMBER)
            .map_err(|refusal| ClosedToolError::RcloneUnavailable {
                detail: refusal.code.to_owned(),
                guidance: refusal.guidance.to_owned(),
            })
    }

    #[cfg(windows)]
    {
        let _ = executable;
        crate::windows_tool::verify_package_and_get_tool(
            solstone_core_installed_payload::windows_payload::WINDOWS_RCLONE_WORKER,
        )
        .map_err(|err| {
            let msg = err.to_string();
            ClosedToolError::RcloneUnavailable {
                detail: msg.clone(),
                guidance: msg,
            }
        })
    }
}

#[cfg(not(windows))]
pub fn resolve_restic_member_for_platform(
    executable: &Path,
    platform: ToolPlatform,
    android_authority: &dyn AndroidAuthority,
) -> Result<PathBuf, ClosedToolError> {
    match platform {
        #[cfg(target_os = "ios")]
        ToolPlatform::IosCallable => Err(ClosedToolError::ResticUnavailable {
            detail: "ios_executable_backup_tool_unavailable".into(),
            guidance: solstone_core_installed_payload::guidance::PACKAGE_MISMATCH.into(),
        }),
        ToolPlatform::Desktop => {
            let package = admit_package(executable, false)?;
            package
                .member(RESTIC_MEMBER)
                .map_err(|refusal| ClosedToolError::ResticUnavailable {
                    detail: refusal.code.to_owned(),
                    guidance: refusal.guidance.to_owned(),
                })
        }
        ToolPlatform::AndroidAarch64 => {
            let _ = executable;
            resolve_android_restic_member(android_authority)
        }
    }
}

fn journal_is_operated(journal: &Path) -> bool {
    let Ok(config) = get_backup_config(journal) else {
        return false;
    };
    config.get("enabled") == Some(&Value::Bool(true))
        && config.get("mode") == Some(&Value::String("operated".into()))
}

// These fixtures exercise Unix package admission; Windows verifies its worker manifest instead.
#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use crate::engine::{Clock, prepare};
    use crate::runner::ToolRunner;
    use solstone_core_installed_payload::{
        COMPILED_VERSION, INSTALLED_PAYLOAD_MANIFEST, PRODUCT, code, compiled_target, guidance,
        render_installed_payload,
    };
    use std::fs;

    struct NoOpRunner;
    impl ToolRunner for NoOpRunner {
        fn run(
            &self,
            _: &crate::runner::ToolRequest<'_>,
        ) -> std::io::Result<crate::runner::ToolOutput> {
            panic!("resolve must not invoke runner")
        }
    }

    struct FixedClock;
    impl Clock for FixedClock {
        fn now_unix(&self) -> i64 {
            1_700_000_000
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }

    fn write_file(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
    }

    fn build_fixture_tree(root: &Path, restic_bytes: &[u8], rclone_bytes: &[u8]) -> PathBuf {
        let exe = root.join("bin/solstone");
        write_file(root, "bin/solstone", b"launcher");
        write_file(root, RESTIC_MEMBER, restic_bytes);
        write_file(root, RCLONE_MEMBER, rclone_bytes);
        let manifest = render_installed_payload(
            root,
            PRODUCT,
            COMPILED_VERSION,
            compiled_target(),
            "fixture-commit",
        )
        .unwrap();
        write_file(root, INSTALLED_PAYLOAD_MANIFEST, &manifest);
        exe
    }

    fn configured_journal() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let destination = solstone_core_backup::Destination {
            repository: "s3:repo".into(),
            backend: "s3".into(),
            credentials: serde_json::json!({"access_key_id":"access","secret_access_key":"secret"})
                .as_object()
                .unwrap()
                .clone(),
        };
        solstone_core_backup::set_destination(dir.path(), &destination).unwrap();
        solstone_core_backup::generate_and_store_keys(dir.path()).unwrap();
        solstone_core_backup::set_enabled(dir.path(), true).unwrap();
        dir
    }

    #[test]
    fn resolve_tools_success_byo_and_operational() {
        let pkg = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(pkg.path(), b"restic-v0.19", b"rclone-v1.69");
        let journal = configured_journal();
        let clock = FixedClock;
        let capability = prepare(journal.path(), &clock).unwrap();

        let resolved = resolve_tools(&capability, &NoOpRunner, &exe).unwrap();
        assert_eq!(
            resolved.restic.executable_path().unwrap().to_path_buf(),
            pkg.path().join(RESTIC_MEMBER)
        );
        assert_eq!(resolved.rclone_path, None);

        let op_resolved =
            resolve_operational_tools(&NoOpRunner, journal.path(), false, &exe).unwrap();
        assert_eq!(
            op_resolved.restic.executable_path().unwrap().to_path_buf(),
            pkg.path().join(RESTIC_MEMBER)
        );
        assert_eq!(op_resolved.rclone_path, None);
    }

    #[test]
    fn altered_member_refuses_with_member_changed() {
        let pkg = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(pkg.path(), b"restic-v0.19", b"rclone-v1.69");
        // Flip one byte, preserving length
        write_file(pkg.path(), RESTIC_MEMBER, b"restic-x0.19");
        let journal = configured_journal();
        let clock = FixedClock;
        let capability = prepare(journal.path(), &clock).unwrap();

        let err = resolve_tools(&capability, &NoOpRunner, &exe).unwrap_err();
        assert_eq!(err.detail(), code::MEMBER_CHANGED);
        assert_eq!(err.guidance(), guidance::PACKAGE_MISMATCH);
    }

    #[test]
    fn missing_member_refuses_with_member_missing() {
        let pkg = tempfile::tempdir().unwrap();
        let exe = build_fixture_tree(pkg.path(), b"restic-v0.19", b"rclone-v1.69");
        fs::remove_file(pkg.path().join(RESTIC_MEMBER)).unwrap();
        let journal = configured_journal();
        let clock = FixedClock;
        let capability = prepare(journal.path(), &clock).unwrap();

        let err = resolve_tools(&capability, &NoOpRunner, &exe).unwrap_err();
        assert_eq!(err.detail(), code::MEMBER_MISSING);
        assert_eq!(err.guidance(), guidance::PACKAGE_MISMATCH);
    }

    #[test]
    fn newer_manifest_version_refuses_with_restart_update() {
        let pkg = tempfile::tempdir().unwrap();
        write_file(pkg.path(), "bin/solstone", b"launcher");
        write_file(pkg.path(), RESTIC_MEMBER, b"restic-v0.19");
        write_file(pkg.path(), RCLONE_MEMBER, b"rclone-v1.69");
        let manifest = render_installed_payload(
            pkg.path(),
            PRODUCT,
            "999.0.0",
            compiled_target(),
            "fixture-commit",
        )
        .unwrap();
        write_file(pkg.path(), INSTALLED_PAYLOAD_MANIFEST, &manifest);
        let exe = pkg.path().join("bin/solstone");

        let journal = configured_journal();
        let clock = FixedClock;
        let capability = prepare(journal.path(), &clock).unwrap();

        let err = resolve_tools(&capability, &NoOpRunner, &exe).unwrap_err();
        assert_eq!(err.detail(), code::RESTART_TO_FINISH_UPDATE);
        assert_eq!(err.guidance(), guidance::RESTART_UPDATE);
    }

    struct PlantedFallbacks {
        _home: tempfile::TempDir,
        _path_dir: tempfile::TempDir,
        _bundle_dir: tempfile::TempDir,
        script_dirs: Vec<PathBuf>,
        recorded_files: Vec<(PathBuf, Vec<u8>)>,
        prev_home: Option<std::ffi::OsString>,
        prev_path: Option<std::ffi::OsString>,
        prev_restic_bundle: Option<std::ffi::OsString>,
        prev_rclone_bundle: Option<std::ffi::OsString>,
    }

    #[allow(unsafe_code)]
    impl Drop for PlantedFallbacks {
        fn drop(&mut self) {
            unsafe {
                if let Some(ref val) = self.prev_home {
                    std::env::set_var("HOME", val);
                } else {
                    std::env::remove_var("HOME");
                }
                if let Some(ref val) = self.prev_path {
                    std::env::set_var("PATH", val);
                } else {
                    std::env::remove_var("PATH");
                }
                if let Some(ref val) = self.prev_restic_bundle {
                    std::env::set_var("SOLSTONE_RESTIC_BUNDLE", val);
                } else {
                    std::env::remove_var("SOLSTONE_RESTIC_BUNDLE");
                }
                if let Some(ref val) = self.prev_rclone_bundle {
                    std::env::set_var("SOLSTONE_RCLONE_BUNDLE", val);
                } else {
                    std::env::remove_var("SOLSTONE_RCLONE_BUNDLE");
                }
            }
        }
    }

    impl PlantedFallbacks {
        fn verify(&self) {
            for dir in &self.script_dirs {
                assert!(
                    !dir.join("marker").exists(),
                    "marker file was created in {:?}",
                    dir
                );
            }
            for (path, expected_bytes) in &self.recorded_files {
                let actual = fs::read(path).unwrap_or_else(|_| panic!("read {:?}", path));
                assert_eq!(&actual, expected_bytes, "file {:?} was modified", path);
            }
        }
    }

    #[allow(unsafe_code)]
    fn plant_fallbacks(pkg_root: &Path) -> PlantedFallbacks {
        let home = tempfile::tempdir().unwrap();
        let path_dir = tempfile::tempdir().unwrap();
        let bundle_dir = tempfile::tempdir().unwrap();

        let prev_home = std::env::var_os("HOME");
        let prev_path = std::env::var_os("PATH");
        let prev_restic_bundle = std::env::var_os("SOLSTONE_RESTIC_BUNDLE");
        let prev_rclone_bundle = std::env::var_os("SOLSTONE_RCLONE_BUNDLE");

        let script_content = b"#!/bin/sh\ntouch \"$(dirname \"$0\")/marker\"\n";
        let sentinel_content = b"sentinel-complete";
        let license_content = b"LICENSE-TEXT";

        let mut recorded_files = Vec::new();
        let mut script_dirs = Vec::new();

        let mut plant = |dir: &Path, name: &str| -> PathBuf {
            fs::create_dir_all(dir).unwrap();
            let script = dir.join(name);
            fs::write(&script, script_content).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            }
            recorded_files.push((script.clone(), script_content.to_vec()));

            let sentinel = dir.join(".install-complete");
            fs::write(&sentinel, sentinel_content).unwrap();
            recorded_files.push((sentinel, sentinel_content.to_vec()));

            let license = dir.join("LICENSE");
            fs::write(&license, license_content).unwrap();
            recorded_files.push((license, license_content.to_vec()));

            if !script_dirs.contains(&dir.to_path_buf()) {
                script_dirs.push(dir.to_path_buf());
            }
            script
        };

        // 1. $HOME/.cache/solstone/restic and $HOME/.cache/solstone/rclone
        let cache_dir = home.path().join(".cache/solstone");
        plant(&cache_dir, "restic");
        plant(&cache_dir, "rclone");

        // 2. $HOME/Library/Application Support/solstone/restic and the rclone sibling
        let app_support_dir = home.path().join("Library/Application Support/solstone");
        plant(&app_support_dir, "restic");
        plant(&app_support_dir, "rclone");

        // 3. <package-root>/_bin/restic and <package-root>/_bin/rclone
        let pkg_bin_dir = pkg_root.join("_bin");
        plant(&pkg_bin_dir, "restic");
        plant(&pkg_bin_dir, "rclone");

        // 4. two other scripts named by SOLSTONE_RESTIC_BUNDLE and SOLSTONE_RCLONE_BUNDLE
        let restic_bundle = plant(bundle_dir.path(), "bundle-restic");
        let rclone_bundle = plant(bundle_dir.path(), "bundle-rclone");

        // 5. a directory prepended to PATH containing restic and rclone
        plant(path_dir.path(), "restic");
        plant(path_dir.path(), "rclone");

        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::set_var("SOLSTONE_RESTIC_BUNDLE", &restic_bundle);
            std::env::set_var("SOLSTONE_RCLONE_BUNDLE", &rclone_bundle);
        }

        let new_path = if let Some(ref old) = prev_path {
            let mut p = path_dir.path().as_os_str().to_os_string();
            p.push(":");
            p.push(old);
            p
        } else {
            path_dir.path().as_os_str().to_os_string()
        };
        unsafe {
            std::env::set_var("PATH", new_path);
        }

        PlantedFallbacks {
            _home: home,
            _path_dir: path_dir,
            _bundle_dir: bundle_dir,
            script_dirs,
            recorded_files,
            prev_home,
            prev_path,
            prev_restic_bundle,
            prev_rclone_bundle,
        }
    }

    #[test]
    fn fallback_negative_ignores_markers_and_env() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _lock = ENV_LOCK.lock().unwrap();

        let pkg = tempfile::tempdir().unwrap();
        let exe = pkg.path().join("bin/solstone");
        write_file(pkg.path(), "bin/solstone", b"launcher");

        let fallbacks = plant_fallbacks(pkg.path());

        let journal = configured_journal();
        let clock = FixedClock;
        let capability = prepare(journal.path(), &clock).unwrap();

        let err = resolve_tools(&capability, &NoOpRunner, &exe).unwrap_err();
        assert_eq!(err.to_string(), "restic_unavailable");
        assert_eq!(
            err.detail(),
            solstone_core_installed_payload::code::MANIFEST_MISSING
        );
        assert_eq!(
            err.guidance(),
            solstone_core_installed_payload::guidance::MANIFEST_MISSING
        );

        fallbacks.verify();
    }

    // The `_for_platform` resolvers these exercise are not built on Windows.
    #[cfg(not(windows))]
    struct MockAndroidAuthority {
        restic: Result<PathBuf, AndroidServiceToolsRefusal>,
        rclone: Result<PathBuf, AndroidServiceToolsRefusal>,
        revalidate_err: Option<AndroidServiceToolsRefusal>,
    }

    #[cfg(not(windows))]
    impl AndroidAuthority for MockAndroidAuthority {
        fn resolve_tool(
            &self,
            role: AndroidServiceToolRole,
        ) -> Result<PathBuf, AndroidServiceToolsRefusal> {
            match role {
                AndroidServiceToolRole::Restic => self.restic.clone(),
                AndroidServiceToolRole::Rclone => self.rclone.clone(),
                AndroidServiceToolRole::Nvattest | AndroidServiceToolRole::Nvat => {
                    Err(AndroidServiceToolsRefusal::RoleMissing { role })
                }
            }
        }

        fn revalidate(&self) -> Result<(), AndroidServiceToolsRefusal> {
            if let Some(ref err) = self.revalidate_err {
                Err(err.clone())
            } else {
                Ok(())
            }
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn android_resolution_uninstalled_returns_member_missing() {
        let auth = MockAndroidAuthority {
            restic: Err(AndroidServiceToolsRefusal::MemberMissing {
                path: "librestic.so".to_owned(),
                role: Some(AndroidServiceToolRole::Restic),
            }),
            rclone: Err(AndroidServiceToolsRefusal::MemberMissing {
                path: "librclone.so".to_owned(),
                role: Some(AndroidServiceToolRole::Rclone),
            }),
            revalidate_err: None,
        };
        let journal = configured_journal();
        let capability = prepare(journal.path(), &FixedClock).unwrap();
        let dummy_exe = Path::new("/system/bin/app_process64");

        let err = resolve_tools_for_platform(
            &capability,
            &NoOpRunner,
            dummy_exe,
            ToolPlatform::AndroidAarch64,
            &auth,
        )
        .unwrap_err();
        assert_eq!(err.detail(), code::MEMBER_MISSING);
        assert_eq!(err.guidance(), guidance::PACKAGE_MISMATCH);

        let err_restic =
            resolve_restic_member_for_platform(dummy_exe, ToolPlatform::AndroidAarch64, &auth)
                .unwrap_err();
        assert_eq!(err_restic.detail(), code::MEMBER_MISSING);
    }

    #[cfg(not(windows))]
    #[test]
    fn android_resolution_revalidate_changed_returns_member_changed() {
        let auth = MockAndroidAuthority {
            restic: Ok(PathBuf::from("/data/app/librestic.so")),
            rclone: Ok(PathBuf::from("/data/app/librclone.so")),
            revalidate_err: Some(AndroidServiceToolsRefusal::MemberChanged {
                path: "librestic.so".to_owned(),
                role: Some(AndroidServiceToolRole::Restic),
            }),
        };
        let journal = configured_journal();
        let capability = prepare(journal.path(), &FixedClock).unwrap();
        let dummy_exe = Path::new("/system/bin/app_process64");

        let err = resolve_tools_for_platform(
            &capability,
            &NoOpRunner,
            dummy_exe,
            ToolPlatform::AndroidAarch64,
            &auth,
        )
        .unwrap_err();
        assert_eq!(err.detail(), code::MEMBER_CHANGED);
    }

    #[cfg(not(windows))]
    #[test]
    fn android_resolution_revalidate_rclone_missing_when_operated() {
        let auth = MockAndroidAuthority {
            restic: Ok(PathBuf::from("/data/app/librestic.so")),
            rclone: Err(AndroidServiceToolsRefusal::MemberMissing {
                path: "librclone.so".to_owned(),
                role: Some(AndroidServiceToolRole::Rclone),
            }),
            revalidate_err: None,
        };
        let journal = configured_journal();
        solstone_core_backup::set_mode(journal.path(), "operated").unwrap();
        let dummy_exe = Path::new("/system/bin/app_process64");

        let err = resolve_operational_tools_for_platform(
            &NoOpRunner,
            journal.path(),
            true,
            dummy_exe,
            ToolPlatform::AndroidAarch64,
            &auth,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "rclone_unavailable");
        assert_eq!(err.detail(), code::MEMBER_MISSING);
    }

    #[cfg(not(windows))]
    #[test]
    fn android_resolution_succeeds_with_admitted_tools() {
        let auth = MockAndroidAuthority {
            restic: Ok(PathBuf::from("/data/app/lib/arm64/librestic.so")),
            rclone: Ok(PathBuf::from("/data/app/lib/arm64/librclone.so")),
            revalidate_err: None,
        };
        let journal = configured_journal();
        let capability = prepare(journal.path(), &FixedClock).unwrap();
        let dummy_exe = Path::new("/system/bin/app_process64");

        let tools = resolve_tools_for_platform(
            &capability,
            &NoOpRunner,
            dummy_exe,
            ToolPlatform::AndroidAarch64,
            &auth,
        )
        .unwrap();
        assert_eq!(
            tools.restic.executable_path().unwrap().to_path_buf(),
            PathBuf::from("/data/app/lib/arm64/librestic.so")
        );

        let restic =
            resolve_restic_member_for_platform(dummy_exe, ToolPlatform::AndroidAarch64, &auth)
                .unwrap();
        assert_eq!(restic, PathBuf::from("/data/app/lib/arm64/librestic.so"));
    }
}
