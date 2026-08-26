// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use solstone_core_callosum::CallosumEnvelope;
use solstone_core_sol_link::DeviceDoorAuthorization;
use solstone_core_sol_link::ledger::AuthorizedClientsRead;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::watch;

use solstone_core_convey_shell::{
    ConveyServeOptions, authorization_gate, bind_with_authorization, router,
};

const IO_TIMEOUT: Duration = Duration::from_secs(2);

fn health_envelope(
    timestamp: i64,
    state: &str,
    generation: u64,
    error: Option<&str>,
    error_at: Option<u64>,
    success_at: Option<u64>,
) -> CallosumEnvelope {
    let mut extra = Map::new();
    extra.insert("state".to_owned(), json!(state));
    extra.insert("listen_generation".to_owned(), json!(generation));
    extra.insert(
        "last_successful_relay_tunnel_at".to_owned(),
        json!(success_at),
    );
    extra.insert("last_relay_tunnel_error".to_owned(), json!(error));
    extra.insert("last_relay_tunnel_error_at".to_owned(), json!(error_at));
    extra.insert("relay_tunnel_error_status".to_owned(), json!(591));
    extra.insert("relay_admission_saturated_count".to_owned(), json!(592));
    extra.insert("last_relay_listener_ack_at".to_owned(), json!(593));
    extra.insert("last_relay_listener_ack_generation".to_owned(), json!(594));
    CallosumEnvelope {
        tract: "link".to_owned(),
        event: solstone_core_spl::LINK_HEALTH_EVENT.to_owned(),
        ts: Some(timestamp),
        extra,
    }
}

async fn loopback_get(address: std::net::SocketAddr) -> Value {
    tokio::time::timeout(IO_TIMEOUT, async move {
        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("loopback connects");
        stream
            .write_all(
                b"GET /app/network/api/status HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n",
            )
            .await
            .expect("status request writes");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("status reads");
        let header_end = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("status headers")
            + 4;
        serde_json::from_slice(&response[header_end..]).expect("status JSON")
    })
    .await
    .expect("status request reaches its bounded result")
}

async fn accept_subscriber(listener: &UnixListener) -> tokio::net::UnixStream {
    tokio::time::timeout(IO_TIMEOUT, listener.accept())
        .await
        .expect("subscriber connects before the deadline")
        .expect("subscriber connects")
        .0
}

async fn accept_subscribers(listener: &UnixListener, count: usize) -> Vec<tokio::net::UnixStream> {
    let mut peers = Vec::with_capacity(count);
    for _ in 0..count {
        peers.push(accept_subscriber(listener).await);
    }
    peers
}

async fn send_health(peers: &mut [tokio::net::UnixStream], envelope: &CallosumEnvelope) {
    let mut line = serde_json::to_vec(envelope).expect("envelope serializes");
    line.push(b'\n');
    for peer in peers {
        peer.write_all(&line).await.expect("health writes");
    }
}

async fn status_for_generation(address: std::net::SocketAddr, generation: u64) -> Value {
    tokio::time::timeout(IO_TIMEOUT, async move {
        loop {
            let status = loopback_get(address).await;
            if status["relay_listen_generation"] == generation {
                return status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("relay status observes the applied generation before the deadline")
}

fn current_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows the Unix epoch")
        .as_millis()
        .try_into()
        .expect("current epoch milliseconds fit i64")
}

#[tokio::test]
async fn relay_health_subscriber_starts_late_and_drives_live_status() {
    let root = tempfile::TempDir::new_in("/var/tmp").expect("journal");
    fs::create_dir_all(root.path().join("config")).expect("config directory");
    fs::create_dir_all(root.path().join("link/tokens")).expect("token directory");
    fs::create_dir_all(root.path().join("health")).expect("health directory");
    fs::write(
        root.path().join("config/journal.json"),
        br#"{"setup":{"completed_at":1},"link":{"posture":"spl"},"pairing":{"home_address":"203.0.113.77:7657"}}"#,
    )
    .expect("config writes");
    fs::write(
        root.path().join("link/tokens/account.json"),
        br#"{"service_token":"test-token"}"#,
    )
    .expect("token writes");

    let sock = root.path().join("health/callosum.sock");
    let listener = UnixListener::bind(&sock).expect("unix listener binds");
    let _router_without_runtime = router(root.path().to_path_buf());
    let std_listener: StdUnixListener = listener.into_std().expect("std listener");
    std_listener
        .set_nonblocking(true)
        .expect("nonblocking accept");
    assert!(
        matches!(
            std_listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ),
        "router does not start a subscriber"
    );
    let listener = UnixListener::from_std(std_listener).expect("tokio listener");

    let (authorization_sender, _) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let handle = bind_with_authorization(
        ConveyServeOptions {
            journal_root: root.path().to_path_buf(),
            loopback_port: 0,
            door_port: 0,
            handshake_timeout: std::time::Duration::from_secs(1),
            stream_stall_timeout: std::time::Duration::from_secs(1),
            router: router(root.path().to_path_buf()),
            carrier_loop_iterations: std::sync::Arc::new(AtomicU64::new(0)),
            handshake_authorization_read_ticks: std::sync::Arc::new(AtomicU64::new(0)),
        },
        authorization_gate::DoorRouter::unconfined(router(root.path().to_path_buf())),
        authorization_sender,
    )
    .await
    .expect("Convey binds");

    let mut peers = accept_subscribers(&listener, 1).await;
    let now = current_epoch_millis();
    send_health(
        &mut peers,
        &health_envelope(
            now,
            "connected",
            590,
            Some("unrelated_error"),
            Some(596),
            Some(595),
        ),
    )
    .await;

    let status = status_for_generation(handle.loopback_ipv4_addr(), 590).await;
    assert_eq!(status["last_link_event_at"], now);
    assert_eq!(status["relay_listen_generation"], 590);
    assert_eq!(status["last_successful_relay_tunnel_at"], 595);
    assert_eq!(status["last_relay_tunnel_error"], "unrelated_error");
    assert_eq!(status["last_relay_tunnel_error_at"], 596);
    assert_eq!(status["last_relay_listener_ack_at"], 593);
    assert_eq!(status["last_relay_listener_ack_generation"], 594);
    assert!(
        status["home_candidates"]
            .as_array()
            .expect("home candidates array")
            .iter()
            .any(|candidate| candidate["source"] == "override")
    );
    assert_eq!(status["relay_state"], "parked");

    send_health(
        &mut peers,
        &health_envelope(now - 90_001, "connected", 591, None, None, Some(590)),
    )
    .await;
    let offline = status_for_generation(handle.loopback_ipv4_addr(), 591).await;
    assert_eq!(offline["relay_state"], "offline");

    handle.shutdown();
}
