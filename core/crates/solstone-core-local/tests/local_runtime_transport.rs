// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use solstone_core_local::LoopbackAddr;
use solstone_core_local::admission::{AdmissionError, acquire_local_slot};
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

fn closed_peer() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind closed peer");
    let port = listener.local_addr().expect("closed peer address").port();
    let peer = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept health client");
        drop(stream);
    });
    (port, peer)
}

struct ReapChild(Option<std::process::Child>);

impl ReapChild {
    fn spawn(root: &Path, ready_socket: &Path) -> Self {
        Self::spawn_holder(root, ready_socket, false)
    }

    /// Spawns a holder that releases its slot on its own after `TIMED_HOLD`
    /// instead of parking until it is killed.
    fn spawn_timed(root: &Path, ready_socket: &Path) -> Self {
        Self::spawn_holder(root, ready_socket, true)
    }

    fn spawn_holder(root: &Path, ready_socket: &Path, timed_hold: bool) -> Self {
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args(["--exact", "child_holds_slot_until_killed", "--nocapture"])
            .env("SOLSTONE_ADMISSION_HOLDER", "1")
            .env("SOLSTONE_ADMISSION_ROOT", root)
            .env("SOLSTONE_ADMISSION_READY_SOCKET", ready_socket)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if timed_hold {
            command.env("SOLSTONE_ADMISSION_TIMED_HOLD", "1");
        }
        Self(Some(command.spawn().expect("spawn holder")))
    }

    fn kill_wait(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            child.wait().expect("reap holder");
        }
    }

    fn wait_success(&mut self) {
        let mut child = self.0.take().expect("holder already reaped");
        let status = child.wait().expect("reap timed holder");
        assert!(status.success(), "{status:?}");
    }
}

impl Drop for ReapChild {
    fn drop(&mut self) {
        self.kill_wait();
    }
}

struct WithholdingPeer {
    address: SocketAddr,
    release: Option<mpsc::SyncSender<()>>,
    join: Option<thread::JoinHandle<()>>,
}

impl WithholdingPeer {
    fn spawn(listener: TcpListener, accepted: mpsc::SyncSender<()>) -> Self {
        let address = listener.local_addr().expect("peer address");
        let (release, released) = mpsc::sync_channel(1);
        let join = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept health client");
            let _ = accepted.try_send(());
            let _ = released.recv();
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        });
        Self {
            address,
            release: Some(release),
            join: Some(join),
        }
    }
}

impl Drop for WithholdingPeer {
    fn drop(&mut self) {
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(200));
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[test]
fn connect_reports_closed_peer_transport_failure() {
    let (port, peer) = closed_peer();
    let root = journal(Some(port));
    let outcome = connect(input(root.path()));
    peer.join().expect("join closed peer");
    assert!(
        matches!(
            &outcome,
            ConnectOutcome::Failed { .. }
        ),
        "{outcome:?}"
    );
}

#[test]
fn health_response_timeout_is_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let root = journal(Some(port));
    let (accepted, receiver) = mpsc::sync_channel(1);
    let peer = WithholdingPeer::spawn(listener, accepted);
    let input = input(root.path());
    let connect = thread::spawn(move || connect(input));
    receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("health peer accepted the client");
    let outcome = connect.join().expect("connect thread");
    drop(peer);
    assert!(matches!(
        outcome,
        ConnectOutcome::Failed { ref reason } if reason.to_ascii_lowercase().contains("timeout")
    ));
}

const TIMED_HOLD: std::time::Duration = std::time::Duration::from_millis(400);

#[test]
fn child_holds_slot_until_killed() {
    if std::env::var_os("SOLSTONE_ADMISSION_HOLDER").is_none() {
        return;
    }
    let root =
        std::path::PathBuf::from(std::env::var("SOLSTONE_ADMISSION_ROOT").expect("child root"));
    let _permit = acquire_local_slot(&root, 1, Some(std::time::Duration::from_secs(2)), false)
        .expect("child permit");
    let ready_socket = std::env::var("SOLSTONE_ADMISSION_READY_SOCKET").expect("ready socket");
    let ready = UnixDatagram::unbound().expect("create readiness socket");
    ready
        .connect(ready_socket)
        .expect("connect readiness socket");
    ready.send(b"ready").expect("signal ready");
    if std::env::var_os("SOLSTONE_ADMISSION_TIMED_HOLD").is_some() {
        std::thread::sleep(TIMED_HOLD);
        return;
    }
    std::thread::park();
}

#[test]
fn killed_holder_releases_slot_without_repair() {
    let root = tempfile::tempdir().expect("admission root");
    let ready_path = root.path().join("holder-ready.sock");
    let ready = UnixDatagram::bind(&ready_path).expect("bind readiness socket");
    ready
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("bound readiness timeout");
    let mut child = ReapChild::spawn(root.path(), &ready_path);
    let mut message = [0_u8; 16];
    let length = ready.recv(&mut message).expect("receive child readiness");
    assert_eq!(&message[..length], b"ready");
    assert!(matches!(
        acquire_local_slot(root.path(), 1, Some(Duration::from_millis(200)), false),
        Err(AdmissionError::Timeout)
    ));
    child.kill_wait();
    let reclaimed = acquire_local_slot(
        root.path(),
        1,
        Some(std::time::Duration::from_millis(200)),
        false,
    )
    .expect("kernel releases flock on process death");
    drop(reclaimed);
}

#[test]
fn rust_waits_for_a_released_child_slot() {
    let root = tempfile::tempdir().expect("admission root");
    let ready_path = root.path().join("timed-holder-ready.sock");
    let ready = UnixDatagram::bind(&ready_path).expect("bind readiness socket");
    ready
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("bound readiness timeout");
    let mut child = ReapChild::spawn_timed(root.path(), &ready_path);
    let mut message = [0_u8; 16];
    let length = ready.recv(&mut message).expect("receive child readiness");
    assert_eq!(&message[..length], b"ready");

    let started = std::time::Instant::now();
    let permit = acquire_local_slot(root.path(), 1, Some(Duration::from_secs(2)), false)
        .expect("waiter acquires after the child releases");
    assert!(
        started.elapsed() >= TIMED_HOLD / 2,
        "waiter acquired before the child released its flock: {:?}",
        started.elapsed()
    );
    drop(permit);
    child.wait_success();
}
