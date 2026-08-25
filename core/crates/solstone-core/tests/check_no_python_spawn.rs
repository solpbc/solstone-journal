// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
#[cfg(unix)]
fn check_never_reaches_a_sibling_or_path_interpreter() {
    let temp = tempfile::tempdir().expect("temporary sibling directory");
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).expect("create bin");
    let core = bin.join("solstone-core");
    fs::copy(env!("CARGO_BIN_EXE_solstone-core"), &core).expect("copy core");
    fs::set_permissions(&core, fs::Permissions::from_mode(0o755)).expect("make core executable");
    let helper = build_vulkan_probe();
    fs::copy(&helper, bin.join("solstone-core-vulkan-probe")).expect("copy Vulkan helper");
    let marker = temp.path().join("python-invoked.txt");
    for name in ["python", "python3", "uv", "pytest", "ruff"] {
        let shim = bin.join(name);
        fs::write(
            &shim,
            "#!/bin/sh\nprintf '%s\\n' \"$0\" > \"$POISON_MARKER\"\nexit 97\n",
        )
        .expect("write poison shim");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
            .expect("make shim executable");
    }
    let output = Command::new(&core)
        .arg("check")
        .arg("--json")
        .env("PATH", &bin)
        .env("POISON_MARKER", &marker)
        .env("SOLSTONE_JOURNAL", temp.path().join("journal"))
        .output()
        .expect("run check");
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 verdict");
    // The verdict's check set is platform-specific: Linux emits a Vulkan/nvidia
    // `gpu` row and a `ram` row, macOS emits a single `memory` row instead
    // (`check.py::_linux_gpu_check` vs `_macos_memory_check`). Asserting the
    // Linux set everywhere fails on Darwin against a correct verdict.
    let expected: &[&str] = if cfg!(target_os = "macos") {
        &["platform", "memory", "disk"]
    } else {
        &["platform", "gpu", "ram", "disk"]
    };
    for name in expected {
        assert!(
            stdout.contains(&format!("\"name\": \"{name}\"")),
            "missing {name}: {stdout}"
        );
    }
    assert!(
        stdout.contains("\"overall\": \"ok\"")
            || stdout.contains("\"overall\": \"warning\"")
            || stdout.contains("\"overall\": \"blocked\"")
    );
    assert!(
        !marker.exists(),
        "native check reached poison interpreter: {}",
        marker.display()
    );
}

/// Build the Vulkan probe and return the executable cargo actually produced.
///
/// The previous form joined a hardcoded `../../target/debug/` path and assumed
/// something had already built it. That passes in a warm worktree and fails in a
/// fresh one with `NotFound`, so `make ci` was red for every new checkout. Building
/// here also makes the path correct under a non-default profile or
/// `CARGO_TARGET_DIR`. Mirrors `locate_workspace_binary` in
/// `journal_native_process_contract.rs`.
#[cfg(unix)]
fn build_vulkan_probe() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = manifest_dir
        .parent()
        .expect("crates dir")
        .parent()
        .expect("core dir")
        .join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(&workspace_manifest)
        .args([
            "-p",
            "solstone-core-vulkan-probe",
            "--bin",
            "solstone-core-vulkan-probe",
            "--message-format=json",
        ])
        .output()
        .expect("cargo build solstone-core-vulkan-probe should execute");
    assert!(
        output.status.success(),
        "cargo build -p solstone-core-vulkan-probe failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        if let Some(executable) = message["executable"].as_str()
            && std::path::Path::new(executable)
                .file_name()
                .is_some_and(|name| name == "solstone-core-vulkan-probe")
        {
            return std::path::PathBuf::from(executable);
        }
    }
    panic!("cargo did not report a solstone-core-vulkan-probe executable");
}

#[test]
#[cfg(unix)]
fn native_library_paths_never_spawn_python() {
    if std::env::var_os("NATIVE_LIBRARY_PATH_POISON_CHILD").is_some() {
        invoke_native_library_paths(&std::path::PathBuf::from(
            std::env::var_os("SOLSTONE_JOURNAL").expect("child journal"),
        ));
        return;
    }
    let temp = tempfile::tempdir().expect("temporary journal");
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).expect("create poison bin");
    let marker = temp.path().join("python-invoked.txt");
    for name in ["python", "python3", "uv", "pytest", "ruff"] {
        let shim = bin.join(name);
        fs::write(&shim, "#!/bin/sh\nprintf x > \"$POISON_MARKER\"\nexit 97\n")
            .expect("write poison shim");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
            .expect("make shim executable");
    }
    let journal = temp.path().join("journal");
    let output = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "check_no_python_spawn::native_library_paths_never_spawn_python",
            "--nocapture",
        ])
        .env("NATIVE_LIBRARY_PATH_POISON_CHILD", "1")
        .env("PATH", &bin)
        .env("POISON_MARKER", &marker)
        .env("SOLSTONE_JOURNAL", &journal)
        .output()
        .expect("run poisoned child");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!marker.exists(), "native library path reached Python");

    // No in-harness Python route is registered by this test, so prove the
    // shims themselves are live as the required positive control.
    let status = Command::new(bin.join("python3"))
        .env("POISON_MARKER", &marker)
        .status()
        .expect("run shim");
    assert_eq!(status.code(), Some(97));
    assert!(
        marker.exists(),
        "poison-shim self-check did not create its marker"
    );
}

#[cfg(unix)]
fn invoke_native_library_paths(journal: &std::path::Path) {
    use solstone_core_system::activity_state::ActivityStateMachine;
    use solstone_core_system::catchup::{
        DailyCatchupOutcome, SegmentRepairOutcome, record_daily_catchup_attempt,
        record_daily_catchup_outcome, record_daily_catchup_progress, record_segment_repair_attempt,
        record_segment_repair_outcome,
    };
    use solstone_core_system::memory_admission::{
        MemoryAdmissionCache, resolve_memory_floor_bytes, wait_for_memory_headroom,
    };

    let segment = journal.join("chronicle/20260101/120000_60");
    fs::create_dir_all(&segment).expect("create segment");
    fs::write(segment.join("audio.json"), b"raw").expect("raw input");
    let prior = journal.join("chronicle/20260101/110000_60");
    fs::create_dir_all(&prior).expect("create prior");

    record_daily_catchup_progress(journal, "20260101", 1, 2);
    record_daily_catchup_attempt(journal, "20260101", "catchup", 1.0, 0, "fingerprint");
    record_daily_catchup_outcome(
        journal,
        "20260101",
        "catchup",
        0,
        "fingerprint",
        DailyCatchupOutcome {
            success: false,
            timed_out: false,
            timeout_seconds: None,
            ended_at: 2.0,
            exit_code: 1,
            exit_status: "error".to_owned(),
        },
    );
    record_segment_repair_attempt(journal, "20260101", 1.0);
    record_segment_repair_outcome(
        journal,
        "20260101",
        SegmentRepairOutcome {
            success: false,
            timed_out: false,
            timeout_seconds: None,
            ended_at: 2.0,
            cleared: None,
            remaining: None,
        },
    );
    let predecessor =
        solstone_core_system_health::resolve_predecessor(journal, "20260101", None, "120000_60");
    let _ = solstone_core_system_health::detect_segment_change(
        journal,
        "20260101",
        None,
        "120000_60",
        &segment,
        predecessor,
        "now",
    );
    let _ = solstone_core_system_health::read_segment_data_state(
        journal,
        "20260101",
        "120000_60",
        None,
        chrono::Utc::now(),
    );
    let _ = solstone_core_system_health::find_segment_dir(journal, "20260101", "120000_60", None);
    let mut machine = ActivityStateMachine::hydrate(Some(journal));
    let _ = machine.update(
        &serde_json::json!({"density":"idle","content_type":"idle"}),
        "120000_60",
        "20260101",
        None,
        0,
    );
    let mut cache = MemoryAdmissionCache::default();
    let floor = resolve_memory_floor_bytes(
        &mut cache,
        &serde_json::json!({"memory":{"floor_mib":0}}),
        "linux",
        "x86_64",
        || false,
        || None,
        None,
    );
    let _ = wait_for_memory_headroom(floor, None, &|| Some(0), &|_| {}, &|| {});
}
