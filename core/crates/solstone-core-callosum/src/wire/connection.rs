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

/// A continuity boundary observed by the reconnecting socket reader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallosumDiscontinuity {
    /// A fresh socket connection was established for this generation.
    Connected,
    /// The socket reached EOF or an I/O error and its stream was discarded.
    Disconnected,
    /// A malformed or non-UTF-8 frame was dropped.
    MalformedFrameDropped,
    /// An inbound envelope was dropped because the consumer was saturated.
    InboundSaturated,
}

/// An ordered inbound item with the socket generation that produced it.
#[derive(Clone, Debug)]
pub enum CallosumReceiveEvent {
    /// A decoded Callosum envelope.
    Envelope {
        generation: u64,
        envelope: CallosumEnvelope,
    },
    /// A continuity boundary for the current generation.
    Discontinuity {
        generation: u64,
        reason: CallosumDiscontinuity,
    },
}

/// Long-lived, reconnecting Callosum Unix-socket client.
pub struct CallosumSocketConnection {
    socket_path: PathBuf,
    defaults: Map<String, Value>,
    outbound: mpsc::Sender<CallosumEnvelope>,
    outbound_rx: Option<mpsc::Receiver<CallosumEnvelope>>,
    inbound: mpsc::Receiver<CallosumReceiveEvent>,
    inbound_tx: mpsc::Sender<CallosumReceiveEvent>,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    malformed_frame_drops: Arc<AtomicU64>,
    outbound_saturation_drops: Arc<AtomicU64>,
}

impl CallosumSocketConnection {
    /// Construct an idle connection. Call [`Self::start`] before emitting.
    pub fn new(socket_path: impl AsRef<Path>, mut defaults: Map<String, Value>) -> Self {
        Self::with_inbound_capacity(socket_path, &mut defaults, CLIENT_INBOUND_CAPACITY)
    }

    #[cfg(test)]
    pub(crate) fn with_test_inbound_capacity(
        socket_path: impl AsRef<Path>,
        mut defaults: Map<String, Value>,
        inbound_capacity: usize,
    ) -> Self {
        Self::with_inbound_capacity(socket_path, &mut defaults, inbound_capacity)
    }

    fn with_inbound_capacity(
        socket_path: impl AsRef<Path>,
        defaults: &mut Map<String, Value>,
        inbound_capacity: usize,
    ) -> Self {
        defaults.retain(|_, value| !value.is_null());
        let (outbound, outbound_rx) = mpsc::channel(CLIENT_OUTBOUND_CAPACITY);
        let (inbound_tx, inbound) = mpsc::channel(inbound_capacity);
        let (shutdown, _) = watch::channel(false);
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            defaults: std::mem::take(defaults),
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

    /// Receive the next reflected Callosum message, skipping continuity markers.
    ///
    /// This compatibility method intentionally retains its original envelope-only
    /// contract. New consumers that must detect reconnects or frame loss should
    /// use [`Self::next_event`].
    pub async fn next_message(&mut self) -> Option<CallosumEnvelope> {
        while let Some(event) = self.inbound.recv().await {
            if let CallosumReceiveEvent::Envelope { envelope, .. } = event {
                return Some(envelope);
            }
        }
        None
    }

    /// Receive the next ordered envelope or continuity marker.
    pub async fn next_event(&mut self) -> Option<CallosumReceiveEvent> {
        self.inbound.recv().await
    }

    /// Return a ready receive item without waiting for socket activity.
    ///
    /// Interactive consumers use this to drain the ordered stream between
    /// terminal input polls. `None` means no item is immediately available or
    /// that the connection has stopped; callers that need to distinguish those
    /// cases should use [`Self::next_event`].
    pub fn try_next_event(&mut self) -> Option<CallosumReceiveEvent> {
        self.inbound.try_recv().ok()
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

impl Drop for CallosumSocketConnection {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        let _ = self.shutdown.send(true);
    }
}

struct ConnectedStream {
    reader: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

async fn run_connection(
    socket_path: PathBuf,
    mut outbound: mpsc::Receiver<CallosumEnvelope>,
    inbound: mpsc::Sender<CallosumReceiveEvent>,
    mut shutdown: watch::Receiver<bool>,
    running: Arc<AtomicBool>,
    malformed_frame_drops: Arc<AtomicU64>,
    outbound_saturation_drops: Arc<AtomicU64>,
) {
    let mut stream: Option<ConnectedStream> = None;
    let mut last_attempt: Option<Instant> = None;
    let mut buffer = Vec::new();
    let mut generation = 0_u64;
    let mut pending_saturation = false;

    loop {
        if *shutdown.borrow() {
            drain_outbound(&mut outbound, &mut stream).await;
            break;
        }
        if stream.is_none() {
            if pending_saturation
                && try_send_event(
                    &inbound,
                    CallosumReceiveEvent::Discontinuity {
                        generation,
                        reason: CallosumDiscontinuity::InboundSaturated,
                    },
                )
            {
                pending_saturation = false;
            }
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
                generation = generation.wrapping_add(1);
                if !try_send_event(
                    &inbound,
                    CallosumReceiveEvent::Discontinuity {
                        generation,
                        reason: CallosumDiscontinuity::Connected,
                    },
                ) {
                    pending_saturation = true;
                }
                stream = Some(ConnectedStream {
                    reader: reader(read_half),
                    writer,
                });
            }
            continue;
        }

        if pending_saturation
            && try_send_event(
                &inbound,
                CallosumReceiveEvent::Discontinuity {
                    generation,
                    reason: CallosumDiscontinuity::InboundSaturated,
                },
            )
        {
            pending_saturation = false;
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
                            let _ = try_send_event(
                                &inbound,
                                CallosumReceiveEvent::Discontinuity {
                                    generation,
                                    reason: CallosumDiscontinuity::Disconnected,
                                },
                            );
                            stream = None;
                            continue;
                        }
                    };
                    if !matches!(sent, Ok(Ok(()))) {
                        let _ = try_send_event(
                            &inbound,
                            CallosumReceiveEvent::Discontinuity {
                                generation,
                                reason: CallosumDiscontinuity::Disconnected,
                            },
                        );
                        stream = None;
                    }
                }
                None => break,
            },
            frame = read_frame(&mut connected.reader, &mut buffer) => match frame {
                Ok(ReadFrame::Envelope(message)) => {
                    if !try_send_event(
                        &inbound,
                        CallosumReceiveEvent::Envelope { generation, envelope: message },
                    )
                    {
                        pending_saturation = true;
                    }
                }
                Ok(ReadFrame::Whitespace) => {}
                Ok(ReadFrame::Malformed) | Ok(ReadFrame::InvalidUtf8) => {
                    let _ = malformed_frame_drops.fetch_add(1, Ordering::AcqRel);
                    let _ = try_send_event(
                        &inbound,
                        CallosumReceiveEvent::Discontinuity {
                            generation,
                            reason: CallosumDiscontinuity::MalformedFrameDropped,
                        },
                    );
                }
                Ok(ReadFrame::Eof) | Err(_) => {
                    let _ = try_send_event(
                        &inbound,
                        CallosumReceiveEvent::Discontinuity {
                            generation,
                            reason: CallosumDiscontinuity::Disconnected,
                        },
                    );
                    stream = None;
                }
            },
        }
    }
    running.store(false, Ordering::Release);
    let _ = outbound_saturation_drops;
}

fn try_send_event(
    inbound: &mpsc::Sender<CallosumReceiveEvent>,
    event: CallosumReceiveEvent,
) -> bool {
    inbound.try_send(event).is_ok()
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
