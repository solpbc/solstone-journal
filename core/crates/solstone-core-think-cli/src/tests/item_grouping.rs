// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An item from a syncing source is one activity, whichever path writes it.

use super::*;

const DAY: &str = "20260813";
const NOW: i64 = 1_786_615_200_000;

/// A UTC journal with one facet and two screenshots a minute apart on a
/// syncing `screenshots` stream, both thought, neither holding an activity.
fn shots() -> (tempfile::TempDir, tempfile::TempDir, context::ThinkContext) {
    let journal = tempdir().unwrap();
    let roots = tempdir().unwrap();
    fs::create_dir_all(journal.path().join("config")).unwrap();
    fs::write(
        journal.path().join("config/journal.json"),
        r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#,
    )
    .unwrap();
    fs::create_dir_all(journal.path().join("streams")).unwrap();
    fs::write(
        journal.path().join("streams/shots.json"),
        r#"{"name":"shots","kind":"observer","host":null,"platform":null,"created_at":0,"last_day":null,"last_segment":null,"seq":1,"source":"screenshots","cid":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
    )
    .unwrap();
    solstone_core_facets::create_facet(journal.path(), "work", "work", "", "", "", None).unwrap();
    let (talent_root, apps_root) = talent_roots(roots.path(), &[]);
    let (context, _recorder) = recorder_context(journal.path(), DAY, NOW);
    let sense = serde_json::json!({"density":"active","content_type":"work","activity_summary":"work",
        "facets":[{"facet":"work","level":"high","activity":"work"}]});
    for key in ["100000_1", "100100_1"] {
        let dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join("shots")
            .join(key);
        fs::create_dir_all(dir.join("talents")).unwrap();
        fs::write(dir.join("screenshot_screen.png"), b"png").unwrap();
        fs::write(dir.join("talents/sense.json"), sense.to_string()).unwrap();
    }
    (
        journal,
        roots,
        context.with_talent_roots(talent_root, apps_root),
    )
}

fn later(context: &context::ThinkContext, now_ms: i64) -> context::ThinkContext {
    let mut next = context.clone();
    next.now_ms = now_ms;
    next.with_event_clock(Arc::new(move || now_ms))
}

fn rows(context: &context::ThinkContext) -> Vec<Vec<String>> {
    let mut rows = solstone_core_facets::load_activity_records(&context.journal, "work", DAY, true)
        .unwrap()
        .into_iter()
        .map(|record| {
            record["segments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|segment| segment.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    rows.sort();
    rows
}

fn fill_in(context: &context::ThinkContext) {
    let week_on = later(context, NOW + 7 * 86_400_000);
    segment::ensure_historical_day_activities(&week_on, &mut test_log(&week_on, "daily"), 1, true)
        .unwrap();
}

/// The settle rebuild as of `now_ms`: what writes a syncing stream's items
/// for today, yesterday and any day waiting on it.
fn settle(context: &context::ThinkContext, now_ms: i64) {
    let then = later(context, now_ms);
    crate::settle::run(
        &then,
        &mut test_log(&then, "settle"),
        Some("shots"),
        1,
        true,
    )
    .unwrap();
}

fn activities_file(context: &context::ThinkContext) -> std::path::PathBuf {
    context
        .journal
        .join("facets/work/activities")
        .join(format!("{DAY}.jsonl"))
}

#[test]
fn an_item_is_one_activity_whether_it_settles_or_is_filled_in_later() {
    let (_journal, _roots, context) = shots();
    settle(&context, NOW + 86_400_000);
    assert_eq!(rows(&context), [vec!["100000_1"], vec!["100100_1"]]);
    // The fill-in replays only segments no row holds, so it adds nothing here.
    fill_in(&context);
    assert_eq!(rows(&context), [vec!["100000_1"], vec!["100100_1"]]);

    // The same evidence reaching the fill-in first: settle, reading the day
    // after it, groups the items the same way and leaves the file as it is.
    let (_journal, _roots, context) = shots();
    fill_in(&context);
    assert_eq!(rows(&context), [vec!["100000_1"], vec!["100100_1"]]);
    let written = fs::read(activities_file(&context)).unwrap();
    settle(&context, NOW + 86_400_000);
    assert_eq!(fs::read(activities_file(&context)).unwrap(), written);
}

#[test]
fn an_activity_an_older_version_made_of_two_items_stays_as_written() {
    for age_days in [1, 10] {
        let (_journal, _roots, context) = shots();
        fill_in(&context);
        // What the older grouping wrote: one activity over both screenshots.
        let path = activities_file(&context);
        let text = fs::read_to_string(&path).unwrap();
        let mut merged: serde_json::Value =
            serde_json::from_str(text.lines().next().unwrap()).unwrap();
        merged["segments"] = serde_json::json!(["100000_1", "100100_1"]);
        fs::write(&path, format!("{merged}\n")).unwrap();
        let written = fs::read(&path).unwrap();

        let then = later(&context, NOW + age_days * 86_400_000);
        crate::settle::note(&then, "shots", &[DAY]).unwrap();
        settle(&context, NOW + age_days * 86_400_000);
        assert_eq!(fs::read(&path).unwrap(), written, "age {age_days}");
        assert_eq!(rows(&context), [vec!["100000_1", "100100_1"]]);
    }
}

#[test]
fn filling_in_one_day_leaves_reading_all_of_history_to_the_end_of_the_walk() {
    let (_journal, _roots, context) = shots();
    let before = segment::SPAN_READS.with(|reads| reads.get());
    fill_in(&context);
    assert_eq!(rows(&context), [vec!["100000_1"], vec!["100100_1"]]);
    // The day's own activities are written; what history as a whole earns is
    // not read here, and nothing says it was.
    assert_eq!(segment::SPAN_READS.with(|reads| reads.get()), before);
    assert!(
        !context
            .journal
            .join("awareness/backfill_span_state.json")
            .exists()
    );
}
