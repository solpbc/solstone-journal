// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod door_support;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceDid};
use solstone_core_convey_shell::authorization_gate::authorized_router;
use solstone_core_convey_shell::{
    ConveyServeOptions, DoorOutcome, bind_with_authorization, router,
};
use solstone_core_sol_link::DeviceDoorAuthorization;
use solstone_core_sol_link::ledger::{AuthorizationLedger, AuthorizedClientsRead};
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tower::ServiceExt;

use door_support::Fixture;

fn linked_device(fixture: &Fixture, index: usize) -> AccessBasis {
    let did = format!(
        "sha256:{}",
        spl_core::ca::sha256_hex(fixture.client_der(index))
    );
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        did: LinkedDeviceDid::try_from(did.as_str()).expect("fixture DID"),
    }
}

fn unlisted_linked_device() -> AccessBasis {
    AccessBasis::LinkedDevice {
        carrier: Carrier::Direct,
        did: LinkedDeviceDid::try_from(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect("syntactically valid unlisted DID"),
    }
}

fn posture(fixture: &Fixture) -> DeviceDoorAuthorization {
    DeviceDoorAuthorization::from(AuthorizationLedger::new(&fixture.root).read_state())
}

async fn request(app: axum::Router, path: &str, basis: Option<AccessBasis>) -> (StatusCode, Value) {
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
        .expect("response reads");
    let json = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    (status, json)
}

fn revoked_body() -> Value {
    json!({
        "error": "I couldn't use that paired device because it was revoked.",
        "reason": "pl_revoked",
        "reason_code": "pl_revoked",
        "detail": "paired device revoked",
    })
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

async fn get_over_carrier(
    carrier: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    decoder: &mut spl_core::frame::FrameDecoder,
    dialer: &mut spl_core::frame::FrameDialer,
    path: &str,
) -> spl_core::http::HttpResponse {
    use spl_core::frame::{FLAG_CLOSE, FLAG_DATA, FLAG_OPEN, Frame};
    use spl_core::mux::ResponseAssembler;

    let stream_id = dialer.allocate();
    let request = format!("GET {path} HTTP/1.1\r\nhost: spl.local\r\ncontent-length: 0\r\n\r\n");
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
                .expect("close frame"),
        )
        .await
        .expect("request closes");
    carrier.flush().await.expect("request flushes");

    let mut response = ResponseAssembler::new(stream_id);
    let mut bytes = [0_u8; 64 * 1024];
    loop {
        let read = carrier.read(&mut bytes).await.expect("response reads");
        assert!(read > 0, "carrier closed before the response completed");
        decoder.feed(&bytes[..read]);
        for frame in decoder.drain().expect("response frames") {
            if frame.stream_id != stream_id {
                continue;
            }
            let output = response
                .feed(&frame.encode().expect("routed frame"))
                .expect("response frame");
            for frame in output.pongs.into_iter().chain(output.emit_frames) {
                carrier
                    .write_all(&frame)
                    .await
                    .expect("control frame writes");
            }
        }
        carrier.flush().await.expect("control flushes");
        if response.is_closed() {
            return response.into_response().expect("complete response");
        }
    }
}

#[tokio::test]
async fn ac3_authorization_postures_refuse_except_for_a_listed_device() {
    let fixture = Fixture::established(1);
    let listed = linked_device(&fixture, 0);
    let cases = [
        AuthorizedClientsRead::Missing,
        AuthorizedClientsRead::Unreadable,
        AuthorizedClientsRead::Malformed,
        AuthorizedClientsRead::Present(Vec::new()),
    ];

    for state in cases {
        let (_, receiver) = watch::channel(DeviceDoorAuthorization::from(state));
        let (status, body) = request(
            authorized_router(router(fixture.root.clone()), receiver),
            "/api/system/status",
            Some(listed.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, revoked_body());
    }

    let (_, receiver) = watch::channel(posture(&fixture));
    let (status, _) = request(
        authorized_router(router(fixture.root.clone()), receiver),
        "/api/system/status",
        Some(listed),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn ac4_localhost_passes_and_unlisted_linked_device_refuses() {
    let fixture = Fixture::established(1);
    let (_, receiver) = watch::channel(posture(&fixture));
    let app = authorized_router(router(fixture.root.clone()), receiver);

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
        authorized_router(router(fixture.root.clone()), receiver),
        "/api/system/status",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());
}

#[tokio::test]
async fn ac7_only_boot_assets_are_exempt() {
    let fixture = Fixture::established(1);
    let (_, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let app = authorized_router(router(fixture.root.clone()), receiver);

    for path in ["/favicon.ico", "/static/shell.html"] {
        let (status, _) = request(app.clone(), path, Some(linked_device(&fixture, 0))).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
    let (status, body) = request(app, "/api/shell", Some(linked_device(&fixture, 0))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());

    let (_, receiver) = watch::channel(posture(&fixture));
    let app = authorized_router(router(fixture.root.clone()), receiver);
    for path in ["/favicon.ico", "/static/shell.html"] {
        let (status, _) = request(app.clone(), path, Some(linked_device(&fixture, 0))).await;
        assert_eq!(status, StatusCode::OK, "authorized device: {path}");
    }
}

#[tokio::test]
async fn ac8_unmatched_path_keeps_the_shell_fallback() {
    let fixture = Fixture::established(1);
    let (_, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let (status, _) = request(
        authorized_router(router(fixture.root.clone()), receiver),
        "/no-such-route",
        Some(linked_device(&fixture, 0)),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ac17_route_layer_gates_a_405_without_converting_a_strict_slash_404() {
    let fixture = Fixture::established(1);
    let (_, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let app = authorized_router(router(fixture.root.clone()), receiver);
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
async fn ac9_refusal_body_has_the_reference_shape() {
    let fixture = Fixture::established(1);
    let (_, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let (status, body) = request(
        authorized_router(router(fixture.root.clone()), receiver),
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
    // router() uses one chained route builder, without a merge/nest boundary;
    // this proves the layer reaches a route registered deep in that chain.
    let fixture = Fixture::established(1);
    let (_, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let (status, body) = request(
        authorized_router(router(fixture.root.clone()), receiver),
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
    std::fs::write(fixture.root.join("config/journal.json"), b"{bad").expect("corrupt config");
    let (_, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let (status, body) = request(
        authorized_router(router(fixture.root.clone()), receiver),
        "/api/system/status",
        Some(linked_device(&fixture, 0)),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());
}

#[tokio::test]
async fn ac1_unreadable_refuses_on_an_open_carrier_then_ac2_revocation_closes_it() {
    let fixture = Fixture::established(1);
    let (authorization_sender, authorization_receiver) = watch::channel(posture(&fixture));
    let door_router = authorized_router(router(fixture.root.clone()), authorization_receiver);
    let handle = bind_with_authorization(
        ConveyServeOptions {
            journal_root: fixture.root.clone(),
            loopback_port: 0,
            door_port: 0,
            handshake_timeout: Duration::from_secs(2),
            stream_stall_timeout: Duration::from_secs(2),
            router: router(fixture.root.clone()),
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

    let authorization_path = fixture.root.join("link/authorized_clients.json");
    std::fs::remove_file(&authorization_path).expect("authorization file removes");
    std::fs::create_dir(&authorization_path).expect("authorization path becomes unreadable");
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

    std::fs::remove_dir(&authorization_path).expect("unreadable directory removes");
    std::fs::write(&authorization_path, b"[]").expect("explicit revocation writes");
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
    handle.shutdown();
}

#[tokio::test]
async fn posture_change_applies_to_the_next_request() {
    let fixture = Fixture::established(1);
    let (sender, receiver) = watch::channel(posture(&fixture));
    let app = authorized_router(router(fixture.root.clone()), receiver);

    let (status, _) = request(
        app.clone(),
        "/api/system/status",
        Some(linked_device(&fixture, 0)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    sender.send_replace(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Unreadable,
    ));
    let (status, body) = request(app, "/api/system/status", Some(linked_device(&fixture, 0))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, revoked_body());
}
