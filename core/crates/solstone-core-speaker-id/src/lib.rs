// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read headered audio-transcript rows and speaker label/correction stores.
//!
//! Label and correction writes match Python's byte format, including ASCII JSON
//! escaping and persisted unknown fields. P2 also provides a durable transcript
//! and embedding-sidecar writer for the native CLI boundary. A persisted
//! integer sentence ID wins over positional derivation. The crate also
//! provides durable transcript and embedding-sidecar readers and writers for
//! the native attribution boundary.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod calibration;
pub mod corrections;
pub mod embeddings;
pub mod evidence;
pub mod json;
pub mod labels;
pub mod layer1;
pub mod layer2;
pub mod layer3;
pub mod owner_centroid;
pub mod resolve;
pub mod transcript;
pub mod voiceprint_centroid;
pub mod writer;

mod ascii_json;
mod npy_read;
mod npz;
mod person_guard;
