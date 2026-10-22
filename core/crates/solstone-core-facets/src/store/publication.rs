// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_indexer_store::{SavedPublicationOutcome, attempt_saved_publication};

/// Queue a saved facet file. No index lease or entity trust is needed.
pub(super) fn publish_saved(journal_root: &Path, saved: &Path) {
    let attempt = attempt_saved_publication(journal_root, saved, |journal, path| {
        solstone_core_indexer_store::todo::enqueue_path(journal, path).map_err(|e| e.to_string())
    });
    if attempt.outcome == SavedPublicationOutcome::Failed {
        log::warn!(
            target: "solstone::indexer",
            "saved facet file left for the daily scan path={} cause={}",
            attempt.path,
            attempt.cause.as_deref().unwrap_or("unknown")
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{
        ActivityRecord, AppendOutcome, append_activity_record, create_facet, save_detected_entity,
    };

    fn record(value: Value) -> ActivityRecord {
        value.as_object().unwrap().clone()
    }

    fn searchable(journal: &std::path::Path, path: &str, text: &str) -> bool {
        let conn = solstone_core_indexer_store::db::open_index(journal).unwrap();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chunks WHERE path=?1 AND content LIKE ?2)",
            [path, &format!("%{text}%")],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn a_saved_activity_record_is_searchable_after_a_drain() {
        let root = tempfile::tempdir().unwrap();
        create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let appended = append_activity_record(
            root.path(),
            "work",
            "20260510",
            record(json!({"id": "a1", "activity": "meeting", "title": "Kestrel review"})),
        )
        .unwrap();
        assert!(matches!(appended, AppendOutcome::Written(_)));
        let report =
            solstone_core_indexer_store::todo::drain(root.path(), Default::default()).unwrap();
        assert_eq!(report.failed, 0);
        let check = solstone_core_indexer_store::scan::scan_journal(root.path(), false).unwrap();
        assert_eq!((check.indexed, check.removed), (0, 0));
        assert!(searchable(
            root.path(),
            "facets/work/activities/20260510.jsonl",
            "Kestrel review"
        ));
    }

    #[test]
    fn a_write_under_trust_reaches_search_when_the_trust_is_released() {
        let root = tempfile::tempdir().unwrap();
        create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let appended = append_activity_record(
            root.path(),
            "work",
            "20260510",
            record(json!({"id": "a1", "activity": "meeting", "title": "Heron sync"})),
        )
        .unwrap();
        assert!(matches!(appended, AppendOutcome::Written(_)));
        // A scan holds the writer lease, and the caller holds facet trust
        // around the write, as enrichment and the daily schedule do.
        let lease = solstone_core_journal_io::hold_lock(
            solstone_core_indexer_store::db::db_path(root.path()),
            solstone_core_journal_io::LockOptions::default(),
        )
        .unwrap();
        let trust = crate::hold_facet_trust_lock(root.path()).unwrap();
        let started = std::time::Instant::now();
        crate::update_activity_record(
            root.path(),
            "work",
            "20260510",
            "a1",
            &serde_json::Map::from_iter([("title".to_owned(), json!("Heron retro"))]),
            "test",
            "renamed",
            "2026-05-10T00:00:00Z",
        )
        .unwrap()
        .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let path = "facets/work/activities/20260510.jsonl";
        assert!(!searchable(root.path(), path, "Heron retro"));
        // The scan finishes, then the caller's guard drops: the update runs.
        drop(lease);
        drop(trust);
        let report =
            solstone_core_indexer_store::todo::drain(root.path(), Default::default()).unwrap();
        assert_eq!(report.failed, 0);
        let check = solstone_core_indexer_store::scan::scan_journal(root.path(), false).unwrap();
        assert_eq!((check.indexed, check.removed), (0, 0));
        assert!(searchable(root.path(), path, "Heron retro"));
    }

    #[test]
    fn a_release_never_waits_on_the_index_lease_while_holding_trust() {
        let root = tempfile::tempdir().unwrap();
        create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let appended = append_activity_record(
            root.path(),
            "work",
            "20260510",
            record(json!({"id": "a1", "activity": "meeting", "title": "Plover sync"})),
        )
        .unwrap();
        assert!(matches!(appended, AppendOutcome::Written(_)));
        let trust = crate::hold_facet_trust_lock(root.path()).unwrap();
        // Another thread takes the index lease and then asks for trust, the
        // order reconciliation uses.
        let (lease_held, lease_is_held) = std::sync::mpsc::channel();
        let journal = root.path().to_path_buf();
        let reconcile = std::thread::spawn(move || {
            let lease = solstone_core_journal_io::hold_lock(
                solstone_core_indexer_store::db::db_path(&journal),
                solstone_core_journal_io::LockOptions::default(),
            )
            .unwrap();
            lease_held.send(()).unwrap();
            let trust = crate::hold_facet_trust_lock(&journal).unwrap();
            drop(trust);
            drop(lease);
        });
        lease_is_held.recv().unwrap();
        crate::update_activity_record(
            root.path(),
            "work",
            "20260510",
            "a1",
            &serde_json::Map::from_iter([("title".to_owned(), json!("Plover retro"))]),
            "test",
            "renamed",
            "2026-05-10T00:00:00Z",
        )
        .unwrap()
        .unwrap();
        let started = std::time::Instant::now();
        drop(trust);
        reconcile.join().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let report =
            solstone_core_indexer_store::todo::drain(root.path(), Default::default()).unwrap();
        assert_eq!(report.failed, 0);
        let check = solstone_core_indexer_store::scan::scan_journal(root.path(), false).unwrap();
        assert_eq!((check.indexed, check.removed), (0, 0));
        assert!(searchable(
            root.path(),
            "facets/work/activities/20260510.jsonl",
            "Plover retro"
        ));
    }

    #[test]
    fn a_saved_entity_detection_is_searchable_after_a_drain() {
        let root = tempfile::tempdir().unwrap();
        create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        save_detected_entity(
            root.path(),
            "work",
            "20260510",
            "Person",
            "Wren Calloway",
            "Joined the launch call.",
        )
        .unwrap();
        let report =
            solstone_core_indexer_store::todo::drain(root.path(), Default::default()).unwrap();
        assert_eq!(report.failed, 0);
        let check = solstone_core_indexer_store::scan::scan_journal(root.path(), false).unwrap();
        assert_eq!((check.indexed, check.removed), (0, 0));
        assert!(searchable(
            root.path(),
            "facets/work/entities/20260510.jsonl",
            "Wren Calloway"
        ));
    }
}
