// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::Value;

use solstone_core_journal_io::{AtomicWriteOptions, path_lexists, read_text, write_text};

use crate::hold_facet_trust_lock;

use super::error::{FacetStoreError, FacetWriteError};
use super::paths::{FacetContentKind, content_file_path};

/// Read activity JSONL or nested activity bytes without interpretation.
pub fn read_activity_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
) -> Result<Option<String>, FacetStoreError> {
    read_content_file(journal_root, facet_dir, relative_path)
}

/// Atomically replace activity JSONL or nested activity bytes without interpretation.
pub fn write_activity_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
    contents: &str,
) -> Result<(), FacetWriteError> {
    write_content_file(journal_root, facet_dir, relative_path, contents)
}

/// Add one semantic activity row, preserving existing rows and their order.
pub fn add_activity(
    journal_root: &Path,
    facet_dir: &str,
    activity: Value,
) -> Result<Value, FacetWriteError> {
    let mut rows = read_activity_definitions(journal_root, facet_dir)?;
    let id = activity
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !rows
        .iter()
        .any(|row| row.get("id").and_then(Value::as_str) == Some(id))
    {
        rows.push(activity.clone());
        write_rows(journal_root, facet_dir, &rows)?;
    }
    Ok(rows
        .into_iter()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or(activity))
}

/// Update an existing semantic activity row.
pub fn update_activity(
    journal_root: &Path,
    facet_dir: &str,
    id: &str,
    updates: &serde_json::Map<String, Value>,
) -> Result<Option<Value>, FacetWriteError> {
    let mut rows = read_activity_definitions(journal_root, facet_dir)?;
    let Some(output) = apply_activity_updates(&mut rows, id, updates) else {
        return Ok(None);
    };
    write_rows(journal_root, facet_dir, &rows)?;
    Ok(Some(output))
}

/// Update an activity a facet shows but may not store yet. When no row has
/// `id`, the rows `attach` returns for the stored rows are added first, in its
/// order, and none when it returns none; then the update applies as in
/// `update_activity`. When `collapse` says the result stores no more than an
/// empty file would, the file is emptied instead, or left absent when nothing
/// was stored. One read, and at most one write.
pub fn update_activity_attaching(
    journal_root: &Path,
    facet_dir: &str,
    id: &str,
    updates: &serde_json::Map<String, Value>,
    attach: impl FnOnce(&[Value]) -> Vec<Value>,
    collapse: impl FnOnce(&[Value]) -> bool,
) -> Result<Option<Value>, FacetWriteError> {
    let mut rows = read_activity_definitions(journal_root, facet_dir)?;
    let stored = !rows.is_empty();
    if !rows
        .iter()
        .any(|row| row.get("id").and_then(Value::as_str) == Some(id))
    {
        for row in attach(&rows) {
            let row_id = row.get("id").and_then(Value::as_str).map(str::to_owned);
            if row.is_object()
                && !rows
                    .iter()
                    .any(|stored| stored.get("id").and_then(Value::as_str) == row_id.as_deref())
            {
                rows.push(row);
            }
        }
    }
    let Some(output) = apply_activity_updates(&mut rows, id, updates) else {
        return Ok(None);
    };
    if collapse(&rows) {
        rows.clear();
        if !stored {
            return Ok(Some(output));
        }
    }
    write_rows(journal_root, facet_dir, &rows)?;
    Ok(Some(output))
}

/// The row with `id`, changed by `updates` under the semantic rules: a
/// default's empty description or instructions and its `normal` priority are
/// stored as no key, and a default's name and icons are its own. `None` when
/// no row has `id`.
fn apply_activity_updates(
    rows: &mut [Value],
    id: &str,
    updates: &serde_json::Map<String, Value>,
) -> Option<Value> {
    let row = rows
        .iter_mut()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(id))?;
    let object = row.as_object_mut().expect("activity rows are objects");
    let custom = object.get("custom").and_then(Value::as_bool) == Some(true);
    for (key, value) in updates {
        match (key.as_str(), value, custom) {
            ("description" | "instructions", Value::String(value), false) if value.is_empty() => {
                object.remove(key);
            }
            ("priority", Value::String(value), false) if value == "normal" => {
                object.remove(key);
            }
            ("emoji" | "icon", Value::String(value), true) if value.is_empty() => {
                object.remove(key);
            }
            ("name" | "emoji" | "icon", _, false) => {}
            _ => {
                object.insert(key.clone(), value.clone());
            }
        }
    }
    Some(row.clone())
}

/// Remove an explicit activity row by id.
pub fn remove_activity(
    journal_root: &Path,
    facet_dir: &str,
    id: &str,
) -> Result<bool, FacetWriteError> {
    let mut rows = read_activity_definitions(journal_root, facet_dir)?;
    let before = rows.len();
    rows.retain(|row| row.get("id").and_then(Value::as_str) != Some(id));
    if before != rows.len() {
        write_rows(journal_root, facet_dir, &rows)?;
    }
    Ok(before != rows.len())
}

/// Move legacy custom emoji glyphs from `icon` into `emoji`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityIconMigrationReport {
    pub files_scanned: usize,
    pub files_changed: usize,
    pub records_changed: usize,
}

pub fn migrate_custom_activity_icons_to_emoji(
    journal_root: &Path,
    dry_run: bool,
) -> Result<ActivityIconMigrationReport, FacetWriteError> {
    let mut report = ActivityIconMigrationReport::default();
    let facets = journal_root.join("facets");
    let Ok(entries) = fs::read_dir(facets) else {
        return Ok(report);
    };
    for entry in entries.flatten() {
        let facet = entry.file_name().to_string_lossy().to_string();
        let Some(text) = read_activity_file(journal_root, &facet, "activities.jsonl")? else {
            continue;
        };
        report.files_scanned += 1;
        let mut changed = false;
        let path = content_file_path(
            journal_root,
            &facet,
            FacetContentKind::Activities,
            "activities.jsonl",
        )?;
        let mut rows = parse_activity_rows(&path, &text)?;
        for row in &mut rows {
            let object = row.as_object_mut().expect("validated activity object");
            let custom = object.get("custom").and_then(Value::as_bool) == Some(true);
            if custom
                && object
                    .get("emoji")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                && let Some(icon) = object
                    .get("icon")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                && !is_lucide_name(&icon)
            {
                object.insert("emoji".to_owned(), Value::String(icon));
                object.remove("icon");
                changed = true;
                report.records_changed += 1;
            }
        }
        if changed {
            report.files_changed += 1;
            if !dry_run {
                write_rows(journal_root, &facet, &rows)?;
            }
        }
    }
    Ok(report)
}

fn is_lucide_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Read semantic activity definitions without changing absent or damaged files.
pub fn read_activity_definitions(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<Vec<Value>, FacetStoreError> {
    let path = content_file_path(
        journal_root,
        facet_dir,
        FacetContentKind::Activities,
        "activities.jsonl",
    )?;
    let text = read_activity_file(journal_root, facet_dir, "activities.jsonl")?.unwrap_or_default();
    parse_activity_rows(&path, &text)
}

fn parse_activity_rows(path: &Path, text: &str) -> Result<Vec<Value>, FacetStoreError> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let malformed = |reason| FacetStoreError::MalformedActivityDefinition {
                path: path.to_path_buf(),
                line: index + 1,
                reason,
            };
            let row: Value = serde_json::from_str(line).map_err(|_| malformed("invalid JSON"))?;
            if !row.is_object() {
                return Err(malformed("expected an object"));
            }
            Ok(row)
        })
        .collect()
}

fn write_rows(journal_root: &Path, facet_dir: &str, rows: &[Value]) -> Result<(), FacetWriteError> {
    let text = rows
        .iter()
        .map(python_json_line)
        .collect::<Vec<_>>()
        .join("\n");
    write_activity_file(
        journal_root,
        facet_dir,
        "activities.jsonl",
        &(if text.is_empty() {
            text
        } else {
            format!("{text}\n")
        }),
    )
}

fn python_json_line(value: &Value) -> String {
    let compact = serde_json::to_string(value).expect("JSON row");
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

fn read_content_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
) -> Result<Option<String>, FacetStoreError> {
    let path = content_file_path(
        journal_root,
        facet_dir,
        FacetContentKind::Activities,
        relative_path,
    )?;
    if !path_lexists(&path)? {
        return Ok(None);
    }
    read_text(&path, String::new())
        .map(Some)
        .map_err(Into::into)
}

fn write_content_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
    contents: &str,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = content_file_path(
        journal_root,
        facet_dir,
        FacetContentKind::Activities,
        relative_path,
    )?;
    write_text(&path, contents, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(FacetWriteError::ContentWrite)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn declared_root() -> tempfile::TempDir {
        let root = tempdir().unwrap();
        let facet = root.path().join("facets/work");
        fs::create_dir_all(&facet).unwrap();
        fs::write(facet.join("facet.json"), r#"{"title":"Work"}"#).unwrap();
        root
    }

    #[test]
    fn semantic_edits_preserve_damaged_definitions_and_report_physical_line() {
        let root = declared_root();
        let path = root
            .path()
            .join("facets")
            .join("work")
            .join("activities")
            .join("activities.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for bad in ["{sensitive-sentinel", "\"sensitive-sentinel\""] {
            let original =
                format!("\n{{\"id\":\"kept\",\"custom\":true}}\n\n{bad}\n{{\"id\":\"last\"}}\n");
            for operation in 0..3 {
                fs::write(&path, &original).unwrap();
                let expected_path = fs::canonicalize(&path).unwrap();
                let result = match operation {
                    0 => add_activity(root.path(), "work", serde_json::json!({"id":"new"}))
                        .map(|_| ()),
                    1 => update_activity(root.path(), "work", "kept", &serde_json::Map::new())
                        .map(|_| ()),
                    _ => remove_activity(root.path(), "work", "kept").map(|_| ()),
                };
                let error = result.unwrap_err();
                assert!(
                    matches!(
                        &error,
                        FacetWriteError::Read(FacetStoreError::MalformedActivityDefinition {
                            path: actual, line: 4, ..
                        }) if actual == &expected_path
                    ),
                    "{error}"
                );
                assert!(!error.to_string().contains("sensitive-sentinel"));
                assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
            }
        }
    }

    #[test]
    fn semantic_edits_refuse_unreadable_and_invalid_utf8_input() {
        let root = declared_root();
        let path = root
            .path()
            .join("facets")
            .join("work")
            .join("activities")
            .join("activities.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(add_activity(root.path(), "work", serde_json::json!({"id":"new"})).is_err());
        assert_eq!(fs::read(&path).unwrap(), [0xff, 0xfe]);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(remove_activity(root.path(), "work", "kept").is_err());
        assert!(path.is_dir());
    }

    #[test]
    fn healthy_definitions_keep_first_use_and_semantic_edit_behavior() {
        let root = declared_root();
        let path = root
            .path()
            .join("facets")
            .join("work")
            .join("activities")
            .join("activities.jsonl");
        let first = serde_json::json!({"id":"first","custom":true,"extra":{"preserve":1}});
        add_activity(root.path(), "work", first.clone()).unwrap();
        add_activity(root.path(), "work", first.clone()).unwrap();
        assert_eq!(
            read_activity_definitions(root.path(), "work").unwrap(),
            vec![first.clone()]
        );
        fs::write(&path, format!("\n{first}\n\n")).unwrap();
        let updates = serde_json::json!({"name":"Renamed"});
        let changed = update_activity(root.path(), "work", "first", updates.as_object().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(changed["name"], "Renamed");
        assert_eq!(changed["extra"], first["extra"]);
        assert!(remove_activity(root.path(), "work", "first").unwrap());
        assert!(!remove_activity(root.path(), "work", "first").unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"");
        add_activity(root.path(), "work", first.clone()).unwrap();
        assert_eq!(
            read_activity_definitions(root.path(), "work").unwrap(),
            vec![first]
        );
    }

    #[test]
    fn icon_conversion_preserves_damaged_input_in_both_modes() {
        let root = declared_root();
        let path = root
            .path()
            .join("facets")
            .join("work")
            .join("activities")
            .join("activities.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let eligible = "{\"id\":\"x\",\"custom\":true,\"icon\":\"🎯\"}\n";
        for bad in ["{broken", "42"] {
            let original = format!("{eligible}{bad}\n");
            for dry_run in [true, false] {
                fs::write(&path, &original).unwrap();
                assert!(migrate_custom_activity_icons_to_emoji(root.path(), dry_run).is_err());
                assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
            }
        }
        fs::write(&path, eligible).unwrap();
        let preview = migrate_custom_activity_icons_to_emoji(root.path(), true).unwrap();
        assert_eq!(preview.records_changed, 1);
        assert_eq!(fs::read(&path).unwrap(), eligible.as_bytes());
        let committed = migrate_custom_activity_icons_to_emoji(root.path(), false).unwrap();
        assert_eq!(committed.records_changed, 1);
        let rows = read_activity_definitions(root.path(), "work").unwrap();
        assert_eq!(rows[0]["emoji"], "🎯");
        assert!(rows[0].get("icon").is_none());
    }

    #[test]
    fn migrates_custom_glyph_and_preserves_lucide_and_existing_emoji() {
        let temp = declared_root();
        let path = temp.path().join("facets/work/activities/activities.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"id\": \"x\", \"custom\": true, \"icon\": \"🎯\"}\n{\"id\": \"y\", \"custom\": true, \"icon\": \"target\"}\n{\"id\": \"z\", \"custom\": true, \"emoji\": \"✅\", \"icon\": \"old\"}\n").unwrap();
        let report = migrate_custom_activity_icons_to_emoji(temp.path(), false).unwrap();
        assert_eq!(report.records_changed, 1);
        let written = fs::read_to_string(path).unwrap();
        assert!(written.contains("\"emoji\": \"🎯\""));
        assert!(written.contains("\"icon\": \"target\""));
        assert!(written.contains("\"icon\": \"old\""));
    }

    #[test]
    fn attaching_update_stores_the_attached_rows_once_and_applies_the_same_rules() {
        let root = declared_root();
        let path = root.path().join("facets/work/activities/activities.jsonl");
        let high = serde_json::json!({"priority": "high"});
        let normal = serde_json::json!({"priority": "normal"});
        let bare = |id: &str| serde_json::json!({ "id": id });
        let attach = |_: &[Value]| vec![bare("a"), bare("b"), bare("c")];
        let changed = update_activity_attaching(
            root.path(),
            "work",
            "b",
            high.as_object().unwrap(),
            attach,
            |_| false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(changed, serde_json::json!({"id": "b", "priority": "high"}));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\"id\": \"a\"}\n{\"id\": \"b\", \"priority\": \"high\"}\n{\"id\": \"c\"}\n"
        );
        // Stored now: nothing is attached again, and normal is no key.
        let never = |_: &[Value]| -> Vec<Value> { panic!("not asked once the row is stored") };
        update_activity_attaching(
            root.path(),
            "work",
            "b",
            normal.as_object().unwrap(),
            never,
            |_| false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\"id\": \"a\"}\n{\"id\": \"b\"}\n{\"id\": \"c\"}\n"
        );
        // A collapse empties the stored file.
        update_activity_attaching(
            root.path(),
            "work",
            "b",
            normal.as_object().unwrap(),
            never,
            |rows| rows.len() == 3,
        )
        .unwrap()
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn attaching_update_writes_nothing_for_an_id_not_attached_or_a_collapse_from_nothing() {
        let root = declared_root();
        let path = root.path().join("facets/work/activities/activities.jsonl");
        let high = serde_json::json!({"priority": "high"});
        let none = |_: &[Value]| Vec::new();
        assert!(
            update_activity_attaching(
                root.path(),
                "work",
                "x",
                high.as_object().unwrap(),
                none,
                |_| false
            )
            .unwrap()
            .is_none()
        );
        let other = |_: &[Value]| vec![serde_json::json!({"id": "y"})];
        assert!(
            update_activity_attaching(
                root.path(),
                "work",
                "x",
                high.as_object().unwrap(),
                other,
                |_| false
            )
            .unwrap()
            .is_none()
        );
        assert!(!path_lexists(&path).unwrap());
        let normal = serde_json::json!({"priority": "normal"});
        let only = |_: &[Value]| vec![serde_json::json!({"id": "x"})];
        let output = update_activity_attaching(
            root.path(),
            "work",
            "x",
            normal.as_object().unwrap(),
            only,
            |_| true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(output, serde_json::json!({"id": "x"}));
        assert!(
            !path_lexists(&path).unwrap(),
            "nothing stored before, nothing written"
        );
    }

    #[test]
    fn attaching_update_refuses_damaged_definitions_without_rewriting_them() {
        let root = declared_root();
        let path = root.path().join("facets/work/activities/activities.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = "{\"id\":\"kept\"}\n{sensitive-sentinel\n";
        fs::write(&path, original).unwrap();
        let high = serde_json::json!({"priority": "high"});
        let attach = |_: &[Value]| vec![serde_json::json!({"id": "new"})];
        let error = update_activity_attaching(
            root.path(),
            "work",
            "new",
            high.as_object().unwrap(),
            attach,
            |_| false,
        )
        .unwrap_err();
        assert!(!error.to_string().contains("sensitive-sentinel"));
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
    }
}
