// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;

use chrono::{Local, TimeZone, Utc};
use serde_json::json;
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, create_oplog_at},
};
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

fn write_oplog(root: &TempDir, source: &str, at: chrono::DateTime<Local>, contents: &[u8]) {
    let mut writer = create_oplog_at(
        JournalRoot::open(root.path()).expect("journal root"),
        source,
        "support-test",
        OplogFormat::Log,
        at.fixed_offset(),
    )
    .expect("canonical oplog");
    writer.write_all(contents).expect("payload");
}

#[test]
fn redacts_each_reference_shape_and_bounds_characters() {
    let cases = [
        ("MY_TOKEN=abc", "abc", "<secret>"),
        ("OPENAI_API_KEY", "OPENAI_API_KEY", "<secret>"),
        ("sk-x", "sk-x", "<secret>"),
        ("AIzaX", "AIzaX", "<secret>"),
        ("/private", "/private", "<path>"),
        (r"C:\private", r"C:\private", "<path>"),
        (
            "Traceback (most recent call last): frame",
            "Traceback (most recent call last):",
            "traceback redacted frame",
        ),
    ];
    for (raw, sensitive, expected) in cases {
        let redacted = text(raw);
        assert!(raw.contains(sensitive), "raw input omitted {sensitive:?}");
        assert!(
            !redacted.contains(sensitive),
            "redaction leaked {sensitive:?}"
        );
        assert_eq!(redacted, expected);
    }
    let raw = format!("sk-x {}", "x".repeat(595));
    assert_eq!(raw.chars().count(), 600);
    let bounded = bounded_redacted_text(Some(&raw), 500).expect("bounded");
    assert!(raw.contains("sk-x"));
    assert!(!bounded.contains("sk-x"));
    assert_eq!(bounded.chars().count(), 500);
    assert!(bounded.ends_with('…'));
}

#[test]
fn sk_ant_is_redacted_by_the_leftmost_sk_branch() {
    // `sk-ant-` is consumed by the leftmost `sk-` alternation branch.
    assert_eq!(text("sk-ant-x"), "<secret>");
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
    let now = Local
        .with_ymd_and_hms(2026, 1, 2, 4, 0, 0)
        .single()
        .expect("now");
    write_oplog(
        &root,
        "service",
        now,
        b"2026-01-02T03:04:05+00:00 ERROR boom\n",
    );
    assert_eq!(
        collect_recent_errors(root.path(), now).unwrap_err().kind(),
        "oplog_catalog_timestamp"
    );
    let all = collect_all(root.path(), now, platform());
    assert!(!all.contains_key("recent_errors"));
    assert_eq!(
        all.keys().collect::<Vec<_>>(),
        [
            "version",
            "revision",
            "platform",
            "services",
            "log_collection_error",
            "config",
            "brain_health"
        ]
    );
    let fresh = TempDir::new().expect("empty journal");
    let all = collect_all(fresh.path(), now, platform());
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
fn services_and_recent_errors_keep_reference_shapes_and_ordering() {
    let root = root();
    write(&root, "health/sk-x.pid", "1\n");
    write(&root, "health/stopped.pid", "2\n");
    write(&root, "health/unknown.pid", "3\n");
    write(&root, "health/invalid.pid", "not-a-pid\n");
    let public_services = collect_services(root.path());
    assert!(matches!(
        public_services["sk-x"].as_str(),
        Some("running" | "stopped" | "unknown")
    ));
    assert_ne!(public_services["sk-x"], "***");
    let services = collect_services_with_probe(root.path(), |pid| match pid {
        1 => ServiceProbeStatus::Running,
        2 => ServiceProbeStatus::Stopped,
        _ => ServiceProbeStatus::Unknown,
    });
    assert_eq!(services["sk-x"], "running");
    assert_eq!(services["stopped"], "stopped");
    assert_eq!(services["unknown"], "unknown");
    assert_eq!(services["invalid"], "stopped");

    let seed = Local::now();
    let mut lines = vec!["unparseable ERROR sk-x /private".to_owned()];
    for offset in 1..=11 {
        let time = seed - chrono::Duration::seconds(offset);
        lines.push(format!(
            "   {} ERROR MY_TOKEN=abc /private Traceback (most recent call last): entry-{offset}",
            time.format("%Y-%m-%dT%H:%M:%S")
        ));
    }
    write_oplog(&root, "svc", seed, lines.join("\n").as_bytes());
    let errors = collect_recent_errors(root.path(), Local::now() + chrono::Duration::minutes(2))
        .expect("naive local lines");
    let errors = errors.as_array().expect("array");
    assert_eq!(errors.len(), 10);
    assert!(
        errors[0]["time_approximate"]
            .as_bool()
            .expect("approximate")
    );
    assert_eq!(errors[0]["message"], "unparseable ERROR <secret> <path>");
    assert!(
        errors[1]["message"]
            .as_str()
            .expect("message")
            .contains("<secret> <path> traceback redacted")
    );
    assert!(!errors[1]["time_approximate"].as_bool().expect("exact"));
    assert_eq!(errors[0]["service"], "svc");
    assert!(
        errors[9]["message"]
            .as_str()
            .expect("message")
            .contains("entry-9")
    );
    for pair in errors.windows(2) {
        assert!(pair[0]["time"].as_str().expect("time") >= pair[1]["time"].as_str().expect("time"));
    }
}

#[test]
fn recent_errors_decodes_lossily_and_accepts_python_naive_iso_forms() {
    let root = root();
    let now = Local::now();
    let spaced = (now - chrono::Duration::seconds(2)).format("%Y-%m-%d %H:%M:%S");
    let basic = (now - chrono::Duration::seconds(3)).format("%Y%m%dT%H%M%S");
    let mut bytes = format!("{spaced} ERROR bad-utf8: ").into_bytes();
    bytes.push(0xff);
    bytes.extend_from_slice(format!("\n{basic} ERROR basic-naive\n").as_bytes());
    write_oplog(&root, "lossy", now, &bytes);
    let errors = collect_recent_errors(root.path(), now + chrono::Duration::minutes(1))
        .expect("naive ISO forms");
    let errors = errors.as_array().expect("array");
    assert_eq!(errors.len(), 2);
    assert!(errors.iter().any(|error| {
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("�"))
    }));
    assert!(errors.iter().any(|error| {
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("basic-naive"))
    }));
}

#[test]
fn canonical_multiday_errors_are_redacted_and_catalog_failure_keeps_the_bundle_usable() {
    let root = root();
    let now = Local
        .with_ymd_and_hms(2026, 8, 8, 12, 0, 0)
        .single()
        .expect("now");
    let prior = now - chrono::Duration::days(1);
    let outside = now - chrono::Duration::hours(169);
    write_oplog(
        &root,
        "prior-source",
        prior,
        format!(
            "{} ERROR MY_TOKEN=abc /private\n",
            prior.format("%Y-%m-%dT%H:%M:%S")
        )
        .as_bytes(),
    );
    write_oplog(
        &root,
        "outside-source",
        outside,
        format!("{} ERROR old\n", outside.format("%Y-%m-%dT%H:%M:%S")).as_bytes(),
    );
    let errors = collect_recent_errors(root.path(), now).expect("complete catalog");
    let errors = errors.as_array().expect("array");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["service"], "prior-source");
    assert_eq!(errors[0]["message"], "ERROR <secret> <path>");
    assert!(!errors[0].to_string().contains("oplog--"));

    let health = root.path().join("chronicle/20260808/health");
    fs::create_dir_all(&health).expect("health");
    fs::write(health.join("oplog--broken.log"), b"broken").expect("malformed leaf");
    let all = collect_all(root.path(), now, platform());
    assert_eq!(
        all["log_collection_error"]["kind"],
        "oplog_catalog_malformed"
    );
    assert!(all.get("recent_errors").is_none());
    assert!(all.contains_key("version"));
    assert!(all.contains_key("config"));
    assert!(all.contains_key("brain_health"));
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
fn local_runtime_transition_is_progressing_and_suppresses_an_action() {
    assert!(brain_progressing(Some("local_runtime_not_ready"), true));
    assert_eq!(
        support_action(
            "blocked",
            Some("local_runtime_not_ready"),
            brain_progressing(Some("local_runtime_not_ready"), true),
            Some("bundled"),
            None,
        ),
        serde_json::Value::Null
    );
}
