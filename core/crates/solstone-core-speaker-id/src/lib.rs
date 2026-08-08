// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Resolve transcript-row sentence IDs and read headered audio-transcript JSONL
//! while reporting corruption counters.
//!
//! A persisted integer sentence ID wins over a positional ID. This crate has no
//! callers yet; lode P3 wires it into the six positional-derivation sites it
//! will eventually replace. P2 also provides a durable transcript and
//! embedding-sidecar writer for the native CLI boundary.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod corrections;
pub mod json;
pub mod labels;
pub mod transcript;
pub mod writer;

mod ascii_json;
mod npz;
