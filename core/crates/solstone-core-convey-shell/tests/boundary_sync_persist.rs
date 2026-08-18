// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use solstone_core_callosum::CallosumEnvelope;
use solstone_core_observer::store::record::ObserverRecord;
use solstone_core_observer::store::write::save_observer;
use solstone_core_sol_link::DeviceDoorAuthorization;
use solstone_core_sol_link::ledger::AuthorizedClientsRead;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::watch;

use solstone_core_convey_shell::{
    ConveyServeOptions, authorization_gate, bind_with_authorization, router,
};

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const DAY: &str = "20260818";
const PREFIX: &str = "deskdesk";

fn current_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows the Unix epoch")
        .as_millis()
        .try_into()
        .expect("current epoch milliseconds fit i64")
}

async fn accept_subscriber(listener: &UnixListener) -> tokio::net::UnixStream {
    tokio::time::timeout(IO_TIMEOUT, listener.accept())
        .await
        .expect("subscriber connects before the deadline")
        .expect("subscriber connects")
        .0
}

async fn send_envelope(peers: &mut [tokio::net::UnixStream], envelope: &CallosumEnvelope) {
    let mut line = serde_json::to_vec(envelope).expect("envelope serializes");
    line.push(b'\n');
    for peer in peers {
        peer.write_all(&line).await.expect("envelope writes");
    }
}

fn observe_envelope(event: &str, segment: &str) -> CallosumEnvelope {
    let mut extra = Map::new();
    extra.insert("observer".to_owned(), json!("desk"));
    extra.insert("day".to_owned(), json!(DAY));
    extra.insert("segment".to_owned(), json!(segment));
    CallosumEnvelope {
        tract: "observe".to_owned(),
        event: event.to_owned(),
        ts: None,
        extra,
    }
}

#[tokio::test]
async fn sync_persist_subscriber_writes_observed_and_transferred_history() {
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
    let record = ObserverRecord::from_value(json!({
        "key": format!("{PREFIX}x"),
        "name": "desk",
        "stats": {}
    }))
    .expect("record");
    save_observer(root.path(), &record).expect("seed device");

    let sock = root.path().join("health/callosum.sock");
    let listener = UnixListener::bind(&sock).expect("unix listener binds");
    let std_listener: StdUnixListener = listener.into_std().expect("std listener");
    std_listener
        .set_nonblocking(true)
        .expect("nonblocking accept");
    let listener = UnixListener::from_std(std_listener).expect("tokio listener");

    let (authorization_sender, _) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let handle = bind_with_authorization(
        ConveyServeOptions {
            journal_root: root.path().to_path_buf(),
            loopback_port: 0,
            door_port: 0,
            handshake_timeout: Duration::from_secs(1),
            stream_stall_timeout: Duration::from_secs(1),
            router: router(root.path().to_path_buf()),
            carrier_loop_iterations: std::sync::Arc::new(AtomicU64::new(0)),
            handshake_authorization_read_ticks: std::sync::Arc::new(AtomicU64::new(0)),
        },
        authorization_gate::DoorRouter::unconfined(router(root.path().to_path_buf())),
        authorization_sender,
    )
    .await
    .expect("Convey binds");

    let mut peers = vec![
        accept_subscriber(&listener).await,
        accept_subscriber(&listener).await,
    ];
    send_envelope(&mut peers, &observe_envelope("observed", "120000_1")).await;
    send_envelope(&mut peers, &observe_envelope("transferred", "120100_1")).await;

    let hist = root
        .path()
        .join("apps/observer/observers")
        .join(PREFIX)
        .join("hist")
        .join(format!("{DAY}.jsonl"));
    let started = current_epoch_millis();
    let rows = tokio::time::timeout(IO_TIMEOUT, async {
        loop {
            if let Ok(contents) = fs::read_to_string(&hist) {
                let parsed: Vec<Value> = contents
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(|line| serde_json::from_str(line).expect("hist row"))
                    .collect();
                if parsed.len() == 2 {
                    return parsed;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("hist rows appear before the deadline");

    let types: Vec<&str> = rows
        .iter()
        .map(|row| row["type"].as_str().expect("type"))
        .collect();
    assert_eq!(types, ["observed", "transferred"]);
    for row in &rows {
        let ts = row["ts"].as_i64().expect("ts");
        assert!(
            ts > 1_700_000_000_000 && ts >= started - 5_000,
            "ts {ts} must be a live wall-clock millisecond"
        );
    }

    let registry: Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join("apps/observer/observers")
                .join(format!("{PREFIX}.json")),
        )
        .expect("registry"),
    )
    .expect("registry JSON");
    assert_eq!(registry["stats"]["segments_observed"], 1);
    assert_eq!(registry["stats"]["segments_transferred"], 1);

    for peer in &mut peers {
        let mut unexpected = Vec::new();
        let _ = tokio::time::timeout(Duration::from_millis(50), peer.read_to_end(&mut unexpected))
            .await;
        let text = String::from_utf8_lossy(&unexpected);
        assert!(
            !text.contains("indexer") && !text.contains("supervisor/request"),
            "subscriber must not emit indexer or supervisor traffic: {text}"
        );
    }

    handle.shutdown();
}
