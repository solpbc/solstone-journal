// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");

fn notification_listener(root: &Path) -> UnixListener {
    let health = root.join("health");
    fs::create_dir_all(&health).expect("create health directory");
    UnixListener::bind(health.join("callosum.sock")).expect("bind callosum socket")
}

fn notification(listener: &UnixListener) -> Value {
    let (mut stream, _) = listener.accept().expect("accept notification");
    let mut line = String::new();
    stream.read_to_string(&mut line).expect("read notification");
    serde_json::from_str(line.trim()).expect("valid notification JSON")
}

fn navigate(journal: &Path, args: &[&str]) -> Output {
    Command::new(BINARY)
        .arg("navigate")
        .args(args)
        .env("SOLSTONE_JOURNAL", journal)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run solstone-core navigate")
}

fn navigate_without_gate_override(journal: &Path, args: &[&str], spawned: bool) -> Output {
    let mut command = Command::new(BINARY);
    command
        .arg("navigate")
        .args(args)
        .env("SOLSTONE_JOURNAL", journal)
        .env_remove("SOL_SKIP_SUPERVISOR_CHECK");
    if spawned {
        command.env("SOL_SUPERVISOR_SPAWNED", "1");
    } else {
        command.env_remove("SOL_SUPERVISOR_SPAWNED");
    }
    command.output().expect("run solstone-core navigate")
}

#[test]
fn navigate_sends_a_path_only_request_after_the_gate() {
    let journal = tempfile::tempdir().expect("journal");
    let listener = notification_listener(journal.path());

    let output = navigate(journal.path(), &["/app/work"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert_eq!(output.stdout, b"Navigate: /app/work\n");
    assert_eq!(
        notification(&listener),
        json!({"tract": "navigate", "event": "request", "path": "/app/work"})
    );
}

#[test]
fn navigate_preserves_gate_and_parser_ordering() {
    let journal = tempfile::tempdir().expect("journal");
    let interactive = navigate_without_gate_override(journal.path(), &["/app/work"], false);
    assert_eq!(interactive.status.code(), Some(1));
    assert_eq!(interactive.stdout, b"");
    assert_eq!(
        interactive.stderr,
        b"journal isn't running. start it with 'journal up' and retry.\n"
    );

    let spawned = navigate_without_gate_override(journal.path(), &["/app/work"], true);
    assert_eq!(spawned.status.code(), Some(75));
    assert_eq!(spawned.stdout, b"");
    assert_eq!(spawned.stderr, b"");

    let no_args = navigate(journal.path(), &[]);
    assert_eq!(no_args.status.code(), Some(2));
    assert_eq!(no_args.stdout, b"");
    assert!(String::from_utf8_lossy(&no_args.stderr).starts_with("usage: journal navigate"));

    let malformed = navigate_without_gate_override(journal.path(), &["--nonsense"], false);
    assert_eq!(malformed.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&malformed.stderr).starts_with("usage: journal navigate"));

    for args in [
        ["--facet", "work", "/app/work"].as_slice(),
        ["/app/work", "--facet=work"].as_slice(),
        ["-f", "work", "/app/work"].as_slice(),
        ["/app/work", "-fwork"].as_slice(),
    ] {
        let journal = tempfile::tempdir().expect("journal");
        let listener = notification_listener(journal.path());
        listener.set_nonblocking(true).expect("set nonblocking");
        let output = navigate(journal.path(), args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(
            listener
                .accept()
                .expect_err("rejected navigate must not connect")
                .kind(),
            std::io::ErrorKind::WouldBlock,
            "{args:?}"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains(
            "Put facet selection in the destination URL; for example, /app/entities?facet=work."
        ));
    }
}

#[test]
fn navigate_reports_undeliverable_socket_shapes_without_success_output() {
    let journal = tempfile::tempdir().expect("journal");
    let health = journal.path().join("health");
    fs::create_dir_all(&health).expect("health");
    let socket = health.join("callosum.sock");

    let absent = navigate(journal.path(), &["/app/work"]);
    assert_undeliverable(&absent, &socket);

    fs::write(&socket, b"not a socket").expect("regular file");
    let regular_file = navigate(journal.path(), &["/app/work"]);
    assert_undeliverable(&regular_file, &socket);

    fs::remove_file(&socket).expect("remove regular file");
    let listener = UnixListener::bind(&socket).expect("listener");
    drop(listener);
    let stale = navigate(journal.path(), &["/app/work"]);
    assert_undeliverable(&stale, &socket);
}

fn assert_undeliverable(output: &Output, socket: &Path) {
    assert_eq!(output.status.code(), Some(69));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Navigate:"));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "journal navigate: error: Callosum socket unavailable: {}\n",
            socket.display()
        )
    );
}

/// This PATH-shim proof shows native navigate does not invoke a `python` or
/// `python3` found on `PATH`; it is not the sibling-interpreter proof. The
/// journal dispatcher resolves `python3` beside its own executable and never
/// reads `PATH`, so the sibling probe belongs to the cut wave.
#[test]
fn navigate_does_not_invoke_path_python() {
    let journal = tempfile::tempdir().expect("journal");
    let listener = notification_listener(journal.path());
    let shim = tempfile::tempdir().expect("shim");
    let marker = shim.path().join("python-called");
    for name in ["python", "python3"] {
        let path = shim.path().join(name);
        fs::write(
            &path,
            format!("#!/bin/sh\nprintf called > {}\n", marker.display()),
        )
        .expect("write shim");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make shim executable");
    }
    let inherited_path = std::env::var_os("PATH").expect("PATH");
    let mut path = shim.path().as_os_str().to_os_string();
    path.push(":");
    path.push(inherited_path);

    let output = Command::new(BINARY)
        .args(["navigate", "/app/work"])
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .env("PATH", path)
        .output()
        .expect("run solstone-core navigate");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        !marker.exists(),
        "native navigate must not invoke PATH Python"
    );
    assert_eq!(notification(&listener)["tract"], "navigate");
}
