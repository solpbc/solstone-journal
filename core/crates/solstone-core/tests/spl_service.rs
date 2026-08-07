// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-level proof that the native SPL service drives and closes its relay
//! listener under the posture gate.

use std::{
    fs,
    path::PathBuf,
    process::{Child, Command},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{
    io::AsyncBufReadExt,
    net::{TcpListener, UnixListener},
    sync::oneshot,
    time::timeout,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};

struct TempJournal {
    path: PathBuf,
}

impl TempJournal {
    fn new() -> Result<Self, String> {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let ordinal = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-spl-service-process-{}-{ordinal}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(|_| "could not create test journal".to_owned())?;
        Ok(Self { path })
    }

    fn write(&self, relative: &str, contents: &str) -> Result<(), String> {
        let path = self.path.join(relative);
        let parent = path
            .parent()
            .ok_or_else(|| "test file had no parent".to_owned())?;
        fs::create_dir_all(parent).map_err(|_| "could not create test parent".to_owned())?;
        fs::write(path, contents).map_err(|_| "could not write test file".to_owned())
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[tokio::test]
async fn native_service_drives_its_relay_then_closes_it_when_posture_leaves_spl()
-> Result<(), String> {
    let relay = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "could not bind fake relay".to_owned())?;
    let address = relay
        .local_addr()
        .map_err(|_| "could not read fake relay address".to_owned())?;
    let (opened_send, opened_receive) = oneshot::channel();
    let (closed_send, closed_receive) = oneshot::channel();
    let relay_task = tokio::spawn(async move {
        let (stream, _) = relay
            .accept()
            .await
            .map_err(|_| "fake relay did not accept listen socket".to_owned())?;
        let mut socket = accept_async(stream)
            .await
            .map_err(|_| "fake relay WebSocket upgrade failed".to_owned())?;
        let _ = opened_send.send(());
        // The listener heartbeats: it Pings as soon as the socket is up and
        // gates its own Connected health on the matching Pong, so a relay that
        // swallows the Ping tears the transport down for the wrong reason.
        // Answer the heartbeat and keep reading until the transport ends.
        let closed = timeout(Duration::from_secs(7), async {
            loop {
                match socket.next().await {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => {
                        return Ok::<bool, String>(true);
                    }
                    Some(Ok(Message::Ping(nonce))) => socket
                        .send(Message::Pong(nonce))
                        .await
                        .map_err(|_| "fake relay could not answer the heartbeat".to_owned())?,
                    Some(Ok(_)) => return Ok(false),
                }
            }
        })
        .await
        .map_err(|_| "native service kept the relay socket open".to_owned())??;
        let _ = closed_send.send(closed);
        Ok::<(), String>(())
    });

    let journal = TempJournal::new()?;
    journal.write(
        "config/journal.json",
        &format!(r#"{{"link":{{"posture":"spl","relay_url":"http://{address}"}}}}"#),
    )?;
    journal.write(
        "link/state.json",
        r#"{"instance_id":"persisted-instance","home_label":"Home"}"#,
    )?;
    journal.write(
        "link/tokens/account.json",
        r#"{"service_token":"process-test-token"}"#,
    )?;
    let callosum_socket = journal.path.join("health").join("callosum.sock");
    let callosum_parent = callosum_socket
        .parent()
        .ok_or_else(|| "Callosum socket had no parent".to_owned())?;
    fs::create_dir_all(callosum_parent)
        .map_err(|_| "could not create Callosum socket directory".to_owned())?;
    let callosum_listener = UnixListener::bind(&callosum_socket)
        .map_err(|_| "could not bind fake Callosum socket".to_owned())?;
    let callosum_task = tokio::spawn(collect_callosum_tail(callosum_listener));

    let child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["spl", "service"])
        .env("SOLSTONE_JOURNAL", &journal.path)
        .spawn()
        .map_err(|_| "could not spawn native service command".to_owned())?;
    let mut child = ChildGuard { child };

    timeout(Duration::from_secs(3), opened_receive)
        .await
        .map_err(|_| "native service never opened its relay socket".to_owned())?
        .map_err(|_| "fake relay did not report opened socket".to_owned())?;
    if child
        .child_mut()
        .try_wait()
        .map_err(|_| "could not inspect native service status".to_owned())?
        .is_some()
    {
        return Err("native service exited after opening the relay socket".to_owned());
    }

    journal.write("config/journal.json", r#"{"link":{"posture":"direct"}}"#)?;
    let closed = timeout(Duration::from_secs(7), closed_receive)
        .await
        .map_err(|_| "fake relay did not observe native-service closure".to_owned())?
        .map_err(|_| "fake relay closure report dropped".to_owned())?;
    if !closed {
        return Err("native service ended relay transport without closing it".to_owned());
    }
    let events = timeout(Duration::from_secs(3), callosum_task)
        .await
        .map_err(|_| "Callosum did not receive the disconnect-health tail".to_owned())?
        .map_err(|_| "Callosum capture task failed".to_owned())??;
    if events
        .iter()
        .any(|event| event.to_string().contains("process-test-token"))
    {
        return Err("Callosum output leaked the service token".to_owned());
    }
    let tail = events
        .get(events.len().saturating_sub(2)..)
        .ok_or_else(|| "Callosum event list had no final tail".to_owned())?;
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0], json!({"tract": "link", "event": "disconnect"}));
    assert_eq!(tail[1]["tract"], "link");
    assert_eq!(tail[1]["event"], "health");
    assert_eq!(tail[1]["state"], "reconnecting");
    relay_task
        .await
        .map_err(|_| "fake relay task failed".to_owned())??;
    Ok(())
}

async fn collect_callosum_tail(listener: UnixListener) -> Result<Vec<Value>, String> {
    let (stream, _) = listener
        .accept()
        .await
        .map_err(|_| "native service did not connect to Callosum".to_owned())?;
    let mut lines = tokio::io::BufReader::new(stream).lines();
    let mut events = Vec::new();
    loop {
        let line = timeout(Duration::from_secs(7), lines.next_line())
            .await
            .map_err(|_| "native service did not emit a Callosum event".to_owned())?
            .map_err(|_| "Callosum stream could not be read".to_owned())?
            .ok_or_else(|| "Callosum stream closed before final tail".to_owned())?;
        let event: Value =
            serde_json::from_str(&line).map_err(|_| "Callosum output was not JSONL".to_owned())?;
        events.push(event);
        let tail = events.get(events.len().saturating_sub(2)..);
        if matches!(
            tail,
            Some([first, second])
                if first["tract"] == "link"
                    && first["event"] == "disconnect"
                    && second["tract"] == "link"
                    && second["event"] == "health"
        ) {
            return Ok(events);
        }
    }
}
