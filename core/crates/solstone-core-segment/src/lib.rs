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
//! A `tombstone.json` sidecar marks a terminal content-identity state. Ordinary
//! segment deletion is owned by a separate crate; this crate removes bytes only
//! inside the one-time layout migrations it owns, and only through journal-io's
//! removal door.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod chronicle_migration;
mod content_name;
mod device;
mod document_migration;
mod error;
mod identity;
mod manifest;
mod projection;
mod relocate;
mod segment_dir;
mod stream_record;
mod stream_repair;
mod supervisor;
#[cfg(test)]
pub(crate) mod test_support;
mod write;

pub use chronicle_migration::{
    ChronicleMigrationError, ChronicleMigrationReport, migrate_root_days_to_chronicle,
};
pub use content_name::{
    ContentName, ContentNameError, RESERVED_SEGMENT_FILENAMES, is_reserved_name,
};
pub use device::{
    AiChatSource, DeviceSidecarInput, ImportSource, Kind, is_valid_device_cid, write_device,
};
pub use document_migration::{
    PdfExtractionMigrationError, PdfExtractionMigrationReport, migrate_pdf_extractions,
};
pub use error::SegmentError;
pub use identity::{
    ContentIdentity, ContentIdentityEvidence, ContentIdentityFile, TerminalProofVerifier,
    load_content_identity,
};
pub use projection::project_stream_name;
pub use relocate::{
    AgentLayoutMigrationReport, Relocation, RelocationEnd, RelocationError, RelocationOutcome,
    RelocationRefusal, SegmentRestructureReport, available_segment_key, migrate_agent_layout,
    relocate_segment, restructure_segments_by_stream,
};
pub use segment_dir::{
    SegmentDir, is_safe_stream_component, list_days, list_segments, list_segments_in,
};
pub use solstone_core_journal_io::{
    DEFAULT_STREAM, DirEntryKind, LockOptions, PathOrDay, RecordIdentity, Segment,
    SegmentIdentityError, StreamLocation, check_record_identities, day_path, hold_lock,
    iter_segments, list_dir_entries, read_text,
};
pub use stream_record::{
    BoundStream, ResolvedStream, StreamAdvance, StreamHints, StreamRecord,
    UnboundStreamAdvanceError, advance_bound_stream, advance_unbound_stream, bind_named_stream,
    bind_stream, delete_stream_record, has_unattributed_stream_record, lookup_stream,
    resolve_stream,
};
pub use stream_repair::{
    MarkerTail, RepairOutcome, StreamBackfillReport, StreamBackfillSignal, StreamClassification,
    StreamRepairError, TolerantStreamRecords, UnchangedReason, backfill_stream_records,
    list_stream_records_tolerant, read_stream_record, repair_stream_tail_from_markers,
    set_stream_tail_unconditionally, touch_stream_health_marker,
};
pub use supervisor::{
    SUPERVISOR_MESSAGE, SupervisorRefusal, is_solstone_up, read_convey_port, require_solstone,
    require_solstone_with,
};
pub use write::{ContentDescriptor, ContentWriteOutcome, write_content};

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod architecture_tests {
    // Textual structural checks intentionally mirror scripts/check_layer_hygiene.py.
    #[derive(Clone, Copy)]
    enum Source {
        ChronicleMigration,
        ContentName,
        Device,
        DocumentMigration,
        Error,
        Identity,
        Manifest,
        Projection,
        Relocate,
        SegmentDir,
        StreamRepair,
        StreamRecord,
        Supervisor,
        Write,
    }

    const SOURCES: &[(Source, &str)] = &[
        (
            Source::ChronicleMigration,
            include_str!("chronicle_migration.rs"),
        ),
        (Source::ContentName, include_str!("content_name.rs")),
        (Source::Device, include_str!("device.rs")),
        (
            Source::DocumentMigration,
            include_str!("document_migration.rs"),
        ),
        (Source::Error, include_str!("error.rs")),
        (Source::Identity, include_str!("identity.rs")),
        (Source::Manifest, include_str!("manifest.rs")),
        (Source::Projection, include_str!("projection.rs")),
        (Source::Relocate, include_str!("relocate.rs")),
        (Source::SegmentDir, include_str!("segment_dir.rs")),
        (Source::StreamRepair, include_str!("stream_repair.rs")),
        (Source::StreamRecord, include_str!("stream_record.rs")),
        (Source::Supervisor, include_str!("supervisor.rs")),
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
                    assert!(source.contains("remove_file"));
                }
                Source::StreamRepair => {
                    for primitive in ["hold_lock", "write_stream_record", "bump_stream_marker"] {
                        assert!(
                            source.contains(primitive),
                            "missing permitted journal-io write primitive {primitive}"
                        );
                    }
                }
                // A segment move re-authors bytes that already exist. It replaces
                // and renames; it never opens a new exclusive content file nor
                // appends to a log.
                Source::Relocate => {
                    for primitive in ["rename_within", "atomic_replace", "write_json"] {
                        assert!(
                            source.contains(primitive),
                            "missing permitted journal-io write primitive {primitive}"
                        );
                    }
                    for primitive in ["write_bytes_exclusive", "append_jsonl"] {
                        assert!(
                            !source.contains(primitive),
                            "unexpected journal-io write primitive {primitive}"
                        );
                    }
                }
                // A one-time layout migration relocates whole trees that already
                // exist. It renames and removes; it never authors new content
                // bytes or appends to a log.
                Source::ChronicleMigration => {
                    for primitive in ["rename_within", "remove_dir_all"] {
                        assert!(
                            source.contains(primitive),
                            "missing permitted journal-io write primitive {primitive}"
                        );
                    }
                    for primitive in ["write_bytes_exclusive", "append_jsonl", "write_json"] {
                        assert!(
                            !source.contains(primitive),
                            "unexpected journal-io write primitive {primitive}"
                        );
                    }
                }
                // Converting a legacy extraction authors one markdown transcript
                // and unlinks the superseded source. It never appends to a log.
                Source::DocumentMigration => {
                    for primitive in ["write_text", "remove_file"] {
                        assert!(
                            source.contains(primitive),
                            "missing permitted journal-io write primitive {primitive}"
                        );
                    }
                    for primitive in ["write_bytes_exclusive", "append_jsonl"] {
                        assert!(
                            !source.contains(primitive),
                            "unexpected journal-io write primitive {primitive}"
                        );
                    }
                }
                Source::ContentName
                | Source::Error
                | Source::Identity
                | Source::Manifest
                | Source::Projection
                | Source::SegmentDir
                | Source::Supervisor => {
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
