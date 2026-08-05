// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed codecs and one-shot client for the generate contract.

mod client;
mod codec;
mod fixture;
mod types;

pub use client::{ClientError, OneShotClient};
pub use codec::{
    SessionCorrelation, SessionError, decode_one_shot_request, decode_one_shot_response,
    decode_protocol_error, decode_session_request_line, decode_session_response_line,
    encode_one_shot_request, encode_session_request_line,
};
pub use fixture::contract;
pub use types::{
    ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse, Outcome, ProtocolError,
    ReasonCode, ReasonCodeValue, RefusalReason, RefusedResponse, UnknownReasonCode,
};
