// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared byte limits and format validation for the linked-device ingest HTTP contract.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod browser;

pub use browser::{
    BrowserRecordError, browser_schema_bytes, browser_schema_digest, browser_schema_id,
    validate_browser_jsonl, validate_browser_record,
};

pub const CONNECTION_BODY_LIMIT: usize = 128 * 1024 * 1024;
pub const MAX_PART_BYTES: usize = 64 * 1024 * 1024;
