// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Request and NDJSON event wire types for the native cogitate subcommand.

mod client;
mod dispatch;
mod endpoint;
mod event;
mod request;
mod validation;

pub use client::{ClientError, CogitateOneShotClient, CogitateOneShotRun};
pub use dispatch::DispatchConverseProvider;
pub use endpoint::{
    COGITATE_API_KEY_OVERRIDE_ENV, COGITATE_ENDPOINT_URL_OVERRIDE_ENV, EndpointOverrides,
};
pub use event::{
    NativeRun, native_producible_kinds, run_or_dry_run, serialize_dry_run, serialize_event,
    serialize_event_validated,
};
pub use request::{CogitateRequest, MalformedRequest, REQUEST_SCHEMA};
pub use validation::{ValidationError, contract_source, validate_event};

#[cfg(test)]
mod tests;
