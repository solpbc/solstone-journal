// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn established_journal(root: &std::path::Path) {
    fs::create_dir_all(root.join("config")).expect("journal config creates");
    fs::write(
        root.join("config/journal.json"),
        br#"{"setup":{"completed_at":1767225600}}"#,
    )
    .expect("journal config writes");
}

fn stage_layout_anchors(prefix: &std::path::Path) {
    for relative in [
        solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
        solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
        solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
    ] {
        let path = prefix.join("share").join(relative);
        fs::create_dir_all(path.parent().expect("layout anchor has parent"))
            .expect("layout anchor parent creates");
        fs::write(path, b"fixture").expect("layout anchor writes");
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("free port reserves");
    listener.local_addr().expect("free port reads").port()
}

fn wait_for_port_file(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "Convey did not write readiness port"
        );
        thread::yield_now();
    }
}

fn terminate_and_reap(child: &mut Child) {
    child.kill().expect("native Convey terminates");
    for _ in 0..1_000 {
        if child
            .try_wait()
            .expect("native Convey status reads")
            .is_some()
        {
            child.wait().expect("native Convey reaps");
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("native Convey did not exit after kill");
}

fn run_controlled_restart(
    core: &std::path::Path,
    journal: &std::path::Path,
    bin: &std::path::Path,
    poison: &std::path::Path,
) {
    let health = journal.join("health");
    fs::create_dir_all(&health).expect("health creates");
    let socket = health.join("callosum.sock");
    let listener = UnixListener::bind(&socket).expect("controlled Callosum binds");
    let port = 5015_u16;
    let peer_journal = journal.to_path_buf();
    let peer = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("restart client connects");
        let read = stream.try_clone().expect("stream clones");
        let mut line = String::new();
        BufReader::new(read)
            .read_line(&mut line)
            .expect("restart request reads");
        let request: Value = serde_json::from_str(&line).expect("restart request JSON");
        let restart_id = request["restart_id"].as_str().expect("restart id");
        let replacement = peer_journal.join("health/.convey.port.replacement");
        fs::write(&replacement, port.to_string()).expect("fresh port writes");
        fs::rename(replacement, peer_journal.join("health/convey.port"))
            .expect("fresh port renames");
        let event = serde_json::to_vec(&json!({
            "tract": "supervisor", "event": "started", "service": "convey",
            "restart_id": restart_id, "pid": 321, "ref": "supervisor-app-convey",
        }))
        .expect("started event JSON");
        stream.write_all(&event).expect("started event writes");
        stream.write_all(b"\n").expect("started event frames");
    });
    let output = Command::new(core)
        .args(["restart-convey", "--timeout", "1"])
        .env("PATH", bin)
        .env("POISON_DIR", poison)
        .env("SOLSTONE_JOURNAL", journal)
        .output()
        .expect("native restart runs");
    peer.join().expect("controlled peer completes");
    assert!(
        output.status.success(),
        "native restart: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn convey_and_restart_convey_never_reach_an_interpreter_shim() {
    let temp = std::path::Path::new("/var/tmp")
        .join(format!("solstone-convey-poison-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    let bin = temp.join("bin");
    fs::create_dir_all(&bin).expect("poison bin creates");
    let core = bin.join("solstone-core");
    fs::copy(env!("CARGO_BIN_EXE_solstone-core"), &core).expect("core copies");
    fs::set_permissions(&core, fs::Permissions::from_mode(0o755)).expect("core executable");
    stage_layout_anchors(&temp);

    for name in ["python", "python3", "pytest", "uv", "ruff"] {
        let shim = bin.join(name);
        fs::write(
            &shim,
            format!("#!/bin/sh\nprintf '%s' '{name}' > \"$POISON_DIR/{name}\"\nexit 97\n"),
        )
        .expect("shim writes");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("shim executable");
        let status = Command::new(&shim)
            .env("POISON_DIR", &temp)
            .status()
            .expect("shim runs");
        assert_eq!(status.code(), Some(97), "{name} instrument proof");
        assert_eq!(fs::read_to_string(temp.join(name)).expect("marker"), name);
        fs::remove_file(temp.join(name)).expect("marker clears");
    }

    for (verb, argv) in [
        ("convey", vec!["--nonsense"]),
        ("restart-convey", vec!["--nonsense"]),
    ] {
        let output = Command::new(&core)
            .arg(verb)
            .args(argv)
            .env("PATH", &bin)
            .env("POISON_DIR", &temp)
            .output()
            .expect("native grammar probe runs");
        assert_eq!(output.status.code(), Some(2), "{verb} native grammar exit");
    }
    let journal = temp.join("journal");
    established_journal(&journal);
    let port = free_port();
    let mut convey = Command::new(&core)
        .args(["convey", "--port", &port.to_string(), "--journal"])
        .arg(&journal)
        .env("PATH", &bin)
        .env("POISON_DIR", &temp)
        .spawn()
        .expect("native Convey starts");
    wait_for_port_file(&journal.join("health/convey.port"));
    terminate_and_reap(&mut convey);
    run_controlled_restart(&core, &journal, &bin, &temp);
    for name in ["python", "python3", "pytest", "uv", "ruff"] {
        assert!(!temp.join(name).exists(), "{name} shim was reached");
    }
    let _ = fs::remove_dir_all(temp);
}
