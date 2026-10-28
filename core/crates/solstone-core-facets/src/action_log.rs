// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal action-log records.
//!
//! Mirrors the Python reference's `solstone.think.facets._write_action_log`
//! both the journal-level (`facet=None`) and facet-scoped branches: a durable
//! audit trail of user- and agent-initiated actions.

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use serde_json::{Value, json};
use solstone_core_journal_io::{AppendError, append_text_checked};

/// Append the existing journal-level audit row beneath the caller's original
/// journal capability. The native append never follows a diagnostic pathname.
pub fn append_journal_action_log_at_root_checked(
    root: &Arc<solstone_core_journal_io::JournalRoot>,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AppendError> {
    append_journal_action_log_with_hint_boundary(
        root,
        source,
        actor,
        action,
        params,
        checkpoint,
        &|| {},
    )
}

fn append_journal_action_log_with_hint_boundary(
    root: &Arc<solstone_core_journal_io::JournalRoot>,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
    checkpoint: &dyn Fn() -> Result<(), String>,
    before_hint: &dyn Fn(),
) -> Result<(), AppendError> {
    use solstone_core_journal_io::append_text_bound_checked;
    use solstone_core_journal_io::media::MediaDirectory;
    use std::ffi::OsStr;
    let diagnostic = root.canonical_path().join("config/actions");
    let failure = |error: String| AppendError::Io {
        path: diagnostic.clone(),
        source: std::io::Error::other(error),
    };
    let check_root = || {
        checkpoint().map_err(&failure)?;
        root.revalidate_canonical_binding()
            .map_err(|error| failure(error.to_string()))
    };
    check_root()?;
    let config_media = MediaDirectory::open(root.clone(), Path::new("config"))
        .map_err(|error| failure(error.to_string()))?;
    #[cfg(unix)]
    let config = solstone_core_journal_io::open_flat_directory_bound(
        &**root,
        OsStr::new("config"),
        root.canonical_path(),
    )
    .map_err(|error| failure(error.to_string()))?
    .ok_or_else(|| failure("config directory disappeared".to_owned()))?;
    #[cfg(windows)]
    let config = solstone_core_journal_io::open_windows_flat_directory_bound(
        &**root,
        OsStr::new("config"),
        root.canonical_path(),
    )
    .map_err(|error| failure(error.to_string()))?
    .ok_or_else(|| failure("config directory disappeared".to_owned()))?;
    #[cfg(unix)]
    let config_identity = config.identity();
    #[cfg(windows)]
    let config_identity = config.object_identity();
    if config_media.object_identity() != config_identity {
        return Err(failure("config directory changed".to_owned()));
    }
    let check_config = || {
        check_root()?;
        config_media
            .revalidate()
            .map_err(|error| failure(error.to_string()))
    };
    check_config()?;
    #[cfg(unix)]
    let actions = solstone_core_journal_io::create_or_open_flat_directory_bound(
        &config,
        OsStr::new("actions"),
        0o777,
        &root.canonical_path().join("config"),
    )
    .map_err(|error| failure(error.to_string()))?;
    #[cfg(windows)]
    let actions = solstone_core_journal_io::create_or_open_windows_flat_directory_bound(
        &config,
        OsStr::new("actions"),
        &root.canonical_path().join("config"),
    )
    .map_err(|error| failure(error.to_string()))?;
    let actions_media = MediaDirectory::open(root.clone(), Path::new("config/actions"))
        .map_err(|error| failure(error.to_string()))?;
    #[cfg(unix)]
    let actions_identity = actions.identity();
    #[cfg(windows)]
    let actions_identity = actions.object_identity();
    if actions_media.object_identity() != actions_identity {
        return Err(failure("action-log directory changed".to_owned()));
    }
    let check_actions = || {
        check_config()?;
        actions_media
            .revalidate()
            .map_err(|error| failure(error.to_string()))
    };
    check_actions()?;
    let config = solstone_core_journal_config::read_journal_config_bound(root)
        .map_err(|error| failure(error.to_string()))?
        .config
        .unwrap_or_default();
    let zone = config
        .get("identity")
        .and_then(|identity| identity.get("timezone"))
        .and_then(Value::as_str)
        .and_then(solstone_core_journal_config::parse_zone)
        .unwrap_or_else(solstone_core_journal_config::host_zone);
    let day = Utc::now().with_timezone(&zone).format("%Y%m%d").to_string();
    let check = || check_actions().map_err(|error| error.to_string());
    append_text_bound_checked(
        &actions,
        OsStr::new(&format!("{day}.jsonl")),
        &python_json(&action_record(None, source, actor, action, params)),
        &check,
    )?;
    check_root()?;
    before_hint();
    let namespace =
        solstone_core_journal_io::journal_directory::JournalNamespace::new(root.clone());
    if let Err(error) = solstone_core_indexer_store::todo::enqueue_path_bound(
        &namespace,
        &diagnostic.join(format!("{day}.jsonl")),
    ) {
        log::warn!("action log search update could not be queued: {error}");
    }
    check_root()?;
    Ok(())
}

/// Append an action-log record to the journal-level or facet-scoped destination.
pub fn append_action_log(
    journal_root: &Path,
    facet: Option<&str>,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
) -> Result<(), AppendError> {
    append_action_log_checked(journal_root, facet, source, actor, action, params, &|| {
        Ok(())
    })
}

/// Append one action-log record, checking before its parent is created and again
/// immediately before the record is written.
pub fn append_action_log_checked(
    journal_root: &Path,
    facet: Option<&str>,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AppendError> {
    let day = Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal_root))
        .format("%Y%m%d")
        .to_string();
    append_action_log_at_day(
        journal_root,
        facet,
        source,
        actor,
        action,
        params,
        &day,
        checkpoint,
    )
}

pub fn append_action_log_for_day(
    journal_root: &Path,
    facet: Option<&str>,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
    day: &str,
) -> Result<(), AppendError> {
    append_action_log_at_day(
        journal_root,
        facet,
        source,
        actor,
        action,
        params,
        day,
        &|| Ok(()),
    )
}

#[allow(clippy::too_many_arguments)]
fn append_action_log_at_day(
    journal_root: &Path,
    facet: Option<&str>,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
    day: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), AppendError> {
    let destination = match facet {
        Some(facet) => journal_root
            .join("facets")
            .join(facet)
            .join("logs")
            .join(format!("{day}.jsonl")),
        None => journal_root
            .join("config/actions")
            .join(format!("{day}.jsonl")),
    };
    append_text_checked(
        &destination,
        &python_json(&action_record(facet, source, actor, action, params)),
        checkpoint,
    )?;
    if let Err(error) = solstone_core_indexer_store::todo::enqueue_path(journal_root, &destination)
    {
        log::warn!("action log search update could not be queued: {error}");
    }
    Ok(())
}

fn action_record(
    facet: Option<&str>,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
) -> Value {
    let mut record = serde_json::Map::new();
    record.insert("timestamp".to_owned(), json!(Utc::now().to_rfc3339()));
    record.insert("source".to_owned(), json!(source));
    record.insert("actor".to_owned(), json!(actor));
    record.insert("action".to_owned(), json!(action));
    record.insert("params".to_owned(), params);
    if let Some(facet) = facet {
        record.insert("facet".to_owned(), json!(facet));
    }
    Value::Object(record)
}

fn python_json(value: &Value) -> String {
    let compact = serde_json::to_string(value).expect("action log JSON");
    let mut output = String::with_capacity(compact.len());
    let mut in_string = false;
    let mut escaped = false;
    for character in compact.chars() {
        output.push(character);
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
        } else if matches!(character, ',' | ':') {
            output.push(' ');
        }
    }
    output
}

#[cfg(test)]
mod checkpoint_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::append_action_log_checked;

    fn utc_journal() -> tempfile::TempDir {
        let root = tempfile::TempDir::new().expect("journal");
        std::fs::create_dir_all(root.path().join("config")).expect("config");
        std::fs::write(
            root.path().join("config/journal.json"),
            b"{\"identity\":{\"timezone\":\"UTC\"}}\n",
        )
        .expect("timezone");
        root
    }

    #[test]
    fn failed_check_creates_no_action_parent() {
        let root = utc_journal();
        let error = append_action_log_checked(
            root.path(),
            None,
            "app",
            "thinking",
            "env_update",
            json!({}),
            &|| Err("stop".to_owned()),
        );
        assert!(error.is_err());
        assert!(!root.path().join("config/actions").exists());
    }

    #[test]
    fn pre_write_check_does_not_append_the_record() {
        let root = utc_journal();
        let calls = AtomicUsize::new(0);
        let error = append_action_log_checked(
            root.path(),
            None,
            "app",
            "thinking",
            "env_update",
            json!({}),
            &|| {
                if calls.fetch_add(1, Ordering::SeqCst) >= 1 {
                    Err("stop".to_owned())
                } else {
                    Ok(())
                }
            },
        );
        assert!(error.is_err());
        let actions = root.path().join("config/actions");
        assert!(actions.is_dir());
        assert_eq!(std::fs::read_dir(actions).unwrap().count(), 0);
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::append_action_log;

    #[test]
    fn bound_action_audit_hint_refuses_replacement_after_the_final_append_check() {
        let temporary = tempfile::tempdir().unwrap();
        let journal = temporary.path().join("journal");
        let retained = temporary.path().join("retained");
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            br#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
        let root =
            std::sync::Arc::new(solstone_core_journal_io::JournalRoot::open(&journal).unwrap());
        let prevented = std::cell::Cell::new(false);
        let result = super::append_journal_action_log_with_hint_boundary(
            &root,
            "app",
            "settings",
            "sync_update",
            json!({}),
            &|| Ok(()),
            &|| {
                if let Err(error) = fs::rename(&journal, &retained) {
                    assert!(
                        cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32)),
                        "{error}"
                    );
                    prevented.set(true);
                    return;
                }
                fs::create_dir(&journal).unwrap();
            },
        );
        if prevented.get() {
            assert!(result.is_ok(), "{result:?}");
            root.revalidate_canonical_binding().unwrap();
            assert!(!retained.exists());
            assert_eq!(
                fs::read_dir(journal.join("config/actions"))
                    .unwrap()
                    .count(),
                1
            );
            drop(root);
            fs::rename(&journal, &retained).expect("closed owner releases the namespace");
            return;
        }
        let entries = fs::read_dir(&journal)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert!(
            entries.is_empty(),
            "replacement entries after actual audit hint: {entries:?}"
        );
        assert!(
            result.is_err(),
            "changed root cannot report a complete current-root audit"
        );
        let actions = fs::read_dir(retained.join("config/actions"))
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(
            actions.len(),
            1,
            "actual audit committed under the original root"
        );
        assert!(
            fs::read_to_string(actions[0].as_ref().unwrap().path())
                .unwrap()
                .contains("sync_update")
        );
    }

    #[test]
    fn bound_action_audit_keeps_its_normal_search_hint() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir(temporary.path().join("config")).unwrap();
        fs::write(
            temporary.path().join("config/journal.json"),
            br#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
        let root = std::sync::Arc::new(
            solstone_core_journal_io::JournalRoot::open(temporary.path()).unwrap(),
        );
        super::append_journal_action_log_at_root_checked(
            &root,
            "app",
            "settings",
            "sync_update",
            json!({}),
            &|| Ok(()),
        )
        .unwrap();
        let hints = solstone_core_indexer_store::todo::inspect(temporary.path())
            .unwrap()
            .keys;
        assert_eq!(hints.len(), 1);
        assert!(hints[0].starts_with("config/actions/"));
        assert!(temporary.path().join(&hints[0]).is_file());
    }

    #[test]
    fn both_action_log_destinations_queue_under_a_held_index_writer() {
        let root = crate::store_tests::TempDir::new();
        let lease = solstone_core_journal_io::hold_lock(
            solstone_core_indexer_store::db::db_path(root.path()),
            Default::default(),
        )
        .unwrap();
        for facet in [None, Some("work")] {
            super::append_action_log_for_day(
                root.path(),
                facet,
                "app",
                "settings",
                "kestrel",
                json!({}),
                "20260101",
            )
            .unwrap();
        }
        assert_eq!(
            solstone_core_indexer_store::todo::inspect(root.path())
                .unwrap()
                .keys,
            [
                "config/actions/20260101.jsonl",
                "facets/work/logs/20260101.jsonl"
            ]
        );
        assert!(!solstone_core_indexer_store::db::db_path(root.path()).exists());
        drop(lease);
        let report =
            solstone_core_indexer_store::todo::drain(root.path(), Default::default()).unwrap();
        assert_eq!((report.processed, report.failed), (2, 0));
        let conn = solstone_core_indexer_store::open_index_reader(root.path()).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunks WHERE chunks MATCH 'kestrel'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        let check = solstone_core_indexer_store::scan::scan_journal(root.path(), false).unwrap();
        assert_eq!((check.indexed, check.removed), (0, 0));
    }

    #[test]
    fn action_logs_preserve_python_insertion_order_and_unicode() {
        let root = crate::store_tests::TempDir::new();
        append_action_log(
            root.path(),
            Some("work"),
            "app",
            "settings",
            "activity_add",
            json!({"emoji":"🧪"}),
        )
        .expect("action log");
        let zone = solstone_core_journal_config::owner_zone(root.path());
        let day = chrono::Utc::now()
            .with_timezone(&zone)
            .format("%Y%m%d")
            .to_string();
        let line = fs::read_to_string(
            root.path()
                .join("facets/work/logs")
                .join(format!("{day}.jsonl")),
        )
        .expect("action log file");
        assert!(line.starts_with("{\"timestamp\": "));
        assert!(line.contains("\"emoji\": \"🧪\""));
        assert!(!line.contains("\\u"));
    }
}

/// Journal-level action-log rows for one action, read back in day order.
#[derive(Debug, Default)]
pub struct ActionLogScan {
    /// `(day, record)` for each row whose `action` matches, oldest day first.
    pub rows: Vec<(String, Value)>,
    /// Days whose file exists but couldn't be read; they are outside what
    /// the scan saw.
    pub unreadable_days: Vec<String>,
    /// Lines that aren't a JSON object.
    pub malformed: usize,
    /// The first and last day it could read.
    pub window: Option<(String, String)>,
}

/// Read every journal-level action-log row whose `action` is `action`. An
/// absent `config/actions` is an empty log; one that exists but can't be
/// listed is an error.
pub fn read_journal_action_rows(
    journal_root: &Path,
    action: &str,
) -> Result<ActionLogScan, String> {
    let directory = solstone_core_journal_io::contained_path(journal_root, "config/actions")
        .map_err(|error| error.to_string())?;
    if !solstone_core_journal_io::path_lexists(&directory).map_err(|error| error.to_string())? {
        return Ok(ActionLogScan::default());
    }
    let mut days: Vec<(String, std::path::PathBuf, bool)> =
        solstone_core_journal_io::list_dir_entries(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .into_iter()
            .filter_map(|entry| {
                let name = entry.name.to_str()?.to_owned();
                let day = name.strip_suffix(".jsonl")?.to_owned();
                (day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit())).then_some((
                    day,
                    entry.path,
                    entry.kind == solstone_core_journal_io::DirEntryKind::File,
                ))
            })
            .collect();
    days.sort();
    let mut scan = ActionLogScan::default();
    let mut read_days = Vec::new();
    for (day, path, regular) in days {
        let text = if regular {
            solstone_core_journal_io::read_text(&path, String::new()).ok()
        } else {
            None
        };
        let Some(text) = text else {
            scan.unreadable_days.push(day);
            continue;
        };
        read_days.push(day.clone());
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<Value>(line) {
                Ok(record) if record.is_object() => {
                    if record.get("action").and_then(Value::as_str) == Some(action) {
                        scan.rows.push((day.clone(), record));
                    }
                }
                _ => scan.malformed += 1,
            }
        }
    }
    scan.window = match (read_days.first(), read_days.last()) {
        (Some(first), Some(last)) => Some((first.clone(), last.clone())),
        _ => None,
    };
    Ok(scan)
}
