// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Dynamically linked Vulkan device enumeration helper.
//!
//! Linux loads the platform libvulkan.so.1 and emits `[]` with exit 0 on failure.
//! Windows loads the verified declared loader from the signed package and exits nonzero on failure.

use std::ffi::{c_char, c_void};
use std::io::{self, Write};
use std::process::ExitCode;

use libloading::Library;
use serde::Serialize;

const VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO: u32 = 1;
const VK_DEVICE_LOCAL_BIT: u32 = 0x0000_0001;
const VK_PHYSICAL_DEVICE_NAME_SIZE: usize = 256;
const VK_MAX_MEMORY_TYPES: usize = 32;
const VK_MAX_MEMORY_HEAPS: usize = 16;
const MAX_PHYSICAL_DEVICES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct VulkanDevice {
    pub index: u32,
    pub name: String,
    pub device_type: u32,
    pub vram_mib: u64,
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
#[derive(Clone, Copy)]
struct VkPhysicalDeviceProperties {
    api_version: u32,
    driver_version: u32,
    vendor_id: u32,
    device_id: u32,
    device_type: u32,
    device_name: [c_char; VK_PHYSICAL_DEVICE_NAME_SIZE],
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
#[derive(Clone, Copy)]
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
    _library: Library,
    create_instance: VkCreateInstance,
    destroy_instance: VkDestroyInstance,
    enumerate_physical_devices: VkEnumeratePhysicalDevices,
    get_physical_device_properties: VkGetPhysicalDeviceProperties,
    get_physical_device_memory_properties: VkGetPhysicalDeviceMemoryProperties,
}

impl VulkanFns {
    #[cfg(not(windows))]
    fn load_unix() -> Result<Self, ()> {
        let library = unsafe { Library::new("libvulkan.so.1") }.map_err(|_| ())?;
        Self::from_library(library)
    }

    #[cfg(windows)]
    fn load_windows(
        policy: solstone_core_win_dll_load::LoadPolicy,
        path: &std::path::Path,
    ) -> Result<Self, ()> {
        let library = solstone_core_win_dll_load::load_dll(policy, path).map_err(|_| ())?;
        Self::from_library(library)
    }

    fn from_library(library: Library) -> Result<Self, ()> {
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

trait VulkanQuery {
    fn create_instance(&self) -> Result<VkInstance, ()>;
    fn destroy_instance(&self, instance: VkInstance);
    fn enumerate_device_count(&self, instance: VkInstance) -> Result<u32, ()>;
    fn enumerate_device_pointers(
        &self,
        instance: VkInstance,
        count: u32,
        out: &mut [VkPhysicalDevice],
    ) -> Result<u32, ()>;
    fn get_physical_device_properties(
        &self,
        device: VkPhysicalDevice,
        properties: &mut VkPhysicalDeviceProperties,
    );
    fn get_physical_device_memory_properties(
        &self,
        device: VkPhysicalDevice,
        memory: &mut VkPhysicalDeviceMemoryProperties,
    );
}

impl VulkanQuery for VulkanFns {
    fn create_instance(&self) -> Result<VkInstance, ()> {
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
        let mut instance = std::ptr::null_mut();
        if unsafe { (self.create_instance)(&create_info, std::ptr::null(), &mut instance) } != 0 {
            return Err(());
        }
        Ok(instance)
    }

    fn destroy_instance(&self, instance: VkInstance) {
        if !instance.is_null() {
            unsafe { (self.destroy_instance)(instance, std::ptr::null()) };
        }
    }

    fn enumerate_device_count(&self, instance: VkInstance) -> Result<u32, ()> {
        let mut count = 0_u32;
        if unsafe { (self.enumerate_physical_devices)(instance, &mut count, std::ptr::null_mut()) }
            != 0
        {
            return Err(());
        }
        Ok(count)
    }

    fn enumerate_device_pointers(
        &self,
        instance: VkInstance,
        count: u32,
        out: &mut [VkPhysicalDevice],
    ) -> Result<u32, ()> {
        let mut returned_count = count;
        if unsafe {
            (self.enumerate_physical_devices)(instance, &mut returned_count, out.as_mut_ptr())
        } != 0
        {
            return Err(());
        }
        Ok(returned_count)
    }

    fn get_physical_device_properties(
        &self,
        device: VkPhysicalDevice,
        properties: &mut VkPhysicalDeviceProperties,
    ) {
        unsafe { (self.get_physical_device_properties)(device, properties) };
    }

    fn get_physical_device_memory_properties(
        &self,
        device: VkPhysicalDevice,
        memory: &mut VkPhysicalDeviceMemoryProperties,
    ) {
        unsafe { (self.get_physical_device_memory_properties)(device, memory) };
    }
}

fn enumerate_devices_bounded<Q: VulkanQuery>(
    query: &Q,
    capacity: usize,
) -> Result<Vec<VulkanDevice>, ()> {
    let instance = query.create_instance()?;
    let result = (|| -> Result<Vec<VulkanDevice>, ()> {
        let count = query.enumerate_device_count(instance)?;
        if count == 0 {
            return Ok(Vec::new());
        }
        let count = usize::try_from(count).map_err(|_| ())?;
        if count > capacity {
            return Err(());
        }
        let mut raw_devices = Vec::new();
        raw_devices.try_reserve_exact(count).map_err(|_| ())?;
        raw_devices.resize(count, std::ptr::null_mut());
        let returned_count = query.enumerate_device_pointers(
            instance,
            u32::try_from(count).map_err(|_| ())?,
            &mut raw_devices,
        )?;
        let returned_count = usize::try_from(returned_count).map_err(|_| ())?;
        if returned_count > raw_devices.len() {
            return Err(());
        }
        let mut devices = Vec::new();
        devices.try_reserve_exact(returned_count).map_err(|_| ())?;
        for (index, &raw_device) in raw_devices[..returned_count].iter().enumerate() {
            let mut properties = VkPhysicalDeviceProperties {
                api_version: 0,
                driver_version: 0,
                vendor_id: 0,
                device_id: 0,
                device_type: 0,
                device_name: [0; VK_PHYSICAL_DEVICE_NAME_SIZE],
                _tail: [0; 8192],
            };
            query.get_physical_device_properties(raw_device, &mut properties);
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
            query.get_physical_device_memory_properties(raw_device, &mut memory);
            if memory.memory_heap_count as usize > VK_MAX_MEMORY_HEAPS
                || memory.memory_type_count as usize > VK_MAX_MEMORY_TYPES
            {
                return Err(());
            }
            let heap_count = memory.memory_heap_count as usize;
            let vram_bytes = memory.memory_heaps[..heap_count]
                .iter()
                .filter(|heap| heap.flags & VK_DEVICE_LOCAL_BIT != 0)
                .try_fold(0_u64, |total, heap| total.checked_add(heap.size).ok_or(()))?;
            devices.push(VulkanDevice {
                index: u32::try_from(index).map_err(|_| ())?,
                name: String::from_utf8_lossy(&name_bytes).into_owned(),
                device_type: properties.device_type,
                vram_mib: vram_bytes / (1024 * 1024),
            });
        }
        Ok(devices)
    })();
    query.destroy_instance(instance);
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum ProbePlatform {
    Linux,
    Windows,
}

#[derive(Debug, PartialEq, Eq)]
struct ProbeOutput {
    stdout: Vec<u8>,
    exit_code: i32,
}

fn map_enumeration_output(
    platform: ProbePlatform,
    result: Result<Vec<VulkanDevice>, ()>,
) -> ProbeOutput {
    match result {
        Ok(devices) => {
            let mut stdout = serde_json::to_vec(&devices).unwrap_or_else(|_| b"[]".to_vec());
            stdout.push(b'\n');
            ProbeOutput {
                stdout,
                exit_code: 0,
            }
        }
        Err(()) => match platform {
            ProbePlatform::Linux => ProbeOutput {
                stdout: b"[]\n".to_vec(),
                exit_code: 0,
            },
            ProbePlatform::Windows => ProbeOutput {
                stdout: Vec::new(),
                exit_code: 1,
            },
        },
    }
}

#[cfg(any(windows, test))]
fn open_declared_loader<V, C, R, L, T, E>(
    verify: V,
    canonicalize: C,
    restrict: R,
    load: L,
) -> Result<T, E>
where
    V: FnOnce() -> Result<std::path::PathBuf, E>,
    C: FnOnce(&std::path::Path) -> Result<std::path::PathBuf, E>,
    R: FnOnce() -> Result<(), E>,
    L: FnOnce(solstone_core_win_dll_load::LoadPolicy, &std::path::Path) -> Result<T, E>,
{
    let declared_path = verify()?;
    let canonical = canonicalize(&declared_path)?;
    restrict()?;
    load(
        solstone_core_win_dll_load::LoadPolicy::DllLoadDir,
        &canonical,
    )
}

#[cfg(windows)]
fn run_probe() -> ProbeOutput {
    let result = (|| -> Result<Vec<VulkanDevice>, ()> {
        let executable = std::env::current_exe().map_err(|_| ())?;
        let bin = executable.parent().ok_or(())?;
        if bin.file_name() != Some(std::ffi::OsStr::new("bin")) {
            return Err(());
        }
        let package_root = bin.parent().ok_or(())?;
        let payload =
            solstone_core_distribution::windows_payload::verify_windows_payload(package_root)
                .map_err(|_| ())?;
        let functions = open_declared_loader(
            || payload.vulkan_loader_path().map_err(|_| ()),
            |path| std::fs::canonicalize(path).map_err(|_| ()),
            || solstone_core_win_dll_load::restrict_default_dll_directories().map_err(|_| ()),
            |policy, path| VulkanFns::load_windows(policy, path).map_err(|_| ()),
        )?;
        enumerate_devices_bounded(&functions, MAX_PHYSICAL_DEVICES)
    })();
    map_enumeration_output(ProbePlatform::Windows, result)
}

#[cfg(not(windows))]
fn run_probe() -> ProbeOutput {
    let result = (|| -> Result<Vec<VulkanDevice>, ()> {
        let functions = VulkanFns::load_unix()?;
        enumerate_devices_bounded(&functions, MAX_PHYSICAL_DEVICES)
    })();
    map_enumeration_output(ProbePlatform::Linux, result)
}

fn probe_entry<I, S, R>(args: I, run: R) -> ProbeOutput
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
    R: FnOnce() -> ProbeOutput,
{
    let mut iter = args.into_iter();
    if iter.next().as_ref().map(|s| s.as_ref()) == Some(std::ffi::OsStr::new("--version")) {
        let version_line = format!("solstone-core-vulkan-probe {}\n", env!("CARGO_PKG_VERSION"));
        ProbeOutput {
            stdout: version_line.into_bytes(),
            exit_code: 0,
        }
    } else {
        run()
    }
}

fn main() -> ExitCode {
    let output = probe_entry(std::env::args_os().skip(1), run_probe);
    emit_output(output, io::stdout().lock())
}

fn emit_output(output: ProbeOutput, mut writer: impl Write) -> ExitCode {
    if writer.write_all(&output.stdout).is_err() || writer.flush().is_err() {
        return ExitCode::FAILURE;
    }
    if output.exit_code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(u8::try_from(output.exit_code).unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::{Path, PathBuf};

    use super::*;

    struct MockVulkanQuery {
        create_instance_result: Result<VkInstance, ()>,
        enumerate_count_result: Result<u32, ()>,
        enumerate_pointers_result: Result<u32, ()>,
        buffer_requested: Cell<bool>,
        device_properties: Vec<VkPhysicalDeviceProperties>,
        device_memory: Vec<VkPhysicalDeviceMemoryProperties>,
        destroyed: Cell<bool>,
    }

    impl VulkanQuery for MockVulkanQuery {
        fn create_instance(&self) -> Result<VkInstance, ()> {
            self.create_instance_result
        }

        fn destroy_instance(&self, _instance: VkInstance) {
            self.destroyed.set(true);
        }

        fn enumerate_device_count(&self, _instance: VkInstance) -> Result<u32, ()> {
            self.enumerate_count_result
        }

        fn enumerate_device_pointers(
            &self,
            _instance: VkInstance,
            count: u32,
            out: &mut [VkPhysicalDevice],
        ) -> Result<u32, ()> {
            self.buffer_requested.set(true);
            for (idx, slot) in out.iter_mut().enumerate().take(count as usize) {
                *slot = (idx + 1) as VkPhysicalDevice;
            }
            self.enumerate_pointers_result
        }

        fn get_physical_device_properties(
            &self,
            device: VkPhysicalDevice,
            properties: &mut VkPhysicalDeviceProperties,
        ) {
            let idx = (device as usize).saturating_sub(1);
            if let Some(prop) = self.device_properties.get(idx) {
                *properties = *prop;
            }
        }

        fn get_physical_device_memory_properties(
            &self,
            device: VkPhysicalDevice,
            memory: &mut VkPhysicalDeviceMemoryProperties,
        ) {
            let idx = (device as usize).saturating_sub(1);
            if let Some(mem) = self.device_memory.get(idx) {
                *memory = *mem;
            }
        }
    }

    fn make_properties(name: &str, device_type: u32) -> VkPhysicalDeviceProperties {
        let mut props = VkPhysicalDeviceProperties {
            api_version: 0,
            driver_version: 0,
            vendor_id: 0,
            device_id: 0,
            device_type,
            device_name: [0; VK_PHYSICAL_DEVICE_NAME_SIZE],
            _tail: [0; 8192],
        };
        for (slot, byte) in props.device_name.iter_mut().zip(name.bytes()) {
            *slot = byte as c_char;
        }
        props
    }

    fn make_memory(heaps: &[(u64, u32)]) -> VkPhysicalDeviceMemoryProperties {
        let mut mem = VkPhysicalDeviceMemoryProperties {
            memory_type_count: 1,
            memory_types: [VkMemoryType {
                property_flags: 0,
                heap_index: 0,
            }; VK_MAX_MEMORY_TYPES],
            memory_heap_count: u32::try_from(heaps.len()).unwrap(),
            memory_heaps: [VkMemoryHeap { size: 0, flags: 0 }; VK_MAX_MEMORY_HEAPS],
        };
        for (slot, &(size, flags)) in mem.memory_heaps.iter_mut().zip(heaps.iter()) {
            *slot = VkMemoryHeap { size, flags };
        }
        mem
    }

    #[test]
    fn enumeration_table_covers_capacity_boundaries_and_error_handling() {
        let dummy_instance = 1 as VkInstance;

        // 1. VK_SUCCESS (0) and count 0 -> Ok(empty), buffer not called
        {
            let query = MockVulkanQuery {
                create_instance_result: Ok(dummy_instance),
                enumerate_count_result: Ok(0),
                enumerate_pointers_result: Ok(0),
                buffer_requested: Cell::new(false),
                device_properties: Vec::new(),
                device_memory: Vec::new(),
                destroyed: Cell::new(false),
            };
            let result = enumerate_devices_bounded(&query, 64);
            assert_eq!(result, Ok(Vec::new()));
            assert!(!query.buffer_requested.get());
            assert!(query.destroyed.get());
        }

        // 2. Nonzero first result -> Err, buffer not called
        {
            let query = MockVulkanQuery {
                create_instance_result: Ok(dummy_instance),
                enumerate_count_result: Err(()),
                enumerate_pointers_result: Ok(0),
                buffer_requested: Cell::new(false),
                device_properties: Vec::new(),
                device_memory: Vec::new(),
                destroyed: Cell::new(false),
            };
            let result = enumerate_devices_bounded(&query, 64);
            assert_eq!(result, Err(()));
            assert!(!query.buffer_requested.get());
            assert!(query.destroyed.get());
        }

        // 3. Count above capacity -> Err, buffer not requested
        {
            let query = MockVulkanQuery {
                create_instance_result: Ok(dummy_instance),
                enumerate_count_result: Ok(2),
                enumerate_pointers_result: Ok(2),
                buffer_requested: Cell::new(false),
                device_properties: Vec::new(),
                device_memory: Vec::new(),
                destroyed: Cell::new(false),
            };
            let result = enumerate_devices_bounded(&query, 1);
            assert_eq!(result, Err(()));
            assert!(!query.buffer_requested.get());
            assert!(query.destroyed.get());
        }

        // 4. Second count larger than sized buffer -> Err
        {
            let query = MockVulkanQuery {
                create_instance_result: Ok(dummy_instance),
                enumerate_count_result: Ok(1),
                enumerate_pointers_result: Ok(2),
                buffer_requested: Cell::new(false),
                device_properties: vec![make_properties("GPU", 2)],
                device_memory: vec![make_memory(&[(1024 * 1024 * 1024, VK_DEVICE_LOCAL_BIT)])],
                destroyed: Cell::new(false),
            };
            let result = enumerate_devices_bounded(&query, 64);
            assert_eq!(result, Err(()));
            assert!(query.buffer_requested.get());
            assert!(query.destroyed.get());
        }

        // 5. memory_heap_count > 16 -> Err
        {
            let mut invalid_mem = make_memory(&[(1024 * 1024 * 1024, VK_DEVICE_LOCAL_BIT)]);
            invalid_mem.memory_heap_count = 17;
            let query = MockVulkanQuery {
                create_instance_result: Ok(dummy_instance),
                enumerate_count_result: Ok(1),
                enumerate_pointers_result: Ok(1),
                buffer_requested: Cell::new(false),
                device_properties: vec![make_properties("GPU", 2)],
                device_memory: vec![invalid_mem],
                destroyed: Cell::new(false),
            };
            let result = enumerate_devices_bounded(&query, 64);
            assert_eq!(result, Err(()));
        }

        // 6. memory_type_count > 32 -> Err
        {
            let mut invalid_mem = make_memory(&[(1024 * 1024 * 1024, VK_DEVICE_LOCAL_BIT)]);
            invalid_mem.memory_type_count = 33;
            let query = MockVulkanQuery {
                create_instance_result: Ok(dummy_instance),
                enumerate_count_result: Ok(1),
                enumerate_pointers_result: Ok(1),
                buffer_requested: Cell::new(false),
                device_properties: vec![make_properties("GPU", 2)],
                device_memory: vec![invalid_mem],
                destroyed: Cell::new(false),
            };
            let result = enumerate_devices_bounded(&query, 64);
            assert_eq!(result, Err(()));
        }

        // 7. Two populated devices with different properties
        {
            let dev1_props = make_properties("NVIDIA GeForce RTX 4090", 2);
            let dev1_mem = make_memory(&[
                (24 * 1024 * 1024 * 1024, VK_DEVICE_LOCAL_BIT),
                (1024 * 1024, 0),
            ]);
            let dev2_props = make_properties("Intel UHD Graphics", 1);
            let dev2_mem = make_memory(&[(2 * 1024 * 1024 * 1024, VK_DEVICE_LOCAL_BIT)]);

            let query = MockVulkanQuery {
                create_instance_result: Ok(dummy_instance),
                enumerate_count_result: Ok(2),
                enumerate_pointers_result: Ok(2),
                buffer_requested: Cell::new(false),
                device_properties: vec![dev1_props, dev2_props],
                device_memory: vec![dev1_mem, dev2_mem],
                destroyed: Cell::new(false),
            };
            let result = enumerate_devices_bounded(&query, 64);
            assert_eq!(
                result,
                Ok(vec![
                    VulkanDevice {
                        index: 0,
                        name: "NVIDIA GeForce RTX 4090".to_owned(),
                        device_type: 2,
                        vram_mib: 24576,
                    },
                    VulkanDevice {
                        index: 1,
                        name: "Intel UHD Graphics".to_owned(),
                        device_type: 1,
                        vram_mib: 2048,
                    },
                ])
            );
        }
    }

    #[test]
    fn overflowing_device_memory_fails_and_destroys_instance() {
        let query = MockVulkanQuery {
            create_instance_result: Ok(1 as VkInstance),
            enumerate_count_result: Ok(1),
            enumerate_pointers_result: Ok(1),
            buffer_requested: Cell::new(false),
            device_properties: vec![make_properties("GPU", 2)],
            device_memory: vec![make_memory(&[
                (u64::MAX, VK_DEVICE_LOCAL_BIT),
                (1, VK_DEVICE_LOCAL_BIT),
            ])],
            destroyed: Cell::new(false),
        };
        assert_eq!(enumerate_devices_bounded(&query, 64), Err(()));
        assert!(query.destroyed.get());
    }

    #[test]
    fn output_delivery_must_finish_before_success() {
        struct FailingWriter {
            remaining: usize,
            fail_flush: bool,
        }
        impl Write for FailingWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.remaining == 0 {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                let count = self.remaining.min(bytes.len());
                self.remaining -= count;
                Ok(count)
            }

            fn flush(&mut self) -> io::Result<()> {
                if self.fail_flush {
                    Err(io::ErrorKind::BrokenPipe.into())
                } else {
                    Ok(())
                }
            }
        }
        let successful_output = || ProbeOutput {
            stdout: b"[]\n".to_vec(),
            exit_code: 0,
        };
        let mut bytes = Vec::new();
        assert_eq!(
            emit_output(successful_output(), &mut bytes),
            ExitCode::SUCCESS
        );
        assert_eq!(bytes, b"[]\n");
        for (remaining, fail_flush) in [(0, false), (1, false), (3, true)] {
            assert_eq!(
                emit_output(
                    successful_output(),
                    FailingWriter {
                        remaining,
                        fail_flush
                    }
                ),
                ExitCode::FAILURE,
            );
        }
        assert_eq!(
            emit_output(
                ProbeOutput {
                    stdout: Vec::new(),
                    exit_code: 1
                },
                Vec::new()
            ),
            ExitCode::FAILURE,
        );
    }

    #[test]
    fn mapper_table_handles_both_platforms() {
        let devices = vec![VulkanDevice {
            index: 0,
            name: "Test GPU".to_owned(),
            device_type: 2,
            vram_mib: 8192,
        }];

        // Linux Ok
        let linux_ok = map_enumeration_output(ProbePlatform::Linux, Ok(devices.clone()));
        assert_eq!(linux_ok.exit_code, 0);
        assert_eq!(
            serde_json::from_slice::<Vec<VulkanDevice>>(&linux_ok.stdout).unwrap(),
            devices
        );

        // Linux Ok empty
        let linux_ok_empty = map_enumeration_output(ProbePlatform::Linux, Ok(Vec::new()));
        assert_eq!(linux_ok_empty.exit_code, 0);
        assert_eq!(linux_ok_empty.stdout, b"[]\n");

        // Linux Err -> [] exit 0
        let linux_err = map_enumeration_output(ProbePlatform::Linux, Err(()));
        assert_eq!(linux_err.exit_code, 0);
        assert_eq!(linux_err.stdout, b"[]\n");

        // Windows Ok
        let win_ok = map_enumeration_output(ProbePlatform::Windows, Ok(devices.clone()));
        assert_eq!(win_ok.exit_code, 0);
        assert_eq!(
            serde_json::from_slice::<Vec<VulkanDevice>>(&win_ok.stdout).unwrap(),
            devices
        );

        // Windows Ok empty
        let win_ok_empty = map_enumeration_output(ProbePlatform::Windows, Ok(Vec::new()));
        assert_eq!(win_ok_empty.exit_code, 0);
        assert_eq!(win_ok_empty.stdout, b"[]\n");

        // Windows Err -> empty stdout exit 1 (nonzero and not b"[]\n")
        let win_err = map_enumeration_output(ProbePlatform::Windows, Err(()));
        assert_ne!(win_err.exit_code, 0);
        assert!(win_err.stdout.is_empty());
        assert_ne!(win_err.stdout, b"[]\n");
    }

    #[test]
    fn version_flag_prints_version_without_loading() {
        let run_called = Cell::new(0);
        let output = probe_entry(["--version"], || {
            run_called.set(run_called.get() + 1);
            ProbeOutput {
                stdout: Vec::new(),
                exit_code: 1,
            }
        });
        assert_eq!(run_called.get(), 0);
        assert_eq!(output.exit_code, 0);
        assert_eq!(
            output.stdout,
            format!("solstone-core-vulkan-probe {}\n", env!("CARGO_PKG_VERSION")).into_bytes()
        );

        let second = probe_entry(std::iter::empty::<&str>(), || {
            run_called.set(run_called.get() + 1);
            ProbeOutput {
                stdout: b"[]\n".to_vec(),
                exit_code: 0,
            }
        });
        assert_eq!(run_called.get(), 1);
        assert_eq!(second.exit_code, 0);
        assert_eq!(second.stdout, b"[]\n");
    }

    #[test]
    fn loader_verification_failure_performs_zero_restrict_or_load_calls() {
        let restrict_calls = Cell::new(0);
        let load_calls = Cell::new(0);
        let canonicalize_calls = Cell::new(0);

        let result = open_declared_loader(
            || Err("missing member"),
            |_path: &Path| {
                canonicalize_calls.set(canonicalize_calls.get() + 1);
                Ok(PathBuf::from("/canonical/vulkan-1.dll"))
            },
            || {
                restrict_calls.set(restrict_calls.get() + 1);
                Ok(())
            },
            |policy, path| {
                load_calls.set(load_calls.get() + 1);
                assert_eq!(
                    policy,
                    solstone_core_win_dll_load::LoadPolicy::ApplicationDir
                );
                assert_eq!(solstone_core_win_dll_load::flags_for(policy), 0x0A00);
                assert_eq!(path, Path::new("/canonical/vulkan-1.dll"));
                Ok("loaded")
            },
        );

        assert_eq!(result, Err("missing member"));
        assert_eq!(canonicalize_calls.get(), 0);
        assert_eq!(restrict_calls.get(), 0);
        assert_eq!(load_calls.get(), 0);
    }

    #[test]
    fn loader_success_restricts_before_loading_and_passes_application_dir_policy() {
        let events = std::cell::RefCell::new(Vec::new());

        let result = open_declared_loader(
            || {
                Ok::<PathBuf, &'static str>(PathBuf::from(
                    "/package/lib/solstone-native/vulkan-1.dll",
                ))
            },
            |path: &Path| {
                events.borrow_mut().push("canonicalize");
                assert_eq!(path, Path::new("/package/lib/solstone-native/vulkan-1.dll"));
                Ok(PathBuf::from(
                    "/canonical/package/lib/solstone-native/vulkan-1.dll",
                ))
            },
            || {
                events.borrow_mut().push("restrict");
                Ok(())
            },
            |policy, path| {
                events.borrow_mut().push("load");
                assert_eq!(policy, solstone_core_win_dll_load::LoadPolicy::DllLoadDir);
                assert_eq!(solstone_core_win_dll_load::flags_for(policy), 0x0900);
                assert_eq!(
                    path,
                    Path::new("/canonical/package/lib/solstone-native/vulkan-1.dll")
                );
                Ok("loaded_handle")
            },
        );

        assert_eq!(result, Ok("loaded_handle"));
        assert_eq!(
            events.into_inner(),
            vec!["canonicalize", "restrict", "load"]
        );
    }
}
