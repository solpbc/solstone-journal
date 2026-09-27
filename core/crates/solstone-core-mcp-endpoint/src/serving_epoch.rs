// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Ephemeral serving epoch handle and synchronous close coordination.

use std::io;
use std::net::Shutdown;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::thread;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{oneshot, watch};

/// Census and abort diagnosis for one completed serving epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochCompletion {
    pub aborted_after_bound: Vec<&'static str>,
    pub blocked_calls_still_running: usize,
}

impl EpochCompletion {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.aborted_after_bound.is_empty() && self.blocked_calls_still_running == 0
    }
}

/// In-memory serving epoch with synchronous raise storage and parked socket shutdown thread.
pub struct ServingEpoch {
    #[allow(dead_code)]
    pub(crate) id: u64,
    pub(crate) closed: Arc<AtomicBool>,
    pub(crate) shutdown: watch::Sender<bool>,
    #[allow(dead_code)]
    pub(crate) shutdown_rx: watch::Receiver<bool>,
    sockets: Arc<Mutex<Vec<Option<std::net::TcpStream>>>>,
    thread: thread::Thread,
    pub(crate) blocked_calls: Arc<AtomicUsize>,
    pub(crate) aborted: Arc<Mutex<Vec<&'static str>>>,
    #[allow(dead_code)]
    completion_tx: Mutex<Option<oneshot::Sender<EpochCompletion>>>,
}

impl ServingEpoch {
    pub(crate) fn new(id: u64) -> (Arc<Self>, oneshot::Receiver<EpochCompletion>) {
        let closed = Arc::new(AtomicBool::new(false));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let sockets: Arc<Mutex<Vec<Option<std::net::TcpStream>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let (completion_tx, completion_rx) = oneshot::channel();
        let blocked_calls = Arc::new(AtomicUsize::new(0));
        let aborted = Arc::new(Mutex::new(Vec::new()));

        let thread_sockets = Arc::clone(&sockets);
        let thread_closed = Arc::clone(&closed);
        let parked_thread = thread::Builder::new()
            .name(format!("mcp-epoch-{id}-closer"))
            .spawn(move || {
                loop {
                    thread::park();
                    if thread_closed.load(Ordering::Acquire) {
                        let to_shutdown: Vec<Option<std::net::TcpStream>> = {
                            let Ok(mut guard) = thread_sockets.lock() else {
                                return;
                            };
                            std::mem::take(&mut *guard)
                        };
                        for socket in to_shutdown.into_iter().flatten() {
                            let _ = socket.shutdown(Shutdown::Both);
                        }
                        break;
                    }
                }
            })
            .expect("epoch socket thread spawns");

        let epoch = Arc::new(Self {
            id,
            closed,
            shutdown: shutdown_tx,
            shutdown_rx,
            sockets,
            thread: parked_thread.thread().clone(),
            blocked_calls,
            aborted,
            completion_tx: Mutex::new(Some(completion_tx)),
        });

        (epoch, completion_rx)
    }

    pub(crate) fn record_aborted(&self, label: &'static str) {
        if let Ok(mut guard) = self.aborted.lock()
            && !guard.contains(&label)
        {
            guard.push(label);
        }
    }

    /// Synchronously and idempotently raise the close signal across all holders.
    pub fn raise(&self) {
        if self.closed.swap(true, Ordering::Release) {
            return;
        }
        self.shutdown.send_replace(true);
        self.thread.unpark();
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Register a socket handle so the epoch's dedicated closer thread can shut it down on raise.
    pub(crate) fn register_tokio_socket(
        self: &Arc<Self>,
        socket: tokio::net::TcpStream,
    ) -> io::Result<(tokio::net::TcpStream, SocketRegistration)> {
        let std_stream = socket.into_std()?;
        std_stream.set_nonblocking(true)?;
        let clone = std_stream.try_clone()?;
        let tokio_stream = tokio::net::TcpStream::from_std(std_stream)?;

        let index = {
            let mut guard = self
                .sockets
                .lock()
                .map_err(|_| io::Error::other("epoch sockets lock poisoned"))?;
            let idx = guard.len();
            guard.push(Some(clone));
            idx
        };

        let guard = SocketRegistration {
            epoch: Arc::clone(self),
            index,
        };

        Ok((tokio_stream, guard))
    }

    #[allow(dead_code)]
    pub(crate) fn finish_completion(&self, completion: EpochCompletion) {
        if let Ok(mut slot) = self.completion_tx.lock()
            && let Some(sender) = slot.take()
        {
            let _ = sender.send(completion);
        }
    }
}

pub(crate) struct SocketRegistration {
    epoch: Arc<ServingEpoch>,
    index: usize,
}

impl Drop for SocketRegistration {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.epoch.sockets.lock()
            && self.index < guard.len()
        {
            guard[self.index] = None;
        }
    }
}

/// Endpoint door coordinating sequential serving epochs.
pub struct EndpointDoor {
    current_epoch: Mutex<Option<Arc<ServingEpoch>>>,
    current_attempt: Mutex<Option<Arc<AtomicBool>>>,
    next_id: AtomicU64,
}

impl Default for EndpointDoor {
    fn default() -> Self {
        Self::new()
    }
}

impl EndpointDoor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            current_epoch: Mutex::new(None),
            current_attempt: Mutex::new(None),
            next_id: AtomicU64::new(0),
        }
    }

    /// Return the active serving epoch handle if one is open.
    #[must_use]
    pub fn active_epoch(&self) -> Option<Arc<ServingEpoch>> {
        self.current_epoch
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn current_epoch_id(&self) -> u64 {
        self.next_id.load(Ordering::SeqCst)
    }

    /// Set the attempt flag for an in-flight acquire operation.
    #[allow(dead_code)]
    pub(crate) fn set_attempt_flag(&self, flag: Arc<AtomicBool>) {
        if let Ok(mut guard) = self.current_attempt.lock() {
            *guard = Some(flag);
        }
    }

    /// Clear the in-flight attempt flag.
    #[allow(dead_code)]
    pub(crate) fn clear_attempt_flag(&self) {
        if let Ok(mut guard) = self.current_attempt.lock() {
            *guard = None;
        }
    }

    /// Open a new epoch handle.
    pub(crate) fn open_epoch(&self) -> (Arc<ServingEpoch>, oneshot::Receiver<EpochCompletion>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (epoch, rx) = ServingEpoch::new(id);
        if let Ok(mut guard) = self.current_epoch.lock() {
            *guard = Some(Arc::clone(&epoch));
        }
        (epoch, rx)
    }

    /// Clear the current epoch handle from the door.
    #[allow(dead_code)]
    pub(crate) fn clear_epoch(&self) {
        if let Ok(mut guard) = self.current_epoch.lock() {
            *guard = None;
        }
    }

    /// Synchronously raise any active epoch and in-flight attempt flag.
    pub fn raise(&self) {
        let epoch = self
            .current_epoch
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let attempt = self
            .current_attempt
            .lock()
            .ok()
            .and_then(|guard| guard.clone());

        if let Some(flag) = attempt {
            flag.store(true, Ordering::Release);
        }
        if let Some(epoch) = epoch {
            epoch.raise();
        }
    }
}

/// An AsyncRead/AsyncWrite stream wrapper that counts write offers and blocks output once closed.
pub(crate) struct OfferGuardedStream<S> {
    inner: S,
    epoch_closed: Arc<AtomicBool>,
    watch: watch::Receiver<bool>,
    offers: Arc<AtomicUsize>,
}

impl<S> OfferGuardedStream<S> {
    pub(crate) fn new(
        inner: S,
        epoch_closed: Arc<AtomicBool>,
        watch: watch::Receiver<bool>,
        offers: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            inner,
            epoch_closed,
            watch,
            offers,
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn offers_handle(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.offers)
    }

    fn is_closed(&self) -> bool {
        self.epoch_closed.load(Ordering::Acquire) || *self.watch.borrow()
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for OfferGuardedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for OfferGuardedStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.is_closed() {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if !buf.is_empty() {
            self.offers.fetch_add(1, Ordering::Relaxed);
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.is_closed() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.is_closed() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
