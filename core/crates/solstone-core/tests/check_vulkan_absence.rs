// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::time::Duration;

use solstone_core_check::{
    CheckInputs, DiskInput, MemoryInput, NvidiaInput, PlatformInput, Severity, VulkanInput,
    build_check_report, exit_code,
};
use solstone_core_local::{VulkanProbeConfig, VulkanProbeProgram, enumerate_gpus};

fn inputs(probe_ok: bool) -> CheckInputs {
    CheckInputs {
        platform: PlatformInput {
            os: "Linux".into(),
            os_version: "x".into(),
            arch: "x86_64".into(),
        },
        memory: MemoryInput {
            total_bytes: Some(16 << 30),
            available_bytes: Some(16 << 30),
        },
        disk: DiskInput::Ok {
            free_bytes: 30 << 30,
        },
        journal_path: "/journal".into(),
        nvidia: NvidiaInput {
            detected: false,
            vram_mib: None,
            tiering_memory_mib: None,
            memory_source: "unavailable".into(),
        },
        vulkan: VulkanInput {
            probe_ok,
            devices: vec![],
        },
        render_nodes_present_but_inaccessible: false,
        gpu_evaluation_error: None,
        version: "x".into(),
        ced: solstone_core_check::CedCheckInput::Omit,
        rfdetr: solstone_core_check::RfdetrCheckInput::Ready,
    }
}
#[test]
fn clean_empty_helper_is_blocked() {
    let config = VulkanProbeConfig {
        program: VulkanProbeProgram::Explicit {
            executable: PathBuf::from("sh"),
            args: vec!["-c".into(), "printf '[]'".into()],
            env: vec![],
        },
        timeout: Duration::from_secs(1),
    };
    assert_eq!(enumerate_gpus(&config), (vec![], true));
    let report = build_check_report(&inputs(true));
    assert_eq!(report.checks[1].severity, Severity::Blocked);
    assert_eq!(exit_code(&report), 2);
}
#[test]
fn missing_helper_is_unknown_warning() {
    let config = VulkanProbeConfig {
        program: VulkanProbeProgram::Explicit {
            executable: PathBuf::from("/definitely/missing-solstone-vulkan-helper"),
            args: vec![],
            env: vec![],
        },
        timeout: Duration::from_secs(1),
    };
    assert_eq!(enumerate_gpus(&config), (vec![], false));
    let report = build_check_report(&inputs(false));
    assert_eq!(report.checks[1].severity, Severity::Unknown);
    assert_eq!(exit_code(&report), 1);
}
