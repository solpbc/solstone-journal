// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_local::install::{archive, manifest, pins};

struct TempJournal(PathBuf);
impl TempJournal {
    fn new(fixture: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-core-supervisor-providers-{stamp}"));
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1},"transcribe":{"backend":"parakeet-cpp","parakeet-cpp":{"device":"cpu"}}}"#,
        )
        .expect("journal config");
        install_native_local_readiness(&root);
        let parakeet_paths =
            solstone_core_local::install::pins::parakeet_paths(&root, "x86_64-unknown-linux-gnu");
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
    let runtime = cache.join("bin/aarch64-apple-darwin/b10068");
    let model = cache.join("models/local__qwen3.5-4b");
    fs::create_dir_all(&runtime).expect("runtime directory");
    fs::create_dir_all(&model).expect("model directory");
    fs::write(runtime.join("llama-server"), b"#!/bin/sh\nexit 0\n").expect("runtime");
    archive::make_executable(&runtime.join("llama-server")).expect("executable runtime");
    fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"model").expect("model");
    fs::write(model.join("mmproj-F16.gguf"), b"projector").expect("projector");
    let runtime_manifest = manifest::build_manifest(
        "local",
        "llama-server-vulkan",
        "test",
        json!({"pin_identity":pins::vulkan_identity("aarch64-apple-darwin").unwrap()}),
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
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(self.0.id() as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
            for _ in 0..1_000 {
                if self.0.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn ac12_local_and_parakeet_reconcile_real_fixture_cycles() {
    let fixture = env!("CARGO_BIN_EXE_solstone-core-system-test-child");
    let journal = TempJournal::new(fixture);
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(["supervisor", "--journal"])
            .arg(&journal.0)
            .env("SOLSTONE_LOCAL_BINARY", fixture)
            .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
            .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
            .env("SOLSTONE_SUPERVISOR_APP_BINARY", fixture)
            .env("SOLSTONE_SUPERVISOR_PARAKEET_FIXTURE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("supervisor starts"),
    );
    let local = journal.0.join("health/providers/runtime/local.json");
    let parakeet = journal.0.join("health/providers/runtime/parakeet.json");
    let mut local_ready = false;
    let mut parakeet_ready = false;
    let mut parakeet_state = None;
    for _ in 0..1200 {
        if let Ok(bytes) = fs::read(&local)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            local_ready |= value["phase"] == "ready";
        }
        if let Ok(bytes) = fs::read(&parakeet)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            parakeet_ready |= value["phase"] == "ready";
            parakeet_state = Some(value);
        }
        if local_ready && parakeet_ready {
            break;
        }
        if let Some(status) = child.0.try_wait().expect("supervisor status") {
            panic!("supervisor exited: {status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        local_ready,
        "Local did not complete the fixture desired/start/truth/ready cycle"
    );
    assert!(
        parakeet_ready,
        "Parakeet did not complete the fixture desired/start/truth/ready cycle: {parakeet_state:?}"
    );
}
