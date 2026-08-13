// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_facets::append_action_log;

use crate::command::ObserverCommand;
use crate::store::format::{fmt_bytes, render_list, render_status_all, render_status_single};
use crate::store::prune::{DaySelector, format_result, resolve_prune_days, run_prune};
use crate::store::reconcile::{ReconcilePlan, reconcile_plan};
use crate::store::record::ObserverRecord;
use crate::store::reload::{find_observer, load_observers};
use crate::store::write::save_observer;

pub const CREATE_RETIRED_MESSAGE: &str = "journal observer create is retired. observers register themselves: from this machine directly, or from a paired device.\nfor a remote device, pair it first:  sol call link pair --device-label <name>\nif a device was re-paired and its stream is stuck, clear the old observer first:\n  journal observer revoke <name>\n";

#[derive(Debug)]
pub enum ObserverError {
    NotFound(String),
    AlreadyRevoked(String),
    NameExists(String),
    AlreadyNamed(String),
    InvalidIdentifier,
    Internal(String),
}

impl ObserverError {
    pub fn is_user_error(&self) -> bool {
        matches!(
            self,
            Self::NotFound(_)
                | Self::AlreadyRevoked(_)
                | Self::NameExists(_)
                | Self::AlreadyNamed(_)
                | Self::InvalidIdentifier
        )
    }
}

impl std::fmt::Display for ObserverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(identifier) => write!(f, "Error: observer '{identifier}' not found"),
            Self::AlreadyRevoked(name) => write!(f, "Observer '{name}' is already revoked."),
            Self::NameExists(name) => write!(f, "Error: observer '{name}' already exists"),
            Self::AlreadyNamed(name) => write!(f, "Observer is already named '{name}'."),
            Self::InvalidIdentifier => f.write_str("Error: invalid observer identifier"),
            Self::Internal(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for ObserverError {}

pub fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// `prune`'s exit-code contract (0 clean, 1 usage/unexpected error, 2
/// refusals present) does not fit `execute`'s binary success/failure
/// `Result`, so it is dispatched separately -- the same reason `Create`
/// short-circuits before `execute` is ever called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneOutcome {
    /// A usage error (bad day/day-range). Print to stderr, exit 1.
    Usage(String),
    /// A completed plan or execution. Print to stdout, exit `exit_code`.
    Report { text: String, exit_code: i32 },
}

#[allow(clippy::too_many_arguments)]
pub fn execute_prune(
    journal_root: &Path,
    day: Option<String>,
    day_range: Option<(String, String)>,
    all: bool,
    stream: Option<String>,
    execute: bool,
    cross_start: bool,
    now_ms: i64,
) -> PruneOutcome {
    if cross_start {
        // TODO: --cross-start is not yet ported (server-authored
        // segment_original provenance resolution). Refuse loudly rather than
        // silently running same-start-only and under-reporting duplicates.
        return PruneOutcome::Usage("--cross-start is not yet implemented".to_owned());
    }
    let selector = if let Some(day) = day {
        DaySelector::Day(day)
    } else if let Some(range) = day_range {
        DaySelector::DayRange(range.0, range.1)
    } else {
        debug_assert!(all, "parser guarantees exactly one selector");
        DaySelector::All
    };
    let days = match resolve_prune_days(journal_root, &selector) {
        Ok(days) => days,
        Err(message) => return PruneOutcome::Usage(message),
    };
    let result = run_prune(journal_root, &days, stream.as_deref(), execute, now_ms);
    PruneOutcome::Report {
        text: format_result(&result),
        exit_code: result.exit_code(),
    }
}

pub fn execute(
    journal_root: &Path,
    command: ObserverCommand,
    now_ms: i64,
) -> Result<String, ObserverError> {
    match command {
        ObserverCommand::Create => unreachable!("create is dispatched before journal resolution"),
        ObserverCommand::Prune { .. } => {
            unreachable!("prune is dispatched via execute_prune, not execute")
        }
        ObserverCommand::List { json } => Ok(render_list(&load(journal_root)?, json, now_ms)),
        ObserverCommand::Status {
            identifier: None,
            json,
        } => Ok(render_status_all(&load(journal_root)?, json, now_ms)),
        ObserverCommand::Status {
            identifier: Some(identifier),
            json,
        } => {
            let record = find(journal_root, &identifier)?;
            Ok(render_status_single(journal_root, &record, json, now_ms))
        }
        ObserverCommand::Rename { old, new, json } => rename(journal_root, &old, new, json),
        ObserverCommand::Revoke { identifier, json } => {
            revoke(journal_root, &identifier, json, now_ms)
        }
        ObserverCommand::Reconcile { dry_run, json } => {
            reconcile(journal_root, dry_run, json, now_ms)
        }
    }
}

fn load(root: &Path) -> Result<Vec<ObserverRecord>, ObserverError> {
    load_observers(root).map_err(|error| ObserverError::Internal(error.to_string()))
}
fn find(root: &Path, identifier: &str) -> Result<ObserverRecord, ObserverError> {
    find_observer(root, identifier)
        .map_err(|error| {
            if error.to_string() == "invalid observer identifier" {
                ObserverError::InvalidIdentifier
            } else {
                ObserverError::Internal(error.to_string())
            }
        })?
        .ok_or_else(|| ObserverError::NotFound(identifier.to_owned()))
}

fn rename(root: &Path, old: &str, new: String, json_output: bool) -> Result<String, ObserverError> {
    let mut observer = find(root, old)?;
    if let Some(existing) = load(root)?
        .into_iter()
        .find(|record| record.name() == Some(new.as_str()))
        && existing.key() != observer.key()
    {
        return Err(ObserverError::NameExists(new));
    }
    let old_name = observer.name().unwrap_or_default().to_owned();
    if old_name == new {
        return Err(ObserverError::AlreadyNamed(new));
    }
    let prefix = observer.prefix();
    observer.set_name(new.clone());
    save_observer(root, &observer)
        .map_err(|_| ObserverError::Internal("Error: failed to save observer".to_owned()))?;
    append_action_log(
        root,
        None,
        "app",
        "observer",
        "observer_rename",
        json!({"old_name":old_name,"new_name":new,"key_prefix":prefix}),
    )
    .map_err(|error| ObserverError::Internal(error.to_string()))?;
    if json_output {
        Ok(
            serde_json::to_string(&json!({"old_name":old_name,"new_name":new,"prefix":prefix}))
                .expect("JSON values serialize"),
        )
    } else {
        Ok(format!(
            "Renamed observer '{old_name}' -> '{new}' ({prefix})\n  Future segments will use stream: {new}\n  Existing segments remain under stream: {old_name}"
        ))
    }
}

fn revoke(
    root: &Path,
    identifier: &str,
    json_output: bool,
    now_ms: i64,
) -> Result<String, ObserverError> {
    let observer = revoke_record(root, find(root, identifier)?, now_ms)?;
    let name = observer.name().unwrap_or_default();
    let prefix = observer.prefix();
    if json_output {
        Ok(
            serde_json::to_string(&json!({"name":name,"prefix":prefix,"revoked":true}))
                .expect("JSON values serialize"),
        )
    } else {
        Ok(format!("Revoked observer '{name}' ({prefix})"))
    }
}

fn revoke_record(
    root: &Path,
    mut observer: ObserverRecord,
    now_ms: i64,
) -> Result<ObserverRecord, ObserverError> {
    if observer.revoked() {
        return Err(ObserverError::AlreadyRevoked(
            observer.name().unwrap_or_default().to_owned(),
        ));
    }
    let name = observer.name().unwrap_or_default().to_owned();
    let prefix = observer.prefix();
    observer.set_revoked(true);
    observer.set_revoked_at(now_ms);
    save_observer(root, &observer)
        .map_err(|_| ObserverError::Internal("Error: failed to save observer".to_owned()))?;
    append_action_log(
        root,
        None,
        "app",
        "observer",
        "observer_revoke",
        json!({"name":name,"key_prefix":prefix}),
    )
    .map_err(|error| ObserverError::Internal(error.to_string()))?;
    Ok(observer)
}

fn reconcile(
    root: &Path,
    dry_run: bool,
    json_output: bool,
    now_ms: i64,
) -> Result<String, ObserverError> {
    let records = load(root)?;
    let plan = reconcile_plan(&records);
    if !dry_run {
        for entry in &plan {
            let mut survivor = records
                .iter()
                .find(|record| record.prefix() == entry.survivor_prefix)
                .expect("plan survivor exists")
                .clone();
            survivor.set_stats(entry.stats.clone());
            save_observer(root, &survivor).map_err(ObserverError::Internal)?;
            for prefix in &entry.revoked_prefixes {
                revoke_record(root, find(root, prefix)?, now_ms)?;
            }
        }
    }
    if json_output {
        return Ok(
            serde_json::to_string(&Value::Array(plan.iter().map(plan_value).collect()))
                .expect("JSON values serialize"),
        );
    }
    if plan.is_empty() {
        return Ok("No duplicate observer streams to reconcile.".to_owned());
    }
    Ok(plan
        .iter()
        .map(|entry| format_plan(entry, dry_run))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn plan_value(entry: &ReconcilePlan) -> Value {
    json!({"name":entry.name,"survivor_prefix":entry.survivor_prefix,"revoked_prefixes":entry.revoked_prefixes,"stats":entry.stats})
}
fn format_plan(entry: &ReconcilePlan, dry_run: bool) -> String {
    let prefix = if dry_run {
        "[dry-run] would reconcile"
    } else {
        "Reconciled"
    };
    let segments = entry
        .stats
        .get("segments_received")
        .cloned()
        .unwrap_or_else(|| Value::from(0));
    let bytes = entry
        .stats
        .get("bytes_received")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let mut output = format!(
        "{prefix} stream '{}':\n  survivor:  {}\n  revoking:  {}\n  segments:  {}\n  bytes:     {}",
        entry.name,
        entry.survivor_prefix,
        entry.revoked_prefixes.join(", "),
        segments,
        fmt_bytes(bytes)
    );
    if let Some(value) = entry
        .stats
        .get("duplicates_rejected")
        .filter(|value| !value.is_null() && value != &&Value::from(0))
    {
        output.push_str(&format!("\n  duplicates: {value} rejected"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::paths::observer_path;
    use serde_json::json;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    fn root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "observer-service-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }
    fn seed(root: &Path, key: &str, name: &str, created: i64) {
        let mut record = ObserverRecord::from_value(json!({"key":key,"name":name,"created_at":created,"stats":{"segments_received":1,"bytes_received":2}})).expect("record");
        record.set_revoked(false);
        save_observer(root, &record).expect("save");
    }
    #[test]
    fn errors_have_python_messages() {
        let root = root("errors");
        assert_eq!(
            execute(
                &root,
                ObserverCommand::Revoke {
                    identifier: "missing".into(),
                    json: false
                },
                1
            )
            .expect_err("missing")
            .to_string(),
            "Error: observer 'missing' not found"
        );
        seed(&root, "abcdefghx", "one", 1);
        seed(&root, "ijklmnopy", "two", 2);
        assert_eq!(
            execute(
                &root,
                ObserverCommand::Rename {
                    old: "one".into(),
                    new: "one".into(),
                    json: false
                },
                1
            )
            .expect_err("same")
            .to_string(),
            "Observer is already named 'one'."
        );
        assert_eq!(
            execute(
                &root,
                ObserverCommand::Rename {
                    old: "one".into(),
                    new: "two".into(),
                    json: false
                },
                1
            )
            .expect_err("collision")
            .to_string(),
            "Error: observer 'two' already exists"
        );
        let mut revoked = ObserverRecord::from_value(
            json!({"key":"qrstuvwxz","name":"gone","revoked":true,"stats":{}}),
        )
        .expect("record");
        revoked.set_revoked(true);
        save_observer(&root, &revoked).expect("save");
        assert_eq!(
            execute(
                &root,
                ObserverCommand::Revoke {
                    identifier: "gone".into(),
                    json: false
                },
                1
            )
            .expect_err("already revoked")
            .to_string(),
            "Observer 'gone' is already revoked."
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rename_revoke_and_reconcile_retain_every_unowned_field() {
        let root = root("attrition");
        let now = 50_000;
        save_rich(
            &root,
            "abcdefghx",
            "rename",
            1,
            json!({"segments_received":1}),
        );
        let before = read_record(&root, "abcdefgh");
        execute(
            &root,
            ObserverCommand::Rename {
                old: "rename".into(),
                new: "renamed".into(),
                json: false,
            },
            now,
        )
        .expect("rename");
        assert_equal_except(before, read_record(&root, "abcdefgh"), &["name"]);

        save_rich(
            &root,
            "ijklmnopy",
            "revoke",
            2,
            json!({"segments_received":2}),
        );
        let before = read_record(&root, "ijklmnop");
        execute(
            &root,
            ObserverCommand::Revoke {
                identifier: "revoke".into(),
                json: false,
            },
            now,
        )
        .expect("revoke");
        assert_equal_except(
            before,
            read_record(&root, "ijklmnop"),
            &["revoked", "revoked_at"],
        );

        save_rich(
            &root,
            "qrstuvwxz",
            "reconcile",
            3,
            json!({"segments_received":3}),
        );
        save_rich(
            &root,
            "yzabcdefg",
            "reconcile",
            4,
            json!({"segments_received":4}),
        );
        let before = read_record(&root, "qrstuvwx");
        execute(
            &root,
            ObserverCommand::Reconcile {
                dry_run: false,
                json: false,
            },
            now,
        )
        .expect("reconcile");
        assert_equal_except(before, read_record(&root, "qrstuvwx"), &["stats"]);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn reconcile_commit_keeps_oldest_sums_stats_and_prints_truthy_duplicates() {
        let root = root("reconcile-commit");
        save_rich(
            &root,
            "aaaaaaaa1",
            "stream",
            1,
            json!({"segments_received":1,"bytes_received":100,"duplicates_rejected":3}),
        );
        save_rich(
            &root,
            "bbbbbbbb2",
            "stream",
            2,
            json!({"segments_received":5,"bytes_received":900,"duplicates_rejected":2}),
        );
        let output = execute(
            &root,
            ObserverCommand::Reconcile {
                dry_run: false,
                json: false,
            },
            99,
        )
        .expect("commit");
        assert_eq!(
            output,
            "Reconciled stream 'stream':\n  survivor:  aaaaaaaa\n  revoking:  bbbbbbbb\n  segments:  6\n  bytes:     1000 B\n  duplicates: 5 rejected"
        );
        assert_eq!(
            read_record(&root, "aaaaaaaa")["stats"],
            json!({"segments_received":6,"bytes_received":1000,"duplicates_rejected":5})
        );
        assert_eq!(read_record(&root, "bbbbbbbb")["revoked"], true);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn reconcile_commit_omits_duplicates_line_when_counter_is_absent() {
        let root = root("reconcile-no-duplicates");
        save_rich(
            &root,
            "cccccccc1",
            "plain",
            1,
            json!({"segments_received":2,"bytes_received":1}),
        );
        save_rich(
            &root,
            "dddddddd2",
            "plain",
            2,
            json!({"segments_received":7,"bytes_received":2}),
        );
        let output = execute(
            &root,
            ObserverCommand::Reconcile {
                dry_run: false,
                json: false,
            },
            99,
        )
        .expect("commit");
        assert_eq!(
            output,
            "Reconciled stream 'plain':\n  survivor:  cccccccc\n  revoking:  dddddddd\n  segments:  9\n  bytes:     3 B"
        );
        assert!(!output.contains("duplicates:"));
        fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn dry_run_does_not_modify_observer_tree() {
        let root = root("dry");
        seed(&root, "abcdefghx", "same", 1);
        seed(&root, "ijklmnopy", "same", 2);
        let before = fs::read(observer_path(&root, "abcdefgh")).expect("before");
        let output = execute(
            &root,
            ObserverCommand::Reconcile {
                dry_run: true,
                json: false,
            },
            1,
        )
        .expect("plan");
        assert!(output.starts_with("[dry-run]"));
        assert_eq!(
            fs::read(observer_path(&root, "abcdefgh")).expect("after"),
            before
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn dry_run_snapshot_keeps_every_path_size_and_mtime() {
        let root = root("dry-snapshot");
        seed(&root, "abcdefghx", "same", 1);
        seed(&root, "ijklmnopy", "same", 2);
        let before = snapshot(&root.join("apps/observer"));
        execute(
            &root,
            ObserverCommand::Reconcile {
                dry_run: true,
                json: false,
            },
            1,
        )
        .expect("plan");
        assert_eq!(snapshot(&root.join("apps/observer")), before);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rename_and_revoke_json_shapes_and_action_params_match_python() {
        let root = root("actions");
        seed(&root, "abcdefghx", "old", 1);
        assert_eq!(
            execute(
                &root,
                ObserverCommand::Rename {
                    old: "old".into(),
                    new: "new".into(),
                    json: true
                },
                10
            )
            .expect("rename"),
            r#"{"old_name":"old","new_name":"new","prefix":"abcdefgh"}"#
        );
        assert_eq!(
            execute(
                &root,
                ObserverCommand::Revoke {
                    identifier: "new".into(),
                    json: true
                },
                11
            )
            .expect("revoke"),
            r#"{"name":"new","prefix":"abcdefgh","revoked":true}"#
        );
        let action_path = fs::read_dir(root.join("config/actions"))
            .expect("actions")
            .next()
            .expect("action file")
            .expect("entry")
            .path();
        let rows: Vec<Value> = fs::read_to_string(action_path)
            .expect("read")
            .lines()
            .map(|line| serde_json::from_str(line).expect("row"))
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["source"], "app");
        assert_eq!(rows[0]["actor"], "observer");
        assert_eq!(rows[0]["action"], "observer_rename");
        assert_eq!(
            rows[0]["params"],
            json!({"old_name":"old","new_name":"new","key_prefix":"abcdefgh"})
        );
        assert_eq!(rows[1]["action"], "observer_revoke");
        assert_eq!(
            rows[1]["params"],
            json!({"name":"new","key_prefix":"abcdefgh"})
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    fn snapshot(root: &Path) -> Vec<(String, bool, u64, u128)> {
        fn walk(root: &Path, path: &Path, rows: &mut Vec<(String, bool, u64, u128)>) {
            for entry in fs::read_dir(path).expect("directory") {
                let path = entry.expect("entry").path();
                let metadata = fs::symlink_metadata(&path).expect("metadata");
                rows.push((
                    path.strip_prefix(root)
                        .expect("relative")
                        .display()
                        .to_string(),
                    metadata.is_dir(),
                    metadata.len(),
                    metadata
                        .modified()
                        .expect("mtime")
                        .duration_since(UNIX_EPOCH)
                        .expect("epoch")
                        .as_nanos(),
                ));
                if metadata.is_dir() {
                    walk(root, &path, rows);
                }
            }
        }
        let mut rows = Vec::new();
        walk(root, root, &mut rows);
        rows.sort();
        rows
    }

    fn save_rich(root: &Path, key: &str, name: &str, created_at: i64, stats: Value) {
        let record = ObserverRecord::from_value(json!({
            "key":key, "name":name, "created_at":created_at, "device_binding":{"device":format!("sha256:{}", "a".repeat(64)),"kind":"cert"}, "enabled":false,
            "health":{"beacon":{"at":1},"ingest_rejection":{"reason":"test"}}, "platform":"linux", "hostname":"host", "stream_type":"screen", "label":"label", "version":"1.2.3", "future_field":{"nested":[1,2]}, "stats":stats
        })).expect("record");
        save_observer(root, &record).expect("save");
    }

    fn read_record(root: &Path, prefix: &str) -> Value {
        serde_json::from_slice(&fs::read(observer_path(root, prefix)).expect("read")).expect("JSON")
    }

    fn assert_equal_except(mut before: Value, mut after: Value, keys: &[&str]) {
        for key in keys {
            before.as_object_mut().expect("object").remove(*key);
            after.as_object_mut().expect("object").remove(*key);
        }
        assert_eq!(before, after);
    }
}
