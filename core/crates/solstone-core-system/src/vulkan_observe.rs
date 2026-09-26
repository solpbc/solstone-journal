// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! System-owned Vulkan observation entry point.
//!
//! Unix takes one memoized snapshot from `solstone-core-local`.
//! Windows verifies the signed package payload and invokes the bounded helper.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use solstone_core_local::VulkanDevice;

pub const VULKAN_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
pub const VULKAN_PROBE_STDIN_LIMIT_BYTES: usize = 1;
pub const VULKAN_PROBE_STDOUT_LIMIT_BYTES: usize = 64 * 1024;
pub const VULKAN_PROBE_STDERR_LIMIT_BYTES: usize = 64 * 1024;

/// Result of observing host Vulkan device topology.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VulkanObservation {
    pub devices: Vec<VulkanDevice>,
    pub succeeded: bool,
}

/// Host-independent result of executing the Windows bounded helper.
#[derive(Debug, PartialEq, Eq)]
pub enum WindowsProbeFinish {
    Completed {
        exit_code: i32,
        stdout: Vec<u8>,
        quiescent: bool,
    },
    Incomplete,
    TimedOut,
    Overflow,
}

/// Verified package locations required for Windows Vulkan probe invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedVulkanPackage {
    pub package_root: PathBuf,
    pub executable: PathBuf,
    pub loader: PathBuf,
}

/// Host-independent launch specification for Windows bounded Vulkan helper execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VulkanWindowsLaunchSpec {
    pub package_root: PathBuf,
    pub executable: PathBuf,
    pub current_directory: PathBuf,
    pub loader: PathBuf,
    pub environment: BTreeMap<OsString, OsString>,
    pub stdin: Vec<u8>,
    pub timeout: Duration,
    pub stdin_limit_bytes: usize,
    pub stdout_limit_bytes: usize,
    pub stderr_limit_bytes: usize,
}

/// Derive the containing package root for an executable in `<root>/bin/`.
#[allow(clippy::result_unit_err)]
pub fn package_root_from_executable(executable: &Path) -> Result<PathBuf, ()> {
    let bin = executable.parent().ok_or(())?;
    if bin.file_name() != Some(OsStr::new("bin")) {
        return Err(());
    }
    let root = bin.parent().ok_or(())?;
    Ok(root.to_path_buf())
}

/// Resolve and verify the Vulkan helper and loader from the signed package containing `executable`.
#[cfg(any(windows, test))]
pub fn verified_windows_vulkan_package_at(
    executable: &Path,
) -> Result<VerifiedVulkanPackage, String> {
    let package_root = package_root_from_executable(executable)
        .map_err(|_| "running executable is not in the package bin directory".to_owned())?;
    let payload =
        solstone_core_distribution::windows_payload::verify_windows_payload(&package_root)
            .map_err(|error| error.to_string())?;
    let probe = payload
        .vulkan_probe_path()
        .map_err(|error| error.to_string())?;
    let loader = payload
        .vulkan_loader_path()
        .map_err(|error| error.to_string())?;
    Ok(VerifiedVulkanPackage {
        package_root,
        executable: probe,
        loader,
    })
}

/// Construct the complete Windows launch specification from verified package members.
#[allow(clippy::result_unit_err)]
pub fn vulkan_windows_launch_spec(
    package: &VerifiedVulkanPackage,
    system_root: Option<OsString>,
) -> Result<VulkanWindowsLaunchSpec, ()> {
    let system_root = match system_root {
        Some(val) if !val.is_empty() => val,
        _ => return Err(()),
    };
    let mut environment = BTreeMap::from([(OsString::from("SystemRoot"), system_root)]);
    environment
        .extend(solstone_core_distribution::manifest_verify::signed_package_pin_environment());
    Ok(VulkanWindowsLaunchSpec {
        package_root: package.package_root.clone(),
        executable: package.executable.clone(),
        current_directory: package.package_root.join("bin"),
        loader: package.loader.clone(),
        environment,
        stdin: Vec::new(),
        timeout: VULKAN_PROBE_TIMEOUT,
        stdin_limit_bytes: VULKAN_PROBE_STDIN_LIMIT_BYTES,
        stdout_limit_bytes: VULKAN_PROBE_STDOUT_LIMIT_BYTES,
        stderr_limit_bytes: VULKAN_PROBE_STDERR_LIMIT_BYTES,
    })
}

fn validate_launch_spec_shape(spec: &VulkanWindowsLaunchSpec) -> bool {
    let mut has_system_root = false;
    for (key, val) in &spec.environment {
        let key_str = key.to_string_lossy();
        if key_str.eq_ignore_ascii_case("path") {
            return false;
        }
        if key_str.eq_ignore_ascii_case("systemroot") && !val.is_empty() {
            has_system_root = true;
        }
    }
    if !has_system_root {
        return false;
    }
    spec.timeout == VULKAN_PROBE_TIMEOUT
        && spec.stdin.is_empty()
        && spec.stdin_limit_bytes == VULKAN_PROBE_STDIN_LIMIT_BYTES
        && spec.stdout_limit_bytes == VULKAN_PROBE_STDOUT_LIMIT_BYTES
        && spec.stderr_limit_bytes == VULKAN_PROBE_STDERR_LIMIT_BYTES
}

/// Observe Windows Vulkan devices through injected verification and launch adapters.
pub fn observe_windows_vulkan<V, L>(verify: V, launch: L) -> VulkanObservation
where
    V: FnOnce() -> Result<VulkanWindowsLaunchSpec, ()>,
    L: FnOnce(&VulkanWindowsLaunchSpec) -> WindowsProbeFinish,
{
    let spec = match verify() {
        Ok(spec) => spec,
        Err(()) => {
            return VulkanObservation {
                devices: Vec::new(),
                succeeded: false,
            };
        }
    };
    if !validate_launch_spec_shape(&spec) {
        return VulkanObservation {
            devices: Vec::new(),
            succeeded: false,
        };
    }
    let finish = launch(&spec);
    match finish {
        WindowsProbeFinish::Completed {
            exit_code,
            stdout,
            quiescent,
        } => {
            if exit_code != 0 || !quiescent {
                return VulkanObservation {
                    devices: Vec::new(),
                    succeeded: false,
                };
            }
            match serde_json::from_slice::<Vec<VulkanDevice>>(&stdout) {
                Ok(devices) if devices.iter().all(|d| d.device_type.is_some()) => {
                    VulkanObservation {
                        devices,
                        succeeded: true,
                    }
                }
                _ => VulkanObservation {
                    devices: Vec::new(),
                    succeeded: false,
                },
            }
        }
        WindowsProbeFinish::Incomplete
        | WindowsProbeFinish::TimedOut
        | WindowsProbeFinish::Overflow => VulkanObservation {
            devices: Vec::new(),
            succeeded: false,
        },
    }
}

/// Observe Vulkan devices on the current host.
pub fn observe_vulkan_devices() -> VulkanObservation {
    #[cfg(not(windows))]
    {
        let (devices, ok) = solstone_core_local::vulkan_probe_snapshot();
        VulkanObservation {
            devices,
            succeeded: ok,
        }
    }
    #[cfg(windows)]
    {
        observe_windows_vulkan(
            || {
                let executable = std::env::current_exe().map_err(|_| ())?;
                let package = verified_windows_vulkan_package_at(&executable).map_err(|_| ())?;
                let system_root = std::env::var_os("SystemRoot");
                vulkan_windows_launch_spec(&package, system_root)
            },
            |spec| {
                use crate::process::{
                    BoundedHelperBudget, BoundedHelperError, BoundedHelperRequest,
                    BoundedHelperResources, run_bounded_helper,
                };
                let request = BoundedHelperRequest {
                    package_root: spec.package_root.clone(),
                    executable: spec.executable.clone(),
                    current_directory: spec.current_directory.clone(),
                    arguments: Vec::new(),
                    environment: spec.environment.clone(),
                    stdin: spec.stdin.clone(),
                    budget: BoundedHelperBudget {
                        timeout: spec.timeout,
                        stdin_limit_bytes: spec.stdin_limit_bytes,
                        stdout_limit_bytes: spec.stdout_limit_bytes,
                        stderr_limit_bytes: spec.stderr_limit_bytes,
                    },
                    resource_limits: None,
                    resources: BoundedHelperResources::new(),
                };
                match run_bounded_helper(request) {
                    Ok(output) => {
                        if !output.quiescent {
                            WindowsProbeFinish::Incomplete
                        } else {
                            WindowsProbeFinish::Completed {
                                exit_code: output.exit_code,
                                stdout: output.stdout,
                                quiescent: output.quiescent,
                            }
                        }
                    }
                    Err(failure) => match failure.cause() {
                        BoundedHelperError::DeadlineExceeded { .. } => WindowsProbeFinish::TimedOut,
                        BoundedHelperError::OutputLimitExceeded { .. } => {
                            WindowsProbeFinish::Overflow
                        }
                        _ => WindowsProbeFinish::Incomplete,
                    },
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeSet;

    use solstone_core_distribution::windows_payload::{
        WINDOWS_VULKAN_LOADER, WINDOWS_VULKAN_PROBE,
    };

    use super::*;

    #[test]
    fn package_root_from_executable_requires_bin_parent() {
        assert_eq!(
            package_root_from_executable(Path::new("/opt/app/bin/probe.exe")),
            Ok(PathBuf::from("/opt/app"))
        );
        assert_eq!(
            package_root_from_executable(Path::new("/opt/app/lib/probe.exe")),
            Err(())
        );
        assert_eq!(
            package_root_from_executable(Path::new("probe.exe")),
            Err(())
        );
    }

    #[test]
    fn windows_launch_spec_builder_binds_environment_and_geometry() {
        let root = PathBuf::from("/opt/solstone");
        let package = VerifiedVulkanPackage {
            package_root: root.clone(),
            executable: root.join(WINDOWS_VULKAN_PROBE),
            loader: root.join(WINDOWS_VULKAN_LOADER),
        };
        let system_root = OsString::from("C:\\Windows");
        let spec = vulkan_windows_launch_spec(&package, Some(system_root.clone()))
            .expect("spec construction succeeds");

        assert_eq!(spec.package_root, root);
        assert_eq!(spec.executable, root.join(WINDOWS_VULKAN_PROBE));
        assert_eq!(spec.current_directory, root.join("bin"));
        assert_eq!(spec.loader, root.join(WINDOWS_VULKAN_LOADER));
        assert_eq!(spec.timeout, Duration::from_secs(10));
        assert!(spec.stdin.is_empty());
        assert_eq!(spec.stdin_limit_bytes, 1);
        assert_eq!(spec.stdout_limit_bytes, 64 * 1024);
        assert_eq!(spec.stderr_limit_bytes, 64 * 1024);

        let actual_keys: BTreeSet<_> = spec.environment.keys().cloned().collect();
        let mut expected_keys = BTreeSet::from([OsString::from("SystemRoot")]);
        for (k, _) in solstone_core_distribution::manifest_verify::signed_package_pin_environment()
        {
            expected_keys.insert(k);
        }
        assert_eq!(actual_keys, expected_keys);
        assert!(!actual_keys.contains(&OsString::from("PATH")));
    }

    #[test]
    fn launch_spec_shape_validation_rejects_malformed_requests() {
        let root = PathBuf::from("/opt/solstone");
        let package = VerifiedVulkanPackage {
            package_root: root.clone(),
            executable: root.join(WINDOWS_VULKAN_PROBE),
            loader: root.join(WINDOWS_VULKAN_LOADER),
        };
        let good_spec =
            vulkan_windows_launch_spec(&package, Some(OsString::from("C:\\Windows"))).unwrap();
        assert!(validate_launch_spec_shape(&good_spec));

        // Missing/empty SystemRoot
        assert!(vulkan_windows_launch_spec(&package, None).is_err());
        assert!(vulkan_windows_launch_spec(&package, Some(OsString::new())).is_err());

        // Spec containing PATH
        let mut path_spec = good_spec.clone();
        path_spec
            .environment
            .insert(OsString::from("Path"), OsString::from("C:\\bad"));
        assert!(!validate_launch_spec_shape(&path_spec));

        // Invalid timeout
        let mut timeout_spec = good_spec.clone();
        timeout_spec.timeout = Duration::from_secs(5);
        assert!(!validate_launch_spec_shape(&timeout_spec));

        // Nonzero stdin
        let mut stdin_spec = good_spec.clone();
        stdin_spec.stdin = vec![1];
        assert!(!validate_launch_spec_shape(&stdin_spec));

        // Invalid byte limit
        let mut limit_spec = good_spec.clone();
        limit_spec.stdout_limit_bytes = 1024;
        assert!(!validate_launch_spec_shape(&limit_spec));
    }

    #[test]
    fn observe_windows_vulkan_refuses_spec_with_path_in_env_without_launching() {
        let launch_calls = Cell::new(0);
        let root = PathBuf::from("/opt/solstone");
        let package = VerifiedVulkanPackage {
            package_root: root.clone(),
            executable: root.join(WINDOWS_VULKAN_PROBE),
            loader: root.join(WINDOWS_VULKAN_LOADER),
        };
        let mut spec =
            vulkan_windows_launch_spec(&package, Some(OsString::from("C:\\Windows"))).unwrap();
        spec.environment
            .insert(OsString::from("Path"), OsString::from("C:\\bad"));
        let obs = observe_windows_vulkan(
            || Ok(spec),
            |_| {
                launch_calls.set(launch_calls.get() + 1);
                WindowsProbeFinish::Completed {
                    exit_code: 0,
                    stdout: b"[]\n".to_vec(),
                    quiescent: true,
                }
            },
        );
        assert!(!obs.succeeded);
        assert!(obs.devices.is_empty());
        assert_eq!(launch_calls.get(), 0);
    }

    #[test]
    fn decoder_table_handles_success_empty_and_all_failure_modes() {
        let make_spec = || {
            let root = PathBuf::from("/opt/solstone");
            let package = VerifiedVulkanPackage {
                package_root: root.clone(),
                executable: root.join(WINDOWS_VULKAN_PROBE),
                loader: root.join(WINDOWS_VULKAN_LOADER),
            };
            vulkan_windows_launch_spec(&package, Some(OsString::from("C:\\Windows"))).unwrap()
        };

        let dev1 = VulkanDevice {
            index: 0,
            name: "RTX 4090".to_owned(),
            device_type: Some(2),
            vram_mib: 24576,
        };
        let dev2 = VulkanDevice {
            index: 1,
            name: "Intel Graphics".to_owned(),
            device_type: Some(1),
            vram_mib: 2048,
        };

        // 1. Populated success
        {
            let obs = observe_windows_vulkan(
                || Ok(make_spec()),
                |_| WindowsProbeFinish::Completed {
                    exit_code: 0,
                    stdout: serde_json::to_vec(&vec![dev1.clone(), dev2.clone()]).unwrap(),
                    quiescent: true,
                },
            );
            assert_eq!(
                obs,
                VulkanObservation {
                    devices: vec![dev1.clone(), dev2.clone()],
                    succeeded: true,
                }
            );
        }

        // 2. Empty success
        {
            let obs = observe_windows_vulkan(
                || Ok(make_spec()),
                |_| WindowsProbeFinish::Completed {
                    exit_code: 0,
                    stdout: b"[]\n".to_vec(),
                    quiescent: true,
                },
            );
            assert_eq!(
                obs,
                VulkanObservation {
                    devices: Vec::new(),
                    succeeded: true,
                }
            );
        }

        // 3. Nonzero exit with stdout "[]" -> failure with empty devices
        {
            let obs = observe_windows_vulkan(
                || Ok(make_spec()),
                |_| WindowsProbeFinish::Completed {
                    exit_code: 1,
                    stdout: b"[]\n".to_vec(),
                    quiescent: true,
                },
            );
            assert_eq!(
                obs,
                VulkanObservation {
                    devices: Vec::new(),
                    succeeded: false,
                }
            );
        }

        // 4. Quiescent false -> failure
        {
            let obs = observe_windows_vulkan(
                || Ok(make_spec()),
                |_| WindowsProbeFinish::Completed {
                    exit_code: 0,
                    stdout: serde_json::to_vec(&vec![dev1.clone()]).unwrap(),
                    quiescent: false,
                },
            );
            assert_eq!(
                obs,
                VulkanObservation {
                    devices: Vec::new(),
                    succeeded: false,
                }
            );
        }

        // 5. Incomplete, TimedOut, Overflow
        for finish in [
            WindowsProbeFinish::Incomplete,
            WindowsProbeFinish::TimedOut,
            WindowsProbeFinish::Overflow,
        ] {
            let obs = observe_windows_vulkan(|| Ok(make_spec()), |_| finish);
            assert_eq!(
                obs,
                VulkanObservation {
                    devices: Vec::new(),
                    succeeded: false,
                }
            );
        }

        // 6. Malformed JSON
        {
            let obs = observe_windows_vulkan(
                || Ok(make_spec()),
                |_| WindowsProbeFinish::Completed {
                    exit_code: 0,
                    stdout: b"not json".to_vec(),
                    quiescent: true,
                },
            );
            assert_eq!(
                obs,
                VulkanObservation {
                    devices: Vec::new(),
                    succeeded: false,
                }
            );
        }

        // 7. Object missing device_type
        {
            let json_missing_type = b"[{\"index\":0,\"name\":\"GPU\",\"vram_mib\":1024}]";
            let obs = observe_windows_vulkan(
                || Ok(make_spec()),
                |_| WindowsProbeFinish::Completed {
                    exit_code: 0,
                    stdout: json_missing_type.to_vec(),
                    quiescent: true,
                },
            );
            assert_eq!(
                obs,
                VulkanObservation {
                    devices: Vec::new(),
                    succeeded: false,
                }
            );
        }

        // 8. Verify error -> launch count 0
        {
            let launch_calls = Cell::new(0);
            let obs = observe_windows_vulkan(
                || Err(()),
                |_| {
                    launch_calls.set(launch_calls.get() + 1);
                    WindowsProbeFinish::Completed {
                        exit_code: 0,
                        stdout: b"[]\n".to_vec(),
                        quiescent: true,
                    }
                },
            );
            assert_eq!(
                obs,
                VulkanObservation {
                    devices: Vec::new(),
                    succeeded: false,
                }
            );
            assert_eq!(launch_calls.get(), 0);
        }
    }

    #[test]
    fn observe_windows_vulkan_recovers_on_second_call_without_cache_reset() {
        let verify_calls = Cell::new(0);
        let launch_calls = Cell::new(0);

        let make_spec = || {
            let package = VerifiedVulkanPackage {
                package_root: PathBuf::from("/opt/solstone"),
                executable: PathBuf::from("/opt/solstone/bin/probe.exe"),
                loader: PathBuf::from("/opt/solstone/bin/vulkan-1.dll"),
            };
            vulkan_windows_launch_spec(&package, Some(OsString::from("C:\\Windows"))).unwrap()
        };

        // First call: verify fails
        let obs1 = observe_windows_vulkan(
            || {
                verify_calls.set(verify_calls.get() + 1);
                Err(())
            },
            |_| {
                launch_calls.set(launch_calls.get() + 1);
                WindowsProbeFinish::Incomplete
            },
        );
        assert!(!obs1.succeeded);
        assert_eq!(verify_calls.get(), 1);
        assert_eq!(launch_calls.get(), 0);

        // Second call: verify succeeds and launch succeeds
        let obs2 = observe_windows_vulkan(
            || {
                verify_calls.set(verify_calls.get() + 1);
                Ok(make_spec())
            },
            |_| {
                launch_calls.set(launch_calls.get() + 1);
                WindowsProbeFinish::Completed {
                    exit_code: 0,
                    stdout: b"[]\n".to_vec(),
                    quiescent: true,
                }
            },
        );
        assert!(obs2.succeeded);
        assert_eq!(verify_calls.get(), 2);
        assert_eq!(launch_calls.get(), 1);
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod full_package_tests {
    use std::cell::Cell;
    use std::fs;
    use std::io::Cursor;

    use minisign::KeyPair;
    use solstone_core_distribution::manifest_verify::install_test_fixture_pin;
    use solstone_core_distribution::windows_payload::{
        WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE, WINDOWS_VULKAN_LOADER,
        WINDOWS_VULKAN_PROBE, render_windows_payload_manifest,
    };

    use super::*;

    const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const LOCK: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    static TEST_KEYPAIR: std::sync::LazyLock<KeyPair> = std::sync::LazyLock::new(|| {
        let keypair = KeyPair::generate_unencrypted_keypair().unwrap();
        let pin_dir = tempfile::tempdir().unwrap();
        let pin = pin_dir.path().join("payload.pub");
        fs::write(&pin, keypair.pk.to_box().unwrap().to_bytes()).unwrap();
        install_test_fixture_pin(&pin).unwrap();
        keypair
    });

    fn create_signed_tree(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let keypair = &*TEST_KEYPAIR;
        let root = tempfile::tempdir().expect("temp root");
        for (rel, content) in files {
            let path = root.path().join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, content).unwrap();
        }
        let manifest_bytes = render_windows_payload_manifest(root.path(), COMMIT, LOCK).unwrap();
        let manifest_path = root.path().join(WINDOWS_PAYLOAD_MANIFEST);
        fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        fs::write(&manifest_path, &manifest_bytes).unwrap();

        let signature =
            minisign::sign(None, &keypair.sk, Cursor::new(&manifest_bytes), None, None).unwrap();
        let sig_path = root.path().join(WINDOWS_PAYLOAD_SIGNATURE);
        fs::write(&sig_path, signature.to_bytes()).unwrap();

        root
    }

    #[test]
    fn signed_payload_verification_guards_execution_on_missing_or_corrupt_members() {
        // 1. Missing package
        {
            let non_existent_exe = PathBuf::from("/nonexistent/package/bin/probe.exe");
            let launch_calls = Cell::new(0);
            let obs = observe_windows_vulkan(
                || {
                    let pkg =
                        verified_windows_vulkan_package_at(&non_existent_exe).map_err(|_| ())?;
                    vulkan_windows_launch_spec(&pkg, Some(OsString::from("C:\\Windows")))
                },
                |_| {
                    launch_calls.set(launch_calls.get() + 1);
                    WindowsProbeFinish::Incomplete
                },
            );
            assert!(!obs.succeeded);
            assert_eq!(launch_calls.get(), 0);
        }

        // 2. Signed tree missing loader
        {
            let root = create_signed_tree(&[(WINDOWS_VULKAN_PROBE, b"probe exe")]);
            let exe = root.path().join(WINDOWS_VULKAN_PROBE);
            let launch_calls = Cell::new(0);
            let obs = observe_windows_vulkan(
                || {
                    let pkg = verified_windows_vulkan_package_at(&exe).map_err(|_| ())?;
                    vulkan_windows_launch_spec(&pkg, Some(OsString::from("C:\\Windows")))
                },
                |_| {
                    launch_calls.set(launch_calls.get() + 1);
                    WindowsProbeFinish::Incomplete
                },
            );
            assert!(!obs.succeeded);
            assert_eq!(launch_calls.get(), 0);
        }

        // 3. Signed tree missing probe
        {
            let root = create_signed_tree(&[(WINDOWS_VULKAN_LOADER, b"loader dll")]);
            let exe = root.path().join(WINDOWS_VULKAN_PROBE);
            let launch_calls = Cell::new(0);
            let obs = observe_windows_vulkan(
                || {
                    let pkg = verified_windows_vulkan_package_at(&exe).map_err(|_| ())?;
                    vulkan_windows_launch_spec(&pkg, Some(OsString::from("C:\\Windows")))
                },
                |_| {
                    launch_calls.set(launch_calls.get() + 1);
                    WindowsProbeFinish::Incomplete
                },
            );
            assert!(!obs.succeeded);
            assert_eq!(launch_calls.get(), 0);
        }

        // 4. Digest mismatch (tampered file after manifest creation)
        {
            let root = create_signed_tree(&[
                (WINDOWS_VULKAN_PROBE, b"probe exe"),
                (WINDOWS_VULKAN_LOADER, b"loader dll"),
            ]);
            fs::write(root.path().join(WINDOWS_VULKAN_LOADER), b"tampered").unwrap();
            let exe = root.path().join(WINDOWS_VULKAN_PROBE);
            let launch_calls = Cell::new(0);
            let obs = observe_windows_vulkan(
                || {
                    let pkg = verified_windows_vulkan_package_at(&exe).map_err(|_| ())?;
                    vulkan_windows_launch_spec(&pkg, Some(OsString::from("C:\\Windows")))
                },
                |_| {
                    launch_calls.set(launch_calls.get() + 1);
                    WindowsProbeFinish::Incomplete
                },
            );
            assert!(!obs.succeeded);
            assert_eq!(launch_calls.get(), 0);
        }

        // 5. Valid signed tree listing both members -> verify succeeds and launch called once
        {
            let root = create_signed_tree(&[
                (WINDOWS_VULKAN_PROBE, b"probe exe"),
                (WINDOWS_VULKAN_LOADER, b"loader dll"),
            ]);
            let exe = root.path().join(WINDOWS_VULKAN_PROBE);
            let launch_calls = Cell::new(0);

            let obs = observe_windows_vulkan(
                || {
                    let pkg = verified_windows_vulkan_package_at(&exe).map_err(|_| ())?;
                    vulkan_windows_launch_spec(&pkg, Some(OsString::from("C:\\Windows")))
                },
                |spec| {
                    launch_calls.set(launch_calls.get() + 1);
                    assert_eq!(spec.executable, root.path().join(WINDOWS_VULKAN_PROBE));
                    assert_eq!(spec.loader, root.path().join(WINDOWS_VULKAN_LOADER));
                    WindowsProbeFinish::Completed {
                        exit_code: 0,
                        stdout: b"[]\n".to_vec(),
                        quiescent: true,
                    }
                },
            );
            assert!(obs.succeeded);
            assert_eq!(launch_calls.get(), 1);
        }
    }
}
