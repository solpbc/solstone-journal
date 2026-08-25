// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-search tolerates malformed internal domain field values by skipping
//! wrong-typed values instead of rendering them or aborting indexing:
//! `name`/`type` fall back, non-string `description` is omitted, `aka`/`tags`
//! render only string list members, and day parsing accepts ASCII digits only.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{Local, TimeZone, Utc};
use serde_json::{Map, Value};

pub const ENTITY_SEARCH_WATERMARK_MTIME_PATH: &str = "entity_search:__mtime__";
pub const ENTITY_SEARCH_WATERMARK_COUNT_PATH: &str = "entity_search:__count__";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitySearchRow {
    pub content: String,
    pub path: String,
    pub day: String,
    pub facet: String,
    pub agent: String,
    pub stream: String,
    pub idx: i64,
    pub time_bucket: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitySearchBuild {
    pub rows: Vec<EntitySearchRow>,
    pub watermark_mtime_secs: i64,
    pub count: i64,
}

pub fn build_entity_search(journal: &Path) -> io::Result<EntitySearchBuild> {
    let mut watermark = EntitySearchWatermark::default();
    let mut identities = BTreeMap::new();
    let mut relationships: BTreeMap<String, Vec<(String, JsonObject)>> = BTreeMap::new();

    load_identities(journal, &mut watermark, &mut identities)?;
    load_relationships(journal, &mut watermark, &mut relationships)?;

    let mut rows = Vec::new();
    for (entity_id, identity) in identities {
        if json_truthy(identity.get("blocked")) {
            continue;
        }

        let name = string_field(identity.get("name"))
            .unwrap_or_else(|| entity_id.replace('_', " ").to_title_case());
        let entity_type =
            string_field(identity.get("type")).unwrap_or_else(|| "Unknown".to_string());
        let aka_list = string_array(identity.get("aka"));

        let mut identity_lines = vec![format!("{name} ({entity_type})")];
        if !aka_list.is_empty() {
            identity_lines.push(format!("Also known as: {}", aka_list.join(", ")));
        }

        let path = format!("entity_search:{entity_id}");
        let rels = relationships.remove(&entity_id).unwrap_or_default();

        if rels.is_empty() {
            let content = identity_lines.join("\n");
            let day = ts_to_day(identity.get("updated_at").unwrap_or(&Value::Null))
                .or_else_empty(|| ts_to_day(identity.get("created_at").unwrap_or(&Value::Null)));
            rows.push(EntitySearchRow::new(content, path, day, String::new(), 0));
            continue;
        }

        for (idx, (facet_name, relationship)) in rels.into_iter().enumerate() {
            let mut lines = identity_lines.clone();
            if let Some(description) = string_field(relationship.get("description")) {
                lines.push(description);
            }
            let tags = string_array(relationship.get("tags"));
            if !tags.is_empty() {
                lines.push(format!("Tags: {}", tags.join(", ")));
            }

            let day = relationship_day(&relationship);
            rows.push(EntitySearchRow::new(
                lines.join("\n"),
                path.clone(),
                day,
                facet_name.to_lowercase(),
                idx as i64,
            ));
        }
    }

    Ok(EntitySearchBuild {
        rows,
        watermark_mtime_secs: watermark.max_mtime_secs,
        count: watermark.count,
    })
}

pub fn ts_to_day(value: &Value) -> String {
    ts_to_day_in(value, &Local)
}

fn ts_to_day_in<Tz>(value: &Value, timezone: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: std::fmt::Display,
{
    let Some(ms) = coerce_timestamp_millis(value) else {
        return String::new();
    };
    if ms <= 0 {
        return String::new();
    }
    let Some(utc) = Utc.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    utc.with_timezone(timezone).format("%Y%m%d").to_string()
}

fn relationship_day(relationship: &JsonObject) -> String {
    if let Some(Value::String(last_seen)) = relationship.get("last_seen")
        && last_seen.len() == 8
        && last_seen.bytes().all(|ch| ch.is_ascii_digit())
    {
        return last_seen.clone();
    }
    ts_to_day(relationship.get("updated_at").unwrap_or(&Value::Null))
        .or_else_empty(|| ts_to_day(relationship.get("attached_at").unwrap_or(&Value::Null)))
}

fn load_identities(
    journal: &Path,
    watermark: &mut EntitySearchWatermark,
    identities: &mut BTreeMap<String, JsonObject>,
) -> io::Result<()> {
    for (entity_id, entity_dir) in sorted_child_dirs(&journal.join("entities"))? {
        let entity_file = entity_dir.join("entity.json");
        if !entity_file.is_file() {
            continue;
        }
        watermark.record_file(&entity_file)?;
        if let Some(identity) = read_json_object(&entity_file) {
            identities.insert(entity_id, identity);
        }
    }
    Ok(())
}

fn load_relationships(
    journal: &Path,
    watermark: &mut EntitySearchWatermark,
    relationships: &mut BTreeMap<String, Vec<(String, JsonObject)>>,
) -> io::Result<()> {
    for (facet_name, facet_dir) in sorted_child_dirs(&journal.join("facets"))? {
        let entity_root = facet_dir.join("entities");
        if !entity_root.is_dir() {
            continue;
        }
        for (entity_id, entity_dir) in sorted_child_dirs(&entity_root)? {
            let relationship_file = entity_dir.join("entity.json");
            if !relationship_file.is_file() {
                continue;
            }
            watermark.record_file(&relationship_file)?;
            let Some(relationship) = read_json_object(&relationship_file) else {
                continue;
            };
            if json_truthy(relationship.get("detached")) {
                continue;
            }
            relationships
                .entry(entity_id)
                .or_default()
                .push((facet_name.clone(), relationship));
        }
    }
    Ok(())
}

fn sorted_child_dirs(root: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        dirs.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
        ));
    }
    dirs.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(dirs)
}

fn read_json_object(path: &Path) -> Option<JsonObject> {
    let text = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(record) => Some(record),
        _ => None,
    }
}

fn coerce_timestamp_millis(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => {
            if let Some(ms) = number.as_i64() {
                Some(ms)
            } else if let Some(ms) = number.as_u64() {
                i64::try_from(ms).ok()
            } else {
                let ms = number.as_f64()?.trunc();
                if ms < i64::MIN as f64 || ms > i64::MAX as f64 {
                    None
                } else {
                    Some(ms as i64)
                }
            }
        }
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn string_field(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::String(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Return whether a JSON field has the truthy semantics used by entity indexing.
pub fn json_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64() != Some(0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(value)) => !value.is_empty(),
        Some(Value::Object(value)) => !value.is_empty(),
    }
}

fn file_mtime_secs(path: &Path) -> io::Result<i64> {
    let modified = fs::metadata(path)?.modified()?;
    let duration = modified.duration_since(UNIX_EPOCH).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid mtime: {error}"),
        )
    })?;
    Ok(duration.as_secs() as i64)
}

#[derive(Debug, Default)]
struct EntitySearchWatermark {
    max_mtime_secs: i64,
    count: i64,
}

impl EntitySearchWatermark {
    fn record_file(&mut self, path: &Path) -> io::Result<()> {
        self.max_mtime_secs = self.max_mtime_secs.max(file_mtime_secs(path)?);
        self.count += 1;
        Ok(())
    }
}

impl EntitySearchRow {
    fn new(content: String, path: String, day: String, facet: String, idx: i64) -> Self {
        Self {
            content,
            path,
            day,
            facet,
            agent: "entity".to_string(),
            stream: String::new(),
            idx,
            time_bucket: String::new(),
        }
    }
}

type JsonObject = Map<String, Value>;

trait EmptyFallback {
    fn or_else_empty(self, fallback: impl FnOnce() -> String) -> String;
}

impl EmptyFallback for String {
    fn or_else_empty(self, fallback: impl FnOnce() -> String) -> String {
        if self.is_empty() { fallback() } else { self }
    }
}

trait TitleCase {
    fn to_title_case(&self) -> String;
}

impl TitleCase for str {
    fn to_title_case(&self) -> String {
        let mut result = String::new();
        let mut new_word = true;
        for ch in self.chars() {
            if ch.is_alphabetic() {
                if new_word {
                    result.extend(ch.to_uppercase());
                } else {
                    result.extend(ch.to_lowercase());
                }
                new_word = false;
            } else {
                result.push(ch);
                new_word = true;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;
    use chrono::FixedOffset;
    use serde_json::json;

    fn temp_root(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-core-indexer-entity-search-{name}"))
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("test path should have parent"))
            .expect("create parent");
        fs::write(path, text).expect("write test file");
    }

    #[test]
    fn ts_to_day_matches_python_coercion_without_bool_special_case() {
        let utc = FixedOffset::east_opt(0).expect("utc offset");
        assert_eq!(ts_to_day_in(&json!(1000), &utc), "19700101");
        assert_eq!(ts_to_day_in(&json!(1000.9), &utc), "19700101");
        assert_eq!(ts_to_day_in(&json!("1000"), &utc), "19700101");
        assert_eq!(ts_to_day_in(&json!("  +1000 "), &utc), "19700101");
        assert_eq!(ts_to_day_in(&json!(0), &utc), "");
        assert_eq!(ts_to_day_in(&json!(-1000), &utc), "");
        assert_eq!(ts_to_day_in(&json!("1.5"), &utc), "");
        assert_eq!(ts_to_day_in(&json!(true), &utc), "");
        assert_eq!(ts_to_day_in(&json!(null), &utc), "");
        assert_eq!(ts_to_day_in(&json!({}), &utc), "");
        assert_eq!(
            ts_to_day(&json!(1767249000000i64)),
            ts_to_day_in(&json!(1767249000000i64), &Local)
        );
    }

    #[test]
    fn ts_to_day_uses_supplied_timezone() {
        let value = json!(1767249000000i64);
        let denver = FixedOffset::west_opt(7 * 3600).expect("denver offset");
        let utc = FixedOffset::east_opt(0).expect("utc offset");
        assert_eq!(ts_to_day_in(&value, &denver), "20251231");
        assert_eq!(ts_to_day_in(&value, &utc), "20260101");
    }

    #[test]
    fn builds_relationship_rows_with_content_fields_days_and_sorted_indexes() {
        let root = temp_root("relationships");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice Johnson","type":"Person","aka":["Al","AJ"],"updated_at":1767249000000}"#,
        );
        write(
            &root,
            "facets/work/entities/alice/entity.json",
            r#"{"description":"Works on native indexing","tags":["rust","search"],"last_seen":"20260102"}"#,
        );
        write(
            &root,
            "facets/personal/entities/alice/entity.json",
            r#"{"description":"College friend","tags":["friend"],"last_seen":20260103,"updated_at":1767249000000}"#,
        );

        let build = build_entity_search(&root).expect("build entity search");
        assert_eq!(build.count, 3);
        assert_eq!(build.rows.len(), 2);
        assert!(build.watermark_mtime_secs > 0);

        let personal = &build.rows[0];
        assert_eq!(personal.path, "entity_search:alice");
        assert_eq!(personal.facet, "personal");
        assert_eq!(personal.agent, "entity");
        assert_eq!(personal.stream, "");
        assert_eq!(personal.time_bucket, "");
        assert_eq!(personal.idx, 0);
        assert_eq!(personal.day, ts_to_day(&json!(1767249000000i64)));
        assert_eq!(
            personal.content,
            "Alice Johnson (Person)\nAlso known as: Al, AJ\nCollege friend\nTags: friend"
        );

        let work = &build.rows[1];
        assert_eq!(work.facet, "work");
        assert_eq!(work.idx, 1);
        assert_eq!(work.day, "20260102");
        assert_eq!(
            work.content,
            "Alice Johnson (Person)\nAlso known as: Al, AJ\nWorks on native indexing\nTags: rust, search"
        );
        fs::remove_dir_all(root).expect("cleanup relationships root");
    }

    #[test]
    fn identity_only_rows_use_identity_day_fallback_and_name_defaults() {
        let root = temp_root("identity-only");
        write(
            &root,
            "entities/api_optimization-v2/entity.json",
            r#"{"type":"Project","created_at":1767249000000}"#,
        );

        let build = build_entity_search(&root).expect("build identity-only rows");
        assert_eq!(build.count, 1);
        assert_eq!(build.rows.len(), 1);
        let row = &build.rows[0];
        assert_eq!(row.content, "Api Optimization-V2 (Project)");
        assert_eq!(row.path, "entity_search:api_optimization-v2");
        assert_eq!(row.facet, "");
        assert_eq!(row.day, ts_to_day(&json!(1767249000000i64)));
        assert_eq!(row.idx, 0);
        fs::remove_dir_all(root).expect("cleanup identity-only root");
    }

    #[test]
    fn malformed_identity_fields_fall_back_and_filter_aka() {
        let root = temp_root("malformed-identity-fields");
        write(
            &root,
            "entities/bad_name/entity.json",
            r#"{"name":123,"type":[],"aka":["Al",7,null]}"#,
        );
        write(
            &root,
            "entities/no_aka/entity.json",
            r#"{"name":"No Aka","type":"Person","aka":"notalist"}"#,
        );

        let build = build_entity_search(&root).expect("build malformed identity fields");
        assert_eq!(build.count, 2);
        assert_eq!(build.rows.len(), 2);
        assert_eq!(
            build.rows[0].content,
            "Bad Name (Unknown)\nAlso known as: Al"
        );
        assert_eq!(build.rows[1].content, "No Aka (Person)");
        assert!(!build.rows[1].content.contains("Also known as:"));
        fs::remove_dir_all(root).expect("cleanup malformed identity fields root");
    }

    #[test]
    fn malformed_relationship_fields_are_skipped_and_day_falls_back() {
        let root = temp_root("malformed-relationship-fields");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice","type":"Person"}"#,
        );
        write(
            &root,
            "facets/personal/entities/alice/entity.json",
            r#"{"description":null,"tags":[1,2],"last_seen":"１２３４５６７８","updated_at":1767249000000}"#,
        );
        write(
            &root,
            "facets/work/entities/alice/entity.json",
            r#"{"description":42,"tags":[1,2]}"#,
        );

        let build = build_entity_search(&root).expect("build malformed relationship fields");
        assert_eq!(build.count, 3);
        assert_eq!(build.rows.len(), 2);

        let personal = &build.rows[0];
        assert_eq!(personal.facet, "personal");
        assert_eq!(personal.content, "Alice (Person)");
        assert_eq!(personal.day, ts_to_day(&json!(1767249000000i64)));
        assert!(!personal.content.contains("Tags:"));

        let work = &build.rows[1];
        assert_eq!(work.facet, "work");
        assert_eq!(work.content, "Alice (Person)");
        assert_eq!(work.day, "");
        assert!(!work.content.contains("42"));
        assert!(!work.content.contains("Tags:"));
        fs::remove_dir_all(root).expect("cleanup malformed relationship fields root");
    }

    #[test]
    fn blocked_identities_are_skipped_and_detached_relationships_are_not_active() {
        let root = temp_root("filters");
        write(
            &root,
            "entities/blocked/entity.json",
            r#"{"name":"Blocked","type":"Person","blocked":true}"#,
        );
        write(
            &root,
            "facets/work/entities/blocked/entity.json",
            r#"{"description":"Should not index"}"#,
        );
        write(
            &root,
            "entities/active/entity.json",
            r#"{"name":"Active","type":"Person","created_at":1767249000000}"#,
        );
        write(
            &root,
            "facets/work/entities/active/entity.json",
            r#"{"description":"Detached only","detached":true}"#,
        );

        let build = build_entity_search(&root).expect("build filtered rows");
        assert_eq!(build.count, 4);
        assert_eq!(build.rows.len(), 1);
        assert_eq!(build.rows[0].path, "entity_search:active");
        assert_eq!(build.rows[0].facet, "");
        assert_eq!(build.rows[0].content, "Active (Person)");
        fs::remove_dir_all(root).expect("cleanup filters root");
    }

    #[test]
    fn orphan_relationship_counts_toward_watermark_without_chunk() {
        let root = temp_root("orphan-relationship");
        write(
            &root,
            "facets/work/entities/ghost/entity.json",
            r#"{"description":"No canonical identity exists","tags":["orphan"]}"#,
        );

        let build = build_entity_search(&root).expect("build with orphan relationship");
        assert_eq!(build.count, 1);
        assert!(build.watermark_mtime_secs > 0);
        assert!(build.rows.is_empty());
        assert!(
            !build
                .rows
                .iter()
                .any(|row| row.path == "entity_search:ghost")
        );
        fs::remove_dir_all(root).expect("cleanup orphan relationship root");
    }

    #[test]
    fn malformed_files_count_toward_watermark_without_rows() {
        let root = temp_root("malformed");
        write(&root, "entities/bad/entity.json", "{not json");
        write(&root, "facets/work/entities/bad/entity.json", "{not json");

        let build = build_entity_search(&root).expect("build with malformed files");
        assert_eq!(build.count, 2);
        assert!(build.watermark_mtime_secs > 0);
        assert_eq!(build.rows.len(), 0);
        fs::remove_dir_all(root).expect("cleanup malformed root");
    }
}
