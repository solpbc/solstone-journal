// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bring stored connection classifications up to date after facet names change.
//!
//! A classification row is written when its source is indexed, and a source
//! whose bytes never change is never indexed again. When a facet is merged
//! away or a historical name is recorded as retired, rows for material that
//! names it can carry an id that no longer exists, or no facet at all, and the
//! search prefilter then leaves that material out for the facet it now
//! belongs to. Reconciling re-runs classification for exactly the rows that
//! can be stale, whatever the history that made them so:
//!
//! - rows naming any facet id that is not a current live id;
//! - facet-owned or segment-assigned rows naming no facet;
//! - excluded or unclassified rows (no basis).
//!
//! A correct row classifies to itself and is left alone, so a run is
//! idempotent. Only classification rows are written.

use std::collections::BTreeSet;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use solstone_core_indexer::stream::extract_stream;

use crate::StoreError;
use crate::classification::{FacetDeclarationSet, classify_source};
use crate::db::{ChunkClassification, open_index, replace_chunk_classification};

const BATCH: usize = 500;
const MAX_RESTARTS: usize = 3;

/// What one reconcile run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub candidates: usize,
    pub changed: usize,
    pub restarts: usize,
    /// True when facet declarations kept changing underneath the run and it
    /// stopped before finishing; rerun it.
    pub incomplete: bool,
}

/// Reconcile stale classifications.
///
/// `snapshot` returns the declarations to classify against. The caller takes
/// the facet trust lock inside it, so a snapshot never sees a facet operation
/// half done, and holds its own single-flight lock around the whole call.
/// Before each batch commits, the declarations are read again; if they no
/// longer match the snapshot, the batch is discarded and the pass restarts
/// from the beginning with a fresh snapshot.
pub fn reconcile_stale_classifications(
    journal: &Path,
    snapshot: &mut dyn FnMut() -> Result<FacetDeclarationSet, StoreError>,
) -> Result<ReconcileReport, StoreError> {
    let mut conn = open_index(journal)?;
    let mut report = ReconcileReport::default();
    'pass: loop {
        let declarations = snapshot()?;
        let candidates = stale_candidates(&conn, &declarations)?;
        report.candidates = candidates.len();
        report.changed = 0;
        let paths = candidates.into_iter().collect::<Vec<_>>();
        for batch in paths.chunks(BATCH) {
            let mut updates = Vec::new();
            for path in batch {
                let stream = extract_stream(journal, path).stream;
                let classification =
                    classify_source(journal, path, stream.as_deref(), &declarations);
                if stored_classification(&conn, path)?.as_ref() != Some(&classification) {
                    updates.push(classification);
                }
            }
            if updates.is_empty() {
                continue;
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if !FacetDeclarationSet::from_journal(journal)?.same_as(&declarations) {
                drop(tx);
                if report.restarts == MAX_RESTARTS {
                    report.incomplete = true;
                    return Ok(report);
                }
                report.restarts += 1;
                continue 'pass;
            }
            for classification in &updates {
                replace_chunk_classification(&tx, classification)?;
            }
            tx.commit()?;
            report.changed += updates.len();
        }
        return Ok(report);
    }
}

fn stale_candidates(
    conn: &Connection,
    declarations: &FacetDeclarationSet,
) -> Result<BTreeSet<String>, StoreError> {
    let mut paths = BTreeSet::new();
    let mut dead = conn.prepare("SELECT path, facet_id FROM chunk_classification_facets")?;
    for row in dead.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (path, id) = row?;
        if !declarations.is_live_id(&id) {
            paths.insert(path);
        }
    }
    let mut empty = conn.prepare(
        "SELECT c.path FROM chunk_classification c \
         WHERE c.basis IN ('segment_assigned', 'facet_owned') \
         AND NOT EXISTS (SELECT 1 FROM chunk_classification_facets f WHERE f.path = c.path)",
    )?;
    for row in empty.query_map([], |row| row.get::<_, String>(0))? {
        paths.insert(row?);
    }
    let mut unbased = conn.prepare("SELECT path FROM chunk_classification WHERE basis IS NULL")?;
    for row in unbased.query_map([], |row| row.get::<_, String>(0))? {
        paths.insert(row?);
    }
    Ok(paths)
}

fn stored_classification(
    conn: &Connection,
    path: &str,
) -> Result<Option<ChunkClassification>, StoreError> {
    let Some((category, basis, eligible, unclassified)) = conn
        .query_row(
            "SELECT category, basis, eligible, unclassified FROM chunk_classification WHERE path=?",
            [path],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut ids = conn.prepare(
        "SELECT facet_id FROM chunk_classification_facets WHERE path=? ORDER BY facet_id",
    )?;
    let facet_ids = ids
        .query_map([path], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(ChunkClassification {
        path: path.to_owned(),
        category: category.as_deref().and_then(static_category),
        basis: basis.as_deref().and_then(static_basis),
        eligible: eligible != 0,
        unclassified: unclassified != 0,
        facet_ids,
    }))
}

fn static_category(value: &str) -> Option<&'static str> {
    ["transcripts", "entities", "facets"]
        .into_iter()
        .find(|name| *name == value)
}

fn static_basis(value: &str) -> Option<&'static str> {
    ["facet_owned", "segment_assigned", "journal_wide"]
        .into_iter()
        .find(|name| *name == value)
}

#[cfg(test)]
mod tests {
    use super::reconcile_stale_classifications;
    use crate::classification::FacetDeclarationSet;
    use crate::db::{ChunkClassification, open_index, replace_chunk_classification};
    use crate::test_support::reserve_temp_path;

    const S: &str = "22222222-2222-4222-8222-222222222222";
    const DEAD: &str = "11111111-1111-4111-8111-111111111111";
    const SEGMENT: &str = "20260107/default/123456_300/talents/brief.md";
    const AGENT: &str = "20260107/mcp.agent/123456_300/talents/brief.md";

    fn stored(root: &std::path::Path, path: &str) -> (Option<String>, Vec<String>) {
        let conn = open_index(root).expect("index");
        let basis = conn
            .query_row(
                "SELECT basis FROM chunk_classification WHERE path=?",
                [path],
                |row| row.get::<_, Option<String>>(0),
            )
            .expect("row");
        let mut statement = conn
            .prepare("SELECT facet_id FROM chunk_classification_facets WHERE path=?")
            .expect("prepare");
        let ids = statement
            .query_map([path], |row| row.get::<_, String>(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("ids");
        (basis, ids)
    }

    #[test]
    fn stale_rows_for_a_merged_name_move_to_the_survivor_and_correct_rows_stay() {
        let root = reserve_temp_path("reconcile-merged");
        std::fs::create_dir_all(root.join("facets/solstone")).expect("facet");
        std::fs::write(
            root.join("facets/solstone/facet.json"),
            format!(r#"{{"id":"{S}"}}"#),
        )
        .expect("declaration");
        std::fs::write(
            root.join("facets/retired.json"),
            format!(
                r#"{{"names":{{"sunstone":{{"state":"merged","id":"{DEAD}","successor":"{S}"}}}}}}"#
            ),
        )
        .expect("retired");
        let talents = root.join("chronicle/20260107/default/123456_300/talents");
        std::fs::create_dir_all(&talents).expect("segment");
        std::fs::write(talents.join("facets.json"), r#"[{"facet":"sunstone"}]"#).expect("assign");
        {
            // What an index built before the merge holds: the merged-away id,
            // and an agent-written row that must stay excluded.
            let mut conn = open_index(&root).expect("index");
            let tx = conn.transaction().expect("tx");
            replace_chunk_classification(
                &tx,
                &ChunkClassification {
                    path: SEGMENT.to_owned(),
                    category: Some("transcripts"),
                    basis: Some("segment_assigned"),
                    eligible: true,
                    unclassified: false,
                    facet_ids: vec![DEAD.to_owned()],
                },
            )
            .expect("stale row");
            replace_chunk_classification(
                &tx,
                &ChunkClassification {
                    path: AGENT.to_owned(),
                    category: None,
                    basis: None,
                    eligible: false,
                    unclassified: false,
                    facet_ids: Vec::new(),
                },
            )
            .expect("agent row");
            tx.commit().expect("commit");
        }
        let mut snapshot = || FacetDeclarationSet::from_journal(&root);
        let report = reconcile_stale_classifications(&root, &mut snapshot).expect("reconcile");
        assert_eq!(report.changed, 1);
        assert!(!report.incomplete);
        assert_eq!(
            stored(&root, SEGMENT),
            (Some("segment_assigned".to_owned()), vec![S.to_owned()])
        );
        assert_eq!(stored(&root, AGENT), (None, Vec::new()));
        // A second run finds nothing to change.
        let again = reconcile_stale_classifications(&root, &mut snapshot).expect("again");
        assert_eq!(again.changed, 0);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_change_to_facets_during_a_run_restarts_it_from_a_fresh_snapshot() {
        let root = reserve_temp_path("reconcile-restart");
        std::fs::create_dir_all(root.join("facets/solstone")).expect("facet");
        std::fs::write(
            root.join("facets/solstone/facet.json"),
            format!(r#"{{"id":"{S}"}}"#),
        )
        .expect("declaration");
        let talents = root.join("chronicle/20260107/default/123456_300/talents");
        std::fs::create_dir_all(&talents).expect("segment");
        std::fs::write(talents.join("facets.json"), r#"[{"facet":"sunstone"}]"#).expect("assign");
        {
            let mut conn = open_index(&root).expect("index");
            let tx = conn.transaction().expect("tx");
            replace_chunk_classification(
                &tx,
                &ChunkClassification {
                    path: SEGMENT.to_owned(),
                    category: Some("transcripts"),
                    basis: Some("segment_assigned"),
                    eligible: true,
                    unclassified: false,
                    facet_ids: vec![DEAD.to_owned()],
                },
            )
            .expect("stale row");
            tx.commit().expect("commit");
        }
        // The first snapshot predates the merge record; the record lands
        // before the batch commits, so the run must restart and use it.
        let mut calls = 0;
        let retired = root.join("facets/retired.json");
        let mut snapshot = || {
            calls += 1;
            let set = FacetDeclarationSet::from_journal(&root);
            if calls == 1 {
                std::fs::write(
                    &retired,
                    format!(r#"{{"names":{{"sunstone":{{"state":"merged","id":"{DEAD}","successor":"{S}"}}}}}}"#),
                )
                .expect("retired");
            }
            set
        };
        let report = reconcile_stale_classifications(&root, &mut snapshot).expect("reconcile");
        assert_eq!(report.restarts, 1);
        assert_eq!(stored(&root, SEGMENT).1, vec![S.to_owned()]);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}
