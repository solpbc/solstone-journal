// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_local::install::{archive, manifest, pins};

use super::{
    speakers_analyze_stub, supervisor_guard::SupervisorGuard, temporary_root::temporary_root,
};

struct TempJournal(PathBuf);
impl TempJournal {
    fn new(fixture: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = temporary_root().join(format!("solstone-core-supervisor-providers-{stamp}"));
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1},"transcribe":{"backend":"parakeet-cpp","parakeet-cpp":{"device":"cpu"}}}"#,
        )
        .expect("journal config");
        install_native_local_readiness(&root);
        let parakeet_artifact_key =
            pins::parakeet_artifact_key("linux", "x86_64").expect("fixture parakeet artifact key");
        let parakeet_paths = pins::parakeet_paths(&root, &parakeet_artifact_key);
        let binary = PathBuf::from(
            parakeet_paths["binary_path_cpu"]
                .as_str()
                .expect("cpu binary path"),
        );
        let model = PathBuf::from(parakeet_paths["model_path"].as_str().expect("model path"));
        fs::create_dir_all(binary.parent().expect("binary parent")).expect("binary directory");
        symlink(fixture, &binary).expect("fixture parakeet binary");
        fs::create_dir_all(model.parent().expect("model parent")).expect("model directory");
        fs::write(model, b"test-ready").expect("fixture model");
        Self(root)
    }
}

fn install_native_local_readiness(root: &std::path::Path) {
    let cache = pins::cache_root(root);
    let (platform_key, release_tag, _, _, binary_name) = pins::LLAMA_SERVER_PINS
        .iter()
        .find(|pin| pin.0.ends_with("-apple-darwin"))
        .expect("Darwin llama-server pin");
    let runtime = cache.join("bin").join(platform_key).join(release_tag);
    let model = cache.join("models/local__qwen3.5-4b");
    fs::create_dir_all(&runtime).expect("runtime directory");
    fs::create_dir_all(&model).expect("model directory");
    fs::write(runtime.join(binary_name), b"#!/bin/sh\nexit 0\n").expect("runtime");
    archive::make_executable(&runtime.join(binary_name)).expect("executable runtime");
    fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"model").expect("model");
    fs::write(model.join("mmproj-F16.gguf"), b"projector").expect("projector");
    let runtime_manifest = manifest::build_manifest(
        "local",
        "llama-server-vulkan",
        "test",
        json!({"pin_identity":pins::vulkan_identity(platform_key).unwrap()}),
        manifest::runtime_inventory(&runtime, &[]).unwrap(),
        None,
        None,
    )
    .unwrap();
    manifest::write_manifest(
        &manifest::artifact_manifest_path(&runtime),
        &runtime_manifest,
    )
    .unwrap();
    let model_manifest = manifest::build_manifest(
        "local",
        "local-model",
        "test",
        json!({"pin_identity":pins::model_identity("local/qwen3.5-4b").unwrap()}),
        manifest::inventory_for_tree(&model, "model").unwrap(),
        None,
        None,
    )
    .unwrap();
    manifest::write_manifest(&manifest::artifact_manifest_path(&model), &model_manifest).unwrap();
}
impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn start(journal: &TempJournal, fixture: &str) -> SupervisorGuard {
    let home = super::installation_binding::admit_for(&journal.0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["supervisor", "--journal"])
        .arg(&journal.0)
        .env("SOLSTONE_LOCAL_BINARY", fixture)
        .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_BINARY", fixture)
        .env("SOLSTONE_SUPERVISOR_PARAKEET_FIXTURE", "1")
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    speakers_analyze_stub::apply(&mut command);
    SupervisorGuard::new(command.spawn().expect("supervisor starts"))
}

fn wait_for_provider_records(journal: &TempJournal, child: &mut SupervisorGuard) -> [Value; 2] {
    let paths = [
        journal.0.join("health/providers/runtime/local.json"),
        journal.0.join("health/providers/runtime/parakeet.json"),
    ];
    let mut records = [None, None];
    for _ in 0..1_200 {
        for (record, path) in records.iter_mut().zip(&paths) {
            if let Ok(bytes) = fs::read(path)
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            {
                *record = Some(value);
            }
        }
        if records.iter().all(|record| {
            record
                .as_ref()
                .is_some_and(|value| value["phase"] == "ready")
        }) {
            return records.map(Option::unwrap);
        }
        if let Some(status) = child.try_wait().expect("supervisor status") {
            panic!("supervisor exited: {status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("providers did not become ready: {records:?}");
}

#[test]
fn ac12_local_and_parakeet_reconcile_real_fixture_cycles() {
    let fixture = env!("CARGO_BIN_EXE_solstone-core-system-test-child");
    let journal = TempJournal::new(fixture);
    let mut child = start(&journal, fixture);
    let _ = wait_for_provider_records(&journal, &mut child);
}

#[test]
fn supervisor_guard_reclaims_provider_tree_during_unwind() {
    let fixture = env!("CARGO_BIN_EXE_solstone-core-system-test-child");
    let journal = TempJournal::new(fixture);
    let mut observed = None;

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut child = start(&journal, fixture);
        let records = wait_for_provider_records(&journal, &mut child);
        let providers = records.map(|record| {
            let process = &record["process"];
            (
                u32::try_from(process["pid"].as_u64().expect("provider pid"))
                    .expect("provider pid fits in u32"),
                u16::try_from(process["port"].as_u64().expect("provider port"))
                    .expect("provider port fits in u16"),
            )
        });
        observed = Some((child.id(), providers));
        panic!("exercise supervisor guard unwind cleanup");
    }));
    assert!(
        panic.is_err(),
        "fixture panic must unwind through the guard"
    );

    let (supervisor_pid, providers) = observed.expect("cleanup subjects captured before panic");
    for _ in 0..500 {
        let pids_gone = std::iter::once(supervisor_pid)
            .chain(providers.iter().map(|(pid, _)| *pid))
            .all(process_is_gone);
        let ports_free = providers
            .iter()
            .all(|(_, port)| TcpListener::bind(("127.0.0.1", *port)).is_ok());
        if pids_gone && ports_free {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let surviving_pids = std::iter::once(supervisor_pid)
        .chain(providers.iter().map(|(pid, _)| *pid))
        .filter(|pid| !process_is_gone(*pid))
        .collect::<Vec<_>>();
    let held_ports = providers
        .iter()
        .filter_map(|(_, port)| {
            TcpListener::bind(("127.0.0.1", *port))
                .is_err()
                .then_some(*port)
        })
        .collect::<Vec<_>>();
    panic!(
        "guard cleanup incomplete: surviving_pids={surviving_pids:?}, held_ports={held_ports:?}"
    );
}

fn process_is_gone(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    )
}
