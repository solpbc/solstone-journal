// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

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

#[test]
fn convey_never_reaches_an_interpreter_shim() {
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

    for (verb, argv) in [("convey", vec!["--nonsense"])] {
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
    for name in ["python", "python3", "pytest", "uv", "ruff"] {
        assert!(!temp.join(name).exists(), "{name} shim was reached");
    }
    let _ = fs::remove_dir_all(temp);
}
