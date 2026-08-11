// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use solstone_core_local::{
    CPU_PLACEMENT_COPY, VulkanDevice, VulkanProbeConfig, VulkanProbeProgram, classify,
    cpu_placement_suffix, discrete_hardware_gpu_count, enumerate_gpus, is_discrete,
};

const NO_LOADER_OPT_OUT_ENV: &str = "SOLSTONE_VULKAN_DIFFERENTIAL_NO_LOADER";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn python() -> PathBuf {
    let repository = repository_root();
    let venv = repository.join(".venv/bin/python");
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python3")
    }
}

fn python_loader_available() -> bool {
    Command::new(python())
        .args(["-c", "import ctypes; ctypes.CDLL('libvulkan.so.1')"])
        .current_dir(repository_root())
        .status()
        .is_ok_and(|status| status.success())
}

fn core_helper() -> PathBuf {
    let helper = repository_root().join("core/target/debug/solstone-core-vulkan-probe");
    assert!(
        helper.is_file(),
        "differential requires make check-differentials to build {}",
        helper.display()
    );
    helper
}

#[test]
fn child_enumerator_matches_python_when_vulkan_loader_is_available() {
    if !python_loader_available() {
        if std::env::var_os(NO_LOADER_OPT_OUT_ENV).as_deref() == Some(OsStr::new("1")) {
            eprintln!(
                "SKIP: Python could not load libvulkan.so.1; {}=1 explicitly opted out",
                NO_LOADER_OPT_OUT_ENV
            );
            return;
        }
        panic!(
            "Python could not load libvulkan.so.1; install a Vulkan loader or set {}=1 to explicitly skip this differential",
            NO_LOADER_OPT_OUT_ENV
        );
    }
    eprintln!("RUN: Vulkan loader available; enumerator differential executing");
    let python_output = Command::new(python())
        .args(["-m", "solstone.think.providers.local_vulkan"])
        .current_dir(repository_root())
        .output()
        .expect("start Python Vulkan probe");
    assert!(
        python_output.status.success(),
        "Python Vulkan probe failed: {}",
        String::from_utf8_lossy(&python_output.stderr)
    );
    let python_devices: Vec<VulkanDevice> =
        serde_json::from_slice(&python_output.stdout).expect("Python Vulkan JSON");

    let config = VulkanProbeConfig {
        program: VulkanProbeProgram::Explicit {
            executable: core_helper(),
            args: Vec::new(),
            env: Vec::new(),
        },
        // Well below the 10 s production budget, while leaving ample room for
        // a real loader/ICD probe so the differential is not timing-sensitive.
        timeout: Duration::from_secs(5),
    };
    let (rust_devices, probe_ok) = enumerate_gpus(&config);

    assert!(probe_ok, "Rust Vulkan child did not complete");
    assert_eq!(rust_devices, python_devices);
}

#[test]
fn placement_helpers_match_python_with_python_force_cpu_result() {
    let script = concat!(
        "import json\n",
        "from solstone.think.providers import local_vulkan\n",
        "from solstone.think.providers.parakeet_placement import (\n",
        " cpu_placement_suffix, decide_parakeet_auto_placement,\n",
        " discrete_hardware_gpu_count, is_discrete)\n",
        "devices = [\n",
        " local_vulkan.VulkanDevice(0, 'GPU', 2, 6144),\n",
        " local_vulkan.VulkanDevice(1, 'iGPU', 1, 23814),\n",
        " local_vulkan.VulkanDevice(2, 'llvmpipe', 4, 0)]\n",
        "selected = devices[0]\n",
        "force_cpu = decide_parakeet_auto_placement(\n",
        " selected.vram_mib, is_discrete(selected, local_vulkan),\n",
        " discrete_hardware_gpu_count(devices, local_vulkan), False, True).force_cpu\n",
        "print(json.dumps({'classify': local_vulkan.classify(selected),\n",
        " 'is_discrete': is_discrete(selected, local_vulkan),\n",
        " 'count': discrete_hardware_gpu_count(devices, local_vulkan),\n",
        " 'force_cpu': force_cpu,\n",
        " 'suffix': cpu_placement_suffix(devices=devices, selected=selected,\n",
        " local_vulkan=local_vulkan, unified_memory=False, brain_lane_active=True)}))\n"
    );
    let output = Command::new(python())
        .args(["-c", script])
        .current_dir(repository_root())
        .output()
        .expect("start Python placement helper");
    assert!(
        output.status.success(),
        "Python helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python: serde_json::Value = serde_json::from_slice(&output.stdout).expect("helper JSON");
    let devices = vec![
        VulkanDevice {
            index: 0,
            name: "GPU".into(),
            device_type: Some(2),
            vram_mib: 6144,
        },
        VulkanDevice {
            index: 1,
            name: "iGPU".into(),
            device_type: Some(1),
            vram_mib: 23_814,
        },
        VulkanDevice {
            index: 2,
            name: "llvmpipe".into(),
            device_type: Some(4),
            vram_mib: 0,
        },
    ];
    let force_cpu = python["force_cpu"].as_bool().expect("force_cpu bool");
    assert_eq!(classify(&devices[0]), python["classify"].as_str().unwrap());
    assert_eq!(
        is_discrete(&devices[0]),
        python["is_discrete"].as_bool().unwrap()
    );
    assert_eq!(
        discrete_hardware_gpu_count(&devices),
        u32::try_from(python["count"].as_u64().unwrap()).expect("count fits u32")
    );
    assert_eq!(
        cpu_placement_suffix(Some(&devices[0]), force_cpu),
        python["suffix"].as_str().unwrap()
    );
    let expected_suffix = format!("; {CPU_PLACEMENT_COPY}");
    assert_eq!(python["suffix"].as_str(), Some(expected_suffix.as_str()));
}
