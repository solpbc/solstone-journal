// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Dynamically linked Vulkan device enumeration helper.
//!
//! This Linux glibc leaf owns the loader boundary. Its no-argument protocol is
//! exactly one JSON array on stdout; loader and Vulkan failures are a clean
//! empty array, matching the established Python probe contract.

use std::ffi::{c_char, c_void};
use std::io::{self, Write};

use libloading::Library;
use serde::Serialize;

const VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO: u32 = 1;
const VK_DEVICE_LOCAL_BIT: u32 = 0x0000_0001;
const VK_PHYSICAL_DEVICE_NAME_SIZE: usize = 256;
const VK_MAX_MEMORY_TYPES: usize = 32;
const VK_MAX_MEMORY_HEAPS: usize = 16;

#[derive(Serialize)]
struct VulkanDevice {
    index: u32,
    name: String,
    device_type: u32,
    vram_mib: u64,
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
    // Vulkan writes the rest after device_name. This deliberately oversized
    // tail avoids binding fields the JSON protocol does not need.
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
    _library: Library,
    create_instance: VkCreateInstance,
    destroy_instance: VkDestroyInstance,
    enumerate_physical_devices: VkEnumeratePhysicalDevices,
    get_physical_device_properties: VkGetPhysicalDeviceProperties,
    get_physical_device_memory_properties: VkGetPhysicalDeviceMemoryProperties,
}

impl VulkanFns {
    fn load() -> Result<Self, ()> {
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

fn enumerate_in_process() -> Vec<VulkanDevice> {
    let functions = match VulkanFns::load() {
        Ok(functions) => functions,
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
            let vram_bytes = memory.memory_heaps[..heap_count]
                .iter()
                .filter(|heap| heap.flags & VK_DEVICE_LOCAL_BIT != 0)
                .fold(0_u64, |total, heap| total.saturating_add(heap.size));
            devices.push(VulkanDevice {
                index: u32::try_from(index).map_err(|_| ())?,
                name: String::from_utf8_lossy(&name_bytes).into_owned(),
                device_type: properties.device_type,
                vram_mib: vram_bytes / (1024 * 1024),
            });
        }
        Ok(devices)
    })();
    if !instance.is_null() {
        unsafe { (functions.destroy_instance)(instance, std::ptr::null()) };
    }
    result.unwrap_or_default()
}

fn write_json(mut writer: impl Write) -> io::Result<()> {
    serde_json::to_writer(&mut writer, &enumerate_in_process())?;
    writer.write_all(b"\n")
}

fn main() -> io::Result<()> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--version")) {
        println!("solstone-core-vulkan-probe {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    write_json(io::stdout().lock())
}
