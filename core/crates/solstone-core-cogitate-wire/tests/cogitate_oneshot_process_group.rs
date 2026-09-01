// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-group cancellation for CogitateOneShotClient.
//!
//! The client must not create a new process group. A SIGTERM delivered to the
//! caller's group must therefore terminate the spawned one-shot child.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use solstone_core_cogitate_wire::{CogitateOneShotClient, CogitateRequest, REQUEST_SCHEMA};

const CHILD_ENV: &str = "SOLSTONE_COGITATE_PG_CHILD";
const STUB_ENV: &str = "SOLSTONE_COGITATE_PG_STUB";

fn request() -> CogitateRequest {
    CogitateRequest::from_value(&json!({
        "schema": REQUEST_SCHEMA,
        "access_tier": "normal",
        "max_turns": 4,
        "cost_cap_usd": 1.5,
        "timeout_ms": 30_000,
        "read_call_budget": 5,
        "model": "fixture-model",
        "correlation_id": "corr-pg",
        "initial_prompt": "Do the task.",
        "journal_root": "/var/tmp/solstone-cogitate-pg-test",
        "diagnostic": false,
        "dry_run": false
    }))
    .expect("fixture request is valid")
}

fn alive(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

fn kill_group(pgid: i32, signal: &str) -> bool {
    Command::new("python3")
        .args([
            "-c",
            "import os, signal, sys; os.killpg(int(sys.argv[1]), getattr(signal, sys.argv[2]))",
            &pgid.to_string(),
            signal,
        ])
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn one_shot_child_dies_with_the_caller_process_group() {
    if std::env::var_os(CHILD_ENV).is_some() {
        let stub = std::env::var(STUB_ENV).expect("stub path");
        let client = CogitateOneShotClient::at_path(stub);
        let _ = client.execute(&request());
        return;
    }

    let root = PathBuf::from("/var/tmp").join(format!(
        "solstone-cogitate-pg-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let stub = root.join("sleep-stub.sh");
    let pidfile = root.join("child.pid");
    fs::write(
        &stub,
        format!(
            "#!/bin/sh\necho $$ > '{}'\ncat >/dev/null\nexec sleep 30\n",
            pidfile.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&stub).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&stub, permissions).unwrap();

    let exe = std::env::current_exe().expect("current test executable");
    let mut command = Command::new(&exe);
    command
        .arg("--exact")
        .arg("one_shot_child_dies_with_the_caller_process_group")
        .env(CHILD_ENV, "1")
        .env(STUB_ENV, &stub)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let mut babysitter = command.spawn().expect("babysitter starts");
    let pgid = i32::try_from(babysitter.id()).expect("pid fits i32");

    let deadline = Instant::now() + Duration::from_secs(3);
    let child_pid = loop {
        if let Ok(text) = fs::read_to_string(&pidfile)
            && let Ok(pid) = text.trim().parse::<i32>()
        {
            break pid;
        }
        if Instant::now() >= deadline {
            let _ = babysitter.kill();
            panic!("sleep stub never wrote its pid");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(alive(child_pid), "stub must be running before group kill");

    assert!(
        kill_group(pgid, "SIGTERM"),
        "os.killpg(pgid, SIGTERM) must succeed"
    );

    let wait_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if !alive(child_pid) {
            break;
        }
        if Instant::now() >= wait_deadline {
            let _ = kill_group(pgid, "SIGKILL");
            panic!("one-shot child survived SIGTERM to its process group");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = babysitter.wait();
}

#[test]
fn spawned_child_shares_the_caller_process_group() {
    if std::env::var_os(CHILD_ENV).is_some() {
        return;
    }
    let root = PathBuf::from("/var/tmp").join(format!(
        "solstone-cogitate-pgid-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let stub = root.join("pgid-stub.sh");
    fs::write(
        &stub,
        "#!/bin/sh\ncat >/dev/null\npgid=$(ps -o pgid= -p $$ | tr -d ' ')\nprintf '{\"event\":\"finish\",\"terminal\":true,\"result\":\"%s\"}\\n' \"$pgid\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&stub).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&stub, permissions).unwrap();
    let run = CogitateOneShotClient::at_path(&stub)
        .execute(&request())
        .expect("stub succeeds");
    let child = run.events[0]["result"].as_str().unwrap();
    let parent = Command::new("ps")
        .args(["-o", "pgid=", "-p"])
        .arg(std::process::id().to_string())
        .output()
        .expect("ps");
    let parent = String::from_utf8_lossy(&parent.stdout).trim().to_owned();
    assert_eq!(child, parent);
}
