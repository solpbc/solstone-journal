// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::Cell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{
    CapturedStream, ChildStatus, ClientError, GenerateRequest, GenerateResponse, RefusalReason,
    SessionClient, SessionCompletion, SessionFailureReason, SessionLaunchReason,
    SessionReceiveError, UnexpectedChildFailure,
};

fn session_failed() -> ClientError {
    ClientError::Io {
        primary: "generate session failed".to_owned(),
        cleanup: None,
    }
}

thread_local! {
    static DOCUMENT_RESUBMIT_COUNT: Cell<u64> = const { Cell::new(0) };
}

static REQUEST_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Reads and resets the document import resubmit counter for the current thread.
pub fn take_document_resubmit_count() -> u64 {
    DOCUMENT_RESUBMIT_COUNT.with(|cell| cell.replace(0))
}

fn record_document_resubmit() {
    DOCUMENT_RESUBMIT_COUNT.with(|cell| cell.set(cell.get() + 1));
}

/// A long-lived, lazy session-based Generate adapter that manages a single
/// `solstone-core generate --session 1` child process across multiple requests.
///
/// On encountering `ConfidentialChannelClosed` / `confidential_channel_closed` on the first
/// attempt (`attempt_index == 0`), it logs a warning, records the resubmission in the thread-local counter,
/// and resubmits the request once.
pub struct GenerateSessionAdapter {
    executable: Option<PathBuf>,
    prefix_arguments: Vec<OsString>,
    environment: std::collections::BTreeMap<OsString, OsString>,
    session: Mutex<Option<SessionClient>>,
}

impl GenerateSessionAdapter {
    pub fn new(executable: PathBuf, prefix_arguments: Vec<OsString>) -> Self {
        Self {
            executable: Some(executable),
            prefix_arguments,
            environment: std::collections::BTreeMap::new(),
            session: Mutex::new(None),
        }
    }

    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self::new(path.into(), Vec::new())
    }

    pub fn sibling() -> Self {
        Self {
            executable: None,
            prefix_arguments: vec![OsString::from("generate")],
            environment: std::collections::BTreeMap::new(),
            session: Mutex::new(None),
        }
    }

    pub fn with_prefix_arguments(mut self, arguments: impl IntoIterator<Item = OsString>) -> Self {
        self.prefix_arguments.extend(arguments);
        self
    }

    pub fn with_env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }

    fn ensure_session(&self, guard: &mut Option<SessionClient>) -> Result<(), ClientError> {
        if guard.is_some() {
            return Ok(());
        }
        let executable = match &self.executable {
            Some(path) => path.clone(),
            None => SessionClient::sibling_path().map_err(|_| {
                ClientError::Resolve("generate sibling executable was not found".to_owned())
            })?,
        };
        let mut builder =
            SessionClient::at_path(executable).with_prefix_arguments(self.prefix_arguments.clone());
        for (name, value) in &self.environment {
            builder = builder.with_env(name.clone(), value.clone());
        }
        let client = builder.spawn(1).map_err(|error| match error.reason {
            SessionLaunchReason::Resolve(detail) => ClientError::Resolve(detail),
            _ => ClientError::Io {
                primary: "generate session failed".to_owned(),
                cleanup: None,
            },
        })?;
        *guard = Some(client);
        Ok(())
    }

    pub fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        DOCUMENT_RESUBMIT_COUNT.with(|cell| cell.set(0));

        let mut guard = match self.session.lock() {
            Ok(guard) => guard,
            Err(poison) => poison.into_inner(),
        };

        let response = self.submit_once(&mut guard, request, request.attempt_index)?;
        let is_closed = match &response {
            GenerateResponse::Refused(refused) => {
                refused.reason == RefusalReason::ConfidentialChannelClosed
                    || refused.reason_code.as_ref().map(|c| c.as_wire())
                        == Some("confidential_channel_closed")
            }
            _ => false,
        };
        if !is_closed {
            return Ok(response);
        }

        let resubmit_attempt_index = request.attempt_index + 1;
        log::warn!(
            "confidential channel closed before a response: reason_code=confidential_channel_closed context={} attempt_index={}",
            request.context,
            resubmit_attempt_index
        );
        record_document_resubmit();
        self.submit_once(&mut guard, request, resubmit_attempt_index)
    }

    /// Runs one attempt on the session, starting a child if there is none.
    ///
    /// A request no live child accepted (the submit failed, or the session had
    /// already ended when it was sent) is delivered once more to a new child.
    /// A child that ends while holding the request fails the call, and the
    /// request is never sent again.
    fn submit_once(
        &self,
        guard: &mut Option<SessionClient>,
        request: &GenerateRequest,
        attempt_index: u64,
    ) -> Result<GenerateResponse, ClientError> {
        let mut respawned = false;
        loop {
            self.ensure_session(guard)?;
            let session = guard.as_ref().ok_or_else(session_failed)?;

            let count = REQUEST_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let mut attempt = request.clone();
            attempt.id = Some(match &request.id {
                Some(req_id) => format!("{req_id}-{count}"),
                None => format!("session-req-{count}"),
            });
            attempt.attempt_index = attempt_index;

            let undelivered = match session.submit(attempt) {
                Err(_) => true,
                Ok(()) => match session.recv() {
                    Ok(SessionCompletion::Response(response)) => return Ok(response),
                    Ok(SessionCompletion::Failure(failure)) => {
                        let is_child_exit = failure.reason == SessionFailureReason::ChildExited;
                        drop(guard.take());
                        if is_child_exit {
                            return Err(ClientError::UnexpectedChild(Box::new(
                                UnexpectedChildFailure {
                                    status: ChildStatus {
                                        exit_code: None,
                                        signal: None,
                                    },
                                    stdout: CapturedStream::empty(),
                                    stderr: CapturedStream::empty(),
                                    stdin_closed_early: false,
                                },
                            )));
                        }
                        return Err(session_failed());
                    }
                    // The session had already ended before this request was
                    // registered, so no child received it.
                    Err(SessionReceiveError::Disconnected) => true,
                    Err(_) => {
                        drop(guard.take());
                        return Err(session_failed());
                    }
                },
            };
            drop(guard.take());
            if !undelivered || respawned {
                return Err(session_failed());
            }
            respawned = true;
        }
    }

    pub fn finish(&self) {
        let mut guard = match self.session.lock() {
            Ok(guard) => guard,
            Err(poison) => poison.into_inner(),
        };
        if let Some(session) = guard.take() {
            let _ = session.close();
        }
    }
}

impl Drop for GenerateSessionAdapter {
    fn drop(&mut self) {
        let mut guard = match self.session.lock() {
            Ok(guard) => guard,
            Err(poison) => poison.into_inner(),
        };
        drop(guard.take());
    }
}
