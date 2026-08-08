// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

use crate::CallosumEnvelope;

use super::frame::encode_envelope;
use super::framing::{ReadFrame, read_frame, reader};
use super::{
    SERVER_BROADCAST_CAPACITY, SERVER_CLIENT_OUTBOUND_CAPACITY, SERVER_SEND_TIMEOUT,
    SERVER_STOP_JOIN_TIMEOUT,
};

/// Failure to bind a local Callosum socket server.
#[derive(Debug)]
pub enum CallosumSocketServerError {
    Io(std::io::Error),
}

impl fmt::Display for CallosumSocketServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "Callosum socket server unavailable: {error}"),
        }
    }
}

impl Error for CallosumSocketServerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
        }
    }
}

/// Async Unix-socket Callosum broadcast server.
pub struct CallosumSocketServer {
    inner: Arc<ServerInner>,
}

struct ServerInner {
    socket_path: PathBuf,
    broadcasts: mpsc::Sender<CallosumEnvelope>,
    clients: Mutex<HashMap<u64, ClientEntry>>,
    shutdown: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    next_client_id: AtomicU64,
    stopped: AtomicBool,
    malformed_frame_drops: AtomicU64,
    broadcast_saturation_drops: AtomicU64,
    stalled_client_evictions: AtomicU64,
    #[cfg(test)]
    hooks: Option<Arc<ServerTestHooks>>,
}

struct ClientEntry {
    outbound: mpsc::Sender<Vec<u8>>,
    shutdown: watch::Sender<bool>,
    // Dropping this detaches a client task that is already exiting after shutdown/removal.
    _task: JoinHandle<()>,
}

impl CallosumSocketServer {
    /// Remove a stale file, bind the socket, and begin accepting clients.
    pub async fn bind(socket_path: impl AsRef<Path>) -> Result<Self, CallosumSocketServerError> {
        Self::bind_inner(socket_path.as_ref().to_path_buf(), None).await
    }

    async fn bind_inner(
        socket_path: PathBuf,
        #[cfg(test)] hooks: Option<Arc<ServerTestHooks>>,
        #[cfg(not(test))] _hooks: Option<()>,
    ) -> Result<Self, CallosumSocketServerError> {
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent).map_err(CallosumSocketServerError::Io)?;
        }
        if socket_path.exists() {
            fs::remove_file(&socket_path).map_err(CallosumSocketServerError::Io)?;
        }
        let listener = UnixListener::bind(&socket_path).map_err(CallosumSocketServerError::Io)?;
        let (broadcasts, broadcast_rx) = mpsc::channel(SERVER_BROADCAST_CAPACITY);
        let (shutdown, _) = watch::channel(false);
        let inner = Arc::new(ServerInner {
            socket_path,
            broadcasts,
            clients: Mutex::new(HashMap::new()),
            shutdown,
            tasks: Mutex::new(Vec::new()),
            next_client_id: AtomicU64::new(1),
            stopped: AtomicBool::new(false),
            malformed_frame_drops: AtomicU64::new(0),
            broadcast_saturation_drops: AtomicU64::new(0),
            stalled_client_evictions: AtomicU64::new(0),
            #[cfg(test)]
            hooks,
        });
        track_task(
            &inner,
            tokio::spawn(run_dispatcher(Arc::clone(&inner), broadcast_rx)),
        );
        track_task(
            &inner,
            tokio::spawn(run_accept_loop(Arc::clone(&inner), listener)),
        );
        Ok(Self { inner })
    }

    /// Queue an envelope for broadcast. Success means queued, not delivered.
    pub fn broadcast(&self, mut envelope: CallosumEnvelope) -> bool {
        if self.inner.stopped.load(Ordering::Acquire) {
            return false;
        }
        stamp_timestamp(&mut envelope);
        match self.inner.broadcasts.try_send(envelope) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                if self
                    .inner
                    .broadcast_saturation_drops
                    .fetch_add(1, Ordering::AcqRel)
                    == 0
                {
                    eprintln!("callosum wire: broadcast queue saturated; dropping events");
                }
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Number of clients currently in the broadcast set.
    #[must_use]
    pub fn client_count(&self) -> usize {
        lock(&self.inner.clients).len()
    }

    /// Count malformed JSON, schema, and UTF-8 frames dropped by peer readers.
    #[must_use]
    pub fn malformed_frame_drops(&self) -> u64 {
        self.inner.malformed_frame_drops.load(Ordering::Acquire)
    }

    /// Count broadcasts rejected because the server's global queue was full.
    #[must_use]
    pub fn broadcast_saturation_drops(&self) -> u64 {
        self.inner
            .broadcast_saturation_drops
            .load(Ordering::Acquire)
    }

    /// Count clients evicted for a full outbound queue or timed-out write.
    #[must_use]
    pub fn stalled_client_evictions(&self) -> u64 {
        self.inner.stalled_client_evictions.load(Ordering::Acquire)
    }

    /// Stop immediately, dropping undelivered server and client output.
    pub async fn stop(&self) {
        if self.inner.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        // Unlike connection stop, server stop intentionally abandons every queued frame.
        let _ = self.inner.shutdown.send(true);
        let clients = std::mem::take(&mut *lock(&self.inner.clients));
        for entry in clients.into_values() {
            let _ = entry.shutdown.send(true);
        }

        let deadline = Instant::now() + SERVER_STOP_JOIN_TIMEOUT;
        let tasks = std::mem::take(&mut *lock(&self.inner.tasks));
        for mut task in tasks {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if timeout(remaining, &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
        let _ = fs::remove_file(&self.inner.socket_path);
    }

    #[cfg(test)]
    pub(crate) async fn bind_with_test_hooks(
        socket_path: impl AsRef<Path>,
        hooks: Arc<ServerTestHooks>,
    ) -> Result<Self, CallosumSocketServerError> {
        Self::bind_inner(socket_path.as_ref().to_path_buf(), Some(hooks)).await
    }
}

impl Drop for CallosumSocketServer {
    fn drop(&mut self) {
        if !self.inner.stopped.swap(true, Ordering::AcqRel) {
            let _ = self.inner.shutdown.send(true);
            let _ = fs::remove_file(&self.inner.socket_path);
        }
    }
}

async fn run_accept_loop(inner: Arc<ServerInner>, listener: UnixListener) {
    let mut shutdown = inner.shutdown.subscribe();
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                let _ = changed;
                return;
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    if inner.stopped.load(Ordering::Acquire) {
                        return;
                    }
                    let id = inner.next_client_id.fetch_add(1, Ordering::Relaxed);
                    start_client(Arc::clone(&inner), id, stream);
                }
                Err(_) => return,
            },
        }
    }
}

fn start_client(inner: Arc<ServerInner>, id: u64, stream: UnixStream) {
    let (outbound, outbound_rx) = mpsc::channel(SERVER_CLIENT_OUTBOUND_CAPACITY);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(run_client(
        Arc::clone(&inner),
        id,
        stream,
        outbound_rx,
        shutdown_rx,
    ));
    lock(&inner.clients).insert(
        id,
        ClientEntry {
            outbound,
            shutdown,
            _task: task,
        },
    );
}

async fn run_dispatcher(inner: Arc<ServerInner>, mut broadcasts: mpsc::Receiver<CallosumEnvelope>) {
    let mut shutdown = inner.shutdown.subscribe();
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                let _ = changed;
                return;
            }
            message = broadcasts.recv() => {
                let Some(message) = message else { return; };
                let Ok(line) = encode_envelope(&message) else { continue; };
                let clients: Vec<(u64, mpsc::Sender<Vec<u8>>)> = lock(&inner.clients)
                    .iter()
                    .map(|(id, entry)| (*id, entry.outbound.clone()))
                    .collect();
                for (id, outbound) in clients {
                    if matches!(outbound.try_send(line.clone()), Err(mpsc::error::TrySendError::Full(_))) {
                        evict_client(&inner, id);
                    }
                }
            }
        }
    }
}

async fn run_client(
    inner: Arc<ServerInner>,
    id: u64,
    stream: UnixStream,
    mut outbound: mpsc::Receiver<Vec<u8>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = reader(read_half);
    let mut buffer = Vec::new();
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                let _ = changed;
                break;
            }
            line = read_frame(&mut reader, &mut buffer) => match line {
                Ok(ReadFrame::Envelope(mut envelope)) => {
                    stamp_timestamp(&mut envelope);
                    let _ = queue_broadcast(&inner, envelope);
                }
                Ok(ReadFrame::Whitespace) => {}
                Ok(ReadFrame::Malformed) => record_malformed(&inner),
                Ok(ReadFrame::InvalidUtf8) => {
                    // Intentional deviation from Python: discard one bad frame to preserve bus stability.
                    record_malformed(&inner);
                }
                Ok(ReadFrame::Eof) | Err(_) => break,
            },
            line = outbound.recv() => match line {
                Some(line) => {
                    let result = timeout(SERVER_SEND_TIMEOUT, write_client_line(&inner, id, &mut write_half, &line)).await;
                    if !matches!(result, Ok(Ok(()))) {
                        evict_client(&inner, id);
                        break;
                    }
                }
                None => break,
            },
        }
    }
    remove_client(&inner, id);
}

async fn write_client_line(
    #[cfg_attr(not(test), allow(unused_variables))] inner: &ServerInner,
    #[cfg_attr(not(test), allow(unused_variables))] id: u64,
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
    line: &[u8],
) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(hooks) = &inner.hooks
        && hooks.blocked_client.load(Ordering::Acquire) == id
    {
        hooks.write_started.notify_waiters();
        std::future::pending::<()>().await;
    }
    write_half.write_all(line).await
}

fn queue_broadcast(inner: &ServerInner, envelope: CallosumEnvelope) -> bool {
    match inner.broadcasts.try_send(envelope) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            if inner
                .broadcast_saturation_drops
                .fetch_add(1, Ordering::AcqRel)
                == 0
            {
                eprintln!("callosum wire: broadcast queue saturated; dropping events");
            }
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    }
}

fn stamp_timestamp(envelope: &mut CallosumEnvelope) {
    if envelope.ts.is_none() {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as i64);
        envelope.ts = Some(millis);
    }
}

fn record_malformed(inner: &ServerInner) {
    let _ = inner.malformed_frame_drops.fetch_add(1, Ordering::AcqRel);
}

fn evict_client(inner: &ServerInner, id: u64) {
    let Some(entry) = lock(&inner.clients).remove(&id) else {
        return;
    };
    let _ = entry.shutdown.send(true);
    if inner
        .stalled_client_evictions
        .fetch_add(1, Ordering::AcqRel)
        == 0
    {
        eprintln!("callosum wire: evicting stalled client");
    }
}

fn remove_client(inner: &ServerInner, id: u64) {
    let _ = lock(&inner.clients).remove(&id);
}

fn track_task(inner: &ServerInner, task: JoinHandle<()>) {
    lock(&inner.tasks).push(task);
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
pub(crate) struct ServerTestHooks {
    blocked_client: AtomicU64,
    write_started: tokio::sync::Notify,
}

#[cfg(test)]
impl ServerTestHooks {
    pub(crate) fn block_client(&self, id: u64) {
        self.blocked_client.store(id, Ordering::Release);
    }

    pub(crate) async fn wait_for_write(&self) {
        self.write_started.notified().await;
    }
}

#[cfg(test)]
impl Default for ServerTestHooks {
    fn default() -> Self {
        Self {
            blocked_client: AtomicU64::new(0),
            write_started: tokio::sync::Notify::new(),
        }
    }
}
