// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The single native write door for journal segment content and sidecars.
//!
//! Segment timestamps are device-local wall-clock values, never UTC. Production
//! filesystem writes flow only through `solstone-core-journal-io`; direct reads
//! remain permitted for collision comparison and legacy content discovery.
//! `device.json` is written here as a journal-authored sidecar, never as
//! client-uploaded content. This crate intentionally does not write `ingest.json`:
//! distinguishing a crash
//! partial write from a legacy manifest-less segment requires an ingest writer
//! and coordinated Python-reader change, both outside this wave.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod content_name;
mod device;
mod error;
mod identity;
mod manifest;
mod projection;
mod segment_dir;
mod sidecars;
mod stream_record;
#[cfg(test)]
pub(crate) mod test_support;
mod write;

pub use content_name::{
    ContentName, ContentNameError, RESERVED_SEGMENT_FILENAMES, is_reserved_name,
};
pub use device::{AiChatSource, DeviceSidecarInput, ImportSource, Kind, write_device};
pub use error::SegmentError;
pub use identity::{
    ContentIdentity, ContentIdentityEvidence, ContentIdentityFile, TerminalProofVerifier,
    load_content_identity,
};
pub use projection::project_stream_name;
pub use segment_dir::SegmentDir;
pub use sidecars::append_event;
pub use stream_record::{
    BoundStream, ResolvedStream, StreamAdvance, StreamHints, StreamRecord, advance_bound_stream,
    bind_stream, lookup_stream, resolve_stream,
};
pub use write::{ContentDescriptor, ContentWriteOutcome, write_content};

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod architecture_tests {
    // Textual structural checks intentionally mirror scripts/check_layer_hygiene.py.
    #[derive(Clone, Copy)]
    enum Source {
        ContentName,
        Device,
        Error,
        Identity,
        Manifest,
        Projection,
        SegmentDir,
        Sidecars,
        StreamRecord,
        Write,
    }

    const SOURCES: &[(Source, &str)] = &[
        (Source::ContentName, include_str!("content_name.rs")),
        (Source::Device, include_str!("device.rs")),
        (Source::Error, include_str!("error.rs")),
        (Source::Identity, include_str!("identity.rs")),
        (Source::Manifest, include_str!("manifest.rs")),
        (Source::Projection, include_str!("projection.rs")),
        (Source::SegmentDir, include_str!("segment_dir.rs")),
        (Source::Sidecars, include_str!("sidecars.rs")),
        (Source::StreamRecord, include_str!("stream_record.rs")),
        (Source::Write, include_str!("write.rs")),
    ];

    #[test]
    fn public_byte_writers_require_typed_names_and_handles() {
        for (_, source) in SOURCES {
            for signature in public_signatures(source) {
                if signature.contains("&[u8]") {
                    assert!(
                        !signature.contains("&str") && !signature.contains("&Path"),
                        "raw name/path byte writer: {signature}"
                    );
                    assert!(
                        signature.contains("ContentName") && signature.contains("SegmentDir"),
                        "typed byte writer missing ContentName or SegmentDir: {signature}"
                    );
                }
            }
        }
    }

    #[test]
    fn sidecar_write_surface_is_closed() {
        for (kind, source) in SOURCES {
            match kind {
                Source::Write => {
                    assert!(source.contains("write_bytes_exclusive"));
                }
                Source::Device => {
                    assert!(source.contains("write_bytes_exclusive"));
                    for primitive in ["hold_lock", "write_json", "atomic_replace", "append_jsonl"] {
                        assert!(
                            !source.contains(primitive),
                            "unexpected journal-io write primitive {primitive}"
                        );
                    }
                }
                Source::StreamRecord => {
                    assert!(source.contains("hold_lock"));
                    assert!(source.contains("write_json"));
                }
                Source::Sidecars => {
                    assert!(source.contains("append_jsonl"));
                }
                Source::ContentName
                | Source::Error
                | Source::Identity
                | Source::Manifest
                | Source::Projection
                | Source::SegmentDir => {
                    for primitive in [
                        "write_bytes_exclusive",
                        "hold_lock",
                        "write_json",
                        "atomic_replace",
                        "append_jsonl",
                    ] {
                        assert!(
                            !source.contains(primitive),
                            "unexpected journal-io write primitive {primitive}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn content_identity_has_no_default_escape_hatch() {
        let identity = include_str!("identity.rs");
        assert!(!identity.contains("impl Default for ContentIdentity"));
        assert!(!identity.contains("unwrap_or_default"));
    }

    fn public_signatures(source: &str) -> Vec<&str> {
        source
            .match_indices("pub fn")
            .map(|(start, _)| {
                let remainder = &source[start..];
                let end = remainder
                    .find('{')
                    .expect("public function must have a body");
                &remainder[..end]
            })
            .collect()
    }
}
