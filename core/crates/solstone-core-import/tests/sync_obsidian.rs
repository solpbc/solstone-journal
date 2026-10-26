// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_import::contract::{SyncPreviewRequest, SyncSaveRequest};
use solstone_core_import::sync_obsidian::{
    ObsidianHomeCandidates, ObsidianNote, ObsidianPreviewSeams, ObsidianSaveSeams, ObsidianScanner,
    ObsidianSyncError, ObsidianSyncRequest, ObsidianWriter, sync_obsidian_preview,
    sync_obsidian_save,
};
use solstone_core_import::sync_plaud::SyncClock;
use solstone_core_import::{BackendName, SyncState, write_sync_state};
use tempfile::TempDir;

struct Candidates(Vec<PathBuf>);
impl ObsidianHomeCandidates for Candidates {
    fn candidates(&self) -> &[PathBuf] {
        &self.0
    }
}

struct Scanner {
    directories: BTreeSet<PathBuf>,
    notes: Vec<ObsidianNote>,
    scanned: RefCell<Vec<PathBuf>>,
}
impl ObsidianScanner for Scanner {
    fn is_directory(&self, path: &Path) -> bool {
        self.directories.contains(path)
    }

    fn notes(&self, vault: &Path) -> Result<Vec<ObsidianNote>, String> {
        self.scanned.borrow_mut().push(vault.to_path_buf());
        Ok(self.notes.clone())
    }
}

struct Clock;
impl SyncClock for Clock {
    fn now(&self) -> String {
        "2026-08-11T12:00:00+00:00".to_owned()
    }
}

#[test]
fn source_selection_is_explicit_then_retained_then_ordered_candidates_then_refusal() {
    let tree = TempDir::new().unwrap();
    let explicit = PathBuf::from("explicit");
    let retained = PathBuf::from("retained");
    let first_candidate = PathBuf::from("candidate-one");
    let second_candidate = PathBuf::from("candidate-two");
    let candidates = Candidates(vec![first_candidate.clone(), second_candidate.clone()]);
    let scanner = Scanner {
        directories: [explicit.clone(), retained.clone(), second_candidate.clone()]
            .into_iter()
            .collect(),
        notes: vec![note("present.md")],
        scanned: RefCell::new(Vec::new()),
    };
    let clock = Clock;

    let explicit_outcome = preview(
        tree.path(),
        Some(explicit.clone()),
        &candidates,
        &scanner,
        &clock,
    )
    .unwrap();
    assert_eq!(explicit_outcome.state.root()["source_path"], "explicit");

    write_retained_state(tree.path(), &retained, None);
    let retained_outcome = preview(tree.path(), None, &candidates, &scanner, &clock).unwrap();
    assert_eq!(retained_outcome.state.root()["source_path"], "retained");

    write_retained_state(tree.path(), &first_candidate, None);
    let candidate_outcome = preview(tree.path(), None, &candidates, &scanner, &clock).unwrap();
    assert_eq!(
        candidate_outcome.state.root()["source_path"],
        "candidate-two"
    );

    let no_candidates = Candidates(Vec::new());
    write_retained_state(tree.path(), &first_candidate, None);
    assert!(matches!(
        preview(tree.path(), None, &no_candidates, &scanner, &clock,),
        Err(ObsidianSyncError::NoVault)
    ));
}

#[test]
fn unseen_previously_imported_note_transitions_to_removed() {
    let tree = TempDir::new().unwrap();
    let vault = PathBuf::from("vault");
    let candidates = Candidates(Vec::new());
    let scanner = Scanner {
        directories: [vault.clone()].into_iter().collect(),
        notes: Vec::new(),
        scanned: RefCell::new(Vec::new()),
    };
    let clock = Clock;
    write_retained_state(tree.path(), &vault, Some("gone.md"));

    let outcome = preview(tree.path(), None, &candidates, &scanner, &clock).unwrap();
    assert_eq!(
        outcome.state.root()["files"]["gone.md"]["status"],
        "removed"
    );
}

#[test]
fn unchanged_imported_note_refreshes_its_numeric_mtime() {
    let tree = TempDir::new().unwrap();
    let vault = PathBuf::from("vault");
    let candidates = Candidates(Vec::new());
    let scanner = Scanner {
        directories: [vault.clone()].into_iter().collect(),
        notes: vec![note("present.md")],
        scanned: RefCell::new(Vec::new()),
    };
    let clock = Clock;
    let mut state = SyncState::empty(BackendName::Obsidian);
    state.root_mut().insert(
        "source_path".to_owned(),
        Value::String(vault.display().to_string()),
    );
    state.files_mut().insert(
        "present.md".to_owned(),
        serde_json::json!({
            "status": "imported",
            "content_hash": "hash",
            "mtime": 1.0
        }),
    );
    write_sync_state(tree.path(), &state).unwrap();

    let outcome = preview(tree.path(), None, &candidates, &scanner, &clock).unwrap();
    assert_eq!(
        outcome.state.root()["files"]["present.md"]["status"],
        "imported"
    );
    assert_eq!(
        outcome.state.root()["files"]["present.md"]["mtime"],
        serde_json::json!(1_725_000_000.5)
    );
}

fn preview(
    journal_root: &Path,
    source_path: Option<PathBuf>,
    candidates: &Candidates,
    scanner: &Scanner,
    clock: &Clock,
) -> Result<solstone_core_import::sync_obsidian::ObsidianSyncOutcome, ObsidianSyncError> {
    let mut seams = ObsidianPreviewSeams {
        candidates,
        scanner,
        clock,
    };
    let request = ObsidianSyncRequest::<SyncPreviewRequest>::new(
        journal_root.to_path_buf(),
        source_path,
        false,
    );
    sync_obsidian_preview(&request, &mut seams)
}

fn write_retained_state(journal_root: &Path, source_path: &Path, missing: Option<&str>) {
    let mut state = SyncState::empty(BackendName::Obsidian);
    state.root_mut().insert(
        "source_path".to_owned(),
        Value::String(source_path.display().to_string()),
    );
    if let Some(relative_path) = missing {
        state.files_mut().insert(
            relative_path.to_owned(),
            serde_json::json!({"status": "imported", "content_hash": "old"}),
        );
    }
    write_sync_state(journal_root, &state).unwrap();
}

fn note(relative_path: &str) -> ObsidianNote {
    ObsidianNote {
        relative_path: relative_path.to_owned(),
        filename: relative_path.to_owned(),
        title: relative_path.to_owned(),
        modified_at: 1_725_000_000.5,
        content_hash: "hash".to_owned(),
    }
}

struct StoppingWriter {
    stop: solstone_core_import::WorkStop,
    calls: RefCell<Vec<String>>,
}

impl ObsidianWriter for StoppingWriter {
    fn import_note(&mut self, _: &Path, note: &ObsidianNote) -> Result<u64, String> {
        self.calls.borrow_mut().push(note.relative_path.clone());
        self.stop.request();
        Ok(1)
    }
}

#[test]
fn stop_after_the_first_note_keeps_that_import_and_skips_the_next() {
    let tree = TempDir::new().unwrap();
    let vault = PathBuf::from("vault");
    let candidates = Candidates(Vec::new());
    let scanner = Scanner {
        directories: [vault.clone()].into_iter().collect(),
        notes: vec![note("one.md"), note("two.md")],
        scanned: RefCell::new(Vec::new()),
    };
    let clock = Clock;
    let stop = solstone_core_import::WorkStop::new();
    let _guard = solstone_core_import::WorkStopGuard::install(&stop);
    let mut writer = StoppingWriter {
        stop: stop.clone(),
        calls: RefCell::new(Vec::new()),
    };
    let mut seams = ObsidianSaveSeams {
        preview: ObsidianPreviewSeams {
            candidates: &candidates,
            scanner: &scanner,
            clock: &clock,
        },
        writer: &mut writer,
    };
    let outcome = sync_obsidian_save(
        &ObsidianSyncRequest::<SyncSaveRequest>::new(tree.path().to_path_buf(), Some(vault), false),
        &mut seams,
    )
    .unwrap();
    assert_eq!(writer.calls.borrow().as_slice(), ["one.md"]);
    assert_eq!(outcome.imported, 1);
    assert!(outcome.stopped);
    assert!(outcome.errors.is_empty());
    assert_eq!(
        outcome.state.root()["files"]["one.md"]["status"],
        "imported"
    );
    assert_eq!(
        outcome.state.root()["files"]["two.md"]["status"],
        "available"
    );
}
