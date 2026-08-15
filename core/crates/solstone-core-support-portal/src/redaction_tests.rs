// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use chrono::{Local, TimeZone, Utc};
use serde_json::json;
use tempfile::TempDir;

use super::*;

fn root() -> TempDir {
    TempDir::new().expect("tempdir")
}
fn text(value: &str) -> String {
    bounded_redacted_text(Some(value), 500).expect("text")
}
fn platform() -> PlatformInfo {
    PlatformInfo {
        system: "TestOS".into(),
        release: "test-release".into(),
        machine: "test-machine".into(),
    }
}
fn write(root: &TempDir, path: &str, contents: &str) {
    let path = root.path().join(path);
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(path, contents).expect("write");
}

#[test]
fn redacts_each_reference_shape_and_bounds_characters() {
    let cases = [
        ("MY_TOKEN=abc", "<secret>"),
        ("OPENAI_API_KEY", "<secret>"),
        // `sk-ant-` is consumed by the leftmost `sk-` alternation branch.
        ("sk-ant-x", "<secret>"),
        ("AIzaX", "<secret>"),
        ("/private", "<path>"),
        (r"C:\private", "<path>"),
        (
            "Traceback (most recent call last): frame",
            "traceback redacted frame",
        ),
    ];
    for (raw, expected) in cases {
        assert!(
            raw.contains(if raw.starts_with("sk") { "sk" } else { raw }),
            "raw precondition"
        );
        assert_eq!(text(raw), expected);
    }
    let raw = "x".repeat(600);
    let bounded = bounded_redacted_text(Some(&raw), 500).expect("bounded");
    assert_eq!(bounded.chars().count(), 500);
    assert!(bounded.ends_with('…'));
}

#[test]
fn posix_lookbehind_table_matches_reference() {
    for value in [
        "foo/bar", "_/bar", "9/bar", "é/bar", "中/bar", "٩/bar", "a / b",
    ] {
        assert_eq!(text(value), value);
    }
    assert_eq!(text("//x"), "<path>");
    assert_eq!(text("foo;/bar"), "foo;<path>");
}

#[test]
fn assignment_and_api_key_discriminators_match_reference() {
    assert_eq!(text("AIzaX/foo"), "<secret><path>");
    assert_eq!(text("MY_TOKEN=abc"), "<secret>");
}

#[test]
fn secret_key_substrings_and_non_descending_redaction_match_reference() {
    assert!(is_secret_key("api_key_id"));
    assert!(is_secret_key("TOKEN"));
    assert!(!is_secret_key("colour"));
    let input = json!({"api_key_id":{"nested_token":"must not descend"},"items":[{"TOKEN":"x"}],"colour":"blue"});
    assert_eq!(
        strip_secrets(&input),
        json!({"api_key_id":"***","items":[{"TOKEN":"***"}],"colour":"blue"})
    );
}

#[test]
fn diagnostics_collector_order_and_aware_timestamp_omit_match_reference() {
    let root = root();
    write(
        &root,
        "health/service.log",
        "2026-01-02T03:04:05+00:00 ERROR boom\n",
    );
    let now = Local
        .with_ymd_and_hms(2026, 1, 2, 4, 0, 0)
        .single()
        .expect("now");
    let all = collect_all(root.path(), now, platform());
    assert!(!all.contains_key("recent_errors"));
    assert_eq!(
        all.keys().collect::<Vec<_>>(),
        [
            "version",
            "revision",
            "platform",
            "services",
            "config",
            "brain_health"
        ]
    );
    fs::remove_file(root.path().join("health/service.log")).expect("remove aware log");
    let all = collect_all(root.path(), now, platform());
    assert_eq!(
        all.keys().collect::<Vec<_>>(),
        [
            "version",
            "revision",
            "platform",
            "services",
            "recent_errors",
            "config",
            "brain_health"
        ]
    );
}

#[test]
fn services_are_never_redacted_and_recent_errors_keep_reference_shapes() {
    let root = root();
    write(&root, "health/sk-x.pid", "not-a-pid\n");
    let services = collect_services(root.path());
    assert_eq!(services["sk-x"], "stopped");
    assert_ne!(services["sk-x"], "***");

    write(
        &root,
        "health/svc.log",
        "ERROR sk-x /private\n2026-01-02T03:04:04 ERROR MY_TOKEN=abc /private Traceback (most recent call last): frame\n",
    );
    let now = Local
        .with_ymd_and_hms(2026, 1, 2, 4, 0, 0)
        .single()
        .expect("now");
    let errors = collect_recent_errors(root.path(), now).expect("naive local lines");
    let errors = errors.as_array().expect("array");
    assert_eq!(errors.len(), 2);
    assert!(
        errors[0]["time_approximate"]
            .as_bool()
            .expect("approximate")
    );
    assert!(
        errors[1]["message"]
            .as_str()
            .expect("message")
            .contains("<secret> <path> traceback redacted")
    );
    assert!(!errors[1]["time_approximate"].as_bool().expect("exact"));
    assert_eq!(errors[0]["service"], "svc");
}

#[test]
fn platform_injection_and_brain_live_and_fallback_shapes_are_stable() {
    assert_eq!(
        collect_platform(platform()),
        json!({"system":"TestOS","release":"test-release","machine":"test-machine"})
    );
    let root = root();
    let now = Utc
        .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
        .single()
        .expect("now");
    let non_journal = root.path().join("not-a-journal");
    fs::write(&non_journal, "not a directory").expect("non-journal path");
    let fallback = collect_brain_health(&non_journal, now);
    assert_eq!(
        fallback["snapshot"]["reason_code"],
        "brain_record_unavailable"
    );
    // A real config and missing brain record exercise the live inspector/renderer path.
    write(&root, "config/config.json", "{}");
    let live = collect_brain_health(root.path(), now);
    assert!(live["snapshot"].is_object());
    assert!(
        live["lines"]
            .as_array()
            .expect("lines")
            .first()
            .is_some_and(|line| line == "Brain Health")
    );
}

#[test]
fn service_status_has_running_stopped_and_unknown_outcomes() {
    assert_eq!(service_status(Some("not-a-pid"), Ok(())), "stopped");
    assert_eq!(service_status(Some("1"), Err(())), "unknown");
    assert_eq!(
        service_status(Some("1"), Ok(())),
        if std::path::Path::new("/proc/1").exists() {
            "running"
        } else {
            "stopped"
        }
    );
}
