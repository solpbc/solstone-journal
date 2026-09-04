// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Legacy timeline artifact migration.
//!
//! The V1 timeline schema (`be07691b9`) introduced `SegmentTimelineV1` with
//! `deny_unknown_fields`. Every segment artifact written before it carries a *flattened*
//! shape with `title` at the top level, so the strict reader rejects it and day rollups fail
//! for essentially the whole journal. Measured on the owner's journal 2026-09-04: **87,346
//! legacy artifacts against 481 V1**.
//!
//! Structural migration alone is not sufficient. `load_day_segments` runs a five-gate
//! gauntlet, and two of those gates need evidence a legacy artifact does not carry:
//! `verify_segment_source` requires a source binding, and `evaluate_artifact_currentness`
//! requires a durable publication-state record. So a migration has to go through the
//! product's own write path rather than rewriting JSON in place.
//!
//! It is also not safe to re-bind every artifact. A legacy summary is only truthfully bound
//! to today's source if that source has not been rewritten since the summary was produced --
//! `talents/activity.md` is itself a talent output and segment-repair regenerates it. Measured
//! over the full corpus: **79,062 artifacts (90%) have a source older than their summary and
//! can be re-bound; 8,284 (9%) have a source rewritten afterwards and must be regenerated
//! instead; 0 have no source at all.**
//!
//! This module implements the **plan** phase only: it classifies the corpus and reports what
//! a migration would do. It performs no writes. The commit phase is deliberately separate,
//! because its failure mode is the loss of five years of the owner's journal prose.

use std::fs;
use std::path::Path;

use serde_json::Value;

/// What a migration would do with one legacy artifact.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationPlan {
    /// Already `SegmentTimelineV1`; nothing to do.
    pub current: u64,
    /// Legacy, and its source predates the summary -- re-bindable without regenerating.
    pub rebind: u64,
    /// Legacy, but its source was rewritten after the summary -- the prose is stale.
    pub regenerate: u64,
    /// Legacy with no resolvable activity source -- cannot be source-bound at all.
    pub unbindable: u64,
    /// Present but unreadable as JSON.
    pub unreadable: u64,
}

impl MigrationPlan {
    pub fn total(&self) -> u64 {
        self.current + self.rebind + self.regenerate + self.unbindable + self.unreadable
    }

    pub fn render(&self) -> String {
        let mut out = String::from("timeline legacy-artifact migration plan (no changes made)\n\n");
        out.push_str(&format!("  already V1              {:>8}\n", self.current));
        out.push_str(&format!("  re-bindable             {:>8}\n", self.rebind));
        out.push_str(&format!(
            "  needs regeneration      {:>8}\n",
            self.regenerate
        ));
        out.push_str(&format!(
            "  unbindable (no source)  {:>8}\n",
            self.unbindable
        ));
        out.push_str(&format!(
            "  unreadable              {:>8}\n",
            self.unreadable
        ));
        out.push_str(&format!("  {:-<32}\n", ""));
        out.push_str(&format!("  total                   {:>8}\n", self.total()));
        out
    }
}

/// Classify a single artifact from its parsed body and the mtimes of artifact and source.
///
/// Split out from the filesystem walk so the decision is testable without a journal tree.
pub fn classify(
    body: Option<&Value>,
    source_mtime: Option<u64>,
    artifact_mtime: u64,
) -> &'static str {
    let Some(body) = body else {
        return "unreadable";
    };
    let Some(object) = body.as_object() else {
        return "unreadable";
    };
    if object.contains_key("schema_version") && object.get("summary").is_some_and(Value::is_object)
    {
        return "current";
    }
    if !object.contains_key("title") {
        return "unreadable";
    }
    let Some(source_mtime) = source_mtime else {
        return "unbindable";
    };
    // A source newer than the summary means the prose describes bytes that no longer exist.
    // Re-binding it would assert a correspondence nobody verified, which is exactly the
    // falsehood the source-binding model exists to prevent. One minute of slack absorbs
    // filesystem timestamp granularity.
    if source_mtime > artifact_mtime.saturating_add(60) {
        return "regenerate";
    }
    "rebind"
}

/// Walk the chronicle and classify every segment timeline artifact. Read-only.
pub fn plan(journal: &Path) -> MigrationPlan {
    let mut plan = MigrationPlan::default();
    let chronicle = journal.join("chronicle");
    let Ok(days) = fs::read_dir(&chronicle) else {
        return plan;
    };
    for day in days.flatten() {
        if !day.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(streams) = fs::read_dir(day.path()) else {
            continue;
        };
        for stream in streams.flatten() {
            if !stream.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(segments) = fs::read_dir(stream.path()) else {
                continue;
            };
            for segment in segments.flatten() {
                let directory = segment.path();
                let artifact = directory.join("timeline.json");
                let Ok(metadata) = fs::metadata(&artifact) else {
                    continue;
                };
                let artifact_mtime = seconds(metadata.modified().ok());
                let body = fs::read_to_string(&artifact)
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok());
                let source_mtime = fs::metadata(directory.join("talents").join("activity.md"))
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .map(|time| seconds(Some(time)));
                match classify(body.as_ref(), source_mtime, artifact_mtime) {
                    "current" => plan.current += 1,
                    "rebind" => plan.rebind += 1,
                    "regenerate" => plan.regenerate += 1,
                    "unbindable" => plan.unbindable += 1,
                    _ => plan.unreadable += 1,
                }
            }
        }
    }
    plan
}

fn seconds(time: Option<std::time::SystemTime>) -> u64 {
    time.and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classification_separates_rebindable_from_stale_and_current() {
        let legacy = json!({"title": "t", "description": "d", "origin": "o"});
        let v1 = json!({"schema_version": 1, "summary": {"title": "t"}});

        assert_eq!(classify(Some(&v1), Some(10), 20), "current");
        // source older than the summary: the prose saw these exact bytes
        assert_eq!(classify(Some(&legacy), Some(10), 20), "rebind");
        // source rewritten after the summary: the prose is stale
        assert_eq!(classify(Some(&legacy), Some(200), 20), "regenerate");
        // within the granularity slack, still re-bindable
        assert_eq!(classify(Some(&legacy), Some(50), 20), "rebind");
        assert_eq!(classify(Some(&legacy), None, 20), "unbindable");
        assert_eq!(classify(None, Some(10), 20), "unreadable");
        assert_eq!(classify(Some(&json!([])), Some(10), 20), "unreadable");
        // an object that is neither V1 nor legacy is not silently treated as either
        assert_eq!(
            classify(Some(&json!({"other": 1})), Some(10), 20),
            "unreadable"
        );
    }

    #[test]
    fn a_plan_totals_every_classified_artifact() {
        let plan = MigrationPlan {
            current: 1,
            rebind: 2,
            regenerate: 3,
            unbindable: 4,
            unreadable: 5,
        };
        assert_eq!(plan.total(), 15);
        let rendered = plan.render();
        assert!(rendered.contains("no changes made"), "{rendered}");
        assert!(rendered.contains("re-bindable"), "{rendered}");
    }
}
