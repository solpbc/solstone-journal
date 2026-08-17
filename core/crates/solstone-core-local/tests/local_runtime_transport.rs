// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};

use solstone_core_local::LoopbackAddr;
use solstone_core_local::admission::acquire_local_slot;
use solstone_core_local::connect::{ConnectInput, ConnectOutcome, connect};
use solstone_core_local::plan::Platform;

const INPUT_SCHEMA: &str = "solstone-local-connect-input-v1";

fn journal(port: Option<u16>) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temp journal");
    let health = root.path().join("health");
    std::fs::create_dir_all(&health).expect("health directory");
    if let Some(port) = port {
        std::fs::write(health.join("local.port"), port.to_string()).expect("port");
    }
    root
}

fn input(root: &Path) -> ConnectInput {
    ConnectInput {
        schema: INPUT_SCHEMA.into(),
        journal_path: root.display().to_string(),
        bind_address: LoopbackAddr::IPV4_LOOPBACK,
        default_model_id: "default".into(),
        platform: Platform::Linux,
    }
}

#[test]
fn connect_reports_transport_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    drop(listener);
    let root = journal(Some(port));
    assert!(matches!(
        connect(input(root.path())),
        ConnectOutcome::Failed { .. }
    ));
}

#[test]
fn health_response_timeout_is_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let root = journal(Some(port));
    assert!(matches!(
        connect(input(root.path())),
        ConnectOutcome::Failed { .. }
    ));
    drop(listener);
}

#[test]
fn child_holds_slot_until_killed() {
    if std::env::var_os("SOLSTONE_ADMISSION_HOLDER").is_none() {
        return;
    }
    let root =
        std::path::PathBuf::from(std::env::var("SOLSTONE_ADMISSION_ROOT").expect("child root"));
    let _permit = acquire_local_slot(&root, 1, Some(std::time::Duration::from_secs(2)), false)
        .expect("child permit");
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(b"ready\n").expect("signal ready");
    stdout.flush().expect("flush ready");
    std::thread::park();
}

#[test]
fn killed_holder_releases_slot_without_repair() {
    let root = tempfile::tempdir().expect("admission root");
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "child_holds_slot_until_killed", "--nocapture"])
        .env("SOLSTONE_ADMISSION_HOLDER", "1")
        .env("SOLSTONE_ADMISSION_ROOT", root.path())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn holder");
    let stdout = child.stdout.take().expect("child stdout");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("read ready");
    assert_eq!(line, "ready\n");
    child.kill().expect("kill holder");
    child.wait().expect("reap holder");
    let reclaimed = acquire_local_slot(
        root.path(),
        1,
        Some(std::time::Duration::from_millis(200)),
        false,
    )
    .expect("kernel releases flock on process death");
    drop(reclaimed);
}
