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
//! [`plan`] classifies the corpus and writes nothing. [`commit`] performs the re-bind, and
//! only the re-bind: it never touches an artifact classified `regenerate`, `unbindable` or
//! `unreadable`, so the one thing it cannot do is overwrite prose with something worse.
//!
//! The re-bind goes through [`publish_segment_timeline`], the product's own write path, which
//! validates the artifact, takes the day and segment locks, re-verifies the source binding it
//! was just handed, writes atomically, and records the durable publication state the fifth
//! gate reads. Reconstructing that record by hand is exactly the shortcut this module exists
//! to avoid.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_timeline::{
    AttemptOutcome, AttemptStateV1, CURRENT_SCHEMA_VERSION, SEGMENT_SOURCE_SCHEMA_VERSION,
    SegmentBindingV1, SegmentSourceV1, SegmentSummaryV1, SegmentTimelineV1, TimelineKind,
    new_attempt_id, origin_for_binding, publish_continuation_summary, publish_segment_timeline,
    resolve_activity_source, segment_input_digest, segment_subject_key,
};

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

/// One candidate artifact found by the chronicle walk.
struct Candidate {
    directory: PathBuf,
    body: Option<Value>,
    source_mtime: Option<u64>,
    artifact_mtime: u64,
}

/// Walk the chronicle and yield every segment timeline artifact. Read-only.
///
/// `plan` and `commit` share this so the set `commit` acts on is by construction the set
/// `plan` counted -- a second, drifting walk is the obvious way for the two to disagree.
fn walk(journal: &Path) -> Vec<Candidate> {
    let mut found = Vec::new();
    let chronicle = journal.join("chronicle");
    let Ok(days) = fs::read_dir(&chronicle) else {
        return found;
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
                found.push(Candidate {
                    directory,
                    body,
                    source_mtime,
                    artifact_mtime,
                });
            }
        }
    }
    found
}

/// Classify every segment timeline artifact. Read-only.
pub fn plan(journal: &Path) -> MigrationPlan {
    let mut plan = MigrationPlan::default();
    for entry in walk(journal) {
        match classify(
            entry.body.as_ref(),
            entry.source_mtime,
            entry.artifact_mtime,
        ) {
            "current" => plan.current += 1,
            "rebind" => plan.rebind += 1,
            "regenerate" => plan.regenerate += 1,
            "unbindable" => plan.unbindable += 1,
            _ => plan.unreadable += 1,
        }
    }
    plan
}

/// What a migration actually did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationOutcome {
    /// Legacy artifacts republished as `SegmentTimelineV1`.
    pub rebound: u64,
    /// Left alone because their prose is stale; the think pipeline owns these.
    pub left_for_regeneration: u64,
    /// Already V1.
    pub untouched: u64,
    /// Attempted and refused by the product write path. The artifact is unchanged.
    pub failed: u64,
    /// The first few refusals, for the operator. Bounded so a corpus-wide fault cannot
    /// produce an unreadable wall of identical text.
    pub failures: Vec<String>,
}

const MAX_REPORTED_FAILURES: usize = 10;

/// The exact prose `publish_continuation_summary` writes.
///
/// A legacy continuation artifact carries a predecessor, and the strict schema refuses to let
/// a `generated_activity` source claim one -- that combination is what a continuation source
/// exists to express. Republishing through the product's continuation path is therefore the
/// only truthful option, and it rewrites the summary. Measured over the owner's corpus, all
/// 10,840 legacy continuation artifacts carry exactly these two strings, so that rewrite is
/// byte-identical. The constants below make that measurement a precondition rather than an
/// assumption: prose that differs is left alone instead of being overwritten.
const CONTINUATION_TITLE: &str = "Continued";
const CONTINUATION_DESCRIPTION: &str = "Unchanged from the prior window.";

impl MigrationOutcome {
    pub fn render(&self) -> String {
        let mut out = String::from("timeline legacy-artifact migration\n\n");
        out.push_str(&format!("  re-bound                {:>8}\n", self.rebound));
        out.push_str(&format!(
            "  left for regeneration   {:>8}\n",
            self.left_for_regeneration
        ));
        out.push_str(&format!(
            "  already V1              {:>8}\n",
            self.untouched
        ));
        out.push_str(&format!("  failed (unchanged)      {:>8}\n", self.failed));
        for failure in &self.failures {
            out.push_str(&format!("    - {failure}\n"));
        }
        if self.failed as usize > self.failures.len() {
            out.push_str(&format!(
                "    ... and {} more\n",
                self.failed as usize - self.failures.len()
            ));
        }
        out
    }
}

/// Recover a binding from the artifact's own location.
///
/// The legacy body carries an `origin` string, but the path is the authority: a body copied
/// between segments would otherwise re-bind that summary onto the wrong segment.
pub fn binding_from_directory(directory: &Path) -> Option<SegmentBindingV1> {
    let segment = directory.file_name()?.to_str()?.to_owned();
    let stream_dir = directory.parent()?;
    let stream = stream_dir.file_name()?.to_str()?.to_owned();
    let day = stream_dir.parent()?.file_name()?.to_str()?.to_owned();
    Some(SegmentBindingV1 {
        day,
        stream,
        segment,
    })
}

fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis()),
    )
    .unwrap_or(0)
}

/// A migration attempt record. The `publish_*` entry points overwrite `input_digest` with the
/// digest they actually publish, so the placeholder here is never the one that lands.
fn migration_attempt(subject: &str) -> AttemptStateV1 {
    AttemptStateV1 {
        attempt_id: new_attempt_id(&format!("migrate-{subject}")),
        input_digest: String::new(),
        started_at_ms: now_ms(),
        finished_at_ms: None,
        outcome: AttemptOutcome::Running,
        detail: String::new(),
    }
}

fn text_field(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Republish one legacy artifact as V1 through the product write path.
fn rebind_one(
    journal: &Path,
    directory: &Path,
    body: &Value,
    artifact_mtime: u64,
) -> Result<(), String> {
    let binding = binding_from_directory(directory)
        .ok_or_else(|| format!("{}: unreadable segment path", directory.display()))?;
    let subject = segment_subject_key(&binding);
    // Legacy artifacts stamp `generated_at` in whole seconds. Keeping the original instant
    // matters: currentness is ordered against it, so stamping "now" would assert that a
    // five-year-old summary was produced during the migration.
    let generated_at_ms = body
        .get("generated_at")
        .and_then(Value::as_i64)
        .map_or_else(
            || {
                i64::try_from(artifact_mtime)
                    .unwrap_or(0)
                    .saturating_mul(1000)
            },
            |seconds| seconds.saturating_mul(1000),
        );
    if let Some(predecessor) = body
        .get("continuation_of")
        .and_then(Value::as_str)
        .filter(|predecessor| !predecessor.is_empty())
    {
        if text_field(body, "title") != CONTINUATION_TITLE
            || text_field(body, "description") != CONTINUATION_DESCRIPTION
        {
            return Err(format!(
                "{subject}: continuation carries prose the continuation path would overwrite"
            ));
        }
        return publish_continuation_summary(
            journal,
            binding,
            predecessor.to_owned(),
            generated_at_ms,
            migration_attempt(&subject),
        )
        .map_err(|error| format!("{subject}: {error}"));
    }
    let snapshot = resolve_activity_source(journal, &binding)
        .map_err(|error| format!("{subject}: {error}"))?
        .ok_or_else(|| format!("{subject}: activity source disappeared"))?;
    let source = SegmentSourceV1::GeneratedActivity {
        schema_version: SEGMENT_SOURCE_SCHEMA_VERSION,
        relative_path: snapshot.relative_path,
        sha256: snapshot.sha256,
    };
    let input_digest =
        segment_input_digest(&binding, &source).map_err(|error| format!("{subject}: {error}"))?;
    let summary = SegmentSummaryV1 {
        title: text_field(body, "title"),
        description: text_field(body, "description"),
        origin: origin_for_binding(&binding).map_err(|error| format!("{subject}: {error}"))?,
        continuation_of: body
            .get("continuation_of")
            .and_then(Value::as_str)
            .map(str::to_owned),
    };
    let timeline = SegmentTimelineV1 {
        schema_version: CURRENT_SCHEMA_VERSION,
        kind: TimelineKind::Segment,
        binding: binding.clone(),
        input_digest: input_digest.clone(),
        source: Some(source),
        generated_at_ms,
        summary,
        // A legacy artifact records only `model`, and `GenerationProvenanceV1` also requires a
        // finish reason, schema-validation result, inference record and usage. Synthesizing
        // those would put invented evidence behind a provenance field, so the migration
        // declares no provenance rather than a partly-fabricated one.
        // `publish_continuation_summary` sets the same precedent.
        provenance: None,
    };
    let mut attempt = migration_attempt(&subject);
    attempt.input_digest = input_digest;
    publish_segment_timeline(journal, &timeline, attempt)
        .map_err(|error| format!("{subject}: {error}"))
}

/// Walk the chronicle and re-bind every re-bindable legacy artifact.
///
/// `limit` bounds how many artifacts are ATTEMPTED in one invocation, so a first run can be
/// made small and inspected before the whole corpus is committed to. Bounding successes
/// instead would let a run that refuses hundreds still walk the entire corpus.
pub fn commit(journal: &Path, limit: Option<u64>) -> MigrationOutcome {
    let mut outcome = MigrationOutcome::default();
    let mut attempted = 0u64;
    for entry in walk(journal) {
        match classify(
            entry.body.as_ref(),
            entry.source_mtime,
            entry.artifact_mtime,
        ) {
            "current" => outcome.untouched += 1,
            "regenerate" => outcome.left_for_regeneration += 1,
            "rebind" => {
                if limit.is_some_and(|limit| attempted >= limit) {
                    continue;
                }
                attempted += 1;
                let Some(body) = entry.body.as_ref() else {
                    continue;
                };
                match rebind_one(journal, &entry.directory, body, entry.artifact_mtime) {
                    Ok(()) => outcome.rebound += 1,
                    Err(detail) => {
                        outcome.failed += 1;
                        if outcome.failures.len() < MAX_REPORTED_FAILURES {
                            outcome.failures.push(detail);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    outcome
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

    /// Set an exact mtime rather than sleeping: the classifier's slack is 60 seconds, so a
    /// test that tried to straddle it by sleeping would either be wrong or take a minute.
    fn set_mtime(path: &Path, seconds_ago: u64) {
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(seconds_ago);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(when))
            .unwrap();
    }

    fn legacy_journal(rebindable: bool) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let segment = root
            .path()
            .join("chronicle")
            .join("20260101")
            .join("device")
            .join("120000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        let activity = segment.join("talents").join("activity.md");
        let artifact = segment.join("timeline.json");
        fs::write(&activity, "# activity\n").unwrap();
        fs::write(
            &artifact,
            r#"{"title":"T","description":"D","origin":"20260101/device/120000_300","model":"local/m","generated_at":1700000000}"#,
        )
        .unwrap();
        if rebindable {
            // Source predates the summary: the prose saw these exact bytes.
            set_mtime(&activity, 7200);
            set_mtime(&artifact, 3600);
        } else {
            // Source rewritten well after the summary, past the 60s slack.
            set_mtime(&artifact, 7200);
            set_mtime(&activity, 3600);
        }
        root
    }

    #[test]
    fn a_binding_comes_from_the_path_not_the_body() {
        let binding =
            binding_from_directory(Path::new("/j/chronicle/20260101/device/120000_300")).unwrap();
        assert_eq!(binding.day, "20260101");
        assert_eq!(binding.stream, "device");
        assert_eq!(binding.segment, "120000_300");
    }

    /// The whole point of the commit phase: a legacy artifact comes out the other side as a
    /// `SegmentTimelineV1` that the strict reader accepts, with its prose and its original
    /// generation instant intact.
    #[test]
    fn committing_rebinds_a_legacy_artifact_into_accepted_v1() {
        let root = legacy_journal(true);
        let journal = root.path();
        let outcome = commit(journal, None);
        assert_eq!(outcome.rebound, 1, "{:?}", outcome.failures);
        assert_eq!(outcome.failed, 0, "{:?}", outcome.failures);

        let text = fs::read_to_string(
            journal
                .join("chronicle/20260101/device/120000_300")
                .join("timeline.json"),
        )
        .unwrap();
        let migrated: SegmentTimelineV1 = serde_json::from_str(&text)
            .expect("migrated artifact parses under deny_unknown_fields");
        assert_eq!(migrated.summary.title, "T");
        assert_eq!(migrated.summary.description, "D");
        assert_eq!(migrated.summary.origin, "20260101/device/120000_300");
        assert_eq!(migrated.binding.segment, "120000_300");
        assert!(migrated.source.is_some(), "source binding is required");
        // Preserved, not restamped: currentness is ordered against this instant.
        assert_eq!(migrated.generated_at_ms, 1_700_000_000_000);
        assert!(migrated.provenance.is_none(), "no invented provenance");

        // Idempotent: a second pass sees V1 and does nothing.
        let again = commit(journal, None);
        assert_eq!(again.rebound, 0);
        assert_eq!(again.untouched, 1);
    }

    /// A stale artifact is the one case where re-binding would assert a correspondence
    /// nobody verified, so commit must leave it exactly as it found it.
    #[test]
    fn committing_never_touches_an_artifact_whose_source_moved_on() {
        let root = legacy_journal(false);
        let journal = root.path();
        let artifact = journal
            .join("chronicle/20260101/device/120000_300")
            .join("timeline.json");
        let before = fs::read_to_string(&artifact).unwrap();

        let outcome = commit(journal, None);
        assert_eq!(outcome.rebound, 0);
        assert_eq!(outcome.left_for_regeneration, 1);
        assert_eq!(fs::read_to_string(&artifact).unwrap(), before);
    }

    fn legacy_continuation_journal(title: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let segment = root
            .path()
            .join("chronicle")
            .join("20260101")
            .join("device")
            .join("120000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        let activity = segment.join("talents").join("activity.md");
        let change = segment.join("talents").join("change.json");
        let artifact = segment.join("timeline.json");
        fs::write(&activity, "# activity\n").unwrap();
        fs::write(&change, r#"{"change_class":"active"}"#).unwrap();
        fs::write(
            &artifact,
            format!(
                r#"{{"title":"{title}","description":"Unchanged from the prior window.","origin":"20260101/device/120000_300","continuation_of":"115500_300","generated_at":1700000000}}"#
            ),
        )
        .unwrap();
        set_mtime(&activity, 7200);
        set_mtime(&change, 7200);
        set_mtime(&artifact, 3600);
        root
    }

    /// A legacy continuation cannot be re-bound as a generated activity -- the schema refuses
    /// that pairing -- so it goes through the product's continuation path instead, and the
    /// prose it writes is the prose that was already there.
    #[test]
    fn a_legacy_continuation_migrates_through_the_continuation_path() {
        let root = legacy_continuation_journal(CONTINUATION_TITLE);
        let journal = root.path();
        let outcome = commit(journal, None);
        assert_eq!(outcome.rebound, 1, "{:?}", outcome.failures);
        assert_eq!(outcome.failed, 0, "{:?}", outcome.failures);

        let text = fs::read_to_string(
            journal
                .join("chronicle/20260101/device/120000_300")
                .join("timeline.json"),
        )
        .unwrap();
        let migrated: SegmentTimelineV1 = serde_json::from_str(&text).expect("parses as V1");
        assert_eq!(migrated.summary.title, CONTINUATION_TITLE);
        assert_eq!(migrated.summary.description, CONTINUATION_DESCRIPTION);
        assert_eq!(
            migrated.summary.continuation_of.as_deref(),
            Some("115500_300"),
            "the predecessor survives the migration"
        );
    }

    /// The continuation path rewrites the summary, so it may only be used where that rewrite
    /// is a no-op. Prose that differs is left byte-identical rather than overwritten.
    #[test]
    fn a_continuation_with_unexpected_prose_is_refused_not_overwritten() {
        let root = legacy_continuation_journal("Something an owner would miss");
        let journal = root.path();
        let artifact = journal
            .join("chronicle/20260101/device/120000_300")
            .join("timeline.json");
        let before = fs::read_to_string(&artifact).unwrap();

        let outcome = commit(journal, None);
        assert_eq!(outcome.rebound, 0);
        assert_eq!(outcome.failed, 1);
        assert!(
            outcome.failures[0].contains("would overwrite"),
            "{:?}",
            outcome.failures
        );
        assert_eq!(fs::read_to_string(&artifact).unwrap(), before);
    }

    #[test]
    fn a_zero_limit_is_not_expressible_and_a_limit_bounds_the_rewrite() {
        let root = legacy_journal(true);
        let journal = root.path();
        let outcome = commit(journal, Some(0));
        assert_eq!(outcome.rebound, 0, "a zero limit rewrites nothing");
        let outcome = commit(journal, Some(5));
        assert_eq!(outcome.rebound, 1);
    }

    #[test]
    fn an_outcome_reports_bounded_failures() {
        let mut outcome = MigrationOutcome {
            rebound: 2,
            failed: 12,
            ..MigrationOutcome::default()
        };
        outcome.failures = (0..MAX_REPORTED_FAILURES)
            .map(|index| format!("segment:{index}"))
            .collect();
        let rendered = outcome.render();
        assert!(rendered.contains("re-bound"), "{rendered}");
        assert!(rendered.contains("... and 2 more"), "{rendered}");
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
