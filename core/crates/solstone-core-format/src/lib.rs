// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod briefing;
pub mod chunker;
pub mod content;
pub mod json;
pub mod matcher;
pub mod paths;
pub mod segment;

pub use json::{ensure_ascii, json_compact_ascii, json_compact_utf8};

#[cfg(test)]
mod architecture;
