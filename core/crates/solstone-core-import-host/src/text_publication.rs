// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Terminal attempt and publication for generic text import.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_import::ImportError;
use solstone_core_import::metadata::{
    AttemptFacts, AttemptRead, AttemptState, IMPORT_FAILED_REASON, hold_import_lock,
    read_attempt_facts, read_provenance, record_completed_attempt_unlocked,
    record_unconfirmed_attempt_unlocked,
};
use solstone_core_import::publish::{
    CreatedSegment, NativePublicationOperations, PublicationInput, PublicationOperations,
    PublicationRecord, PublishError, publish_with_operations,
};
use solstone_core_import::text::TextCreated;

/// Input to terminal text attempt handling.
pub enum TextTerminalInput<'a> {
    Success(&'a [TextCreated]),
    Failed(&'a [TextCreated]),
}

impl<'a> TextTerminalInput<'a> {
    pub fn created(&self) -> &'a [TextCreated] {
        match self {
            Self::Success(slice) | Self::Failed(slice) => slice,
        }
    }
}

/// Result of a successful terminal attempt resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextFinish {
    Applied,
    Stale,
}

/// Errors that can occur during terminal attempt finalization.
#[derive(Debug)]
pub enum TextFinishError {
    Lock,
    ProvenanceUnreadable,
    AttemptAbsent,
    AttemptMalformed,
    AttemptWrite,
}

impl fmt::Display for TextFinishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock => formatter.write_str("failed to acquire import lock"),
            Self::ProvenanceUnreadable => formatter.write_str("failed to read import provenance"),
            Self::AttemptAbsent => formatter.write_str("import attempt facts absent"),
            Self::AttemptMalformed => formatter.write_str("import attempt facts malformed"),
            Self::AttemptWrite => formatter.write_str("failed to write terminal attempt facts"),
        }
    }
}

impl std::error::Error for TextFinishError {}

type HoldLockFn<'a> =
    &'a dyn Fn(&Path, &str) -> Result<solstone_core_journal_io::FileLock, ImportError>;
type PublishFn<'a> = &'a dyn Fn(
    PublicationInput<'_>,
    &dyn PublicationOperations,
) -> Result<PublicationRecord, PublishError>;
type RecordCompletedFn<'a> = &'a dyn Fn(
    &Path,
    &str,
    u64,
    u64,
    Option<u64>,
    Option<String>,
) -> Result<AttemptFacts, ImportError>;
type RecordUnconfirmedFn<'a> =
    &'a dyn Fn(&Path, &str, u64, u64, Option<String>) -> Result<AttemptFacts, ImportError>;

/// Test seams for terminal text publication and attempt recording.
#[derive(Default)]
pub struct TextTerminalSeams<'a> {
    pub hold_lock_fn: Option<HoldLockFn<'a>>,
    pub publish_fn: Option<PublishFn<'a>>,
    pub record_completed_fn: Option<RecordCompletedFn<'a>>,
    pub record_unconfirmed_fn: Option<RecordUnconfirmedFn<'a>>,
}

pub fn finish_text_attempt(
    journal: &Path,
    import_id: &str,
    generation: u64,
    input: TextTerminalInput<'_>,
) -> Result<TextFinish, TextFinishError> {
    finish_text_attempt_with(
        journal,
        import_id,
        generation,
        input,
        TextTerminalSeams::default(),
    )
}

pub fn finish_text_attempt_with(
    journal: &Path,
    import_id: &str,
    generation: u64,
    input: TextTerminalInput<'_>,
    seams: TextTerminalSeams<'_>,
) -> Result<TextFinish, TextFinishError> {
    let _lock = match seams.hold_lock_fn {
        Some(custom) => match custom(journal, import_id) {
            Ok(lock) => lock,
            Err(_) => return Err(TextFinishError::Lock),
        },
        None => match hold_import_lock(journal, import_id) {
            Ok(lock) => lock,
            Err(_) => return Err(TextFinishError::Lock),
        },
    };

    let provenance = match read_provenance(journal, import_id) {
        Ok(Some(meta)) => meta,
        Ok(None) => return Err(TextFinishError::AttemptAbsent),
        Err(_) => return Err(TextFinishError::ProvenanceUnreadable),
    };

    let attempt_facts = match read_attempt_facts(&provenance) {
        AttemptRead::Present(facts) => facts,
        AttemptRead::Malformed => return Err(TextFinishError::AttemptMalformed),
        AttemptRead::Absent => return Err(TextFinishError::AttemptAbsent),
    };

    if attempt_facts.generation != generation || attempt_facts.state != AttemptState::Running {
        return Ok(TextFinish::Stale);
    }

    let finished_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let created_segments: Vec<CreatedSegment> = input
        .created()
        .iter()
        .map(|c| c.created_segment())
        .collect();
    let files_created: Vec<PathBuf> = input.created().iter().map(|c| c.path.clone()).collect();

    let mut publication_failed = false;
    let should_publish = match &input {
        TextTerminalInput::Success(_) => true,
        TextTerminalInput::Failed(created) => !created.is_empty(),
    };

    if should_publish {
        let import_dir = journal.join("imports").join(import_id);
        let may_write = || {
            read_provenance(journal, import_id)
                .ok()
                .flatten()
                .and_then(|metadata| match read_attempt_facts(&metadata) {
                    AttemptRead::Present(facts) => Some(facts),
                    AttemptRead::Absent | AttemptRead::Malformed => None,
                })
                .is_some_and(|facts| facts.generation == generation)
        };

        let pub_input = PublicationInput {
            journal,
            import_dir: Some(&import_dir),
            import_id,
            importer: "text",
            revision: None,
            segments: &created_segments,
            files_created: &files_created,
            may_write_record: Some(&may_write),
        };

        let pub_res = match seams.publish_fn {
            Some(custom) => custom(pub_input, &NativePublicationOperations),
            None => publish_with_operations(pub_input, &NativePublicationOperations),
        };

        if pub_res.is_err() {
            publication_failed = true;
        }
    }

    let record_res = match &input {
        TextTerminalInput::Success(_) if !publication_failed => match seams.record_completed_fn {
            Some(custom) => custom(journal, import_id, generation, finished_at_ms, None, None),
            None => record_completed_attempt_unlocked(
                journal,
                import_id,
                generation,
                finished_at_ms,
                None,
                None,
            ),
        },
        TextTerminalInput::Success(_) => match seams.record_unconfirmed_fn {
            Some(custom) => custom(journal, import_id, generation, finished_at_ms, None),
            None => record_unconfirmed_attempt_unlocked(
                journal,
                import_id,
                generation,
                finished_at_ms,
                None,
            ),
        },
        TextTerminalInput::Failed(_) => match seams.record_unconfirmed_fn {
            Some(custom) => custom(
                journal,
                import_id,
                generation,
                finished_at_ms,
                Some(IMPORT_FAILED_REASON.to_owned()),
            ),
            None => record_unconfirmed_attempt_unlocked(
                journal,
                import_id,
                generation,
                finished_at_ms,
                Some(IMPORT_FAILED_REASON.to_owned()),
            ),
        },
    };

    match record_res {
        Ok(_) => Ok(TextFinish::Applied),
        Err(_) => Err(TextFinishError::AttemptWrite),
    }
}
