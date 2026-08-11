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
        let snapshot = root.join("cache/providers/local/mlx/mlx-community--Qwen3.5-9B-MLX-8bit/84f7c2deea248d8df56240f88102def51c7ed5d6");
        fs::create_dir_all(snapshot.join("snapshot")).expect("fixture snapshot");
        fs::write(
            snapshot.join("snapshot.manifest.json"),
            json!({
                "schema_version": 1, "provider": "local", "unit": "mlx-snapshot",
                "target_fingerprint_sha256": "test", "created_by_attempt_id": null,
                "external_root": null,
                "source": {"pin_identity": {"unit": "mlx-snapshot", "model_id": "qwen3.5:9b",
                    "repo": "mlx-community/Qwen3.5-9B-MLX-8bit",
                    "revision": "84f7c2deea248d8df56240f88102def51c7ed5d6",
                    "size_bytes": 10453446077u64}}, "inventory": []
            })
            .to_string(),
        )
        .expect("snapshot manifest");
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
