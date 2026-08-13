// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use crate::CallosumEnvelope;

use super::frame::encode_envelope;
use super::framing::{ReadFrame, read_frame, reader};
use super::{
    CLIENT_INBOUND_CAPACITY, CLIENT_OUTBOUND_CAPACITY, CLIENT_RECONNECT_INTERVAL,
    CLIENT_SEND_TIMEOUT, CLIENT_STOP_JOIN_TIMEOUT,
};

/// The current continuity state of a reconnecting socket reader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallosumConnectionPhase {
    Connecting {
        attempt: u64,
    },
    Unavailable {
        latest_attempt: u64,
        failures_since_success: u64,
    },
    Connected,
    Gapped {
        reason: CallosumGapReason,
        dropped_count: u64,
    },
    Stopped {
        reason: CallosumStoppedReason,
    },
}

/// The reason a complete event stream is no longer available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallosumGapReason {
    Disconnected,
    MalformedFrameDropped,
    InboundSaturated,
}

/// A terminal socket-reader condition which cannot safely reuse counters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallosumStoppedReason {
    CounterOverflow,
    RetrySourceClosed,
}

/// One ordered inbound item. Continuity always has priority over data.
#[derive(Clone, Debug)]
pub enum CallosumReceiveEvent {
    Continuity {
        generation: u64,
        epoch: u64,
        phase: CallosumConnectionPhase,
    },
    Envelope {
        generation: u64,
        epoch: u64,
        envelope: CallosumEnvelope,
    },
}

/// Supplies bounded reconnect opportunities. Production sleeps between attempts;
/// deterministic tests explicitly release each attempt.
pub trait CallosumRetrySource: Send {
    fn next_attempt(&mut self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
}

/// The production reconnect cadence.
pub struct TokioRetrySource {
    first: bool,
}

impl Default for TokioRetrySource {
    fn default() -> Self {
        Self { first: true }
    }
}

impl CallosumRetrySource for TokioRetrySource {
    fn next_attempt(&mut self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        let first = std::mem::replace(&mut self.first, false);
        Box::pin(async move {
            if !first {
                sleep(CLIENT_RECONNECT_INTERVAL).await;
            }
            true
        })
    }
}

/// A single replacing continuity slot which is independent of the bounded
/// envelope channel. It is intentionally private: consumers observe it through
/// `CallosumSocketConnection` so priority cannot be bypassed.
struct PriorityLatch {
    slot: std::sync::Mutex<Option<CallosumReceiveEvent>>,
    changed: Notify,
}

impl PriorityLatch {
    fn replace(&self, event: CallosumReceiveEvent) {
        let mut slot = lock(&self.slot);
        *slot = Some(event);
        self.changed.notify_waiters();
    }

    fn take(&self) -> Option<CallosumReceiveEvent> {
        let event = lock(&self.slot).take();
        if event.is_some() {
            self.changed.notify_waiters();
        }
        event
    }

    fn pending(&self) -> bool {
        lock(&self.slot).is_some()
    }

    fn replace_gap(
        &self,
        generation: u64,
        epoch: u64,
        reason: CallosumGapReason,
        dropped: u64,
    ) -> bool {
        let mut slot = lock(&self.slot);
        let (reason, dropped_count) = match slot.as_ref() {
            Some(CallosumReceiveEvent::Continuity {
                generation: previous_generation,
                epoch: previous_epoch,
                phase:
                    CallosumConnectionPhase::Gapped {
                        reason: previous_reason,
                        dropped_count,
                    },
            }) if *previous_generation == generation && *previous_epoch == epoch => {
                let Some(dropped_count) = dropped_count.checked_add(dropped) else {
                    *slot = Some(CallosumReceiveEvent::Continuity {
                        generation,
                        epoch,
                        phase: CallosumConnectionPhase::Stopped {
                            reason: CallosumStoppedReason::CounterOverflow,
                        },
                    });
                    self.changed.notify_waiters();
                    return false;
                };
                (previous_reason.clone(), dropped_count)
            }
            _ => (reason, dropped),
        };
        *slot = Some(CallosumReceiveEvent::Continuity {
            generation,
            epoch,
            phase: CallosumConnectionPhase::Gapped {
                reason,
                dropped_count,
            },
        });
        self.changed.notify_waiters();
        true
    }

    async fn wait_until_consumed(&self) {
        loop {
            let notified = self.changed.notified();
            if !self.pending() {
                return;
            }
            notified.await;
        }
    }
}

struct InboundQueues {
    priority: Arc<PriorityLatch>,
    data: mpsc::Sender<CallosumReceiveEvent>,
}

impl InboundQueues {
    fn continuity(&self, generation: u64, epoch: u64, phase: CallosumConnectionPhase) {
        self.priority.replace(CallosumReceiveEvent::Continuity {
            generation,
            epoch,
            phase,
        });
    }

    fn try_send_envelope(&self, generation: u64, epoch: u64, envelope: CallosumEnvelope) -> bool {
        if self.priority.pending() {
            return false;
        }
        self.data
            .try_send(CallosumReceiveEvent::Envelope {
                generation,
                epoch,
                envelope,
            })
            .is_ok()
    }
}

/// Long-lived, reconnecting Callosum Unix-socket client.
pub struct CallosumSocketConnection {
    socket_path: PathBuf,
    defaults: Map<String, Value>,
    outbound: mpsc::Sender<CallosumEnvelope>,
    outbound_rx: Option<mpsc::Receiver<CallosumEnvelope>>,
    inbound: mpsc::Receiver<CallosumReceiveEvent>,
    queues: Arc<InboundQueues>,
    retry_source: Option<Box<dyn CallosumRetrySource>>,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    malformed_frame_drops: Arc<AtomicU64>,
    outbound_saturation_drops: Arc<AtomicU64>,
    delivered_generation: u64,
    delivered_epoch: u64,
    initial_counters: ConnectionCounters,
    initial_first_attempt: bool,
}

impl CallosumSocketConnection {
    /// Construct an idle connection. Call [`Self::start`] before emitting.
    pub fn new(socket_path: impl AsRef<Path>, mut defaults: Map<String, Value>) -> Self {
        Self::with_parts(
            socket_path,
            &mut defaults,
            CLIENT_INBOUND_CAPACITY,
            Box::<TokioRetrySource>::default(),
        )
    }

    /// Construct a connection with an explicit inbound data capacity.
    ///
    /// This is primarily useful to deterministic adapters that need to prove
    /// priority behavior at capacity one.
    pub fn with_inbound_capacity(
        socket_path: impl AsRef<Path>,
        mut defaults: Map<String, Value>,
        inbound_capacity: usize,
    ) -> Self {
        Self::with_parts(
            socket_path,
            &mut defaults,
            inbound_capacity,
            Box::<TokioRetrySource>::default(),
        )
    }

    /// Construct a connection with an injected retry cadence.
    pub fn with_retry_source(
        socket_path: impl AsRef<Path>,
        mut defaults: Map<String, Value>,
        inbound_capacity: usize,
        retry_source: Box<dyn CallosumRetrySource>,
    ) -> Self {
        Self::with_parts(socket_path, &mut defaults, inbound_capacity, retry_source)
    }

    fn with_parts(
        socket_path: impl AsRef<Path>,
        defaults: &mut Map<String, Value>,
        inbound_capacity: usize,
        retry_source: Box<dyn CallosumRetrySource>,
    ) -> Self {
        defaults.retain(|_, value| !value.is_null());
        let (outbound, outbound_rx) = mpsc::channel(CLIENT_OUTBOUND_CAPACITY);
        let (data, inbound) = mpsc::channel(inbound_capacity);
        let queues = Arc::new(InboundQueues {
            priority: Arc::new(PriorityLatch {
                slot: std::sync::Mutex::new(None),
                changed: Notify::new(),
            }),
            data,
        });
        let (shutdown, _) = watch::channel(false);
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            defaults: std::mem::take(defaults),
            outbound,
            outbound_rx: Some(outbound_rx),
            inbound,
            queues,
            retry_source: Some(retry_source),
            shutdown,
            task: None,
            running: Arc::new(AtomicBool::new(false)),
            malformed_frame_drops: Arc::new(AtomicU64::new(0)),
            outbound_saturation_drops: Arc::new(AtomicU64::new(0)),
            delivered_generation: 0,
            delivered_epoch: 0,
            initial_counters: ConnectionCounters::initial(),
            initial_first_attempt: true,
        }
    }

    #[cfg(test)]
    fn with_retry_source_and_initial_counters(
        socket_path: impl AsRef<Path>,
        mut defaults: Map<String, Value>,
        inbound_capacity: usize,
        retry_source: Box<dyn CallosumRetrySource>,
        initial_counters: ConnectionCounters,
        initial_first_attempt: bool,
    ) -> Self {
        let mut connection =
            Self::with_parts(socket_path, &mut defaults, inbound_capacity, retry_source);
        connection.initial_counters = initial_counters;
        connection.initial_first_attempt = initial_first_attempt;
        connection
    }

    /// Start background connect, send, and receive processing.
    pub fn start(&mut self) {
        if self.running.load(Ordering::Acquire) {
            return;
        }
        let (Some(outbound), Some(retry_source)) =
            (self.outbound_rx.take(), self.retry_source.take())
        else {
            return;
        };
        // This is deliberately synchronous: a first Top render must see the
        // connection attempt even when the runtime has not scheduled the task.
        self.queues
            .continuity(0, 0, CallosumConnectionPhase::Connecting { attempt: 1 });
        self.running.store(true, Ordering::Release);
        let counters = std::mem::replace(&mut self.initial_counters, ConnectionCounters::initial());
        let first_attempt = self.initial_first_attempt;
        self.task = Some(tokio::spawn(run_connection(ConnectionRun {
            socket_path: self.socket_path.clone(),
            outbound,
            queues: Arc::clone(&self.queues),
            retry_source,
            shutdown: self.shutdown.subscribe(),
            running: Arc::clone(&self.running),
            malformed_frame_drops: Arc::clone(&self.malformed_frame_drops),
            counters,
            first_attempt,
        })));
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
    pub async fn next_message(&mut self) -> Option<CallosumEnvelope> {
        while let Some(event) = self.next_event().await {
            if let CallosumReceiveEvent::Envelope { envelope, .. } = event {
                return Some(envelope);
            }
        }
        None
    }

    /// Receive the next ordered envelope or non-droppable continuity marker.
    pub async fn next_event(&mut self) -> Option<CallosumReceiveEvent> {
        loop {
            if let Some(event) = self.take_priority() {
                return Some(event);
            }
            tokio::select! {
                biased;
                _ = self.queues.priority.changed.notified() => {}
                event = self.inbound.recv() => match event {
                    Some(event) if self.current(event.clone()) => return Some(event),
                    Some(_) => {}
                    None => return self.take_priority(),
                },
            }
        }
    }

    /// Return a ready receive item without waiting for socket activity.
    pub fn try_next_event(&mut self) -> Option<CallosumReceiveEvent> {
        if let Some(event) = self.take_priority() {
            return Some(event);
        }
        while let Ok(event) = self.inbound.try_recv() {
            if self.current(event.clone()) {
                return Some(event);
            }
        }
        None
    }

    fn take_priority(&mut self) -> Option<CallosumReceiveEvent> {
        let event = self.queues.priority.take()?;
        if let CallosumReceiveEvent::Continuity {
            generation,
            epoch,
            ref phase,
        } = event
        {
            self.delivered_generation = generation;
            self.delivered_epoch = epoch;
            if matches!(phase, CallosumConnectionPhase::Gapped { .. }) {
                while self.inbound.try_recv().is_ok() {}
            }
        }
        Some(event)
    }

    fn current(&self, event: CallosumReceiveEvent) -> bool {
        matches!(event, CallosumReceiveEvent::Envelope { generation, epoch, .. }
            if generation == self.delivered_generation && epoch == self.delivered_epoch)
    }

    #[must_use]
    pub fn malformed_frame_drops(&self) -> u64 {
        self.malformed_frame_drops.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn outbound_saturation_drops(&self) -> u64 {
        self.outbound_saturation_drops.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn has_pending_priority(&self) -> bool {
        self.queues.priority.pending()
    }

    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub async fn stop(&mut self) {
        if !self.running.swap(false, Ordering::AcqRel) {
            return;
        }
        let _ = self.shutdown.send(true);
        let Some(mut task) = self.task.take() else {
            return;
        };
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

#[derive(Clone)]
struct ConnectionCounters {
    generation: u64,
    epoch: u64,
    attempt: u64,
    failures_since_success: u64,
}

impl ConnectionCounters {
    fn initial() -> Self {
        Self {
            generation: 0,
            epoch: 0,
            attempt: 1,
            failures_since_success: 0,
        }
    }

    fn checked_increment(value: &mut u64) -> bool {
        let Some(next) = value.checked_add(1) else {
            return false;
        };
        *value = next;
        true
    }
}

struct ConnectionRun {
    socket_path: PathBuf,
    outbound: mpsc::Receiver<CallosumEnvelope>,
    queues: Arc<InboundQueues>,
    retry_source: Box<dyn CallosumRetrySource>,
    shutdown: watch::Receiver<bool>,
    running: Arc<AtomicBool>,
    malformed_frame_drops: Arc<AtomicU64>,
    counters: ConnectionCounters,
    first_attempt: bool,
}

async fn run_connection(run: ConnectionRun) {
    let ConnectionRun {
        socket_path,
        mut outbound,
        queues,
        mut retry_source,
        mut shutdown,
        running,
        malformed_frame_drops,
        mut counters,
        mut first_attempt,
    } = run;
    let mut stream: Option<ConnectedStream> = None;
    let mut buffer = Vec::new();
    let mut gapped = false;
    let mut resume_current = false;

    loop {
        if *shutdown.borrow() {
            drain_outbound(&mut outbound, &mut stream).await;
            break;
        }
        if queues.priority.pending() && (stream.is_some() || gapped) {
            tokio::select! {
                changed = shutdown.changed() => {
                    let _ = changed;
                }
                () = queues.priority.wait_until_consumed() => {
                    if resume_current && stream.is_some() {
                        queues.continuity(
                            counters.generation,
                            counters.epoch,
                            CallosumConnectionPhase::Connected,
                        );
                        resume_current = false;
                    }
                }
            }
            continue;
        }
        if stream.is_none() {
            tokio::select! {
                changed = shutdown.changed() => {
                    let _ = changed;
                    continue;
                }
                permitted = retry_source.next_attempt() => {
                    if !permitted {
                        queues.continuity(counters.generation, counters.epoch, CallosumConnectionPhase::Stopped { reason: CallosumStoppedReason::RetrySourceClosed });
                        break;
                    }
                }
            }
            if !first_attempt {
                if !ConnectionCounters::checked_increment(&mut counters.attempt) {
                    queues.continuity(
                        counters.generation,
                        counters.epoch,
                        CallosumConnectionPhase::Stopped {
                            reason: CallosumStoppedReason::CounterOverflow,
                        },
                    );
                    break;
                }
                queues.continuity(
                    counters.generation,
                    counters.epoch,
                    CallosumConnectionPhase::Connecting {
                        attempt: counters.attempt,
                    },
                );
            }
            first_attempt = false;
            while outbound.try_recv().is_ok() {}
            match UnixStream::connect(&socket_path).await {
                Ok(socket) => {
                    if !ConnectionCounters::checked_increment(&mut counters.generation)
                        || !ConnectionCounters::checked_increment(&mut counters.epoch)
                    {
                        queues.continuity(
                            counters.generation,
                            counters.epoch,
                            CallosumConnectionPhase::Stopped {
                                reason: CallosumStoppedReason::CounterOverflow,
                            },
                        );
                        break;
                    }
                    counters.failures_since_success = 0;
                    gapped = false;
                    resume_current = false;
                    queues.continuity(
                        counters.generation,
                        counters.epoch,
                        CallosumConnectionPhase::Connected,
                    );
                    let (read_half, writer) = socket.into_split();
                    stream = Some(ConnectedStream {
                        reader: reader(read_half),
                        writer,
                    });
                }
                Err(_) => {
                    if !ConnectionCounters::checked_increment(&mut counters.failures_since_success)
                    {
                        queues.continuity(
                            counters.generation,
                            counters.epoch,
                            CallosumConnectionPhase::Stopped {
                                reason: CallosumStoppedReason::CounterOverflow,
                            },
                        );
                        break;
                    }
                    queues.continuity(
                        counters.generation,
                        counters.epoch,
                        CallosumConnectionPhase::Unavailable {
                            latest_attempt: counters.attempt,
                            failures_since_success: counters.failures_since_success,
                        },
                    );
                }
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
                        Ok(line) => matches!(timeout(CLIENT_SEND_TIMEOUT, connected.writer.write_all(&line)).await, Ok(Ok(()))),
                        Err(_) => false,
                    };
                    if !sent {
                        if enter_gap(&queues, &mut counters, &mut gapped, CallosumGapReason::Disconnected, 1) {
                            break;
                        }
                        stream = None;
                    }
                }
                None => break,
            },
            frame = read_frame(&mut connected.reader, &mut buffer) => match frame {
                Ok(ReadFrame::Envelope(message)) => {
                    if !queues.try_send_envelope(counters.generation, counters.epoch, message) {
                        if enter_gap(&queues, &mut counters, &mut gapped, CallosumGapReason::InboundSaturated, 1) {
                            break;
                        }
                        resume_current = stream.is_some();
                    }
                }
                Ok(ReadFrame::Whitespace) => {}
                Ok(ReadFrame::Malformed) | Ok(ReadFrame::InvalidUtf8) => {
                    let _ = malformed_frame_drops.fetch_add(1, Ordering::AcqRel);
                    if enter_gap(&queues, &mut counters, &mut gapped, CallosumGapReason::MalformedFrameDropped, 1) {
                        break;
                    }
                    resume_current = stream.is_some();
                }
                Ok(ReadFrame::Eof) | Err(_) => {
                    if enter_gap(&queues, &mut counters, &mut gapped, CallosumGapReason::Disconnected, 1) {
                        break;
                    }
                    stream = None;
                }
            },
        }
    }
    running.store(false, Ordering::Release);
}

fn enter_gap(
    queues: &InboundQueues,
    counters: &mut ConnectionCounters,
    gapped: &mut bool,
    reason: CallosumGapReason,
    dropped: u64,
) -> bool {
    if !*gapped {
        if !ConnectionCounters::checked_increment(&mut counters.epoch) {
            queues.continuity(
                counters.generation,
                counters.epoch,
                CallosumConnectionPhase::Stopped {
                    reason: CallosumStoppedReason::CounterOverflow,
                },
            );
            return true;
        }
        *gapped = true;
    }
    if !queues
        .priority
        .replace_gap(counters.generation, counters.epoch, reason, dropped)
    {
        return true;
    }
    false
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

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod counter_tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use serde_json::Map;
    use tokio::time::timeout;

    use super::{
        CallosumConnectionPhase, CallosumRetrySource, CallosumSocketConnection,
        CallosumStoppedReason, ConnectionCounters,
    };

    static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);

    struct ImmediateRetry;

    impl CallosumRetrySource for ImmediateRetry {
        fn next_attempt(&mut self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
            Box::pin(async { true })
        }
    }

    #[test]
    fn generation_and_attempt_overflow_stop_without_reuse() {
        let mut generation = u64::MAX;
        let mut attempt = u64::MAX;
        assert!(!ConnectionCounters::checked_increment(&mut generation));
        assert!(!ConnectionCounters::checked_increment(&mut attempt));
        assert_eq!(generation, u64::MAX);
        assert_eq!(attempt, u64::MAX);
        assert_eq!(
            CallosumConnectionPhase::Stopped {
                reason: CallosumStoppedReason::CounterOverflow,
            },
            CallosumConnectionPhase::Stopped {
                reason: CallosumStoppedReason::CounterOverflow,
            }
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn counter_overflow_is_delivered_as_a_stopped_continuity_event() {
        let ordinal = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("solstone-counter-overflow-{ordinal}"));
        std::fs::create_dir_all(&root).unwrap();

        let mut attempt = CallosumSocketConnection::with_retry_source_and_initial_counters(
            root.join("missing.sock"),
            Map::new(),
            1,
            Box::new(ImmediateRetry),
            ConnectionCounters {
                generation: 0,
                epoch: 0,
                attempt: u64::MAX,
                failures_since_success: 0,
            },
            false,
        );
        attempt.start();
        let _ = attempt.next_event().await;
        assert!(matches!(
            timeout(Duration::from_secs(1), attempt.next_event())
                .await
                .unwrap(),
            Some(super::CallosumReceiveEvent::Continuity {
                phase: CallosumConnectionPhase::Stopped {
                    reason: CallosumStoppedReason::CounterOverflow
                },
                ..
            })
        ));

        let socket = root.join("connected.sock");
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let mut generation = CallosumSocketConnection::with_retry_source_and_initial_counters(
            &socket,
            Map::new(),
            1,
            Box::new(ImmediateRetry),
            ConnectionCounters {
                generation: u64::MAX,
                epoch: 0,
                attempt: 1,
                failures_since_success: 0,
            },
            true,
        );
        generation.start();
        let _ = generation.next_event().await;
        assert!(matches!(
            timeout(Duration::from_secs(1), generation.next_event())
                .await
                .unwrap(),
            Some(super::CallosumReceiveEvent::Continuity {
                phase: CallosumConnectionPhase::Stopped {
                    reason: CallosumStoppedReason::CounterOverflow
                },
                ..
            })
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
