// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]
#![cfg(all(test, feature = "full-tests"))]

use super::*;
use crate::{CallosumOneShotSender, CallosumSocketConnection, UnixEndpoint};
use serde_json::{Map, json};
use solstone_core_journal_io::JournalRoot;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{Duration, sleep};

struct Fixture {
    directory: tempfile::TempDir,
    root: Arc<JournalRoot>,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(directory.path()).unwrap());
        let path = root.canonical_path().join("health/callosum.sock");
        Self {
            directory,
            root,
            path,
        }
    }

    async fn server(&self, hooks: Arc<ServerTestHooks>) -> CallosumSocketServer {
        CallosumSocketServer::bind_local_with_test_hooks(Arc::clone(&self.root), hooks)
            .await
            .unwrap()
    }
}

fn envelope(event: &str) -> CallosumEnvelope {
    serde_json::from_value(json!({"tract":"control","event":event,"ref":"original-ref"})).unwrap()
}

async fn clients(server: &CallosumSocketServer, expected: usize) {
    timeout(Duration::from_secs(2), async {
        while server.client_count() != expected {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

async fn confirmed(sender: CallosumOneShotSender, event: &'static str) {
    let line = format!("{}\n", serde_json::to_string(&envelope(event)).unwrap());
    tokio::task::spawn_blocking(move || sender.send_line_confirmed(&line))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn local_transport_preserves_real_echo_reference_and_successor_custody() {
    let fixture = Fixture::new();
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let server = CallosumSocketServer::bind_journal(Arc::clone(&fixture.root))
        .await
        .unwrap();
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    let server = fixture.server(Arc::new(ServerTestHooks::default())).await;
    assert!(!fixture.path.exists());
    assert_eq!(
        crate::local_unix_listener_status(&fixture.path).unwrap(),
        Some(true)
    );
    assert!(
        CallosumSocketServer::bind_local_with_test_hooks(
            Arc::clone(&fixture.root),
            Arc::new(ServerTestHooks::default())
        )
        .await
        .is_err()
    );
    let sender = CallosumOneShotSender::new(&fixture.path, Duration::from_secs(1));
    let mut subscriber = CallosumSocketConnection::new(&fixture.path, Map::new());
    subscriber.start();
    clients(&server, 1).await;
    confirmed(sender.clone(), "published").await;
    let received = timeout(Duration::from_secs(1), subscriber.next_message())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.event, "published");
    assert_eq!(received.extra["ref"], "original-ref");
    assert!(received.ts.is_some());
    subscriber.stop().await;
    server.stop().await;
    let successor = fixture.server(Arc::new(ServerTestHooks::default())).await;
    server.stop().await;
    drop(server);
    confirmed(sender, "successor").await;
    successor.stop().await;
    assert!(!fixture.path.exists());
}

#[tokio::test(flavor = "current_thread")]
async fn local_transport_joins_held_write_after_cancelled_stop_waiter() {
    let fixture = Fixture::new();
    let hooks = Arc::new(ServerTestHooks::default());
    let server = fixture.server(Arc::clone(&hooks)).await;
    let stream = UnixEndpoint::new(&fixture.path)
        .connect_local()
        .unwrap()
        .unwrap();
    clients(&server, 1).await;
    hooks.block_client(1);
    let started = hooks.wait_for_write();
    tokio::pin!(started);
    assert!(server.broadcast(envelope("held")));
    timeout(Duration::from_secs(1), &mut started).await.unwrap();
    assert!(
        timeout(Duration::from_millis(10), server.stop())
            .await
            .is_err()
    );
    assert_eq!(hooks.active_blocked_writes(), 1);
    server.stop().await;
    assert_eq!(hooks.active_blocked_writes(), 0);
    drop(stream);
}

async fn named_listener(path: &Path) -> UnixListener {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(path).unwrap();
    // Prove the planted fallback is reachable, then consume only this control.
    let control = UnixStream::connect(path).await.unwrap();
    let accepted = listener.accept().await.unwrap().0;
    drop((control, accepted));
    listener
}

async fn refuses_fallback(
    endpoint: &UnixEndpoint,
    sender: &CallosumOneShotSender,
    listener: &UnixListener,
) {
    assert!(endpoint.connect_local().is_err());
    assert!(
        sender
            .send_line("{\"tract\":\"control\",\"event\":\"refused\"}\n")
            .is_err()
    );
    let mut async_client = CallosumSocketConnection::new(endpoint.path(), Map::new());
    // Starting the task while the selected registration still exists captures
    // that exact address even if the owner disappears during retry.
    async_client.start();
    assert!(
        timeout(Duration::from_millis(40), listener.accept())
            .await
            .is_err()
    );
    async_client.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn local_transport_full_closed_and_stopped_refuse_working_named_fallback() {
    for refusal in ["full", "closed", "stopped"] {
        let fixture = Fixture::new();
        let hooks = Arc::new(ServerTestHooks::default());
        hooks.hold_local_accept();
        let server = fixture.server(hooks).await;
        let endpoint = UnixEndpoint::new(&fixture.path);
        let sender = CallosumOneShotSender::new(&fixture.path, Duration::from_millis(50));
        let mut queued = Vec::new();
        match refusal {
            "full" => {
                for _ in 0..SERVER_CLIENT_OUTBOUND_CAPACITY {
                    let mut stream = endpoint.connect_local().unwrap().unwrap();
                    stream.set_nonblocking(true).unwrap();
                    let mut byte = [0];
                    assert_eq!(
                        std::io::Read::read(&mut stream, &mut byte)
                            .unwrap_err()
                            .kind(),
                        std::io::ErrorKind::WouldBlock
                    );
                    queued.push(stream);
                }
            }
            "closed" => {
                let accept = lock(&server.inner.tasks).pop().unwrap();
                accept.abort();
                assert!(accept.await.unwrap_err().is_cancelled());
            }
            "stopped" => server.inner.stopped.store(true, Ordering::Release),
            _ => unreachable!(),
        }
        let listener = named_listener(&fixture.path).await;
        refuses_fallback(&endpoint, &sender, &listener).await;
        // Force Stop to perform its real join even for the stopped control.
        server.stop().await;
        for mut stream in queued {
            let mut byte = [0];
            assert_eq!(std::io::Read::read(&mut stream, &mut byte).unwrap(), 0);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn local_transport_original_root_refuses_registration_connection_and_dispatch() {
    let fixture = Fixture::new();
    let server = fixture.server(Arc::new(ServerTestHooks::default())).await;
    let endpoint = UnixEndpoint::new(&fixture.path);
    let sender = CallosumOneShotSender::new(&fixture.path, Duration::from_millis(50));
    let stream = endpoint.connect_local().unwrap().unwrap();
    stream.set_nonblocking(true).unwrap();
    let mut stream = UnixStream::from_std(stream).unwrap();
    clients(&server, 1).await;
    let original = fixture.directory.path().with_extension("original");
    fs::rename(fixture.directory.path(), &original).unwrap();
    fs::create_dir(fixture.directory.path()).unwrap();
    assert!(
        CallosumSocketServer::bind_local_with_test_hooks(
            Arc::clone(&fixture.root),
            Arc::new(ServerTestHooks::default())
        )
        .await
        .is_err()
    );
    assert!(
        fs::read_dir(fixture.directory.path())
            .unwrap()
            .next()
            .is_none()
    );
    let listener = named_listener(&fixture.path).await;
    refuses_fallback(&endpoint, &sender, &listener).await;
    stream
        .write_all(b"{\"tract\":\"control\",\"event\":\"late\"}\n")
        .await
        .unwrap();
    let mut byte = [0];
    assert_eq!(
        timeout(Duration::from_secs(1), stream.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert!(!server.broadcast(envelope("late")));
    server.stop().await;
    assert!(
        fixture.path.exists(),
        "old owner must not unlink the replacement listener"
    );
    fs::remove_dir_all(original).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn selected_local_address_stays_local_after_drop_while_desktop_named_still_works() {
    let fixture = Fixture::new();
    let server = fixture.server(Arc::new(ServerTestHooks::default())).await;
    let endpoint = UnixEndpoint::new(&fixture.path);
    let sender = CallosumOneShotSender::new(&fixture.path, Duration::from_millis(50));
    let mut async_client = CallosumSocketConnection::new(&fixture.path, Map::new());
    drop(server);
    let listener = named_listener(&fixture.path).await;
    async_client.start();
    assert!(endpoint.connect_local().is_err());
    assert!(
        sender
            .send_line("{\"tract\":\"control\",\"event\":\"refused\"}\n")
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(40), listener.accept())
            .await
            .is_err()
    );
    async_client.stop().await;
    drop(listener);
    fs::remove_file(&fixture.path).unwrap();
    let desktop = CallosumSocketServer::bind(&fixture.path).await.unwrap();
    confirmed(
        CallosumOneShotSender::new(&fixture.path, Duration::from_secs(1)),
        "desktop",
    )
    .await;
    desktop.stop().await;
}
