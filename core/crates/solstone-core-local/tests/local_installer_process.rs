// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, ErrorKind};
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use solstone_core_local::install::{lease, test_hooks};

const RECEIVE_TIMEOUT: Duration = Duration::from_secs(2);
const PARAKEET_TEST_KEY: &str = "x86_64-unknown-linux-gnu";

struct LeaseCase {
    socket: UnixDatagram,
    root: tempfile::TempDir,
}

impl LeaseCase {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("solstone-local-installer-process-")
            .tempdir()
            .expect("create lease journal root");
        let socket = UnixDatagram::bind(root.path().join("lease-probe.sock"))
            .expect("bind case-bound lease probe socket");
        socket
            .set_read_timeout(Some(RECEIVE_TIMEOUT))
            .expect("set lease probe receive timeout");
        Self { socket, root }
    }

    fn run_child(&self) -> ReapedChild {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "--ignored", "lease_child_probe"])
            .env("SOLSTONE_LOCAL_LEASE_HELPER_ROOT", self.root.path())
            .env(
                "SOLSTONE_LOCAL_LEASE_HELPER_SOCKET",
                self.root.path().join("lease-probe.sock"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run lease child probe");
        ReapedChild(Some(child))
    }

    fn receive(&self) -> String {
        let mut message = [0_u8; 64];
        let length = self
            .socket
            .recv(&mut message)
            .expect("receive case-bound lease probe result");
        std::str::from_utf8(&message[..length])
            .expect("lease probe result is utf-8")
            .to_owned()
    }

    fn assert_no_extra_message(&self) {
        self.socket
            .set_nonblocking(true)
            .expect("make lease probe socket nonblocking");
        let mut message = [0_u8; 64];
        match self.socket.recv(&mut message) {
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Ok(_) => panic!("lease probe sent more than one result"),
            Err(error) => panic!("check lease probe result count: {error}"),
        }
        self.socket
            .set_nonblocking(false)
            .expect("restore lease probe socket blocking mode");
    }
}

struct ReapedChild(Option<Child>);

impl ReapedChild {
    fn reap(mut self) -> ExitStatus {
        self.0
            .take()
            .expect("child is present")
            .wait()
            .expect("reap lease probe child")
    }
}

impl Drop for ReapedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

fn child_result(root: &Path) -> Result<&'static str, String> {
    match lease::acquire(root, "local") {
        Ok(None) => Ok("contended"),
        Ok(Some(_lease)) => Ok("acquired-and-released"),
        Err(error) => Err(format!("lease acquire failed: {error}")),
    }
}

#[test]
fn two_real_processes_cannot_hold_the_same_lease() {
    let case = LeaseCase::new();
    let held = lease::acquire(case.root.path(), "local").unwrap().unwrap();
    let child = case.run_child();
    assert_eq!(case.receive(), "contended");
    assert!(child.reap().success(), "contended child must exit cleanly");
    case.assert_no_extra_message();
    drop(held);
    let child = case.run_child();
    assert_eq!(case.receive(), "acquired-and-released");
    assert!(
        child.reap().success(),
        "acquired-and-released child must exit cleanly"
    );
    case.assert_no_extra_message();
}

#[test]
fn inspect_parakeet_reports_ready_per_artifact_proofs() {
    let root = tempfile::Builder::new()
        .prefix("solstone-local-parakeet-ready-")
        .tempdir()
        .expect("create ready Parakeet journal");
    let _fixture = test_hooks::stage_ready_parakeet(root.path(), PARAKEET_TEST_KEY, true);
    let result = test_hooks::inspect_parakeet(root.path(), PARAKEET_TEST_KEY);

    assert_eq!(result["provider"], "parakeet");
    assert_eq!(result["target"]["artifact_key"], PARAKEET_TEST_KEY);
    assert_eq!(result["status"], "ready");
    assert_eq!(result["reason_code"], "ready");
    assert_eq!(result["ready"], true);
    assert_eq!(result["in_flight"], false);
    assert_eq!(result["artifacts"]["binary_installed"], true);
    assert_eq!(result["artifacts"]["binary_runnable"], true);
    for name in ["binary", "binary_cpu", "binary_vulkan", "model"] {
        assert_eq!(result["proof"][name]["status"], "ready", "{name}");
        assert!(result["proof"][name].get("cache_hit").is_none(), "{name}");
    }
    assert!(result["install"].is_object());
}

#[test]
fn inspect_parakeet_reports_held_lease_without_creating_a_lease() {
    let root = tempfile::Builder::new()
        .prefix("solstone-local-parakeet-lease-")
        .tempdir()
        .expect("create lease Parakeet journal");
    let _fixture = test_hooks::stage_ready_parakeet(root.path(), PARAKEET_TEST_KEY, true);
    let lease_path = lease::lease_path(root.path(), "parakeet");
    assert!(!lease_path.exists());
    let unlocked = test_hooks::inspect_parakeet(root.path(), PARAKEET_TEST_KEY);
    assert_eq!(unlocked["in_flight"], false);
    assert!(!lease_path.exists());

    let held = lease::acquire(root.path(), "parakeet")
        .expect("acquire Parakeet lease")
        .expect("lease is available");
    let locked = test_hooks::inspect_parakeet(root.path(), PARAKEET_TEST_KEY);
    assert_eq!(locked["in_flight"], true);
    assert_eq!(locked["ready"], true);
    assert_eq!(locked["status"], "ready");
    drop(held);
}

#[test]
fn inspect_parakeet_reports_unrunnable_cpu_binary() {
    let root = tempfile::Builder::new()
        .prefix("solstone-local-parakeet-unrunnable-")
        .tempdir()
        .expect("create unrunnable Parakeet journal");
    let _fixture = test_hooks::stage_ready_parakeet(root.path(), PARAKEET_TEST_KEY, false);
    let result = test_hooks::inspect_parakeet(root.path(), PARAKEET_TEST_KEY);

    assert_eq!(result["status"], "host-ineligible");
    assert_eq!(result["reason_code"], "binary_unavailable");
    assert_eq!(result["artifacts"]["binary_runnable"], false);
    assert_eq!(
        result["host"]["binary_runtime"]["reason_code"],
        "binary_unavailable"
    );
    for name in ["binary", "binary_cpu", "binary_vulkan", "model"] {
        assert_eq!(result["proof"][name]["status"], "ready", "{name}");
    }
}

#[test]
#[ignore]
fn lease_child_probe() {
    let root = std::env::var("SOLSTONE_LOCAL_LEASE_HELPER_ROOT")
        .map_err(|_| "SOLSTONE_LOCAL_LEASE_HELPER_ROOT must name the lease journal")
        .map(std::path::PathBuf::from)
        .unwrap();
    let socket_path = std::env::var("SOLSTONE_LOCAL_LEASE_HELPER_SOCKET")
        .map_err(|_| "SOLSTONE_LOCAL_LEASE_HELPER_SOCKET must name the case socket")
        .unwrap();
    let result = child_result(&root).unwrap();
    let sender = UnixDatagram::unbound().unwrap();
    sender
        .send_to(result.as_bytes(), socket_path)
        .map(|_| ())
        .unwrap_or_else(|error: io::Error| panic!("send lease probe result: {error}"));
}
