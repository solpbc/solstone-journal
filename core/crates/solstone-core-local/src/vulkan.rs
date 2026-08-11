// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Vulkan device discovery for the local provider.
//!
//! The default child is this executable re-invoked in a hidden mode. On the
//! shipped static-musl Linux build it cannot `dlopen` a host glibc
//! `libvulkan.so.1`; that child returns `[]` with exit 0, yielding `([], true)`.
//! This is indistinguishable from a real no-GPU host and silently wrong on a
//! GPU host. The check wave must replace the program resolution with the
//! separate glibc helper before anything selects this path.

use std::ffi::{OsString, c_char, c_void};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use libloading::Library;

use crate::VulkanDevice;

const VK_TYPE_INTEGRATED: u32 = 1;
const VK_TYPE_DISCRETE: u32 = 2;
const VK_TYPE_VIRTUAL: u32 = 3;
const VK_TYPE_CPU: u32 = 4;
const VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO: u32 = 1;
const VK_DEVICE_LOCAL_BIT: u32 = 0x0000_0001;
const VK_PHYSICAL_DEVICE_NAME_SIZE: usize = 256;
const VK_MAX_MEMORY_TYPES: usize = 32;
const VK_MAX_MEMORY_HEAPS: usize = 16;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const SOFTWARE_NAME_SUBSTRINGS: [&str; 3] = ["llvmpipe", "lavapipe", "swiftshader"];
#[doc(hidden)]
pub const VULKAN_PROBE_CHILD_ARG: &str = "--solstone-vulkan-probe-child";

/// Owner-facing advisory copy for forced CPU transcription placement.
pub const CPU_PLACEMENT_COPY: &str =
    "sol thinks on your GPU; transcription runs on your CPU on this machine";

static DETECT_CACHE: Mutex<Option<(Vec<VulkanDevice>, bool)>> = Mutex::new(None);

/// Program resolution for the isolated Vulkan probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VulkanProbeProgram {
    /// Re-invoke the current executable in the private child mode.
    CurrentExecutable,
    /// Run an explicitly supplied child program. This is the seam a future
    /// sibling glibc helper uses without changing probe callers.
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

struct ResolvedProbeProgram {
    executable: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
}

impl Default for VulkanProbeConfig {
    fn default() -> Self {
        Self {
            program: VulkanProbeProgram::CurrentExecutable,
            timeout: PROBE_TIMEOUT,
        }
    }
}

#[repr(C)]
struct VkInstanceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    p_application_info: *const c_void,
    enabled_layer_count: u32,
    pp_enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    pp_enabled_extension_names: *const *const c_char,
}

#[repr(C, align(8))]
struct VkPhysicalDeviceProperties {
    api_version: u32,
    driver_version: u32,
    vendor_id: u32,
    device_id: u32,
    device_type: u32,
    device_name: [c_char; VK_PHYSICAL_DEVICE_NAME_SIZE],
    // Vulkan writes the rest of VkPhysicalDeviceProperties after device_name.
    // This matches Python's deliberately oversized ctypes tail without binding
    // the unneeded limits/sparse-properties fields.
    _tail: [u8; 8192],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VkMemoryType {
    property_flags: u32,
    heap_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VkMemoryHeap {
    size: u64,
    flags: u32,
}

#[repr(C)]
struct VkPhysicalDeviceMemoryProperties {
    memory_type_count: u32,
    memory_types: [VkMemoryType; VK_MAX_MEMORY_TYPES],
    memory_heap_count: u32,
    memory_heaps: [VkMemoryHeap; VK_MAX_MEMORY_HEAPS],
}

type VkInstance = *mut c_void;
type VkPhysicalDevice = *mut c_void;
type VkResult = i32;
type VkCreateInstance = unsafe extern "system" fn(
    *const VkInstanceCreateInfo,
    *const c_void,
    *mut VkInstance,
) -> VkResult;
type VkDestroyInstance = unsafe extern "system" fn(VkInstance, *const c_void);
type VkEnumeratePhysicalDevices =
    unsafe extern "system" fn(VkInstance, *mut u32, *mut VkPhysicalDevice) -> VkResult;
type VkGetPhysicalDeviceProperties =
    unsafe extern "system" fn(VkPhysicalDevice, *mut VkPhysicalDeviceProperties);
type VkGetPhysicalDeviceMemoryProperties =
    unsafe extern "system" fn(VkPhysicalDevice, *mut VkPhysicalDeviceMemoryProperties);

struct VulkanFns {
    // Keep the library live for the full lifetime of all copied function pointers.
    _library: Library,
    create_instance: VkCreateInstance,
    destroy_instance: VkDestroyInstance,
    enumerate_physical_devices: VkEnumeratePhysicalDevices,
    get_physical_device_properties: VkGetPhysicalDeviceProperties,
    get_physical_device_memory_properties: VkGetPhysicalDeviceMemoryProperties,
}

impl VulkanFns {
    fn load() -> Result<Self, ()> {
        // `libloading` is already in the solstone-core shipping closure. Keeping
        // this to five hand-declared symbols rather than adding ash minimizes the
        // dependency/API covenant for this deliberately small Vulkan surface.
        let library = unsafe { Library::new("libvulkan.so.1") }.map_err(|_| ())?;
        let create_instance = unsafe {
            *library
                .get::<VkCreateInstance>(b"vkCreateInstance\0")
                .map_err(|_| ())?
        };
        let destroy_instance = unsafe {
            *library
                .get::<VkDestroyInstance>(b"vkDestroyInstance\0")
                .map_err(|_| ())?
        };
        let enumerate_physical_devices = unsafe {
            *library
                .get::<VkEnumeratePhysicalDevices>(b"vkEnumeratePhysicalDevices\0")
                .map_err(|_| ())?
        };
        let get_physical_device_properties = unsafe {
            *library
                .get::<VkGetPhysicalDeviceProperties>(b"vkGetPhysicalDeviceProperties\0")
                .map_err(|_| ())?
        };
        let get_physical_device_memory_properties = unsafe {
            *library
                .get::<VkGetPhysicalDeviceMemoryProperties>(
                    b"vkGetPhysicalDeviceMemoryProperties\0",
                )
                .map_err(|_| ())?
        };
        Ok(Self {
            _library: library,
            create_instance,
            destroy_instance,
            enumerate_physical_devices,
            get_physical_device_properties,
            get_physical_device_memory_properties,
        })
    }
}

/// Enumerate Vulkan devices in the current process.
fn enumerate_in_process() -> Vec<VulkanDevice> {
    let functions = match VulkanFns::load() {
        Ok(functions) => functions,
        // Python's local_vulkan.py contract intentionally converts loader and
        // Vulkan failures into an empty successful result. This is parity, not
        // a new exception to CLAUDE.md's normal fail-loudly rule.
        Err(()) => return Vec::new(),
    };
    let mut instance = std::ptr::null_mut();
    let result = (|| -> Result<Vec<VulkanDevice>, ()> {
        let create_info = VkInstanceCreateInfo {
            s_type: VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            p_application_info: std::ptr::null(),
            enabled_layer_count: 0,
            pp_enabled_layer_names: std::ptr::null(),
            enabled_extension_count: 0,
            pp_enabled_extension_names: std::ptr::null(),
        };
        if unsafe { (functions.create_instance)(&create_info, std::ptr::null(), &mut instance) }
            != 0
        {
            return Err(());
        }

        let mut count = 0_u32;
        if unsafe {
            (functions.enumerate_physical_devices)(instance, &mut count, std::ptr::null_mut())
        } != 0
            || count == 0
        {
            return Err(());
        }
        let count = usize::try_from(count).map_err(|_| ())?;
        let mut raw_devices = Vec::new();
        raw_devices.try_reserve_exact(count).map_err(|_| ())?;
        raw_devices.resize(count, std::ptr::null_mut());
        let mut returned_count = u32::try_from(count).map_err(|_| ())?;
        if unsafe {
            (functions.enumerate_physical_devices)(
                instance,
                &mut returned_count,
                raw_devices.as_mut_ptr(),
            )
        } != 0
        {
            return Err(());
        }
        let returned_count = usize::try_from(returned_count)
            .map_err(|_| ())?
            .min(raw_devices.len());
        let mut devices = Vec::new();
        devices.try_reserve_exact(returned_count).map_err(|_| ())?;
        for (index, raw_device) in raw_devices.into_iter().take(returned_count).enumerate() {
            let mut properties = VkPhysicalDeviceProperties {
                api_version: 0,
                driver_version: 0,
                vendor_id: 0,
                device_id: 0,
                device_type: 0,
                device_name: [0; VK_PHYSICAL_DEVICE_NAME_SIZE],
                _tail: [0; 8192],
            };
            unsafe { (functions.get_physical_device_properties)(raw_device, &mut properties) };
            let name_bytes = properties
                .device_name
                .iter()
                .map(|byte| *byte as u8)
                .take_while(|byte| *byte != 0)
                .collect::<Vec<_>>();
            let mut memory = VkPhysicalDeviceMemoryProperties {
                memory_type_count: 0,
                memory_types: [VkMemoryType {
                    property_flags: 0,
                    heap_index: 0,
                }; VK_MAX_MEMORY_TYPES],
                memory_heap_count: 0,
                memory_heaps: [VkMemoryHeap { size: 0, flags: 0 }; VK_MAX_MEMORY_HEAPS],
            };
            unsafe { (functions.get_physical_device_memory_properties)(raw_device, &mut memory) };
            let heap_count = usize::try_from(memory.memory_heap_count)
                .map_err(|_| ())?
                .min(VK_MAX_MEMORY_HEAPS);
            // Python sums every DEVICE_LOCAL heap's size. heapBudget and the
            // largest heap are both wrong for this enumeration contract.
            let vram_bytes = memory.memory_heaps[..heap_count]
                .iter()
                .filter(|heap| heap.flags & VK_DEVICE_LOCAL_BIT != 0)
                .try_fold(0_u64, |total, heap| total.checked_add(heap.size))
                .ok_or(())?;
            devices.push(VulkanDevice {
                index: u32::try_from(index).map_err(|_| ())?,
                name: String::from_utf8_lossy(&name_bytes).into_owned(),
                device_type: Some(properties.device_type),
                vram_mib: vram_bytes / (1024 * 1024),
            });
        }
        Ok(devices)
    })();
    if !instance.is_null() {
        unsafe { (functions.destroy_instance)(instance, std::ptr::null()) };
    }
    // See the loader branch above: every in-process error is an empty, clean
    // probe result by the ported Python contract.
    result.unwrap_or_default()
}

/// Write the private child protocol: one JSON device array and nothing else.
pub fn write_vulkan_probe_json(mut writer: impl Write) -> io::Result<()> {
    serde_json::to_writer(&mut writer, &enumerate_in_process())?;
    writer.write_all(b"\n")
}

/// Perform a child probe without reading or writing the memoized snapshot.
pub fn enumerate_gpus(config: &VulkanProbeConfig) -> (Vec<VulkanDevice>, bool) {
    let program = match resolve_program(&config.program) {
        Ok(resolved) => resolved,
        Err(()) => return (Vec::new(), false),
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
                    Ok(devices) => (devices, true),
                    Err(_) => (Vec::new(), false),
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
pub(crate) fn reset_detect_cache() {
    *lock_cache() = None;
}

fn resolve_program(program: &VulkanProbeProgram) -> Result<ResolvedProbeProgram, ()> {
    match program {
        VulkanProbeProgram::CurrentExecutable => Ok(ResolvedProbeProgram {
            executable: std::env::current_exe().map_err(|_| ())?,
            args: vec![OsString::from(VULKAN_PROBE_CHILD_ARG)],
            env: Vec::new(),
        }),
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
        reset_detect_cache();
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
