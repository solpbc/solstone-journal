// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::codec::{
    SessionCorrelation, SessionError, decode_session_response_line, encode_session_request_line,
    encode_session_terminal_line,
};
use crate::fixture::contract;
use crate::{GenerateRequest, GenerateResponse, SessionTerminal};

const SESSION_FAILURE_RETRYABLE: bool = false;
const SESSION_FAILURE_BLOCKING: bool = true;

#[derive(Debug, Clone, PartialEq)]
pub enum SessionCompletion {
    Response(GenerateResponse),
    Failure(SessionFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionFailureReason {
    Desynchronized(String),
    ChildExited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFailure {
    pub id: String,
    pub reason: SessionFailureReason,
    pub retryable: bool,
    pub blocking: bool,
}

impl SessionFailure {
    fn new(id: String, reason: SessionFailureReason) -> Self {
        Self {
            id,
            reason,
            retryable: SESSION_FAILURE_RETRYABLE,
            blocking: SESSION_FAILURE_BLOCKING,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLaunchReason {
    Resolve(String),
    Spawn(String),
    Contract(String),
    InvalidConcurrency,
    AlreadyStarted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLaunchError {
    pub reason: SessionLaunchReason,
    pub retryable: bool,
    pub blocking: bool,
}

impl SessionLaunchError {
    fn new(reason: SessionLaunchReason) -> Self {
        Self {
            reason,
            retryable: SESSION_FAILURE_RETRYABLE,
            blocking: SESSION_FAILURE_BLOCKING,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSubmitError {
    NotStarted,
    Closed,
    Encode(String),
    Correlation(SessionError),
    WriterUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCloseError {
    NotStarted,
    AlreadyClosed,
    Encode(String),
    WriterUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionReceiveError {
    NotStarted,
    TimedOut,
    Disconnected,
}

enum OutgoingRecord {
    Request(String),
    Terminal(String),
}

enum WorkerEvent {
    Desynchronized(String),
    StdoutFinished,
    ChildExited,
}

struct SessionRuntime {
    outgoing: Sender<OutgoingRecord>,
    completions: Receiver<SessionCompletion>,
    correlation: Arc<Mutex<SessionCorrelation>>,
    closed: AtomicBool,
    child_id: u32,
}

struct InFlightGate {
    state: Mutex<InFlightGateState>,
    available: std::sync::Condvar,
}

struct InFlightGateState {
    permits: usize,
    aborted: bool,
}

impl InFlightGate {
    fn new(permits: usize) -> Self {
        Self {
            state: Mutex::new(InFlightGateState {
                permits,
                aborted: false,
            }),
            available: std::sync::Condvar::new(),
        }
    }

    fn acquire(&self) -> bool {
        let mut state = self.state.lock().expect("in-flight gate lock poisoned");
        while state.permits == 0 && !state.aborted {
            state = self
                .available
                .wait(state)
                .expect("in-flight gate lock poisoned");
        }
        if state.aborted {
            return false;
        }
        state.permits -= 1;
        true
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("in-flight gate lock poisoned");
        state.permits += 1;
        self.available.notify_one();
    }

    fn abort(&self) {
        let mut state = self.state.lock().expect("in-flight gate lock poisoned");
        state.aborted = true;
        self.available.notify_all();
    }
}

/// A one-child, newline-framed generate session.
///
/// Build it with [`Self::at_path`], configure it with
/// [`Self::with_prefix_arguments`], [`Self::with_session_journal`], or
/// [`Self::with_env`], then start it with [`Self::spawn`]. [`Self::recv`] blocks until one
/// submitted request completes, in response-arrival order.
pub struct SessionClient {
    executable: PathBuf,
    prefix_arguments: Vec<std::ffi::OsString>,
    session_journal: Option<PathBuf>,
    environment: BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    runtime: Option<SessionRuntime>,
}

impl SessionClient {
    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self {
            executable: path.into(),
            prefix_arguments: Vec::new(),
            session_journal: None,
            environment: BTreeMap::new(),
            runtime: None,
        }
    }

    pub fn with_prefix_arguments(
        mut self,
        arguments: impl IntoIterator<Item = std::ffi::OsString>,
    ) -> Self {
        self.prefix_arguments.extend(arguments);
        self
    }

    pub fn with_session_journal(mut self, path: impl Into<PathBuf>) -> Self {
        self.session_journal = Some(path.into());
        self
    }

    pub fn with_env(
        mut self,
        name: impl Into<std::ffi::OsString>,
        value: impl Into<std::ffi::OsString>,
    ) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }

    pub fn sibling_path() -> Result<PathBuf, SessionLaunchError> {
        let current = env::current_exe().map_err(|error| {
            SessionLaunchError::new(SessionLaunchReason::Resolve(error.to_string()))
        })?;
        let parent = current.parent().ok_or_else(|| {
            SessionLaunchError::new(SessionLaunchReason::Resolve(
                "current executable has no parent".to_owned(),
            ))
        })?;
        let path = parent.join("solstone-core");
        if Path::new(&path).is_file() {
            Ok(path)
        } else {
            Err(SessionLaunchError::new(SessionLaunchReason::Resolve(
                format!("missing sibling executable {}", path.display()),
            )))
        }
    }

    pub fn session_arguments(
        &self,
        max_in_flight: usize,
    ) -> Result<Vec<std::ffi::OsString>, SessionLaunchError> {
        let session = &contract()["framing"]["session"];
        let selector = session["selector"].as_str().ok_or_else(|| {
            SessionLaunchError::new(SessionLaunchReason::Contract(
                "session selector is not a string".to_owned(),
            ))
        })?;
        let concurrency = &session["concurrency"];
        let flag = concurrency["flag"].as_str().ok_or_else(|| {
            SessionLaunchError::new(SessionLaunchReason::Contract(
                "session concurrency flag is not a string".to_owned(),
            ))
        })?;
        let minimum = concurrency["minimum"].as_u64().ok_or_else(|| {
            SessionLaunchError::new(SessionLaunchReason::Contract(
                "session concurrency minimum is not an integer".to_owned(),
            ))
        })? as usize;
        let journal_flag = session["journal"]["flag"].as_str().ok_or_else(|| {
            SessionLaunchError::new(SessionLaunchReason::Contract(
                "session journal flag is not a string".to_owned(),
            ))
        })?;
        if max_in_flight < minimum {
            return Err(SessionLaunchError::new(
                SessionLaunchReason::InvalidConcurrency,
            ));
        }

        let mut arguments = self.prefix_arguments.clone();
        arguments.push(selector.into());
        arguments.push(flag.into());
        arguments.push(max_in_flight.to_string().into());
        if let Some(path) = &self.session_journal {
            arguments.push(journal_flag.into());
            arguments.push(path.as_os_str().to_owned());
        }
        Ok(arguments)
    }

    pub fn spawn(mut self, max_in_flight: usize) -> Result<Self, SessionLaunchError> {
        if self.runtime.is_some() {
            return Err(SessionLaunchError::new(SessionLaunchReason::AlreadyStarted));
        }

        let arguments = self.session_arguments(max_in_flight)?;
        let mut child = Command::new(&self.executable)
            .args(arguments)
            .envs(&self.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                SessionLaunchError::new(SessionLaunchReason::Spawn(error.to_string()))
            })?;
        let child_id = child.id();
        let stdin = take_pipe(&mut child, |child| child.stdin.take())?;
        let stdout = take_pipe(&mut child, |child| child.stdout.take())?;
        let stderr = take_pipe(&mut child, |child| child.stderr.take())?;

        let correlation = Arc::new(Mutex::new(SessionCorrelation::default()));
        let (outgoing_tx, outgoing_rx) = mpsc::channel();
        let (completion_tx, completion_rx) = mpsc::channel();
        let (worker_tx, worker_rx) = mpsc::channel();
        let (kill_tx, kill_rx) = mpsc::channel();
        let gate = Arc::new(InFlightGate::new(max_in_flight));

        spawn_writer(stdin, outgoing_rx, Arc::clone(&gate));
        spawn_stdout_reader(
            stdout,
            Arc::clone(&correlation),
            Arc::clone(&gate),
            completion_tx.clone(),
            worker_tx.clone(),
            kill_tx.clone(),
        );
        spawn_stderr_drain(stderr);
        spawn_waiter(child, worker_tx.clone(), kill_rx);
        spawn_completion_coordinator(correlation.clone(), gate, completion_tx, worker_rx);
        drop(worker_tx);
        drop(kill_tx);

        self.runtime = Some(SessionRuntime {
            outgoing: outgoing_tx,
            completions: completion_rx,
            correlation,
            closed: AtomicBool::new(false),
            child_id,
        });
        Ok(self)
    }

    pub fn submit(&self, request: GenerateRequest) -> Result<(), SessionSubmitError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(SessionSubmitError::NotStarted)?;
        if runtime.closed.load(Ordering::Acquire) {
            return Err(SessionSubmitError::Closed);
        }
        let encoded = encode_session_request_line(&request).map_err(SessionSubmitError::Encode)?;
        let id = request
            .id
            .clone()
            .ok_or(SessionSubmitError::Correlation(SessionError::MissingId))?;
        runtime
            .correlation
            .lock()
            .expect("session correlation lock poisoned")
            .submit(id)
            .map_err(SessionSubmitError::Correlation)?;
        runtime
            .outgoing
            .send(OutgoingRecord::Request(encoded))
            .map_err(|_| SessionSubmitError::WriterUnavailable)
    }

    pub fn recv(&self) -> Result<SessionCompletion, SessionReceiveError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(SessionReceiveError::NotStarted)?;
        runtime
            .completions
            .recv()
            .map_err(|_| SessionReceiveError::Disconnected)
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<SessionCompletion, SessionReceiveError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(SessionReceiveError::NotStarted)?;
        runtime
            .completions
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => SessionReceiveError::TimedOut,
                mpsc::RecvTimeoutError::Disconnected => SessionReceiveError::Disconnected,
            })
    }

    pub fn child_id(&self) -> Option<u32> {
        self.runtime.as_ref().map(|runtime| runtime.child_id)
    }

    pub fn close(&self) -> Result<(), SessionCloseError> {
        let runtime = self.runtime.as_ref().ok_or(SessionCloseError::NotStarted)?;
        if runtime.closed.swap(true, Ordering::AcqRel) {
            return Err(SessionCloseError::AlreadyClosed);
        }
        let terminal =
            encode_session_terminal_line(SessionTerminal).map_err(SessionCloseError::Encode)?;
        runtime
            .outgoing
            .send(OutgoingRecord::Terminal(terminal))
            .map_err(|_| SessionCloseError::WriterUnavailable)
    }
}

fn take_pipe<T>(
    child: &mut std::process::Child,
    take: impl FnOnce(&mut std::process::Child) -> Option<T>,
) -> Result<T, SessionLaunchError> {
    match take(child) {
        Some(pipe) => Ok(pipe),
        None => {
            let _ = child.kill();
            Err(SessionLaunchError::new(SessionLaunchReason::Spawn(
                "session child pipe is unavailable".to_owned(),
            )))
        }
    }
}

fn spawn_writer(
    mut stdin: std::process::ChildStdin,
    outgoing: Receiver<OutgoingRecord>,
    gate: Arc<InFlightGate>,
) {
    thread::spawn(move || {
        for record in outgoing {
            let (line, terminal) = match record {
                OutgoingRecord::Request(line) => (line, false),
                OutgoingRecord::Terminal(line) => (line, true),
            };
            if !terminal && !gate.acquire() {
                return;
            }
            if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                return;
            }
            if terminal {
                return;
            }
        }
    });
}

fn spawn_stdout_reader(
    stdout: std::process::ChildStdout,
    correlation: Arc<Mutex<SessionCorrelation>>,
    gate: Arc<InFlightGate>,
    completions: Sender<SessionCompletion>,
    workers: Sender<WorkerEvent>,
    killer: Sender<()>,
) {
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    desynchronize(&workers, &killer, error.to_string());
                    break;
                }
            };
            let response = match decode_session_response_line(&line) {
                Ok(response) => response,
                Err(error) => {
                    desynchronize(&workers, &killer, error);
                    break;
                }
            };
            if let Err(error) = correlation
                .lock()
                .expect("session correlation lock poisoned")
                .accept(&response)
            {
                desynchronize(
                    &workers,
                    &killer,
                    format!("invalid response correlation: {error:?}"),
                );
                break;
            }
            gate.release();
            if completions
                .send(SessionCompletion::Response(response))
                .is_err()
            {
                break;
            }
        }
        let _ = workers.send(WorkerEvent::StdoutFinished);
    });
}

fn desynchronize(workers: &Sender<WorkerEvent>, killer: &Sender<()>, detail: String) {
    let _ = workers.send(WorkerEvent::Desynchronized(detail));
    let _ = killer.send(());
}

fn spawn_stderr_drain(stderr: std::process::ChildStderr) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buffer = [0_u8; 8_192];
        while reader.read(&mut buffer).is_ok_and(|read| read != 0) {}
    });
}

fn spawn_waiter(
    mut child: std::process::Child,
    workers: Sender<WorkerEvent>,
    killer: Receiver<()>,
) {
    thread::spawn(move || {
        loop {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    let _ = workers.send(WorkerEvent::ChildExited);
                    return;
                }
                Ok(None) => {}
            }
            if killer.recv_timeout(Duration::from_millis(10)).is_ok() {
                let _ = child.kill();
                let _ = child.wait();
                let _ = workers.send(WorkerEvent::ChildExited);
                return;
            }
        }
    });
}

fn spawn_completion_coordinator(
    correlation: Arc<Mutex<SessionCorrelation>>,
    gate: Arc<InFlightGate>,
    completions: Sender<SessionCompletion>,
    workers: Receiver<WorkerEvent>,
) {
    thread::spawn(move || {
        let mut desynchronization = None;
        let mut stdout_finished = false;
        let mut child_exited = false;
        while let Ok(event) = workers.recv() {
            match event {
                WorkerEvent::Desynchronized(detail) => desynchronization = Some(detail),
                WorkerEvent::StdoutFinished => stdout_finished = true,
                WorkerEvent::ChildExited => child_exited = true,
            }
            if stdout_finished && child_exited {
                gate.abort();
                let reason = desynchronization
                    .map(SessionFailureReason::Desynchronized)
                    .unwrap_or(SessionFailureReason::ChildExited);
                let ids = correlation
                    .lock()
                    .expect("session correlation lock poisoned")
                    .fail_outstanding();
                for id in ids {
                    if completions
                        .send(SessionCompletion::Failure(SessionFailure::new(
                            id,
                            reason.clone(),
                        )))
                        .is_err()
                    {
                        return;
                    }
                }
                return;
            }
        }
    });
}
