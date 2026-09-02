// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed codecs and one-shot client for the generate contract.

mod client;
mod codec;
mod fixture;
mod session;
mod types;

pub use client::{
    CapturedStream, ChildStatus, ClientError, OneShotClient, ProtocolFailure, STDERR_LIMIT,
    STDOUT_LIMIT, UnexpectedChildFailure, sibling_executable,
};
pub use codec::{
    SessionCorrelation, SessionError, decode_one_shot_request, decode_one_shot_response,
    decode_protocol_error, decode_session_request_line, decode_session_response_line,
    decode_session_terminal_line, encode_one_shot_request, encode_one_shot_response,
    encode_protocol_error, encode_session_request_line, encode_session_response_line,
    encode_session_terminal_line,
};
pub use fixture::{contract, contract_source};
pub use session::{
    SessionClient, SessionCloseError, SessionCompletion, SessionFailure, SessionFailureReason,
    SessionLaunchError, SessionLaunchReason, SessionReceiveError, SessionSubmitError,
};
pub use types::{
    ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse, Outcome, ProtocolError,
    ReasonCode, ReasonCodeValue, RefusalReason, RefusedResponse, SessionTerminal,
    UnknownReasonCode,
};
