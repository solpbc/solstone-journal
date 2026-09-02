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

#[cfg(windows)]
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

use crate::CallosumEnvelope;

use super::frame::encode_envelope;
use super::framing::{ReadFrame, read_frame, reader};
use super::{
    SERVER_BROADCAST_CAPACITY, SERVER_CLIENT_OUTBOUND_CAPACITY, SERVER_SEND_TIMEOUT,
    SERVER_STOP_JOIN_TIMEOUT,
};

#[cfg(unix)]
type ServerListener = UnixListener;
#[cfg(unix)]
type ServerStream = UnixStream;
#[cfg(unix)]
type ServerReadHalf = tokio::net::unix::OwnedReadHalf;
#[cfg(unix)]
type ServerWriteHalf = tokio::net::unix::OwnedWriteHalf;
#[cfg(windows)]
type ServerListener = interprocess::local_socket::tokio::Listener;
#[cfg(windows)]
type ServerStream = interprocess::local_socket::tokio::Stream;
#[cfg(windows)]
type ServerReadHalf = interprocess::local_socket::tokio::RecvHalf;
#[cfg(windows)]
type ServerWriteHalf = interprocess::local_socket::tokio::SendHalf;

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

/// Async Callosum local-transport broadcast server.
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
    unauthenticated_connection_drops: AtomicU64,
    #[cfg(windows)]
    pipe_secret: [u8; crate::windows::PIPE_CHALLENGE_LEN],
    #[cfg(any(test, feature = "test-hooks"))]
    hooks: Option<Arc<ServerTestHooks>>,
}

struct ClientEntry {
    outbound: mpsc::Sender<Vec<u8>>,
    shutdown: watch::Sender<bool>,
    // Dropping this detaches a client task that is already exiting after shutdown/removal.
    _task: JoinHandle<()>,
}

impl CallosumSocketServer {
    /// Bind the platform endpoint and begin accepting clients.
    pub async fn bind(socket_path: impl AsRef<Path>) -> Result<Self, CallosumSocketServerError> {
        Self::bind_inner(socket_path.as_ref().to_path_buf(), None).await
    }

    async fn bind_inner(
        socket_path: PathBuf,
        #[cfg(any(test, feature = "test-hooks"))] hooks: Option<Arc<ServerTestHooks>>,
        #[cfg(not(any(test, feature = "test-hooks")))] _hooks: Option<()>,
    ) -> Result<Self, CallosumSocketServerError> {
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent).map_err(CallosumSocketServerError::Io)?;
        }
        #[cfg(unix)]
        if socket_path.exists() {
            fs::remove_file(&socket_path).map_err(CallosumSocketServerError::Io)?;
        }
        #[cfg(unix)]
        let listener = UnixListener::bind(&socket_path).map_err(CallosumSocketServerError::Io)?;
        #[cfg(windows)]
        let (listener, pipe_secret) =
            bind_windows_listener(&socket_path).map_err(CallosumSocketServerError::Io)?;
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
            unauthenticated_connection_drops: AtomicU64::new(0),
            #[cfg(windows)]
            pipe_secret,
            #[cfg(any(test, feature = "test-hooks"))]
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
                    log::warn!("callosum wire: broadcast queue saturated; dropping events");
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

    /// Count named-pipe peers rejected before they join the Callosum broadcast set.
    #[must_use]
    pub fn unauthenticated_connection_drops(&self) -> u64 {
        self.inner
            .unauthenticated_connection_drops
            .load(Ordering::Acquire)
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
        cleanup_endpoint(&self.inner.socket_path);
    }

    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub async fn bind_with_test_hooks(
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
            let clients = std::mem::take(&mut *lock(&self.inner.clients));
            for entry in clients.into_values() {
                let _ = entry.shutdown.send(true);
            }
            cleanup_endpoint(&self.inner.socket_path);
        }
    }
}

#[cfg(unix)]
async fn run_accept_loop(inner: Arc<ServerInner>, listener: ServerListener) {
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

#[cfg(windows)]
async fn run_accept_loop(inner: Arc<ServerInner>, listener: ServerListener) {
    use interprocess::local_socket::traits::{StreamCommon as _, tokio::Listener as _};

    let mut shutdown = inner.shutdown.subscribe();
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                let _ = changed;
                return;
            }
            accepted = listener.accept() => match accepted {
                Ok(mut stream) => {
                    if inner.stopped.load(Ordering::Acquire) {
                        return;
                    }
                    let peer_pid = stream.peer_creds().ok().and_then(|credentials| credentials.pid());
                    if !authenticate_windows_peer(&mut stream, &inner.pipe_secret, peer_pid).await {
                        record_unauthenticated_peer(&inner, peer_pid);
                        continue;
                    }
                    let id = inner.next_client_id.fetch_add(1, Ordering::Relaxed);
                    start_client(Arc::clone(&inner), id, stream);
                }
                Err(_) => return,
            },
        }
    }
}

fn start_client(inner: Arc<ServerInner>, id: u64, stream: ServerStream) {
    let (outbound, outbound_rx) = mpsc::channel(SERVER_CLIENT_OUTBOUND_CAPACITY);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let (registered, registered_rx) = oneshot::channel();
    let task = tokio::spawn(run_client(
        Arc::clone(&inner),
        id,
        stream,
        outbound_rx,
        shutdown_rx,
        registered_rx,
    ));
    lock(&inner.clients).insert(
        id,
        ClientEntry {
            outbound,
            shutdown,
            _task: task,
        },
    );
    // The peer may have already written a frame by the time accept returns.
    // Do not let its reader process that frame until this client is in the
    // broadcast map, otherwise a response can race past its own subscriber.
    let _ = registered.send(());
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
    stream: ServerStream,
    mut outbound: mpsc::Receiver<Vec<u8>>,
    mut shutdown: watch::Receiver<bool>,
    registered: oneshot::Receiver<()>,
) {
    if registered.await.is_err() {
        return;
    }
    let (read_half, mut write_half) = split_stream(stream);
    let mut reader = reader(read_half);
    let mut buffer = Vec::new();
    loop {
        tokio::select! {
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
    #[cfg_attr(not(any(test, feature = "test-hooks")), allow(unused_variables))]
    inner: &ServerInner,
    #[cfg_attr(not(any(test, feature = "test-hooks")), allow(unused_variables))] id: u64,
    write_half: &mut ServerWriteHalf,
    line: &[u8],
) -> std::io::Result<()> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(hooks) = &inner.hooks
        && hooks.blocked_client.load(Ordering::Acquire) == id
    {
        hooks.write_started.notify_waiters();
        std::future::pending::<()>().await;
    }
    write_half.write_all(line).await
}

#[cfg(unix)]
fn split_stream(stream: ServerStream) -> (ServerReadHalf, ServerWriteHalf) {
    stream.into_split()
}

#[cfg(windows)]
fn split_stream(stream: ServerStream) -> (ServerReadHalf, ServerWriteHalf) {
    use interprocess::local_socket::traits::tokio::Stream as _;

    stream.split()
}

#[cfg(unix)]
fn cleanup_endpoint(socket_path: &Path) {
    let _ = fs::remove_file(socket_path);
}

#[cfg(windows)]
fn cleanup_endpoint(_: &Path) {
    // Named-pipe lifetime is tied to listener and stream handles; there is no socket file to unlink.
}

#[cfg(windows)]
fn bind_windows_listener(
    socket_path: &Path,
) -> std::io::Result<(ServerListener, [u8; crate::windows::PIPE_CHALLENGE_LEN])> {
    use interprocess::local_socket::{ListenerOptions, ToFsName};
    use interprocess::os::windows::local_socket::{ListenerOptionsExt as _, NamedPipe};

    let secret = crate::windows::create_or_read_secret(socket_path)?;
    let pipe_name = crate::windows::pipe_name(socket_path)?;
    let descriptor = current_user_pipe_security_descriptor()?;
    let name = pipe_name.to_fs_name::<NamedPipe>()?;
    // interprocess 2.4.3 source proof:
    // src/os/windows/named_pipe/local_socket/tokio/listener.rs:23-28 starts with
    // PipeListenerOptions::new(); src/os/windows/named_pipe/listener/options.rs:81-94 defaults
    // accept_remote to false; src/os/windows/named_pipe/listener/create_instance.rs:93-103 ORs
    // PIPE_REJECT_REMOTE_CLIENTS when that field is false. The high-level API has no setter.
    let listener = ListenerOptions::new()
        .name(name)
        .security_descriptor(descriptor)
        .create_tokio()?;
    Ok((listener, secret))
}

#[cfg(windows)]
fn current_user_pipe_sddl() -> std::io::Result<String> {
    // This current-user-SID DACL, with remote clients rejected at listener creation, protects
    // cross-user/cross-identity and remote-network access—not same-SID malware, which is out of scope.
    let sid = crate::windows::sid::current_user_sid()?;
    Ok(format!("O:{sid}D:P(A;;GA;;;{sid})"))
}

#[cfg(windows)]
fn current_user_pipe_security_descriptor()
-> std::io::Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    use widestring::U16CString;

    let sddl = current_user_pipe_sddl()?;
    let sddl = U16CString::from_str(&sddl).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Callosum SDDL contains NUL",
        )
    })?;
    interprocess::os::windows::security_descriptor::SecurityDescriptor::deserialize(sddl.as_ucstr())
}

#[cfg(windows)]
async fn authenticate_windows_peer(
    stream: &mut ServerStream,
    secret: &[u8; crate::windows::PIPE_CHALLENGE_LEN],
    peer_pid: Option<u32>,
) -> bool {
    use tokio::time::timeout;

    let mut challenge = [0_u8; crate::windows::PIPE_CHALLENGE_LEN];
    if getrandom::fill(&mut challenge).is_err() {
        return false;
    }
    let greeting = crate::windows::server_greeting(challenge);
    let mut proof = [0_u8; crate::windows::PIPE_HANDSHAKE_LEN];
    let result = timeout(SERVER_SEND_TIMEOUT, async {
        stream.write_all(&greeting).await?;
        stream.read_exact(&mut proof).await
    })
    .await;
    matches!(result, Ok(Ok(_))) && windows_peer_is_admitted(secret, &greeting, &proof, peer_pid)
}

#[cfg(windows)]
fn windows_peer_is_admitted(
    secret: &[u8; crate::windows::PIPE_CHALLENGE_LEN],
    greeting: &[u8; crate::windows::PIPE_HANDSHAKE_LEN],
    proof: &[u8; crate::windows::PIPE_HANDSHAKE_LEN],
    _peer_pid: Option<u32>,
) -> bool {
    // PID is only optional telemetry. Admission is strictly the nonce/HMAC result.
    crate::windows::verify_client_proof(secret, greeting, proof)
}

#[cfg(windows)]
fn record_unauthenticated_peer(inner: &ServerInner, peer_pid: Option<u32>) {
    let count = inner
        .unauthenticated_connection_drops
        .fetch_add(1, Ordering::AcqRel)
        .saturating_add(1);
    if count.is_power_of_two() {
        log::warn!(
            "callosum wire: rejected unauthenticated named-pipe peer; count={count}, peer_pid={peer_pid:?}"
        );
    }
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
                log::warn!("callosum wire: broadcast queue saturated; dropping events");
            }
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    }
}

pub(crate) fn stamp_timestamp(envelope: &mut CallosumEnvelope) {
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
        log::warn!("callosum wire: evicting stalled client");
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

#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub struct ServerTestHooks {
    blocked_client: AtomicU64,
    write_started: tokio::sync::Notify,
}

#[cfg(any(test, feature = "test-hooks"))]
impl ServerTestHooks {
    pub fn block_client(&self, id: u64) {
        self.blocked_client.store(id, Ordering::Release);
    }

    pub async fn wait_for_write(&self) {
        self.write_started.notified().await;
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl Default for ServerTestHooks {
    fn default() -> Self {
        Self {
            blocked_client: AtomicU64::new(0),
            write_started: tokio::sync::Notify::new(),
        }
    }
}

#[cfg(all(test, windows))]
mod windows_native_tests {
    #![cfg(windows)]

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use interprocess::local_socket::traits::StreamCommon as _;
    use interprocess::local_socket::{ConnectOptions, ToFsName};
    use interprocess::os::windows::local_socket::NamedPipe;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{sleep, timeout};

    use super::{
        CallosumSocketServer, current_user_pipe_sddl, current_user_pipe_security_descriptor,
        windows_peer_is_admitted,
    };
    use crate::CallosumOneShotSender;
    use crate::windows::{
        PIPE_CHALLENGE_LEN, PIPE_HANDSHAKE_LEN, client_proof, pipe_name, read_secret,
        server_greeting,
    };

    fn socket_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-callosum-windows-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("health")).expect("create Callosum test health directory");
        path.join("health").join("callosum.sock")
    }

    fn remove_socket_parent(socket_path: &Path) {
        let Some(health) = socket_path.parent() else {
            return;
        };
        let Some(root) = health.parent() else {
            return;
        };
        let _ = fs::remove_dir_all(root);
    }

    async fn connect_authenticated(
        socket_path: &Path,
    ) -> interprocess::local_socket::tokio::Stream {
        let name = pipe_name(socket_path)
            .expect("derive Windows pipe name")
            .to_fs_name::<NamedPipe>()
            .expect("parse Windows pipe name");
        let mut stream = ConnectOptions::new()
            .name(name)
            .connect_tokio()
            .await
            .expect("connect test pipe");
        let secret = read_secret(socket_path).expect("read test secret");
        let mut greeting = [0_u8; PIPE_HANDSHAKE_LEN];
        stream
            .read_exact(&mut greeting)
            .await
            .expect("read server greeting");
        let proof = client_proof(&secret, &greeting).expect("construct client proof");
        stream.write_all(&proof).await.expect("write client proof");
        stream
    }

    async fn wait_for_clients(server: &CallosumSocketServer, count: usize) {
        timeout(Duration::from_secs(2), async {
            while server.client_count() != count {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("server client count reached expected value");
    }

    #[test]
    fn windows_native_literal_sid_descriptor_constructs_current_user_only_does_not_cover_two_windows_identity_denial()
     {
        let sddl = current_user_pipe_sddl().expect("resolve current user SID");
        assert!(sddl.starts_with("O:S-1-"));
        assert!(sddl.contains("D:P(A;;GA;;;S-1-"));
        assert!(!sddl.contains(";;;OW"));
        current_user_pipe_security_descriptor().expect("deserialize protected literal-SID DACL");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn windows_native_first_instance_collision_does_not_cover_listener_handle_noninheritance()
    {
        let socket = socket_path("collision");
        let first = CallosumSocketServer::bind(&socket)
            .await
            .expect("bind first named-pipe listener");
        assert!(CallosumSocketServer::bind(&socket).await.is_err());
        first.stop().await;
        remove_socket_parent(&socket);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn windows_native_peer_pid_retrieval_is_telemetry_only_and_does_not_cover_remote_client_rejection()
     {
        let socket = socket_path("peer-pid");
        let server = CallosumSocketServer::bind(&socket)
            .await
            .expect("bind named-pipe listener");
        let stream = connect_authenticated(&socket).await;
        assert!(
            stream
                .peer_creds()
                .expect("retrieve peer credentials")
                .pid()
                .is_some()
        );
        wait_for_clients(&server, 1).await;
        drop(stream);
        server.stop().await;
        remove_socket_parent(&socket);
    }

    #[test]
    fn windows_native_implausible_peer_pid_with_valid_hmac_is_admitted_and_does_not_cover_two_windows_identity_denial()
     {
        let secret = [7_u8; PIPE_CHALLENGE_LEN];
        let greeting = server_greeting([9_u8; PIPE_CHALLENGE_LEN]);
        let proof = client_proof(&secret, &greeting).expect("construct valid proof");
        assert!(windows_peer_is_admitted(
            &secret,
            &greeting,
            &proof,
            Some(u32::MAX)
        ));
    }

    #[test]
    fn windows_native_plausible_peer_pid_with_invalid_hmac_is_rejected_and_does_not_cover_two_windows_identity_denial()
     {
        let secret = [7_u8; PIPE_CHALLENGE_LEN];
        let greeting = server_greeting([9_u8; PIPE_CHALLENGE_LEN]);
        let mut proof = client_proof(&secret, &greeting).expect("construct valid proof");
        proof[5] ^= 1;
        assert!(!windows_peer_is_admitted(
            &secret,
            &greeting,
            &proof,
            Some(1)
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn windows_native_live_pipe_handshake_admits_valid_hmac_and_does_not_cover_remote_client_rejection()
     {
        let socket = socket_path("handshake");
        let server = CallosumSocketServer::bind(&socket)
            .await
            .expect("bind named-pipe listener");
        let stream = connect_authenticated(&socket).await;
        wait_for_clients(&server, 1).await;
        drop(stream);
        server.stop().await;
        remove_socket_parent(&socket);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn windows_native_one_shot_round_trip_does_not_cover_listener_handle_noninheritance() {
        let socket = socket_path("one-shot");
        let server = CallosumSocketServer::bind(&socket)
            .await
            .expect("bind named-pipe listener");
        let mut observer = connect_authenticated(&socket).await;
        wait_for_clients(&server, 1).await;
        let sender = CallosumOneShotSender::new(&socket, Duration::from_secs(2));
        tokio::task::spawn_blocking(move || {
            sender.send_line("{\"tract\":\"windows\",\"event\":\"one-shot\"}\n")
        })
        .await
        .expect("join one-shot sender")
        .expect("send one-shot line");
        let mut line = [0_u8; 256];
        let received = timeout(Duration::from_secs(2), observer.read(&mut line))
            .await
            .expect("read one-shot broadcast")
            .expect("one-shot broadcast read succeeds");
        assert!(
            std::str::from_utf8(&line[..received])
                .expect("broadcast is UTF-8")
                .contains("\"event\":\"one-shot\"")
        );
        drop(observer);
        server.stop().await;
        remove_socket_parent(&socket);
    }
}
