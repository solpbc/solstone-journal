// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One Task Scheduler control worker kept for the length of one journal
//! command: still deliberately Unowned (spawn census E33/I02), still a direct
//! child with a fixed script, now answering one request per line.
//!
//! A cold `powershell.exe` start is scanned by the endpoint's real-time
//! protection before its runtime loads. On a stock Windows owner that cost up
//! to five extra seconds per start, and a service stop started five of them
//! inside its 30-second deadline (req_zngowlqq). The session pays that once.

use std::cell::{Cell, RefCell};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

const INPUT_LIMIT: usize = 128 * 1024;
const REPLY_LIMIT: usize = 256 * 1024;
const STDERR_LIMIT: usize = 16 * 1024;
/// The most one request may take, whatever the caller's own deadline.
const REQUEST_BUDGET: Duration = Duration::from_secs(15);
/// Added to the budget of the request that starts the worker. A cold
/// `powershell.exe` start is not a stalled scheduler call: its warm request
/// measured 3.3-3.6 s on an owner-class Broadwell, and its first cold start
/// there overran the whole 15 s. The caller's own deadline still bounds it.
const COLD_START_ALLOWANCE: Duration = Duration::from_secs(30);
/// Kept back from a request's budget to observe a stopped worker.
const OBSERVATION_RESERVE: Duration = Duration::from_secs(2);
/// The field every request carries and every reply echoes.
const REQUEST_ID: &str = "request_id";

pub(crate) enum ControlReply {
    /// This request's own reply, with its request id removed.
    Reply { value: Value, stderr: Vec<u8> },
    /// The worker exited without replying; what it wrote to stderr.
    Exited { stderr: Vec<u8> },
}

struct Worker {
    child: Child,
    requests: Option<mpsc::Sender<Vec<u8>>>,
    replies: mpsc::Receiver<io::Result<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_drain: JoinHandle<()>,
}

impl Worker {
    fn spawn(mut command: Command) -> Result<Self, String> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("task control spawn failed: {error}"))?;
        let (Some(stdin), Some(stdout), Some(mut stderr_pipe)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err("task control stdio missing".to_owned());
        };
        let (requests, requests_rx) = mpsc::channel::<Vec<u8>>();
        let (replies_tx, replies) = mpsc::channel();
        std::thread::spawn(move || exchange_loop(stdin, stdout, &requests_rx, &replies_tx));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stderr);
        // Drained continuously so a talkative worker never blocks on a full
        // pipe; only the most recent bytes are kept for a failure reason.
        let stderr_drain = std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = stderr_pipe.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let mut kept = sink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                kept.extend_from_slice(&buffer[..count]);
                let excess = kept.len().saturating_sub(STDERR_LIMIT);
                kept.drain(..excess);
            }
        });
        Ok(Self {
            child,
            requests: Some(requests),
            replies,
            stderr,
            stderr_drain,
        })
    }

    fn take_stderr(&self) -> Vec<u8> {
        std::mem::take(
            &mut *self
                .stderr
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Stop the worker and watch for its exit until `deadline`.
    fn stop(mut self, deadline: Instant) -> String {
        self.requests = None;
        let termination = self.child.kill();
        let observed = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break Ok(Some(status)),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                other => break other,
            }
        };
        format!(
            "direct-worker stop request: {termination:?}; direct-worker observation: {observed:?}"
        )
    }

    /// A worker that closed its output: let it and its stderr finish until
    /// `deadline`, then return everything it wrote there.
    fn exited(mut self, deadline: Instant) -> Vec<u8> {
        self.requests = None;
        while Instant::now() < deadline
            && (!matches!(self.child.try_wait(), Ok(Some(_))) || !self.stderr_drain.is_finished())
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.take_stderr()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing stdin ends the worker's read loop; the kill is the bound for
        // a worker that is not reading. It holds no state between requests.
        self.requests = None;
        let _ = self.child.kill();
    }
}

fn exchange_loop(
    mut stdin: ChildStdin,
    stdout: ChildStdout,
    requests: &mpsc::Receiver<Vec<u8>>,
    replies: &mpsc::Sender<io::Result<Vec<u8>>>,
) {
    let mut stdout = BufReader::new(stdout);
    while let Ok(mut request) = requests.recv() {
        request.push(b'\n');
        let reply = stdin
            .write_all(&request)
            .and_then(|()| stdin.flush())
            .and_then(|()| read_reply(&mut stdout));
        let failed = reply.is_err();
        if replies.send(reply).is_err() || failed {
            return;
        }
    }
}

fn read_reply(stdout: &mut BufReader<ChildStdout>) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let read = stdout
        .by_ref()
        .take((REPLY_LIMIT + 1) as u64)
        .read_until(b'\n', &mut line)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "task control worker exited",
        ));
    }
    if line.last() != Some(&b'\n') {
        return Err(io::Error::other(
            "task control output exceeds its byte limit",
        ));
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    Ok(line)
}

/// A command's Task Scheduler worker, started on its first request.
#[derive(Default)]
pub(crate) struct ControlSession {
    worker: RefCell<Option<Worker>>,
    sent: Cell<u64>,
}

impl ControlSession {
    /// Send one request and read its own reply before `caller_deadline` (and
    /// never longer than one request's own budget). A request that fails,
    /// overruns, or draws a reply that is not its own stops the worker; the
    /// next request starts a new one, and the caller must re-inspect before
    /// trusting state.
    pub(crate) fn exchange(
        &self,
        command: impl FnOnce() -> Command,
        mut request: Map<String, Value>,
        caller_deadline: Instant,
    ) -> Result<ControlReply, String> {
        let id = self.sent.get() + 1;
        self.sent.set(id);
        let id = id.to_string();
        request.insert(REQUEST_ID.to_owned(), Value::String(id.clone()));
        let input = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        if input.len() > INPUT_LIMIT {
            return Err("task control input exceeds its byte limit".into());
        }
        let remaining = caller_deadline.saturating_duration_since(Instant::now());
        if remaining <= Duration::from_millis(10) {
            return Err("task control deadline elapsed before spawn".into());
        }
        let mut slot = self.worker.borrow_mut();
        if slot
            .as_mut()
            .is_some_and(|worker| !matches!(worker.child.try_wait(), Ok(None)))
        {
            *slot = None;
        }
        let cold = slot.is_none();
        let budget = remaining.min(if cold {
            REQUEST_BUDGET + COLD_START_ALLOWANCE
        } else {
            REQUEST_BUDGET
        });
        let deadline = Instant::now() + budget;
        let work_deadline = deadline - OBSERVATION_RESERVE.min(budget / 4);
        if cold {
            *slot = Some(Worker::spawn(command())?);
        }
        let worker = slot.as_ref().expect("worker was just ensured");
        worker.take_stderr();
        let sent = worker
            .requests
            .as_ref()
            .is_some_and(|requests| requests.send(input).is_ok());
        let reason = if sent {
            match worker
                .replies
                .recv_timeout(work_deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Ok(line)) => {
                    let stderr = worker.take_stderr();
                    match own_reply(&line, &id) {
                        Some(value) => return Ok(ControlReply::Reply { value, stderr }),
                        // Not this request's reply: nothing later from this
                        // worker can be trusted to line up either.
                        None => {
                            *slot = None;
                            return Err("task operation returned invalid JSON".to_owned());
                        }
                    }
                }
                Ok(Err(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    let stderr = slot.take().expect("worker present").exited(deadline);
                    return Ok(ControlReply::Exited { stderr });
                }
                Ok(Err(error)) => format!("task control exchange failed: {error}"),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    "task control exchange timed out".to_owned()
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    "task control exchange failed: worker channel closed".to_owned()
                }
            }
        } else {
            "task control exchange failed: worker channel closed".to_owned()
        };
        let observation = slot.take().expect("worker present").stop(deadline);
        Err(format!(
            "{reason}; {observation}; scheduler RPC may have changed state; re-inspection required"
        ))
    }
}

/// The reply as JSON without its request id, if it is a JSON object that
/// echoes exactly `id`.
fn own_reply(line: &[u8], id: &str) -> Option<Value> {
    let Value::Object(mut reply) = serde_json::from_slice::<Value>(line).ok()? else {
        return None;
    };
    match reply.remove(REQUEST_ID) {
        Some(Value::String(echoed)) if echoed == id => Some(Value::Object(reply)),
        _ => None,
    }
}
