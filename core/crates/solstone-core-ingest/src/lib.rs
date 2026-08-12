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
//! D3: this crate serves four of nine published `observer.*` operations:
//! `ingestUpload`, `ingestSegments`, `ingestManifest`, and
//! `ingestManifestDay`. `register` and bearer-credential issuance are removed
//! by the hard cut; `ingestEvent` and `callosumStream` await a Rust Callosum
//! client; `health` has no settled semantics; and `deleteSource` belongs to a
//! later delete/tombstone wave. Those five deferred operations are an
//! intentional strand delta, not missing routes.
//!
//! Segment bytes and sidecars are written only through `solstone-core-segment`.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod model;
mod paths;
mod read_routes;
mod router;
mod validation;

pub use paths::{
    INGEST_MANIFEST_DAY_PATH, INGEST_MANIFEST_PATH, INGEST_PATH_PREFIX, INGEST_SEGMENTS_DAY_PATH,
    INGEST_UPLOAD_PATH,
};
pub use router::{router, router_with_notifier};

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod architecture_tests {
    // Unlike solstone-core-segment's older scanner, `public_signatures` also
    // recognizes `pub async fn`; handlers must not open a raw byte-write door.
    const SOURCES: &[&str] = &[
        include_str!("model.rs"),
        include_str!("read_routes.rs"),
        include_str!("router.rs"),
        include_str!("validation.rs"),
    ];

    #[test]
    fn crate_has_no_direct_journal_write_primitive() {
        for source in SOURCES {
            let source = source.split("#[cfg(test)]").next().unwrap_or(source);
            for primitive in [
                "std::fs::write",
                "File::create",
                "OpenOptions::new",
                ".write_all(",
                "write_bytes_exclusive",
                "hold_lock",
                "write_json",
                "append_jsonl",
            ] {
                assert!(
                    !source.contains(primitive),
                    "forbidden primitive {primitive}"
                );
            }
        }
    }

    #[test]
    fn public_byte_signatures_do_not_expose_raw_write_paths() {
        for source in SOURCES {
            let source = source.split("#[cfg(test)]").next().unwrap_or(source);
            for signature in public_signatures(source) {
                assert!(
                    !signature.contains("&[u8]"),
                    "public raw byte surface: {signature}"
                );
            }
        }
    }

    fn public_signatures(source: &str) -> impl Iterator<Item = &str> {
        source.lines().filter(|line| {
            let line = line.trim_start();
            line.starts_with("pub fn ") || line.starts_with("pub async fn ")
        })
    }
}
