// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Synthetic cross-process parity for the native local fit report.

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};
use solstone_core_local::install::fit_report::{FitSeverity, build_local_fit_report};
use solstone_core_local::{Backend, BackendChoice, NvidiaProbe, VulkanDevice};

const END_TO_END_BRANCH_COUNT: usize = 18;
const GIB: u64 = 1024 * 1024 * 1024;

// Parity is text-wise: severity plus detail only. The builder accepts injected
// force_cpu and therefore has 19 constructible branches, while this end-to-end
// Python matrix has 18: unified memory makes placement return force_cpu=false,
// so CUDA unified-memory plus the placement suffix is unreachable.
const PYTHON: &str = r#"
import json, os, sys
from pathlib import Path
from types import SimpleNamespace
from solstone.think.providers import fit_report, local_cuda, local_install, local_vulkan
from solstone.think.providers import memory
from solstone.think import models
from solstone.think.providers import local_endpoint

GIB = 1024 ** 3
cases = json.loads(os.environ['LOCAL_FIT_CASES'])

def run(case):
    sys.platform = 'linux'
    key = case.get('key', 'x86_64-unknown-linux-gnu')
    backend = case.get('backend', 'vulkan')
    detected = case.get('detected', False)
    source = case.get('source', 'unavailable')
    devices = [] if case.get('devices') == 'none' else [local_vulkan.VulkanDevice(0, 'Test GPU', local_vulkan.VK_TYPE_DISCRETE, 6144)]
    local_install.llama_server_artifact_key = lambda: key
    if case.get('platform_blocked'):
        from solstone.think.providers.local import LocalProviderError
        local_install.pin_for_current_platform = lambda: (_ for _ in ()).throw(LocalProviderError('unsupported_platform', f'No pinned llama-server artifact for platform {key}'))
    else:
        local_install.pin_for_current_platform = lambda: {}
    local_install.cache_root = lambda: Path('/synthetic/journal/cache/providers/local')
    local_install.cuda_server_pin = lambda: object()
    local_install.cuda_artifact_pin_for_current_platform = lambda _pin: SimpleNamespace(size_bytes=550238443) if case.get('cuda_pin', False) else None
    local_install.gpu_device_override = lambda: None
    local_cuda.probe_nvidia_gpu = lambda: SimpleNamespace(detected=detected, memory_source=source, vram_mib=case.get('vram'), tiering_memory_mib=case.get('unified'))
    local_cuda.resolve_local_backend = lambda _pin: SimpleNamespace(backend=backend, reason='test choice')
    local_vulkan.detect_gpus = lambda: list(devices)
    local_vulkan.gpu_probe_ok = lambda: case.get('probe_ok', True)
    available = case.get('ram')
    severity = 'warning' if available is None or available < 8 * GIB else 'ok'
    fit_report.assess_memory = lambda required, *, block_below_floor: memory.MemoryVerdict(available, required, severity)
    if case.get('disk_error'):
        fit_report.free_bytes = lambda _path: (_ for _ in ()).throw(OSError('disk unavailable'))
    else:
        fit_report.free_bytes = lambda _path: case.get('disk', 20 * GIB)
    models.is_local_provider_needed = lambda: case.get('brain', True)
    local_endpoint.resolve_local_endpoint = lambda: SimpleNamespace(is_bundled=True)
    report = fit_report.build_local_fit_report('local/qwen3.5-4b')
    target = case['target']
    check = next(check for check in report.checks if check.name == target)
    return [check.severity, check.detail]

print(json.dumps([run(case) for case in cases]))
"#;

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

fn probe(case: &Value) -> NvidiaProbe {
    let source = case["source"].as_str().unwrap_or("unavailable");
    NvidiaProbe {
        schema: "test".to_owned(),
        detected: case["detected"].as_bool().unwrap_or(false),
        gpu_index: None,
        gpu_name: None,
        compute_cap: None,
        arch: None,
        driver_cuda_major: None,
        vram_mib: (source == "nvidia memory.total").then_some(6144),
        unified_memory_mib: (source == "system MemAvailable (unified memory)").then_some(6144),
        probe_error: None,
    }
}

fn native(case: &Value) -> (FitSeverity, String) {
    let backend = if case["backend"].as_str() == Some("cuda") {
        Backend::Cuda
    } else {
        Backend::Vulkan
    };
    let choice = BackendChoice {
        backend,
        reason: "test choice".to_owned(),
    };
    let devices = if case["devices"].as_str() == Some("none") {
        Vec::new()
    } else {
        vec![VulkanDevice {
            index: 0,
            name: "Test GPU".to_owned(),
            device_type: Some(2),
            vram_mib: 6144,
        }]
    };
    let disk = if case["disk_error"].as_bool().unwrap_or(false) {
        Err("disk unavailable".to_owned())
    } else {
        Ok(case["disk"].as_u64().unwrap_or(20 * GIB))
    };
    let report = build_local_fit_report(
        std::path::Path::new("/synthetic/journal"),
        "local/qwen3.5-4b",
        "linux",
        if case["platform_blocked"].as_bool().unwrap_or(false) {
            "riscv64"
        } else {
            "x86_64"
        },
        disk,
        case.get("ram").and_then(Value::as_u64),
        &probe(case),
        &choice,
        case["probe_ok"].as_bool().unwrap_or(true),
        &devices,
        None,
        case["force_cpu"].as_bool().unwrap_or(false),
    );
    let check = report
        .checks
        .iter()
        .find(|check| check.name == case["target"].as_str().unwrap())
        .unwrap();
    (check.severity, check.detail.clone())
}

fn severity(value: &str) -> FitSeverity {
    match value {
        "ok" => FitSeverity::Ok,
        "warning" => FitSeverity::Warning,
        "blocked" => FitSeverity::Blocked,
        "unknown" => FitSeverity::Unknown,
        value => panic!("unknown Python severity {value}"),
    }
}

#[test]
fn local_fit_report_matches_python_for_each_reachable_branch() {
    let cases = vec![
        json!({"target":"platform", "ram":8*GIB}),
        json!({"target":"platform", "platform_blocked":true, "key":"riscv64-unknown-linux-gnu", "ram":8*GIB}),
        json!({"target":"ram", "ram":null}),
        json!({"target":"ram", "ram":8*GIB}),
        json!({"target":"ram", "ram":1}),
        json!({"target":"disk", "ram":8*GIB, "disk_error":true}),
        json!({"target":"disk", "ram":8*GIB, "disk":1}),
        json!({"target":"disk", "ram":8*GIB, "disk":20*GIB}),
        json!({"target":"disk", "backend":"cuda", "cuda_pin":true, "detected":true, "source":"nvidia memory.total", "ram":8*GIB, "disk":20*GIB}),
        json!({"target":"gpu", "backend":"cuda", "detected":false, "source":"unavailable", "ram":8*GIB}),
        json!({"target":"gpu", "backend":"cuda", "detected":true, "source":"unavailable", "ram":8*GIB}),
        json!({"target":"gpu", "backend":"cuda", "detected":true, "source":"nvidia memory.total", "brain":false, "ram":8*GIB}),
        json!({"target":"gpu", "backend":"cuda", "detected":true, "source":"nvidia memory.total", "force_cpu":true, "ram":8*GIB}),
        json!({"target":"gpu", "backend":"cuda", "detected":true, "source":"system MemAvailable (unified memory)", "ram":8*GIB}),
        json!({"target":"gpu", "backend":"vulkan", "probe_ok":false, "ram":8*GIB}),
        json!({"target":"gpu", "backend":"vulkan", "devices":"none", "ram":8*GIB}),
        json!({"target":"gpu", "backend":"vulkan", "brain":false, "ram":8*GIB}),
        json!({"target":"gpu", "backend":"vulkan", "force_cpu":true, "ram":8*GIB}),
    ];
    assert_eq!(cases.len(), END_TO_END_BRANCH_COUNT);
    let output = Command::new(python())
        .args(["-c", PYTHON])
        .current_dir(repository_root())
        .env("LOCAL_FIT_CASES", serde_json::to_string(&cases).unwrap())
        .output()
        .expect("Python local fit-report oracle runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python: Vec<(String, String)> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        python.len(),
        cases.len(),
        "Python result count must cover every case"
    );
    for (case, (python_severity, python_detail)) in cases.iter().zip(python) {
        let (native_severity, native_detail) = native(case);
        assert_eq!(native_severity, severity(&python_severity), "{case}");
        assert_eq!(native_detail, python_detail, "{case}");
    }
}
