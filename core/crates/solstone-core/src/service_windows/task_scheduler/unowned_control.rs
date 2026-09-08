// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Task Scheduler control command only: deliberately Unowned (spawn census E33/I02).
//! Its direct worker is bounded; scheduler RPC and tasks are outside this process.

use std::io;
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

pub(super) struct ControlOutput {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

async fn read_capped(reader: impl AsyncRead + Unpin, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > limit {
        return Err(io::Error::other(
            "task control output exceeds its byte limit",
        ));
    }
    Ok(bytes)
}

pub(super) fn run(
    mut command: Command,
    input: Vec<u8>,
    caller_deadline: std::time::Instant,
) -> Result<ControlOutput, String> {
    if input.len() > 128 * 1024 {
        return Err("task control input exceeds its byte limit".into());
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err("task control requires the synchronous service entry".into());
    }
    // The async deadline begins after runtime construction. Command::spawn is
    // synchronous too: neither call is falsely described as deadline-cancellable.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let remaining = caller_deadline.saturating_duration_since(std::time::Instant::now());
        if remaining <= Duration::from_millis(10) { return Err("task control deadline elapsed before spawn".into()); }
        let budget = remaining.min(Duration::from_secs(15));
        let deadline = tokio::time::Instant::now() + budget;
        // Reserve the last two seconds for direct-worker observation only.
        // No Job, descendant ownership, breakaway, or helper-cleanup registry.
        let work_deadline = deadline - Duration::from_secs(2).min(budget / 4);
        let mut command = tokio::process::Command::from(command);
        command.kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| format!("task control spawn failed: {error}"))?;
        let exchange = async {
            let mut stdin = child.stdin.take().ok_or_else(|| io::Error::other("task control stdin missing"))?;
            let stdout = child.stdout.take().ok_or_else(|| io::Error::other("task control stdout missing"))?;
            let stderr = child.stderr.take().ok_or_else(|| io::Error::other("task control stderr missing"))?;
            let write = async move {
                stdin.write_all(&input).await?;
                stdin.shutdown().await?;
                drop(stdin);
                Ok::<(), io::Error>(())
            };
            let (_, stdout, stderr, status) = tokio::try_join!(write,
                read_capped(stdout, 256 * 1024), read_capped(stderr, 16 * 1024), child.wait())?;
            Ok::<_, io::Error>(ControlOutput { code: status.code(), stdout, stderr })
        };
        let reason = match tokio::time::timeout_at(work_deadline, exchange).await {
            Ok(Ok(output)) => return Ok(output),
            Ok(Err(error)) => format!("task control exchange failed: {error}"),
            Err(_) => "task control exchange timed out".to_owned(),
        };
        let termination = child.start_kill();
        let observed = tokio::time::timeout_at(deadline, child.wait()).await;
        Err(format!("{reason}; direct-worker stop request: {termination:?}; direct-worker observation: {observed:?}; scheduler RPC may have changed state; re-inspection required"))
    })
}
