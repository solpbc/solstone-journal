// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Concrete process composition for the posture-gated SPL service.
//!
//! The generic supervisor in [`crate::run_service`] owns the C4 state
//! machine. This module gives it the production dependencies: read-only
//! journal state, the retained TLS relay client, the local private listener,
//! a Callosum output task, and operating-system shutdown signals.

use std::{
    collections::VecDeque,
    env,
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Map, Value};
use solstone_core_journal_config::{
    ConfigLoadError, DirectDoorPortError, plain_defaults, read_direct_door_port,
    read_journal_config,
};
use solstone_core_system::lifecycle::{
    HostedServiceParentRuntime, HostedServiceShutdownEvidence, ParentLossReason,
};
use tokio::{
    io::AsyncWriteExt,
    sync::{Notify, watch},
    task::JoinHandle,
    time::timeout,
};

use crate::{
    CallosumEmit, LinkServiceTokenRead, LinkStateRead, LoopbackConnect, LoopbackDialer,
    LoopbackStream, RelayClient, RelayClientConfig, RelayError, RelayServiceToken, ServiceDeps,
    ServiceError, ServicePoll, ServiceToken,
    callosum::{LoggingEmit, Verbosity},
    load_link_service_token, load_link_state, run_service,
};

const DEFAULT_RELAY_ENDPOINT: &str = "https://link.solstone.app";
const DISPATCH_READ_DEADLINE: Duration = Duration::from_secs(10);
const GLOBAL_ADMISSION_CEILING: usize = 32;
/// Bounded regular Callosum output capacity.
const CALLOSUM_QUEUE_CAPACITY: usize = 1_000;
const CALLOSUM_IO_TIMEOUT: Duration = Duration::from_secs(2);
const CALLOSUM_STOP_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

/// Class-only failures from starting or running the native SPL service.
///
/// The details of journal state, relay URLs, and credentials are intentionally
/// discarded before this reaches process output.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NativeServiceError {
    /// Tokio's production runtime could not be created.
    #[error("service runtime unavailable")]
    Runtime,
    /// The supervised service stopped with a stable service error class.
    #[error("service supervision failed")]
    Service,
    /// Parent-loss cleanup could not publish its durable handoff evidence.
    #[error("hosted parent-loss handoff failed")]
    ParentLoss,
}

impl NativeServiceError {
    /// Returns the safe process-output error class.
    #[must_use]
    pub const fn class(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Service => "supervision",
            Self::ParentLoss => "parent-loss",
        }
    }
}

/// Starts the production Tokio runtime and drives the native service topology.
///
/// This is intentionally a process boundary, not a test helper: the relay
/// listen task must be driven by a live runtime after the executable starts.
///
/// # Errors
///
/// Returns a class-only error if the runtime cannot start or the supervised
/// service exits unexpectedly.
pub fn run_native_service(
    journal_root: PathBuf,
    verbosity: Verbosity,
) -> Result<(), NativeServiceError> {
    run_native_service_with_hosted_parent(journal_root, verbosity, None)
}

/// Starts SPL with an optional birth-admitted hosted parent lifetime.
pub fn run_native_service_with_hosted_parent(
    journal_root: PathBuf,
    verbosity: Verbosity,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), NativeServiceError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("spl-service")
        .build()
        .map_err(|_| NativeServiceError::Runtime)?;
    runtime.block_on(run_native_service_async(
        journal_root,
        verbosity,
        hosted_parent,
    ))
}

async fn run_native_service_async(
    journal_root: PathBuf,
    verbosity: Verbosity,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), NativeServiceError> {
    let callosum = CallosumOutput::start(journal_root.join("health").join("callosum.sock"));
    if verbosity != Verbosity::Quiet {
        eprintln!("spl service: starting; watching link posture");
    }
    let (shutdown_send, shutdown_receive) = watch::channel(false);
    let signal_task = tokio::spawn(wait_for_shutdown_signal(shutdown_send.clone()));
    let (parent_loss_send, mut parent_loss_receive) = tokio::sync::oneshot::channel();
    let parent_task = hosted_parent.as_ref().map(|parent| {
        tokio::spawn(wait_for_hosted_parent(
            Arc::clone(parent),
            shutdown_send.clone(),
            parent_loss_send,
        ))
    });
    let mut deps = ProcessServiceDeps::new(journal_root, callosum, verbosity, shutdown_receive);

    let result = run_service(&mut deps).await.map_err(classify_service_error);
    let service_stopped = result.is_ok();
    signal_task.abort();
    let _ = signal_task.await;
    if let Some(parent_task) = parent_task {
        parent_task.abort();
        let _ = parent_task.await;
    }
    if let (Some(parent), Ok(reason)) = (hosted_parent, parent_loss_receive.try_recv()) {
        parent
            .finish_parent_loss(
                reason,
                HostedServiceShutdownEvidence {
                    // A successful service loop includes relay cleanup and
                    // the bounded Callosum-output stop.
                    listener_stopped: service_stopped,
                    service_runner_stopped: service_stopped,
                    // SPL has no distinct health artifact to withdraw.
                    operational_artifacts_cleaned: true,
                },
            )
            .map_err(|_| NativeServiceError::ParentLoss)?;
    }
    result
}

async fn wait_for_hosted_parent(
    parent: Arc<HostedServiceParentRuntime>,
    shutdown: watch::Sender<bool>,
    parent_loss: tokio::sync::oneshot::Sender<ParentLossReason>,
) {
    let reason = parent.await_parent_loss().await;
    let _ = parent_loss.send(reason);
    let _ = shutdown.send(true);
}

fn classify_service_error(_error: ServiceError) -> NativeServiceError {
    NativeServiceError::Service
}

async fn wait_for_shutdown_signal(shutdown: watch::Sender<bool>) {
    #[cfg(unix)]
    {
        let termination = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match termination {
            Ok(mut termination) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        let _ = result;
                    }
                    _ = termination.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    let _ = shutdown.send(true);
}

struct ProcessServiceDeps {
    journal_root: PathBuf,
    callosum: Arc<CallosumOutput>,
    verbosity: Verbosity,
    shutdown: watch::Receiver<bool>,
}

impl ProcessServiceDeps {
    fn new(
        journal_root: PathBuf,
        callosum: Arc<CallosumOutput>,
        verbosity: Verbosity,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            journal_root,
            callosum,
            verbosity,
            shutdown,
        }
    }

    fn load_relay_endpoint(&self) -> Result<String, ProcessStartError> {
        let environment_endpoint = env::var("SOL_LINK_RELAY_URL")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .filter(|value| !value.is_empty());
        if let Some(endpoint) = environment_endpoint {
            return Ok(endpoint);
        }

        let config = read_journal_config_map(&self.journal_root).map_err(|_| ProcessStartError)?;
        let configured_endpoint = config
            .get("link")
            .and_then(Value::as_object)
            .and_then(|link| link.get("relay_url"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_end_matches('/').to_owned());

        Ok(configured_endpoint.unwrap_or_else(|| DEFAULT_RELAY_ENDPOINT.to_owned()))
    }

    fn load_instance_id(&self) -> Result<String, ProcessStartError> {
        match load_link_state(&self.journal_root, "solstone") {
            LinkStateRead::Present(state) => Ok(state.instance_id),
            LinkStateRead::Missing | LinkStateRead::Unreadable | LinkStateRead::Malformed => {
                Err(ProcessStartError)
            }
        }
    }
}

#[derive(Debug)]
struct ProcessPostureError;

#[derive(Debug)]
struct ProcessStartError;

impl ServiceDeps for ProcessServiceDeps {
    type Client = RelayClient;
    type PostureError = ProcessPostureError;
    type StartError = ProcessStartError;
    type RunError = RelayError;

    fn read_posture(&mut self) -> Result<String, Self::PostureError> {
        let config =
            read_journal_config_map(&self.journal_root).map_err(|_| ProcessPostureError)?;
        let posture = config
            .get("link")
            .and_then(Value::as_object)
            .and_then(|link| link.get("posture"))
            .and_then(Value::as_str);
        Ok(if posture == Some("spl") {
            "spl".to_owned()
        } else {
            "direct".to_owned()
        })
    }

    fn load_service_token(&mut self) -> Option<RelayServiceToken> {
        match load_link_service_token(&self.journal_root) {
            LinkServiceTokenRead::Present(token) => {
                Some(RelayServiceToken::new(token.as_str().to_owned()))
            }
            LinkServiceTokenRead::Missing
            | LinkServiceTokenRead::Unreadable
            | LinkServiceTokenRead::Malformed => None,
        }
    }

    fn start_relay(
        &mut self,
        token: RelayServiceToken,
    ) -> Result<crate::StartedRelay<Self::Client, Self::RunError>, Self::StartError> {
        let instance_id = self.load_instance_id()?;
        let relay_endpoint = self.load_relay_endpoint()?;
        let client = RelayClient::new(
            RelayClientConfig {
                instance_id,
                relay_endpoint,
                service_token: ServiceToken::new(token.as_str().to_owned()),
                dispatch_read_deadline: DISPATCH_READ_DEADLINE,
                ping_interval: crate::LISTEN_PING_INTERVAL,
                ping_ack_timeout: crate::LISTEN_PING_ACK_TIMEOUT,
                ack_stability_window: crate::LISTEN_ACK_STABILITY_WINDOW,
                global_admission_ceiling: GLOBAL_ADMISSION_CEILING,
            },
            Arc::new(LoggingEmit::new(
                Arc::clone(&self.callosum) as Arc<dyn CallosumEmit>,
                self.verbosity,
            )) as Arc<dyn CallosumEmit>,
            Arc::new(local_loopback_dialer(&self.journal_root).map_err(|_| ProcessStartError)?),
        );
        let running_client = client.clone();
        let run_task = tokio::spawn(async move { running_client.run().await });
        Ok((client, run_task))
    }

    fn missing_service_token(&mut self) {
        eprintln!("spl service: service token missing; staying idle");
    }

    fn wait_for_poll(&mut self, interval: Duration) -> impl Future<Output = ServicePoll> + Send {
        let mut shutdown = self.shutdown.clone();
        async move {
            if *shutdown.borrow() {
                return ServicePoll::Shutdown;
            }
            tokio::select! {
                _ = tokio::time::sleep(interval) => ServicePoll::Elapsed,
                changed = shutdown.changed() => {
                    let _ = changed;
                    ServicePoll::Shutdown
                }
            }
        }
    }

    fn callosum_stop(&mut self) -> impl Future<Output = ()> + Send {
        let callosum = Arc::clone(&self.callosum);
        async move {
            callosum.stop().await;
        }
    }
}

struct LocalLoopbackDialer {
    port: u16,
}

impl LoopbackDialer for LocalLoopbackDialer {
    fn connect(&self) -> LoopbackConnect {
        let port = self.port;
        Box::pin(async move {
            let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
            Ok(Box::new(stream) as Box<dyn LoopbackStream>)
        })
    }
}

fn read_journal_config_map(journal_root: &Path) -> Result<Map<String, Value>, ConfigLoadError> {
    let read = read_journal_config(journal_root)?;
    Ok(read.config.unwrap_or_else(plain_defaults))
}

fn local_loopback_dialer(journal_root: &Path) -> Result<LocalLoopbackDialer, DirectDoorPortError> {
    Ok(LocalLoopbackDialer {
        port: read_direct_door_port(journal_root)?,
    })
}

/// Nonblocking Callosum output task.
struct CallosumOutput {
    queue: Arc<Mutex<CallosumQueue>>,
    lifecycle_notify: Arc<Notify>,
    dropped_regular_events: AtomicU64,
    dropped_terminal_events: AtomicU64,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl CallosumOutput {
    fn start(socket_path: PathBuf) -> Arc<Self> {
        Self::start_with_gate(socket_path, None)
    }

    fn start_with_gate(socket_path: PathBuf, start_gate: Option<Arc<Notify>>) -> Arc<Self> {
        let queue = Arc::new(Mutex::new(CallosumQueue::default()));
        let lifecycle_notify = Arc::new(Notify::new());
        let task = tokio::spawn(run_callosum_output(
            socket_path,
            Arc::clone(&queue),
            Arc::clone(&lifecycle_notify),
            start_gate,
        ));
        Arc::new(Self {
            queue,
            lifecycle_notify,
            dropped_regular_events: AtomicU64::new(0),
            dropped_terminal_events: AtomicU64::new(0),
            task: Mutex::new(Some(task)),
        })
    }

    #[cfg(test)]
    fn inactive() -> Arc<Self> {
        let queue = Arc::new(Mutex::new(CallosumQueue::closed()));
        Arc::new(Self {
            queue,
            lifecycle_notify: Arc::new(Notify::new()),
            dropped_regular_events: AtomicU64::new(0),
            dropped_terminal_events: AtomicU64::new(0),
            task: Mutex::new(None),
        })
    }

    #[cfg(any(test, feature = "test-hooks"))]
    fn paused(socket_path: PathBuf) -> (Arc<Self>, Arc<Notify>) {
        let gate = Arc::new(Notify::new());
        let output = Self::start_with_gate(socket_path, Some(Arc::clone(&gate)));
        (output, gate)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    fn dropped_regular_events(&self) -> u64 {
        self.dropped_regular_events.load(Ordering::Acquire)
    }

    /// Stops the output task after draining or aborting the writer.
    async fn stop(&self) {
        match self.queue.lock() {
            Ok(mut queue) => queue.close(),
            Err(poisoned) => poisoned.into_inner().close(),
        }
        self.lifecycle_notify.notify_one();

        let task = match self.task.lock() {
            Ok(mut task) => task.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(mut task) = task
            && timeout(CALLOSUM_STOP_DRAIN_TIMEOUT, &mut task)
                .await
                .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

/// Feature-gated driver for exercising the real Callosum output lifecycle.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub struct CallosumTestDriver {
    output: Arc<CallosumOutput>,
    start_gate: Arc<Notify>,
}

#[cfg(feature = "test-hooks")]
impl CallosumTestDriver {
    /// Creates a real output task held immediately before its Unix-socket connect.
    pub fn paused(socket_path: PathBuf) -> Self {
        let (output, start_gate) = CallosumOutput::paused(socket_path);
        Self { output, start_gate }
    }

    /// Fills the ordinary lane and returns its fixed total-frame capacity.
    pub fn saturate_regular_output(&self) -> usize {
        let payload = serde_json::json!({"state": "connected", "padding": "x".repeat(4096)});
        for _ in 0..=CALLOSUM_QUEUE_CAPACITY {
            self.output.emit("health", payload.clone());
        }
        CALLOSUM_QUEUE_CAPACITY
    }

    /// Reports whether saturation dropped at least one ordinary frame.
    pub fn regular_output_saturated(&self) -> bool {
        self.output.dropped_regular_events() > 0
    }

    /// Releases the held output task to connect and drain.
    pub fn start(&self) {
        self.start_gate.notify_one();
    }

    /// Stops the real output task after its bounded drain.
    pub async fn stop(&self) {
        self.output.stop().await;
    }
}

#[cfg(feature = "test-hooks")]
impl CallosumEmit for CallosumTestDriver {
    fn emit(&self, event: &'static str, payload: Value) {
        self.output.emit(event, payload);
    }
}

impl CallosumEmit for CallosumOutput {
    fn emit(&self, event: &'static str, payload: Value) {
        let Some(line) = callosum_line(event, payload) else {
            return;
        };
        let insertion = match self.queue.lock() {
            Ok(mut queue) => queue.push(event, line),
            Err(poisoned) => poisoned.into_inner().push(event, line),
        };
        match insertion {
            QueueInsertion::Queued(evictions) => {
                self.record_evictions(evictions);
                self.lifecycle_notify.notify_one();
            }
            QueueInsertion::DroppedRegular
                if self.dropped_regular_events.fetch_add(1, Ordering::AcqRel) == 0 =>
            {
                eprintln!("spl service: Callosum output saturated; dropping non-lifecycle events");
            }
            QueueInsertion::Pending(evictions) => self.record_evictions(evictions),
            QueueInsertion::DroppedRegular | QueueInsertion::Closed => {}
        }
    }
}

impl CallosumOutput {
    fn record_evictions(&self, evictions: QueueEvictions) {
        if evictions.regular > 0
            && self
                .dropped_regular_events
                .fetch_add(evictions.regular, Ordering::AcqRel)
                == 0
        {
            eprintln!("spl service: Callosum output saturated; dropping non-lifecycle events");
        }
        if evictions.terminal > 0
            && self
                .dropped_terminal_events
                .fetch_add(evictions.terminal, Ordering::AcqRel)
                == 0
        {
            eprintln!(
                "spl service: Callosum terminal queue saturated; evicting oldest terminal telemetry"
            );
        }
    }
}

/// Maximum unmatched terminal notices retained while waiting for health.
///
/// Correct relay code calls health immediately after every terminal event; the
/// cap confines malformed or concurrent sequences that do not.
const PENDING_TERMINAL_CAPACITY: usize = 32;

/// Insertion accounting for the nonblocking Callosum output queue.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct QueueEvictions {
    regular: u64,
    terminal: u64,
}

/// Result from inserting one Callosum event without ever awaiting output I/O.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueueInsertion {
    /// A normal event or a completed terminal pair is ready for the writer.
    Queued(QueueEvictions),
    /// The first half of a terminal pair is retained until its following health.
    Pending(QueueEvictions),
    /// The bounded queue refused only ordinary telemetry.
    DroppedRegular,
    /// Service shutdown has already detached Callosum output.
    Closed,
}

/// A bounded nonblocking output queue with an ordered terminal lane.
///
/// Python's Callosum writer drops uniformly when it is backed up. Rust keeps a
/// priority lane for disconnect-health and tunnel-close-health pairs. A newer
/// terminal pair evicts ordinary telemetry first; when none remains, it
/// evicts the oldest terminal telemetry as a whole pair. The output path never
/// blocks a tunnel finally or shutdown. Distinct tunnel-close IDs remain
/// distinct batches and drain ahead of normal telemetry.
#[derive(Default)]
struct CallosumQueue {
    regular: VecDeque<Vec<u8>>,
    lifecycle: VecDeque<VecDeque<Vec<u8>>>,
    pending_terminals: VecDeque<Vec<u8>>,
    closed: bool,
}

impl CallosumQueue {
    #[cfg(test)]
    fn closed() -> Self {
        Self {
            closed: true,
            ..Self::default()
        }
    }

    fn close(&mut self) {
        self.closed = true;
    }

    fn push(&mut self, event: &'static str, line: Vec<u8>) -> QueueInsertion {
        if self.closed {
            return QueueInsertion::Closed;
        }
        match event {
            "disconnect" | "tunnel_close" => self.push_terminal(line),
            "health" if !self.pending_terminals.is_empty() => {
                let Some(terminal) = self.pending_terminals.pop_front() else {
                    return QueueInsertion::DroppedRegular;
                };
                self.push_lifecycle_pair(terminal, line)
            }
            _ => self.push_regular(line),
        }
    }

    fn push_terminal(&mut self, terminal: Vec<u8>) -> QueueInsertion {
        let mut evictions = self.make_room_for_terminal(1);
        if self.pending_terminals.len() >= PENDING_TERMINAL_CAPACITY {
            let _ = self.pending_terminals.pop_front();
            evictions.terminal = evictions.terminal.saturating_add(1);
        }
        self.pending_terminals.push_back(terminal);
        QueueInsertion::Pending(evictions)
    }

    fn push_regular(&mut self, line: Vec<u8>) -> QueueInsertion {
        if self.queued_line_count() >= CALLOSUM_QUEUE_CAPACITY {
            QueueInsertion::DroppedRegular
        } else {
            self.regular.push_back(line);
            QueueInsertion::Queued(QueueEvictions::default())
        }
    }

    fn push_lifecycle_pair(&mut self, terminal: Vec<u8>, health: Vec<u8>) -> QueueInsertion {
        // Pop removed the terminal's accounted slot, so completing the pair
        // needs two slots to reinsert the terminal and its following health.
        let evictions = self.make_room_for_terminal(2);
        self.lifecycle.push_back(VecDeque::from([terminal, health]));
        QueueInsertion::Queued(evictions)
    }

    fn make_room_for_terminal(&mut self, additional_lines: usize) -> QueueEvictions {
        let mut evictions = QueueEvictions::default();
        while self.queued_line_count().saturating_add(additional_lines) > CALLOSUM_QUEUE_CAPACITY {
            if self.regular.pop_front().is_some() {
                evictions.regular = evictions.regular.saturating_add(1);
                continue;
            }
            if self.lifecycle.pop_front().is_some() {
                evictions.terminal = evictions.terminal.saturating_add(1);
                continue;
            }
            if self.pending_terminals.pop_front().is_some() {
                evictions.terminal = evictions.terminal.saturating_add(1);
                continue;
            }
            break;
        }
        evictions
    }

    fn pop_next(&mut self) -> Option<Vec<u8>> {
        if let Some(pair) = self.lifecycle.front_mut() {
            let line = pair.pop_front();
            if pair.is_empty() {
                self.lifecycle.pop_front();
            }
            return line;
        }
        self.regular.pop_front()
    }

    fn queued_line_count(&self) -> usize {
        self.regular
            .len()
            .saturating_add(self.lifecycle.iter().map(VecDeque::len).sum::<usize>())
            .saturating_add(self.pending_terminals.len())
    }
}

fn callosum_line(event: &'static str, payload: Value) -> Option<Vec<u8>> {
    let Value::Object(fields) = payload else {
        return None;
    };
    let mut message = Map::new();
    message.insert("tract".to_owned(), Value::String("link".to_owned()));
    message.insert("event".to_owned(), Value::String(event.to_owned()));
    message.extend(fields);
    let mut line = serde_json::to_vec(&Value::Object(message)).ok()?;
    line.push(b'\n');
    Some(line)
}

#[cfg(unix)]
async fn run_callosum_output(
    socket_path: PathBuf,
    queue: Arc<Mutex<CallosumQueue>>,
    lifecycle_notify: Arc<Notify>,
    start_gate: Option<Arc<Notify>>,
) {
    if let Some(start_gate) = start_gate {
        start_gate.notified().await;
    }
    let mut connection: Option<tokio::net::UnixStream> = None;
    loop {
        let notification = lifecycle_notify.notified();
        let next = match queue.lock() {
            Ok(mut queue) => (queue.pop_next(), queue.closed),
            Err(poisoned) => {
                let mut queue = poisoned.into_inner();
                (queue.pop_next(), queue.closed)
            }
        };
        match next {
            (Some(line), _) => {
                drop(notification);
                write_callosum_line(&socket_path, &mut connection, line).await;
                continue;
            }
            (None, true) => break,
            (None, false) => notification.await,
        }
    }
}

#[cfg(unix)]
async fn write_callosum_line(
    socket_path: &Path,
    connection: &mut Option<tokio::net::UnixStream>,
    line: Vec<u8>,
) {
    if connection.is_none() {
        *connection = match timeout(
            CALLOSUM_IO_TIMEOUT,
            tokio::net::UnixStream::connect(socket_path),
        )
        .await
        {
            Ok(Ok(connection)) => Some(connection),
            Ok(Err(_)) | Err(_) => None,
        };
    }
    if let Some(stream) = connection.as_mut() {
        let written = timeout(CALLOSUM_IO_TIMEOUT, stream.write_all(&line)).await;
        if !matches!(written, Ok(Ok(()))) {
            *connection = None;
        }
    }
}

#[cfg(not(unix))]
async fn run_callosum_output(
    _socket_path: PathBuf,
    queue: Arc<Mutex<CallosumQueue>>,
    lifecycle_notify: Arc<Notify>,
    _start_gate: Option<Arc<Notify>>,
) {
    loop {
        let notification = lifecycle_notify.notified();
        let finished = match queue.lock() {
            Ok(mut queue) => {
                let _ = queue.pop_next();
                queue.closed && queue.queued_line_count() == 0
            }
            Err(poisoned) => {
                let mut queue = poisoned.into_inner();
                let _ = queue.pop_next();
                queue.closed && queue.queued_line_count() == 0
            }
        };
        if finished {
            break;
        }
        notification.await;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    use serde_json::{Value, json};
    use solstone_core_sol_link::establish;
    use tokio::time::timeout;

    use super::{
        CALLOSUM_QUEUE_CAPACITY, CallosumOutput, CallosumQueue, DEFAULT_RELAY_ENDPOINT,
        PENDING_TERMINAL_CAPACITY, ProcessServiceDeps, QueueEvictions, QueueInsertion, ServiceDeps,
        callosum_line,
    };
    use crate::CallosumEmit;

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

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn callosum_output_uses_the_link_tract_and_newline_json_protocol() -> Result<(), String> {
        let line = callosum_line("health", json!({"state": "connected"}))
            .ok_or_else(|| "could not serialize callosum event".to_owned())?;
        assert_eq!(
            line,
            b"{\"tract\":\"link\",\"event\":\"health\",\"state\":\"connected\"}\n"
        );
        Ok(())
    }

    #[test]
    fn unpaired_terminal_notices_are_bounded_and_newest_wins() -> Result<(), String> {
        let mut queue = CallosumQueue::default();
        for ordinal in 0..PENDING_TERMINAL_CAPACITY.saturating_mul(2) {
            let line = callosum_line(
                "tunnel_close",
                json!({"tunnel_id": format!("unpaired-{ordinal}")}),
            )
            .ok_or_else(|| "could not serialize unmatched terminal event".to_owned())?;
            let _ = queue.push("tunnel_close", line);
        }

        assert_eq!(queue.pending_terminals.len(), PENDING_TERMINAL_CAPACITY);
        assert_eq!(queue.queued_line_count(), PENDING_TERMINAL_CAPACITY);
        let first_pending: Value = serde_json::from_slice(
            queue
                .pending_terminals
                .front()
                .ok_or_else(|| "unmatched terminal queue was unexpectedly empty".to_owned())?,
        )
        .map_err(|_| "unmatched terminal queue was not JSONL".to_owned())?;
        assert_eq!(
            first_pending["tunnel_id"],
            format!("unpaired-{PENDING_TERMINAL_CAPACITY}")
        );

        let health = callosum_line("health", json!({"state": "reconnecting"}))
            .ok_or_else(|| "could not serialize terminal health".to_owned())?;
        assert_eq!(
            queue.push("health", health),
            QueueInsertion::Queued(QueueEvictions::default())
        );
        assert!(queue.queued_line_count() <= CALLOSUM_QUEUE_CAPACITY);
        let terminal: Value =
            serde_json::from_slice(&queue.pop_next().ok_or_else(|| {
                "completed terminal pair did not enter priority output".to_owned()
            })?)
            .map_err(|_| "priority terminal event was not JSONL".to_owned())?;
        let health: Value =
            serde_json::from_slice(&queue.pop_next().ok_or_else(|| {
                "completed terminal health did not enter priority output".to_owned()
            })?)
            .map_err(|_| "priority terminal health was not JSONL".to_owned())?;
        assert_eq!(
            terminal["tunnel_id"],
            format!("unpaired-{PENDING_TERMINAL_CAPACITY}")
        );
        assert_eq!(health["event"], "health");
        Ok(())
    }

    #[test]
    fn all_terminal_capacity_retains_the_newest_pair_in_order() -> Result<(), String> {
        let mut queue = CallosumQueue::default();
        for ordinal in 0..(CALLOSUM_QUEUE_CAPACITY / 2) {
            let terminal = callosum_line(
                "tunnel_close",
                json!({"tunnel_id": format!("terminal-{ordinal}")}),
            )
            .ok_or_else(|| "could not serialize terminal event".to_owned())?;
            let health = callosum_line("health", json!({"state": "connected"}))
                .ok_or_else(|| "could not serialize terminal health".to_owned())?;
            assert_eq!(
                queue.push("tunnel_close", terminal),
                QueueInsertion::Pending(QueueEvictions::default())
            );
            assert_eq!(
                queue.push("health", health),
                QueueInsertion::Queued(QueueEvictions::default())
            );
        }
        assert_eq!(queue.queued_line_count(), CALLOSUM_QUEUE_CAPACITY);

        let newest = callosum_line("tunnel_close", json!({"tunnel_id": "terminal-newest"}))
            .ok_or_else(|| "could not serialize newest terminal event".to_owned())?;
        assert_eq!(
            queue.push("tunnel_close", newest),
            QueueInsertion::Pending(QueueEvictions {
                regular: 0,
                terminal: 1,
            })
        );
        let newest_health = callosum_line("health", json!({"state": "reconnecting"}))
            .ok_or_else(|| "could not serialize newest terminal health".to_owned())?;
        assert_eq!(
            queue.push("health", newest_health),
            QueueInsertion::Queued(QueueEvictions::default())
        );
        assert_eq!(queue.queued_line_count(), CALLOSUM_QUEUE_CAPACITY);

        let mut lines = Vec::with_capacity(CALLOSUM_QUEUE_CAPACITY);
        while let Some(line) = queue.pop_next() {
            lines.push(
                serde_json::from_slice::<Value>(&line)
                    .map_err(|_| "terminal queue was not JSONL".to_owned())?,
            );
        }
        assert_eq!(lines.len(), CALLOSUM_QUEUE_CAPACITY);
        assert_eq!(lines[0]["tunnel_id"], "terminal-1");
        assert_eq!(
            lines[CALLOSUM_QUEUE_CAPACITY - 2]["tunnel_id"],
            "terminal-newest"
        );
        assert_eq!(lines[CALLOSUM_QUEUE_CAPACITY - 1]["state"], "reconnecting");
        for pair in lines.chunks_exact(2) {
            assert_eq!(pair[0]["event"], "tunnel_close");
            assert_eq!(pair[1]["event"], "health");
        }
        Ok(())
    }

    #[test]
    fn saturated_regular_queue_still_retains_the_final_lifecycle_tail() -> Result<(), String> {
        let mut queue = CallosumQueue::default();
        let regular = callosum_line(
            "health",
            json!({"state": "connected", "padding": "x".repeat(4096)}),
        )
        .ok_or_else(|| "could not serialize regular health".to_owned())?;
        for _ in 0..CALLOSUM_QUEUE_CAPACITY {
            assert_eq!(
                queue.push("health", regular.clone()),
                QueueInsertion::Queued(QueueEvictions::default())
            );
        }
        assert_eq!(
            queue.push("health", regular),
            QueueInsertion::DroppedRegular
        );
        let disconnect = callosum_line("disconnect", json!({}))
            .ok_or_else(|| "could not serialize disconnect".to_owned())?;
        assert!(matches!(
            queue.push("disconnect", disconnect),
            QueueInsertion::Pending(_)
        ));
        let health = callosum_line("health", json!({"state": "reconnecting"}))
            .ok_or_else(|| "could not serialize reconnecting health".to_owned())?;
        assert!(matches!(
            queue.push("health", health),
            QueueInsertion::Queued(_)
        ));

        let first: Value = serde_json::from_slice(
            &queue
                .pop_next()
                .ok_or_else(|| "lifecycle tail missing disconnect".to_owned())?,
        )
        .map_err(|_| "disconnect line was not JSONL".to_owned())?;
        let second: Value = serde_json::from_slice(
            &queue
                .pop_next()
                .ok_or_else(|| "lifecycle tail missing health".to_owned())?,
        )
        .map_err(|_| "health line was not JSONL".to_owned())?;
        assert_eq!(first["event"], "disconnect");
        assert_eq!(second["event"], "health");
        assert_eq!(second["state"], "reconnecting");
        Ok(())
    }

    #[test]
    fn saturated_regular_queue_preserves_each_tunnel_close_id_and_health() -> Result<(), String> {
        let mut queue = CallosumQueue::default();
        let regular = callosum_line(
            "health",
            json!({"state": "connected", "padding": "x".repeat(4096)}),
        )
        .ok_or_else(|| "could not serialize regular health".to_owned())?;
        for _ in 0..CALLOSUM_QUEUE_CAPACITY {
            assert_eq!(
                queue.push("health", regular.clone()),
                QueueInsertion::Queued(QueueEvictions::default())
            );
        }
        assert_eq!(
            queue.push("health", regular),
            QueueInsertion::DroppedRegular
        );

        let close_seven = callosum_line("tunnel_close", json!({"tunnel_id": "terminal-tunnel-7"}))
            .ok_or_else(|| "could not serialize first tunnel close".to_owned())?;
        let health_seven = callosum_line("health", json!({"state": "connected"}))
            .ok_or_else(|| "could not serialize first tunnel health".to_owned())?;
        let close_eight = callosum_line("tunnel_close", json!({"tunnel_id": "terminal-tunnel-8"}))
            .ok_or_else(|| "could not serialize second tunnel close".to_owned())?;
        let health_eight = callosum_line("health", json!({"state": "reconnecting"}))
            .ok_or_else(|| "could not serialize second tunnel health".to_owned())?;
        assert!(matches!(
            queue.push("tunnel_close", close_seven),
            QueueInsertion::Pending(_)
        ));
        assert!(matches!(
            queue.push("health", health_seven),
            QueueInsertion::Queued(_)
        ));
        assert!(matches!(
            queue.push("tunnel_close", close_eight),
            QueueInsertion::Pending(_)
        ));
        assert!(matches!(
            queue.push("health", health_eight),
            QueueInsertion::Queued(_)
        ));

        let mut lines = Vec::new();
        for _ in 0..4 {
            lines.push(
                serde_json::from_slice::<Value>(
                    &queue
                        .pop_next()
                        .ok_or_else(|| "saturated queue lost a tunnel-close pair".to_owned())?,
                )
                .map_err(|_| "tunnel-close pair was not JSONL".to_owned())?,
            );
        }
        assert_eq!(lines[0]["event"], "tunnel_close");
        assert_eq!(lines[0]["tunnel_id"], "terminal-tunnel-7");
        assert_eq!(lines[1]["event"], "health");
        assert_eq!(lines[1]["state"], "connected");
        assert_eq!(lines[2]["event"], "tunnel_close");
        assert_eq!(lines[2]["tunnel_id"], "terminal-tunnel-8");
        assert_eq!(lines[3]["event"], "health");
        assert_eq!(lines[3]["state"], "reconnecting");
        Ok(())
    }

    #[tokio::test]
    async fn wedged_callosum_never_blocks_terminal_emit_or_output_shutdown() -> Result<(), String> {
        let journal = TempJournal::new()?;
        let socket_path = journal.path().join("health").join("callosum.sock");
        let (output, _start_gate) = CallosumOutput::paused(socket_path);
        let regular_payload = json!({"state": "connected", "padding": "x".repeat(4096)});
        for _ in 0..=super::CALLOSUM_QUEUE_CAPACITY {
            output.emit("health", regular_payload.clone());
        }
        if output.dropped_regular_events() == 0 {
            return Err("regular Callosum output queue did not saturate".to_owned());
        }

        let started = Instant::now();
        output.emit("tunnel_close", json!({"tunnel_id": "wedged-tunnel"}));
        output.emit("health", json!({"state": "reconnecting"}));
        if started.elapsed() > Duration::from_millis(50) {
            return Err("wedged Callosum blocked terminal tunnel cleanup".to_owned());
        }

        timeout(Duration::from_secs(1), output.stop())
            .await
            .map_err(|_| "wedged Callosum blocked output shutdown".to_owned())?;
        Ok(())
    }

    #[test]
    fn relay_endpoint_matches_the_config_and_default_precedence() -> Result<(), String> {
        let journal = TempJournal::new()?;
        journal.write(
            "config/journal.json",
            r#"{"link":{"relay_url":"https://configured.example/"}}"#,
        )?;
        let deps = test_deps(journal.path());
        assert_eq!(
            deps.load_relay_endpoint()
                .map_err(|_| "configured endpoint failed")?,
            "https://configured.example"
        );

        let missing = TempJournal::new()?;
        let defaults = test_deps(missing.path());
        assert_eq!(
            defaults
                .load_relay_endpoint()
                .map_err(|_| "default endpoint failed")?,
            DEFAULT_RELAY_ENDPOINT
        );

        let corrupt = TempJournal::new()?;
        corrupt.write("config/journal.json", r#"{"link":NaN}"#)?;
        let mut corrupt_deps = test_deps(corrupt.path());
        assert!(corrupt_deps.load_relay_endpoint().is_err());
        assert!(corrupt_deps.read_posture().is_err());
        Ok(())
    }

    #[test]
    fn process_service_loads_instance_id_from_native_committed_state() -> Result<(), String> {
        let journal = TempJournal::new()?;
        establish::current_candidate(journal.path()).map_err(|error| error.to_string())?;
        let expected = establish::lock_in(journal.path(), Some("Native Service"))
            .map_err(|error| error.to_string())?;
        assert!(!journal.path().join("link/state.json").exists());

        let actual = test_deps(journal.path())
            .load_instance_id()
            .map_err(|_| "native committed instance ID did not load".to_owned())?;

        assert_eq!(actual, expected.instance_id);
        Ok(())
    }

    #[test]
    fn custom_direct_port_constructs_a_dialer_for_that_port() -> Result<(), String> {
        let journal = TempJournal::new()?;
        journal.write("config/journal.json", r#"{"pairing":{"direct_port":9000}}"#)?;
        let dialer =
            super::local_loopback_dialer(journal.path()).map_err(|error| error.to_string())?;
        assert_eq!(dialer.port, 9000);
        Ok(())
    }

    #[test]
    fn omitted_direct_port_constructs_a_dialer_for_the_default() -> Result<(), String> {
        let journal = TempJournal::new()?;
        journal.write("config/journal.json", "{}")?;
        let dialer =
            super::local_loopback_dialer(journal.path()).map_err(|error| error.to_string())?;
        assert_eq!(
            dialer.port,
            solstone_core_journal_config::DEFAULT_DIRECT_DOOR_PORT
        );
        Ok(())
    }

    fn test_deps(root: &Path) -> ProcessServiceDeps {
        let (shutdown_send, shutdown_receive) = tokio::sync::watch::channel(false);
        drop(shutdown_send);
        let callosum = super::CallosumOutput::inactive();
        ProcessServiceDeps::new(
            root.to_path_buf(),
            callosum,
            crate::callosum::Verbosity::Quiet,
            shutdown_receive,
        )
    }
}
