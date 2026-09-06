// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use nix::sys::stat::Mode;
use nix::unistd::mkfifo;
use serde_json::{Value, json};
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_convey_shell::authorization_gate::{
    AuthorizationGateReadProbe, authorized_router, authorized_router_with_read_probe,
};
use solstone_core_convey_shell::{
    ConveyServeOptions, DoorOutcome, bind_with_authorization, router,
};
use solstone_core_sol_link::DeviceDoorAuthorization;
use solstone_core_sol_link::ledger::{AuthorizationLedger, AuthorizedClientsRead};
use tokio::io::AsyncReadExt;
use tokio::sync::watch;
use tower::ServiceExt;

use crate::door_support::{Fixture, get_over_carrier};
use crate::warn_capture;

fn linked_device(fixture: &Fixture, index: usize) -> AccessBasis {
    let cid = format!(
        "sha256:{}",
        spl_core::ca::sha256_hex(fixture.client_der(index))
    );
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        cid: LinkedDeviceCid::try_from(cid.as_str()).expect("fixture CID"),
    }
}

fn unlisted_linked_device() -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        cid: LinkedDeviceCid::try_from(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("syntactically valid unlisted CID"),
    }
}

fn posture(fixture: &Fixture) -> DeviceDoorAuthorization {
    DeviceDoorAuthorization::from(AuthorizationLedger::new(&fixture.root).read_state())
}

async fn request_bytes(
    app: axum::Router,
    path: &str,
    basis: Option<AccessBasis>,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::get(path)
        .body(Body::empty())
        .expect("request builds");
    if let Some(basis) = basis {
        request.extensions_mut().insert(basis);
    }
    let response = app.oneshot(request).await.expect("router responds");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response reads")
        .to_vec();
    (status, body)
}

async fn request(app: axum::Router, path: &str, basis: Option<AccessBasis>) -> (StatusCode, Value) {
    let (status, body) = request_bytes(app, path, basis).await;
    let json = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    (status, json)
}

fn revoked_body() -> Value {
    json!({
        "error": "that paired device couldn't be used because it was revoked.",
        "reason": "pl_revoked",
        "reason_code": "pl_revoked",
        "detail": "paired device revoked",
    })
}

fn authorization_path(fixture: &Fixture) -> PathBuf {
    fixture.root.join("link/authorized_clients.json")
}

fn remove_authorization_path(path: &Path) {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir(path).expect("authorization directory removes")
        }
        Ok(_) => fs::remove_file(path).expect("authorization file removes"),
        Err(_) => {}
    }
}

#[derive(Clone, Copy)]
enum DiskPosture {
    Present,
    Missing,
    Unreadable,
    Malformed,
}

fn induce_posture(fixture: &Fixture, posture: DiskPosture) {
    match posture {
        DiskPosture::Present => fixture.warm_authorization_to_present(),
        DiskPosture::Missing => remove_authorization_path(&authorization_path(fixture)),
        DiskPosture::Unreadable => {
            fixture.warm_authorization_to_present();
            fixture.induce_unreadable_authorization();
        }
        DiskPosture::Malformed => {
            fixture.warm_authorization_to_present();
            fixture.induce_malformed_authorization();
        }
    }
}

/// Keeps both FIFO ends open so a blocking `fs::read` cannot observe EOF.
struct BlockingAuthorizationFifo {
    path: PathBuf,
    drain: Option<File>,
}

impl BlockingAuthorizationFifo {
    fn replace(path: PathBuf) -> Self {
        remove_authorization_path(&path);
        mkfifo(&path, Mode::S_IRUSR | Mode::S_IWUSR).expect("authorization FIFO creates");
        let drain = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("authorization FIFO drain opens");
        Self {
            path,
            drain: Some(drain),
        }
    }
}

impl Drop for BlockingAuthorizationFifo {
    fn drop(&mut self) {
        self.drain.take();
        let _ = fs::remove_file(&self.path);
    }
}

fn door_port(outcome: &DoorOutcome) -> u16 {
    match outcome {
        DoorOutcome::Bound(address) => address.port(),
        other => panic!("door did not bind: {other:?}"),
    }
}

async fn live_carrier(
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

#[tokio::test]
async fn ac1_disk_revocation_wins_over_stale_present_channel_and_ac2_disk_present_wins() {
    let fixture = Fixture::established(1);
    let listed = linked_device(&fixture, 0);
    let (sender, receiver) = watch::channel(posture(&fixture));
    let app = authorized_router(fixture.root.clone(), receiver).into_inner();

    assert!(fixture.remove_authorization(0).authorized_removed);
    let (status, body) = request(app.clone(), "/api/system/status", Some(listed.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());

    fixture.warm_authorization_to_present();
    sender.send_replace(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Unreadable,
    ));
    let (status, _) = request(app, "/api/system/status", Some(listed)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn ac3_authorization_postures_refuse_except_for_a_listed_device() {
    let fixture = Fixture::established(1);
    let listed = linked_device(&fixture, 0);
    let (_, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let app = authorized_router(fixture.root.clone(), receiver).into_inner();

    for posture in [
        DiskPosture::Missing,
        DiskPosture::Unreadable,
        DiskPosture::Malformed,
    ] {
        induce_posture(&fixture, posture);
        let (status, body) = request(app.clone(), "/api/system/status", Some(listed.clone())).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, revoked_body());
    }

    induce_posture(&fixture, DiskPosture::Present);
    let (status, _) = request(app, "/api/system/status", Some(listed)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn ac4_localhost_passes_and_unlisted_linked_device_refuses() {
    let fixture = Fixture::established(1);
    let (_, receiver) = watch::channel(posture(&fixture));
    let app = authorized_router(fixture.root.clone(), receiver).into_inner();

    let (localhost_status, _) = request(
        app.clone(),
        "/api/system/status",
        Some(AccessBasis::Localhost),
    )
    .await;
    assert_eq!(localhost_status, StatusCode::OK);

    let (linked_status, body) =
        request(app, "/api/system/status", Some(unlisted_linked_device())).await;
    assert_eq!(linked_status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());
}

#[tokio::test]
async fn ac5_missing_access_basis_refuses_before_the_route() {
    let fixture = Fixture::established(1);
    let (_, receiver) = watch::channel(posture(&fixture));
    let (status, body) = request(
        authorized_router(fixture.root.clone(), receiver).into_inner(),
        "/api/system/status",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac6_gate_rejects_non_regular_ledger_without_blocking() {
    let fixture = Fixture::established(1);
    let listed = linked_device(&fixture, 0);
    let (_, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let app = authorized_router(fixture.root.clone(), receiver).into_inner();
    let path = authorization_path(&fixture);

    warn_capture::install_and_clear();
    log::warn!("authorization-gate warn capture control");
    assert!(warn_capture::contains("warn capture control"));

    warn_capture::clear();
    let (status, _) = request(app.clone(), "/api/system/status", Some(listed.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !warn_capture::contains("authorization read timed out"),
        "normal ledger read must not emit the timeout warning"
    );

    induce_posture(&fixture, DiskPosture::Missing);
    let (_, expected_body) =
        request_bytes(app.clone(), "/api/system/status", Some(listed.clone())).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&expected_body).expect("revoked baseline JSON"),
        revoked_body()
    );
    fixture.warm_authorization_to_present();

    warn_capture::clear();
    let started = Instant::now();
    let (status, body) = {
        let _fifo = BlockingAuthorizationFifo::replace(path.clone());
        request_bytes(app, "/api/system/status", Some(listed)).await
    };
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "non-regular ledger must be rejected before it can block"
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        body, expected_body,
        "non-regular ledger refusal body is byte-identical"
    );
    assert!(
        !warn_capture::contains("authorization read timed out"),
        "rejection happens before the timeout path"
    );
}

#[tokio::test]
async fn ac7_gate_reads_every_matched_request_without_posture_memoization() {
    let fixture = Fixture::established(1);
    let listed = linked_device(&fixture, 0);
    let (_, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let reads = AuthorizationGateReadProbe::new();
    let app = authorized_router_with_read_probe(fixture.root.clone(), receiver, reads.clone())
        .into_inner();

    for (posture, expected) in [
        (DiskPosture::Present, StatusCode::OK),
        (DiskPosture::Missing, StatusCode::FORBIDDEN),
        (DiskPosture::Unreadable, StatusCode::FORBIDDEN),
        (DiskPosture::Malformed, StatusCode::FORBIDDEN),
    ] {
        induce_posture(&fixture, posture);
        let before = reads.reads();
        for _ in 0..2 {
            let (status, _) =
                request(app.clone(), "/api/system/status", Some(listed.clone())).await;
            assert_eq!(status, expected);
        }
        assert_eq!(reads.reads() - before, 2);
    }
}

#[tokio::test]
async fn ac7_only_boot_assets_are_exempt() {
    let fixture = Fixture::established(1);
    induce_posture(&fixture, DiskPosture::Missing);
    let (_, receiver) = watch::channel(posture(&fixture));
    let app = authorized_router(fixture.root.clone(), receiver).into_inner();

    for path in ["/favicon.ico", "/static/shell.html"] {
        let (status, _) = request(app.clone(), path, Some(linked_device(&fixture, 0))).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
    let (status, body) = request(app, "/api/shell", Some(linked_device(&fixture, 0))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());
}

#[tokio::test]
async fn ac8_unmatched_path_keeps_the_shell_fallback() {
    let fixture = Fixture::established(1);
    induce_posture(&fixture, DiskPosture::Missing);
    let (_, receiver) = watch::channel(posture(&fixture));
    let (status, _) = request(
        authorized_router(fixture.root.clone(), receiver).into_inner(),
        "/no-such-route",
        Some(linked_device(&fixture, 0)),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ac9_refusal_body_has_the_reference_shape() {
    let fixture = Fixture::established(1);
    induce_posture(&fixture, DiskPosture::Malformed);
    let (_, receiver) = watch::channel(posture(&fixture));
    let (status, body) = request(
        authorized_router(fixture.root.clone(), receiver).into_inner(),
        "/api/system/status",
        Some(linked_device(&fixture, 0)),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());
    assert_eq!(body.as_object().expect("object").len(), 4);
}

#[tokio::test]
async fn ac12_every_composed_shell_route_is_gated() {
    let fixture = Fixture::established(1);
    induce_posture(&fixture, DiskPosture::Unreadable);
    let (_, receiver) = watch::channel(posture(&fixture));
    let (status, body) = request(
        authorized_router(fixture.root.clone(), receiver).into_inner(),
        "/app/speakers/api/state",
        Some(linked_device(&fixture, 0)),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());
}

#[tokio::test]
async fn ac14_authorization_refusal_precedes_a_corrupt_session_response() {
    let fixture = Fixture::established(1);
    fs::write(fixture.root.join("config/journal.json"), b"{bad").expect("corrupt config");
    induce_posture(&fixture, DiskPosture::Missing);
    let (_, receiver) = watch::channel(posture(&fixture));
    let (status, body) = request(
        authorized_router(fixture.root.clone(), receiver).into_inner(),
        "/api/system/status",
        Some(linked_device(&fixture, 0)),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());
}

#[tokio::test]
async fn ac17_route_layer_gates_a_405_without_converting_a_strict_slash_404() {
    let fixture = Fixture::established(1);
    induce_posture(&fixture, DiskPosture::Missing);
    let (_, receiver) = watch::channel(posture(&fixture));
    let app = authorized_router(fixture.root.clone(), receiver).into_inner();
    let mut method_mismatch = Request::post("/api/system/status")
        .body(Body::empty())
        .expect("request builds");
    method_mismatch
        .extensions_mut()
        .insert(linked_device(&fixture, 0));
    assert_eq!(
        app.clone()
            .oneshot(method_mismatch)
            .await
            .expect("router responds")
            .status(),
        StatusCode::FORBIDDEN,
    );

    let (status, _) = request(app, "/api/system/status/", Some(linked_device(&fixture, 0))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ac1_unreadable_refuses_on_an_open_carrier_then_ac2_revocation_closes_it() {
    let fixture = Fixture::established(1);
    let (authorization_sender, authorization_receiver) = watch::channel(posture(&fixture));
    // The gate reads the ledger per request, so a separately built router is an
    // equivalent in-process view of the same on-disk authorization state.
    let app =
        authorized_router(fixture.root.clone(), authorization_sender.subscribe()).into_inner();
    let door_router = authorized_router(fixture.root.clone(), authorization_receiver);
    let mut handle = bind_with_authorization(
        ConveyServeOptions {
            journal_root: fixture.root.clone(),
            loopback_port: 0,
            door_port: 0,
            handshake_timeout: Duration::from_secs(2),
            stream_stall_timeout: Duration::from_secs(2),
            router: router(fixture.root.clone()),
            carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
            handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
        },
        door_router,
        authorization_sender,
    )
    .await
    .expect("serve");
    let mut carrier = live_carrier(&fixture, door_port(handle.door_outcome())).await;
    let mut decoder = spl_core::frame::FrameDecoder::new();
    let mut dialer = spl_core::frame::FrameDialer::default();
    let path = "/api/system/status";

    let initial = get_over_carrier(&mut carrier, &mut decoder, &mut dialer, path).await;
    assert_eq!(initial.status, 200, "listed device opens the carrier");

    let authorization_path = authorization_path(&fixture);
    fixture.induce_unreadable_authorization();
    let refused = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let response = get_over_carrier(&mut carrier, &mut decoder, &mut dialer, path).await;
            if response.status == 403 {
                return response;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("unreadable posture reaches the shared authorization gate");
    assert_eq!(
        serde_json::from_slice::<Value>(&refused.body).expect("403 body JSON"),
        revoked_body()
    );

    tokio::time::sleep(Duration::from_millis(700)).await;
    let held = get_over_carrier(&mut carrier, &mut decoder, &mut dialer, path).await;
    assert_eq!(held.status, 403, "unreadable posture keeps carrier open");
    assert_eq!(
        serde_json::from_slice::<Value>(&held.body).expect("403 body JSON"),
        revoked_body()
    );

    fs::remove_dir(&authorization_path).expect("unreadable directory removes");
    fs::write(&authorization_path, b"[]").expect("explicit revocation writes");
    let closed = tokio::time::timeout(Duration::from_secs(3), async {
        let mut buffer = [0_u8; 1024];
        loop {
            match carrier.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "carrier did not close after definite authorization absence"
    );

    // AC12b: this is a fresh request after the publisher is stopped, so recovery
    // proves the gate itself has no remembered unreadable posture.
    handle.stop_authorization_refresh().await;
    fixture.warm_authorization_to_present();
    fixture.induce_unreadable_authorization();
    let (status, body) = request(
        app.clone(),
        "/api/system/status",
        Some(linked_device(&fixture, 0)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());

    fixture.warm_authorization_to_present();
    let recovered = tokio::time::timeout(
        Duration::from_secs(2),
        request(app, "/api/system/status", Some(linked_device(&fixture, 0))),
    )
    .await
    .expect("valid ledger recovers promptly");
    assert_eq!(recovered.0, StatusCode::OK);
    handle.shutdown();
}
