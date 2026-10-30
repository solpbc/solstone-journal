// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
#[cfg(any(test, feature = "test-hooks"))]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{Map, Value};
#[cfg(windows)]
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use super::brain_startup::BrainStartupCapture;
use crate::CallosumEnvelope;

#[cfg(windows)]
use super::SERVER_SEND_TIMEOUT;
use super::frame::encode_envelope;
use super::framing::{ReadFrame, read_frame, reader};
use super::{
    CLIENT_INBOUND_CAPACITY, CLIENT_OUTBOUND_CAPACITY, CLIENT_RECONNECT_INTERVAL,
    CLIENT_SEND_TIMEOUT, CLIENT_STOP_JOIN_TIMEOUT,
};

#[cfg(unix)]
type ConnectionReadHalf = tokio::net::unix::OwnedReadHalf;
#[cfg(unix)]
type ConnectionWriteHalf = tokio::net::unix::OwnedWriteHalf;
#[cfg(windows)]
type ConnectionReadHalf = interprocess::local_socket::tokio::RecvHalf;
#[cfg(windows)]
type ConnectionWriteHalf = interprocess::local_socket::tokio::SendHalf;

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

#[cfg(any(test, feature = "test-hooks"))]
struct FrameProgress {
    count: AtomicUsize,
    changed: Notify,
}

#[cfg(any(test, feature = "test-hooks"))]
impl FrameProgress {
    fn record(&self) {
        self.count.fetch_add(1, Ordering::AcqRel);
        self.changed.notify_waiters();
    }
}

/// Nonblocking, defaults-free diagnostics over an existing connection queue.
/// Retains no endpoint, journal root, receiver, task or join ownership.
#[derive(Clone)]
pub struct CallosumDiagnosticEmitter {
    outbound: mpsc::Sender<CallosumEnvelope>,
    running: Arc<AtomicBool>,
    admission: Arc<AtomicBool>,
    brain_startup: Option<Arc<BrainStartupCapture>>,
}

impl CallosumDiagnosticEmitter {
    /// Queued only. Missing capacity, contention and stopped owners omit the row.
    pub fn emit(&self, tract: &str, event: &str, fields: Map<String, Value>) -> bool {
        self.emit_with_private(tract, event, fields, None)
    }

    /// The sidecar belongs only to this send's private preparation/finalization.
    pub fn emit_with_private(
        &self,
        tract: &str,
        event: &str,
        fields: Map<String, Value>,
        proof: Option<&super::PrivateProofReceive>,
    ) -> bool {
        self.emit_prepared(
            tract,
            event,
            fields,
            proof,
            #[cfg(test)]
            None,
        )
    }

    fn emit_prepared(
        &self,
        tract: &str,
        event: &str,
        fields: Map<String, Value>,
        proof: Option<&super::PrivateProofReceive>,
        #[cfg(test)] mut pause: Option<&mut dyn FnMut(u8)>,
    ) -> bool {
        let Some(envelope) = outbound_envelope(None, tract, event, fields) else {
            if let Some(capture) = &self.brain_startup {
                capture.missing_emission(tract, event);
            }
            return false;
        };
        let pending = self
            .brain_startup
            .as_ref()
            .and_then(|capture| capture.prepare(&envelope, proof));
        #[cfg(test)]
        if let Some(pause) = pause.as_mut() {
            pause(0);
        }
        let sent = self.send(envelope);
        #[cfg(test)]
        if let Some(pause) = pause.as_mut() {
            pause(1);
        }
        if let Some(pending) = pending {
            pending.finish(sent);
        }
        sent
    }

    pub fn missing_brain_identity(&self) {
        if let Some(capture) = &self.brain_startup {
            capture.missing_identity();
        }
    }

    fn send(&self, envelope: CallosumEnvelope) -> bool {
        if !self.running.load(Ordering::Acquire)
            || self
                .admission
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        struct Release<'a>(&'a AtomicBool);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _release = Release(&self.admission);
        let Ok(permit) = self.outbound.try_reserve() else {
            return false;
        };
        // Two existing eight-row renewal batches retain their original slots.
        if self.outbound.capacity() < 16 || !self.running.load(Ordering::Acquire) {
            return false;
        }
        permit.send(envelope);
        true
    }
}

fn outbound_envelope(
    defaults: Option<&Map<String, Value>>,
    tract: &str,
    event: &str,
    fields: Map<String, Value>,
) -> Option<CallosumEnvelope> {
    let mut merged = defaults.cloned().unwrap_or_default();
    merged.insert("tract".to_owned(), Value::String(tract.to_owned()));
    merged.insert("event".to_owned(), Value::String(event.to_owned()));
    merged.extend(fields);
    serde_json::from_value(Value::Object(merged)).ok()
}

/// Long-lived, reconnecting Callosum local-transport client.
pub struct CallosumSocketConnection {
    #[cfg(unix)]
    socket_path: crate::UnixEndpoint,
    #[cfg(windows)]
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
    diagnostic_admission: Arc<AtomicBool>,
    brain_startup: Option<Arc<BrainStartupCapture>>,
    delivered_generation: u64,
    delivered_epoch: u64,
    initial_counters: ConnectionCounters,
    initial_first_attempt: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    frames: Arc<FrameProgress>,
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

    /// Attach the closed startup prefix at construction, before any sink exists.
    pub fn new_with_brain_startup_capture(
        socket_path: impl AsRef<Path>,
        defaults: Map<String, Value>,
        capture: Arc<BrainStartupCapture>,
    ) -> Self {
        let mut connection = Self::new(socket_path, defaults);
        connection.brain_startup = Some(capture);
        connection
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
            #[cfg(unix)]
            socket_path: crate::UnixEndpoint::new(socket_path.as_ref()),
            #[cfg(windows)]
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
            diagnostic_admission: Arc::new(AtomicBool::new(false)),
            brain_startup: None,
            delivered_generation: 0,
            delivered_epoch: 0,
            initial_counters: ConnectionCounters::initial(),
            initial_first_attempt: true,
            #[cfg(any(test, feature = "test-hooks"))]
            frames: Arc::new(FrameProgress {
                count: AtomicUsize::new(0),
                changed: Notify::new(),
            }),
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn with_retry_source_and_initial_counters(
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
            #[cfg(any(test, feature = "test-hooks"))]
            frames: Arc::clone(&self.frames),
        })));
    }

    /// Share diagnostics with this queue without inheriting caller defaults.
    pub fn diagnostic_emitter(&self) -> CallosumDiagnosticEmitter {
        CallosumDiagnosticEmitter {
            outbound: self.outbound.clone(),
            running: Arc::clone(&self.running),
            admission: Arc::clone(&self.diagnostic_admission),
            brain_startup: self.brain_startup.clone(),
        }
    }

    /// Queue an outbound message with Python-compatible field precedence.
    pub fn emit(&self, tract: &str, event: &str, caller_fields: Map<String, Value>) -> bool {
        if !self.running.load(Ordering::Acquire) {
            if let Some(capture) = &self.brain_startup {
                capture.missing_emission(tract, event);
            }
            return false;
        }
        let Some(envelope) = outbound_envelope(Some(&self.defaults), tract, event, caller_fields)
        else {
            if let Some(capture) = &self.brain_startup {
                capture.missing_emission(tract, event);
            }
            return false;
        };
        let pending = self
            .brain_startup
            .as_ref()
            .and_then(|capture| capture.prepare(&envelope, None));
        let sent = match self.outbound.try_send(envelope) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                let _ = self
                    .outbound_saturation_drops
                    .fetch_add(1, Ordering::AcqRel);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        };
        if let Some(pending) = pending {
            pending.finish(sent);
        }
        sent
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

    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn has_pending_priority(&self) -> bool {
        self.queues.priority.pending()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub async fn wait_for_frames_processed(&self, at_least: usize) {
        timeout(std::time::Duration::from_secs(2), async {
            loop {
                if self.frames.count.load(Ordering::Acquire) >= at_least {
                    return;
                }
                self.frames.changed.notified().await;
            }
        })
        .await
        .expect("frames should be processed");
    }

    pub async fn stop(&mut self) {
        if let Some(capture) = &self.brain_startup {
            capture.close();
        }
        if !self.running.swap(false, Ordering::AcqRel) {
            return;
        }
        let _ = self.shutdown.send(true);
        let Some(mut task) = self.task.take() else {
            return;
        };
        if timeout(CLIENT_STOP_JOIN_TIMEOUT, &mut task).await.is_err() {
            log::warn!("callosum wire: connection drain continues after stop returns");
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) async fn join_terminated_for_test(&mut self) {
        assert!(
            !self.running.load(Ordering::Acquire),
            "test cleanup may only join an already-terminated connection"
        );
        let Some(mut task) = self.task.take() else {
            return;
        };
        timeout(CLIENT_STOP_JOIN_TIMEOUT, &mut task)
            .await
            .expect("terminated Callosum test connection should join within its deadline")
            .expect("terminated Callosum test connection task should not panic");
    }
}

impl Drop for CallosumSocketConnection {
    fn drop(&mut self) {
        if let Some(capture) = &self.brain_startup {
            capture.close();
        }
        self.running.store(false, Ordering::Release);
        let _ = self.shutdown.send(true);
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn diagnostic_fixture(
    capacity: usize,
) -> (CallosumDiagnosticEmitter, mpsc::Receiver<CallosumEnvelope>) {
    let (outbound, receiver) = mpsc::channel(capacity);
    (
        CallosumDiagnosticEmitter {
            outbound,
            running: Arc::new(AtomicBool::new(true)),
            admission: Arc::new(AtomicBool::new(false)),
            brain_startup: None,
        },
        receiver,
    )
}

#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn diagnostic_fixture_with_startup_capture(
    capacity: usize,
    capture: Arc<BrainStartupCapture>,
) -> (CallosumDiagnosticEmitter, mpsc::Receiver<CallosumEnvelope>) {
    let (mut emitter, receiver) = diagnostic_fixture(capacity);
    emitter.brain_startup = Some(capture);
    (emitter, receiver)
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use serde_json::json;

    fn private_fixture() -> super::super::PrivateProofReceive {
        super::super::PrivateProofReceive::encode(&json!({
            "destination":"[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff%4294967295]:65535",
            "ordinal":8,"attempted":8,"stage":"complete","category":"channel_state",
            "header_bytes":16384,"body_bytes":8388608,"declared_body_bytes":8388608,
            "branch":"buffered_before_tls_failure","saw_eof":true,"tls_failed":true,"close_observed":true
        }))
    }
    fn publication_fields() -> Map<String, Value> {
        serde_json::from_value(json!({"observation_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "caller":"generate","phase":"runtime_establish","outcome":"published",
            "revision":9007199254740991u64,"reason_code":"attestation_rejected",
            "verification_reason":"proof_http_channel_state","runtime_failure_marker":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"})).unwrap()
    }
    fn next_capture(
        capture: &BrainStartupCapture,
        cursor: &mut super::super::BrainStartupCursor,
    ) -> super::super::BrainStartupEvent {
        use std::future::Future;
        let mut future = Box::pin(capture.next(cursor));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(Some(event)) => event,
            _ => panic!("capture event expected"),
        }
    }

    #[test]
    fn private_sidecar_keeps_both_real_send_gaps_pending_and_bus_bytes_unchanged() {
        let proof = private_fixture();
        let fields = publication_fields();
        let (ordinary, mut ordinary_rx) = diagnostic_fixture(128);
        assert!(ordinary.emit("supervisor", "brain_observation", fields.clone()));
        let original =
            super::super::frame::encode_envelope(&ordinary_rx.try_recv().unwrap()).unwrap();
        let capture = BrainStartupCapture::new();
        let (emitter, mut receiver) =
            diagnostic_fixture_with_startup_capture(128, Arc::clone(&capture));
        let mut events = Vec::new();
        let mut stages = Vec::new();
        let sent = emitter.emit_prepared(
            "supervisor",
            "brain_observation",
            fields.clone(),
            Some(&proof),
            Some(&mut |stage| {
                stages.push(stage);
                let event =
                    next_capture(&capture, &mut super::super::BrainStartupCursor::default());
                let marker: Value = serde_json::from_str(&event.data).unwrap();
                assert_eq!(marker["coverage"], "pending");
                assert_eq!(marker["pending"], 1);
                events.push(json!({"name":event.name,"data":event.data}));
            }),
        );
        assert!(sent);
        assert_eq!(stages, vec![0, 1]);
        let received = receiver.try_recv().unwrap();
        assert_eq!(
            super::super::frame::encode_envelope(&received).unwrap(),
            original
        );
        assert!(!received.extra.contains_key("proof_receive"));
        assert!(receiver.try_recv().is_err());
        let mut cursor = super::super::BrainStartupCursor::default();
        let event = next_capture(&capture, &mut cursor);
        let row: Value = serde_json::from_str(&event.data).unwrap();
        let row_bytes = serde_json::to_vec(&row["row"]).unwrap();
        assert!(row_bytes.len() <= 1024);
        assert_eq!(row["row"]["proof_receive"]["ordinal"], 8);
        events.push(json!({"name":event.name,"data":event.data}));
        let event = next_capture(&capture, &mut cursor);
        let marker: Value = serde_json::from_str(&event.data).unwrap();
        assert_eq!(marker["coverage"], "complete");
        events.push(json!({"name":event.name,"data":event.data}));
        println!(
            "PROOF_CAPTURE_WIRE={}",
            json!({"events":events,"ordinary":String::from_utf8(original).unwrap(),"row_bytes":row_bytes.len()})
        );
        // Capture absence and invalid private evidence cannot change the actual send.
        let (absent, mut rx) = diagnostic_fixture(128);
        assert!(absent.emit_with_private(
            "supervisor",
            "brain_observation",
            fields.clone(),
            Some(&proof)
        ));
        assert_eq!(
            serde_json::to_value(rx.try_recv().unwrap()).unwrap(),
            serde_json::to_value(received).unwrap()
        );
        let malformed =
            super::super::PrivateProofReceive::encode(&json!({"arbitrary":"must not escape"}));
        let lost = BrainStartupCapture::new();
        let (bad, mut rx) = diagnostic_fixture_with_startup_capture(128, Arc::clone(&lost));
        assert!(bad.emit_with_private("supervisor", "brain_observation", fields, Some(&malformed)));
        assert!(rx.try_recv().is_ok());
        let event = next_capture(&lost, &mut super::super::BrainStartupCursor::default());
        assert_eq!(
            serde_json::from_str::<Value>(&event.data).unwrap()["coverage"],
            "incomplete"
        );
    }

    #[test]
    fn diagnostic_headroom_preserves_original_batches_defaults_and_drop_accounting() {
        for capture_enabled in [false, true] {
            let mut connection = CallosumSocketConnection::new(
                "unused",
                Map::from_iter([
                    (
                        "private_default".into(),
                        json!("must not reach diagnostics"),
                    ),
                    ("service".into(), json!("default")),
                ]),
            );
            if capture_enabled {
                connection.brain_startup = Some(BrainStartupCapture::new());
            }
            let emitter = connection.diagnostic_emitter();
            assert!(!emitter.emit("supervisor", "brain_observation", Map::new()));
            connection.running.store(true, Ordering::Release);
            let mut receiver = connection.outbound_rx.take().unwrap();
            let capacity = connection.outbound.max_capacity();
            for _ in 0..capacity - 17 {
                assert!(connection.emit("supervisor", "status", Map::new()));
            }
            assert!(emitter.emit("supervisor", "brain_observation", Map::new()));
            assert_eq!(connection.outbound.capacity(), 16);
            assert!(!emitter.emit("supervisor", "brain_observation", Map::new()));
            assert_eq!(connection.outbound.capacity(), 16);
            for _ in 0..16 {
                assert!(connection.emit(
                    "supervisor",
                    "request",
                    Map::from_iter([("service".into(), json!("explicit"))])
                ));
            }
            assert!(!emitter.emit("supervisor", "brain_observation", Map::new()));
            assert_eq!(
                connection.outbound_saturation_drops.load(Ordering::Acquire),
                0
            );
            assert!(!connection.emit("supervisor", "request", Map::new()));
            assert_eq!(
                connection.outbound_saturation_drops.load(Ordering::Acquire),
                1
            );
            let mut seen_diagnostic = false;
            let mut requests = 0;
            while let Ok(row) = receiver.try_recv() {
                if row.event == "brain_observation" {
                    seen_diagnostic = true;
                    assert!(row.extra.is_empty());
                } else if row.event == "request" {
                    requests += 1;
                    assert_eq!(row.extra["service"], "explicit");
                }
            }
            assert!(seen_diagnostic);
            assert_eq!(requests, 16);
            drop(receiver);
            assert!(!emitter.emit("supervisor", "brain_observation", Map::new()));
            assert!(!emitter.admission.load(Ordering::Acquire));
            drop(connection);
            assert!(!emitter.running.load(Ordering::Acquire));
            assert!(!emitter.emit("supervisor", "brain_observation", Map::new()));
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn racing_diagnostics_retain_two_original_batches() {
        let (emitter, mut receiver) = diagnostic_fixture(64);
        std::thread::scope(|threads| {
            for _ in 0..8 {
                let emitter = emitter.clone();
                threads.spawn(move || {
                    for _ in 0..64 {
                        let _ = emitter.emit("supervisor", "brain_observation", Map::new());
                    }
                });
            }
        });
        assert!(emitter.outbound.capacity() >= 16);
        for _ in 0..16 {
            assert!(
                emitter
                    .outbound
                    .try_send(CallosumEnvelope {
                        tract: "supervisor".into(),
                        event: "request".into(),
                        ts: None,
                        extra: Map::new(),
                    })
                    .is_ok()
            );
        }
        let mut originals = 0;
        while let Ok(row) = receiver.try_recv() {
            originals += usize::from(row.event == "request");
        }
        assert_eq!(originals, 16);
    }
}

struct ConnectedStream {
    reader: tokio::io::BufReader<ConnectionReadHalf>,
    writer: ConnectionWriteHalf,
}

#[derive(Clone)]
pub(super) struct ConnectionCounters {
    pub(super) generation: u64,
    pub(super) epoch: u64,
    pub(super) attempt: u64,
    pub(super) failures_since_success: u64,
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
    #[cfg(unix)]
    socket_path: crate::UnixEndpoint,
    #[cfg(windows)]
    socket_path: PathBuf,
    outbound: mpsc::Receiver<CallosumEnvelope>,
    queues: Arc<InboundQueues>,
    retry_source: Box<dyn CallosumRetrySource>,
    shutdown: watch::Receiver<bool>,
    running: Arc<AtomicBool>,
    malformed_frame_drops: Arc<AtomicU64>,
    counters: ConnectionCounters,
    first_attempt: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    frames: Arc<FrameProgress>,
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
        #[cfg(any(test, feature = "test-hooks"))]
        frames,
    } = run;
    let mut stream: Option<ConnectedStream> = None;
    let mut buffer = crate::local_inference::FrameAccum::new();
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
            match connect_stream(&socket_path).await {
                Ok((read_half, writer)) => {
                    // A canceled read may have retained bytes from the old socket.
                    buffer.clear();
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
                            #[cfg(any(test, feature = "test-hooks"))]
                            frames.record();
                            break;
                        }
                        resume_current = stream.is_some();
                    }
                    #[cfg(any(test, feature = "test-hooks"))]
                    frames.record();
                }
                Ok(ReadFrame::Whitespace) => {
                    #[cfg(any(test, feature = "test-hooks"))]
                    frames.record();
                }
                Ok(ReadFrame::PrivateRequest(_)) | Ok(ReadFrame::PrivateResponse(_)) => {
                    // Ignore private frames on general subscriber connection without gapping.
                    #[cfg(any(test, feature = "test-hooks"))]
                    frames.record();
                }
                Ok(ReadFrame::Malformed) | Ok(ReadFrame::InvalidUtf8) => {
                    let _ = malformed_frame_drops.fetch_add(1, Ordering::AcqRel);
                    if enter_gap(&queues, &mut counters, &mut gapped, CallosumGapReason::MalformedFrameDropped, 1) {
                        #[cfg(any(test, feature = "test-hooks"))]
                        frames.record();
                        break;
                    }
                    resume_current = stream.is_some();
                    #[cfg(any(test, feature = "test-hooks"))]
                    frames.record();
                }
                Ok(ReadFrame::Eof) | Ok(ReadFrame::PrivateRejected) | Err(_) => {
                    if enter_gap(&queues, &mut counters, &mut gapped, CallosumGapReason::Disconnected, 1) {
                        #[cfg(any(test, feature = "test-hooks"))]
                        frames.record();
                        break;
                    }
                    stream = None;
                    #[cfg(any(test, feature = "test-hooks"))]
                    frames.record();
                }
            },
        }
    }
    running.store(false, Ordering::Release);
}

#[cfg(unix)]
async fn connect_stream(
    endpoint: &crate::UnixEndpoint,
) -> std::io::Result<(ConnectionReadHalf, ConnectionWriteHalf)> {
    let stream = match endpoint.connect_local()? {
        Some(stream) => {
            stream.set_nonblocking(true)?;
            UnixStream::from_std(stream)?
        }
        None => UnixStream::connect(crate::endpoint_path(endpoint.path())?.as_ref()).await?,
    };
    Ok(stream.into_split())
}

#[cfg(windows)]
async fn connect_stream(
    socket_path: &Path,
) -> std::io::Result<(ConnectionReadHalf, ConnectionWriteHalf)> {
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{ConnectOptions, ToFsName};
    use interprocess::os::windows::local_socket::NamedPipe;

    let name = crate::windows::pipe_name(socket_path)?.to_fs_name::<NamedPipe>()?;
    let mut stream = ConnectOptions::new().name(name).connect_tokio().await?;
    authenticate_windows_server(&mut stream, socket_path).await?;
    Ok(stream.split())
}

#[cfg(windows)]
async fn authenticate_windows_server(
    stream: &mut interprocess::local_socket::tokio::Stream,
    socket_path: &Path,
) -> std::io::Result<()> {
    let secret = crate::windows::read_secret(socket_path)?;
    let mut greeting = [0_u8; crate::windows::PIPE_HANDSHAKE_LEN];
    tokio::time::timeout(SERVER_SEND_TIMEOUT, async {
        stream.read_exact(&mut greeting).await?;
        let proof = crate::windows::client_proof(&secret, &greeting)?;
        stream.write_all(&proof).await
    })
    .await
    .map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "Callosum handshake timed out")
    })??;
    Ok(())
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
    use super::{CallosumConnectionPhase, CallosumStoppedReason, ConnectionCounters};

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
}
