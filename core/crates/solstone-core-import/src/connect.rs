// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed routing to the native owner-present Oura authorization flow.

use std::path::PathBuf;

use solstone_core_body_ingest::{
    BodyIngestError, OuraConnectOptions, OuraConnectReport, connect_oura as body_connect_oura,
};

/// Inputs for the owner-present Oura connect operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OuraConnectRequest {
    pub journal_root: PathBuf,
    pub timeout_seconds: u64,
}

/// Data returned by the native authorization owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OuraConnectOutcome {
    pub report: OuraConnectReport,
}

/// Route to the body owner's authorization flow without introducing token storage.
pub fn connect_oura(request: &OuraConnectRequest) -> Result<OuraConnectOutcome, BodyIngestError> {
    let options = OuraConnectOptions {
        timeout_seconds: request.timeout_seconds,
    };
    body_connect_oura(&request.journal_root, &options).map(|report| OuraConnectOutcome { report })
}
