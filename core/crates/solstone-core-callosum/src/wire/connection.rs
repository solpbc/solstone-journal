// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout};

use crate::CallosumEnvelope;

use super::frame::encode_envelope;
use super::framing::{ReadFrame, read_frame, reader};
use super::{
    CLIENT_INBOUND_CAPACITY, CLIENT_OUTBOUND_CAPACITY, CLIENT_RECONNECT_INTERVAL,
    CLIENT_SEND_TIMEOUT, CLIENT_STOP_JOIN_TIMEOUT,
};

/// Long-lived, reconnecting Callosum Unix-socket client.
pub struct CallosumSocketConnection {
    socket_path: PathBuf,
    defaults: Map<String, Value>,
    outbound: mpsc::Sender<CallosumEnvelope>,
    outbound_rx: Option<mpsc::Receiver<CallosumEnvelope>>,
    inbound: mpsc::Receiver<CallosumEnvelope>,
    inbound_tx: mpsc::Sender<CallosumEnvelope>,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    malformed_frame_drops: Arc<AtomicU64>,
    outbound_saturation_drops: Arc<AtomicU64>,
}

impl CallosumSocketConnection {
    /// Construct an idle connection. Call [`Self::start`] before emitting.
    pub fn new(socket_path: impl AsRef<Path>, mut defaults: Map<String, Value>) -> Self {
        defaults.retain(|_, value| !value.is_null());
        let (outbound, outbound_rx) = mpsc::channel(CLIENT_OUTBOUND_CAPACITY);
        let (inbound_tx, inbound) = mpsc::channel(CLIENT_INBOUND_CAPACITY);
        let (shutdown, _) = watch::channel(false);
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            defaults,
            outbound,
            outbound_rx: Some(outbound_rx),
            inbound,
            inbound_tx,
            shutdown,
            task: None,
            running: Arc::new(AtomicBool::new(false)),
            malformed_frame_drops: Arc::new(AtomicU64::new(0)),
            outbound_saturation_drops: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Start background connect, send, and receive processing.
    pub fn start(&mut self) {
        if self.running.load(Ordering::Acquire) {
            return;
        }
        let Some(outbound) = self.outbound_rx.take() else {
            return;
        };
        self.running.store(true, Ordering::Release);
        self.task = Some(tokio::spawn(run_connection(
            self.socket_path.clone(),
            outbound,
            self.inbound_tx.clone(),
            self.shutdown.subscribe(),
            Arc::clone(&self.running),
            Arc::clone(&self.malformed_frame_drops),
            Arc::clone(&self.outbound_saturation_drops),
        )));
    }

    /// Queue an outbound message with Python-compatible field precedence.
    pub fn emit(&self, tract: &str, event: &str, caller_fields: Map<String, Value>) -> bool {
        if !self.running.load(Ordering::Acquire) {
            return false;
        }
        let mut merged = self.defaults.clone();
        merged.insert("tract".to_owned(), Value::String(tract.to_owned()));
        merged.insert("event".to_owned(), Value::String(event.to_owned()));
        merged.extend(caller_fields);
        let Ok(envelope) = serde_json::from_value::<CallosumEnvelope>(Value::Object(merged)) else {
            return false;
        };
        match self.outbound.try_send(envelope) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                let _ = self
                    .outbound_saturation_drops
                    .fetch_add(1, Ordering::AcqRel);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Receive the next reflected Callosum message.
    pub async fn next_message(&mut self) -> Option<CallosumEnvelope> {
        self.inbound.recv().await
    }

    /// Count invalid JSON, schema, or UTF-8 frames dropped by this peer.
    #[must_use]
    pub fn malformed_frame_drops(&self) -> u64 {
        self.malformed_frame_drops.load(Ordering::Acquire)
    }

    /// Count emits rejected because this connection's outbound queue was full.
    #[must_use]
    pub fn outbound_saturation_drops(&self) -> u64 {
        self.outbound_saturation_drops.load(Ordering::Acquire)
    }

    /// Request best-effort outbound draining without starting a new connection.
    pub async fn stop(&mut self) {
        if !self.running.swap(false, Ordering::AcqRel) {
            return;
        }
        let _ = self.shutdown.send(true);
        let Some(mut task) = self.task.take() else {
            return;
        };
        // Unlike server stop, connection stop keeps draining; this is only a nonblocking join wait.
        if timeout(CLIENT_STOP_JOIN_TIMEOUT, &mut task).await.is_err() {
            eprintln!("callosum wire: connection drain continues after stop returns");
        }
    }
}

struct ConnectedStream {
    reader: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

async fn run_connection(
    socket_path: PathBuf,
    mut outbound: mpsc::Receiver<CallosumEnvelope>,
    inbound: mpsc::Sender<CallosumEnvelope>,
    mut shutdown: watch::Receiver<bool>,
    running: Arc<AtomicBool>,
    malformed_frame_drops: Arc<AtomicU64>,
    outbound_saturation_drops: Arc<AtomicU64>,
) {
    let mut stream: Option<ConnectedStream> = None;
    let mut last_attempt: Option<Instant> = None;
    let mut buffer = Vec::new();

    loop {
        if *shutdown.borrow() {
            drain_outbound(&mut outbound, &mut stream).await;
            break;
        }
        if stream.is_none() {
            while outbound.try_recv().is_ok() {}
            let delay = last_attempt
                .map(|attempt| CLIENT_RECONNECT_INTERVAL.saturating_sub(attempt.elapsed()))
                .unwrap_or_default();
            if !delay.is_zero() {
                tokio::select! {
                    changed = shutdown.changed() => {
                        let _ = changed;
                        continue;
                    }
                    _ = sleep(delay) => {}
                }
                continue;
            }
            last_attempt = Some(Instant::now());
            if let Ok(socket) = UnixStream::connect(&socket_path).await {
                let (read_half, writer) = socket.into_split();
                stream = Some(ConnectedStream {
                    reader: reader(read_half),
                    writer,
                });
            }
            continue;
        }

        let connected = stream.as_mut().expect("connection checked above");
        tokio::select! {
            changed = shutdown.changed() => {
                let _ = changed;
            }
            message = outbound.recv() => match message {
                Some(message) => {
                    let sent = match encode_envelope(&message) {
                        Ok(line) => timeout(CLIENT_SEND_TIMEOUT, connected.writer.write_all(&line)).await,
                        Err(_) => {
                            stream = None;
                            continue;
                        }
                    };
                    if !matches!(sent, Ok(Ok(()))) {
                        stream = None;
                    }
                }
                None => break,
            },
            frame = read_frame(&mut connected.reader, &mut buffer) => match frame {
                Ok(ReadFrame::Envelope(message)) => {
                    let _ = inbound.try_send(message);
                }
                Ok(ReadFrame::Whitespace) => {}
                Ok(ReadFrame::Malformed) | Ok(ReadFrame::InvalidUtf8) => {
                    let _ = malformed_frame_drops.fetch_add(1, Ordering::AcqRel);
                }
                Ok(ReadFrame::Eof) | Err(_) => stream = None,
            },
        }
    }
    running.store(false, Ordering::Release);
    let _ = outbound_saturation_drops;
}

async fn drain_outbound(
    outbound: &mut mpsc::Receiver<CallosumEnvelope>,
    stream: &mut Option<ConnectedStream>,
) {
    while let Ok(message) = outbound.try_recv() {
        let Some(connected) = stream.as_mut() else {
            continue;
        };
        let Ok(line) = encode_envelope(&message) else {
            continue;
        };
        if !matches!(
            timeout(CLIENT_SEND_TIMEOUT, connected.writer.write_all(&line)).await,
            Ok(Ok(()))
        ) {
            *stream = None;
        }
    }
}
