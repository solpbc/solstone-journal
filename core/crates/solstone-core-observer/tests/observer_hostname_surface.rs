// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::observer_render_support::{NOW_MS, write_record};
use serde_json::{Value, json};
use solstone_core_observer::store::format::{
    TimeDisplay, render_list, render_status_all, render_status_single,
};
use solstone_core_observer::store::record::ObserverRecord;
use solstone_core_observer::store::reload::load_observers;

fn seed(root: &std::path::Path, name: &str, key: &str, hostname: Option<&str>) -> ObserverRecord {
    let mut value = json!({
        "key": key,
        "name": name,
        "created_at": NOW_MS,
        "last_seen": NOW_MS - 30_000,
        "enabled": true,
        "revoked": false,
        "stats": {"segments_received": 1, "bytes_received": 1024}
    });
    if let Some(hostname) = hostname {
        value
            .as_object_mut()
            .expect("object")
            .insert("hostname".to_owned(), Value::from(hostname));
    }
    write_record(root, value)
}

fn record_named<'a>(records: &'a [ObserverRecord], name: &str) -> &'a ObserverRecord {
    records
        .iter()
        .find(|record| record.name() == Some(name))
        .unwrap_or_else(|| panic!("missing record {name}"))
}

fn json_named<'a>(entries: &'a [Value], name: &str) -> &'a Value {
    entries
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("missing json entry {name}"))
}

fn row_named<'a>(output: &'a str, name: &str) -> &'a str {
    let prefix = format!("{name:<20}");
    output
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("missing human row {name}"))
}

fn parse_json(text: &str) -> Value {
    serde_json::from_str(text).expect("json")
}

fn list_json(records: &[ObserverRecord]) -> Value {
    parse_json(&render_list(records, true, NOW_MS, TimeDisplay::Utc))
}

fn status_all_json(records: &[ObserverRecord]) -> Value {
    parse_json(&render_status_all(records, true, NOW_MS, TimeDisplay::Utc))
}

fn status_single_json(root: &std::path::Path, record: &ObserverRecord) -> Value {
    parse_json(&render_status_single(
        root,
        record,
        true,
        NOW_MS,
        TimeDisplay::Utc,
    ))
}

fn assert_json_hostname(entry: &Value, expected: Option<&str>) {
    match expected {
        Some(hostname) => {
            assert_eq!(
                entry.get("hostname").and_then(Value::as_str),
                Some(hostname),
                "{entry}"
            );
        }
        None => {
            assert!(
                entry.get("hostname").is_none(),
                "hostname key must be absent: {entry}"
            );
        }
    }
}

fn table_header_and_rule(output: &str, header_marker: &str) -> (String, String) {
    let mut lines = output.lines();
    let header = lines
        .find(|line| line.contains(header_marker))
        .expect("header")
        .to_owned();
    let rule = lines.next().expect("rule").to_owned();
    (header, rule)
}

#[test]
fn two_distinct_hostname_pairs_surface_on_every_path() {
    let root = tempfile::tempdir().expect("journal");
    seed(
        root.path(),
        "boron",
        "bbbbbbbb111",
        Some("Davids-Mac-Studio.local"),
    );
    seed(root.path(), "office", "aaaaaaaa111", Some("archon.local"));
    let records = load_observers(root.path()).expect("records");

    let list = list_json(&records);
    let list_entries = list.as_array().expect("list array");
    assert_json_hostname(
        json_named(list_entries, "boron"),
        Some("Davids-Mac-Studio.local"),
    );
    assert_json_hostname(json_named(list_entries, "office"), Some("archon.local"));

    let all = status_all_json(&records);
    let observers = all.get("observers").and_then(Value::as_array).expect("obs");
    assert_json_hostname(
        json_named(observers, "boron"),
        Some("Davids-Mac-Studio.local"),
    );
    assert_json_hostname(json_named(observers, "office"), Some("archon.local"));

    assert_json_hostname(
        &status_single_json(root.path(), record_named(&records, "boron")),
        Some("Davids-Mac-Studio.local"),
    );
    assert_json_hostname(
        &status_single_json(root.path(), record_named(&records, "office")),
        Some("archon.local"),
    );

    let list_human = render_list(&records, false, NOW_MS, TimeDisplay::Utc);
    let boron_list = row_named(&list_human, "boron");
    let office_list = row_named(&list_human, "office");
    assert!(
        boron_list.ends_with(" Davids-Mac-Studio.local"),
        "{boron_list}"
    );
    assert!(office_list.ends_with(" archon.local"), "{office_list}");
    assert!(!boron_list.contains("archon.local"), "{boron_list}");
    assert!(
        !office_list.contains("Davids-Mac-Studio.local"),
        "{office_list}"
    );

    let status_human = render_status_all(&records, false, NOW_MS, TimeDisplay::Utc);
    let boron_status = row_named(&status_human, "boron");
    let office_status = row_named(&status_human, "office");
    assert!(
        boron_status.ends_with(" Davids-Mac-Studio.local"),
        "{boron_status}"
    );
    assert!(office_status.ends_with(" archon.local"), "{office_status}");
    assert!(!boron_status.contains("archon.local"), "{boron_status}");
    assert!(
        !office_status.contains("Davids-Mac-Studio.local"),
        "{office_status}"
    );

    let boron_single = render_status_single(
        root.path(),
        record_named(&records, "boron"),
        false,
        NOW_MS,
        TimeDisplay::Utc,
    );
    let office_single = render_status_single(
        root.path(),
        record_named(&records, "office"),
        false,
        NOW_MS,
        TimeDisplay::Utc,
    );
    assert!(
        boron_single.contains("  Hostname:     Davids-Mac-Studio.local"),
        "{boron_single}"
    );
    assert!(
        office_single.contains("  Hostname:     archon.local"),
        "{office_single}"
    );
    assert!(!boron_single.contains("archon.local"), "{boron_single}");
    assert!(
        !office_single.contains("Davids-Mac-Studio.local"),
        "{office_single}"
    );
}

#[test]
fn mixed_registry_grows_column_and_omits_absent_key() {
    let root = tempfile::tempdir().expect("journal");
    seed(
        root.path(),
        "boron",
        "bbbbbbbb111",
        Some("Davids-Mac-Studio.local"),
    );
    seed(root.path(), "office", "aaaaaaaa111", None);
    let records = load_observers(root.path()).expect("records");
    let office = record_named(&records, "office");

    let list = list_json(&records);
    let list_entries = list.as_array().expect("list array");
    assert_json_hostname(
        json_named(list_entries, "boron"),
        Some("Davids-Mac-Studio.local"),
    );
    assert_json_hostname(json_named(list_entries, "office"), None);

    let all = status_all_json(&records);
    let observers = all.get("observers").and_then(Value::as_array).expect("obs");
    assert_json_hostname(
        json_named(observers, "boron"),
        Some("Davids-Mac-Studio.local"),
    );
    assert_json_hostname(json_named(observers, "office"), None);

    assert_json_hostname(&status_single_json(root.path(), office), None);

    let list_human = render_list(&records, false, NOW_MS, TimeDisplay::Utc);
    let (list_header, list_rule) = table_header_and_rule(&list_human, "Name");
    assert!(
        list_header.ends_with(" Hostname"),
        "list header should grow Hostname column: {list_header}"
    );
    assert_eq!(list_rule.len(), 118 + 1 + "Davids-Mac-Studio.local".len());
    let office_list = row_named(&list_human, "office");
    assert!(
        !office_list.contains("Davids-Mac-Studio.local"),
        "{office_list}"
    );
    let solo_list = render_list(
        std::slice::from_ref(office),
        false,
        NOW_MS,
        TimeDisplay::Utc,
    );
    assert_eq!(
        office_list,
        row_named(&solo_list, "office"),
        "hostname-less list row must not grow a cell"
    );

    let status_human = render_status_all(&records, false, NOW_MS, TimeDisplay::Utc);
    let (status_header, status_rule) = table_header_and_rule(&status_human, "Name");
    assert!(
        status_header.ends_with(" Hostname"),
        "status-all header should grow Hostname column: {status_header}"
    );
    assert_eq!(status_rule.len(), 98 + 1 + "Davids-Mac-Studio.local".len());
    let office_status = row_named(&status_human, "office");
    assert!(
        !office_status.contains("Davids-Mac-Studio.local"),
        "{office_status}"
    );
    let solo_status = render_status_all(
        std::slice::from_ref(office),
        false,
        NOW_MS,
        TimeDisplay::Utc,
    );
    assert_eq!(
        office_status,
        row_named(&solo_status, "office"),
        "hostname-less status-all row must not grow a cell"
    );

    let office_single = render_status_single(root.path(), office, false, NOW_MS, TimeDisplay::Utc);
    assert!(!office_single.contains("Hostname:"), "{office_single}");
}

#[test]
fn blank_and_whitespace_hostnames_are_absent_on_every_surface() {
    let root = tempfile::tempdir().expect("journal");
    seed(root.path(), "empty", "cccccccc111", Some(""));
    seed(root.path(), "spaces", "dddddddd111", Some("   "));
    let records = load_observers(root.path()).expect("records");
    assert_eq!(records.len(), 2);

    let list = list_json(&records);
    for entry in list.as_array().expect("list array") {
        assert_json_hostname(entry, None);
    }
    let all = status_all_json(&records);
    for entry in all.get("observers").and_then(Value::as_array).expect("obs") {
        assert_json_hostname(entry, None);
    }
    for record in &records {
        assert_json_hostname(&status_single_json(root.path(), record), None);
        let single = render_status_single(root.path(), record, false, NOW_MS, TimeDisplay::Utc);
        assert!(!single.contains("Hostname:"), "{single}");
    }

    let list_human = render_list(&records, false, NOW_MS, TimeDisplay::Utc);
    let (list_header, list_rule) = table_header_and_rule(&list_human, "Name");
    assert!(
        !list_header.contains("Hostname"),
        "blank hostnames must not grow the list column: {list_header}"
    );
    assert_eq!(list_rule, "-".repeat(118));

    let status_human = render_status_all(&records, false, NOW_MS, TimeDisplay::Utc);
    let (status_header, status_rule) = table_header_and_rule(&status_human, "Name");
    assert!(
        !status_header.contains("Hostname"),
        "blank hostnames must not grow the status-all column: {status_header}"
    );
    assert_eq!(status_rule, "-".repeat(98));
}
