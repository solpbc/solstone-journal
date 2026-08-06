// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The predicate is driven by the committed release oracle, not by expectations
//! typed here.
//!
//! `core/fixtures/retention_release_oracle.json` records, for 23 constructed
//! on-disk shapes, what the Python reference decides and what a unified predicate
//! must decide. Every reference verdict in it was observed by executing the
//! reference; its generator gates that each row rebuilds from the JSON alone.
//!
//! So this test reads the file and reproduces it. ⛔ It does not restate the rows:
//! a hand-typed expectation would be a second, decaying copy of a fact the fixture
//! already owns, and the disagreements it encodes are exactly what a fresh reading
//! would smooth over.

use std::collections::BTreeMap;

use serde_json::Value;
use solstone_core_retention::content::{ContentName, HandlerRegistry};
use solstone_core_retention::eligibility::{
    Blocker, Evidence, FileVerdict, SidecarFacts, resolve_file,
};

const ORACLE: &str = include_str!("../../../fixtures/retention_release_oracle.json");

/// The extension map, as the media-processing boundary would supply it.
///
/// ⚠ Images are absent deliberately, and that absence is what the fixture's two
/// still-image rows exercise: no handler claims them, so nothing can prove one was
/// consumed.
struct ClosedSet;

impl HandlerRegistry for ClosedSet {
    fn expected_handler(&self, name: &ContentName) -> Option<&str> {
        match name.extension()?.as_str() {
            "flac" | "opus" | "ogg" | "m4a" | "mp3" | "wav" => Some("transcribe"),
            "webm" | "mp4" | "mov" => Some("describe"),
            _ => None,
        }
    }
}

/// A registry that claims every name, for the falsification direction.
struct ClaimsEverything;

impl HandlerRegistry for ClaimsEverything {
    fn expected_handler(&self, name: &ContentName) -> Option<&str> {
        // ⚠ Must return the handler the record NAMES, not a fixed one: the
        // conjunction compares them, so a fixed "transcribe" would refuse the
        // image row for the wrong reason and the test would go red on a correct
        // predicate.
        match name.extension().as_deref() {
            Some("png" | "jpg" | "jpeg" | "heic" | "webp" | "gif" | "tiff") => Some("depict"),
            _ => self_transcribe(),
        }
    }
}

fn self_transcribe() -> Option<&'static str> {
    Some("transcribe")
}

#[derive(Clone)]
struct Row {
    id: String,
    media: String,
    size: u64,
    record: Option<Value>,
    has_analysis_row: bool,
    unified_verdict: String,
    unified_blocker: Option<String>,
    unified_evidence: Option<String>,
}

/// Rebuild each row's inputs from the published shape.
///
/// ⚠ The record is read from `sidecar_lines`, not from the `sidecar` field: the
/// record is a value under `_solstone_processing` in a metadata header, and the
/// fixture says so in its own provenance block. Taking `sidecar` as the header
/// yields header-only semantics on every record-bearing row — which most rows
/// already report as held, so the mistake stays green.
fn rows() -> Vec<Row> {
    let document: Value = serde_json::from_str(ORACLE).expect("the oracle is valid JSON");
    let analysis_keys = ["start", "timestamp"];
    document
        .get("rows")
        .and_then(Value::as_array)
        .expect("the oracle has rows")
        .iter()
        .map(|row| {
            let shape = row.get("shape").expect("a row has a shape");
            let lines = shape.get("sidecar_lines").expect("a shape names its lines");
            let mut record = None;
            let mut has_analysis_row = false;
            if let Some(lines) = lines.as_array() {
                for (index, line) in lines.iter().enumerate() {
                    let text = line.as_str().expect("a sidecar line is a string");
                    let parsed: Value =
                        serde_json::from_str(text).expect("a sidecar line is valid JSON");
                    if index == 0 {
                        record = parsed.get("_solstone_processing").cloned();
                    } else if analysis_keys.iter().any(|key| parsed.get(key).is_some()) {
                        has_analysis_row = true;
                    }
                }
            }
            let unified = row.get("unified").expect("a row has a unified verdict");
            Row {
                id: string(row, "id"),
                media: string(shape, "media"),
                size: shape
                    .get("size")
                    .and_then(Value::as_u64)
                    .expect("a shape has a size"),
                record,
                has_analysis_row,
                unified_verdict: string(unified, "verdict"),
                unified_blocker: unified
                    .get("blocker")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                unified_evidence: unified
                    .get("evidence")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }
        })
        .collect()
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string at {key}"))
        .to_owned()
}

fn decide(registry: &dyn HandlerRegistry, row: &Row) -> FileVerdict {
    let name = ContentName::new(&row.media).expect("the fixture's media names are plain");
    resolve_file(
        registry,
        "20260805",
        "field.audio",
        "070000_17",
        &name,
        row.size,
        &SidecarFacts {
            record: row.record.clone(),
            has_analysis_row: row.has_analysis_row,
        },
    )
}

/// Every row's verdict AND its discriminant.
///
/// ⛔ Asserting the verdict alone is not enough. Most rows report `held`, so a
/// predicate that reached the right verdict for the wrong reason — never
/// constructing `Failed`, say, because the borrowed conjunction cannot
/// distinguish a failure from a mismatch — would pass on a majority of them.
#[test]
fn every_oracle_row_is_reproduced_with_its_discriminant() {
    let rows = rows();
    assert_eq!(rows.len(), 23, "the oracle's row count changed");
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();

    for row in &rows {
        let verdict = decide(&ClosedSet, row);
        match (&verdict, row.unified_verdict.as_str()) {
            (FileVerdict::Releasable(proven), "releasable") => {
                let expected = row
                    .unified_evidence
                    .as_deref()
                    .expect("a releasable row names its evidence");
                let actual = match proven.evidence() {
                    Evidence::Record => "record",
                    Evidence::LegacyRows => "legacy_rows",
                };
                assert_eq!(actual, expected, "row `{}` evidence", row.id);
                *seen.entry("releasable").or_default() += 1;
            }
            (FileVerdict::Held(blocker), "held") => {
                let expected = row
                    .unified_blocker
                    .as_deref()
                    .expect("a held row names its blocker");
                let actual = match blocker {
                    Blocker::Incomplete { .. } => "incomplete",
                    Blocker::Failed { .. } => "failed",
                    Blocker::Unprovable { .. } => "unprovable",
                };
                assert_eq!(actual, expected, "row `{}` blocker", row.id);
                *seen.entry("held").or_default() += 1;
            }
            (got, expected) => {
                panic!(
                    "row `{}`: oracle says {expected}, predicate says {got:?}",
                    row.id
                )
            }
        }
    }

    // The guard must have exercised a non-empty surface in both directions, and
    // every discriminant the fixture uses must have been reached.
    assert!(seen.get("releasable").copied().unwrap_or(0) > 0);
    assert!(seen.get("held").copied().unwrap_or(0) > 0);
}

/// Every blocker variant and both evidence kinds are actually reached.
///
/// Without this, a predicate could collapse two variants and still match every
/// row whose expected discriminant happened to be the surviving one.
#[test]
fn the_oracle_exercises_every_discriminant() {
    let mut blockers = BTreeMap::new();
    let mut evidence = BTreeMap::new();
    for row in &rows() {
        match decide(&ClosedSet, row) {
            FileVerdict::Held(Blocker::Incomplete { .. }) => {
                *blockers.entry("incomplete").or_insert(0) += 1;
            }
            FileVerdict::Held(Blocker::Failed { .. }) => {
                *blockers.entry("failed").or_insert(0) += 1;
            }
            FileVerdict::Held(Blocker::Unprovable { .. }) => {
                *blockers.entry("unprovable").or_insert(0) += 1;
            }
            FileVerdict::Releasable(proven) => {
                let key = match proven.evidence() {
                    Evidence::Record => "record",
                    Evidence::LegacyRows => "legacy_rows",
                };
                *evidence.entry(key).or_insert(0) += 1;
            }
        }
    }
    for key in ["incomplete", "failed", "unprovable"] {
        assert!(blockers.contains_key(key), "no row reached blocker `{key}`");
    }
    for key in ["record", "legacy_rows"] {
        assert!(
            evidence.contains_key(key),
            "no row reached evidence `{key}`"
        );
    }
}

/// A still image is unprovable under the closed set, and releasable the moment a
/// handler claims it — the two directions that prove the arm is consulted.
///
/// ⚠ Only the inverted direction shows the `Unprovable` arm is reached at all:
/// under the closed set alone, an implementation that never constructs it would
/// still hold every image row for some other reason.
#[test]
fn a_still_image_is_held_only_because_no_handler_claims_it() {
    let image = rows()
        .into_iter()
        .find(|row| row.media.ends_with(".png") && row.record.is_some())
        .expect("the oracle carries a still image with a record");

    match decide(&ClosedSet, &image) {
        FileVerdict::Held(Blocker::Unprovable { name }) => assert_eq!(name, image.media),
        other => panic!("expected Unprovable under the closed set, got {other:?}"),
    }
    match decide(&ClaimsEverything, &image) {
        FileVerdict::Releasable(_) => {}
        other => panic!("a registry that claims it should release it, got {other:?}"),
    }
}

/// The analyzed-rows condition holds exactly one row, and `has_analysis_row`
/// feeds two conditions rather than one.
///
/// The fixture's whole reason for existing is that the two irreversible readers
/// disagree, and this is the disagreement the borrowed conjunction does not
/// contain: the ingest path accepts a record claiming `analyzed` whose rows are
/// gone, and releasing that raw destroys the only surviving copy of the content.
///
/// ⚠ Two earlier versions of this test were wrong in instructive ways. The first
/// selected rows whose SHAPE matched the condition's target and caught a row held
/// for an unrelated reason (no schema, so the conjunction refuses first) —
/// describing intent rather than measuring. The second toggled `has_analysis_row`
/// and caught three rows, because that flag also feeds the pre-record legacy-rows
/// branch. So the condition's own signature is the observable: its reason string.
#[test]
fn exactly_one_row_is_held_by_the_analyzed_rows_condition() {
    let held_by_condition: Vec<String> = rows()
        .into_iter()
        .filter_map(|row| match decide(&ClosedSet, &row) {
            FileVerdict::Held(Blocker::Incomplete { because, .. })
                if because.contains("only surviving copy") =>
            {
                Some(row.id)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        held_by_condition,
        vec!["record_terminal_no_row".to_owned()],
        "exactly one row is held by the analyzed-rows condition"
    );
}

/// `has_analysis_row` feeds TWO conditions, and which rows each holds is not
/// obvious from either one.
///
/// Recorded because it is a live trap for anyone later "simplifying" the two into
/// one: the flag decides both whether a record claiming `analyzed` is trustworthy
/// AND whether a pre-record sidecar has any evidence at all.
#[test]
fn the_analysis_row_flag_feeds_both_the_record_and_the_legacy_arms() {
    let flipped: Vec<String> = rows()
        .into_iter()
        .filter(|row| {
            let held_now = matches!(decide(&ClosedSet, row), FileVerdict::Held(_));
            let mut with_rows = row.clone();
            with_rows.has_analysis_row = true;
            held_now && matches!(decide(&ClosedSet, &with_rows), FileVerdict::Releasable(_))
        })
        .map(|row| row.id)
        .collect();
    assert_eq!(
        flipped,
        vec![
            "record_terminal_no_row".to_owned(),
            "record_absent_no_row".to_owned(),
            "non_analysis_row".to_owned(),
        ],
        "one row flips through the record arm, two through the legacy arm"
    );
}
