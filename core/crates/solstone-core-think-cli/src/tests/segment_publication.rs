// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use solstone_core_cortex_client::{CortexRequest, WaitForUsesReport};
use solstone_core_format::content::{
    AdmittedCategory, Family, IndexDisposition, ScopeBasis, resolve_spec,
};
use solstone_core_indexer_store::db::open_index;

use super::*;
use crate::context::{CortexBoundary, IndexBoundary, ThinkContext};
use crate::run_log::RunLogWriter;
use crate::segment::write_sense_and_change;

fn drain_and_stored_chunk_facet_ids(
    journal: &Path,
    path: &str,
) -> Result<Option<Vec<String>>, solstone_core_indexer_store::StoreError> {
    solstone_core_indexer_store::todo::drain(journal, Default::default()).unwrap();
    let conn = open_index(journal)?;
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chunk_classification WHERE path=?)",
        [path],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let mut stmt = conn.prepare(
        "SELECT facet_id FROM chunk_classification_facets WHERE path=? ORDER BY facet_id",
    )?;
    let ids = stmt
        .query_map([path], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(ids))
}

const DAY: &str = "20260813";
const NOW: i64 = 1_786_615_200_000;

#[test]
fn segment_publication_replaces_whatever_its_output_files_already_hold() {
    let (_journal, context) = fixture_journal();
    let segment = context.day_dir.join("default/120000_60");
    let talents = segment.join("talents");
    fs::create_dir_all(&talents).unwrap();
    // A digest file an earlier version left beside its outputs is not consulted.
    fs::write(
        talents.join(".generated-outputs.json"),
        r#"{"version":1,"files":{"sense.json":"0000000000000000000000000000000000000000000000000000000000000000"}}"#,
    )
    .unwrap();
    for filename in ["sense.json", "activity.md", "topics.md"] {
        fs::write(talents.join(filename), "written by an earlier version").unwrap();
    }
    let sense = serde_json::json!({"density":"idle","content_type":"idle","activity_summary":"generated summary","facets":[]});
    let mut log = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log,
        "120000_60",
        Some("default"),
        &segment,
        sense.as_object().unwrap(),
    )
    .unwrap();
    for filename in ["sense.json", "activity.md", "topics.md"] {
        assert_ne!(
            fs::read_to_string(talents.join(filename)).unwrap(),
            "written by an earlier version"
        );
    }
    assert_eq!(
        fs::read_to_string(talents.join("activity.md")).unwrap(),
        "generated summary"
    );
}

#[test]
fn sense_partial_projections_replace_their_notice_with_the_result() {
    let (_journal, context) = fixture_journal();
    let segment = context.day_dir.join("default/120000_60");
    let mut log = RunLogWriter::open(&context.journal, DAY, "segment");
    for partial in [true, false, true] {
        let mut sense = serde_json::json!({"density":"idle","content_type":"idle","activity_summary":"kept summary","entities":[{"type":"Tool","name":"Editor"}],"facets":[]});
        if partial {
            sense["partial_input"] = serde_json::json!({"dropped_entries":2});
        }
        write_sense_and_change(
            &context,
            &mut log,
            "120000_60",
            Some("default"),
            &segment,
            sense.as_object().unwrap(),
        )
        .unwrap();
        for file in ["activity.md", "sense.md", "topics.md"] {
            let text = fs::read_to_string(segment.join("talents").join(file)).unwrap();
            assert_eq!(text.contains("Left out:"), partial, "{file}: {text}");
        }
        for file in ["sense.json", "density.json"] {
            let saved: Value =
                serde_json::from_slice(&fs::read(segment.join("talents").join(file)).unwrap())
                    .unwrap();
            assert_eq!(saved.get("partial_input").is_some(), partial, "{file}");
        }
    }
}

fn fixture_journal() -> (tempfile::TempDir, ThinkContext) {
    let journal = tempfile::tempdir().unwrap();
    let config_path = journal.path().join("config/journal.json");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        &config_path,
        r#"{"providers":{"active":{"provider":"openai","model":"test-model"}}}"#,
    )
    .unwrap();
    let day_dir = crate::day::create_day(journal.path(), DAY).unwrap();
    let context =
        ThinkContext::new(journal.path(), DAY.to_owned(), day_dir, NOW).expect("think context");
    (journal, context)
}

fn drain_and_chunk_text_contains(journal: &Path, rel_path: &str, substring: &str) -> bool {
    solstone_core_indexer_store::todo::drain(journal, Default::default()).unwrap();
    let conn = open_index(journal).expect("open index db");
    let text: Option<String> = conn
        .query_row(
            "SELECT content FROM chunks WHERE path=?",
            [rel_path],
            |row| row.get(0),
        )
        .ok();
    text.is_some_and(|t| t.contains(substring))
}

#[test]
fn actual_results_with_entities_indexes_activity_sense_md_and_sense_json() {
    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(context.journal.as_path(), "work", "Work", "", "", "", None)
        .unwrap();

    let segment_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    let lease = solstone_core_journal_io::hold_lock(
        solstone_core_indexer_store::db::db_path(&context.journal),
        Default::default(),
    )
    .unwrap();

    let sense_val = serde_json::json!({
        "density": "active",
        "content_type": "work",
        "activity_summary": "Sprint planning discussion for the new release",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": [{
            "type": "person",
            "name": "Alice Developer",
            "role": "lead",
            "source": "transcript",
            "context": "project planning"
        }]
    });

    let mut log = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log,
        "120000_60",
        Some("default"),
        &segment_dir,
        sense_val.as_object().unwrap(),
    )
    .unwrap();
    log.finish().unwrap();

    let keys = solstone_core_indexer_store::todo::inspect(&context.journal)
        .unwrap()
        .keys;
    for name in ["activity.md", "sense.md", "sense.json"] {
        assert!(
            keys.contains(&format!("{DAY}/default/120000_60/talents/{name}")),
            "{keys:?}"
        );
    }
    assert!(!solstone_core_indexer_store::db::db_path(&context.journal).exists());
    drop(lease);

    // 1. activity.md is indexed and searchable in SQLite chunks
    let rel_activity = "20260813/default/120000_60/talents/activity.md";
    assert!(drain_and_chunk_text_contains(
        &context.journal,
        rel_activity,
        "Sprint planning"
    ));

    // 2. sense.md is indexed and searchable in SQLite chunks
    let rel_sense_md = "20260813/default/120000_60/talents/sense.md";
    assert!(drain_and_chunk_text_contains(
        &context.journal,
        rel_sense_md,
        "Alice Developer"
    ));

    // 3. sense.json is indexed content (agents never read it), so its
    // attempt indexes it at publication rather than waiting for the next scan
    let records = oplog_records(&context.journal, DAY, "segment");
    let sense_json_attempts: Vec<_> = records
        .iter()
        .filter(|r| {
            r["event"] == "index.enqueue"
                && r["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("sense.json"))
        })
        .collect();
    assert!(!sense_json_attempts.is_empty());
    for attempt in sense_json_attempts {
        assert_eq!(attempt["outcome"], "queued");
    }
    let rel_sense_json = "20260813/default/120000_60/talents/sense.json";
    assert!(drain_and_chunk_text_contains(
        &context.journal,
        rel_sense_json,
        "Sprint planning"
    ));

    let sense_md_path = segment_dir.join("talents/sense.md");
    let original_sense_md = fs::read_to_string(&sense_md_path).unwrap();

    // 4. Empty-entity second call leaves first sense.md bytes in place
    let empty_entity_sense = serde_json::json!({
        "density": "active",
        "content_type": "work",
        "activity_summary": "Sprint planning discussion for the new release",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": []
    });
    let mut log2 = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log2,
        "120000_60",
        Some("default"),
        &segment_dir,
        empty_entity_sense.as_object().unwrap(),
    )
    .unwrap();
    log2.finish().unwrap();

    let post_sense_md = fs::read_to_string(&sense_md_path).unwrap();
    assert_eq!(post_sense_md, original_sense_md);
}

#[test]
fn no_input_and_retained_retry_paths_reach_write_sense_and_change() {
    // Confirmed call sites reaching write_sense_and_change: segment.rs:225 (retained routing retry) and segment.rs:240 (no-input).
    let (_journal, context) = fixture_journal();
    let segment_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("120000_60");
    fs::create_dir_all(&segment_dir).unwrap();

    let idle_sense = serde_json::json!({
        "density": "idle",
        "content_type": "idle",
        "activity_summary": "",
        "facets": []
    });

    let mut log = RunLogWriter::open(&context.journal, DAY, "segment");
    let change = write_sense_and_change(
        &context,
        &mut log,
        "120000_60",
        Some("default"),
        &segment_dir,
        idle_sense.as_object().unwrap(),
    )
    .unwrap();
    log.finish().unwrap();

    assert_eq!(change["change_class"], "idle");
    assert!(segment_dir.join("talents/activity.md").exists());
    assert!(segment_dir.join("talents/sense.json").exists());
    assert!(segment_dir.join("talents/facets.json").exists());
    assert!(segment_dir.join("talents/density.json").exists());
}

#[test]
fn later_error_on_routing_rejected_stops_before_facets_json() {
    struct FailingSink {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl std::io::Write for FailingSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if let Ok(text) = std::str::from_utf8(buf)
                && text.contains("facet.routing_rejected")
            {
                return Err(std::io::Error::other("routing rejected sink write error"));
            }
            self.buffer.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let (_journal, context) = fixture_journal();
    let segment_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("120000_60");
    fs::create_dir_all(&segment_dir).unwrap();

    let buffer = Arc::new(Mutex::new(Vec::new()));
    let sink = FailingSink {
        buffer: buffer.clone(),
    };
    let mut log = RunLogWriter::with_sink(context.journal.join("chronicle/20260813/health"), sink);

    let sense_val = serde_json::json!({
        "density": "active",
        "content_type": "work",
        "activity_summary": "Summary before error",
        "facets": [{"facet": "unknown_facet_slug", "level": "high"}]
    });

    let result = write_sense_and_change(
        &context,
        &mut log,
        "120000_60",
        Some("default"),
        &segment_dir,
        sense_val.as_object().unwrap(),
    );
    assert!(
        result.is_err(),
        "expected run-log finish error from log.finish()?"
    );

    let activity_path = segment_dir.join("talents/activity.md");
    assert!(activity_path.exists());
    assert_eq!(
        fs::read_to_string(&activity_path).unwrap(),
        "Summary before error"
    );

    let written = buffer.lock().unwrap().clone();
    let written_str = String::from_utf8_lossy(&written);
    assert!(written_str.contains("index.enqueue"));

    let facets_path = segment_dir.join("talents/facets.json");
    assert!(
        !facets_path.exists(),
        "facets.json must be absent when error occurs before write"
    );
}

#[test]
fn segment_reassignment_refreshes_sibling_text_and_classification() {
    let (_journal, context) = fixture_journal();

    solstone_core_facets::create_facet(
        context.journal.as_path(),
        "facet-a",
        "Facet A",
        "",
        "",
        "",
        None,
    )
    .unwrap();
    let id_a =
        solstone_core_facets::observe_facet_write_identity(context.journal.as_path(), "facet-a")
            .unwrap();

    solstone_core_facets::create_facet(
        context.journal.as_path(),
        "facet-b",
        "Facet B",
        "",
        "",
        "",
        None,
    )
    .unwrap();
    let id_b =
        solstone_core_facets::observe_facet_write_identity(context.journal.as_path(), "facet-b")
            .unwrap();

    let seg1_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("import.document")
        .join("120000_60");
    let seg1_talents = seg1_dir.join("talents");
    fs::create_dir_all(&seg1_talents).unwrap();

    let notes_path = seg1_talents.join("notes.md");
    fs::write(&notes_path, "Untouched sibling notes text").unwrap();
    let imported_path = seg1_dir.join("imported.md");
    let transcript_path = seg1_dir.join("record_transcript.md");
    fs::write(&imported_path, "Untouched imported document").unwrap();
    fs::write(&transcript_path, "Untouched imported transcript").unwrap();
    fs::write(seg1_dir.join("audio.jsonl"), "raw transcript input").unwrap();

    let nested_dir = seg1_talents.join("work");
    fs::create_dir_all(&nested_dir).unwrap();
    let nested_path = nested_dir.join("nested.md");
    fs::write(&nested_path, "Nested directory text").unwrap();

    let seg2_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("import.document")
        .join("130000_60");
    let seg2_talents = seg2_dir.join("talents");
    fs::create_dir_all(&seg2_talents).unwrap();
    let seg2_other_path = seg2_talents.join("other.md");
    fs::write(&seg2_other_path, "Other segment markdown").unwrap();

    // 1. First call assigns Facet A
    let sense_a = serde_json::json!({
        "density": "active",
        "content_type": "work",
        "activity_summary": "First assignment to Facet A",
        "facets": [{"facet": "facet-a", "level": "high"}],
        "entities": []
    });
    let mut log1 = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log1,
        "120000_60",
        Some("import.document"),
        &seg1_dir,
        sense_a.as_object().unwrap(),
    )
    .unwrap();
    log1.finish().unwrap();

    // Also assign seg2 to Facet A
    let mut log_seg2 = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log_seg2,
        "130000_60",
        Some("import.document"),
        &seg2_dir,
        sense_a.as_object().unwrap(),
    )
    .unwrap();
    log_seg2.finish().unwrap();

    // Verify notes.md has Facet A
    let rel_notes = "20260813/import.document/120000_60/talents/notes.md";
    let ids_after_a = drain_and_stored_chunk_facet_ids(&context.journal, rel_notes)
        .unwrap()
        .expect("notes indexed");
    assert_eq!(ids_after_a, vec![id_a.clone()]);

    // 2. Second call assigns Facet B with entity (produces sense.md)
    let sense_b = serde_json::json!({
        "density": "active",
        "content_type": "work",
        "activity_summary": "Second assignment to Facet B",
        "facets": [{"facet": "facet-b", "level": "high"}],
        "entities": [{
            "type": "tool",
            "name": "RustCompiler",
            "role": "tool",
            "source": "transcript",
            "context": "build process"
        }]
    });
    let mut log2 = RunLogWriter::open(&context.journal, DAY, "reassign");
    write_sense_and_change(
        &context,
        &mut log2,
        "120000_60",
        Some("import.document"),
        &seg1_dir,
        sense_b.as_object().unwrap(),
    )
    .unwrap();
    log2.finish().unwrap();

    // Verify notes.md was refreshed to Facet B
    let ids_after_b = drain_and_stored_chunk_facet_ids(&context.journal, rel_notes)
        .unwrap()
        .expect("notes reindexed");
    assert_eq!(ids_after_b, vec![id_b.clone()]);

    // Verify activity.md has Facet B
    let rel_activity = "20260813/import.document/120000_60/talents/activity.md";
    let ids_activity = drain_and_stored_chunk_facet_ids(&context.journal, rel_activity)
        .unwrap()
        .expect("activity indexed");
    assert_eq!(ids_activity, vec![id_b.clone()]);

    // Verify seg2 other.md remains on Facet A
    let rel_seg2_other = "20260813/import.document/130000_60/talents/other.md";
    let ids_seg2 = drain_and_stored_chunk_facet_ids(&context.journal, rel_seg2_other)
        .unwrap()
        .expect("other indexed");
    assert_eq!(ids_seg2, vec![id_a.clone()]);

    // Verify oplog index attempts for reassign run:
    let reassign_records = oplog_records(&context.journal, DAY, "reassign");
    let attempted_paths = reassign_records
        .iter()
        .filter(|r| r["event"] == "index.enqueue")
        .map(|r| r["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    let unique_paths = attempted_paths
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        attempted_paths.len(),
        unique_paths.len(),
        "each path is attempted once"
    );
    assert_eq!(
        attempted_paths
            .iter()
            .filter(|p| p.ends_with("activity.md"))
            .count(),
        1
    );
    assert!(!attempted_paths.iter().any(|p| p.ends_with("audio.jsonl")));
    assert!(drain_and_chunk_text_contains(
        &context.journal,
        rel_activity,
        "Second assignment"
    ));
    for path in ["imported.md", "record_transcript.md"] {
        let rel = format!("{DAY}/import.document/120000_60/{path}");
        assert_eq!(
            drain_and_stored_chunk_facet_ids(&context.journal, &rel)
                .unwrap()
                .unwrap(),
            vec![id_b.clone()]
        );
        assert_eq!(attempted_paths.iter().filter(|p| **p == rel).count(), 1);
    }

    let sense_md_attempts: Vec<_> = reassign_records
        .iter()
        .filter(|r| {
            r["event"] == "index.enqueue"
                && r["path"].as_str().is_some_and(|p| p.ends_with("sense.md"))
        })
        .collect();
    assert_eq!(
        sense_md_attempts.len(),
        1,
        "sense.md must have exactly 1 index attempt"
    );

    let nested_attempts: Vec<_> = reassign_records
        .iter()
        .filter(|r| {
            r["event"] == "index.enqueue"
                && r["path"].as_str().is_some_and(|p| p.contains("nested.md"))
        })
        .collect();
    assert_eq!(
        nested_attempts.len(),
        0,
        "nested file must not be in attempts"
    );
    let mut unchanged_log = RunLogWriter::open(&context.journal, DAY, "unchanged");
    write_sense_and_change(
        &context,
        &mut unchanged_log,
        "120000_60",
        Some("import.document"),
        &seg1_dir,
        sense_b.as_object().unwrap(),
    )
    .unwrap();
    unchanged_log.finish().unwrap();
    let unchanged_records = oplog_records(&context.journal, DAY, "unchanged");
    assert!(
        !unchanged_records
            .iter()
            .any(|r| r["event"] == "index.enqueue"
                && r["path"].as_str().is_some_and(|p| p.ends_with("notes.md")
                    || p.ends_with("imported.md")
                    || p.ends_with("record_transcript.md")))
    );
}

#[test]
fn a_later_projection_write_failure_still_indexes_earlier_saved_activity() {
    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(&context.journal, "work", "Work", "", "", "", None).unwrap();
    let segment = context
        .journal
        .join(format!("chronicle/{DAY}/default/120000_60"));
    fs::create_dir_all(segment.join("talents/density.json")).unwrap();
    let sense = serde_json::json!({
        "density": "active",
        "activity_summary": "Activity saved before the density write fails",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": []
    });
    let mut log = RunLogWriter::open(&context.journal, DAY, "partial-projection");
    let result = write_sense_and_change(
        &context,
        &mut log,
        "120000_60",
        Some("default"),
        &segment,
        sense.as_object().unwrap(),
    );
    assert!(
        result.is_err(),
        "the original source-publication error must remain an error"
    );
    log.finish().unwrap();
    let path = "20260813/default/120000_60/talents/activity.md";
    assert!(drain_and_chunk_text_contains(
        &context.journal,
        path,
        "before the density write fails"
    ));
    let records = oplog_records(&context.journal, DAY, "partial-projection");
    let attempts = records
        .iter()
        .filter(|r| r["event"] == "index.enqueue" && r["path"] == path)
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["outcome"], "queued");
}

#[test]
fn multi_path_index_failure_records_oplog_and_preserves_successful_indexes() {
    struct PartialFailingIndexBoundary {
        failing_suffix: &'static str,
    }

    impl IndexBoundary for PartialFailingIndexBoundary {
        fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<(), String> {
            if path.to_string_lossy().ends_with(self.failing_suffix) {
                Err("index failed notes".to_owned())
            } else {
                solstone_core_indexer_store::todo::enqueue_path(journal, path)
                    .map_err(|e| e.to_string())
            }
        }
    }

    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(context.journal.as_path(), "work", "Work", "", "", "", None)
        .unwrap();

    let segment_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("120000_60");
    let talents = segment_dir.join("talents");
    fs::create_dir_all(&talents).unwrap();
    fs::write(talents.join("notes.md"), "Notes file content").unwrap();

    let boundary = Arc::new(PartialFailingIndexBoundary {
        failing_suffix: "notes.md",
    });
    let context = context.with_index_boundary(boundary);

    let sense_val = serde_json::json!({
        "density": "active",
        "content_type": "work",
        "activity_summary": "Summary of active work",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": []
    });

    let mut log = RunLogWriter::open(&context.journal, DAY, "multipath");
    let res = write_sense_and_change(
        &context,
        &mut log,
        "120000_60",
        Some("default"),
        &segment_dir,
        sense_val.as_object().unwrap(),
    );
    assert!(
        res.is_ok(),
        "write_sense_and_change must return Ok despite individual path index failures"
    );
    log.finish().unwrap();

    let records = oplog_records(&context.journal, DAY, "multipath");
    let notes_attempt = records
        .iter()
        .find(|r| {
            r["event"] == "index.enqueue"
                && r["path"].as_str().is_some_and(|p| p.ends_with("notes.md"))
        })
        .expect("notes.md attempt in oplog");
    assert_eq!(notes_attempt["outcome"], "failed");
    assert_eq!(notes_attempt["cause"], "index failed notes");

    let activity_attempt = records
        .iter()
        .find(|r| {
            r["event"] == "index.enqueue"
                && r["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("activity.md"))
        })
        .expect("activity.md attempt in oplog");
    assert_eq!(activity_attempt["outcome"], "queued");

    // Successes are in index chunks
    let rel_activity = "20260813/default/120000_60/talents/activity.md";
    assert!(drain_and_chunk_text_contains(
        &context.journal,
        rel_activity,
        "Summary of active work"
    ));
}

#[test]
fn topics_happy_path_indexes_topics_md_with_admitted_transcripts_classification() {
    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(context.journal.as_path(), "work", "Work", "", "", "", None)
        .unwrap();

    let segment_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("120000_60");
    fs::create_dir_all(&segment_dir).unwrap();

    fs::write(
        segment_dir.join("audio.jsonl"),
        r#"{"start": 1.0, "speaker": "Alice", "text": "We decided on the unique_arch_plan_alpha roadmap."}"#,
    )
    .unwrap();

    let sense_val = serde_json::json!({
        "density": "active",
        "content_type": "meeting",
        "activity_summary": "Architecture roadmap review",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": [],
        "topics": ["unique_arch_plan_alpha roadmap"],
        "quotes": [{
            "speaker": "wrong_speaker",
            "text": "“We decided on the unique_arch_plan_alpha roadmap.”"
        }]
    });

    let mut log = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log,
        "120000_60",
        Some("default"),
        &segment_dir,
        sense_val.as_object().unwrap(),
    )
    .unwrap();
    log.finish().unwrap();

    let rel_topics = "20260813/default/120000_60/talents/topics.md";
    assert!(drain_and_chunk_text_contains(
        &context.journal,
        rel_topics,
        "unique_arch_plan_alpha"
    ));

    let spec = resolve_spec(rel_topics).expect("topics.md resolves to spec");
    assert_eq!(spec.family, Family::Markdown);
    assert_eq!(
        spec.disposition,
        IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        }
    );

    let topics_content = fs::read_to_string(segment_dir.join("talents/topics.md")).unwrap();
    assert!(
        topics_content.contains("- Alice: We decided on the unique_arch_plan_alpha roadmap.\n")
    );
}

#[test]
fn topics_emptied_second_call_clears_file_and_removes_indexed_chunks() {
    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(context.journal.as_path(), "work", "Work", "", "", "", None)
        .unwrap();

    // Segment 1: cleared by empty topics and quotes arrays
    let seg1_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("120000_60");
    fs::create_dir_all(&seg1_dir).unwrap();
    fs::write(
        seg1_dir.join("audio.jsonl"),
        r#"{"start": 1.0, "speaker": "Alice", "text": "Distinctive alpha keyword here."}"#,
    )
    .unwrap();

    let sense_active1 = serde_json::json!({
        "density": "active",
        "content_type": "meeting",
        "activity_summary": "Active session one",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": [],
        "topics": ["distinctive_alpha_topic"],
        "quotes": [{"speaker": "Alice", "text": "Distinctive alpha keyword here."}]
    });

    let mut log1 = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log1,
        "120000_60",
        Some("default"),
        &seg1_dir,
        sense_active1.as_object().unwrap(),
    )
    .unwrap();
    log1.finish().unwrap();

    let rel_topics1 = "20260813/default/120000_60/talents/topics.md";
    assert!(drain_and_chunk_text_contains(
        &context.journal,
        rel_topics1,
        "distinctive_alpha_topic"
    ));

    // Second call: empty topics and quotes
    let sense_empty1 = serde_json::json!({
        "density": "active",
        "content_type": "meeting",
        "activity_summary": "Now empty session",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": [],
        "topics": [],
        "quotes": []
    });

    let mut log1_empty = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log1_empty,
        "120000_60",
        Some("default"),
        &seg1_dir,
        sense_empty1.as_object().unwrap(),
    )
    .unwrap();
    log1_empty.finish().unwrap();

    let seg1_topics_text = fs::read_to_string(seg1_dir.join("talents/topics.md")).unwrap();
    assert_eq!(seg1_topics_text, "");
    assert!(!drain_and_chunk_text_contains(
        &context.journal,
        rel_topics1,
        "distinctive_alpha_topic"
    ));

    // Segment 2: cleared by empty_input_sense_output()
    let seg2_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("121000_60");
    fs::create_dir_all(&seg2_dir).unwrap();
    fs::write(
        seg2_dir.join("audio.jsonl"),
        r#"{"start": 1.0, "speaker": "Bob", "text": "Distinctive beta keyword here."}"#,
    )
    .unwrap();

    let sense_active2 = serde_json::json!({
        "density": "active",
        "content_type": "meeting",
        "activity_summary": "Active session two",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": [],
        "topics": ["distinctive_beta_topic"],
        "quotes": [{"speaker": "Bob", "text": "Distinctive beta keyword here."}]
    });

    let mut log2 = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log2,
        "121000_60",
        Some("default"),
        &seg2_dir,
        sense_active2.as_object().unwrap(),
    )
    .unwrap();
    log2.finish().unwrap();

    let rel_topics2 = "20260813/default/121000_60/talents/topics.md";
    assert!(drain_and_chunk_text_contains(
        &context.journal,
        rel_topics2,
        "distinctive_beta_topic"
    ));

    // Second call: empty_input_sense_output()
    let idle_sense = crate::segment::empty_input_sense_output();
    let mut log2_idle = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log2_idle,
        "121000_60",
        Some("default"),
        &seg2_dir,
        &idle_sense,
    )
    .unwrap();
    log2_idle.finish().unwrap();

    let seg2_topics_text = fs::read_to_string(seg2_dir.join("talents/topics.md")).unwrap();
    assert_eq!(seg2_topics_text, "");
    assert!(!drain_and_chunk_text_contains(
        &context.journal,
        rel_topics2,
        "distinctive_beta_topic"
    ));
}

#[test]
fn topics_no_prior_file_keyless_and_empty_does_not_create_file() {
    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(context.journal.as_path(), "work", "Work", "", "", "", None)
        .unwrap();

    // Case 1: Keyless sense object (no topics or quotes keys)
    let seg1_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("122000_60");
    fs::create_dir_all(&seg1_dir).unwrap();

    let keyless_sense = serde_json::json!({
        "density": "idle",
        "content_type": "idle",
        "activity_summary": "Idle interval",
        "facets": [{"facet": "work", "level": "low"}],
        "entities": []
    });

    let mut log1 = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log1,
        "122000_60",
        Some("default"),
        &seg1_dir,
        keyless_sense.as_object().unwrap(),
    )
    .unwrap();
    log1.finish().unwrap();

    assert!(!seg1_dir.join("talents/topics.md").exists());
    let records1 = oplog_records(&context.journal, DAY, "segment");
    assert!(!records1.iter().any(|r| {
        r["event"] == "index.enqueue"
            && r["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("talents/topics.md"))
    }));
    assert!(
        records1
            .iter()
            .any(|r| { r["kept"] == 0 && r["dropped"] == 0 })
    );

    // Case 2: Sense object with empty topics and quotes
    let seg2_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("123000_60");
    fs::create_dir_all(&seg2_dir).unwrap();

    let empty_sense = serde_json::json!({
        "density": "idle",
        "content_type": "idle",
        "activity_summary": "Idle interval two",
        "facets": [{"facet": "work", "level": "low"}],
        "entities": [],
        "topics": [],
        "quotes": []
    });

    let mut log2 = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log2,
        "123000_60",
        Some("default"),
        &seg2_dir,
        empty_sense.as_object().unwrap(),
    )
    .unwrap();
    log2.finish().unwrap();

    assert!(!seg2_dir.join("talents/topics.md").exists());
    let records2 = oplog_records(&context.journal, DAY, "segment");
    assert!(!records2.iter().any(|r| {
        r["event"] == "index.enqueue"
            && r["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("talents/topics.md"))
    }));
    assert!(
        records2
            .iter()
            .any(|r| { r["kept"] == 0 && r["dropped"] == 0 })
    );
}

#[test]
fn topics_directory_collision_reports_unfinished_publication_and_logs_cause() {
    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(context.journal.as_path(), "work", "Work", "", "", "", None)
        .unwrap();

    let segment_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("124000_60");
    fs::create_dir_all(segment_dir.join("talents/topics.md")).unwrap();

    let sense_val = serde_json::json!({
        "density": "active",
        "content_type": "coding",
        "activity_summary": "Coding session with directory collision",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": [{
            "type": "Person",
            "name": "Alice Lead",
            "role": "lead",
            "source": "transcript",
            "context": "debugging"
        }],
        "topics": ["Compiler architecture"],
        "quotes": []
    });

    let mut log = RunLogWriter::open(&context.journal, DAY, "segment");
    let res = write_sense_and_change(
        &context,
        &mut log,
        "124000_60",
        Some("default"),
        &segment_dir,
        sense_val.as_object().unwrap(),
    );
    assert!(res.is_err());
    log.finish().unwrap();

    assert!(segment_dir.join("talents/sense.md").is_file());
    assert!(segment_dir.join("talents/change.json").is_file());

    let records = oplog_records(&context.journal, DAY, "segment");
    let failure_record = records.iter().find(|r| {
        r["path"].as_str().is_some_and(|p| p.ends_with("topics.md"))
            && r["cause"].as_str().is_some_and(|c| !c.is_empty())
    });
    assert!(
        failure_record.is_some(),
        "expected failure record with non-empty cause"
    );

    assert!(!records.iter().any(|r| {
        r["event"] == "index.enqueue"
            && r["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("talents/topics.md"))
    }));
}

#[test]
fn topics_malformed_transcript_line_preserves_valid_transcript_quotes() {
    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(context.journal.as_path(), "work", "Work", "", "", "", None)
        .unwrap();

    let segment_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("125000_60");
    fs::create_dir_all(&segment_dir).unwrap();

    fs::write(
        segment_dir.join("audio.jsonl"),
        [
            r#"{"start": 1.0, "speaker": "Dev", "text": "First valid speech line."}"#,
            "corrupt json { not valid",
            r#"{"start": 2.0, "speaker": "Dev", "text": "Second valid speech line."}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let sense_val = serde_json::json!({
        "density": "active",
        "content_type": "coding",
        "activity_summary": "Active work",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": [],
        "topics": ["Testing parser"],
        "quotes": [
            {"speaker": "Dev", "text": "First valid speech line."},
            {"speaker": "Dev", "text": "Second valid speech line."}
        ]
    });

    let mut log = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log,
        "125000_60",
        Some("default"),
        &segment_dir,
        sense_val.as_object().unwrap(),
    )
    .unwrap();
    log.finish().unwrap();

    let topics_content = fs::read_to_string(segment_dir.join("talents/topics.md")).unwrap();
    assert!(topics_content.contains("- Dev: First valid speech line.\n"));
    assert!(topics_content.contains("- Dev: Second valid speech line.\n"));
}

#[test]
fn topics_imported_audio_and_chat_conversations_produce_labeled_quotes() {
    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(context.journal.as_path(), "work", "Work", "", "", "", None)
        .unwrap();

    let segment_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("126000_60");
    fs::create_dir_all(&segment_dir).unwrap();

    fs::write(
        segment_dir.join("imported_audio.jsonl"),
        r#"{"start": 1.0, "speaker": "Host", "text": "Welcome to our recorded session."}"#,
    )
    .unwrap();
    fs::write(
        segment_dir.join("conversation_transcript.jsonl"),
        [
            r#"{"start": 2.0, "speaker": "Human", "text": "Can you inspect this log?"}"#,
            r#"{"start": 3.0, "speaker": "Assistant", "text": "The log is clean and valid."}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let sense_val = serde_json::json!({
        "density": "active",
        "content_type": "meeting",
        "activity_summary": "Mixed conversation",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": [],
        "topics": ["Session and chat"],
        "quotes": [
            {"speaker": "Host", "text": "Welcome to our recorded session."},
            {"speaker": "Human", "text": "Can you inspect this log?"},
            {"speaker": "Assistant", "text": "The log is clean and valid."}
        ]
    });

    let mut log = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log,
        "126000_60",
        Some("default"),
        &segment_dir,
        sense_val.as_object().unwrap(),
    )
    .unwrap();
    log.finish().unwrap();

    let topics_content = fs::read_to_string(segment_dir.join("talents/topics.md")).unwrap();
    assert!(topics_content.contains("- Host: Welcome to our recorded session.\n"));
    assert!(topics_content.contains("- Human: Can you inspect this log?\n"));
    assert!(topics_content.contains("- Assistant: The log is clean and valid.\n"));
}

#[test]
fn topics_no_speech_fixtures_retain_topics_with_empty_quotes_section() {
    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(context.journal.as_path(), "work", "Work", "", "", "", None)
        .unwrap();

    let non_speech_files = [
        // Each file holds the quoted words, so only the speech-file selector can drop them.
        (
            "seg_imported_md",
            "imported.md",
            "# Imported document\nUnmatched quote words here\n",
        ),
        (
            "seg_transcript_md",
            "event_transcript.md",
            "# Event notes\nUnmatched quote words here\n",
        ),
        (
            "seg_workout_json",
            "workout.json",
            "{\"start\": \"00:00:01\", \"text\": \"Unmatched quote words here\"}",
        ),
        ("seg_note_txt", "note.txt", "Unmatched quote words here"),
        (
            "seg_screen_jsonl",
            "display_screen.jsonl",
            "{\"start\": \"00:00:01\", \"text\": \"Unmatched quote words here\"}\n",
        ),
    ];

    for (seg_name, filename, file_content) in non_speech_files {
        let segment_dir = context
            .journal
            .join("chronicle")
            .join(DAY)
            .join("default")
            .join(seg_name);
        fs::create_dir_all(&segment_dir).unwrap();
        fs::write(segment_dir.join(filename), file_content).unwrap();

        let sense_val = serde_json::json!({
            "density": "active",
            "content_type": "coding",
            "activity_summary": format!("Summary for {seg_name}"),
            "facets": [{"facet": "work", "level": "high"}],
            "entities": [],
            "topics": [format!("Topic for {seg_name}")],
            "quotes": [
                {"speaker": "x", "text": "Unmatched quote words here"}
            ]
        });

        let mut log = RunLogWriter::open(&context.journal, DAY, "segment");
        write_sense_and_change(
            &context,
            &mut log,
            seg_name,
            Some("default"),
            &segment_dir,
            sense_val.as_object().unwrap(),
        )
        .unwrap();
        log.finish().unwrap();

        let topics_content = fs::read_to_string(segment_dir.join("talents/topics.md")).unwrap();
        assert!(topics_content.contains(&format!("- Topic for {seg_name}\n")));
        assert!(topics_content.contains("## Quotes\n"));
        assert!(!topics_content.contains("Unmatched quote words here"));
        let quotes_section = topics_content.split("## Quotes\n").nth(1).unwrap();
        assert_eq!(quotes_section.trim(), "");
    }
}

#[test]
fn hosted_checkpoint_skips_segment_publication_and_uncancelled_twin_publishes() {
    struct TestCortex;
    impl CortexBoundary for TestCortex {
        fn dispatch(
            &self,
            _runtime: &tokio::runtime::Runtime,
            _request: &CortexRequest,
        ) -> Result<String, crate::context::DispatchFailure> {
            Ok(String::new())
        }
        fn dispatch_prepared(
            &self,
            _runtime: &tokio::runtime::Runtime,
            _request: &CortexRequest,
            _reserved: Option<&str>,
            _prepare: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
        ) -> Result<String, crate::context::DispatchFailure> {
            Ok(String::new())
        }
        fn wait(
            &self,
            _runtime: &tokio::runtime::Runtime,
            _use_ids: &[String],
            _deadline: Option<std::time::Duration>,
        ) -> Result<WaitForUsesReport, String> {
            Ok(WaitForUsesReport {
                completed: BTreeMap::new(),
                timed_out: Vec::new(),
            })
        }
    }

    struct TestIndex {
        rescanned: Arc<AtomicUsize>,
    }
    impl IndexBoundary for TestIndex {
        fn enqueue_path(&self, _journal: &Path, _path: &Path) -> Result<(), String> {
            self.rescanned.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let journal = tempfile::tempdir().unwrap();
    let day_dir = journal.path().join("chronicle").join(DAY);
    let segment_dir = day_dir.join("default/120000_60");
    let talents_dir = segment_dir.join("talents");
    fs::create_dir_all(&talents_dir).unwrap();
    fs::write(
        segment_dir.join("display_screen.jsonl"),
        "{\"text\":\"test\"}\n",
    )
    .unwrap();
    fs::write(talents_dir.join("sense.json"), "{\"density\":\"idle\"}").unwrap();

    let rescanned = Arc::new(AtomicUsize::new(0));
    let cancelled_context = ThinkContext::hosted(
        journal.path(),
        DAY.to_owned(),
        day_dir.clone(),
        NOW,
        journal.path().join("talents"),
        journal.path().join("apps"),
        Arc::new(TestCortex),
        Arc::new(TestIndex {
            rescanned: Arc::clone(&rescanned),
        }),
        Arc::new(|| NOW),
        None,
        Arc::new(|| true),
    );

    let publication_path = talents_dir.join("publication.json");
    assert!(!publication_path.exists());

    assert_eq!(
        crate::segment::publish_saved_outputs(&cancelled_context, &segment_dir).unwrap_err(),
        "cancelled before publication"
    );
    assert!(!publication_path.exists());
    assert_eq!(rescanned.load(Ordering::SeqCst), 0);

    let uncancelled_context = ThinkContext::hosted(
        journal.path(),
        DAY.to_owned(),
        day_dir,
        NOW,
        journal.path().join("talents"),
        journal.path().join("apps"),
        Arc::new(TestCortex),
        Arc::new(TestIndex {
            rescanned: Arc::clone(&rescanned),
        }),
        Arc::new(|| NOW),
        None,
        Arc::new(|| false),
    );

    crate::segment::publish_saved_outputs(&uncancelled_context, &segment_dir).unwrap();
    assert!(publication_path.exists());
    assert!(rescanned.load(Ordering::SeqCst) > 0);
}

#[test]
fn hosted_checkpoint_skips_activity_provenance_and_work_removal_and_uncancelled_twin_finishes() {
    struct TestCortex;
    impl CortexBoundary for TestCortex {
        fn dispatch(
            &self,
            _runtime: &tokio::runtime::Runtime,
            _request: &CortexRequest,
        ) -> Result<String, crate::context::DispatchFailure> {
            Ok(String::new())
        }
        fn dispatch_prepared(
            &self,
            _runtime: &tokio::runtime::Runtime,
            _request: &CortexRequest,
            _reserved: Option<&str>,
            _prepare: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
        ) -> Result<String, crate::context::DispatchFailure> {
            Ok(String::new())
        }
        fn wait(
            &self,
            _runtime: &tokio::runtime::Runtime,
            _use_ids: &[String],
            _deadline: Option<std::time::Duration>,
        ) -> Result<WaitForUsesReport, String> {
            Ok(WaitForUsesReport {
                completed: BTreeMap::new(),
                timed_out: Vec::new(),
            })
        }
    }
    struct TestIndex;
    impl IndexBoundary for TestIndex {
        fn enqueue_path(&self, _journal: &Path, _path: &Path) -> Result<(), String> {
            Ok(())
        }
    }

    let journal = tempfile::tempdir().unwrap();
    let day_dir = journal.path().join("chronicle").join(DAY);
    fs::create_dir_all(&day_dir).unwrap();
    solstone_core_facets::create_facet(journal.path(), "work", "Work", "", "", "", None).unwrap();

    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    let cancelled_context = ThinkContext::hosted(
        journal.path(),
        DAY.to_owned(),
        day_dir.clone(),
        NOW,
        journal.path().join("talents"),
        journal.path().join("apps"),
        Arc::new(TestCortex),
        Arc::new(TestIndex),
        Arc::new(|| NOW),
        None,
        Arc::new(move || flag.load(Ordering::Acquire)),
    );

    let mut work = crate::activity_work::ActivityWork::begin(
        &cancelled_context,
        "work",
        "meeting1",
        "hash_abc".to_owned(),
        std::collections::BTreeSet::new(),
        false,
    )
    .unwrap();

    cancelled.store(true, Ordering::Release);

    let prov_path =
        crate::segment::activity_provenance_path(&cancelled_context, DAY, "work", "meeting1");
    assert!(!prov_path.exists());

    // When cancelled, finish leaves provenance unwritten and activity work file intact
    assert_eq!(
        work.finish(&cancelled_context).unwrap_err(),
        "cancelled before publication"
    );
    assert!(!prov_path.exists());
    drop(work);

    // Uncancelled twin finishes: writes provenance and deletes activity work file
    let uncancelled_context = ThinkContext::hosted(
        journal.path(),
        DAY.to_owned(),
        day_dir,
        NOW,
        journal.path().join("talents"),
        journal.path().join("apps"),
        Arc::new(TestCortex),
        Arc::new(TestIndex),
        Arc::new(|| NOW),
        None,
        Arc::new(|| false),
    );

    // Reopen existing work to verify file is intact on disk
    let mut work =
        crate::activity_work::ActivityWork::open_existing(&uncancelled_context, "work", "meeting1")
            .unwrap()
            .expect("activity work should still exist on disk");

    work.finish(&uncancelled_context).unwrap();
    drop(work);
    assert!(prov_path.exists());
    let prov_content = fs::read_to_string(&prov_path).unwrap();
    assert!(prov_content.contains("hash_abc"));

    let after_finish =
        crate::activity_work::ActivityWork::open_existing(&uncancelled_context, "work", "meeting1")
            .unwrap();
    assert!(after_finish.is_none());
}

#[test]
fn a_terminal_segment_publishes_and_indexes_its_screen_text() {
    let (_journal, context) = fixture_journal();
    solstone_core_facets::create_facet(context.journal.as_path(), "work", "Work", "", "", "", None)
        .unwrap();
    let segment_dir = context
        .journal
        .join("chronicle")
        .join(DAY)
        .join("default")
        .join("120000_60");
    fs::create_dir_all(&segment_dir).unwrap();
    let frame = serde_json::json!({"timestamp": 2.0, "content": {"tmux": {
        "session": "main",
        "window": {"id": "@1", "index": 0, "name": "work"},
        "panes": [{"id": "%0", "index": 0, "active": true,
                   "content": "$ tmux show -g status-left-length\nzebraquartzmarker 40\n"}],
    }}});
    fs::write(segment_dir.join("tmux_0_screen.jsonl"), frame.to_string()).unwrap();
    let sense_val = serde_json::json!({
        "density": "active",
        "content_type": "coding",
        "activity_summary": "Terminal work",
        "facets": [{"facet": "work", "level": "high"}],
        "entities": [],
    });

    let mut log = RunLogWriter::open(&context.journal, DAY, "segment");
    write_sense_and_change(
        &context,
        &mut log,
        "120000_60",
        Some("default"),
        &segment_dir,
        sense_val.as_object().unwrap(),
    )
    .unwrap();
    log.finish().unwrap();

    let rel = "20260813/default/120000_60/talents/screen_text.md";
    let record = fs::read_to_string(segment_dir.join("talents/screen_text.md")).unwrap();
    assert!(record.contains("zebraquartzmarker 40"), "{record}");
    // The record indexes as its provenance line and then one chunk per timed group.
    solstone_core_indexer_store::todo::drain(&context.journal, Default::default()).unwrap();
    let conn = open_index(&context.journal).expect("open index db");
    let mut statement = conn
        .prepare("SELECT content FROM chunks WHERE path=?")
        .unwrap();
    let chunks = statement
        .query_map([rel], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        chunks
            .iter()
            .any(|chunk| chunk.contains("zebraquartzmarker 40")),
        "{chunks:?}"
    );
    let spec = resolve_spec(rel).expect("screen text resolves to spec");
    assert_eq!(spec.family, Family::ScreenText);
}
