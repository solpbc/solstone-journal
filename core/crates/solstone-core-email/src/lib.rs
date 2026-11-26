// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Phase 1 email source: structurally read-only IMAP4rev1 + Gmail client,
//! regenerable header cache, deterministic selection with versioned re-reach,
//! two-signal quote/signature/forward extraction, and selective sync into
//! `chronicle/<day>/import.email/<key>/conversation_transcript.jsonl`.

#![forbid(unsafe_code)]

pub mod cache;
pub mod choice;
pub mod error;
pub mod extract;
pub mod headers;
pub mod html;
pub mod imap;
pub mod model;
pub mod select;
pub mod store;
pub mod sync;
pub mod text;

#[cfg(test)]
mod tests;
