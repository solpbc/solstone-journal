// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_callosum::test_support::ServerTestHooks;
use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer, UnixEndpoint};
use solstone_core_journal_io::JournalRoot;
use std::sync::Arc;

#[tokio::test(flavor = "current_thread")]
async fn embedded_output_delivers_actual_terminal_and_health_in_order() {
    let journal = TempJournal::new().unwrap();
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let path = journal.path().join("health/callosum.sock");
    let server = CallosumSocketServer::bind_local_with_test_hooks(
        root,
        Arc::new(ServerTestHooks::default()),
    )
    .await
    .unwrap();
    let mut subscriber = CallosumSocketConnection::new(&path, serde_json::Map::new());
    subscriber.start();
    timeout(Duration::from_secs(1), async {
        while server.client_count() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let output = CallosumTestDriver::paused(path.clone());
    output.emit("tunnel_close", json!({"tunnel_id":"owned-tunnel"}));
    output.emit("health", json!({"state":"reconnecting"}));
    output.start();
    let terminal = timeout(Duration::from_secs(1), subscriber.next_message())
        .await
        .unwrap()
        .unwrap();
    let health = timeout(Duration::from_secs(1), subscriber.next_message())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (terminal.tract.as_str(), terminal.event.as_str()),
        ("link", "tunnel_close")
    );
    assert_eq!(terminal.extra["tunnel_id"], "owned-tunnel");
    assert_eq!(health.event, "health");
    assert_eq!(health.extra["state"], "reconnecting");
    timeout(Duration::from_secs(1), output.stop())
        .await
        .unwrap();
    subscriber.stop().await;
    server.stop().await;
    assert!(!path.exists());
}

#[tokio::test(flavor = "current_thread")]
async fn embedded_output_refuses_full_stopped_dropped_and_replaced_local_owner_without_named_fallback()
 {
    for refusal in ["full", "closed", "stopped", "dropped", "replaced"] {
        let journal = TempJournal::new().unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let path = journal.path().join("health/callosum.sock");
        let hooks = Arc::new(ServerTestHooks::default());
        if refusal == "full" {
            hooks.hold_local_accept();
        }
        if refusal == "closed" {
            hooks.close_local_accept();
        }
        let mut server = Some(
            CallosumSocketServer::bind_local_with_test_hooks(root, hooks)
                .await
                .unwrap(),
        );
        let output = CallosumTestDriver::paused(path.clone());
        output.emit("disconnect", json!({}));
        let endpoint = UnixEndpoint::new(&path);
        let mut queued = Vec::new();
        let mut original = None;
        match refusal {
            "full" => {
                while let Ok(Some(stream)) = endpoint.connect_local() {
                    queued.push(stream);
                    assert!(queued.len() < 1000, "real local queue must be bounded");
                }
                assert!(!queued.is_empty());
            }
            "stopped" => server.as_ref().unwrap().stop().await,
            "closed" => {
                timeout(Duration::from_secs(1), async {
                    while solstone_core_callosum::local_unix_listener_status(&path).unwrap()
                        == Some(true)
                    {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            }
            "dropped" => drop(server.take()),
            "replaced" => {
                let saved = journal.path().with_extension("original");
                fs::rename(journal.path(), &saved).unwrap();
                fs::create_dir(journal.path()).unwrap();
                original = Some(saved);
            }
            _ => unreachable!(),
        }
        fs::create_dir(path.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&path).unwrap();
        let control = tokio::net::UnixStream::connect(&path).await.unwrap();
        let accepted = listener.accept().await.unwrap().0;
        drop((control, accepted));
        output.start();
        timeout(Duration::from_secs(1), output.stop())
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(40), listener.accept())
                .await
                .is_err(),
            "{refusal} must not route SPL output into the working named listener"
        );
        if let Some(server) = server {
            server.stop().await;
        }
        drop(queued);
        if let Some(original) = original {
            fs::remove_dir_all(original).unwrap();
        }
    }
}
