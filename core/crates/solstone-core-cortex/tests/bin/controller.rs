// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use solstone_core_cortex::test_hooks::{CortexStore, Work, new_state, spawn_one};

fn main() {
    let executable_dir = PathBuf::from(required("CORTEX_EXECUTABLE_DIR"));
    let talent_root = PathBuf::from(required("CORTEX_TALENT_ROOT"));
    let apps_root = PathBuf::from(required("CORTEX_APPS_ROOT"));
    let templates_dir = PathBuf::from(required("CORTEX_TEMPLATES_DIR"));
    let journal = PathBuf::from(required("CORTEX_JOURNAL"));
    let request_path = required("CORTEX_REQUEST_PATH");
    let status_path = required("CORTEX_STATUS_PATH");
    let request: Map<String, Value> =
        serde_json::from_slice(&fs::read(&request_path).expect("request")).expect("request json");
    let use_id = request
        .get("use_id")
        .and_then(Value::as_str)
        .expect("use_id")
        .to_owned();
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .expect("name")
        .to_owned();
    let store = CortexStore::new(journal).expect("store");
    let (active, identity) = store
        .claim(&name, &use_id, &request)
        .expect("claim")
        .expect("claimed");
    let state = new_state(store);
    let result = spawn_one(
        state,
        executable_dir,
        &talent_root,
        &apps_root,
        &templates_dir,
        Work {
            use_id,
            talent_name: name,
            active,
            identity,
            request,
        },
        None,
    );
    match result {
        Ok(()) => fs::write(&status_path, "ok").expect("status"),
        Err(error) => fs::write(&status_path, format!("err:{error}")).expect("status"),
    }
    if let Ok(receipt) = env::var("CORTEX_RECEIPT_PATH") {
        poll_until_exists(Path::new(&receipt), Duration::from_secs(2));
    }
}

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn poll_until_exists(path: &Path, timeout: Duration) {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("receipt {} was not written", path.display());
}
