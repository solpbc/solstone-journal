// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The paired-device sync routes must not be able to silence the carrier.
//!
//! Convey runs one two-worker Tokio runtime for every HTTP handler *and* for
//! `spl-home`'s carrier driver — the single task that answers a device's
//! keepalive PING and emits its WINDOW grants. A device-sync handler that folds
//! the journal on an async worker can therefore stop the journal answering the
//! very device it is serving, which the client reads as a dead link and tears
//! down mid-upload.
//!
//! These tests measure that property where it lives: on the wire, as a PING
//! answered or not answered across a real mTLS carrier, against a door served
//! by a runtime with production's two workers. The oracle is *whether a PONG
//! comes back at all* while a fold is held open indefinitely — not how fast it
//! comes back, which would be a timing test that passes by luck.

#![allow(dead_code)]

#[path = "door_support.rs"]
mod door_support;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use solstone_core_convey_http::owner_read::{OwnerReadRole, test_hooks};
use solstone_core_convey_shell::{ConveyServeOptions, DoorOutcome, router, serve};
use spl_core::frame::{FLAG_CLOSE, FLAG_DATA, FLAG_OPEN, FLAG_PONG, Frame, FrameDecoder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::door_support::Fixture;

/// Generous on purpose. The shipped client gives up after three 500 ms pings;
/// anything that needs longer than this has already lost the carrier, and a
/// healthy loopback PONG returns in under a millisecond either way.
const PONG_DEADLINE: Duration = Duration::from_secs(5);

/// The hooks are process-global, so the tests in this file run one at a time.
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Releases every held role on any exit path, including a panic. Without this a
/// failing assertion leaves a handler parked on a Convey worker and a runtime
/// that can never shut down, so the test hangs instead of reporting.
struct ReleaseOnDrop;

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        test_hooks::release_all();
    }
}

struct Door {
    port: u16,
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Door {
    /// Serve the door on its own runtime with production's two async workers,
    /// so the test's own client never competes for them. The runtime is built
    /// on a plain thread: Tokio refuses `block_on` from inside a runtime, and
    /// the test itself is async.
    fn start(fixture: &Fixture) -> Self {
        let journal_root = fixture.root.clone();
        let (port_tx, port_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("door runtime");
            let options = ConveyServeOptions {
                journal_root: journal_root.clone(),
                loopback_port: 0,
                door_port: 0,
                handshake_timeout: Duration::from_secs(5),
                stream_stall_timeout: Duration::from_secs(30),
                router: router(journal_root),
                carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
                handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
            };
            let handle = runtime.block_on(serve(options)).expect("door serves");
            let port = match handle.door_outcome() {
                DoorOutcome::Bound(address) => address.port(),
                other => panic!("door did not bind: {other:?}"),
            };
            port_tx.send(port).expect("door port reaches the test");
            // Park this thread; the runtime's two workers carry the door.
            let _ = stop_rx.recv();
            handle.shutdown();
        });
        let port = port_rx.recv().expect("door binds");
        Self {
            port,
            stop: Some(stop_tx),
            thread: Some(thread),
        }
    }
}

impl Drop for Door {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn carrier(
    fixture: &Fixture,
    port: u16,
) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
    let tcp = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .expect("TCP carrier");
    tokio_rustls::TlsConnector::from(Arc::new(fixture.client_config(0)))
        .connect(
            rustls::pki_types::ServerName::try_from("spl.local").expect("server name"),
            tcp,
        )
        .await
        .expect("mTLS carrier")
}

/// Open one device-sync request and leave it in flight. The reply is never
/// read: the point is to occupy the journal, not to finish the request.
async fn begin_device_request(
    carrier: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    stream_id: u32,
    path: &str,
) {
    let request = format!(
        "GET {path} HTTP/1.1\r\nhost: spl.local\r\nx-solstone-protocol-version: 3\r\ncontent-length: 0\r\n\r\n"
    );
    carrier
        .write_all(
            &Frame::new(stream_id, FLAG_OPEN | FLAG_DATA, request.into_bytes())
                .encode()
                .expect("request frame"),
        )
        .await
        .expect("request writes");
    carrier
        .write_all(
            &Frame::new(stream_id, FLAG_CLOSE, Vec::new())
                .encode()
                .expect("request close"),
        )
        .await
        .expect("close writes");
}

/// Write a PING and wait for its matching PONG, ignoring application frames.
/// `true` means the journal answered inside `deadline`.
async fn carrier_answers_ping(
    carrier: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    decoder: &mut FrameDecoder,
    nonce: [u8; 8],
    deadline: Duration,
) -> bool {
    carrier
        .write_all(&Frame::control_ping(nonce).encode().expect("ping frame"))
        .await
        .expect("ping writes");
    carrier.flush().await.expect("ping flushes");

    let mut buffer = [0_u8; 64 * 1024];
    let answered = async {
        loop {
            let read = carrier.read(&mut buffer).await.expect("carrier reads");
            if read == 0 {
                return false;
            }
            decoder.feed(&buffer[..read]);
            for frame in decoder.drain().expect("carrier frames") {
                if frame.stream_id == 0
                    && frame.flags == FLAG_PONG
                    && frame.control_pong_nonce() == Some(nonce)
                {
                    return true;
                }
            }
        }
    };
    tokio::time::timeout(deadline, answered)
        .await
        .unwrap_or(false)
}

/// Wait for `count` handlers to reach the held fold, polling from the async
/// side so no blocking-pool thread is stranded if they never do.
async fn folds_started(role: OwnerReadRole, count: usize, within: Duration) -> usize {
    let started = tokio::time::Instant::now();
    loop {
        let entered = test_hooks::entered(role);
        if entered >= count || started.elapsed() >= within {
            return entered;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Hold both of the door's async workers inside one device-sync fold and assert
/// the carrier still answers.
///
/// This is the root-cause measurement for `req_7yqrbmow`. Before the folds moved
/// to the blocking pool it failed exactly as reported: both async workers sat
/// inside `day_listing`, `run_driver` was never polled, and no PONG came back at
/// all — which is what tore down the reporter's uploads.
async fn assert_role_leaves_the_carrier_answering(role: OwnerReadRole) {
    let fixture = Fixture::established(1);
    let door = Door::start(&fixture);
    let mut carrier = carrier(&fixture, door.port).await;
    let mut decoder = FrameDecoder::new();

    assert!(
        carrier_answers_ping(&mut carrier, &mut decoder, [1; 8], PONG_DEADLINE).await,
        "an idle carrier did not answer a PING; the fixture is broken, not the journal"
    );

    test_hooks::reset();
    let _release = ReleaseOnDrop;
    test_hooks::hold(role);

    // Two requests in one write, so the driver reads both before either handler
    // can occupy a worker. One held fold leaves a worker free and proves nothing.
    begin_device_request(&mut carrier, 3, role.probe_uri()).await;
    begin_device_request(&mut carrier, 5, role.probe_uri()).await;
    carrier.flush().await.expect("requests flush");

    let started = folds_started(role, 2, Duration::from_secs(10)).await;
    assert!(
        started >= 2,
        "only {started} of 2 {role:?} folds started within 10 s — the door could not \
         even dispatch the second request, which is itself worker starvation"
    );

    let answered = carrier_answers_ping(&mut carrier, &mut decoder, [2; 8], PONG_DEADLINE).await;
    assert!(
        answered,
        "no PONG within {PONG_DEADLINE:?} while two {role:?} folds were held open — \
         the carrier driver is sharing their workers, and a real client would have \
         declared the link dead after three 500 ms pings"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_device_fold_does_not_silence_the_carrier() {
    let _lock = TEST_LOCK.lock().await;
    assert_role_leaves_the_carrier_answering(OwnerReadRole::DeviceIngestManifest).await;
    test_hooks::reset();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_device_sync_read_leaves_the_carrier_answering() {
    let _lock = TEST_LOCK.lock().await;
    for &role in OwnerReadRole::DEVICE_SYNC {
        if role == OwnerReadRole::DeviceIngestUpload {
            // A POST needs a multipart body; `devices_ingest_mount` owns that
            // route's contract. One route per shape is enough on the wire.
            continue;
        }
        assert_role_leaves_the_carrier_answering(role).await;
        test_hooks::reset();
    }
}
