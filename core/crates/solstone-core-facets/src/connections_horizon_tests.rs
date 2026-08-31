// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::store_tests::{TempDir, create_test_facet};
use crate::{list_facet_directories, refresh_connections_horizon};

const CACHE_REL: &str = "facets/.connections-horizon-cache.json";
const QUALIFYING: &str = "{\"name\":\"Ada\",\"segments\":[\"seg-1\"]}\n";

fn write_detected(root: &Path, facet: &str, day: &str, body: &str) {
    let path = root.join(format!("facets/{facet}/entities/{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn write_chronicle(root: &Path, day: &str) {
    fs::create_dir_all(root.join("chronicle").join(day)).unwrap();
}

fn append_detected(root: &Path, facet: &str, day: &str, line: &str) {
    let path = root.join(format!("facets/{facet}/entities/{day}.jsonl"));
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(line.as_bytes()).unwrap();
}

fn cache_value(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join(CACHE_REL)).unwrap()).unwrap()
}

fn write_cache_value(root: &Path, value: &Value) {
    fs::write(
        root.join(CACHE_REL),
        serde_json::to_vec_pretty(value).unwrap(),
    )
    .unwrap();
}

fn journal_with_facet() -> TempDir {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    temporary
}

#[cfg(unix)]
fn skip_if_unreadable_still_opens(path: &Path) -> bool {
    match fs::File::open(path) {
        Ok(_) => {
            eprintln!(
                "skipping: chmod 0o000 did not make {} unreadable (likely running as root)",
                path.display()
            );
            true
        }
        Err(_) => false,
    }
}

#[cfg(unix)]
struct RestoreMode {
    path: PathBuf,
    mode: u32,
}

#[cfg(unix)]
impl Drop for RestoreMode {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(self.mode));
    }
}

#[test]
fn horizon_is_reported_when_chronicle_starts_months_before_store_day() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260115");
    write_chronicle(root, "20260215");
    write_chronicle(root, "20260315");
    write_detected(root, "work", "20260615", QUALIFYING);
    let horizon = refresh_connections_horizon(root).expect("horizon");
    assert_eq!(horizon.day, "20260615");
    assert_eq!(horizon.earlier_days, 3);
}

#[test]
fn no_horizon_when_store_day_equals_earliest_chronicle_day() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260615");
    write_chronicle(root, "20260620");
    write_detected(root, "work", "20260615", QUALIFYING);
    assert_eq!(refresh_connections_horizon(root), None);
}

#[test]
fn horizon_ignores_non_detected_store_sources() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260601", QUALIFYING);
    let events = root.join("facets/work/events/20240101.jsonl");
    fs::create_dir_all(events.parent().unwrap()).unwrap();
    fs::write(&events, QUALIFYING).unwrap();
    let activities = root.join("facets/work/activities/20240201.jsonl");
    fs::create_dir_all(activities.parent().unwrap()).unwrap();
    fs::write(&activities, QUALIFYING).unwrap();
    let horizon = refresh_connections_horizon(root).expect("horizon");
    assert_eq!(horizon.day, "20260601");
}

#[test]
fn empty_or_non_string_segments_do_not_qualify() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(
        root,
        "work",
        "20260201",
        concat!(
            "{\"name\":\"Ada\",\"segments\":[]}\n",
            "{\"name\":\"Ada\",\"segments\":[\"\"]}\n",
            "{\"name\":\"Ada\",\"segments\":[\"  \"]}\n",
            "{\"name\":\"Ada\",\"segments\":[{\"segment\":\"a\"}]}\n",
            "{\"name\":\"Ada\",\"segments\":\"s1\"}\n",
        ),
    );
    write_detected(root, "work", "20260301", QUALIFYING);
    let horizon = refresh_connections_horizon(root).expect("horizon");
    assert_eq!(horizon.day, "20260301");
}

#[test]
fn missing_non_string_or_blank_name_does_not_qualify() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(
        root,
        "work",
        "20260201",
        concat!(
            "{\"segments\":[\"s1\"]}\n",
            "{\"name\":1,\"segments\":[\"s1\"]}\n",
            "{\"name\":\"\",\"segments\":[\"s1\"]}\n",
            "{\"name\":\"   \",\"segments\":[\"s1\"]}\n",
        ),
    );
    write_detected(root, "work", "20260301", QUALIFYING);
    let horizon = refresh_connections_horizon(root).expect("horizon");
    assert_eq!(horizon.day, "20260301");
}

#[test]
fn missing_or_short_type_still_qualifies() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(
        root,
        "work",
        "20260201",
        "{\"name\":\"Ada\",\"segments\":[\"s1\"]}\n",
    );
    assert_eq!(
        refresh_connections_horizon(root).expect("horizon").day,
        "20260201"
    );
    fs::remove_file(root.join(CACHE_REL)).ok();
    write_detected(
        root,
        "work",
        "20260115",
        "{\"name\":\"Ada\",\"type\":\"ab\",\"segments\":[\"s1\"]}\n",
    );
    assert_eq!(
        refresh_connections_horizon(root).expect("horizon").day,
        "20260115"
    );
}

#[test]
fn unparseable_jsonl_line_does_not_qualify_its_day() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(
        root,
        "work",
        "20260201",
        "not-json\n{\"name\":\"Ada\",\"segments\":[\"s1\"]\n",
    );
    write_detected(root, "work", "20260301", QUALIFYING);
    let horizon = refresh_connections_horizon(root).expect("horizon");
    assert_eq!(horizon.day, "20260301");
}

#[test]
fn earlier_days_counts_empty_chronicle_and_ignores_future() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_chronicle(root, "20261231");
    write_detected(root, "work", "20260301", QUALIFYING);
    let horizon = refresh_connections_horizon(root).expect("n==1");
    assert_eq!(horizon.day, "20260301");
    assert_eq!(horizon.earlier_days, 1);

    fs::remove_file(root.join(CACHE_REL)).ok();
    write_chronicle(root, "20260201");
    let horizon = refresh_connections_horizon(root).expect("n>=2");
    assert_eq!(horizon.earlier_days, 2);
}

#[test]
fn failure_paths_return_no_horizon() {
    let missing = TempDir::new();
    assert_eq!(refresh_connections_horizon(missing.path()), None);

    let temporary = journal_with_facet();
    let root = temporary.path();
    write_detected(root, "work", "20260301", QUALIFYING);
    assert_eq!(refresh_connections_horizon(root), None);
}

#[cfg(unix)]
#[test]
fn unreadable_day_file_returns_no_horizon() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    let path = root.join("facets/work/entities/20260301.jsonl");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if skip_if_unreadable_still_opens(&path) {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    assert_eq!(refresh_connections_horizon(root), None);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn blank_nonobject_malformed_lines_are_skipped() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(
        root,
        "work",
        "20260301",
        "\n\n[]\n{\n{\"name\":\"Ada\",\"segments\":[\"s1\"]}\n",
    );
    assert_eq!(
        refresh_connections_horizon(root).expect("horizon").day,
        "20260301"
    );
}

#[test]
fn same_day_in_two_facets_is_one_horizon_day() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    create_test_facet(root, "personal");
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", "{\"name\":\"Ada\"}\n");
    write_detected(root, "personal", "20260301", QUALIFYING);
    let horizon = refresh_connections_horizon(root).expect("horizon");
    assert_eq!(horizon.day, "20260301");
}

#[test]
fn non_day_keyed_store_file_is_ignored() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "foo", QUALIFYING);
    write_detected(root, "work", "20260301", QUALIFYING);
    assert_eq!(
        refresh_connections_horizon(root).expect("horizon").day,
        "20260301"
    );
}

#[test]
fn second_call_returns_cached_horizon() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    let first = refresh_connections_horizon(root).expect("first");
    let second = refresh_connections_horizon(root).expect("second");
    assert_eq!(first, second);
}

#[test]
fn matching_cache_with_wrong_horizon_is_consulted() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    assert_eq!(
        refresh_connections_horizon(root).expect("seed").day,
        "20260301"
    );
    let mut cached = cache_value(root);
    cached["horizon_day"] = json!("20991231");
    write_cache_value(root, &cached);
    let poisoned = refresh_connections_horizon(root).expect("cache hit");
    assert_eq!(poisoned.day, "20991231");
}

#[test]
fn new_earlier_day_file_lowers_d() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    assert_eq!(
        refresh_connections_horizon(root).expect("first").day,
        "20260301"
    );
    write_detected(root, "work", "20260201", QUALIFYING);
    assert_eq!(
        refresh_connections_horizon(root).expect("lowered").day,
        "20260201"
    );
}

#[test]
fn same_second_append_invalidates() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    write_detected(root, "work", "20260201", "{\"name\":\"Ada\"}\n");
    assert_eq!(
        refresh_connections_horizon(root).expect("first").day,
        "20260301"
    );
    append_detected(root, "work", "20260201", QUALIFYING);
    let stamp = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for day in ["20260201", "20260301"] {
        let file = fs::File::options()
            .write(true)
            .open(root.join(format!("facets/work/entities/{day}.jsonl")))
            .unwrap();
        file.set_modified(stamp).unwrap();
    }
    assert_eq!(
        refresh_connections_horizon(root).expect("appended").day,
        "20260201"
    );
}

#[cfg(unix)]
#[test]
fn later_day_file_is_outside_fingerprint_set() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260115", QUALIFYING);
    let first = refresh_connections_horizon(root).expect("first");
    assert_eq!(first.day, "20260115");
    let original = root.join("facets/work/entities/20260115.jsonl");
    write_detected(root, "work", "20260601", QUALIFYING);
    fs::set_permissions(&original, fs::Permissions::from_mode(0o000)).unwrap();
    if skip_if_unreadable_still_opens(&original) {
        fs::set_permissions(&original, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    let second = refresh_connections_horizon(root);
    fs::set_permissions(&original, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(second.expect("still cached").day, "20260115");
}

#[test]
fn backdated_mtime_same_size_still_invalidates() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    assert_eq!(
        refresh_connections_horizon(root).expect("first").day,
        "20260301"
    );
    let path = root.join("facets/work/entities/20260301.jsonl");
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_secs(1_600_000_000))
        .unwrap();
    drop(file);
    let mut cached = cache_value(root);
    cached["horizon_day"] = json!("20991231");
    write_cache_value(root, &cached);
    assert_eq!(
        refresh_connections_horizon(root).expect("recomputed").day,
        "20260301"
    );
}

#[cfg(unix)]
#[test]
fn cache_hit_does_not_open_fingerprinted_files() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    assert_eq!(
        refresh_connections_horizon(root).expect("first").day,
        "20260301"
    );
    let path = root.join("facets/work/entities/20260301.jsonl");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if skip_if_unreadable_still_opens(&path) {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    let second = refresh_connections_horizon(root);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(second.expect("hit").day, "20260301");
}

#[test]
fn cache_miss_on_missing_truncated_stale_schema() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    assert_eq!(
        refresh_connections_horizon(root).expect("seed").day,
        "20260301"
    );

    fs::remove_file(root.join(CACHE_REL)).unwrap();
    assert_eq!(
        refresh_connections_horizon(root).expect("missing").day,
        "20260301"
    );

    fs::write(root.join(CACHE_REL), "{").unwrap();
    assert_eq!(
        refresh_connections_horizon(root).expect("truncated").day,
        "20260301"
    );

    let mut stale = cache_value(root);
    stale["schema_version"] = json!(0);
    write_cache_value(root, &stale);
    assert_eq!(
        refresh_connections_horizon(root).expect("stale").day,
        "20260301"
    );
}

#[cfg(unix)]
#[test]
fn unreadable_cache_file_rescans() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    assert_eq!(
        refresh_connections_horizon(root).expect("seed").day,
        "20260301"
    );
    let cache = root.join(CACHE_REL);
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o000)).unwrap();
    if skip_if_unreadable_still_opens(&cache) {
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    let second = refresh_connections_horizon(root);
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(second.expect("rescan").day, "20260301");
}

#[cfg(unix)]
#[test]
fn unwritable_facets_dir_still_returns_horizon() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    let facets = root.join("facets");
    let _restore = RestoreMode {
        path: facets.clone(),
        mode: 0o755,
    };
    fs::set_permissions(&facets, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(facets.join(".write-probe"), b"x").is_ok() {
        eprintln!("skipping: chmod 0o555 did not make facets unwritable (likely running as root)");
        return;
    }
    let horizon = refresh_connections_horizon(root);
    assert_eq!(horizon.expect("computed").day, "20260301");
}

#[cfg(unix)]
#[test]
fn hit_recomputes_earlier_days_from_day_dirs() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_chronicle(root, "20260201");
    write_detected(root, "work", "20260301", QUALIFYING);
    assert_eq!(
        refresh_connections_horizon(root)
            .expect("first")
            .earlier_days,
        2
    );
    let path = root.join("facets/work/entities/20260301.jsonl");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if skip_if_unreadable_still_opens(&path) {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    fs::remove_dir_all(root.join("chronicle/20260201")).unwrap();
    let second = refresh_connections_horizon(root);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(second.expect("recomputed n").earlier_days, 1);
}

#[cfg(unix)]
#[test]
fn hit_promotes_n0_to_some_when_earlier_chronicle_appears() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_detected(root, "work", "20260301", QUALIFYING);
    assert_eq!(refresh_connections_horizon(root), None);
    assert_eq!(cache_value(root)["horizon_day"], "20260301");
    let path = root.join("facets/work/entities/20260301.jsonl");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if skip_if_unreadable_still_opens(&path) {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    write_chronicle(root, "20260101");
    let second = refresh_connections_horizon(root);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let horizon = second.expect("promoted");
    assert_eq!(horizon.day, "20260301");
    assert_eq!(horizon.earlier_days, 1);
}

#[test]
fn absent_horizon_is_cached_and_not_rescanned() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_detected(root, "work", "20260301", "{\"name\":\"Ada\"}\n");
    assert_eq!(refresh_connections_horizon(root), None);
    assert_eq!(cache_value(root)["horizon_day"], Value::Null);
    assert_eq!(refresh_connections_horizon(root), None);
    assert_eq!(cache_value(root)["horizon_day"], Value::Null);
}

#[test]
fn cache_file_is_not_listed_as_a_facet() {
    let temporary = journal_with_facet();
    let root = temporary.path();
    write_chronicle(root, "20260101");
    write_detected(root, "work", "20260301", QUALIFYING);
    refresh_connections_horizon(root).unwrap();
    let directories = list_facet_directories(root).unwrap();
    assert!(
        directories
            .iter()
            .all(|name| name != ".connections-horizon-cache.json")
    );
    assert!(root.join(CACHE_REL).is_file());
}
