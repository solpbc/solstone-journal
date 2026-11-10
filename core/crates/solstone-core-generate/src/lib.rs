// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed codecs and one-shot client for the generate contract.

mod cancellation;
mod client;
mod codec;
mod fixture;
mod session;
mod session_adapter;
mod types;

pub use cancellation::Cancellation;

pub use client::{
    CapturedStream, ChildStatus, ClientError, InvalidResponseFailure, OneShotClient,
    ProcessFailure, ProtocolFailure, STDERR_LIMIT, STDOUT_LIMIT, UnexpectedChildFailure,
    sibling_executable,
};
pub use codec::{
    SessionCorrelation, SessionError, decode_one_shot_request, decode_one_shot_response,
    decode_protocol_error, decode_session_request_line, decode_session_response_line,
    decode_session_terminal_line, encode_one_shot_request, encode_one_shot_response,
    encode_protocol_error, encode_session_request_line, encode_session_response_line,
    encode_session_terminal_line,
};
pub use fixture::{contract, contract_source, is_attestation_family_reason, is_blocking_reason};
pub use session::{
    SessionClient, SessionCloseError, SessionCompletion, SessionFailure, SessionFailureReason,
    SessionLaunchError, SessionLaunchReason, SessionReceiveError, SessionSubmitError,
};
pub use session_adapter::{GenerateSessionAdapter, take_document_resubmit_count};
pub use types::{
    ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse,
    HEALTH_BRAIN_GENERATE_CONTEXT, Outcome, ProtocolError, ReasonCode, ReasonCodeValue,
    RefusalReason, RefusedResponse, SessionTerminal, UnknownReasonCode,
};

/// A synchronous Generate boundary used by talent execution. Process clients
/// and app-owned provider dispatch preserve the same typed request and reply.
pub trait GenerateExecutor: Send + Sync {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError>;

    /// Trusted in-process talent allowance. Process clients keep the base request;
    /// the mobile provider boundary selects this only after resolving its lane.
    fn execute_with_mobile_local_timeout(
        &self,
        request: &GenerateRequest,
        _timeout: std::time::Duration,
    ) -> Result<GenerateResponse, ClientError> {
        self.execute(request)
    }
}

impl GenerateExecutor for OneShotClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        OneShotClient::execute(self, request)
    }
}

impl<F> GenerateExecutor for F
where
    F: Fn(&GenerateRequest) -> Result<GenerateResponse, ClientError> + Send + Sync,
{
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self(request)
    }
}

/// The Windows launch-protocol environment names a child may only receive fresh
/// from its own launcher (`solstone_core_system::process::launch_only_environment_names`).
/// This crate is publishable and cannot depend on `solstone-core-system`, so it keeps
/// its own copy; `solstone-core/tests/launch_only_names.rs` tests that the two lists are equal.
pub const WINDOWS_LAUNCH_ONLY_ENVIRONMENT: [&str; 9] = [
    "SOL_WINDOWS_LAUNCH",
    "SOL_SUPERVISOR_SPAWNED",
    "SOL_HOSTED_LAUNCH_ID",
    "SOL_HOSTED_PARENT_INSTANCE",
    "SOL_HOSTED_ACK_HANDLE",
    "SOL_HOSTED_STOP_HANDLE",
    "SOL_PARENT_LOSS_GENERATION",
    "SOL_PARENT_LOSS_LAUNCH_ID",
    "SOL_PARENT_LOSS_PARENT_LAUNCH_ID",
];
