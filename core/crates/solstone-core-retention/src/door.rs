// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The only place in this tree that removes the owner's media.
//!
//! Every other module is barred from naming a removal primitive, and
//! `tests/architecture.rs` makes that a test failure rather than a convention.
//! Within this module every filesystem call goes through the journal-I/O
//! boundary's contained primitives, so containment is resolved immediately before
//! each syscall rather than once for a batch.
//!
//! # What this module does not do yet
//!
//! Only the raw-release verb. The whole-segment verb — with its staging rename,
//! its tombstone, its lock and its crash-recovery pass — is a later wave, and it
//! lands here, on an outcome model this verb will already have exercised.

#![allow(
    clippy::disallowed_methods,
    reason = "this is the door: the crate-wide ban exists so that only this module reaches a removal primitive"
)]

use std::path::Path;

use solstone_core_journal_io::entry::{Removed, remove_file, sync_dir};

use crate::eligibility::{Evidence, ProvenRaw};
use crate::receipt::{NotRemoved, Outcome, RemovedPath, Target, TargetOutcome};

/// How many files a run released on pre-record evidence rather than on a record.
///
/// Reported so a receipt can say which files rested on the weaker justification.
/// ⚠ The read-old rule requires honouring that evidence; it does not permit
/// honouring it silently.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EvidenceTally {
    pub on_record: usize,
    pub on_legacy_rows: usize,
}

/// Release proven raw originals, leaving every derived output in place.
///
/// ⛔ Takes proofs, never paths. A caller cannot ask this to remove a sidecar, a
/// derived output or journal-authored metadata, because it cannot construct the
/// value that would name one — the guarantee is that the request is
/// unrepresentable, not that a check refuses it.
///
/// ⛔ Returns [`Outcome`] and never `Result`: `?` is illegal in a function
/// returning it, so this cannot lose its own report by propagating an error out
/// from under the caller. That is the defect the reference implementation has —
/// an owner told an irreversible removal failed after it completed.
///
/// One [`TargetOutcome`] per segment, in first-seen order. A file that fails is a
/// row on its segment; ⛔ it never aborts the run and never becomes a run-level
/// halt, because one file cannot be allowed to discard a sibling's result.
pub fn release_raw(journal: &Path, proven: &[ProvenRaw]) -> (Outcome, EvidenceTally) {
    let mut outcome = Outcome {
        targets: Vec::new(),
        halted: None,
    };
    let mut tally = EvidenceTally::default();

    for item in proven {
        let target = Target {
            day: item.day().to_owned(),
            stream: item.stream().to_owned(),
            dir: item.dir().to_owned(),
        };
        // Group by segment: the row an owner reads is per-segment, and the proof
        // carries its own segment so no extra parameter is needed to find it.
        let index = match outcome
            .targets
            .iter()
            .position(|existing| existing.target == target)
        {
            Some(index) => index,
            None => {
                outcome.targets.push(TargetOutcome {
                    target,
                    removed: Vec::new(),
                    not_removed: Vec::new(),
                });
                outcome.targets.len().saturating_sub(1)
            }
        };
        let Some(row) = outcome.targets.get_mut(index) else {
            continue;
        };

        let rel = item.rel();
        match remove_file(journal, &rel) {
            Ok(Removed::Unlinked) => {
                // The directory entry is gone; make that durable before anything
                // reports it. `fsync` on a file does not persist the entry naming
                // it, and a record saying a removal happened at a given moment
                // has to be true at that moment.
                if let Err(error) = sync_dir(journal, &item.segment_rel()) {
                    row.not_removed.push(NotRemoved {
                        entry: rel.clone(),
                        reason: format!(
                            "removed, but the change could not be flushed to disk: {error}"
                        ),
                    });
                    continue;
                }
                row.removed.push(RemovedPath::confirmed(rel));
                match item.evidence() {
                    Evidence::Record => tally.on_record = tally.on_record.saturating_add(1),
                    Evidence::LegacyRows => {
                        tally.on_legacy_rows = tally.on_legacy_rows.saturating_add(1);
                    }
                }
            }
            // ⛔ Not a removal this run performed, so no RemovedPath is minted.
            // The receipt describes THIS run; conflating the two would report a
            // deletion that did not happen here.
            Ok(Removed::AlreadyAbsent) => row.not_removed.push(NotRemoved {
                entry: rel,
                reason: "this was already gone before the run started".to_owned(),
            }),
            Err(error) => row.not_removed.push(NotRemoved {
                entry: rel,
                reason: owner_reason(&error),
            }),
        }
    }

    (outcome, tally)
}

/// Turn a path error into something a person can act on.
///
/// ⛔ Never the raw error's `Display`: it carries absolute paths and errno prose,
/// and it is the text an owner would be shown about their own data.
fn owner_reason(error: &solstone_core_journal_io::errors::PathError) -> String {
    use solstone_core_journal_io::errors::PathError;
    match error {
        PathError::Escape(_) => "this is not inside your journal, so it was left alone".to_owned(),
        PathError::InvalidRelativePath { .. } => {
            "the location does not name a file inside your journal".to_owned()
        }
        PathError::Io { source, .. } => match source.kind() {
            std::io::ErrorKind::PermissionDenied => {
                "the file could not be read or removed, which usually means a permission problem"
                    .to_owned()
            }
            std::io::ErrorKind::IsADirectory => {
                "there is a folder where a recording was expected".to_owned()
            }
            _ => "the file could not be removed; the journal was left unchanged".to_owned(),
        },
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code; the crate-level denials exist to constrain the verbs"
)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::content::{ContentName, MediaClassifier};

    struct Bed {
        root: PathBuf,
    }

    impl Bed {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "retention-door-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn segment(&self, day: &str, stream: &str, dir: &str) -> PathBuf {
            let path = self.root.join(format!("chronicle/{day}/{stream}/{dir}"));
            fs::create_dir_all(&path).unwrap();
            path
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.root);
        }
    }

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn media_only(name: &ContentName) -> bool {
        matches!(name.extension().as_deref(), Some("flac" | "mp4" | "wav"))
    }

    fn proof(day: &str, stream: &str, dir: &str, name: &str, size: u64) -> ProvenRaw {
        let classifier: &dyn MediaClassifier = &media_only;
        ProvenRaw::for_test(classifier, day, stream, dir, name, size).unwrap()
    }

    #[test]
    fn releases_only_the_proven_files_and_leaves_everything_else() {
        let bed = Bed::new();
        let segment = bed.segment("20260805", "field.audio", "070000_17");
        for name in ["a.flac", "a.jsonl", "stream.json"] {
            fs::write(segment.join(name), b"bytes").unwrap();
        }
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(segment.join("talents/sense.json"), b"{}").unwrap();

        let (outcome, tally) = release_raw(
            &bed.root,
            &[proof("20260805", "field.audio", "070000_17", "a.flac", 5)],
        );

        assert!(outcome.halted.is_none());
        assert_eq!(outcome.targets.len(), 1);
        assert_eq!(outcome.targets[0].removed.len(), 1);
        assert!(outcome.targets[0].not_removed.is_empty());
        assert_eq!(tally.on_record, 1);

        assert!(!segment.join("a.flac").exists(), "the proven raw is gone");
        for survivor in ["a.jsonl", "stream.json", "talents/sense.json"] {
            assert!(
                segment.join(survivor).exists(),
                "{survivor} must survive a raw release"
            );
        }
        assert!(
            !segment.join("tombstone.json").exists(),
            "a released segment is not a removed segment"
        );
    }

    /// Two files in the SAME segment failing for different reasons are both
    /// reported, and the run does not abort.
    ///
    /// This is the shape a single failure slot could not represent, one level
    /// below the run: the verb groups N files into one row.
    #[test]
    fn two_files_in_one_segment_failing_differently_are_both_reported() {
        let bed = Bed::new();
        let segment = bed.segment("20260805", "field.audio", "070000_17");
        fs::write(segment.join("kept.flac"), b"gone").unwrap();
        // A directory where a recording was expected: unlink refuses it.
        fs::create_dir_all(segment.join("dir.flac")).unwrap();
        // And one that is simply not there.

        let (outcome, _) = release_raw(
            &bed.root,
            &[
                proof("20260805", "field.audio", "070000_17", "kept.flac", 4),
                proof("20260805", "field.audio", "070000_17", "dir.flac", 4),
                proof("20260805", "field.audio", "070000_17", "absent.flac", 4),
            ],
        );

        assert_eq!(outcome.targets.len(), 1, "one segment, one row");
        let row = &outcome.targets[0];
        assert_eq!(row.removed.len(), 1);
        assert_eq!(row.not_removed.len(), 2, "both failures are reported");
        assert_ne!(
            row.not_removed[0].reason, row.not_removed[1].reason,
            "the two reasons are distinct"
        );
        assert!(outcome.halted.is_none(), "a file failure is not a run halt");
    }

    /// An absent file is disclosed, and mints no removal.
    #[test]
    fn an_already_absent_file_mints_no_removed_path() {
        let bed = Bed::new();
        bed.segment("20260805", "field.audio", "070000_17");
        let (outcome, tally) = release_raw(
            &bed.root,
            &[proof(
                "20260805",
                "field.audio",
                "070000_17",
                "gone.flac",
                4,
            )],
        );
        assert!(outcome.targets[0].removed.is_empty());
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert_eq!(tally.on_record, 0);
    }

    #[test]
    fn files_from_two_segments_produce_two_rows_in_first_seen_order() {
        let bed = Bed::new();
        for dir in ["070000_17", "070100_17"] {
            let segment = bed.segment("20260805", "field.audio", dir);
            fs::write(segment.join("a.flac"), b"x").unwrap();
        }
        let (outcome, _) = release_raw(
            &bed.root,
            &[
                proof("20260805", "field.audio", "070100_17", "a.flac", 1),
                proof("20260805", "field.audio", "070000_17", "a.flac", 1),
            ],
        );
        assert_eq!(outcome.targets.len(), 2);
        assert_eq!(outcome.targets[0].target.dir, "070100_17");
        assert_eq!(outcome.targets[1].target.dir, "070000_17");
    }

    #[test]
    fn a_second_run_over_the_same_set_removes_nothing() {
        let bed = Bed::new();
        let segment = bed.segment("20260805", "field.audio", "070000_17");
        fs::write(segment.join("a.flac"), b"x").unwrap();
        let set = [proof("20260805", "field.audio", "070000_17", "a.flac", 1)];

        let (first, _) = release_raw(&bed.root, &set);
        assert_eq!(first.targets[0].removed.len(), 1);

        let (second, tally) = release_raw(&bed.root, &set);
        assert!(second.targets[0].removed.is_empty(), "idempotent");
        assert_eq!(second.targets[0].not_removed.len(), 1);
        assert_eq!(tally.on_record, 0);
    }

    #[test]
    fn every_minted_path_is_absent_when_the_verb_returns() {
        let bed = Bed::new();
        let segment = bed.segment("20260805", "field.audio", "070000_17");
        for name in ["a.flac", "b.wav"] {
            fs::write(segment.join(name), b"x").unwrap();
        }
        let (outcome, _) = release_raw(
            &bed.root,
            &[
                proof("20260805", "field.audio", "070000_17", "a.flac", 1),
                proof("20260805", "field.audio", "070000_17", "b.wav", 1),
            ],
        );
        let minted: Vec<&str> = outcome.removed_paths().map(RemovedPath::as_str).collect();
        assert_eq!(minted.len(), 2);
        for rel in minted {
            assert!(!rel.starts_with('/'), "{rel} must be journal-relative");
            assert!(!rel.contains(".."), "{rel} must not contain ..");
            assert!(
                !bed.root.join(rel).exists(),
                "{rel} was minted but still exists"
            );
        }
    }

    /// A reason must be readable by a person, not an errno dump.
    #[test]
    fn a_failure_reason_is_owner_readable() {
        let bed = Bed::new();
        let segment = bed.segment("20260805", "field.audio", "070000_17");
        fs::create_dir_all(segment.join("dir.flac")).unwrap();
        let (outcome, _) = release_raw(
            &bed.root,
            &[proof("20260805", "field.audio", "070000_17", "dir.flac", 1)],
        );
        let reason = &outcome.targets[0].not_removed[0].reason;
        assert!(!reason.contains(&bed.root.display().to_string()));
        assert!(reason.contains("folder"), "got {reason}");
    }
}
