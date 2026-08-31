// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded SPL mux session over a completed journal-to-bridge carrier.
//!
//! Stream 1 is private lease control. Only later bridge-opened streams reach
//! the caller, so the control channel has no public byte-stream capability.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use chrono::Utc;
use ring::rand::{SecureRandom as _, SystemRandom};
use spl_core::frame::{Frame, RECOMMENDED_CHUNK};
use spl_core::mux::INITIAL_WINDOW;
use spl_home::{MuxAcceptor, MuxEvent, MuxLimits, MuxOutput, ResetReason};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};
use tokio::time::{Duration, Instant, sleep, sleep_until};
use tokio_rustls::client::TlsStream;

use crate::McpBridgeCarrierError;
use crate::McpEndpointOwnerContext;
use crate::account_wire::refresh_mcp_bridge_authority;
use crate::bridge_carrier::{BridgeAuthority, BridgeBinding};

const CONTROL_STREAM_ID: u32 = 1;
const PUBLIC_STREAM_CAPACITY: usize = 255;
const DRIVER_COMMAND_CAPACITY: usize = 512;
const STREAM_SIGNAL_CAPACITY: usize = 1;
const CARRIER_READ_BYTES: usize = 4_096;
const RENEW_FETCH_LEAD_SECONDS: i64 = 180;
const RENEW_CHALLENGE_LEAD_SECONDS: i64 = 120;
const CONTROL_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONTROL_FRAME_BYTES: usize = 65_536 + 4;

const STREAM_LIVE: u8 = 0;
const STREAM_RESET: u8 = 1;
const STREAM_GONE: u8 = 2;

/// An authenticated bridge session. Only bridge-opened public streams emerge.
pub struct McpBridgeSession {
    accepts: AsyncMutex<mpsc::Receiver<McpPublicStream>>,
    commands: mpsc::Sender<DriverCommand>,
    cancel: watch::Sender<bool>,
}

/// An opaque bounded bidirectional byte stream for one public SPL stream.
pub struct McpPublicStream {
    id: u32,
    signals: mpsc::Receiver<StreamSignal>,
    commands: mpsc::Sender<DriverCommand>,
    command_wakers: Arc<CommandWakers>,
    state: Arc<StreamStatus>,
    read_buffer: VecDeque<u8>,
    read_eof: bool,
    write_shutdown: bool,
}

enum DriverCommand {
    Write { stream_id: u32, bytes: Vec<u8> },
    Wake,
    CloseSession,
}

enum StreamSignal {
    Data(Vec<u8>),
    ReadEof,
    Reset,
    Gone,
}

struct StreamStatus {
    state: AtomicU8,
    outbound_staged: AtomicUsize,
    inbound_buffered: AtomicUsize,
    consumed: AtomicUsize,
    close_requested: AtomicBool,
    cancel_requested: AtomicBool,
    write_waker: Mutex<Option<Waker>>,
}

impl StreamStatus {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(STREAM_LIVE),
            outbound_staged: AtomicUsize::new(0),
            inbound_buffered: AtomicUsize::new(0),
            consumed: AtomicUsize::new(0),
            close_requested: AtomicBool::new(false),
            cancel_requested: AtomicBool::new(false),
            write_waker: Mutex::new(None),
        }
    }

    fn reserve_write(&self, requested: usize, waker: &Waker) -> usize {
        let mut staged = self.outbound_staged.load(Ordering::Acquire);
        loop {
            let available = RECOMMENDED_CHUNK.saturating_sub(staged);
            if available == 0 {
                if let Ok(mut slot) = self.write_waker.lock() {
                    *slot = Some(waker.clone());
                }
                return 0;
            }
            let granted = requested.min(available);
            match self.outbound_staged.compare_exchange_weak(
                staged,
                staged + granted,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return granted,
                Err(current) => staged = current,
            }
        }
    }

    fn release_write(&self, bytes: usize) {
        self.outbound_staged.fetch_sub(bytes, Ordering::AcqRel);
        if let Ok(mut slot) = self.write_waker.lock()
            && let Some(waker) = slot.take()
        {
            waker.wake();
        }
    }

    fn record_consumed(&self, bytes: usize) {
        self.inbound_buffered.fetch_sub(bytes, Ordering::AcqRel);
        let _ = self
            .consumed
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(bytes)
            });
    }

    fn take_consumed(&self) -> usize {
        self.consumed.swap(0, Ordering::AcqRel)
    }

    fn reserve_inbound(&self, bytes: usize) -> bool {
        let mut buffered = self.inbound_buffered.load(Ordering::Acquire);
        loop {
            let Some(next) = buffered.checked_add(bytes) else {
                return false;
            };
            if next > INITIAL_WINDOW {
                return false;
            }
            match self.inbound_buffered.compare_exchange_weak(
                buffered,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => buffered = current,
            }
        }
    }
}

/// At most one parked writer per public stream may wait for command capacity.
struct CommandWakers {
    entries: Mutex<Vec<(u32, Waker)>>,
}

impl CommandWakers {
    fn register(&self, stream_id: u32, waker: &Waker) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if let Some((_, registered)) = entries.iter_mut().find(|(id, _)| *id == stream_id) {
            *registered = waker.clone();
        } else if entries.len() < PUBLIC_STREAM_CAPACITY {
            entries.push((stream_id, waker.clone()));
        }
    }

    fn wake_all(&self) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        for (_, waker) in entries.drain(..) {
            waker.wake();
        }
    }
}

struct DriverStream {
    tx: mpsc::Sender<StreamSignal>,
    signals: VecDeque<StreamSignal>,
    state: Arc<StreamStatus>,
}

struct ControlState {
    bytes: Vec<u8>,
    deadline: Option<Instant>,
    response: Option<(Vec<u8>, BridgeAuthority)>,
}

impl ControlState {
    fn append(&mut self, bytes: Vec<u8>, expiry: Instant) -> Result<Option<Vec<u8>>, ()> {
        if self.response.is_some() {
            return Err(());
        }
        if self.bytes.is_empty() {
            self.deadline = Some((Instant::now() + CONTROL_EXCHANGE_TIMEOUT).min(expiry));
        }
        if self.bytes.len().saturating_add(bytes.len()) > MAX_CONTROL_FRAME_BYTES {
            return Err(());
        }
        self.bytes.extend_from_slice(&bytes);
        if self.bytes.len() < 4 {
            return Ok(None);
        }
        let length = u32::from_be_bytes(self.bytes[..4].try_into().map_err(|_| ())?) as usize;
        if length > MAX_CONTROL_FRAME_BYTES - 4 {
            return Err(());
        }
        let total = length + 4;
        if self.bytes.len() < total {
            return Ok(None);
        }
        if self.bytes.len() != total {
            return Err(());
        }
        Ok(Some(std::mem::take(&mut self.bytes)))
    }
}

/// Start the pinned mux session after the first PoP exchange has succeeded.
pub(crate) fn start_bridge_session(
    carrier: TlsStream<tokio::net::TcpStream>,
    authority: BridgeAuthority,
    renewal_owner: McpEndpointOwnerContext,
    external_shutdown: watch::Receiver<bool>,
) -> Result<McpBridgeSession, McpBridgeCarrierError> {
    let acceptor =
        MuxAcceptor::new(MuxLimits::default()).map_err(|_| McpBridgeCarrierError::Pop)?;
    let (accept_tx, accept_rx) = mpsc::channel(PUBLIC_STREAM_CAPACITY);
    let (command_tx, command_rx) = mpsc::channel(DRIVER_COMMAND_CAPACITY);
    let (renewal_tx, renewal_rx) = mpsc::channel(1);
    let (advance_tx, advance_rx) = mpsc::channel(1);
    let (cancel, internal_shutdown) = watch::channel(false);
    let command_wakers = Arc::new(CommandWakers {
        entries: Mutex::new(Vec::with_capacity(PUBLIC_STREAM_CAPACITY)),
    });
    let binding = authority.binding();
    let epoch = MonotonicEpoch::new(Utc::now().timestamp());
    let proof_key = renewal_owner.proof_keypair();
    tokio::spawn(run_renewal_fetcher(
        renewal_owner,
        binding,
        authority.expires_at(),
        epoch,
        internal_shutdown.clone(),
        renewal_tx,
        advance_rx,
    ));
    tokio::spawn(run_driver(
        carrier,
        acceptor,
        authority,
        epoch,
        accept_tx,
        command_tx.clone(),
        command_rx,
        Arc::clone(&command_wakers),
        external_shutdown,
        internal_shutdown,
        cancel.clone(),
        renewal_rx,
        advance_tx,
        proof_key,
    ));
    Ok(McpBridgeSession {
        accepts: AsyncMutex::new(accept_rx),
        commands: command_tx,
        cancel,
    })
}

impl McpBridgeSession {
    /// Wait for the next bridge-opened public stream after private control is live.
    pub async fn accept_public(&self) -> Result<McpPublicStream, McpBridgeCarrierError> {
        self.accepts
            .lock()
            .await
            .recv()
            .await
            .ok_or(McpBridgeCarrierError::Io)
    }

    /// Stop the carrier task and all of its public streams.
    pub async fn shutdown(self) -> Result<(), McpBridgeCarrierError> {
        self.cancel.send_replace(true);
        let _ = self.commands.send(DriverCommand::CloseSession).await;
        Ok(())
    }
}

impl Drop for McpBridgeSession {
    fn drop(&mut self) {
        self.cancel.send_replace(true);
        let _ = self.commands.try_send(DriverCommand::CloseSession);
    }
}

impl McpPublicStream {
    fn new(
        id: u32,
        signals: mpsc::Receiver<StreamSignal>,
        commands: mpsc::Sender<DriverCommand>,
        command_wakers: Arc<CommandWakers>,
        state: Arc<StreamStatus>,
    ) -> Self {
        Self {
            id,
            signals,
            commands,
            command_wakers,
            state,
            read_buffer: VecDeque::new(),
            read_eof: false,
            write_shutdown: false,
        }
    }

    fn stream_error(&self) -> Option<io::Error> {
        match self.state.state.load(Ordering::Acquire) {
            STREAM_RESET => Some(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "stream reset",
            )),
            STREAM_GONE => Some(io::Error::new(io::ErrorKind::BrokenPipe, "carrier closed")),
            _ => None,
        }
    }

    fn read_error(&self) -> Option<io::Error> {
        match self.state.state.load(Ordering::Acquire) {
            STREAM_RESET => Some(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "stream reset",
            )),
            STREAM_GONE => Some(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "carrier closed",
            )),
            _ => None,
        }
    }

    fn wake_driver(&self) {
        let _ = self.commands.try_send(DriverCommand::Wake);
    }
}

impl AsyncRead for McpPublicStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        read_buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if let Some(error) = self.read_error() {
                self.read_buffer.clear();
                return Poll::Ready(Err(error));
            }
            if !self.read_buffer.is_empty() {
                let count = read_buf.remaining().min(self.read_buffer.len());
                let bytes: Vec<u8> = self.read_buffer.drain(..count).collect();
                read_buf.put_slice(&bytes);
                self.state.record_consumed(count);
                self.wake_driver();
                return Poll::Ready(Ok(()));
            }
            if self.read_eof {
                return Poll::Ready(Ok(()));
            }
            match Pin::new(&mut self.signals).poll_recv(context) {
                Poll::Ready(Some(StreamSignal::Data(bytes))) => self.read_buffer.extend(bytes),
                Poll::Ready(Some(StreamSignal::ReadEof)) => self.read_eof = true,
                Poll::Ready(Some(StreamSignal::Reset)) => {
                    self.read_buffer.clear();
                    self.state.state.store(STREAM_RESET, Ordering::Release);
                }
                Poll::Ready(Some(StreamSignal::Gone)) | Poll::Ready(None) => {
                    self.read_buffer.clear();
                    self.state.state.store(STREAM_GONE, Ordering::Release);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl AsyncWrite for McpPublicStream {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let Some(error) = this.stream_error() {
            return Poll::Ready(Err(error));
        }
        if this.write_shutdown {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stream writer closed",
            )));
        }
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let staged = this.state.reserve_write(bytes.len(), context.waker());
        if staged == 0 {
            return Poll::Pending;
        }
        match this.commands.try_send(DriverCommand::Write {
            stream_id: this.id,
            bytes: bytes[..staged].to_vec(),
        }) {
            Ok(()) => Poll::Ready(Ok(staged)),
            Err(mpsc::error::TrySendError::Full(_)) => {
                this.state.release_write(staged);
                this.command_wakers.register(this.id, context.waker());
                Poll::Pending
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                this.state.release_write(staged);
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "carrier closed",
                )))
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.stream_error()
            .map_or(Poll::Ready(Ok(())), |error| Poll::Ready(Err(error)))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(error) = this.stream_error() {
            return Poll::Ready(Err(error));
        }
        this.write_shutdown = true;
        this.state.close_requested.store(true, Ordering::Release);
        this.wake_driver();
        Poll::Ready(Ok(()))
    }
}

impl Drop for McpPublicStream {
    fn drop(&mut self) {
        if !self.write_shutdown {
            self.state.cancel_requested.store(true, Ordering::Release);
            self.wake_driver();
        }
    }
}

#[derive(Clone, Copy)]
struct MonotonicEpoch {
    wall_anchor: i64,
    monotonic_anchor: Instant,
}

impl MonotonicEpoch {
    fn new(wall_anchor: i64) -> Self {
        Self {
            wall_anchor,
            monotonic_anchor: Instant::now(),
        }
    }

    fn at(self, epoch_seconds: i64) -> Option<Instant> {
        let seconds = epoch_seconds.checked_sub(self.wall_anchor)?;
        if seconds <= 0 {
            return Some(self.monotonic_anchor);
        }
        self.monotonic_anchor
            .checked_add(Duration::from_secs(u64::try_from(seconds).ok()?))
    }
}

enum RenewalUpdate {
    Success(BridgeAuthority),
}

async fn run_renewal_fetcher(
    owner: McpEndpointOwnerContext,
    binding: BridgeBinding,
    mut current_expiry: i64,
    epoch: MonotonicEpoch,
    mut shutdown: watch::Receiver<bool>,
    updates: mpsc::Sender<RenewalUpdate>,
    mut advances: mpsc::Receiver<i64>,
) {
    loop {
        let Some(fetch_at) = epoch.at(current_expiry.saturating_sub(RENEW_FETCH_LEAD_SECONDS))
        else {
            return;
        };
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() { return; }
            }
            _ = sleep_until(fetch_at) => {}
        }
        let mut retries = 0u8;
        loop {
            let Some(expiry) = epoch.at(current_expiry) else {
                return;
            };
            if Instant::now() >= expiry || *shutdown.borrow() {
                return;
            }
            let result = refresh_mcp_bridge_authority(&owner, &mut shutdown).await;
            if let Ok(successor) = result
                && binding.accepts_successor(&successor, current_expiry)
            {
                if updates
                    .send(RenewalUpdate::Success(successor))
                    .await
                    .is_err()
                {
                    return;
                }
                match advances.recv().await {
                    Some(next_expiry) if next_expiry > current_expiry => {
                        current_expiry = next_expiry;
                        break;
                    }
                    _ => return,
                }
            }
            let cap = 1u64 << u32::from(retries.min(3));
            retries = retries.saturating_add(1);
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow_and_update() { return; }
                }
                _ = sleep(renewal_backoff_delay(cap.min(15))) => {}
            }
        }
    }
}

fn renewal_backoff_delay(cap_seconds: u64) -> Duration {
    let mut bytes = [0u8; 8];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        return Duration::from_secs(1);
    }
    Duration::from_secs(1 + u64::from_be_bytes(bytes) % cap_seconds.max(1))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the mux remains in one cancellation-owned task"
)]
async fn run_driver(
    carrier: TlsStream<tokio::net::TcpStream>,
    mut acceptor: MuxAcceptor,
    mut authority: BridgeAuthority,
    epoch: MonotonicEpoch,
    accepts: mpsc::Sender<McpPublicStream>,
    command_tx: mpsc::Sender<DriverCommand>,
    mut commands: mpsc::Receiver<DriverCommand>,
    command_wakers: Arc<CommandWakers>,
    mut external_shutdown: watch::Receiver<bool>,
    mut internal_shutdown: watch::Receiver<bool>,
    cancel: watch::Sender<bool>,
    mut renewals: mpsc::Receiver<RenewalUpdate>,
    advances: mpsc::Sender<i64>,
    proof_key: Arc<ring::signature::Ed25519KeyPair>,
) {
    let (mut reader, mut writer) = tokio::io::split(carrier);
    let mut streams = HashMap::<u32, DriverStream>::new();
    let mut pending_writes = HashMap::<u32, Vec<u8>>::new();
    let mut ready = VecDeque::<u32>::new();
    let mut closing = HashSet::<u32>::new();
    let mut control_open = false;
    let mut control_events = VecDeque::<Vec<u8>>::new();
    let mut control = ControlState {
        bytes: Vec::new(),
        deadline: None,
        response: None,
    };
    let mut successor = None;
    let mut bytes = [0u8; CARRIER_READ_BYTES];
    let Some(mut expiry) = epoch.at(authority.expires_at()) else {
        return;
    };

    'driver: loop {
        let control_deadline = control.deadline.unwrap_or(expiry);
        tokio::select! {
            changed = external_shutdown.changed() => {
                if changed.is_err() || *external_shutdown.borrow_and_update() { break; }
            }
            changed = internal_shutdown.changed() => {
                if changed.is_err() || *internal_shutdown.borrow_and_update() { break; }
            }
            _ = sleep_until(expiry) => break,
            _ = sleep_until(control_deadline), if control.deadline.is_some() => break,
            update = renewals.recv() => {
                let Some(RenewalUpdate::Success(candidate)) = update else { break; };
                if successor.is_some() || !authority.renewal_matches(&candidate) { break; }
                successor = Some(candidate);
            }
            read = reader.read(&mut bytes) => {
                let Ok(read) = read else { break; };
                let output = if read == 0 { acceptor.finish_eof() } else {
                    let Ok(output) = acceptor.feed(&bytes[..read]) else { break; };
                    output
                };
                if !write_output(output, &mut control_open, &mut streams, &accepts, &command_tx, &command_wakers, &mut control_events, &mut writer).await { break; }
                if read == 0 { break; }
            }
            command = commands.recv() => {
                let Some(command) = command else { break; };
                command_wakers.wake_all();
                match command {
                    DriverCommand::Write { stream_id, bytes } => {
                        if pending_writes.insert(stream_id, bytes).is_some() { break; }
                        ready.push_back(stream_id);
                    }
                    DriverCommand::Wake => {}
                    DriverCommand::CloseSession => break,
                }
            }
        }

        while let Some(control_bytes) = control_events.pop_front() {
            let challenge_at = epoch
                .at(authority
                    .expires_at()
                    .saturating_sub(RENEW_CHALLENGE_LEAD_SECONDS))
                .unwrap_or(expiry);
            if Instant::now() < challenge_at {
                break 'driver;
            }
            let frame = match control.append(control_bytes, expiry) {
                Ok(frame) => frame,
                Err(()) => break 'driver,
            };
            let Some(frame) = frame else {
                continue;
            };
            let Some(next) = successor.take() else {
                // No placeholder response and no later response to this challenge.
                control.deadline = None;
                continue;
            };
            let response = match authority.renewal_response(
                &next,
                proof_key.as_ref(),
                &frame,
                Utc::now().timestamp(),
            ) {
                Ok(response) => response,
                Err(_) => break 'driver,
            };
            control.response = Some((response, next));
        }

        if !flush_control_response(
            &mut acceptor,
            &mut authority,
            &mut expiry,
            epoch,
            &mut control,
            &advances,
            &mut control_open,
            &mut streams,
            &accepts,
            &command_tx,
            &command_wakers,
            &mut control_events,
            &mut writer,
        )
        .await
        {
            break;
        }

        if !process_stream_requests(
            &mut acceptor,
            &mut streams,
            &mut pending_writes,
            &mut ready,
            &mut closing,
            &mut control_open,
            &accepts,
            &command_tx,
            &command_wakers,
            &mut control_events,
            &mut writer,
        )
        .await
        {
            break;
        }
        if !flush_stream_signals(&mut streams) {
            break;
        }

        if !flush_ready(
            &mut acceptor,
            &mut streams,
            &mut pending_writes,
            &mut ready,
            &mut closing,
            &mut control_open,
            &accepts,
            &command_tx,
            &command_wakers,
            &mut control_events,
            &mut writer,
        )
        .await
        {
            break;
        }
    }
    cancel.send_replace(true);
    for (_, stream) in streams {
        stream.state.state.store(STREAM_GONE, Ordering::Release);
        let _ = stream.tx.try_send(StreamSignal::Gone);
    }
}

fn queue_signal(streams: &mut HashMap<u32, DriverStream>, stream_id: u32, signal: StreamSignal) {
    let Some(stream) = streams.get_mut(&stream_id) else {
        return;
    };
    if let StreamSignal::Data(bytes) = &signal
        && !stream.state.reserve_inbound(bytes.len())
    {
        stream.state.cancel_requested.store(true, Ordering::Release);
        return;
    }
    if stream.signals.is_empty() {
        match stream.tx.try_send(signal) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(signal)) => stream.signals.push_back(signal),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                stream.state.cancel_requested.store(true, Ordering::Release)
            }
        }
    } else {
        stream.signals.push_back(signal);
    }
}

fn flush_stream_signals(streams: &mut HashMap<u32, DriverStream>) -> bool {
    for stream in streams.values_mut() {
        while let Some(signal) = stream.signals.pop_front() {
            match stream.tx.try_send(signal) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(signal)) => {
                    stream.signals.push_front(signal);
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    stream.state.cancel_requested.store(true, Ordering::Release);
                    break;
                }
            }
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
async fn process_stream_requests<W: AsyncWrite + Unpin>(
    acceptor: &mut MuxAcceptor,
    streams: &mut HashMap<u32, DriverStream>,
    pending_writes: &mut HashMap<u32, Vec<u8>>,
    ready: &mut VecDeque<u32>,
    closing: &mut HashSet<u32>,
    control_open: &mut bool,
    accepts: &mpsc::Sender<McpPublicStream>,
    command_tx: &mpsc::Sender<DriverCommand>,
    command_wakers: &Arc<CommandWakers>,
    control_events: &mut VecDeque<Vec<u8>>,
    writer: &mut W,
) -> bool {
    let stream_ids: Vec<u32> = streams.keys().copied().collect();
    for stream_id in stream_ids {
        let Some(state) = streams
            .get(&stream_id)
            .map(|stream| Arc::clone(&stream.state))
        else {
            continue;
        };
        if state.cancel_requested.swap(false, Ordering::AcqRel) {
            if let Some(bytes) = pending_writes.remove(&stream_id) {
                state.release_write(bytes.len());
            }
            ready.retain(|id| *id != stream_id);
            closing.remove(&stream_id);
            let Ok(output) = acceptor.reset(stream_id, ResetReason::Cancel) else {
                return false;
            };
            state.state.store(STREAM_RESET, Ordering::Release);
            queue_signal(streams, stream_id, StreamSignal::Reset);
            if !write_output(
                output,
                control_open,
                streams,
                accepts,
                command_tx,
                command_wakers,
                control_events,
                writer,
            )
            .await
            {
                return false;
            }
            continue;
        }
        let consumed = state.take_consumed();
        if consumed > 0 {
            let Ok(output) = acceptor.consume(stream_id, consumed) else {
                return false;
            };
            if !write_output(
                output,
                control_open,
                streams,
                accepts,
                command_tx,
                command_wakers,
                control_events,
                writer,
            )
            .await
            {
                return false;
            }
        }
        if state.close_requested.load(Ordering::Acquire) && !pending_writes.contains_key(&stream_id)
        {
            closing.insert(stream_id);
            if !ready.contains(&stream_id) {
                ready.push_back(stream_id);
            }
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
async fn flush_control_response<W: AsyncWrite + Unpin>(
    acceptor: &mut MuxAcceptor,
    authority: &mut BridgeAuthority,
    expiry: &mut Instant,
    epoch: MonotonicEpoch,
    control: &mut ControlState,
    advances: &mpsc::Sender<i64>,
    control_open: &mut bool,
    streams: &mut HashMap<u32, DriverStream>,
    accepts: &mpsc::Sender<McpPublicStream>,
    command_tx: &mpsc::Sender<DriverCommand>,
    command_wakers: &Arc<CommandWakers>,
    control_events: &mut VecDeque<Vec<u8>>,
    writer: &mut W,
) -> bool {
    let Some((response, next)) = control.response.take() else {
        return true;
    };
    let output = match acceptor.try_send_data(CONTROL_STREAM_ID, response.clone()) {
        Ok(Some(output)) => output,
        Ok(None) => {
            control.response = Some((response, next));
            return true;
        }
        Err(_) => return false,
    };
    if !write_output(
        output,
        control_open,
        streams,
        accepts,
        command_tx,
        command_wakers,
        control_events,
        writer,
    )
    .await
    {
        return false;
    }
    let next_expiry = next.expires_at();
    let Some(next_deadline) = epoch.at(next_expiry) else {
        return false;
    };
    *authority = next;
    *expiry = next_deadline;
    control.deadline = None;
    advances.try_send(next_expiry).is_ok()
}

#[allow(clippy::too_many_arguments)]
async fn flush_ready<W: AsyncWrite + Unpin>(
    acceptor: &mut MuxAcceptor,
    streams: &mut HashMap<u32, DriverStream>,
    pending_writes: &mut HashMap<u32, Vec<u8>>,
    ready: &mut VecDeque<u32>,
    closing: &mut HashSet<u32>,
    control_open: &mut bool,
    accepts: &mpsc::Sender<McpPublicStream>,
    command_tx: &mpsc::Sender<DriverCommand>,
    command_wakers: &Arc<CommandWakers>,
    control_events: &mut VecDeque<Vec<u8>>,
    writer: &mut W,
) -> bool {
    let rounds = ready.len();
    for _ in 0..rounds {
        let Some(stream_id) = ready.pop_front() else {
            break;
        };
        if streams
            .get(&stream_id)
            .is_some_and(|stream| stream.state.state.load(Ordering::Acquire) != STREAM_LIVE)
        {
            if let Some(bytes) = pending_writes.remove(&stream_id)
                && let Some(stream) = streams.get(&stream_id)
            {
                stream.state.release_write(bytes.len());
            }
            closing.remove(&stream_id);
            continue;
        }
        let Some(bytes) = pending_writes.remove(&stream_id) else {
            if closing.remove(&stream_id) {
                let Ok(output) = acceptor.close_write(stream_id) else {
                    return false;
                };
                if !write_output(
                    output,
                    control_open,
                    streams,
                    accepts,
                    command_tx,
                    command_wakers,
                    control_events,
                    writer,
                )
                .await
                {
                    return false;
                }
            }
            continue;
        };
        match acceptor.try_send_data(stream_id, bytes.clone()) {
            Ok(Some(output)) => {
                if !write_output(
                    output,
                    control_open,
                    streams,
                    accepts,
                    command_tx,
                    command_wakers,
                    control_events,
                    writer,
                )
                .await
                {
                    return false;
                }
                if let Some(stream) = streams.get(&stream_id) {
                    stream.state.release_write(bytes.len());
                }
                if closing.remove(&stream_id) {
                    let Ok(output) = acceptor.close_write(stream_id) else {
                        return false;
                    };
                    if !write_output(
                        output,
                        control_open,
                        streams,
                        accepts,
                        command_tx,
                        command_wakers,
                        control_events,
                        writer,
                    )
                    .await
                    {
                        return false;
                    }
                }
            }
            Ok(None) => {
                pending_writes.insert(stream_id, bytes);
                ready.push_back(stream_id);
            }
            Err(_) => return false,
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
async fn write_output<W: AsyncWrite + Unpin>(
    output: MuxOutput,
    control_open: &mut bool,
    streams: &mut HashMap<u32, DriverStream>,
    accepts: &mpsc::Sender<McpPublicStream>,
    command_tx: &mpsc::Sender<DriverCommand>,
    command_wakers: &Arc<CommandWakers>,
    control_events: &mut VecDeque<Vec<u8>>,
    writer: &mut W,
) -> bool {
    if write_mux_frames(writer, &output.frames).await.is_err() {
        return false;
    }
    for event in output.events {
        match event {
            MuxEvent::Opened { stream_id } if !*control_open => {
                if stream_id != CONTROL_STREAM_ID {
                    return false;
                }
                *control_open = true;
            }
            MuxEvent::Opened { stream_id } => {
                if stream_id == CONTROL_STREAM_ID || streams.len() >= PUBLIC_STREAM_CAPACITY {
                    return false;
                }
                let (tx, rx) = mpsc::channel(STREAM_SIGNAL_CAPACITY);
                let state = Arc::new(StreamStatus::new());
                let handle = McpPublicStream::new(
                    stream_id,
                    rx,
                    command_tx.clone(),
                    Arc::clone(command_wakers),
                    Arc::clone(&state),
                );
                if accepts.try_send(handle).is_err() {
                    return false;
                }
                streams.insert(
                    stream_id,
                    DriverStream {
                        tx,
                        signals: VecDeque::new(),
                        state,
                    },
                );
            }
            MuxEvent::Data {
                stream_id: CONTROL_STREAM_ID,
                bytes,
            } => {
                let queued = control_events
                    .iter()
                    .try_fold(bytes.len(), |total, pending| {
                        total.checked_add(pending.len())
                    });
                if queued.is_none_or(|total| total > MAX_CONTROL_FRAME_BYTES) {
                    return false;
                }
                control_events.push_back(bytes);
            }
            MuxEvent::ReadClosed {
                stream_id: CONTROL_STREAM_ID,
            }
            | MuxEvent::Reset {
                stream_id: CONTROL_STREAM_ID,
                ..
            } => return false,
            MuxEvent::Data { stream_id, bytes } => {
                queue_signal(streams, stream_id, StreamSignal::Data(bytes))
            }
            MuxEvent::ReadClosed { stream_id } => {
                queue_signal(streams, stream_id, StreamSignal::ReadEof)
            }
            MuxEvent::Reset { stream_id, .. } => {
                if let Some(stream) = streams.get(&stream_id) {
                    stream.state.state.store(STREAM_RESET, Ordering::Release);
                }
                queue_signal(streams, stream_id, StreamSignal::Reset);
            }
            MuxEvent::PeerGone { .. } => {}
        }
    }
    true
}

async fn write_mux_frames<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frames: &[Frame],
) -> Result<(), McpBridgeCarrierError> {
    for frame in frames {
        let bytes = frame.encode().map_err(|_| McpBridgeCarrierError::Pop)?;
        writer
            .write_all(&bytes)
            .await
            .map_err(|_| McpBridgeCarrierError::Io)?;
    }
    writer.flush().await.map_err(|_| McpBridgeCarrierError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spl_core::frame::FLAG_OPEN;

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut bytes = u32::try_from(body.len())
            .expect("fixture length fits")
            .to_be_bytes()
            .to_vec();
        bytes.extend_from_slice(body);
        bytes
    }

    #[test]
    fn control_frame_is_fragment_bounded_and_has_no_trailing_bytes() {
        let expiry = Instant::now() + Duration::from_secs(30);
        let mut control = ControlState {
            bytes: Vec::new(),
            deadline: None,
            response: None,
        };
        let expected = frame(br#"{"nonce":"fixture"}"#);
        assert!(
            control
                .append(expected[..2].to_vec(), expiry)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            control.append(expected[2..].to_vec(), expiry).unwrap(),
            Some(expected)
        );
        assert!(control.deadline.is_some());
        assert!(
            control
                .append(vec![0; MAX_CONTROL_FRAME_BYTES + 1], expiry)
                .is_err()
        );
        assert!(control.append(vec![0, 0, 0, 1, 0, 1], expiry).is_err());
    }

    #[test]
    fn monotonic_expiry_mapping_uses_its_first_wall_sample_only() {
        let epoch = MonotonicEpoch::new(1_000);
        let expiry = epoch.at(1_120).expect("future expiry maps");
        let later = epoch.at(1_121).expect("later expiry maps");
        assert!(later > expiry);
        assert_eq!(epoch.at(999), Some(epoch.monotonic_anchor));
    }

    #[test]
    fn outbound_staging_never_exceeds_one_recommended_chunk() {
        let status = StreamStatus::new();
        let waker = Waker::noop();
        assert_eq!(
            status.reserve_write(RECOMMENDED_CHUNK + 1, &waker),
            RECOMMENDED_CHUNK
        );
        assert_eq!(status.reserve_write(1, &waker), 0);
        status.release_write(RECOMMENDED_CHUNK);
        assert_eq!(status.reserve_write(1, &waker), 1);
    }

    #[test]
    fn inbound_staging_never_exceeds_the_pinned_receive_window() {
        let status = StreamStatus::new();
        assert!(status.reserve_inbound(INITIAL_WINDOW));
        assert!(!status.reserve_inbound(1));
        status.record_consumed(1);
        assert!(status.reserve_inbound(1));
    }

    #[tokio::test]
    async fn reset_stream_discards_a_staged_write_before_any_data_frame() {
        let mut acceptor = MuxAcceptor::new(MuxLimits::default()).expect("default limits work");
        let (accept_tx, _accept_rx) = mpsc::channel(PUBLIC_STREAM_CAPACITY);
        let (command_tx, _command_rx) = mpsc::channel(DRIVER_COMMAND_CAPACITY);
        let command_wakers = Arc::new(CommandWakers {
            entries: Mutex::new(Vec::new()),
        });
        let state = Arc::new(StreamStatus::new());
        let waker = Waker::noop();
        assert_eq!(state.reserve_write(1, waker), 1);
        state.state.store(STREAM_RESET, Ordering::Release);
        let (tx, _rx) = mpsc::channel(STREAM_SIGNAL_CAPACITY);
        let mut streams = HashMap::from([(
            3,
            DriverStream {
                tx,
                signals: VecDeque::new(),
                state: Arc::clone(&state),
            },
        )]);
        let mut pending_writes = HashMap::from([(3, vec![7])]);
        let mut ready = VecDeque::from([3]);
        let mut closing = HashSet::new();
        let mut control_open = true;
        let mut control_events = VecDeque::new();
        let (mut writer, _reader) = tokio::io::duplex(1024);

        assert!(
            flush_ready(
                &mut acceptor,
                &mut streams,
                &mut pending_writes,
                &mut ready,
                &mut closing,
                &mut control_open,
                &accept_tx,
                &command_tx,
                &command_wakers,
                &mut control_events,
                &mut writer,
            )
            .await
        );
        assert!(pending_writes.is_empty());
        assert_eq!(state.outbound_staged.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn control_stream_one_is_internal_and_stream_three_is_the_first_public_handle() {
        let mut acceptor = MuxAcceptor::new(MuxLimits::default()).expect("default limits work");
        let (accept_tx, mut accept_rx) = mpsc::channel(PUBLIC_STREAM_CAPACITY);
        let (command_tx, _command_rx) = mpsc::channel(DRIVER_COMMAND_CAPACITY);
        let command_wakers = Arc::new(CommandWakers {
            entries: Mutex::new(Vec::new()),
        });
        let (mut writer, _reader) = tokio::io::duplex(1024);
        let mut control_open = false;
        let mut streams = HashMap::new();
        let mut control_events = VecDeque::new();

        let control = acceptor
            .feed(
                &Frame::new(CONTROL_STREAM_ID, FLAG_OPEN, Vec::new())
                    .encode()
                    .unwrap(),
            )
            .expect("control open parses");
        assert!(
            write_output(
                control,
                &mut control_open,
                &mut streams,
                &accept_tx,
                &command_tx,
                &command_wakers,
                &mut control_events,
                &mut writer,
            )
            .await
        );
        assert!(control_open);
        assert!(accept_rx.try_recv().is_err());

        let public = acceptor
            .feed(&Frame::new(3, FLAG_OPEN, Vec::new()).encode().unwrap())
            .expect("public open parses");
        assert!(
            write_output(
                public,
                &mut control_open,
                &mut streams,
                &accept_tx,
                &command_tx,
                &command_wakers,
                &mut control_events,
                &mut writer,
            )
            .await
        );
        assert_eq!(streams.len(), 1);
        let public = accept_rx.try_recv().expect("stream three is accepted");
        assert_eq!(public.id, 3);
    }
}
