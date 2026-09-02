// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Vulkan device discovery for the local provider.
//!
//! The production probe is a separately packaged glibc sibling helper. This
//! wave makes the Rust probe shippable so a later wave can select it: Rust
//! `detect_gpus`/`gpu_probe_ok` have no external Rust caller today, and Python
//! `local_vulkan.py` still owns production probing, so owner-visible behaviour
//! does not change here.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::VulkanDevice;

const VK_TYPE_INTEGRATED: u32 = 1;
const VK_TYPE_DISCRETE: u32 = 2;
const VK_TYPE_VIRTUAL: u32 = 3;
const VK_TYPE_CPU: u32 = 4;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const SOFTWARE_NAME_SUBSTRINGS: [&str; 3] = ["llvmpipe", "lavapipe", "swiftshader"];
const HELPER: &str = "solstone-core-vulkan-probe";

/// Owner-facing advisory copy for forced CPU transcription placement.
pub const CPU_PLACEMENT_COPY: &str =
    "a model runs on your GPU; transcription runs on your CPU on this machine";

static DETECT_CACHE: Mutex<Option<(Vec<VulkanDevice>, bool)>> = Mutex::new(None);

#[cfg(test)]
static TEST_HELPER_BASE_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
#[cfg(test)]
static TEST_PROBE_SERIAL: Mutex<()> = Mutex::new(());

/// Program resolution for the isolated Vulkan probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VulkanProbeProgram {
    /// Resolve the packaged sibling helper beside the current executable.
    SiblingHelper,
    /// Run an explicitly supplied child program for direct protocol tests.
    Explicit {
        executable: PathBuf,
        args: Vec<OsString>,
        env: Vec<(OsString, OsString)>,
    },
}

/// Configuration for a non-memoized Vulkan child probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VulkanProbeConfig {
    pub program: VulkanProbeProgram,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedProbeProgram {
    executable: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolveProbeProgramError {
    CurrentExeUnavailable,
    CurrentExeParentMissing,
    HelperMissing(PathBuf),
    HelperNotExecutable(PathBuf),
}

impl Default for VulkanProbeConfig {
    fn default() -> Self {
        Self {
            program: VulkanProbeProgram::SiblingHelper,
            timeout: PROBE_TIMEOUT,
        }
    }
}
/// Perform a child probe without reading or writing the memoized snapshot.
pub fn enumerate_gpus(config: &VulkanProbeConfig) -> (Vec<VulkanDevice>, bool) {
    let program = match resolve_program(&config.program) {
        Ok(resolved) => resolved,
        Err(_) => return (Vec::new(), false),
    };
    let mut child = match Command::new(program.executable)
        .args(program.args)
        .envs(program.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return (Vec::new(), false),
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = match child.wait_with_output() {
                    Ok(output) => output,
                    Err(_) => return (Vec::new(), false),
                };
                if !status.success() {
                    return (Vec::new(), false);
                }
                return match serde_json::from_slice::<Vec<VulkanDevice>>(&output.stdout) {
                    // The supervisor's legacy plan payload may omit device_type,
                    // but the probe-child protocol must contain all four fields.
                    Ok(devices) if devices.iter().all(|device| device.device_type.is_some()) => {
                        (devices, true)
                    }
                    Ok(_) | Err(_) => (Vec::new(), false),
                };
            }
            Ok(None) if started.elapsed() >= config.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return (Vec::new(), false);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => return (Vec::new(), false),
        }
    }
}

/// Return the memoized device list, cloning the one shared probe snapshot.
pub fn detect_gpus() -> Vec<VulkanDevice> {
    cached_probe().0
}

/// Return the memoized child-probe completion status from the same snapshot.
pub fn gpu_probe_ok() -> bool {
    cached_probe().1
}

fn cached_probe() -> (Vec<VulkanDevice>, bool) {
    let mut cache = lock_cache();
    if cache.is_none() {
        *cache = Some(enumerate_gpus(&VulkanProbeConfig::default()));
    }
    cache.as_ref().expect("cache initialized").clone()
}

fn lock_cache() -> MutexGuard<'static, Option<(Vec<VulkanDevice>, bool)>> {
    DETECT_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
pub(crate) struct VulkanProbeTestGuard {
    _serial: MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for VulkanProbeTestGuard {
    fn drop(&mut self) {
        *TEST_HELPER_BASE_DIR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *lock_cache() = None;
    }
}

#[cfg(test)]
pub(crate) fn reset_detect_cache(helper_base_dir: Option<PathBuf>) -> VulkanProbeTestGuard {
    let serial = TEST_PROBE_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *TEST_HELPER_BASE_DIR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = helper_base_dir;
    *lock_cache() = None;
    VulkanProbeTestGuard { _serial: serial }
}

fn resolve_program(
    program: &VulkanProbeProgram,
) -> Result<ResolvedProbeProgram, ResolveProbeProgramError> {
    match program {
        // Keep Vulkan enumeration in its own glibc helper rather than solstone-core-speakers-analyze. The speaker helper has a DT_NEEDED libonnxruntime and a pinned DT_RUNPATH; coupling this probe to it would make GPU discovery fail when the unrelated audio runtime cannot load. This leaf has a one-time packaging cost but keeps the loader dependency boundary honest.
        // `CurrentExecutable` and the private child argument/environment were deliberately removed: the static-musl solstone-core binary must never be a fallback Vulkan loader. The only production program is the packaged sibling helper; absence is a failed probe, not an empty successful enumeration.
        VulkanProbeProgram::SiblingHelper => {
            let executable = std::env::current_exe()
                .map_err(|_| ResolveProbeProgramError::CurrentExeUnavailable)?;
            #[cfg(test)]
            let base_dir = TEST_HELPER_BASE_DIR
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .or_else(|| executable.parent().map(PathBuf::from));
            #[cfg(not(test))]
            let base_dir = executable.parent().map(PathBuf::from);
            let path = base_dir
                .ok_or(ResolveProbeProgramError::CurrentExeParentMissing)?
                .join(HELPER);
            let metadata = fs::metadata(&path)
                .map_err(|_| ResolveProbeProgramError::HelperMissing(path.clone()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return Err(ResolveProbeProgramError::HelperNotExecutable(path));
                }
            }
            #[cfg(not(unix))]
            let _ = metadata;
            Ok(ResolvedProbeProgram {
                executable: path,
                args: Vec::new(),
                env: Vec::new(),
            })
        }
        VulkanProbeProgram::Explicit {
            executable,
            args,
            env,
        } => Ok(ResolvedProbeProgram {
            executable: executable.clone(),
            args: args.clone(),
            env: env.clone(),
        }),
    }
}

/// True for integrated/discrete Vulkan hardware, excluding known software ICDs.
pub fn is_hardware_device(device: &VulkanDevice) -> bool {
    let name = device.name.to_lowercase();
    if SOFTWARE_NAME_SUBSTRINGS
        .iter()
        .any(|substring| name.contains(substring))
    {
        return false;
    }
    matches!(
        device.device_type,
        Some(VK_TYPE_INTEGRATED | VK_TYPE_DISCRETE)
    )
}

/// Select an explicitly overridden hardware index, or the first discrete then integrated device.
pub fn select_device(
    devices: &[VulkanDevice],
    override_index: Option<u32>,
) -> Option<VulkanDevice> {
    if let Some(override_index) = override_index {
        return devices
            .iter()
            .find(|device| device.index == override_index && is_hardware_device(device))
            .cloned();
    }
    [VK_TYPE_DISCRETE, VK_TYPE_INTEGRATED]
        .into_iter()
        .find_map(|device_type| {
            devices
                .iter()
                .filter(|device| {
                    is_hardware_device(device) && device.device_type == Some(device_type)
                })
                .min_by_key(|device| device.index)
                .cloned()
        })
}

/// Return the same device classification labels as Python local_vulkan.py.
pub fn classify(device: &VulkanDevice) -> &'static str {
    let name = device.name.to_lowercase();
    if SOFTWARE_NAME_SUBSTRINGS
        .iter()
        .any(|substring| name.contains(substring))
    {
        return "software";
    }
    match device.device_type {
        Some(VK_TYPE_DISCRETE) => "discrete",
        Some(VK_TYPE_INTEGRATED) => "integrated",
        Some(VK_TYPE_CPU) => "cpu",
        Some(VK_TYPE_VIRTUAL) => "virtual",
        _ => "other",
    }
}

/// Return whether a pre-enumerated device classifies as discrete.
pub fn is_discrete(device: &VulkanDevice) -> bool {
    classify(device) == "discrete"
}

/// Count devices which are both hardware and discrete.
pub fn discrete_hardware_gpu_count(devices: &[VulkanDevice]) -> u32 {
    devices
        .iter()
        .filter(|device| is_hardware_device(device) && is_discrete(device))
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

/// Return the CPU-transcription advisory after a caller has made placement.
pub fn cpu_placement_suffix(selected: Option<&VulkanDevice>, force_cpu: bool) -> String {
    if selected.is_some() && force_cpu {
        format!("; {CPU_PLACEMENT_COPY}")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str =
        include_str!("../../../../tests/fixtures/llama_server/vulkan_devices.json");

    fn device(index: u32, name: &str, device_type: Option<u32>, vram_mib: u64) -> VulkanDevice {
        VulkanDevice {
            index,
            name: name.into(),
            device_type,
            vram_mib,
        }
    }

    #[test]
    fn fixture_json_prefers_discrete_despite_larger_integrated_vram() {
        let devices: Vec<VulkanDevice> = serde_json::from_str(FIXTURE).expect("fixture JSON");
        assert_eq!(select_device(&devices, None).expect("selected").index, 1);
        assert_eq!(devices[0].vram_mib, 23_814);
        assert_eq!(devices[1].vram_mib, 6_390);
        assert_eq!(classify(&devices[2]), "software");
    }

    #[test]
    fn selection_uses_raw_index_and_override_never_falls_back() {
        let devices = vec![
            device(0, "Intel", Some(VK_TYPE_INTEGRATED), 1),
            device(3, "NVIDIA B", Some(VK_TYPE_DISCRETE), 1),
            device(2, "NVIDIA A", Some(VK_TYPE_DISCRETE), 1),
            device(1, "llvmpipe", Some(VK_TYPE_DISCRETE), 1),
        ];
        assert_eq!(select_device(&devices, None).expect("selected").index, 2);
        assert_eq!(select_device(&devices, Some(0)).expect("override").index, 0);
        assert_eq!(select_device(&devices, Some(1)), None);
        assert_eq!(select_device(&devices, Some(99)), None);
    }

    #[test]
    fn software_name_precedes_device_type_and_non_hardware_types_reject() {
        let rejected = [
            device(0, "SwiftShader Device", Some(VK_TYPE_DISCRETE), 1),
            device(1, "virtual", Some(VK_TYPE_VIRTUAL), 1),
            device(2, "cpu", Some(VK_TYPE_CPU), 1),
            device(3, "other", Some(0), 1),
        ];
        assert!(rejected.iter().all(|device| !is_hardware_device(device)));
        assert_eq!(classify(&rejected[0]), "software");
        assert_eq!(classify(&rejected[1]), "virtual");
        assert_eq!(classify(&rejected[2]), "cpu");
        assert_eq!(classify(&rejected[3]), "other");
    }

    #[test]
    fn placement_helpers_match_the_constructed_device_contract() {
        let discrete = device(0, "GPU", Some(VK_TYPE_DISCRETE), 6_144);
        let integrated = device(1, "iGPU", Some(VK_TYPE_INTEGRATED), 1);
        let software = device(2, "llvmpipe", Some(VK_TYPE_CPU), 0);
        assert!(is_discrete(&discrete));
        assert_eq!(
            discrete_hardware_gpu_count(&[discrete.clone(), integrated, software]),
            1
        );
        assert_eq!(
            cpu_placement_suffix(Some(&discrete), true),
            format!("; {CPU_PLACEMENT_COPY}")
        );
        assert!(cpu_placement_suffix(Some(&discrete), false).is_empty());
        assert!(cpu_placement_suffix(None, true).is_empty());
    }

    #[test]
    fn test_only_cache_reset_is_available_to_unit_tests() {
        let _guard = reset_detect_cache(None);
    }

    #[test]
    fn memoized_probe_reports_missing_sibling_as_failed() {
        let _guard = reset_detect_cache(None);
        assert!(!gpu_probe_ok());
        assert!(detect_gpus().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn memoized_probe_uses_test_sibling_helper() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary helper directory");
        let helper = directory.path().join(HELPER);
        fs::write(&helper, "#!/bin/sh\nprintf '[]\\n'\n").expect("write helper");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755))
            .expect("make helper executable");
        let _guard = reset_detect_cache(Some(directory.path().to_path_buf()));
        assert!(gpu_probe_ok());
        assert!(detect_gpus().is_empty());
    }

    #[test]
    fn default_program_resolves_only_the_sibling_helper() {
        let _guard = reset_detect_cache(None);
        match resolve_program(&VulkanProbeConfig::default().program) {
            Ok(program) => assert!(program.executable.ends_with(HELPER)),
            Err(ResolveProbeProgramError::HelperMissing(path)) => {
                assert!(path.ends_with(HELPER));
            }
            Err(error) => panic!("unexpected helper resolution error: {error:?}"),
        }
    }

    #[test]
    fn non_memoized_child_contract_distinguishes_clean_empty_from_failures() {
        let clean_empty = VulkanProbeConfig {
            program: VulkanProbeProgram::Explicit {
                executable: PathBuf::from("sh"),
                args: vec!["-c".into(), "printf '[]'".into()],
                env: Vec::new(),
            },
            timeout: Duration::from_secs(1),
        };
        assert_eq!(enumerate_gpus(&clean_empty), (Vec::new(), true));

        let invalid_json = VulkanProbeConfig {
            program: VulkanProbeProgram::Explicit {
                executable: PathBuf::from("sh"),
                args: vec!["-c".into(), "printf '{'".into()],
                env: Vec::new(),
            },
            timeout: Duration::from_secs(1),
        };
        assert_eq!(enumerate_gpus(&invalid_json), (Vec::new(), false));

        let legacy_payload = VulkanProbeConfig {
            program: VulkanProbeProgram::Explicit {
                executable: PathBuf::from("sh"),
                args: vec![
                    "-c".into(),
                    "printf '[{\"index\":0,\"name\":\"GPU\",\"vram_mib\":1}]'".into(),
                ],
                env: Vec::new(),
            },
            timeout: Duration::from_secs(1),
        };
        assert_eq!(enumerate_gpus(&legacy_payload), (Vec::new(), false));

        let timeout = VulkanProbeConfig {
            program: VulkanProbeProgram::Explicit {
                executable: PathBuf::from("sh"),
                args: vec!["-c".into(), "sleep 1".into()],
                env: Vec::new(),
            },
            timeout: Duration::ZERO,
        };
        assert_eq!(enumerate_gpus(&timeout), (Vec::new(), false));
    }
}
