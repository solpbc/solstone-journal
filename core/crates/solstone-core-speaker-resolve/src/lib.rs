// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-backed speaker-attribution resolution.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod admission;
pub mod artifact_wipe;
pub mod audio_sample;
pub mod backfill;
pub mod backfill_operations;
pub mod bootstrap;
pub mod candidate_pair_suggestions;
pub mod candidate_tracker;
pub mod direct_voiceprints;
pub mod discovery_cache;
pub mod eligibility;
pub mod evidence;
pub mod identify_cluster;
pub mod identify_forward_phases;
pub mod identify_operations;
pub mod identify_target;
pub mod identify_undo;
pub mod identify_undo_phases;
pub mod keep_separate;
pub mod layer1;
pub mod layer2;
pub mod layer3;
pub mod name_variant_scan;
pub mod owner_admission;
pub mod owner_candidate;
pub mod owner_centroid;
pub mod owner_contamination_screen;
pub mod owner_provisional;
pub mod resolve;
pub mod retroactive_confirm;
pub mod speaker_candidate_pair_review_candidates;
pub mod speaker_review_candidates;
pub mod voiceprint_accumulation;
pub mod voiceprint_centroid;
pub mod voiceprint_metadata;

pub use owner_admission::{OWNER_IDENTITY_INVALID_REASON, OWNER_TARGET_MISMATCH_REASON};

/// Resolve a speaker record's segment, where `_default` denotes direct layout.
pub fn segment_path(
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: &str,
    create: bool,
) -> Result<std::path::PathBuf, solstone_core_journal_io::PathError> {
    use solstone_core_journal_io::PathError;
    use solstone_core_segment::{SegmentDir, SegmentError};
    let directory =
        SegmentDir::resolve(journal, day, segment, stream).map_err(|error| match error {
            SegmentError::Path(error) => error,
            SegmentError::StreamInput(message) => PathError::InvalidRelativePath {
                rel: format!("{day}/{stream}/{segment}"),
                message,
            },
            other => PathError::Io {
                path: journal.to_path_buf(),
                source: std::io::Error::other(other),
            },
        })?;
    let path = directory.path().to_path_buf();
    if create {
        std::fs::create_dir_all(&path).map_err(|source| PathError::Io {
            path: path.clone(),
            source,
        })?;
    }
    Ok(path)
}
