// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Recorded backup facts only. No repository, tool, scheduler or operation probe.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use solstone_core_backup::{backup_setup_fingerprint, load_hosted_binding, merge_backup_config};
use solstone_core_journal_config::read_journal_config;

use crate::context::CheckContext;
use crate::vocabulary::{Check, CheckResult, RunnerResult, Status, make_result};

const ROUTE: &str = "open backup or health in your journal for details";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let row = |status, detail: &str| result(check, status, detail.to_owned());
    match std::fs::metadata(&context.journal_path) {
        Ok(metadata) if metadata.is_dir() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(row(Status::Skip, "no local journal"));
        }
        _ => {
            return Ok(row(
                Status::Warn,
                "backup status unavailable: journal unreadable",
            ));
        }
    }
    // Validate the raw boundary before the domain reader's defaults can turn a
    // null or scalar backup section into an apparently intentional off state.
    let read = match read_journal_config(&context.journal_path) {
        Ok(read) => read,
        Err(_) => {
            return Ok(row(
                Status::Warn,
                "backup status unavailable: settings unreadable",
            ));
        }
    };
    let config = read.config.unwrap_or_default();
    let backup = match config.get("backup") {
        None => return Ok(row(Status::Skip, "backup not configured")),
        Some(Value::Object(backup)) => backup,
        _ => {
            return Ok(row(
                Status::Warn,
                "backup status unavailable: invalid backup settings",
            ));
        }
    };
    match backup.get("enabled") {
        None | Some(Value::Bool(false)) => return Ok(row(Status::Skip, "backup off")),
        Some(Value::Bool(true)) => {}
        _ => {
            return Ok(row(
                Status::Warn,
                "backup status unavailable: invalid on/off setting",
            ));
        }
    }
    // A pause is the owner's choice: it reads as paused with its history, and
    // never as failing or stale.
    match backup.get("paused_at") {
        None | Some(Value::Null) => {}
        Some(paused) if paused.is_i64() => {
            let history = backup.get("last_backup").map_or_else(
                || "last successful copy unknown".to_owned(),
                |latest| success_history(latest, context.now),
            );
            return Ok(row(
                Status::Skip,
                &format!(
                    "backup paused by you since {}; {history}",
                    date(Some(paused))
                ),
            ));
        }
        Some(_) => {
            return Ok(row(
                Status::Warn,
                "backup status unavailable: invalid pause setting",
            ));
        }
    }
    // Off deliberately avoids all unused history and identity inputs. Identity
    // is needed only for a record that actually carries a valid fingerprint.
    let latest = match backup.get("last_backup") {
        None => return Ok(row(Status::Skip, "backup on; no recorded attempt")),
        Some(value) => value,
    };
    let current_identity = || {
        let merged = merge_backup_config(&config);
        let binding = (merged.get("mode").and_then(Value::as_str) == Some("operated"))
            .then(|| load_hosted_binding(&context.journal_path))
            .flatten();
        backup_setup_fingerprint(&merged, binding.as_ref())
    };
    Ok(assess(check, latest, context.now, current_identity))
}

fn result(check: Check, status: Status, detail: String) -> CheckResult {
    make_result(
        check,
        status,
        detail,
        (status == Status::Warn).then_some(ROUTE),
    )
}

struct Attempt<'a> {
    raw: &'a Map<String, Value>,
    success: bool,
    incomplete: bool,
}

fn attempt(value: &Value) -> Result<Option<Attempt<'_>>, ()> {
    let raw = value.as_object().ok_or(())?;
    let status = raw.get("status").filter(|v| !v.is_null());
    if status.is_none()
        && [
            "time",
            "snapshot_id",
            "error_reason",
            "setup_fingerprint",
            "last_success",
        ]
        .iter()
        .all(|key| raw.get(*key).is_none_or(Value::is_null))
    {
        return Ok(None);
    }
    let success = match status.and_then(Value::as_str) {
        Some("ok") => true,
        Some("error") => false,
        _ => return Err(()),
    };
    for key in ["snapshot_id", "error_reason"] {
        if raw.get(key).is_some_and(|v| !v.is_null() && !v.is_string()) {
            return Err(());
        }
    }
    Ok(Some(Attempt {
        raw,
        success,
        incomplete: raw.get("error_reason").and_then(Value::as_str) == Some("incomplete"),
    }))
}

fn assess(
    check: Check,
    latest: &Value,
    now: DateTime<Utc>,
    current_identity: impl FnOnce() -> Option<String>,
) -> CheckResult {
    let history = success_history(latest, now);
    let latest = match attempt(latest) {
        Ok(Some(latest)) => latest,
        Ok(None) => return result(check, Status::Skip, "backup on; no recorded attempt".into()),
        Err(()) => {
            return result(
                check,
                Status::Warn,
                format!("backup on; recorded status unavailable; {history}"),
            );
        }
    };
    let outcome = if latest.incomplete {
        "incomplete copy"
    } else if latest.success {
        "successful copy"
    } else {
        "failed"
    };
    let reason = match latest.raw.get("error_reason").and_then(Value::as_str) {
        None => "",
        Some("incomplete") => "; some files were not copied",
        Some("auth_failed") => "; backup key refused",
        Some("timeout") => "; timed out",
        Some("locked") => "; repository was locked",
        Some(_) => "; cause unknown",
    };
    let detail = format!(
        "backup on; latest recorded attempt: {outcome} at {}{reason}; {history}",
        date(latest.raw.get("time"))
    );
    let (status, applicability) = match latest.raw.get("setup_fingerprint") {
        None => (Status::Skip, "setup association unknown; history only"),
        Some(Value::String(old))
            if old.len() == 64 && old.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            match current_identity() {
                None => (Status::Warn, "current setup association unavailable"),
                Some(current) if current != *old => {
                    (Status::Skip, "superseded setup; history only")
                }
                Some(_) if !latest.success || latest.incomplete => {
                    (Status::Warn, "matches the selected setup")
                }
                Some(_) => (Status::Ok, "matches the selected setup"),
            }
        }
        Some(_) => (Status::Warn, "recorded setup association unavailable"),
    };
    result(check, status, format!("{detail}; {applicability}"))
}

fn success_history(latest: &Value, now: DateTime<Utc>) -> String {
    // Presence matters: an explicit unknown must never fall back to the latest
    // successful attempt. An absent member retains the domain's legacy history.
    let saved = latest.get("last_success").unwrap_or(latest);
    let Ok(Some(saved)) = attempt(saved) else {
        return "last successful copy unknown".into();
    };
    if !saved.success {
        return "last successful copy unknown".into();
    }
    let snapshot = saved
        .raw
        .get("snapshot_id")
        .and_then(Value::as_str)
        .map(identifier)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let time = saved.raw.get("time");
    let age = timestamp(time)
        .filter(|time| *time <= now)
        .map(|time| format!("{} seconds", now.signed_duration_since(time).num_seconds()))
        .unwrap_or_else(|| "unknown".into());
    let caveat = if saved.incomplete {
        "; incomplete copy, some files were not copied"
    } else {
        ""
    };
    format!(
        "last successful copy (historical): snapshot {snapshot} at {}, age {age}{caveat}; present coverage and verification unknown",
        date(time)
    )
}

fn timestamp(value: Option<&Value>) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(value?.as_i64()?, 0)
}

fn date(value: Option<&Value>) -> String {
    timestamp(value)
        .map(|time| time.to_rfc3339())
        .unwrap_or_else(|| "unknown date".into())
}

fn identifier(value: &str) -> String {
    value
        .chars()
        .take(80)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '?'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        output,
        registry::{self, Battery},
        vocabulary::{Platform, Severity, results_failed},
    };
    use serde_json::json;

    fn check() -> Check {
        registry::lookup(Battery::Journal, "backup_status")
            .unwrap()
            .check
    }

    #[test]
    fn optional_advisory_registered_on_every_journal_platform_only() {
        let check = check();
        assert_eq!(check.severity, Severity::Advisory);
        for platform in [Platform::Linux, Platform::Darwin, Platform::Windows] {
            assert!(check.platforms.contains(&platform));
        }
        assert!(registry::lookup(Battery::JournalReadiness, check.name).is_none());
    }

    #[test]
    fn applicability_precedes_every_outcome_and_never_uses_dates() {
        let now = DateTime::from_timestamp(200, 0).unwrap();
        for (status, reason, matched) in [
            ("ok", Value::Null, Status::Ok),
            ("ok", json!("incomplete"), Status::Warn),
            ("error", json!("auth_failed"), Status::Warn),
        ] {
            let mut raw = json!({"status":status,"error_reason":reason,"time":100});
            let unbound = assess(check(), &raw, now, || {
                panic!("unbound history needs no identity")
            });
            assert_eq!(unbound.status, Status::Skip);
            raw["setup_fingerprint"] = json!("a".repeat(64));
            assert_eq!(
                assess(check(), &raw, now, || Some("a".repeat(64))).status,
                matched
            );
            assert_eq!(
                assess(check(), &raw, now, || Some("b".repeat(64))).status,
                Status::Skip
            );
            assert_eq!(assess(check(), &raw, now, || None).status, Status::Warn);
            for bad in [Value::Null, json!(12), json!("wrong")] {
                raw["setup_fingerprint"] = bad;
                assert_eq!(
                    assess(check(), &raw, now, || panic!("invalid fingerprint")).status,
                    Status::Warn
                );
            }
        }
    }

    #[test]
    fn explicit_unknown_success_and_invalid_age_are_preserved() {
        let now = DateTime::from_timestamp(200, 0).unwrap();
        let mut raw =
            json!({"status":"ok","time":100,"snapshot_id":"legacy","error_reason":"incomplete"});
        assert!(success_history(&raw, now).contains("snapshot legacy"));
        raw["last_success"] = Value::Null;
        assert!(!success_history(&raw, now).contains("snapshot legacy"));
        for time in [Value::Null, json!("bad"), json!(201), json!(i64::MAX)] {
            raw["last_success"] = json!({"status":"ok","time":time});
            assert!(success_history(&raw, now).contains("age unknown"));
        }
        raw["last_success"] = json!({"status":"ok","time":100});
        assert!(success_history(&raw, now).contains("age 100 seconds"));
    }

    #[test]
    fn only_allowlisted_facts_reach_every_existing_output() {
        let raw = json!({"status":"error", "time":100, "setup_fingerprint":"a".repeat(64),
            "error_reason":"secret-repository-url\u{1b}[31m", "detail":"broker-token-secret",
            "last_success":{"status":"ok","time":50,"snapshot_id":"snap\n\u{1b}[31m\u{202e}".repeat(100)}});
        let row = assess(
            check(),
            &raw,
            DateTime::from_timestamp(200, 0).unwrap(),
            || Some("a".repeat(64)),
        );
        assert!(!row.detail.chars().any(char::is_control));
        assert!(!row.detail.contains('\u{202e}'));
        assert!(row.detail.len() < 700);
        let rows = [row];
        let mut text = Vec::new();
        output::emit_text_to(&mut text, &rows, false).unwrap();
        let json = serde_json::to_value(&rows[0]).unwrap();
        let mut jsonl = Vec::new();
        output::emit_jsonl_to(&mut jsonl, &rows, "test", 0, 0);
        let jsonl: Vec<Value> = String::from_utf8(jsonl)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(json["detail"], jsonl[1]["detail"]);
        assert!(
            String::from_utf8(text)
                .unwrap()
                .contains(rows[0].detail.as_str())
        );
        for output in [
            serde_json::to_string(&json).unwrap(),
            serde_json::to_string(&jsonl).unwrap(),
        ] {
            for secret in [
                "secret-repository-url",
                "broker-token-secret",
                &"a".repeat(64),
            ] {
                assert!(!output.contains(secret));
            }
        }
        assert!(!results_failed(&rows));
        assert_eq!(output::summary_status(&rows), "warning");
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod filesystem_tests {
    use super::*;
    use crate::{
        checks::test_support::context,
        output,
        registry::{self, Battery},
        vocabulary::{Platform, results_failed},
    };
    use serde_json::json;
    use solstone_core_backup::{
        Destination, HostedBinding, generate_and_store_keys, get_backup_config,
        record_backup_result, save_hosted_binding, set_destination, set_enabled, set_mode,
    };
    use std::{collections::BTreeMap, fs, path::Path};

    fn inventory(root: &Path) -> BTreeMap<std::path::PathBuf, (bool, Vec<u8>)> {
        fn walk(dir: &Path, out: &mut BTreeMap<std::path::PathBuf, (bool, Vec<u8>)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let directory = path.is_dir();
                out.insert(
                    path.clone(),
                    (
                        directory,
                        if directory {
                            Vec::new()
                        } else {
                            fs::read(&path).unwrap()
                        },
                    ),
                );
                if directory {
                    walk(&path, out);
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, &mut out);
        out
    }

    fn observe(context: &CheckContext, name: &str, expected: Status) -> CheckResult {
        let parent = context.journal_path.parent().unwrap();
        let before = inventory(parent);
        let entry = registry::lookup(Battery::Journal, "backup_status").unwrap();
        let row = (entry.runner)(context).unwrap();
        assert_eq!(row.status, expected, "{name}: {}", row.detail);
        assert_eq!(
            inventory(parent),
            before,
            "check wrote filesystem state: {name}"
        );
        assert!(!results_failed(std::slice::from_ref(&row)));
        let mut normal = Vec::new();
        output::emit_text_to(&mut normal, std::slice::from_ref(&row), false).unwrap();
        assert_eq!(
            String::from_utf8(normal).unwrap().contains("backup_status"),
            expected == Status::Warn
        );
        let mut verbose = Vec::new();
        output::emit_text_to(&mut verbose, std::slice::from_ref(&row), true).unwrap();
        assert!(String::from_utf8(verbose).unwrap().contains(&row.detail));
        let mut jsonl = Vec::new();
        output::emit_jsonl_to(&mut jsonl, std::slice::from_ref(&row), "test", 0, 0);
        let event: Value =
            serde_json::from_str(String::from_utf8(jsonl).unwrap().lines().nth(1).unwrap())
                .unwrap();
        assert_eq!(
            event["detail"],
            serde_json::to_value(&row).unwrap()["detail"]
        );
        println!("{}", json!({"case":name,"result":row,"read_only":true}));
        row
    }

    fn write_config(context: &CheckContext, value: Value) {
        fs::create_dir_all(context.journal_path.join("config")).unwrap();
        fs::write(
            context.journal_path.join("config/journal.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn actual_writer_through_doctor_and_existing_consumers_is_read_only() {
        let mut staged = context();
        staged.context.now = DateTime::from_timestamp(400, 0).unwrap();
        observe(&staged, "missing root", Status::Skip);
        fs::create_dir_all(&staged.journal_path).unwrap();
        observe(&staged, "missing config", Status::Skip);
        set_destination(
            &staged.journal_path,
            &Destination {
                repository: "s3:synthetic-a".into(),
                backend: "s3".into(),
                credentials: Map::new(),
            },
        )
        .unwrap();
        generate_and_store_keys(&staged.journal_path).unwrap();
        set_enabled(&staged.journal_path, true).unwrap();
        observe(&staged, "enabled no attempt", Status::Skip);
        let a = backup_setup_fingerprint(&get_backup_config(&staged.journal_path).unwrap(), None)
            .unwrap();
        record_backup_result(
            &staged.journal_path,
            "ok",
            json!(100),
            json!("snapshot-a"),
            json!("incomplete"),
            None,
            Some(&a),
        )
        .unwrap();
        observe(&staged, "incomplete current success", Status::Warn);
        record_backup_result(
            &staged.journal_path,
            "error",
            json!(200),
            Value::Null,
            json!("auth_failed"),
            Some("secret stderr"),
            Some(&a),
        )
        .unwrap();
        let failed = observe(&staged, "failure retains incomplete success", Status::Warn);
        assert!(failed.detail.contains("snapshot-a"));
        assert!(failed.detail.contains("age 300 seconds"));
        assert!(failed.detail.contains("incomplete copy"));
        set_destination(
            &staged.journal_path,
            &Destination {
                repository: "s3:synthetic-b".into(),
                backend: "s3".into(),
                credentials: Map::new(),
            },
        )
        .unwrap();
        observe(
            &staged,
            "replacement keeps failure historical",
            Status::Skip,
        );
        record_backup_result(
            &staged.journal_path,
            "error",
            json!(300),
            Value::Null,
            json!("timeout"),
            None,
            Some(&a),
        )
        .unwrap();
        observe(
            &staged,
            "old in-flight result after replacement",
            Status::Skip,
        );
        let b = backup_setup_fingerprint(&get_backup_config(&staged.journal_path).unwrap(), None)
            .unwrap();
        record_backup_result(
            &staged.journal_path,
            "ok",
            json!(350),
            json!("snapshot-b"),
            Value::Null,
            None,
            Some(&b),
        )
        .unwrap();
        for platform in [Platform::Linux, Platform::Darwin, Platform::Windows] {
            staged.context.platform = platform;
            let row = observe(&staged, "newer current success", Status::Ok);
            assert!(row.detail.contains("snapshot-b"));
            assert!(!row.detail.contains("snapshot-a"));
        }
        record_backup_result(
            &staged.journal_path,
            "ok",
            json!(360),
            json!("old-success"),
            Value::Null,
            None,
            Some(&a),
        )
        .unwrap();
        observe(&staged, "replaced successful result", Status::Skip);
        set_enabled(&staged.journal_path, false).unwrap();
        observe(&staged, "explicit off", Status::Skip);
        set_mode(&staged.journal_path, "operated").unwrap();
        set_enabled(&staged.journal_path, true).unwrap();
        let binding = HostedBinding {
            broker_endpoint: "https://example.invalid".into(),
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: "prefix/".into(),
            broker_token: "secret-token".into(),
        };
        save_hosted_binding(&staged.journal_path, &binding).unwrap();
        let operated = backup_setup_fingerprint(
            &get_backup_config(&staged.journal_path).unwrap(),
            Some(&binding),
        )
        .unwrap();
        record_backup_result(
            &staged.journal_path,
            "ok",
            json!(370),
            json!("operated-copy"),
            Value::Null,
            None,
            Some(&operated),
        )
        .unwrap();
        observe(&staged, "current operated success", Status::Ok);
        fs::remove_file(staged.journal_path.join("backup/hosted/binding.json")).unwrap();
        observe(
            &staged,
            "missing operated binding with valid keys",
            Status::Warn,
        );
        fs::write(
            staged.journal_path.join("backup/hosted/binding.json"),
            b"{broken",
        )
        .unwrap();
        observe(
            &staged,
            "malformed operated binding with valid keys",
            Status::Warn,
        );
        save_hosted_binding(
            &staged.journal_path,
            &HostedBinding {
                bucket: "replacement".into(),
                ..binding
            },
        )
        .unwrap();
        observe(&staged, "replaced operated binding", Status::Skip);
    }

    #[test]
    fn raw_boundaries_unknown_history_and_binding_never_look_healthy() {
        let staged = context();
        for value in [
            json!({"backup":null}),
            json!({"backup":[]}),
            json!({"backup":{"enabled":null}}),
            json!({"backup":{"enabled":"false"}}),
        ] {
            write_config(&staged, value);
            observe(&staged, "invalid backup boundary", Status::Warn);
        }
        for malformed in [
            Value::Null,
            json!([]),
            json!({"status":"unknown"}),
            json!({"status":"ok","snapshot_id":42}),
        ] {
            write_config(
                &staged,
                json!({"backup":{"enabled":false,"last_backup":malformed}}),
            );
            observe(
                &staged,
                "off ignores malformed unused history",
                Status::Skip,
            );
            write_config(
                &staged,
                json!({"backup":{"enabled":true,"last_backup":malformed}}),
            );
            observe(
                &staged,
                "enabled malformed history unavailable",
                Status::Warn,
            );
        }
        for status in ["ok", "error"] {
            let mut latest = json!({"status":status,"snapshot_id":"legacy","time":100,"error_reason":"incomplete"});
            write_config(
                &staged,
                json!({"backup":{"enabled":true,"last_backup":latest}}),
            );
            observe(&staged, "unbound history", Status::Skip);
            latest["setup_fingerprint"] = json!("a".repeat(64));
            for mode in ["byo", "operated"] {
                write_config(
                    &staged,
                    json!({"backup":{"enabled":true,"mode":mode,"last_backup":latest}}),
                );
                observe(&staged, "unavailable current identity", Status::Warn);
            }
        }
        fs::create_dir_all(staged.journal_path.join("backup/hosted")).unwrap();
        fs::write(
            staged.journal_path.join("backup/hosted/binding.json"),
            b"{broken",
        )
        .unwrap();
        observe(&staged, "malformed binding", Status::Warn);
        fs::write(staged.journal_path.join("config/journal.json"), b"{broken").unwrap();
        observe(&staged, "invalid JSON config", Status::Warn);
        fs::remove_file(staged.journal_path.join("config/journal.json")).unwrap();
        fs::create_dir(staged.journal_path.join("config/journal.json")).unwrap();
        observe(&staged, "unreadable config entry", Status::Warn);
    }

    #[test]
    fn a_paused_backup_reads_as_paused_with_its_history_never_as_a_warning() {
        let mut staged = context();
        staged.context.now = DateTime::from_timestamp(400, 0).unwrap();
        fs::create_dir_all(&staged.journal_path).unwrap();
        generate_and_store_keys(&staged.journal_path).unwrap();
        set_enabled(&staged.journal_path, true).unwrap();
        record_backup_result(
            &staged.journal_path,
            "error",
            json!(300),
            Value::Null,
            json!("timeout"),
            None,
            None,
        )
        .unwrap();
        solstone_core_backup::set_backup_paused(&staged.journal_path, Some(350)).unwrap();
        let row = observe(&staged, "paused", Status::Skip);
        assert!(
            row.detail
                .starts_with("backup paused by you since 1970-01-01T00:05:50+00:00; "),
            "{}",
            row.detail
        );
        assert!(
            row.detail.contains("last successful copy unknown"),
            "{}",
            row.detail
        );
    }
}
