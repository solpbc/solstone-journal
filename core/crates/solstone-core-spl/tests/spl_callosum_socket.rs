// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real-boundary Callosum JSONL delivery tests that bind a Unix socket.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde_json::json;
use solstone_core_spl::CallosumEmit;
use solstone_core_spl::test_hooks::CallosumTestDriver;
use tokio::{io::AsyncBufReadExt, net::UnixListener, time::timeout};

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

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[tokio::test]
async fn saturated_regular_output_still_delivers_the_final_lifecycle_tail() -> Result<(), String> {
    let journal = TempJournal::new()?;
    let socket_path = journal.path().join("health").join("callosum.sock");
    let socket_parent = socket_path
        .parent()
        .ok_or_else(|| "Callosum test socket had no parent".to_owned())?;
    fs::create_dir_all(socket_parent)
        .map_err(|_| "could not create Callosum test directory".to_owned())?;
    let listener = UnixListener::bind(&socket_path)
        .map_err(|_| "could not bind Callosum test socket".to_owned())?;
    let output = CallosumTestDriver::paused(socket_path);
    let expected_frame_count = output.saturate_regular_output();
    if !output.regular_output_saturated() {
        return Err("regular Callosum output queue did not saturate".to_owned());
    }
    output.emit("disconnect", json!({}));
    output.emit("health", json!({"state": "reconnecting"}));

    let reader = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|_| "writer did not connect to Callosum test socket".to_owned())?;
        let mut lines = tokio::io::BufReader::new(stream).lines();
        let first = lines
            .next_line()
            .await
            .map_err(|_| "could not read first lifecycle line".to_owned())?
            .ok_or_else(|| "Callosum closed before first lifecycle line".to_owned())?;
        let second = lines
            .next_line()
            .await
            .map_err(|_| "could not read second lifecycle line".to_owned())?
            .ok_or_else(|| "Callosum closed before second lifecycle line".to_owned())?;
        let mut received = vec![first, second];
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|_| "could not read lifecycle output through EOF".to_owned())?
        {
            received.push(line);
        }
        Ok::<Vec<String>, String>(received)
    });
    output.start();

    timeout(Duration::from_secs(3), output.stop())
        .await
        .map_err(|_| "Callosum output did not stop".to_owned())?;
    let lines = timeout(Duration::from_secs(3), reader)
        .await
        .map_err(|_| "lifecycle tail was not delivered under saturation".to_owned())?
        .map_err(|_| "Callosum saturation reader task failed".to_owned())??;
    assert_eq!(lines[0], "{\"tract\":\"link\",\"event\":\"disconnect\"}");
    assert_eq!(
        lines[1],
        "{\"tract\":\"link\",\"event\":\"health\",\"state\":\"reconnecting\"}"
    );
    if lines.len() != expected_frame_count {
        return Err(format!("Callosum frame count differed: {}", lines.len()));
    }
    let expected_regular = json!({
        "tract": "link",
        "event": "health",
        "state": "connected",
        "padding": "x".repeat(4096),
    });
    if lines[2..].iter().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .map_or(true, |frame| frame != expected_regular)
    }) {
        return Err(
            "Callosum emitted an unexpected regular frame after the lifecycle tail".to_owned(),
        );
    }
    if lines.iter().any(|line| line.contains("process-test-token")) {
        return Err("lifecycle tail leaked a service token".to_owned());
    }
    Ok(())
}

#[tokio::test]
async fn saturated_regular_output_preserves_each_tunnel_close_id_and_health() -> Result<(), String>
{
    let journal = TempJournal::new()?;
    let socket_path = journal.path().join("health").join("callosum.sock");
    let socket_parent = socket_path
        .parent()
        .ok_or_else(|| "Callosum test socket had no parent".to_owned())?;
    fs::create_dir_all(socket_parent)
        .map_err(|_| "could not create Callosum test directory".to_owned())?;
    let listener = UnixListener::bind(&socket_path)
        .map_err(|_| "could not bind Callosum test socket".to_owned())?;
    let output = CallosumTestDriver::paused(socket_path);
    let expected_frame_count = output.saturate_regular_output();
    if !output.regular_output_saturated() {
        return Err("regular Callosum output queue did not saturate".to_owned());
    }
    output.emit("tunnel_close", json!({"tunnel_id": "terminal-tunnel-7"}));
    output.emit("health", json!({"state": "connected"}));
    output.emit("tunnel_close", json!({"tunnel_id": "terminal-tunnel-8"}));
    output.emit("health", json!({"state": "reconnecting"}));

    let reader = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|_| "writer did not connect to Callosum test socket".to_owned())?;
        let mut lines = tokio::io::BufReader::new(stream).lines();
        let first = lines
            .next_line()
            .await
            .map_err(|_| "could not read first lifecycle line".to_owned())?
            .ok_or_else(|| "Callosum closed before first lifecycle line".to_owned())?;
        let second = lines
            .next_line()
            .await
            .map_err(|_| "could not read second lifecycle line".to_owned())?
            .ok_or_else(|| "Callosum closed before second lifecycle line".to_owned())?;
        let third = lines
            .next_line()
            .await
            .map_err(|_| "could not read third lifecycle line".to_owned())?
            .ok_or_else(|| "Callosum closed before third lifecycle line".to_owned())?;
        let fourth = lines
            .next_line()
            .await
            .map_err(|_| "could not read fourth lifecycle line".to_owned())?
            .ok_or_else(|| "Callosum closed before fourth lifecycle line".to_owned())?;
        let mut received = vec![first, second, third, fourth];
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|_| "could not read tunnel-close output through EOF".to_owned())?
        {
            received.push(line);
        }
        Ok::<Vec<String>, String>(received)
    });
    output.start();

    timeout(Duration::from_secs(3), output.stop())
        .await
        .map_err(|_| "Callosum output did not stop".to_owned())?;
    let lines = timeout(Duration::from_secs(3), reader)
        .await
        .map_err(|_| "tunnel-close tail was not delivered under saturation".to_owned())?
        .map_err(|_| "Callosum tunnel-close reader task failed".to_owned())??;
    assert_eq!(
        lines[0],
        "{\"tract\":\"link\",\"event\":\"tunnel_close\",\"tunnel_id\":\"terminal-tunnel-7\"}"
    );
    assert_eq!(
        lines[1],
        "{\"tract\":\"link\",\"event\":\"health\",\"state\":\"connected\"}"
    );
    assert_eq!(
        lines[2],
        "{\"tract\":\"link\",\"event\":\"tunnel_close\",\"tunnel_id\":\"terminal-tunnel-8\"}"
    );
    assert_eq!(
        lines[3],
        "{\"tract\":\"link\",\"event\":\"health\",\"state\":\"reconnecting\"}"
    );
    if lines.len() != expected_frame_count {
        return Err(format!("Callosum frame count differed: {}", lines.len()));
    }
    let expected_regular = json!({
        "tract": "link",
        "event": "health",
        "state": "connected",
        "padding": "x".repeat(4096),
    });
    if lines[4..].iter().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .map_or(true, |frame| frame != expected_regular)
    }) {
        return Err(
            "Callosum emitted an unexpected regular frame after the tunnel-close tail".to_owned(),
        );
    }
    if lines.iter().any(|line| line.contains("process-test-token")) {
        return Err("tunnel-close tail leaked a service token".to_owned());
    }
    Ok(())
}
