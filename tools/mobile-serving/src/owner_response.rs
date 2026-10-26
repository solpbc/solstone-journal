// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The embedding app owns both ends of each response pipe. No network listener.
//! A streamed upload uses a second pipe for request bytes. The control message
//! stays small; the file is not copied into it.

use axum::{
    body::{Body, Bytes},
    http::Request,
    response::Response,
};
use http_body_util::BodyExt;
use solstone_core_convey_shell::EmbeddedConveyHandle;
use solstone_core_journal_io::JournalRoot;
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tokio::{io::unix::AsyncFd, runtime::Handle, sync::watch, task::JoinHandle};

pub const UPLOAD_CHUNK_BYTES: usize = 64 * 1024;
pub const UPLOAD_TRANSFER_BYTES: usize = 256 * 1024;
pub const UPLOAD_AGGREGATE_BYTES: usize = 8 * 1024 * 1024;
const UPLOAD_STALL_MS: u64 = 60_000;
pub const UPLOAD_STALL_MESSAGE: &str =
    "the transfer stopped before it finished. check the result before trying again.";

static NEXT_OWNER_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// One process-wide identity for an ordinary owner read or a streamed upload.
/// A folder copy does not take an id from here.
pub fn next_owner_request_id() -> u64 {
    loop {
        let id = NEXT_OWNER_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StallWait {
    Open,
    Cancelled,
    Stalled,
}

/// Progress resets the input deadline. Retire it once the body is closed:
/// time spent saving after EOF is not a stall. Cancel still wins.
#[derive(Clone, Copy, Debug)]
pub struct StallDeadline {
    progress_at: u64,
    limit: u64,
    cancelled: bool,
    retired: bool,
}

impl StallDeadline {
    pub fn arm(now: u64, limit: u64) -> Self {
        Self {
            progress_at: now,
            limit,
            cancelled: false,
            retired: false,
        }
    }

    pub fn note_progress(&mut self, now: u64) {
        if !self.cancelled && !self.retired {
            self.progress_at = now;
        }
    }

    /// The body has been handed off. Further silence is save work, not a stall.
    pub fn retire(&mut self) {
        self.retired = true;
    }

    #[cfg(test)]
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    pub fn poll(&self, now: u64) -> StallWait {
        if self.cancelled {
            return StallWait::Cancelled;
        }
        if self.retired {
            return StallWait::Open;
        }
        if now.saturating_sub(self.progress_at) >= self.limit {
            return StallWait::Stalled;
        }
        StallWait::Open
    }
}

pub fn admit_chunk(len: usize) -> Result<(), &'static str> {
    if len > UPLOAD_CHUNK_BYTES {
        Err("chunk exceeds transfer budget")
    } else {
        Ok(())
    }
}

pub struct OwnerResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

struct ResponseTask {
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
}

struct BudgetGuard {
    budget: Arc<Mutex<usize>>,
    bytes: usize,
}

impl Drop for BudgetGuard {
    fn drop(&mut self) {
        let mut held = self
            .budget
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *held = held.saturating_sub(self.bytes);
    }
}

pub(crate) struct OwnerResponses {
    tasks: Mutex<HashMap<u64, ResponseTask>>,
    budget: Arc<Mutex<usize>>,
}

impl Default for OwnerResponses {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            budget: Arc::new(Mutex::new(0)),
        }
    }
}

struct Admitted {
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: Vec<(String, String)>,
}

impl Admitted {
    fn into_request(self, body: Body) -> Result<Request<Body>, String> {
        let mut request = Request::builder().method(self.method).uri(self.uri);
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        request.body(body).map_err(|error| error.to_string())
    }
}

fn admit(method: &str, uri: &str, headers: &[(String, String)]) -> Result<Admitted, String> {
    if !matches!(
        method,
        "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
    ) || uri.len() > 8192
        || headers
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum::<usize>()
            > 16 * 1024
    {
        return Err("bounded owner request required".into());
    }
    let uri: axum::http::Uri = uri
        .parse()
        .map_err(|error: axum::http::uri::InvalidUri| error.to_string())?;
    if uri.scheme().is_some()
        || uri.authority().is_some()
        || !uri.path().starts_with('/')
        || uri.path().starts_with("//")
    {
        return Err("owner request requires an origin-form path".into());
    }
    for (key, _) in headers {
        if !matches!(
            key.to_ascii_lowercase().as_str(),
            "content-type"
                | "accept"
                | "range"
                | "if-none-match"
                | "if-modified-since"
                | "last-event-id"
        ) {
            return Err("owner request header not admitted".into());
        }
    }
    let method = method
        .parse()
        .map_err(|error: axum::http::method::InvalidMethod| error.to_string())?;
    Ok(Admitted {
        method,
        uri,
        headers: headers.to_vec(),
    })
}

/// Streaming uploads use the same origin admission as a buffered request and do
/// not apply the buffered body cap.
fn upload_admission(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
) -> Result<Admitted, String> {
    admit(method, uri, headers)
}

fn request(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Result<Request<Body>, String> {
    if body.len() > 256 * 1024 {
        return Err("bounded owner request required".into());
    }
    admit(method, uri, headers)?.into_request(Body::from(body))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
}

fn cap_pipe(file: &File) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::fd::AsRawFd;
        // Linux F_GETPIPE_SZ/F_SETPIPE_SZ. An already bounded pipe does not
        // need resize permission; only shrink one that exceeds the budget.
        let capacity = unsafe { fcntl(file.as_raw_fd(), 1032, 0) };
        if capacity <= 0 {
            return Err("owner upload pipe unavailable".into());
        }
        if capacity as usize > UPLOAD_CHUNK_BYTES {
            let resized = unsafe { fcntl(file.as_raw_fd(), 1031, UPLOAD_CHUNK_BYTES as i32) };
            if resized <= 0 || resized as usize > UPLOAD_CHUNK_BYTES {
                return Err("owner upload pipe unavailable".into());
            }
        }
    }
    let _ = file;
    Ok(())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

async fn write_response_body(
    sink: &AsyncFd<File>,
    body: &mut Body,
    stop: &mut watch::Receiver<bool>,
    cancelled: &mut watch::Receiver<bool>,
) {
    loop {
        let frame = tokio::select! {
            biased;
            _ = stop.changed() => return,
            _ = cancelled.changed() => return,
            frame = body.frame() => frame,
        };
        let Some(Ok(frame)) = frame else {
            return;
        };
        let Ok(bytes) = frame.into_data() else {
            continue;
        };
        let mut remaining = bytes.as_ref();
        while !remaining.is_empty() {
            let ready = tokio::select! {
                biased;
                _ = stop.changed() => return,
                _ = cancelled.changed() => return,
                ready = sink.writable() => ready,
            };
            let Ok(mut ready) = ready else {
                return;
            };
            match ready.try_io(|sink| sink.get_ref().write(remaining)) {
                Ok(Ok(0)) | Ok(Err(_)) => return,
                Ok(Ok(written)) => remaining = &remaining[written..],
                Err(_) => continue,
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive_upload(
    router: Arc<EmbeddedConveyHandle>,
    root: Arc<JournalRoot>,
    request: Request<Body>,
    body_fd: AsyncFd<File>,
    sink: AsyncFd<File>,
    mut sender: Option<http_body_util::channel::Sender<Bytes, std::io::Error>>,
    head: mpsc::Sender<Result<OwnerResponseHead, String>>,
    mut stop: watch::Receiver<bool>,
    mut cancelled: watch::Receiver<bool>,
) {
    if let Err(error) = root.revalidate_canonical_binding() {
        let _ = head.send(Err(error.to_string()));
        return;
    }
    let started = Instant::now();
    let mut stall = StallDeadline::arm(0, UPLOAD_STALL_MS);
    let request_fut = router.request_owner(request);
    tokio::pin!(request_fut);
    let mut buffer = vec![0u8; UPLOAD_CHUNK_BYTES];
    let mut pending: Option<Bytes> = None;
    let mut finished = false;
    loop {
        if *cancelled.borrow() || *stop.borrow() {
            return;
        }
        let now = elapsed_ms(started);
        if body_handed_off(finished, pending.is_some()) {
            sender.take();
            stall.retire();
        } else if stall.poll(now) == StallWait::Stalled {
            if let Some(sender) = sender.take() {
                sender.abort(std::io::Error::new(std::io::ErrorKind::TimedOut, "stalled"));
            }
            let _ = head.send(Err(UPLOAD_STALL_MESSAGE.to_owned()));
            return;
        }
        let remaining = stall
            .progress_at
            .saturating_add(stall.limit)
            .saturating_sub(elapsed_ms(started))
            .max(1);
        let sleep = tokio::time::sleep(Duration::from_millis(remaining));
        tokio::pin!(sleep);
        let step = if pending.is_some() {
            tokio::select! {
                biased;
                _ = cancelled.changed() => UploadStep::Cancel,
                _ = stop.changed() => UploadStep::Cancel,
                response = &mut request_fut => UploadStep::Response(response),
                result = send_pending(sender.as_mut(), pending.clone()) => UploadStep::Sent(result),
                _ = &mut sleep => UploadStep::Stall,
            }
        } else if !finished {
            tokio::select! {
                biased;
                _ = cancelled.changed() => UploadStep::Cancel,
                _ = stop.changed() => UploadStep::Cancel,
                response = &mut request_fut => UploadStep::Response(response),
                outcome = read_available(&body_fd, &mut buffer) => outcome,
                _ = &mut sleep => UploadStep::Stall,
            }
        } else {
            tokio::select! {
                biased;
                _ = cancelled.changed() => UploadStep::Cancel,
                _ = stop.changed() => UploadStep::Cancel,
                response = &mut request_fut => UploadStep::Response(response),
            }
        };
        match step {
            UploadStep::Cancel => return,
            UploadStep::Again => {}
            UploadStep::Stall => {
                if let Some(sender) = sender.take() {
                    sender.abort(std::io::Error::new(std::io::ErrorKind::TimedOut, "stalled"));
                }
                let _ = head.send(Err(UPLOAD_STALL_MESSAGE.to_owned()));
                return;
            }
            UploadStep::Sent(Ok(())) => {
                pending = None;
                stall.note_progress(elapsed_ms(started));
            }
            UploadStep::Sent(Err(())) => return,
            UploadStep::Eof => {
                finished = true;
            }
            UploadStep::Chunk(bytes) => match admit_chunk(bytes.len()) {
                Ok(()) => {
                    pending = Some(bytes);
                    stall.note_progress(elapsed_ms(started));
                }
                Err(error) => {
                    if let Some(sender) = sender.take() {
                        sender.abort(std::io::Error::new(std::io::ErrorKind::InvalidData, error));
                    }
                    let _ = head.send(Err(error.to_owned()));
                    return;
                }
            },
            UploadStep::Response(response) => {
                if let Some(sender) = sender.take() {
                    sender.abort(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "closed",
                    ));
                }
                deliver_response(response, &sink, &head, &mut stop, &mut cancelled).await;
                return;
            }
        }
    }
}

/// Input is finished only when the last read returned EOF and no chunk is
/// still waiting to be written. Hashing that body is not an input stall.
fn body_handed_off(finished: bool, pending: bool) -> bool {
    finished && !pending
}

enum UploadStep {
    Cancel,
    Again,
    Stall,
    Eof,
    Chunk(Bytes),
    Sent(Result<(), ()>),
    Response(Response),
}

async fn read_available(body_fd: &AsyncFd<File>, buffer: &mut [u8]) -> UploadStep {
    let Ok(mut ready) = body_fd.readable().await else {
        return UploadStep::Cancel;
    };
    match ready.try_io(|file| file.get_ref().read(buffer)) {
        Ok(Ok(0)) => UploadStep::Eof,
        Ok(Ok(read)) => UploadStep::Chunk(Bytes::copy_from_slice(&buffer[..read])),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => UploadStep::Again,
        Ok(Err(_)) => UploadStep::Cancel,
        Err(_) => UploadStep::Again,
    }
}

async fn send_pending(
    sender: Option<&mut http_body_util::channel::Sender<Bytes, std::io::Error>>,
    pending: Option<Bytes>,
) -> Result<(), ()> {
    let (Some(sender), Some(pending)) = (sender, pending) else {
        return Err(());
    };
    sender.send_data(pending).await.map_err(|_| ())
}

async fn deliver_response(
    response: Response,
    sink: &AsyncFd<File>,
    head: &mpsc::Sender<Result<OwnerResponseHead, String>>,
    stop: &mut watch::Receiver<bool>,
    cancelled: &mut watch::Receiver<bool>,
) {
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(key, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (key.to_string(), value.to_owned()))
        })
        .collect();
    if head
        .send(Ok(OwnerResponseHead { status, headers }))
        .is_err()
    {
        return;
    }
    let mut body = response.into_body();
    write_response_body(sink, &mut body, stop, cancelled).await;
}

impl OwnerResponses {
    fn reserve(&self) -> Result<BudgetGuard, String> {
        let mut held = self
            .budget
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if held.saturating_add(UPLOAD_TRANSFER_BYTES) > UPLOAD_AGGREGATE_BYTES {
            return Err("owner responses stopped or busy".into());
        }
        *held += UPLOAD_TRANSFER_BYTES;
        Ok(BudgetGuard {
            budget: Arc::clone(&self.budget),
            bytes: UPLOAD_TRANSFER_BYTES,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &self,
        runtime: &Handle,
        router: Arc<EmbeddedConveyHandle>,
        root: Arc<JournalRoot>,
        mut stop: watch::Receiver<bool>,
        id: u64,
        method: &str,
        uri: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
        sink: File,
    ) -> Result<mpsc::Receiver<Result<OwnerResponseHead, String>>, String> {
        let request = request(method, uri, headers, body)?;
        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| "owner response registry unavailable")?;
        tasks.retain(|_, task| !task.task.is_finished());
        if *stop.borrow() || tasks.contains_key(&id) || tasks.len() >= 32 {
            return Err("owner responses stopped or busy".into());
        }
        let _entered = runtime.enter();
        // SAFETY: this owned File stays inside AsyncFd and is used only through
        // shared references, so its descriptor stays open and unchanged until drop.
        let sink = unsafe { AsyncFd::register(sink) }.map_err(|error| error.to_string())?;
        let (sender, receiver) = mpsc::channel();
        let (cancel, mut cancelled) = watch::channel(false);
        let task = runtime.spawn(async move {
            if let Err(error) = root.revalidate_canonical_binding() {
                let _ = sender.send(Err(error.to_string()));
                return;
            }
            let response = tokio::select! {
                biased;
                _ = stop.changed() => return,
                _ = cancelled.changed() => return,
                response = router.request_owner(request) => response,
            };
            deliver_response(response, &sink, &sender, &mut stop, &mut cancelled).await;
        });
        tasks.insert(id, ResponseTask { cancel, task });
        Ok(receiver)
    }

    /// `body` is the read end of a caller-owned pipe. One chunk is in flight.
    /// The returned receiver is ready when the response head arrives; this
    /// method does not wait for the file.
    #[allow(clippy::too_many_arguments)]
    pub fn open_upload(
        &self,
        runtime: &Handle,
        router: Arc<EmbeddedConveyHandle>,
        root: Arc<JournalRoot>,
        stop: watch::Receiver<bool>,
        id: u64,
        method: &str,
        uri: &str,
        headers: &[(String, String)],
        body: File,
        sink: File,
    ) -> Result<mpsc::Receiver<Result<OwnerResponseHead, String>>, String> {
        let admitted = upload_admission(method, uri, headers)?;
        cap_pipe(&body)?;
        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| "owner response registry unavailable")?;
        tasks.retain(|_, task| !task.task.is_finished());
        if *stop.borrow() || tasks.contains_key(&id) || tasks.len() >= 32 {
            return Err("owner responses stopped or busy".into());
        }
        let guard = self.reserve()?;
        let _entered = runtime.enter();
        // SAFETY: this owned File stays inside AsyncFd and is used only through
        // shared references, so its descriptor stays open and unchanged until drop.
        let body = unsafe { AsyncFd::register(body) }.map_err(|error| error.to_string())?;
        // SAFETY: this owned File stays inside AsyncFd and is used only through
        // shared references, so its descriptor stays open and unchanged until drop.
        let sink = unsafe { AsyncFd::register(sink) }.map_err(|error| error.to_string())?;
        let (chunk_sender, channel) = http_body_util::Channel::<Bytes, std::io::Error>::new(1);
        let request = admitted.into_request(Body::new(channel))?;
        let (sender, receiver) = mpsc::channel();
        let (cancel, cancelled) = watch::channel(false);
        let task = runtime.spawn(async move {
            let _guard = guard;
            drive_upload(
                router,
                root,
                request,
                body,
                sink,
                Some(chunk_sender),
                sender,
                stop,
                cancelled,
            )
            .await;
        });
        tasks.insert(id, ResponseTask { cancel, task });
        Ok(receiver)
    }

    pub fn cancel(&self, id: u64) {
        if let Some(task) = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&id)
        {
            task.cancel.send_replace(true);
        }
    }

    pub async fn drain(&self) -> Result<(), ()> {
        let tasks: Vec<_> = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .drain()
            .map(|(_, task)| task)
            .collect();
        for task in &tasks {
            task.cancel.send_replace(true);
        }
        for task in tasks {
            task.task.await.map_err(|_| ())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representation_is_preserved_and_authority_cannot_be_supplied() {
        let body = b"{\"text\":\"hello\"}".to_vec();
        let form = request(
            "POST",
            "/init/finalize",
            &[("content-type".into(), "text/plain".into())],
            body.clone(),
        )
        .unwrap();
        assert_eq!(form.headers()["content-type"], "text/plain");
        let json = request(
            "POST",
            "/init/finalize",
            &[("content-type".into(), "application/json".into())],
            body,
        )
        .unwrap();
        assert_eq!(json.headers()["content-type"], "application/json");
        for uri in [
            "https://outside.invalid/",
            "//outside.invalid/",
            "outside.invalid:443",
        ] {
            assert!(request("GET", uri, &[], vec![]).is_err());
            assert!(upload_admission("POST", uri, &[]).is_err());
        }
        assert!(
            request(
                "GET",
                "/",
                &[("authorization".into(), "Bearer other".into())],
                vec![]
            )
            .is_err()
        );
        assert!(
            upload_admission(
                "POST",
                "/app/import/api/save",
                &[("authorization".into(), "Bearer other".into())]
            )
            .is_err()
        );
        assert!(request("POST", "/", &[], vec![0; 256 * 1024 + 1]).is_err());
        assert!(request("CONNECT", "/", &[], vec![]).is_err());
        let streamed = upload_admission(
            "POST",
            "/app/import/api/save",
            &[("content-type".into(), "multipart/form-data".into())],
        )
        .unwrap();
        assert_eq!(streamed.uri.path(), "/app/import/api/save");
        assert_eq!(streamed.headers[0].1, "multipart/form-data");
    }

    #[test]
    fn stall_cancel_wins_and_progress_at_the_deadline_resets_it() {
        let mut stall = StallDeadline::arm(0, 60);
        assert_eq!(stall.poll(59), StallWait::Open);
        assert_eq!(stall.poll(60), StallWait::Stalled);
        stall.note_progress(60);
        assert_eq!(stall.poll(60), StallWait::Open);
        assert_eq!(stall.poll(119), StallWait::Open);
        assert_eq!(stall.poll(120), StallWait::Stalled);
        let mut cancelled = StallDeadline::arm(0, 60);
        cancelled.cancel();
        assert_eq!(cancelled.poll(60), StallWait::Cancelled);
        cancelled.note_progress(60);
        assert_eq!(cancelled.poll(10_000), StallWait::Cancelled);
    }

    #[test]
    fn completion_after_eof_is_not_a_stall_and_cancel_still_wins() {
        let transfer = StallDeadline::arm(0, 60_000);
        assert!(!body_handed_off(false, false));
        assert!(!body_handed_off(true, true));
        assert_eq!(transfer.poll(60_000), StallWait::Stalled);

        let mut held = StallDeadline::arm(0, 60_000);
        held.note_progress(1_000);
        assert!(body_handed_off(true, false));
        held.retire();
        assert_eq!(held.poll(1_000 + 60_000), StallWait::Open);
        assert_eq!(held.poll(1_000 + 120_000), StallWait::Open);
        held.note_progress(1_000 + 120_000);
        assert_eq!(held.poll(u64::MAX), StallWait::Open);
        held.cancel();
        assert_eq!(held.poll(u64::MAX), StallWait::Cancelled);

        let mut cancelled = StallDeadline::arm(0, 60_000);
        cancelled.cancel();
        cancelled.retire();
        assert_eq!(cancelled.poll(120_000), StallWait::Cancelled);
    }

    #[test]
    fn concurrent_read_and_upload_identities_do_not_cancel_each_other() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let barrier = Arc::new(Barrier::new(4));
        let mut joins = Vec::new();
        for _ in 0..4 {
            let barrier = Arc::clone(&barrier);
            joins.push(thread::spawn(move || {
                barrier.wait();
                (0..250)
                    .map(|_| next_owner_request_id())
                    .collect::<Vec<_>>()
            }));
        }
        let mut ids = Vec::new();
        for join in joins {
            ids.extend(join.join().expect("request identity thread"));
        }
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
        assert!(unique.iter().all(|id| *id != 0));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let responses = OwnerResponses::default();
        let read_id = next_owner_request_id();
        let upload_id = next_owner_request_id();
        assert_ne!(read_id, upload_id);
        let (read_cancel, upload_cancel) = {
            let (read_tx, read_rx) = watch::channel(false);
            let (upload_tx, upload_rx) = watch::channel(false);
            let read_task = runtime.spawn(std::future::pending::<()>());
            let upload_task = runtime.spawn(std::future::pending::<()>());
            let mut tasks = responses.tasks.lock().unwrap();
            tasks.insert(
                read_id,
                ResponseTask {
                    cancel: read_tx,
                    task: read_task,
                },
            );
            tasks.insert(
                upload_id,
                ResponseTask {
                    cancel: upload_tx,
                    task: upload_task,
                },
            );
            (read_rx, upload_rx)
        };
        responses.cancel(read_id);
        assert!(*read_cancel.borrow());
        assert!(!*upload_cancel.borrow());
        responses.cancel(upload_id);
        assert!(*upload_cancel.borrow());
        let mut tasks = responses.tasks.lock().unwrap();
        tasks.remove(&read_id).unwrap().task.abort();
        tasks.remove(&upload_id).unwrap().task.abort();
    }

    #[test]
    fn chunk_and_reservation_budgets_do_not_grow_with_the_source() {
        assert!(admit_chunk(0).is_ok());
        assert!(admit_chunk(UPLOAD_CHUNK_BYTES).is_ok());
        assert!(admit_chunk(UPLOAD_CHUNK_BYTES + 1).is_err());
        let responses = OwnerResponses::default();
        let mut guards = Vec::new();
        for _ in 0..32 {
            guards.push(responses.reserve().unwrap());
        }
        assert!(responses.reserve().is_err());
        guards.pop();
        let kept = responses.reserve().unwrap();
        drop(guards);
        assert_eq!(
            *responses
                .budget
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
            UPLOAD_TRANSFER_BYTES
        );
        drop(kept);
        assert_eq!(
            *responses
                .budget
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
            0
        );
    }
}
