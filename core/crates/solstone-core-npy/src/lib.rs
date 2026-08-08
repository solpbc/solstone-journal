// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Minimal NPY reading and writing primitives.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod reader;
pub mod writer;

pub use reader::{NpyBlob, NpyReadError, parse_npy};
pub use writer::write_npy;
