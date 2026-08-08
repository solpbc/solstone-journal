// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Resolve transcript-row sentence IDs and read headered audio-transcript JSONL
//! while reporting corruption counters.
//!
//! A persisted integer sentence ID wins over a positional ID. This crate has no
//! callers yet; lode P3 wires it into the six positional-derivation sites it
//! will eventually replace.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod transcript;
