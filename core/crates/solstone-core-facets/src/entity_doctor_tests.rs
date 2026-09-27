// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::entity_doctor::{
    DeleteVerdict, EntityDoctorError, check_entity_records, repair_entity_records,
};
use crate::store_tests::TempDir;

fn action(root: &Path, day: &str, timestamp: &str, params: Value) {
    let path = root.join("config/actions").join(format!("{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let row = json!({"timestamp": timestamp, "source": "app", "actor": "entities",
        "action": "journal_entity_delete", "params": params});
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{row}\n"));
    fs::write(path, text).unwrap();
}

fn merge(root: &Path, source: &str, target: &str, ts: i64) {
    let path = root.join("logs/entity-merges.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!(
        "{}\n",
        json!({"ts": ts, "merge_id": "m", "source_id": source, "target_id": target})
    ));
    fs::write(path, text).unwrap();
}

fn live(root: &Path, id: &str) {
    solstone_core_entity::save_entity_identity(root, id, &json!({"id": id, "name": id}), None)
        .unwrap();
}

fn record(root: &Path) -> Option<Value> {
    fs::read_to_string(root.join("entities/retired.json"))
        .ok()
        .map(|text| serde_json::from_str(&text).unwrap())
}

fn verdict(report: &crate::entity_doctor::EntityDoctorReport, id: &str) -> DeleteVerdict {
    report
        .findings
        .iter()
        .find(|finding| finding.entity_id == id)
        .unwrap_or_else(|| panic!("no finding for {id}: {report:?}"))
        .verdict
        .clone()
}

const T1: &str = "2026-01-23T10:00:00.000000-07:00";
const T2: &str = "2026-05-01T10:00:00+00:00";
const T3: &str = "2026-05-01T10:00:30+00:00";

#[test]
fn deletes_that_removed_the_entity_are_recorded_and_the_rest_are_not() {
    let journal = TempDir::new();
    let root = journal.path();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "immediate", "facets_deleted": []}),
    );
    for (pending, id, phase, extra) in [
        ("p1", "committed", "committed", json!({})),
        (
            "p2",
            "failed_removed",
            "failed",
            json!({"entity_removed": true}),
        ),
        ("p3", "refused", "refused", json!({})),
        (
            "p4",
            "failed_kept",
            "failed",
            json!({"entity_removed": false}),
        ),
        ("p5", "unsettled", "failed", json!({"unsettled": true})),
    ] {
        action(
            root,
            "20260501",
            T2,
            json!({"entity_id": id, "pending_id": pending, "phase": "pending"}),
        );
        let mut params = json!({"entity_id": id, "pending_id": pending, "phase": phase});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        action(root, "20260501", T3, params);
    }
    action(
        root,
        "20260501",
        T2,
        json!({"entity_id": "cancelled", "pending_id": "p6", "phase": "pending"}),
    );
    action(
        root,
        "20260501",
        T3,
        json!({"pending_id": "p6", "phase": "cancelled"}),
    );
    action(
        root,
        "20260501",
        T2,
        json!({"entity_id": "never_finished", "pending_id": "p7", "phase": "pending"}),
    );

    let report = repair_entity_records(root).unwrap();
    let mut recorded = report.recorded.clone();
    recorded.sort();
    assert_eq!(recorded, ["committed", "failed_removed", "immediate"]);
    let ids = record(root).unwrap()["ids"].clone();
    assert_eq!(ids.as_object().unwrap().len(), 3);
    assert_eq!(ids["immediate"]["state"], "deleted");
    assert_eq!(ids["immediate"]["seeded"], true);
    assert_eq!(ids["immediate"]["at"], T1);
    // A deferred delete is dated by its confirmation, not its outcome row.
    assert_eq!(ids["committed"]["at"], T2);
    let unfinished: Vec<Option<String>> = report
        .unfinished
        .iter()
        .map(|u| u.entity_id.clone())
        .collect();
    assert!(
        unfinished.contains(&Some("never_finished".into())),
        "{unfinished:?}"
    );
    assert!(
        unfinished.contains(&Some("unsettled".into())),
        "{unfinished:?}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root.join("entities/retired.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // A second run changes nothing.
    let before = fs::read(root.join("entities/retired.json")).unwrap();
    let again = repair_entity_records(root).unwrap();
    assert!(again.recorded.is_empty());
    assert_eq!(
        fs::read(root.join("entities/retired.json")).unwrap(),
        before
    );
}

#[test]
fn an_entity_still_in_the_journal_is_never_recorded() {
    let journal = TempDir::new();
    let root = journal.path();
    live(root, "back_again");
    // Something is still at the entity's folder, though it has no identity.
    fs::create_dir_all(root.join("entities/broken")).unwrap();
    fs::write(root.join("entities/broken/voiceprints.npz"), "x").unwrap();
    for id in ["back_again", "broken"] {
        action(
            root,
            "20260123",
            T1,
            json!({"entity_id": id, "facets_deleted": []}),
        );
    }
    let report = repair_entity_records(root).unwrap();
    assert!(report.recorded.is_empty(), "{report:?}");
    assert!(matches!(
        verdict(&report, "back_again"),
        DeleteVerdict::Leave(_)
    ));
    assert!(matches!(
        verdict(&report, "broken"),
        DeleteVerdict::Leave(_)
    ));
    assert_eq!(record(root), None);
}

#[test]
fn an_entry_already_in_the_record_is_left_byte_for_byte() {
    let journal = TempDir::new();
    let root = journal.path();
    fs::create_dir_all(root.join("entities")).unwrap();
    let bytes =
        br#"{"ids": {"odd": {"state": "merged", "dir": "odd"}, "other": {"state": "someday"}}}"#;
    fs::write(root.join("entities/retired.json"), bytes).unwrap();
    for id in ["odd", "other"] {
        action(
            root,
            "20260123",
            T1,
            json!({"entity_id": id, "facets_deleted": []}),
        );
    }
    let report = repair_entity_records(root).unwrap();
    assert!(report.recorded.is_empty());
    assert_eq!(fs::read(root.join("entities/retired.json")).unwrap(), bytes);
}

#[test]
fn a_merge_newer_than_the_delete_stands() {
    let journal = TempDir::new();
    let root = journal.path();
    let confirmed = chrono::DateTime::parse_from_rfc3339(T2)
        .unwrap()
        .timestamp_millis();
    // Confirmed, then merged inside the hold, then an outcome row saying
    // committed: the merge is newer than the delete.
    action(
        root,
        "20260501",
        T2,
        json!({"entity_id": "held", "pending_id": "p1", "phase": "pending"}),
    );
    merge(root, "held", "survivor", confirmed + 10_000);
    action(
        root,
        "20260501",
        T3,
        json!({"entity_id": "held", "pending_id": "p1", "phase": "committed"}),
    );
    // Merged long before a later delete of a re-created entity.
    merge(root, "earlier", "survivor", confirmed - 86_400_000);
    action(
        root,
        "20260501",
        T2,
        json!({"entity_id": "earlier", "pending_id": "p2", "phase": "pending"}),
    );
    action(
        root,
        "20260501",
        T3,
        json!({"entity_id": "earlier", "pending_id": "p2", "phase": "committed"}),
    );
    // Merged, with a committed row whose confirmation row is gone.
    merge(root, "orphan_row", "survivor", confirmed - 86_400_000);
    action(
        root,
        "20260501",
        T3,
        json!({"entity_id": "orphan_row", "pending_id": "p3", "phase": "committed"}),
    );

    let report = repair_entity_records(root).unwrap();
    assert!(matches!(verdict(&report, "held"), DeleteVerdict::Leave(_)));
    assert_eq!(verdict(&report, "earlier"), DeleteVerdict::Record);
    assert!(matches!(
        verdict(&report, "orphan_row"),
        DeleteVerdict::Leave(_)
    ));
    assert_eq!(report.recorded, ["earlier"]);
    assert_eq!(
        solstone_core_entity::retired_state(root, "held").unwrap(),
        Some(solstone_core_entity::RetiredState::Merged {
            successor: "survivor".to_owned()
        })
    );
}

#[test]
fn an_input_that_cannot_be_read_stops_the_repair_before_it_writes() {
    let journal = TempDir::new();
    let root = journal.path();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "gone", "facets_deleted": []}),
    );
    // The merge log's path holds a folder, so it can't be read as a log.
    fs::create_dir_all(root.join("logs/entity-merges.jsonl")).unwrap();
    assert!(matches!(
        check_entity_records(root),
        Err(EntityDoctorError::Unreadable(_))
    ));
    assert!(repair_entity_records(root).is_err());
    assert_eq!(record(root), None);

    let other = TempDir::new();
    let root = other.path();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "gone", "facets_deleted": []}),
    );
    fs::write(
        root.join("config/actions/20260124.jsonl"),
        [0xff, 0xfe, 0x00],
    )
    .unwrap();
    let report = check_entity_records(root).unwrap();
    assert_eq!(report.unreadable_days, ["20260124"]);
    assert_eq!(
        report.window,
        Some(("20260123".to_owned(), "20260123".to_owned()))
    );
    assert!(matches!(
        repair_entity_records(root),
        Err(EntityDoctorError::Refused(_))
    ));
    assert_eq!(record(root), None);
}

#[test]
fn a_damaged_record_is_reported_and_never_written() {
    let journal = TempDir::new();
    let root = journal.path();
    fs::create_dir_all(root.join("entities")).unwrap();
    fs::write(root.join("entities/retired.json"), "{broken").unwrap();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "gone", "facets_deleted": []}),
    );
    let report = check_entity_records(root).unwrap();
    assert!(matches!(verdict(&report, "gone"), DeleteVerdict::Leave(_)));
    assert!(repair_entity_records(root).is_err());
    assert_eq!(
        fs::read_to_string(root.join("entities/retired.json")).unwrap(),
        "{broken"
    );
}

#[test]
fn a_merged_id_that_is_live_again_is_reported() {
    let journal = TempDir::new();
    let root = journal.path();
    live(root, "jerry");
    merge(root, "jerry", "jeremy", 1);
    let report = check_entity_records(root).unwrap();
    assert_eq!(report.live_again_merged, ["jerry"]);
    assert!(report.findings.is_empty());
}

#[test]
fn an_entity_whose_file_is_damaged_or_unreadable_is_never_recorded() {
    let journal = TempDir::new();
    let root = journal.path();
    // alice lives in another folder, and her file doesn't parse.
    fs::create_dir_all(root.join("entities/alice_dir")).unwrap();
    fs::write(
        root.join("entities/alice_dir/entity.json"),
        "{\"id\": \"alice\"",
    )
    .unwrap();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "alice", "facets_deleted": []}),
    );
    let report = repair_entity_records(root).unwrap();
    assert!(matches!(verdict(&report, "alice"), DeleteVerdict::Leave(_)));
    assert!(report.recorded.is_empty());

    // An identity file that can't be read at all makes every answer unknown.
    let other = TempDir::new();
    let root = other.path();
    fs::create_dir_all(root.join("entities/carol_dir/entity.json")).unwrap();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "carol", "facets_deleted": []}),
    );
    let report = check_entity_records(root).unwrap();
    assert_eq!(report.unreadable_entities, ["carol_dir"]);
    assert!(matches!(verdict(&report, "carol"), DeleteVerdict::Leave(_)));
    assert!(matches!(
        repair_entity_records(root),
        Err(EntityDoctorError::Refused(_))
    ));
    assert_eq!(record(root), None);
}

#[test]
fn a_delete_whose_time_cannot_be_read_is_left() {
    let journal = TempDir::new();
    let root = journal.path();
    action(
        root,
        "20260123",
        "yesterday-ish",
        json!({"entity_id": "vague", "facets_deleted": []}),
    );
    let report = repair_entity_records(root).unwrap();
    assert!(matches!(verdict(&report, "vague"), DeleteVerdict::Leave(_)));
    assert_eq!(record(root), None);
}

#[test]
fn a_merge_line_that_cannot_be_read_counts_as_a_merge_of_its_id() {
    let journal = TempDir::new();
    let root = journal.path();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "bob", "facets_deleted": []}),
    );
    fs::create_dir_all(root.join("logs")).unwrap();
    fs::write(
        root.join("logs/entity-merges.jsonl"),
        "{\"ts\": 1900000000000, \"source_id\": \"bob\", \"target_id\": null}\n",
    )
    .unwrap();
    let report = repair_entity_records(root).unwrap();
    assert_eq!(report.malformed_merge_lines, 1);
    assert!(matches!(verdict(&report, "bob"), DeleteVerdict::Leave(_)));
    assert!(report.recorded.is_empty());
}

#[test]
fn a_counted_row_without_an_id_takes_its_confirmation_rows_id() {
    let journal = TempDir::new();
    let root = journal.path();
    action(
        root,
        "20260501",
        T2,
        json!({"entity_id": "quiet", "pending_id": "p1", "phase": "pending"}),
    );
    action(
        root,
        "20260501",
        T3,
        json!({"pending_id": "p1", "phase": "committed"}),
    );
    let report = repair_entity_records(root).unwrap();
    assert_eq!(report.recorded, ["quiet"]);
}

#[test]
fn a_damaged_identity_file_that_names_no_id_makes_every_answer_unknown() {
    let journal = TempDir::new();
    let root = journal.path();
    fs::create_dir_all(root.join("entities/alice_dir")).unwrap();
    fs::write(root.join("entities/alice_dir/entity.json"), [0u8; 8]).unwrap();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "alice", "facets_deleted": []}),
    );
    let report = check_entity_records(root).unwrap();
    assert_eq!(report.unreadable_entities, ["alice_dir"]);
    assert!(matches!(
        repair_entity_records(root),
        Err(EntityDoctorError::Refused(_))
    ));
    assert_eq!(record(root), None);
}

#[test]
fn a_set_aside_identity_file_still_names_its_entity() {
    let journal = TempDir::new();
    let root = journal.path();
    fs::create_dir_all(root.join("entities/alice_dir")).unwrap();
    fs::write(
        root.join("entities/alice_dir/entity.wedged-1700000000.json"),
        "{\"id\": \"alice\"",
    )
    .unwrap();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "alice", "facets_deleted": []}),
    );
    let report = repair_entity_records(root).unwrap();
    assert!(matches!(verdict(&report, "alice"), DeleteVerdict::Leave(_)));
    assert!(report.recorded.is_empty());
}

#[cfg(unix)]
#[test]
fn a_special_file_in_an_entity_folder_is_unreadable_not_read() {
    let journal = TempDir::new();
    let root = journal.path();
    fs::create_dir_all(root.join("entities/x_dir")).unwrap();
    let fifo = root.join("entities/x_dir/entity.json.pipe");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "gone", "facets_deleted": []}),
    );
    let report = check_entity_records(root).unwrap();
    assert_eq!(report.unreadable_entities, ["x_dir"]);
}

#[test]
fn a_merge_line_cut_before_its_source_stops_the_repair() {
    let journal = TempDir::new();
    let root = journal.path();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "bob", "facets_deleted": []}),
    );
    fs::create_dir_all(root.join("logs")).unwrap();
    fs::write(
        root.join("logs/entity-merges.jsonl"),
        "{\"ts\": 1, \"merge_id\": \"m\", \"source_id\": \"bo",
    )
    .unwrap();
    let report = check_entity_records(root).unwrap();
    assert!(report.merge_log_incomplete);
    assert!(matches!(verdict(&report, "bob"), DeleteVerdict::Leave(_)));
    assert!(matches!(
        repair_entity_records(root),
        Err(EntityDoctorError::Refused(_))
    ));
    assert_eq!(record(root), None);
}

#[test]
fn a_damaged_merge_line_holds_back_only_its_source() {
    let journal = TempDir::new();
    let root = journal.path();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "carol", "facets_deleted": []}),
    );
    fs::create_dir_all(root.join("logs")).unwrap();
    fs::write(
        root.join("logs/entity-merges.jsonl"),
        "{\"ts\": 1, \"source_id\": \"dave\", \"target_id\": \"carol\"\n",
    )
    .unwrap();
    let report = repair_entity_records(root).unwrap();
    assert_eq!(report.recorded, ["carol"]);
}

#[test]
fn a_torn_merge_line_joined_to_the_next_stops_the_repair() {
    let journal = TempDir::new();
    let root = journal.path();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "alice", "facets_deleted": []}),
    );
    fs::create_dir_all(root.join("logs")).unwrap();
    // A crash tore the first append mid-value and the next one landed on the
    // same line; the second, whole source can't vouch for the torn one.
    fs::write(
        root.join("logs/entity-merges.jsonl"),
        "{\"ts\":1900000000000,\"merge_id\":\"m1\",\"source_id\":\"ali{\"ts\":1,\"merge_id\":\"m2\",\"source_id\":\"zed\",\"target_id\":\"t\"}\n",
    )
    .unwrap();
    let report = check_entity_records(root).unwrap();
    assert!(report.merge_log_incomplete);
    assert!(matches!(verdict(&report, "alice"), DeleteVerdict::Leave(_)));
    assert!(matches!(
        repair_entity_records(root),
        Err(EntityDoctorError::Refused(_))
    ));
    assert_eq!(record(root), None);
}

#[test]
fn a_merge_line_with_an_empty_source_stops_the_repair() {
    let journal = TempDir::new();
    let root = journal.path();
    action(
        root,
        "20260123",
        T1,
        json!({"entity_id": "alice", "facets_deleted": []}),
    );
    merge(root, "", "t", 1_900_000_000_000);
    let report = check_entity_records(root).unwrap();
    assert!(report.merge_log_incomplete);
    assert!(matches!(
        repair_entity_records(root),
        Err(EntityDoctorError::Refused(_))
    ));
    assert_eq!(record(root), None);
}

#[test]
fn an_identity_file_that_names_no_usable_id_makes_every_answer_unknown() {
    let cases: [(&str, &[u8]); 4] = [
        ("an empty id", b"{\"id\": \"\", \"name\": \"Alice\""),
        ("an id that isn't text", b"{\"id\": 7, \"name\": \"Alice\""),
        ("an id that isn't UTF-8", b"{\"id\": \"al\xffce\", \"name\""),
        // Stricter than the identity census, which reads an empty file as
        // no entity: a file truncated to nothing may have named one.
        ("an empty file", b""),
    ];
    for (case, contents) in cases {
        let journal = TempDir::new();
        let root = journal.path();
        fs::create_dir_all(root.join("entities/alice_dir")).unwrap();
        fs::write(root.join("entities/alice_dir/entity.json"), contents).unwrap();
        action(
            root,
            "20260123",
            T1,
            json!({"entity_id": "alice", "facets_deleted": []}),
        );
        let report = check_entity_records(root).unwrap();
        assert_eq!(report.unreadable_entities, ["alice_dir"], "{case}");
        assert!(
            matches!(
                repair_entity_records(root),
                Err(EntityDoctorError::Refused(_))
            ),
            "{case}"
        );
        assert_eq!(record(root), None, "{case}");
    }
}

#[test]
fn an_unfinished_delete_says_when_its_presence_cannot_be_told() {
    let journal = TempDir::new();
    let root = journal.path();
    action(
        root,
        "20260501",
        T2,
        json!({"entity_id": "stays", "pending_id": "p1", "phase": "pending"}),
    );
    live(root, "stays");
    action(
        root,
        "20260501",
        T2,
        json!({"entity_id": "went", "pending_id": "p2", "phase": "pending"}),
    );
    let report = check_entity_records(root).unwrap();
    let presence = |report: &crate::entity_doctor::EntityDoctorReport, id: &str| {
        report
            .unfinished
            .iter()
            .find(|unfinished| unfinished.entity_id.as_deref() == Some(id))
            .unwrap()
            .still_here
    };
    assert_eq!(presence(&report, "stays"), Some(true));
    assert_eq!(presence(&report, "went"), Some(false));

    fs::create_dir_all(root.join("entities/odd_dir")).unwrap();
    fs::write(root.join("entities/odd_dir/entity.json"), b"{\"id\": ").unwrap();
    let report = check_entity_records(root).unwrap();
    assert_eq!(presence(&report, "stays"), Some(true));
    assert_eq!(presence(&report, "went"), None);
}
