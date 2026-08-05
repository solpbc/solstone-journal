// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The single native write door for journal segment content and sidecars.
//!
//! Segment timestamps are device-local wall-clock values, never UTC. Production
//! filesystem writes flow only through `solstone-core-journal-io`; direct reads
//! remain permitted for collision comparison and legacy content discovery.
//! `device.json` is reserved but has no writer in this crate. Likewise, this
//! crate intentionally does not write `ingest.json`: distinguishing a crash
//! partial write from a legacy manifest-less segment requires an ingest writer
//! and coordinated Python-reader change, both outside this wave.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod content_name;
mod error;
mod identity;
mod manifest;
mod segment_dir;
mod sidecars;
mod stream_record;
#[cfg(test)]
pub(crate) mod test_support;
mod write;

pub use content_name::{
    ContentName, ContentNameError, RESERVED_SEGMENT_FILENAMES, is_reserved_name,
};
pub use error::SegmentError;
pub use identity::{
    ContentIdentity, ContentIdentityEvidence, ContentIdentityFile, load_content_identity,
};
pub use segment_dir::SegmentDir;
pub use sidecars::append_event;
pub use stream_record::{StreamAdvance, StreamHints, StreamRecord, advance_stream};
pub use write::{ContentDescriptor, ContentWriteOutcome, write_content};

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod architecture_tests {
    // Textual structural checks intentionally mirror scripts/check_layer_hygiene.py.
    const SOURCES: &[&str] = &[
        include_str!("content_name.rs"),
        include_str!("error.rs"),
        include_str!("identity.rs"),
        include_str!("manifest.rs"),
        include_str!("segment_dir.rs"),
        include_str!("sidecars.rs"),
        include_str!("stream_record.rs"),
        include_str!("write.rs"),
    ];

    #[test]
    fn public_byte_writers_require_typed_names_and_handles() {
        for source in SOURCES {
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
        for source in SOURCES {
            match *source {
                source if source == include_str!("write.rs") => {
                    assert!(source.contains("write_bytes_exclusive"));
                }
                source if source == include_str!("stream_record.rs") => {
                    assert!(source.contains("hold_lock"));
                    assert!(source.contains("write_json"));
                }
                source if source == include_str!("sidecars.rs") => {
                    assert!(source.contains("append_jsonl"));
                }
                source => {
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
