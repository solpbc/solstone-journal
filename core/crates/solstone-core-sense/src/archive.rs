// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Recoverable archive admission into the ordinary priority-aware Sense workers.

use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use solstone_core_format::audio_archive::AudioArchivePlacement;
use solstone_core_processing_record::{
    TerminalProofOutcome, evaluate_terminal_proof, is_failure_exhausted, jsonl_has_row_with_key,
    read_processing_record_header, vocab,
};

use crate::SenseDispatcher;

pub const MAX_PENDING_FILES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveTranscription {
    Owed,
    Complete,
    /// The existing handler's retry policy has exhausted this input. Retain
    /// the pending fact; exhaustion is not successful processing.
    Held,
}

pub fn transcription_state(raw: &Path) -> ArchiveTranscription {
    let Ok(metadata) = std::fs::symlink_metadata(raw) else {
        return ArchiveTranscription::Held;
    };
    if !metadata.file_type().is_file() {
        return ArchiveTranscription::Held;
    }
    let output = raw.with_extension("jsonl");
    let record = read_processing_record_header(&output);
    if evaluate_terminal_proof(record.as_ref(), vocab::HANDLER_TRANSCRIBE, metadata.len())
        == TerminalProofOutcome::Held
    {
        return ArchiveTranscription::Complete;
    }
    if record.as_ref().is_some_and(|record| {
        record.get("schema").and_then(Value::as_str) == Some(vocab::SCHEMA)
            && record.get("handler").and_then(Value::as_str) == Some(vocab::HANDLER_TRANSCRIBE)
            && record.get("input_size").and_then(Value::as_u64) == Some(metadata.len())
            && is_failure_exhausted(record)
    }) {
        return ArchiveTranscription::Held;
    }
    // Older successful transcripts have no processing header. A failed
    // modern header must never be masked by a stray transcript row.
    if record.is_none() && jsonl_has_row_with_key(&output, vocab::AUDIO_TRANSCRIPT_ROW_KEY) {
        return ArchiveTranscription::Complete;
    }
    ArchiveTranscription::Owed
}

fn pump(dispatcher: &SenseDispatcher) {
    if dispatcher.archive_pending_files() >= MAX_PENDING_FILES {
        return;
    }
    let journal = dispatcher.journal();
    let batch = match solstone_core_import::audio_archive::update_pending_archive_tiles(journal, 64)
    {
        Ok(batch) => batch,
        Err(error) => {
            crate::log::warn!("archive sensing discovery: {error}");
            return;
        }
    };
    for (id, error) in batch.errors {
        crate::log::warn!("archive sensing {id}: {error}");
    }
    for tile in batch.tiles {
        if dispatcher.archive_pending_files() >= MAX_PENDING_FILES {
            break;
        }
        let directory = journal
            .join("chronicle")
            .join(&tile.day)
            .join(&tile.stream)
            .join(&tile.segment_key);
        let placement = std::fs::read(directory.join("placement.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<AudioArchivePlacement>(&bytes).ok());
        let Some(placement) = placement else {
            continue;
        };
        let mut files: Vec<_> = tile
            .parts
            .iter()
            .filter_map(|part| {
                let placed = placement
                    .parts
                    .iter()
                    .find(|p| p.stored_name == part.stored_name)?;
                (transcription_state(&directory.join(&part.stored_name))
                    == ArchiveTranscription::Owed)
                    .then_some((placed.offset_ms, part.stored_name.clone()))
            })
            .collect();
        files.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        if files.is_empty() {
            continue;
        }
        dispatcher.handle(&solstone_core_callosum::CallosumEnvelope {
            tract: "observe".into(),
            event: "observing".into(),
            ts: None,
            extra: serde_json::Map::from_iter([
                ("day".into(), json!(tile.day)),
                ("segment".into(), json!(tile.segment_key)),
                ("stream".into(), json!(tile.stream)),
                (
                    "files".into(),
                    json!(files.into_iter().map(|(_, name)| name).collect::<Vec<_>>()),
                ),
                ("source".into(), json!("audio_archive")),
                ("batch".into(), json!(true)),
                (
                    "meta".into(),
                    json!({"import_id":tile.import_id,"audio_archive":true}),
                ),
            ]),
        });
    }
}

/// The worker does SQLite/filesystem discovery away from the socket loop.
/// Joining it is part of the service's shutdown evidence.
pub(crate) fn start(
    dispatcher: Arc<SenseDispatcher>,
) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
    let (stop, receiver) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("archive-admission".into())
        .spawn(move || {
            let mut deferred = dispatcher.tally.deferred();
            loop {
                pump(&dispatcher);
                let current = dispatcher.tally.deferred();
                let delay = if current > deferred {
                    Duration::from_secs(30)
                } else {
                    Duration::from_secs(1)
                };
                deferred = current;
                match receiver.recv_timeout(delay) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    _ => break,
                }
            }
        })
        .expect("archive admission worker");
    (stop, worker)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    #[test]
    fn a_failed_or_stale_header_cannot_hide_behind_a_transcript_row() {
        let root = tempfile::tempdir().unwrap();
        let raw = root.path().join("part.ogg");
        std::fs::write(&raw, b"audio").unwrap();
        let output = raw.with_extension("jsonl");
        for (state, size, expected) in [
            ("analyzed", 5, ArchiveTranscription::Complete),
            ("empty", 5, ArchiveTranscription::Complete),
            ("analyzed", 6, ArchiveTranscription::Owed),
            ("failed", 5, ArchiveTranscription::Held),
        ] {
            let record = json!({"schema":vocab::SCHEMA,"state":state,"handler":"transcribe","input_size":size,"attempts":3});
            std::fs::write(
                &output,
                format!(
                    "{}\n{{\"start\":\"12:00:01\",\"text\":\"fixture\"}}\n",
                    json!({"_solstone_processing":record})
                ),
            )
            .unwrap();
            assert_eq!(transcription_state(&raw), expected);
        }
        std::fs::write(
            &output,
            "{}\n{\"start\":\"12:00:01\",\"text\":\"legacy\"}\n",
        )
        .unwrap();
        assert_eq!(transcription_state(&raw), ArchiveTranscription::Complete);
    }

    #[test]
    fn old_parakeet_availability_failures_remain_owed_in_archive_admission() {
        let root = tempfile::tempdir().unwrap();
        let raw = root.path().join("part.ogg");
        std::fs::write(&raw, b"audio").unwrap();
        for reason in ["server_not_ready", "no_port"] {
            for attempts in [3, 1000] {
                let header = json!({"_solstone_processing":{
                    "schema":vocab::SCHEMA,"state":"failed","handler":"transcribe",
                    "reason_code":reason,"attempts":attempts,"input_size":5
                }});
                std::fs::write(
                    raw.with_extension("jsonl"),
                    format!("{header}\n{{\"start\":\"12:00:01\",\"text\":\"stray row\"}}\n"),
                )
                .unwrap();
                assert_eq!(transcription_state(&raw), ArchiveTranscription::Owed);
                assert_eq!(std::fs::read(&raw).unwrap(), b"audio");
            }
        }
    }
}
