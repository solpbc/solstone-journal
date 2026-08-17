// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The relay listen client and its TLS-only tunnel dispatcher.

use std::{
    collections::HashSet,
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use thiserror::Error;
use tokio::{
    task::JoinSet,
    time::{Instant, sleep, sleep_until},
};

/// The production interval between acknowledged relay-listener Pings.
pub(crate) const LISTEN_PING_INTERVAL: Duration = Duration::from_secs(30);
/// The absolute deadline for one relay-listener Ping acknowledgement.
pub(crate) const LISTEN_PING_ACK_TIMEOUT: Duration = Duration::from_secs(10);
/// The continuously acknowledged duration required before resetting reconnect backoff.
pub(crate) const LISTEN_ACK_STABILITY_WINDOW: Duration = Duration::from_secs(60);

use crate::relay_websocket::ListenEvent;
use crate::{
    BufferedWsReader, CallosumEmit, ListenControl, RelayAdmissionGate, RelayHealth,
    RelayHealthState, RelayTunnelFailure, RelayTunnelFailureSignal, RelayWebSocket,
    RelayWebSocketError, RelayWebSocketReader, RelayWebSocketWriter, ServiceToken, TunnelRoute,
    WsByteSink, WsByteSource, WsClosed, classify_relay_tunnel_failure, pipe_tunnel,
    relay_tunnel_url, route_tunnel_prefix, schedule_reconnect,
};

/// A stream accepted by the local private listener.
pub trait LoopbackStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}

impl<T> LoopbackStream for T where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}

/// The object-safe future returned by a local-loopback dialer.
pub type LoopbackConnect =
    Pin<Box<dyn Future<Output = io::Result<Box<dyn LoopbackStream>>> + Send>>;

/// The concrete seam for the local private listener.
pub trait LoopbackDialer: Send + Sync {
    /// Opens a loopback stream for one TLS tunnel.
    fn connect(&self) -> LoopbackConnect;
}

/// Object-safe listen-socket reader used by [`RelayClient`] and test doubles.
pub(crate) trait ListenReader: Send + 'static {
    /// Reads one listen-channel event, retaining raw Pong payloads.
    fn next_listen_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<ListenEvent, WsClosed>> + Send + '_>>;

    /// Reads the next data-bearing WebSocket message, skipping control frames.
    fn next_message(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Bytes>, WsClosed>> + Send + '_>>;
}

/// Object-safe listen-socket writer used by [`RelayClient`] and test doubles.
pub(crate) trait ListenWriter: Send + 'static {
    /// Sends a WebSocket Ping carrying the acknowledgement nonce.
    fn send_ping(
        &mut self,
        payload: Bytes,
    ) -> Pin<Box<dyn Future<Output = Result<(), WsClosed>> + Send + '_>>;

    /// Sends one data-bearing WebSocket message.
    fn send(
        &mut self,
        bytes: Bytes,
    ) -> Pin<Box<dyn Future<Output = Result<(), WsClosed>> + Send + '_>>;

    /// Closes the write half.
    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<(), WsClosed>> + Send + '_>>;
}

/// The swappable seam for opening a relay listen or tunnel WebSocket.
pub(crate) trait RelayConnector: Send + Sync {
    /// Connects one authenticated relay WebSocket and splits it.
    fn connect(
        &self,
        url: &str,
        token: &ServiceToken,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        (Box<dyn ListenReader>, Box<dyn ListenWriter>),
                        RelayWebSocketError,
                    >,
                > + Send,
        >,
    >;
}

struct DefaultRelayConnector;

impl RelayConnector for DefaultRelayConnector {
    fn connect(
        &self,
        url: &str,
        token: &ServiceToken,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        (Box<dyn ListenReader>, Box<dyn ListenWriter>),
                        RelayWebSocketError,
                    >,
                > + Send,
        >,
    > {
        let url = url.to_owned();
        let token = token.clone();
        Box::pin(async move {
            let websocket = RelayWebSocket::connect(&url, &token).await?;
            let (reader, writer) = websocket.split();
            Ok((
                Box::new(reader) as Box<dyn ListenReader>,
                Box::new(writer) as Box<dyn ListenWriter>,
            ))
        })
    }
}

impl ListenReader for RelayWebSocketReader {
    fn next_listen_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<ListenEvent, WsClosed>> + Send + '_>> {
        Box::pin(async move { RelayWebSocketReader::next_listen_event(self).await })
    }

    fn next_message(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Bytes>, WsClosed>> + Send + '_>> {
        Box::pin(async move { WsByteSource::next_message(self).await })
    }
}

impl ListenWriter for RelayWebSocketWriter {
    fn send_ping(
        &mut self,
        payload: Bytes,
    ) -> Pin<Box<dyn Future<Output = Result<(), WsClosed>> + Send + '_>> {
        Box::pin(async move { RelayWebSocketWriter::send_ping(self, payload).await })
    }

    fn send(
        &mut self,
        bytes: Bytes,
    ) -> Pin<Box<dyn Future<Output = Result<(), WsClosed>> + Send + '_>> {
        Box::pin(async move { WsByteSink::send(self, bytes).await })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<(), WsClosed>> + Send + '_>> {
        Box::pin(async move { WsByteSink::close(self).await })
    }
}

impl<'a> WsByteSource for Box<dyn ListenReader + 'a> {
    fn next_message(&mut self) -> impl Future<Output = Result<Option<Bytes>, WsClosed>> + Send {
        ListenReader::next_message(self.as_mut())
    }
}

impl<'a> WsByteSink for Box<dyn ListenWriter + 'a> {
    fn send(&mut self, bytes: Bytes) -> impl Future<Output = Result<(), WsClosed>> + Send {
        ListenWriter::send(self.as_mut(), bytes)
    }

    fn close(&mut self) -> impl Future<Output = Result<(), WsClosed>> + Send {
        ListenWriter::close(self.as_mut())
    }
}

/// Configuration that is fixed for one relay-client lifetime.
pub struct RelayClientConfig {
    /// The persisted home instance identifier used in relay URL query strings.
    pub instance_id: String,
    /// The configured HTTP(S) or WebSocket relay endpoint.
    pub relay_endpoint: String,
    /// The service credential sent in both the query and bearer header.
    pub service_token: ServiceToken,
    /// The absolute bound for collecting the four-byte dispatch prefix.
    pub dispatch_read_deadline: Duration,
    /// The interval between acknowledged relay-listener Ping frames.
    pub ping_interval: Duration,
    /// The absolute deadline for a relay-listener Pong acknowledgement.
    pub ping_ack_timeout: Duration,
    /// The continuously acknowledged duration required before resetting backoff.
    pub ack_stability_window: Duration,
    /// Maximum concurrently-prefix-peeking relay tunnels.
    pub global_admission_ceiling: usize,
}

/// Class-only relay-client failures.
///
/// These errors deliberately retain neither URLs nor upstream error strings:
/// every relay URL contains the service credential in its query string.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum RelayError {
    /// The relay refused or could not establish the listen WebSocket.
    #[error("relay listen connection failed")]
    ListenConnection,
}

#[derive(Clone)]
pub struct RelayClient {
    inner: Arc<RelayClientInner>,
}

struct RelayClientInner {
    config: RelayClientConfig,
    emit: Arc<dyn CallosumEmit>,
    dialer: Arc<dyn LoopbackDialer>,
    connector: Arc<dyn RelayConnector>,
    admission: Arc<RelayAdmissionGate>,
    health: Mutex<RelayHealth>,
    accepting_tunnels: AtomicBool,
    disconnect_announced: AtomicBool,
    tunnels: tokio::sync::Mutex<JoinSet<()>>,
    in_flight_tunnel_ids: Arc<Mutex<HashSet<String>>>,
}

/// The result of one listen attempt after it has ended and must reconnect.
struct ListenAttemptEnd {
    reset_backoff: bool,
}

impl RelayClient {
    /// Constructs a relay listener around the frozen U4 seams.
    #[must_use]
    pub fn new(
        config: RelayClientConfig,
        emit: Arc<dyn CallosumEmit>,
        dialer: Arc<dyn LoopbackDialer>,
    ) -> Self {
        Self::compose(config, emit, dialer, Arc::new(DefaultRelayConnector))
    }

    #[cfg(test)]
    fn new_with_connector(
        config: RelayClientConfig,
        emit: Arc<dyn CallosumEmit>,
        dialer: Arc<dyn LoopbackDialer>,
        connector: Arc<dyn RelayConnector>,
    ) -> Self {
        Self::compose(config, emit, dialer, connector)
    }

    fn compose(
        config: RelayClientConfig,
        emit: Arc<dyn CallosumEmit>,
        dialer: Arc<dyn LoopbackDialer>,
        connector: Arc<dyn RelayConnector>,
    ) -> Self {
        let admission = Arc::new(RelayAdmissionGate::new(config.global_admission_ceiling));
        Self {
            inner: Arc::new(RelayClientInner {
                config,
                emit,
                dialer,
                connector,
                admission,
                health: Mutex::new(RelayHealth::new()),
                accepting_tunnels: AtomicBool::new(true),
                disconnect_announced: AtomicBool::new(false),
                tunnels: tokio::sync::Mutex::new(JoinSet::new()),
                in_flight_tunnel_ids: Arc::new(Mutex::new(HashSet::new())),
            }),
        }
    }

    /// Runs the listen WebSocket and reconnects after every disconnected attempt.
    ///
    /// `stop` intentionally only stops paired tunnels. The supervisor owns this
    /// task and aborts it after calling `stop`, which is what closes the listen
    /// WebSocket on a posture transition.
    pub async fn run(&self) -> Result<(), RelayError> {
        let mut reconnect_base = Duration::ZERO;
        loop {
            let attempt = self.run_once().await;
            if attempt.reset_backoff {
                reconnect_base = Duration::ZERO;
            }
            self.announce_disconnect();
            let schedule = schedule_reconnect(reconnect_base, jitter_sample())
                .map_err(|_| RelayError::ListenConnection)?;
            reconnect_base = schedule.next_base;
            sleep(schedule.delay).await;
        }
    }

    /// Cancels and awaits all tunnel work without closing the listen WebSocket.
    pub async fn stop(&self) {
        self.inner.accepting_tunnels.store(false, Ordering::Release);
        let mut tunnels = self.inner.tunnels.lock().await;
        tunnels.shutdown().await;
        drop(tunnels);
        self.announce_disconnect();
    }

    async fn run_once(&self) -> ListenAttemptEnd {
        let mut reset_backoff_earned = false;
        self.begin_listen_attempt();
        let listen_url = relay_tunnel_url(
            &self.inner.config.relay_endpoint,
            "/session/listen",
            &self.inner.config.instance_id,
            self.inner.config.service_token.as_str(),
        );
        let (mut reader, mut writer) = match self
            .inner
            .connector
            .connect(&listen_url, &self.inner.config.service_token)
            .await
        {
            Ok(websocket) => websocket,
            Err(_) => {
                return ListenAttemptEnd {
                    reset_backoff: false,
                };
            }
        };
        let mut nonce_sequence = 0_u64;
        let initial_nonce = next_ping_nonce(&mut nonce_sequence);
        let mut outstanding_nonce = Some(initial_nonce.clone());
        let mut ack_deadline = Instant::now() + self.inner.config.ping_ack_timeout;
        let mut next_ping_at = Instant::now() + self.inner.config.ping_interval;
        let mut first_ack_at = None;
        let mut generation_acknowledged = false;

        if !send_ping_before_deadline(&mut writer, initial_nonce, ack_deadline).await {
            return ListenAttemptEnd {
                reset_backoff: false,
            };
        }

        loop {
            tokio::select! {
                _ = sleep_until(next_ping_at), if outstanding_nonce.is_none() => {
                    let nonce = next_ping_nonce(&mut nonce_sequence);
                    ack_deadline = Instant::now() + self.inner.config.ping_ack_timeout;
                    outstanding_nonce = Some(nonce.clone());
                    if !send_ping_before_deadline(&mut writer, nonce, ack_deadline).await {
                        return ListenAttemptEnd { reset_backoff: reset_backoff_earned };
                    }
                }
                _ = sleep_until(ack_deadline), if outstanding_nonce.is_some() => {
                    return ListenAttemptEnd { reset_backoff: reset_backoff_earned };
                }
                event = reader.next_listen_event() => match event {
                    Ok(ListenEvent::Message(message)) => {
                        if let ListenControl::Incoming { tunnel_id } = crate::parse_listen_control(message) {
                            self.accept_tunnel_offer(tunnel_id).await;
                        }
                    }
                    Ok(ListenEvent::Pong(pong)) => {
                        let acknowledged_at = Instant::now();
                        if outstanding_nonce.as_ref() == Some(&pong) && acknowledged_at <= ack_deadline {
                            outstanding_nonce = None;
                            let acknowledged_at_ms = now_ms();
                            self.record_listener_ack(!generation_acknowledged, acknowledged_at_ms);
                            generation_acknowledged = true;
                            if let Some(first_ack_at) = first_ack_at {
                                if stability_window_reached(
                                    first_ack_at,
                                    acknowledged_at,
                                    self.inner.config.ack_stability_window,
                                ) {
                                    reset_backoff_earned = true;
                                }
                            } else {
                                first_ack_at = Some(acknowledged_at);
                            }
                            next_ping_at = Instant::now() + self.inner.config.ping_interval;
                        }
                    }
                    Err(_) => return ListenAttemptEnd { reset_backoff: reset_backoff_earned },
                }
            }
        }
    }

    async fn accept_tunnel_offer(&self, tunnel_id: String) {
        if !self.inner.accepting_tunnels.load(Ordering::Acquire) {
            self.emit_tunnel_close(&tunnel_id);
            return;
        }
        if !lock_unpoisoned(&self.inner.in_flight_tunnel_ids).insert(tunnel_id.clone()) {
            return;
        }
        self.emit_tunnel_pair(&tunnel_id);
        self.start_tunnel_after_admission_check(tunnel_id).await;
    }

    async fn start_tunnel_after_admission_check(&self, tunnel_id: String) {
        let mut tunnels = self.inner.tunnels.lock().await;
        while tunnels.try_join_next().is_some() {}
        if !self.inner.accepting_tunnels.load(Ordering::Acquire) {
            lock_unpoisoned(&self.inner.in_flight_tunnel_ids).remove(&tunnel_id);
            self.emit_tunnel_close(&tunnel_id);
            return;
        }
        let client = self.clone();
        let lifecycle = TunnelLifecycle::new(
            client.clone(),
            tunnel_id.clone(),
            Arc::clone(&self.inner.in_flight_tunnel_ids),
        );
        tunnels.spawn(async move {
            client.handle_tunnel(tunnel_id, lifecycle).await;
        });
    }

    async fn handle_tunnel(&self, tunnel_id: String, _lifecycle: TunnelLifecycle) {
        let url = relay_tunnel_url(
            &self.inner.config.relay_endpoint,
            &format!("/tunnel/{tunnel_id}"),
            &self.inner.config.instance_id,
            self.inner.config.service_token.as_str(),
        );
        let websocket = self
            .inner
            .connector
            .connect(&url, &self.inner.config.service_token)
            .await;
        let (reader, mut writer) = match websocket {
            Ok(websocket) => websocket,
            Err(error) => {
                self.record_connect_failure(error);
                return;
            }
        };
        self.record_tunnel_success();
        let mut buffered = BufferedWsReader::new(reader);

        let Some(mut admission) = GlobalAdmission::acquire(Arc::clone(&self.inner.admission))
        else {
            self.record_admission_saturated();
            let _ = writer.close().await;
            return;
        };
        let prefix = buffered
            .peek_bounded(4, self.inner.config.dispatch_read_deadline)
            .await;
        let prefix = match prefix {
            Ok(prefix) => prefix,
            Err(_) => {
                self.record_failure(RelayTunnelFailure::RelayTunnelUnreachable);
                let _ = writer.close().await;
                return;
            }
        };

        match route_tunnel_prefix(&prefix) {
            TunnelRoute::TlsLoopback => {
                admission.release();
                match self.inner.dialer.connect().await {
                    Ok(loopback) => {
                        let _ = pipe_tunnel(&mut buffered, &mut writer, loopback).await;
                    }
                    Err(_) => {
                        self.record_failure(RelayTunnelFailure::LocalPrivateListenerUnreachable)
                    }
                }
            }
            TunnelRoute::Unsupported | TunnelRoute::NeedMorePrefix => {
                self.emit_unknown_prefix(&prefix);
            }
        }

        let _ = writer.close().await;
    }

    fn begin_listen_attempt(&self) {
        self.inner
            .disconnect_announced
            .store(false, Ordering::Release);
        {
            let mut health = lock_unpoisoned(&self.inner.health);
            health.begin_listen_attempt();
            health.set_state(RelayHealthState::Connecting);
        }
        self.inner.emit.emit("connecting", serde_json::json!({}));
        self.emit_health();
    }

    fn set_state(&self, state: RelayHealthState, event: &'static str) {
        lock_unpoisoned(&self.inner.health).set_state(state);
        self.inner.emit.emit(event, serde_json::json!({}));
        self.emit_health();
    }

    fn announce_disconnect(&self) {
        if !self.inner.disconnect_announced.swap(true, Ordering::AcqRel) {
            self.set_state(RelayHealthState::Reconnecting, "disconnect");
        }
    }

    fn record_tunnel_success(&self) {
        lock_unpoisoned(&self.inner.health).record_tunnel_success(now_ms());
        self.emit_health();
    }

    fn record_listener_ack(&self, first_for_generation: bool, timestamp_ms: u64) {
        {
            let mut health = lock_unpoisoned(&self.inner.health);
            health.record_listener_ack(timestamp_ms);
            if first_for_generation {
                health.set_state(RelayHealthState::Connected);
            }
        }
        if first_for_generation {
            self.inner.emit.emit("connected", serde_json::json!({}));
        }
        self.emit_health();
    }

    fn record_connect_failure(&self, error: RelayWebSocketError) {
        let failure = match error {
            RelayWebSocketError::Status(status) => {
                classify_relay_tunnel_failure(RelayTunnelFailureSignal::HttpStatus(status))
            }
            RelayWebSocketError::Request | RelayWebSocketError::Connection => {
                classify_relay_tunnel_failure(RelayTunnelFailureSignal::TransportFailure)
            }
        };
        self.record_failure(failure);
    }

    fn record_failure(&self, failure: RelayTunnelFailure) {
        lock_unpoisoned(&self.inner.health).record_tunnel_failure(failure, now_ms());
        self.emit_health();
    }

    fn record_admission_saturated(&self) {
        let count = self.inner.admission.saturated_count();
        {
            let mut health = lock_unpoisoned(&self.inner.health);
            health.set_relay_admission_saturated_count(count);
        }
        self.inner.emit.emit(
            "admission_saturated",
            serde_json::json!({"reason": "relay_admission_saturated", "count": count}),
        );
        self.emit_health();
    }

    fn emit_unknown_prefix(&self, prefix: &Bytes) {
        self.inner.emit.emit(
            "tunnel_unknown_prefix",
            serde_json::json!({"prefix": prefix_hex(prefix)}),
        );
    }

    fn emit_tunnel_pair(&self, tunnel_id: &str) {
        self.inner
            .emit
            .emit("tunnel_pair", serde_json::json!({"tunnel_id": tunnel_id}));
    }

    fn emit_tunnel_close(&self, tunnel_id: &str) {
        self.inner
            .emit
            .emit("tunnel_close", serde_json::json!({"tunnel_id": tunnel_id}));
        self.emit_health();
    }

    fn emit_health(&self) {
        let payload = {
            let mut health = lock_unpoisoned(&self.inner.health);
            health.set_relay_admission_saturated_count(self.inner.admission.saturated_count());
            health.payload()
        };
        self.inner.emit.emit("health", payload);
    }
}

impl crate::RelayStop for RelayClient {
    type Error = std::convert::Infallible;

    async fn stop(&mut self) -> Result<(), Self::Error> {
        RelayClient::stop(self).await;
        Ok(())
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn now_ms() -> u64 {
    let duration = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration,
        Err(_) => Duration::ZERO,
    };
    u64::try_from(duration.as_millis()).map_or(u64::MAX, std::convert::identity)
}

fn stability_window_reached(first_ack: Instant, current_ack: Instant, window: Duration) -> bool {
    current_ack.saturating_duration_since(first_ack) >= window
}

async fn send_ping_before_deadline(
    writer: &mut Box<dyn ListenWriter>,
    nonce: Bytes,
    deadline: Instant,
) -> bool {
    tokio::select! {
        result = ListenWriter::send_ping(writer.as_mut(), nonce) => result.is_ok(),
        _ = sleep_until(deadline) => false,
    }
}

fn next_ping_nonce(sequence: &mut u64) -> Bytes {
    let duration = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration,
        Err(_) => Duration::ZERO,
    };
    let nonce = duration.as_nanos().try_into().unwrap_or(u64::MAX) ^ *sequence;
    *sequence = sequence.wrapping_add(1);
    Bytes::copy_from_slice(&nonce.to_be_bytes())
}

fn jitter_sample() -> f64 {
    let duration = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration,
        Err(_) => Duration::ZERO,
    };
    f64::from(duration.subsec_nanos()) / 1_000_000_000.0
}

fn prefix_hex(prefix: &Bytes) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(prefix.len().saturating_mul(2));
    for byte in prefix {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

struct GlobalAdmission {
    gate: Arc<RelayAdmissionGate>,
    held: bool,
}

struct TunnelLifecycle {
    client: RelayClient,
    tunnel_id: String,
    in_flight_tunnel_ids: Arc<Mutex<HashSet<String>>>,
}

impl TunnelLifecycle {
    fn new(
        client: RelayClient,
        tunnel_id: String,
        in_flight_tunnel_ids: Arc<Mutex<HashSet<String>>>,
    ) -> Self {
        Self {
            client,
            tunnel_id,
            in_flight_tunnel_ids,
        }
    }
}

impl Drop for TunnelLifecycle {
    fn drop(&mut self) {
        lock_unpoisoned(&self.in_flight_tunnel_ids).remove(&self.tunnel_id);
        self.client.emit_tunnel_close(&self.tunnel_id);
    }
}

impl GlobalAdmission {
    fn acquire(gate: Arc<RelayAdmissionGate>) -> Option<Self> {
        gate.try_acquire().then_some(Self { gate, held: true })
    }

    fn release(&mut self) {
        if self.held {
            self.gate.release();
            self.held = false;
        }
    }
}

impl Drop for GlobalAdmission {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
struct FakeListenReader {
    events: tokio::sync::mpsc::UnboundedReceiver<Result<ListenEvent, WsClosed>>,
}

#[cfg(test)]
struct FakeListenWriter {
    pings: tokio::sync::mpsc::UnboundedSender<Bytes>,
    writes: tokio::sync::mpsc::UnboundedSender<Bytes>,
    closed: Arc<AtomicBool>,
}

#[cfg(test)]
struct FakeSocketHandle {
    events: tokio::sync::mpsc::UnboundedSender<Result<ListenEvent, WsClosed>>,
    pings: tokio::sync::mpsc::UnboundedReceiver<Bytes>,
    #[allow(dead_code)]
    writes: tokio::sync::mpsc::UnboundedReceiver<Bytes>,
    closed: Arc<AtomicBool>,
}

#[cfg(test)]
impl FakeSocketHandle {
    async fn recv_ping(&mut self) -> Result<Bytes, ()> {
        self.pings.recv().await.ok_or(())
    }

    fn push_pong(&self, nonce: Bytes) {
        let _ = self.events.send(Ok(ListenEvent::Pong(nonce)));
    }

    fn push_message(&self, bytes: impl Into<Bytes>) {
        let _ = self.events.send(Ok(ListenEvent::Message(bytes.into())));
    }

    fn close_read(&self) {
        let _ = self.events.send(Err(WsClosed));
    }

    async fn answer_one_ping(&mut self) -> Result<(), ()> {
        let nonce = self.recv_ping().await?;
        self.push_pong(nonce);
        Ok(())
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
struct FakeConnector {
    sockets: Mutex<std::collections::VecDeque<(FakeListenReader, FakeListenWriter)>>,
}

#[cfg(test)]
impl FakeConnector {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            sockets: Mutex::new(std::collections::VecDeque::new()),
        })
    }

    fn push_socket(self: &Arc<Self>) -> FakeSocketHandle {
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ping_tx, ping_rx) = tokio::sync::mpsc::unbounded_channel();
        let (write_tx, write_rx) = tokio::sync::mpsc::unbounded_channel();
        let closed = Arc::new(AtomicBool::new(false));
        lock_unpoisoned(&self.sockets).push_back((
            FakeListenReader { events: event_rx },
            FakeListenWriter {
                pings: ping_tx,
                writes: write_tx,
                closed: Arc::clone(&closed),
            },
        ));
        FakeSocketHandle {
            events: event_tx,
            pings: ping_rx,
            writes: write_rx,
            closed,
        }
    }
}

#[cfg(test)]
impl ListenReader for FakeListenReader {
    fn next_listen_event(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<ListenEvent, WsClosed>> + Send + '_>> {
        Box::pin(async move { self.events.recv().await.unwrap_or(Err(WsClosed)) })
    }

    fn next_message(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Bytes>, WsClosed>> + Send + '_>> {
        Box::pin(async move {
            loop {
                match self.events.recv().await.unwrap_or(Err(WsClosed)) {
                    Ok(ListenEvent::Message(bytes)) => return Ok(Some(bytes)),
                    Ok(ListenEvent::Pong(_)) => {}
                    Err(error) => return Err(error),
                }
            }
        })
    }
}

#[cfg(test)]
impl ListenWriter for FakeListenWriter {
    fn send_ping(
        &mut self,
        payload: Bytes,
    ) -> Pin<Box<dyn Future<Output = Result<(), WsClosed>> + Send + '_>> {
        let pings = self.pings.clone();
        Box::pin(async move { pings.send(payload).map_err(|_| WsClosed) })
    }

    fn send(
        &mut self,
        bytes: Bytes,
    ) -> Pin<Box<dyn Future<Output = Result<(), WsClosed>> + Send + '_>> {
        let writes = self.writes.clone();
        Box::pin(async move { writes.send(bytes).map_err(|_| WsClosed) })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<(), WsClosed>> + Send + '_>> {
        self.closed.store(true, Ordering::Release);
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
impl RelayConnector for FakeConnector {
    fn connect(
        &self,
        _url: &str,
        _token: &ServiceToken,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        (Box<dyn ListenReader>, Box<dyn ListenWriter>),
                        RelayWebSocketError,
                    >,
                > + Send,
        >,
    > {
        let pair = lock_unpoisoned(&self.sockets).pop_front();
        Box::pin(async move {
            pair.ok_or(RelayWebSocketError::Connection)
                .map(|(reader, writer)| {
                    (
                        Box::new(reader) as Box<dyn ListenReader>,
                        Box::new(writer) as Box<dyn ListenWriter>,
                    )
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::{
        FakeConnector, GlobalAdmission, LoopbackConnect, LoopbackDialer, RelayClient,
        RelayClientConfig, TunnelLifecycle, lock_unpoisoned, prefix_hex, stability_window_reached,
    };
    use bytes::Bytes;
    use tokio::{
        io::{AsyncReadExt, DuplexStream},
        sync::{Notify, oneshot},
        time::timeout,
    };

    use crate::{CallosumEmit, ServiceToken};

    struct Emitter {
        events: Mutex<Vec<(String, serde_json::Value)>>,
        changed: Notify,
    }

    impl Default for Emitter {
        fn default() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                changed: Notify::new(),
            }
        }
    }

    impl Emitter {
        fn snapshot(&self) -> Vec<(String, serde_json::Value)> {
            match self.events.lock() {
                Ok(events) => events.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }

        async fn wait_for_event(&self, expected: &str) {
            loop {
                let notified = self.changed.notified();
                if self
                    .events
                    .lock()
                    .is_ok_and(|events| events.iter().any(|(event, _)| event == expected))
                {
                    return;
                }
                notified.await;
            }
        }
    }

    impl CallosumEmit for Emitter {
        fn emit(&self, event: &'static str, fields: serde_json::Value) {
            match self.events.lock() {
                Ok(mut events) => events.push((event.to_owned(), fields)),
                Err(poisoned) => poisoned.into_inner().push((event.to_owned(), fields)),
            }
            self.changed.notify_waiters();
        }
    }

    struct Dialer {
        peer: Mutex<Option<oneshot::Sender<DuplexStream>>>,
    }

    impl Dialer {
        fn new(peer: oneshot::Sender<DuplexStream>) -> Self {
            Self {
                peer: Mutex::new(Some(peer)),
            }
        }
    }

    impl LoopbackDialer for Dialer {
        fn connect(&self) -> LoopbackConnect {
            let (client, peer) = tokio::io::duplex(1024);
            let sender = match self.peer.lock() {
                Ok(mut peer_slot) => peer_slot.take(),
                Err(poisoned) => poisoned.into_inner().take(),
            };
            Box::pin(async move {
                if let Some(sender) = sender {
                    let _ = sender.send(peer);
                }
                Ok(Box::new(client) as Box<dyn super::LoopbackStream>)
            })
        }
    }

    fn client_config(address: std::net::SocketAddr, token: &str) -> RelayClientConfig {
        RelayClientConfig {
            // Effectively never for tests asserting exact event sequences; the
            // heartbeat cadence test shortens these explicitly.
            ping_interval: Duration::from_secs(3600),
            ping_ack_timeout: Duration::from_secs(2),
            ack_stability_window: Duration::from_secs(3600),
            instance_id: "home-instance".to_owned(),
            relay_endpoint: format!("http://{address}"),
            service_token: ServiceToken::new(token.to_owned()),
            dispatch_read_deadline: Duration::from_secs(1),
            global_admission_ceiling: 1,
        }
    }

    fn dummy_addr() -> std::net::SocketAddr {
        "127.0.0.1:9".parse().expect("dummy address")
    }

    #[test]
    fn prefix_logging_is_bounded_hex_not_utf8_or_transport_text() {
        assert_eq!(
            prefix_hex(&Bytes::from_static(&[0, 0xff, 0x16, 0x03])),
            "00ff1603"
        );
    }

    #[test]
    fn listener_stability_uses_the_exact_monotonic_boundary() {
        let first_ack = tokio::time::Instant::now();
        let window = Duration::from_secs(60);

        assert!(!stability_window_reached(
            first_ack,
            first_ack + window - Duration::from_nanos(1),
            window,
        ));
        assert!(stability_window_reached(
            first_ack,
            first_ack + window,
            window,
        ));
    }

    #[test]
    fn global_admission_releases_on_drop_and_explicit_early_release() {
        let gate = Arc::new(crate::RelayAdmissionGate::new(1));
        let mut guard = GlobalAdmission::acquire(Arc::clone(&gate));
        assert!(guard.is_some());
        assert_eq!(gate.count(), 1);
        if let Some(guard) = guard.as_mut() {
            guard.release();
        }
        assert_eq!(gate.count(), 0);
        drop(guard);
        assert_eq!(gate.count(), 0);

        let guard = GlobalAdmission::acquire(Arc::clone(&gate));
        assert!(guard.is_some());
        drop(guard);
        assert_eq!(gate.count(), 0);
    }

    #[test]
    fn tunnel_lifecycle_drop_releases_its_coalescing_id() -> Result<(), String> {
        let (peer_sender, _peer_receiver) = oneshot::channel();
        let emitter = Arc::new(Emitter::default());
        let client = RelayClient::new(
            client_config(
                "127.0.0.1:9".parse().map_err(|_| "test address invalid")?,
                "token",
            ),
            Arc::clone(&emitter) as Arc<dyn CallosumEmit>,
            Arc::new(Dialer::new(peer_sender)),
        );
        let ids = Arc::new(Mutex::new(HashSet::from(["reusable".to_owned()])));
        drop(TunnelLifecycle::new(
            client,
            "reusable".to_owned(),
            Arc::clone(&ids),
        ));

        assert!(!lock_unpoisoned(&ids).contains("reusable"));
        Ok(())
    }

    #[tokio::test]
    async fn stop_race_after_pair_emits_one_terminal_close_and_health() -> Result<(), String> {
        let (peer_sender, _peer_receiver) = oneshot::channel();
        let emitter = Arc::new(Emitter::default());
        let emission: Arc<dyn CallosumEmit> = emitter.clone();
        let client = RelayClient::new(
            client_config(
                "127.0.0.1:9".parse().map_err(|_| "test address invalid")?,
                "token",
            ),
            emission,
            Arc::new(Dialer::new(peer_sender)),
        );

        client.emit_tunnel_pair("race");
        client
            .inner
            .accepting_tunnels
            .store(false, std::sync::atomic::Ordering::Release);
        client
            .start_tunnel_after_admission_check("race".to_owned())
            .await;

        assert_eq!(
            emitter.snapshot(),
            vec![
                (
                    "tunnel_pair".to_owned(),
                    serde_json::json!({"tunnel_id": "race"})
                ),
                (
                    "tunnel_close".to_owned(),
                    serde_json::json!({"tunnel_id": "race"})
                ),
                (
                    "health".to_owned(),
                    serde_json::json!({
                        "state": "connecting",
                        "listen_generation": 0,
                        "last_successful_relay_tunnel_at": null,
                        "last_relay_tunnel_error": null,
                        "last_relay_tunnel_error_at": null,
                        "relay_tunnel_error_status": null,
                        "relay_admission_saturated_count": 0,
                        "last_relay_listener_ack_at": null,
                        "last_relay_listener_ack_generation": null,
                    }),
                ),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn listener_requires_a_matching_pong_before_reporting_connected() -> Result<(), String> {
        let connector = FakeConnector::new();
        let mut listen = connector.push_socket();
        let emitter = Arc::new(Emitter::default());
        let client = RelayClient::new_with_connector(
            client_config(dummy_addr(), "test-token"),
            Arc::clone(&emitter) as Arc<dyn CallosumEmit>,
            Arc::new(Dialer::new(oneshot::channel().0)),
            connector,
        );
        let running = {
            let client = client.clone();
            tokio::spawn(async move { client.run_once().await })
        };

        let nonce = timeout(Duration::from_secs(1), listen.recv_ping())
            .await
            .map_err(|_| "initial ping was not observed".to_owned())?
            .map_err(|_| "initial ping sender dropped".to_owned())?;
        assert_eq!(nonce.len(), 8);
        let pre_ack = emitter.snapshot();
        assert!(pre_ack.iter().all(|(event, _)| event != "connected"));
        let health = pre_ack
            .iter()
            .rev()
            .find_map(|(event, fields)| (event == "health").then_some(fields))
            .ok_or_else(|| "missing pre-ack health".to_owned())?;
        assert_eq!(health["state"], "connecting");
        assert!(health["last_relay_listener_ack_at"].is_null());
        assert!(health["last_relay_listener_ack_generation"].is_null());

        listen.push_pong(nonce);
        timeout(Duration::from_secs(1), emitter.wait_for_event("connected"))
            .await
            .map_err(|_| "matching pong did not connect listener".to_owned())?;
        let connected_health = emitter
            .snapshot()
            .into_iter()
            .rev()
            .find_map(|(event, fields)| {
                (event == "health" && fields["state"] == "connected").then_some(fields)
            })
            .ok_or_else(|| "missing connected health".to_owned())?;
        assert!(connected_health["last_relay_listener_ack_at"].is_u64());
        assert_eq!(connected_health["last_relay_listener_ack_generation"], 1);

        running.abort();
        let _ = running.await;
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn wrong_pongs_and_control_traffic_do_not_extend_the_ack_deadline() -> Result<(), String>
    {
        let connector = FakeConnector::new();
        let mut listen = connector.push_socket();
        let emitter = Arc::new(Emitter::default());
        let mut config = client_config(dummy_addr(), "test-token");
        config.ping_ack_timeout = Duration::from_millis(80);
        let client = RelayClient::new_with_connector(
            config,
            Arc::clone(&emitter) as Arc<dyn CallosumEmit>,
            Arc::new(Dialer::new(oneshot::channel().0)),
            connector,
        );
        let run = {
            let client = client.clone();
            tokio::spawn(async move { client.run().await })
        };

        timeout(Duration::from_secs(1), listen.recv_ping())
            .await
            .map_err(|_| "initial ping was not observed".to_owned())?
            .map_err(|_| "initial ping sender dropped".to_owned())?;
        for _ in 0..4 {
            tokio::time::advance(Duration::from_millis(40)).await;
            listen.push_pong(Bytes::from_static(b"wrong"));
            listen.push_message(&b"{\"type\":\"ignored\"}"[..]);
        }
        tokio::time::advance(Duration::from_millis(80)).await;
        timeout(
            Duration::from_millis(150),
            emitter.wait_for_event("disconnect"),
        )
        .await
        .map_err(|_| "missed acknowledgement did not disconnect".to_owned())?;
        let events = emitter.snapshot();
        assert!(events.iter().all(|(event, _)| event != "connected"));
        assert!(
            events
                .iter()
                .any(|(event, fields)| { event == "health" && fields["state"] == "reconnecting" })
        );

        run.abort();
        let _ = run.await;
        Ok(())
    }

    #[tokio::test]
    async fn listener_ack_before_stability_window_does_not_reset_backoff() -> Result<(), String> {
        let connector = FakeConnector::new();
        let mut listen = connector.push_socket();
        let mut config = client_config(dummy_addr(), "test-token");
        config.ping_interval = Duration::from_millis(60);
        config.ping_ack_timeout = Duration::from_millis(30);
        config.ack_stability_window = Duration::from_millis(120);
        let client = RelayClient::new_with_connector(
            config,
            Arc::new(Emitter::default()) as Arc<dyn CallosumEmit>,
            Arc::new(Dialer::new(oneshot::channel().0)),
            connector,
        );
        let running = {
            let client = client.clone();
            tokio::spawn(async move { client.run_once().await })
        };
        timeout(Duration::from_secs(1), listen.answer_one_ping())
            .await
            .map_err(|_| "initial ping was not observed".to_owned())?
            .map_err(|_| "initial ping sender dropped".to_owned())?;
        listen.close_read();

        let attempt = timeout(Duration::from_secs(2), running)
            .await
            .map_err(|_| "under-window listener did not end".to_owned())?
            .map_err(|_| "under-window listener task failed".to_owned())?;
        assert!(!attempt.reset_backoff);
        Ok(())
    }

    #[tokio::test]
    async fn continuously_acknowledged_listener_earns_backoff_reset() -> Result<(), String> {
        let connector = FakeConnector::new();
        let mut listen = connector.push_socket();
        let mut config = client_config(dummy_addr(), "test-token");
        config.ping_interval = Duration::from_millis(60);
        config.ping_ack_timeout = Duration::from_millis(30);
        config.ack_stability_window = Duration::from_millis(120);
        let client = RelayClient::new_with_connector(
            config,
            Arc::new(Emitter::default()) as Arc<dyn CallosumEmit>,
            Arc::new(Dialer::new(oneshot::channel().0)),
            connector,
        );
        let running = {
            let client = client.clone();
            tokio::spawn(async move { client.run_once().await })
        };
        for _ in 0..3 {
            timeout(Duration::from_secs(1), listen.answer_one_ping())
                .await
                .map_err(|_| "acknowledged ping was not observed".to_owned())?
                .map_err(|_| "acknowledged ping sender dropped".to_owned())?;
        }
        listen.close_read();

        let attempt = timeout(Duration::from_secs(2), running)
            .await
            .map_err(|_| "acknowledged listener did not end".to_owned())?
            .map_err(|_| "acknowledged listener task failed".to_owned())?;
        assert!(attempt.reset_backoff);
        Ok(())
    }

    #[tokio::test]
    async fn replacement_generation_keeps_old_ack_but_stays_connecting_until_its_pong()
    -> Result<(), String> {
        let connector = FakeConnector::new();
        let mut first = connector.push_socket();
        let mut replacement = connector.push_socket();
        let emitter = Arc::new(Emitter::default());
        let client = RelayClient::new_with_connector(
            client_config(dummy_addr(), "test-token"),
            Arc::clone(&emitter) as Arc<dyn CallosumEmit>,
            Arc::new(Dialer::new(oneshot::channel().0)),
            connector,
        );
        let run = {
            let client = client.clone();
            tokio::spawn(async move { client.run().await })
        };

        timeout(Duration::from_secs(1), first.answer_one_ping())
            .await
            .map_err(|_| "first generation ping was not observed".to_owned())?
            .map_err(|_| "first generation ping sender dropped".to_owned())?;
        timeout(Duration::from_secs(1), emitter.wait_for_event("connected"))
            .await
            .map_err(|_| "first generation did not connect".to_owned())?;
        first.close_read();
        timeout(Duration::from_secs(3), replacement.recv_ping())
            .await
            .map_err(|_| "replacement generation did not begin".to_owned())?
            .map_err(|_| "replacement signal dropped".to_owned())?;
        let replacement_health = emitter
            .snapshot()
            .into_iter()
            .rev()
            .find_map(|(event, fields)| {
                (event == "health"
                    && fields["listen_generation"] == 2
                    && fields["state"] == "connecting")
                    .then_some(fields)
            })
            .ok_or_else(|| "missing unacknowledged replacement health".to_owned())?;
        assert!(replacement_health["last_relay_listener_ack_at"].is_u64());
        assert_eq!(replacement_health["last_relay_listener_ack_generation"], 1);

        client.stop().await;
        run.abort();
        let _ = run.await;
        Ok(())
    }

    #[tokio::test]
    async fn tunnel_offers_coalesce_across_listener_generations() -> Result<(), String> {
        let connector = FakeConnector::new();
        let mut first_listen = connector.push_socket();
        let tunnel = connector.push_socket();
        let mut replacement_listen = connector.push_socket();
        let mut extra = connector.push_socket();
        let (peer_sender, mut peer_receiver) = oneshot::channel();
        let emitter = Arc::new(Emitter::default());
        let client = RelayClient::new_with_connector(
            client_config(dummy_addr(), "test-token"),
            Arc::clone(&emitter) as Arc<dyn CallosumEmit>,
            Arc::new(Dialer::new(peer_sender)),
            connector,
        );
        let run = {
            let client = client.clone();
            tokio::spawn(async move { client.run().await })
        };

        timeout(Duration::from_secs(1), first_listen.answer_one_ping())
            .await
            .map_err(|_| "first listen ping was not observed".to_owned())?
            .map_err(|_| "first listen ping sender dropped".to_owned())?;
        for _ in 0..2 {
            first_listen.push_message(&b"{\"type\":\"incoming\",\"tunnel_id\":\"shared\"}"[..]);
        }
        tunnel.push_message(Bytes::from_static(&[0x16, 0x03, 0x01, 0x00]));
        let mut peer = timeout(Duration::from_secs(2), &mut peer_receiver)
            .await
            .map_err(|_| "shared tunnel did not reach loopback".to_owned())?
            .map_err(|_| "loopback peer dropped".to_owned())?;
        let mut prefix = [0_u8; 4];
        peer.read_exact(&mut prefix)
            .await
            .map_err(|_| "loopback prefix read failed".to_owned())?;
        assert_eq!(prefix, [0x16, 0x03, 0x01, 0x00]);
        first_listen.close_read();
        timeout(Duration::from_secs(3), replacement_listen.answer_one_ping())
            .await
            .map_err(|_| "replacement listener did not begin".to_owned())?
            .map_err(|_| "replacement listener ping dropped".to_owned())?;
        replacement_listen.push_message(&b"{\"type\":\"incoming\",\"tunnel_id\":\"shared\"}"[..]);
        let duplicate_opened = timeout(Duration::from_millis(250), extra.recv_ping())
            .await
            .is_ok();
        assert!(!duplicate_opened, "duplicate offer opened a second tunnel");
        assert_eq!(
            emitter
                .snapshot()
                .iter()
                .filter(|(event, fields)| event == "tunnel_pair" && fields["tunnel_id"] == "shared")
                .count(),
            1
        );

        client.stop().await;
        run.abort();
        let _ = run.await;
        Ok(())
    }

    #[tokio::test]
    async fn listener_dispatches_tls_to_loopback_and_replays_the_peeked_prefix()
    -> Result<(), String> {
        let connector = FakeConnector::new();
        let mut listen = connector.push_socket();
        let tunnel = connector.push_socket();
        let (peer_sender, mut peer_receiver) = oneshot::channel();
        let emitter = Arc::new(Emitter::default());
        let emission: Arc<dyn CallosumEmit> = emitter.clone();
        let client = RelayClient::new_with_connector(
            client_config(dummy_addr(), "known-service-token"),
            emission,
            Arc::new(Dialer::new(peer_sender)),
            connector,
        );
        let running = {
            let client = client.clone();
            tokio::spawn(async move { client.run().await })
        };

        timeout(Duration::from_secs(1), listen.answer_one_ping())
            .await
            .map_err(|_| "listen ping was not observed".to_owned())?
            .map_err(|_| "listen ping sender dropped".to_owned())?;
        listen.push_message(&b"{\"type\":\"incoming\",\"tunnel_id\":\"tls\"}"[..]);
        tunnel.push_message(Bytes::from_static(&[0x16, 0x03]));
        tunnel.push_message(Bytes::from_static(&[0x01, 0x00]));

        let mut peer = timeout(Duration::from_secs(2), &mut peer_receiver)
            .await
            .map_err(|_| "loopback dial timed out".to_owned())?
            .map_err(|_| "loopback dial was dropped".to_owned())?;
        let mut received = [0_u8; 4];
        peer.read_exact(&mut received)
            .await
            .map_err(|_| "loopback read failed".to_owned())?;
        assert_eq!(received, [0x16, 0x03, 0x01, 0x00]);
        assert_eq!(client.inner.admission.count(), 0);

        client.stop().await;
        let events_after_stop = emitter.snapshot();
        let tail = events_after_stop
            .get(events_after_stop.len().saturating_sub(4)..)
            .ok_or_else(|| "missing cancelled tunnel event tail".to_owned())?;
        if tail.len() != 4 {
            return Err("cancelled tunnel event tail had an unexpected length".to_owned());
        }
        assert_eq!(
            tail[0],
            (
                "tunnel_close".to_owned(),
                serde_json::json!({"tunnel_id": "tls"})
            )
        );
        assert_eq!(tail[1].0, "health");
        assert_eq!(tail[2], ("disconnect".to_owned(), serde_json::json!({})));
        assert_eq!(tail[3].0, "health");
        running.abort();
        let _ = running.await;
        Ok(())
    }

    #[tokio::test]
    async fn unsupported_prefixes_close_as_the_same_unknown_route() -> Result<(), String> {
        let connector = FakeConnector::new();
        let mut listen = connector.push_socket();
        let tunnel = connector.push_socket();
        let (peer_sender, _peer_receiver) = oneshot::channel();
        let emitter = Arc::new(Emitter::default());
        let emission: Arc<dyn CallosumEmit> = emitter.clone();
        let client = RelayClient::new_with_connector(
            client_config(dummy_addr(), "known-service-token"),
            emission,
            Arc::new(Dialer::new(peer_sender)),
            connector,
        );
        let running = {
            let client = client.clone();
            tokio::spawn(async move { client.run().await })
        };

        timeout(Duration::from_secs(1), listen.answer_one_ping())
            .await
            .map_err(|_| "listen ping was not observed".to_owned())?
            .map_err(|_| "listen ping sender dropped".to_owned())?;
        listen.push_message(&b"{\"type\":\"incoming\",\"tunnel_id\":\"unknown\"}"[..]);
        tunnel.push_message(Bytes::from_static(b"RETI"));
        timeout(Duration::from_secs(2), async {
            loop {
                if tunnel.is_closed() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "unknown prefix did not close".to_owned())?;
        assert_eq!(client.inner.admission.count(), 0);
        let formatted = {
            let events = match emitter.events.lock() {
                Ok(events) => events,
                Err(poisoned) => poisoned.into_inner(),
            };
            format!("{events:?}")
        };
        assert!(formatted.contains("52455449"));
        assert!(!formatted.contains("known-service-token"));

        client.stop().await;
        running.abort();
        let _ = running.await;
        Ok(())
    }

    #[tokio::test]
    async fn short_prefix_error_releases_the_global_admission_slot() -> Result<(), String> {
        let connector = FakeConnector::new();
        let mut listen = connector.push_socket();
        let tunnel = connector.push_socket();
        let (peer_sender, _peer_receiver) = oneshot::channel();
        let emitter = Arc::new(Emitter::default());
        let emission: Arc<dyn CallosumEmit> = emitter.clone();
        let client = RelayClient::new_with_connector(
            client_config(dummy_addr(), "known-service-token"),
            emission,
            Arc::new(Dialer::new(peer_sender)),
            connector,
        );
        let running = {
            let client = client.clone();
            tokio::spawn(async move { client.run().await })
        };

        timeout(Duration::from_secs(1), listen.answer_one_ping())
            .await
            .map_err(|_| "listen ping was not observed".to_owned())?
            .map_err(|_| "listen ping sender dropped".to_owned())?;
        timeout(Duration::from_secs(1), emitter.wait_for_event("connected"))
            .await
            .map_err(|_| "listener did not connect before the short prefix".to_owned())?;
        listen.push_message(&b"{\"type\":\"incoming\",\"tunnel_id\":\"short\"}"[..]);
        tunnel.push_message(Bytes::from_static(b"no"));
        tunnel.close_read();
        timeout(
            Duration::from_secs(2),
            emitter.wait_for_event("tunnel_close"),
        )
        .await
        .map_err(|_| "short-prefix tunnel did not finish".to_owned())?;
        {
            let mut tunnels = client.inner.tunnels.lock().await;
            let joined = tunnels.try_join_next();
            assert!(joined.is_some());
        }
        assert_eq!(client.inner.admission.count(), 0);
        let events = emitter.snapshot();
        assert!(events.contains(&(
            "tunnel_pair".to_owned(),
            serde_json::json!({"tunnel_id": "short"}),
        )));
        assert!(events.windows(2).any(|events| {
            events[0]
                == (
                    "tunnel_close".to_owned(),
                    serde_json::json!({"tunnel_id": "short"}),
                )
                && events[1].0 == "health"
        }));

        client.stop().await;
        running.abort();
        let _ = running.await;
        Ok(())
    }

    /// A quiet but acknowledged listener refreshes health at every Ping/Pong.
    #[tokio::test(start_paused = true)]
    async fn a_quiet_connected_listener_keeps_refreshing_its_health_snapshot() -> Result<(), String>
    {
        let connector = FakeConnector::new();
        let mut listen = connector.push_socket();
        let emitter = Arc::new(Emitter::default());
        let mut config = client_config(dummy_addr(), "test-token");
        config.ping_interval = Duration::from_millis(120);
        config.ping_ack_timeout = Duration::from_millis(40);
        config.ack_stability_window = Duration::from_millis(240);
        let client = RelayClient::new_with_connector(
            config,
            Arc::clone(&emitter) as Arc<dyn CallosumEmit>,
            Arc::new(Dialer::new(oneshot::channel().0)),
            connector,
        );
        let run = {
            let client = client.clone();
            tokio::spawn(async move { client.run().await })
        };

        for _ in 0..4 {
            timeout(Duration::from_secs(1), listen.answer_one_ping())
                .await
                .map_err(|_| "quiet listener ping was not observed".to_owned())?
                .map_err(|_| "quiet listener ping sender dropped".to_owned())?;
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_millis(120)).await;
        }
        let health_emits = emitter
            .snapshot()
            .into_iter()
            .filter(|(event, _)| event == "health")
            .count();
        run.abort();

        if health_emits < 4 {
            return Err(format!(
                "a quiet connected listener stopped refreshing health: {health_emits} emits"
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn stop_emits_final_disconnect_and_health_before_run_task_cancellation()
    -> Result<(), String> {
        let connector = FakeConnector::new();
        let mut listen = connector.push_socket();
        let (peer_sender, _peer_receiver) = oneshot::channel();
        let emitter = Arc::new(Emitter::default());
        let emission: Arc<dyn CallosumEmit> = emitter.clone();
        let client = RelayClient::new_with_connector(
            client_config(dummy_addr(), "known-service-token"),
            emission,
            Arc::new(Dialer::new(peer_sender)),
            connector,
        );
        let running = {
            let client = client.clone();
            tokio::spawn(async move { client.run().await })
        };

        timeout(Duration::from_secs(1), listen.answer_one_ping())
            .await
            .map_err(|_| "listen websocket did not connect".to_owned())?
            .map_err(|_| "listen ping sender dropped".to_owned())?;
        timeout(Duration::from_secs(2), emitter.wait_for_event("connected"))
            .await
            .map_err(|_| "connected event did not arrive".to_owned())?;
        let ack_at = emitter
            .snapshot()
            .into_iter()
            .find_map(|(event, fields)| {
                (event == "health" && fields["state"] == "connected")
                    .then(|| fields["last_relay_listener_ack_at"].as_u64())
                    .flatten()
            })
            .ok_or_else(|| "connected health omitted listener acknowledgement".to_owned())?;

        client.stop().await;
        let events_after_stop = emitter.snapshot();
        let tail = events_after_stop
            .get(events_after_stop.len().saturating_sub(2)..)
            .ok_or_else(|| "missing disconnect event tail".to_owned())?;
        if tail.len() != 2 {
            return Err("disconnect event tail had an unexpected length".to_owned());
        }
        assert_eq!(tail[0], ("disconnect".to_owned(), serde_json::json!({})));
        assert_eq!(
            tail[1],
            (
                "health".to_owned(),
                serde_json::json!({
                    "state": "reconnecting",
                    "listen_generation": 1,
                    "last_successful_relay_tunnel_at": null,
                    "last_relay_tunnel_error": null,
                    "last_relay_tunnel_error_at": null,
                    "relay_tunnel_error_status": null,
                    "relay_admission_saturated_count": 0,
                    "last_relay_listener_ack_at": ack_at,
                    "last_relay_listener_ack_generation": 1,
                }),
            )
        );
        assert!(!listen.is_closed());

        running.abort();
        let _ = running.await;
        assert_eq!(emitter.snapshot(), events_after_stop);
        Ok(())
    }
}
