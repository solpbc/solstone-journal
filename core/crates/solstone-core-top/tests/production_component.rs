// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
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

fn key(code: KeyCode, kind: KeyEventKind, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new_with_kind(code, modifiers, kind))
}

fn feed(events: impl Into<VecDeque<Event>>) -> ProductionTerminal {
    ProductionTerminal::from_events(events.into())
}

#[test]
fn terminal_input_maps_crossterm_events_from_the_scripted_source() {
    let mut terminal = feed([
        key(KeyCode::Up, KeyEventKind::Press, KeyModifiers::NONE),
        key(KeyCode::Up, KeyEventKind::Repeat, KeyModifiers::NONE),
        key(KeyCode::Down, KeyEventKind::Press, KeyModifiers::NONE),
        key(KeyCode::Char('q'), KeyEventKind::Press, KeyModifiers::NONE),
        key(KeyCode::Char('q'), KeyEventKind::Repeat, KeyModifiers::NONE),
        key(
            KeyCode::Char('c'),
            KeyEventKind::Press,
            KeyModifiers::CONTROL,
        ),
        key(
            KeyCode::Char('d'),
            KeyEventKind::Press,
            KeyModifiers::CONTROL,
        ),
        Event::Resize(120, 40),
        Event::FocusGained,
        Event::FocusLost,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }),
        key(KeyCode::Up, KeyEventKind::Release, KeyModifiers::NONE),
    ]);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::Up);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::Up);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::Down);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::Quit);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::None);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::Interrupt);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::EndOfFile);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::None);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::None);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::None);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::None);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::None);
    assert_eq!(terminal.input(0.0).unwrap(), TopInput::None);
}
