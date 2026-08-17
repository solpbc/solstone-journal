// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Map;
use solstone_core_callosum::{CallosumEnvelope, CallosumReceiveEvent, CallosumSocketServer};
use solstone_core_top::{
    ProductionCallosum, ProductionReceive, ProductionTerminal, TopInput, TopReceiveTransport,
    TopTerminal,
};

static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);

#[tokio::test]
async fn production_receive_is_driven_while_the_sync_consumer_polls() {
    let ordinal = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("solstone-top-production-{ordinal}"));
    std::fs::create_dir_all(&root).unwrap();
    let socket = root.join("callosum.sock");
    let server = CallosumSocketServer::bind(&socket).await.unwrap();
    let shared = ProductionCallosum::new(&socket).unwrap();
    let mut receive = ProductionReceive::new(shared);
    receive.start().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.client_count() != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(server.broadcast(CallosumEnvelope {
        tract: "supervisor".to_owned(),
        event: "status".to_owned(),
        ts: None,
        extra: Map::new(),
    }));
    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(event @ CallosumReceiveEvent::Envelope { .. }) = receive.next().unwrap() {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(event, CallosumReceiveEvent::Envelope { .. }));
    std::thread::spawn(move || receive.stop().unwrap())
        .join()
        .unwrap();
    server.stop().await;
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn terminal_input_decodes_complete_escape_sequences_from_the_key_channel() {
    let (sender, keys) = mpsc::channel();
    let mut terminal = ProductionTerminal::from_key_source(keys);
    sender.send(b'\x1b').unwrap();
    sender.send(b'[').unwrap();
    sender.send(b'A').unwrap();
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::Up);
    sender.send(b'\x1b').unwrap();
    sender.send(b'[').unwrap();
    sender.send(b'B').unwrap();
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::Down);
    sender.send(b'\x1b').unwrap();
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::None);
}
