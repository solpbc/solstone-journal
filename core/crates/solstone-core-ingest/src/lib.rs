// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Segment-arrival routes for linked devices.
//!
//! D1: requests use one JSON multipart `envelope` field.  File bytes stay in
//! repeated `files` parts, while envelope metadata and per-file extension keys
//! are retained in the accepted event.  This avoids ambiguous scattered form
//! fields and preserves forward-compatible descriptors.
//!
//! D2: only a linked-device `AccessBasis` admits these routes.  Localhost has
//! no device identity and is refused rather than being implicitly attributed.
//!
//! D3: this crate serves four published device-ingest operations:
//! `ingestUpload`, `ingestSegments`, `ingestManifest`, and
//! `ingestManifestDay`. `register` and bearer-credential issuance are removed
//! by the hard cut; `ingestEvent` and `callosumStream` await a Rust Callosum
//! client; `health` has no settled semantics. `deleteSource` is served by
//! `solstone-core-clients-web` as a whole-segment location erase through
//! retention's door. The remaining deferred operations are an intentional
//! strand delta, not missing routes.
//!
//! Segment bytes and sidecars are written only through `solstone-core-segment`.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod listing;
mod model;
mod read_routes;
mod router;
mod stream_identity;
mod validation;

pub use router::api_router;
