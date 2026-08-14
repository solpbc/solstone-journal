// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-file facet ingest merges.  Each merge deliberately owns its reference contract.

use std::{fs, path::Path};

use chrono::Utc;
use serde_json::{Value, json};
use sha2::Digest;
use solstone_core_journal_io::{
    AtomicWriteOptions, append_jsonl as append_json_line, atomic_replace,
};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
static DISPATCH_CALLS: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct MergeResult {
    pub(crate) status: &'static str,
    pub(crate) reason: &'static str,
}
pub(crate) struct FacetItem<'a> {
    pub(crate) path: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) bytes: &'a [u8],
}
pub(crate) struct ProcessResult {
    pub(crate) created: usize,
    pub(crate) merged: usize,
    pub(crate) skipped: usize,
    pub(crate) staged: usize,
    pub(crate) decisions: Vec<Value>,
}

pub(crate) fn append_jsonl(path: &Path, items: &[Value]) -> Result<(), String> {
    fs::create_dir_all(path.parent().expect("facet item parent"))
        .map_err(|error| error.to_string())?;
    for item in items {
        append_json_line(path, item).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(path.parent().expect("facet item parent"))
        .map_err(|error| error.to_string())?;
    atomic_replace(path, bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(|error| error.to_string())
}

pub(crate) fn stage_unmapped_entity(
    staged: &Path,
    facet: &str,
    kind: &str,
    relative: &str,
    entity_id: &str,
    source_data: &str,
) -> Result<(), String> {
    let path = staged
        .join(facet)
        .join(kind)
        .join(format!("{}.staged.json", relative.replace('/', "__")));
    write_value(
        &path,
        &json!({"reason":"unmapped_entity","source_entity_id":entity_id,"explanation":format!("Entity '{entity_id}' has no mapping in entities/state.json id_map"),"source_path":relative,"source_data":source_data,"staged_at":Utc::now().to_rfc3339()}),
    )
}

pub(crate) fn stage_facet_json_conflict(
    staged: &Path,
    facet: &str,
    relative: &str,
    source: &Value,
    target: &Value,
) -> Result<(), String> {
    let path = staged
        .join(facet)
        .join("facet_json")
        .join(format!("{}.staged.json", relative.replace('/', "__")));
    write_value(
        &path,
        &json!({"reason":"facet_json_conflict","source_content":source,"target_content":target,"staged_at":Utc::now().to_rfc3339()}),
    )
}

fn write_value(path: &Path, value: &Value) -> Result<(), String> {
    write_bytes(
        path,
        &serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
}

pub(crate) fn merge_facet_json(
    target: &Path,
    bytes: &[u8],
    new_facet: bool,
    staged: &Path,
    facet: &str,
    relative: &str,
) -> Result<MergeResult, String> {
    let source: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if new_facet || !target.exists() {
        write_bytes(target, bytes)?;
        return Ok(MergeResult {
            status: "written",
            reason: if new_facet {
                "new_facet"
            } else {
                "overlap_merged"
            },
        });
    }
    let owner: Value =
        serde_json::from_slice(&fs::read(target).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if owner == source {
        return Ok(MergeResult {
            status: "skipped",
            reason: "facet_json_match",
        });
    }
    stage_facet_json_conflict(staged, facet, relative, &source, &owner)?;
    Ok(MergeResult {
        status: "staged",
        reason: "facet_json_conflict",
    })
}

pub(crate) fn merge_entity_relationship(
    target: &Path,
    bytes: &[u8],
    new_facet: bool,
) -> Result<MergeResult, String> {
    let mut source: serde_json::Map<String, Value> =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if !new_facet
        && let Ok(Value::Object(owner)) =
            serde_json::from_slice(&fs::read(target).unwrap_or_default())
    {
        source.extend(owner);
    }
    write_value(target, &Value::Object(source))?;
    Ok(MergeResult {
        status: "written",
        reason: if new_facet {
            "new_facet"
        } else {
            "overlap_merged"
        },
    })
}

pub(crate) fn merge_observations(
    target: &Path,
    bytes: &[u8],
    new_facet: bool,
) -> Result<MergeResult, String> {
    let source = parse_jsonl(bytes)?;
    let mut merged = if new_facet {
        Vec::new()
    } else {
        parse_jsonl(&fs::read(target).unwrap_or_default())?
    };
    for item in source {
        if !merged.iter().any(|owner| {
            owner.get("content") == item.get("content")
                && owner.get("observed_at") == item.get("observed_at")
        }) {
            merged.push(item);
        }
    }
    write_bytes(target, &serialize_jsonl(&merged))?;
    Ok(MergeResult {
        status: "written",
        reason: if new_facet {
            "new_facet"
        } else {
            "overlap_merged"
        },
    })
}

fn parse_jsonl(bytes: &[u8]) -> Result<Vec<Value>, String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|error| error.to_string()))
        .collect()
}
fn serialize_jsonl(items: &[Value]) -> Vec<u8> {
    items
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .expect("JSON values serialize")
        .join("\n")
        .into_bytes()
}

pub(crate) fn merge_detected_entities(
    target: &Path,
    bytes: &[u8],
    new_facet: bool,
) -> Result<MergeResult, String> {
    let source = parse_jsonl(bytes)?;
    let owner = if new_facet {
        Vec::new()
    } else {
        parse_jsonl(&fs::read(target).unwrap_or_default())?
    };
    let seen = owner
        .iter()
        .filter_map(|item| item.get("id"))
        .filter(|id| !id.is_null() && id.as_str().is_some_and(|id| !id.is_empty()))
        .collect::<Vec<_>>();
    let append = source
        .into_iter()
        .filter(|item| !seen.iter().any(|id| Some(*id) == item.get("id")))
        .collect::<Vec<_>>();
    append_jsonl(target, &append)?;
    Ok(MergeResult {
        status: "written",
        reason: if new_facet {
            "new_facet"
        } else {
            "overlap_merged"
        },
    })
}

pub(crate) fn merge_activity_config(
    target: &Path,
    bytes: &[u8],
    new_facet: bool,
) -> Result<MergeResult, String> {
    merge_activity_id_items(target, bytes, new_facet)
}
pub(crate) fn merge_activity_records(
    target: &Path,
    bytes: &[u8],
    new_facet: bool,
) -> Result<MergeResult, String> {
    merge_activity_id_items(target, bytes, new_facet)
}
fn merge_activity_id_items(
    target: &Path,
    bytes: &[u8],
    new_facet: bool,
) -> Result<MergeResult, String> {
    let source = parse_jsonl(bytes)?;
    let owner = if new_facet {
        Vec::new()
    } else {
        parse_jsonl(&fs::read(target).unwrap_or_default())?
    };
    let append = source
        .into_iter()
        .filter(|item| !owner.iter().any(|old| old.get("id") == item.get("id")))
        .collect::<Vec<_>>();
    append_jsonl(target, &append)?;
    Ok(MergeResult {
        status: "written",
        reason: if new_facet {
            "new_facet"
        } else {
            "overlap_merged"
        },
    })
}

pub(crate) fn merge_todos(
    target: &Path,
    bytes: &[u8],
    new_facet: bool,
) -> Result<MergeResult, String> {
    let source = parse_jsonl(bytes)?;
    let owner = if new_facet {
        Vec::new()
    } else {
        parse_jsonl(&fs::read(target).unwrap_or_default())?
    };
    for item in owner.iter().chain(source.iter()) {
        if item.get("text").is_none() {
            return Err("todo item is missing text".into());
        }
    }
    let append = source
        .into_iter()
        .filter(|item| {
            !owner.iter().any(|old| {
                old["text"] == item["text"] && old.get("created_at") == item.get("created_at")
            })
        })
        .collect::<Vec<_>>();
    append_jsonl(target, &append)?;
    Ok(MergeResult {
        status: "written",
        reason: if new_facet {
            "new_facet"
        } else {
            "overlap_merged"
        },
    })
}

pub(crate) fn merge_logs(
    target: &Path,
    bytes: &[u8],
    new_facet: bool,
) -> Result<MergeResult, String> {
    let items = parse_jsonl(bytes)?;
    append_jsonl(target, &items)?;
    Ok(MergeResult {
        status: "written",
        reason: if new_facet {
            "new_facet"
        } else {
            "overlap_merged"
        },
    })
}

pub(crate) fn merge_activity_output(
    target: &Path,
    bytes: &[u8],
    output_dir: &Path,
    new_facet: bool,
) -> Result<MergeResult, String> {
    if output_dir.exists() {
        return Ok(MergeResult {
            status: "skipped",
            reason: "output_dir_exists",
        });
    }
    write_bytes(target, bytes)?;
    Ok(MergeResult {
        status: "written",
        reason: if new_facet {
            "new_facet"
        } else {
            "overlap_merged"
        },
    })
}
pub(crate) fn merge_news(
    target: &Path,
    bytes: &[u8],
    new_facet: bool,
) -> Result<MergeResult, String> {
    if target.exists() {
        return Ok(MergeResult {
            status: "skipped",
            reason: "news_exists",
        });
    }
    write_bytes(target, bytes)?;
    Ok(MergeResult {
        status: "written",
        reason: if new_facet {
            "new_facet"
        } else {
            "overlap_merged"
        },
    })
}

pub(crate) fn process_facet(
    journal_root: &Path,
    facet: &str,
    items: &[FacetItem<'_>],
    staged: &Path,
    id_map: &serde_json::Map<String, Value>,
    received: &mut serde_json::Map<String, Value>,
) -> Result<ProcessResult, String> {
    let facet_dir = journal_root.join("facets").join(facet);
    // This is intentionally the sole latch computation. Every merge receives this value.
    let new_facet = !facet_dir.exists();
    let mut result = ProcessResult {
        created: 0,
        merged: 0,
        skipped: 0,
        staged: 0,
        decisions: Vec::new(),
    };
    for item in items {
        let item_id = format!("{facet}/{}", item.path);
        let digest = format!("{:x}", sha2::Sha256::digest(item.bytes));
        // This guard is above dispatch: no target inspection or merge call can occur on replay.
        if received.get(&item_id).and_then(Value::as_str) == Some(&digest) {
            result.skipped += 1;
            result.decisions.push(json!({"ts":Utc::now().to_rfc3339(),"action":"facet_file_skipped","item_type":item.kind,"item_id":item_id,"facet":facet,"reason":"idempotent"}));
            continue;
        }
        #[cfg(test)]
        DISPATCH_CALLS.fetch_add(1, Ordering::Relaxed);
        let mut relative = item.path.to_owned();
        let mut bytes = item.bytes.to_vec();
        if let Some(unmapped) = unmapped_entity(item.kind, &relative, &bytes, id_map)? {
            stage_unmapped_entity(
                staged,
                facet,
                item.kind,
                &relative,
                &unmapped,
                std::str::from_utf8(&bytes).map_err(|error| error.to_string())?,
            )?;
            received.insert(item_id.clone(), json!(digest));
            result.staged += 1;
            result.decisions.push(json!({"ts":Utc::now().to_rfc3339(),"action":"facet_file_staged","item_type":item.kind,"item_id":item_id,"facet":facet,"reason":"unmapped_entity","staged_path":staged.join(facet).join(item.kind).join(format!("{}.staged.json", relative.replace('/', "__")))}));
            continue;
        }
        remap_entity_ids(item.kind, &mut relative, &mut bytes, id_map)?;
        let target = facet_dir.join(&relative);
        let merge = match item.kind {
            "facet_json" => merge_facet_json(&target, &bytes, new_facet, staged, facet, &relative),
            "entity_relationship" => merge_entity_relationship(&target, &bytes, new_facet),
            "entity_observations" => merge_observations(&target, &bytes, new_facet),
            "detected_entities" => merge_detected_entities(&target, &bytes, new_facet),
            "activity_config" => merge_activity_config(&target, &bytes, new_facet),
            "activity_records" => merge_activity_records(&target, &bytes, new_facet),
            "activity_output" => merge_activity_output(
                &target,
                &bytes,
                target.parent().expect("activity output parent"),
                new_facet,
            ),
            "todos" => merge_todos(&target, &bytes, new_facet),
            "news" => merge_news(&target, &bytes, new_facet),
            "logs" => merge_logs(&target, &bytes, new_facet),
            _ => return Err(format!("Unsupported file type: {}", item.kind)),
        }?;
        received.insert(item_id.clone(), json!(digest));
        match merge.status {
            "staged" => result.staged += 1,
            "skipped" => result.skipped += 1,
            "written" if new_facet => result.created += 1,
            "written" => result.merged += 1,
            _ => {}
        }
        let action = match merge.status {
            "staged" => "facet_file_staged",
            "skipped" => "facet_file_skipped",
            "written" if new_facet => "facet_file_created",
            "written" => "facet_file_merged",
            _ => unreachable!("facet merge status"),
        };
        result.decisions.push(json!({"ts":Utc::now().to_rfc3339(),"action":action,"item_type":item.kind,"item_id":item_id,"facet":facet,"reason":merge.reason}));
    }
    Ok(result)
}

fn unmapped_entity(
    kind: &str,
    relative: &str,
    bytes: &[u8],
    id_map: &serde_json::Map<String, Value>,
) -> Result<Option<String>, String> {
    let missing = |id: &str| (!id.is_empty() && !id_map.contains_key(id)).then(|| id.to_owned());
    let path_entity = || {
        relative
            .split('/')
            .nth(1)
            .filter(|_| relative.starts_with("entities/"))
    };
    match kind {
        "entity_relationship" | "entity_observations" => Ok(path_entity().and_then(missing)),
        "detected_entities" => Ok(parse_jsonl(bytes)?
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str))
            .find_map(missing)),
        "activity_records" => Ok(parse_jsonl(bytes)?
            .iter()
            .filter_map(|item| item.get("active_entities").and_then(Value::as_array))
            .flatten()
            .filter_map(Value::as_str)
            .find_map(missing)),
        _ => Ok(None),
    }
}

fn remap_entity_ids(
    kind: &str,
    relative: &mut String,
    bytes: &mut Vec<u8>,
    id_map: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    let mapped = |id: &str| {
        id_map
            .get(id)
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_owned()
    };
    match kind {
        "entity_relationship" | "entity_observations" => {
            if let Some(source_id) = relative
                .split('/')
                .nth(1)
                .filter(|_| relative.starts_with("entities/"))
                .map(str::to_owned)
            {
                let target_id = mapped(&source_id);
                *relative = relative.replacen(&source_id, &target_id, 1);
                if kind == "entity_relationship" {
                    let mut relationship: serde_json::Map<String, Value> =
                        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
                    relationship.insert("entity_id".into(), json!(target_id));
                    *bytes = serde_json::to_vec_pretty(&relationship)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        "detected_entities" | "activity_records" => {
            let mut items = parse_jsonl(bytes)?;
            for item in &mut items {
                if let Some(id) = item.get("id").and_then(Value::as_str).map(str::to_owned) {
                    item["id"] = json!(mapped(&id));
                }
                if let Some(ids) = item
                    .get_mut("active_entities")
                    .and_then(Value::as_array_mut)
                {
                    for id in ids {
                        if let Some(source) = id.as_str().map(str::to_owned) {
                            *id = json!(mapped(&source));
                        }
                    }
                }
            }
            *bytes = serialize_jsonl(&items);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DISPATCH_CALLS, FacetItem, merge_activity_config, merge_activity_output,
        merge_activity_records, merge_detected_entities, merge_entity_relationship,
        merge_facet_json, merge_logs, merge_news, merge_observations, merge_todos, process_facet,
    };
    use serde_json::{Value, json};
    use sha2::Digest;
    use std::fs;
    use std::sync::atomic::Ordering;
    use tempfile::TempDir;

    #[test]
    fn merge_facet_json_keeps_or_stages_owner_content_and_new_latch_discards_it() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("facets/work/facet.json");
        let staged = temp.path().join("state/staged");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, br#"{"owner":true}"#).unwrap();
        let merged = merge_facet_json(
            &target,
            br#"{"source":true}"#,
            false,
            &staged,
            "work",
            "facet.json",
        )
        .unwrap();
        assert_eq!(
            (merged.status, merged.reason),
            ("staged", "facet_json_conflict")
        );
        assert_eq!(fs::read(&target).unwrap(), br#"{"owner":true}"#);
        let same = merge_facet_json(
            &target,
            br#"{"owner":true}"#,
            false,
            &staged,
            "work",
            "facet.json",
        )
        .unwrap();
        assert_eq!((same.status, same.reason), ("skipped", "facet_json_match"));
        let new = merge_facet_json(
            &target,
            br#"{"source":true}"#,
            true,
            &staged,
            "work",
            "facet.json",
        )
        .unwrap();
        assert_eq!((new.status, new.reason), ("written", "new_facet"));
        assert_eq!(fs::read(&target).unwrap(), br#"{"source":true}"#);
        let conflict: Value = serde_json::from_slice(
            &fs::read(staged.join("work/facet_json/facet.json.staged.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(conflict["source_content"], json!({"source":true}));
    }

    #[test]
    fn relationship_merge_is_target_wins_and_new_facet_is_source_only() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("entities/ada/entity.json");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, br#"{"shared":"owner","owner":true}"#).unwrap();
        merge_entity_relationship(&target, br#"{"shared":"source","source":true}"#, false).unwrap();
        let merged: Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        assert_eq!(merged, json!({"shared":"owner","source":true,"owner":true}));
        merge_entity_relationship(&target, br#"{"shared":"source","source":true}"#, true).unwrap();
        let new: Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        assert_eq!(new, json!({"shared":"source","source":true}));
    }

    #[test]
    fn observations_merge_dedupes_owner_history_but_new_facet_discards_it() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("entities/ada/observations.jsonl");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "{\"content\":\"owner\",\"observed_at\":1}\n").unwrap();
        merge_observations(&target, b"{\"content\":\"owner\",\"observed_at\":1}\n{\"content\":\"source\",\"observed_at\":2}\n", false).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap().lines().count(), 2);
        merge_observations(
            &target,
            b"{\"content\":\"source\",\"observed_at\":2}\n",
            true,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "{\"content\":\"source\",\"observed_at\":2}"
        );
    }

    #[test]
    fn falsy_id_divergence_is_preserved_for_both_latch_values() {
        let temp = TempDir::new().unwrap();
        let bytes = b"{\"id\":null,\"from\":\"source\"}\n";
        for new_facet in [false, true] {
            for (name, merge, expected) in [
                (
                    "detected",
                    merge_detected_entities
                        as fn(&std::path::Path, &[u8], bool) -> Result<super::MergeResult, String>,
                    2,
                ),
                (
                    "config",
                    merge_activity_config
                        as fn(&std::path::Path, &[u8], bool) -> Result<super::MergeResult, String>,
                    1,
                ),
                (
                    "records",
                    merge_activity_records
                        as fn(&std::path::Path, &[u8], bool) -> Result<super::MergeResult, String>,
                    1,
                ),
            ] {
                let target = temp.path().join(format!("{name}-{new_facet}.jsonl"));
                if !new_facet {
                    fs::write(&target, "{\"id\":null,\"from\":\"owner\"}\n").unwrap();
                }
                merge(&target, bytes, new_facet).unwrap();
                let count = fs::read_to_string(target).unwrap().lines().count();
                assert_eq!(
                    count,
                    if new_facet { 1 } else { expected },
                    "{name} {new_facet}"
                );
            }
        }
    }

    #[test]
    fn todos_require_text_for_both_latch_values() {
        let temp = TempDir::new().unwrap();
        for new_facet in [false, true] {
            let target = temp.path().join(format!("todos-{new_facet}.jsonl"));
            assert_eq!(
                merge_todos(&target, b"{}\n", new_facet).err().as_deref(),
                Some("todo item is missing text")
            );
        }
    }

    #[test]
    fn logs_append_exact_duplicates_for_both_latch_values() {
        let temp = TempDir::new().unwrap();
        for new_facet in [false, true] {
            let target = temp.path().join(format!("logs-{new_facet}.jsonl"));
            fs::write(&target, "{\"event\":\"same\"}\n").unwrap();
            merge_logs(&target, b"{\"event\":\"same\"}\n", new_facet).unwrap();
            assert_eq!(fs::read_to_string(target).unwrap().lines().count(), 2);
        }
    }

    #[test]
    fn activity_output_and_news_keep_existing_owner_bytes_for_both_latches() {
        let temp = TempDir::new().unwrap();
        for new_facet in [false, true] {
            let output = temp.path().join(format!("output-{new_facet}/child/file"));
            fs::create_dir_all(output.parent().unwrap()).unwrap();
            fs::write(&output, b"owner").unwrap();
            let before = fs::read(&output).unwrap();
            let result =
                merge_activity_output(&output, b"source", output.parent().unwrap(), new_facet)
                    .unwrap();
            assert_eq!(
                (result.status, result.reason),
                ("skipped", "output_dir_exists")
            );
            assert_eq!(fs::read(&output).unwrap(), before);
            let news = temp.path().join(format!("news-{new_facet}.md"));
            fs::write(&news, b"owner").unwrap();
            let before = fs::read(&news).unwrap();
            let result = merge_news(&news, b"source", new_facet).unwrap();
            assert_eq!((result.status, result.reason), ("skipped", "news_exists"));
            assert_eq!(fs::read(&news).unwrap(), before);
        }
    }

    #[test]
    fn process_latches_new_facet_before_the_first_item_creates_its_directory() {
        let temp = TempDir::new().unwrap();
        let mut received = serde_json::Map::new();
        let items = [
            FacetItem {
                path: "facet.json",
                kind: "facet_json",
                bytes: br#"{"first":true}"#,
            },
            FacetItem {
                path: "entities/ada/entity.json",
                kind: "entity_relationship",
                bytes: br#"{"second":true}"#,
            },
        ];
        let result = process_facet(
            temp.path(),
            "work",
            &items,
            &temp.path().join("state/staged"),
            &serde_json::Map::from_iter([("ada".to_owned(), json!("ada"))]),
            &mut received,
        )
        .unwrap();
        assert_eq!(
            (result.created, result.merged),
            (2, 0),
            "the second item receives the batch's immutable new_facet=true latch"
        );
    }

    #[test]
    fn received_hash_short_circuit_never_dispatches_a_merge() {
        let temp = TempDir::new().unwrap();
        let bytes = br#"{"ignored":true}"#;
        let digest = format!("{:x}", sha2::Sha256::digest(bytes));
        let mut received =
            serde_json::Map::from_iter([("work/facet.json".to_owned(), json!(digest))]);
        DISPATCH_CALLS.store(0, Ordering::Relaxed);
        let result = process_facet(
            temp.path(),
            "work",
            &[FacetItem {
                path: "facet.json",
                kind: "facet_json",
                bytes,
            }],
            &temp.path().join("state/staged"),
            &serde_json::Map::new(),
            &mut received,
        )
        .unwrap();
        assert_eq!(result.skipped, 1);
        assert_eq!(DISPATCH_CALLS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn process_returns_owner_visible_decisions_for_written_staged_and_skipped_items() {
        let temp = TempDir::new().unwrap();
        let journal = temp.path();
        let target = journal.join("facets/work/facet.json");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, br#"{"owner":true}"#).unwrap();
        let skipped = br#"{"already":true}"#;
        let digest = format!("{:x}", sha2::Sha256::digest(skipped));
        let mut received =
            serde_json::Map::from_iter([("work/news/20260101.md".to_owned(), json!(digest))]);
        let items = [
            FacetItem {
                path: "facet.json",
                kind: "facet_json",
                bytes: br#"{"source":true}"#,
            },
            FacetItem {
                path: "logs/20260101.jsonl",
                kind: "logs",
                bytes: b"{\"event\":1}\n",
            },
            FacetItem {
                path: "news/20260101.md",
                kind: "news",
                bytes: skipped,
            },
        ];
        let result = process_facet(
            journal,
            "work",
            &items,
            &journal.join("state/staged"),
            &serde_json::Map::new(),
            &mut received,
        )
        .unwrap();
        assert_eq!(
            result
                .decisions
                .iter()
                .map(|entry| (
                    entry["action"].as_str().unwrap(),
                    entry["reason"].as_str().unwrap()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("facet_file_staged", "facet_json_conflict"),
                ("facet_file_merged", "overlap_merged"),
                ("facet_file_skipped", "idempotent")
            ]
        );
    }

    #[test]
    fn process_stages_an_unmapped_entity_before_any_owner_merge() {
        let temp = TempDir::new().unwrap();
        let staged = temp.path().join("imports/prefix/facets/staged");
        let mut received = serde_json::Map::new();
        let item = FacetItem {
            path: "entities/source-person/entity.json",
            kind: "entity_relationship",
            bytes: br#"{"relationship":"source"}"#,
        };

        let result = process_facet(
            temp.path(),
            "work",
            &[item],
            &staged,
            &serde_json::Map::new(),
            &mut received,
        )
        .unwrap();
        assert_eq!((result.staged, result.merged, result.created), (1, 0, 0));
        assert!(
            !temp
                .path()
                .join("facets/work/entities/source-person/entity.json")
                .exists()
        );
        let proposal: Value =
            serde_json::from_slice(
                &fs::read(staged.join(
                    "work/entity_relationship/entities__source-person__entity.json.staged.json",
                ))
                .unwrap(),
            )
            .unwrap();
        assert_eq!(proposal["reason"], "unmapped_entity");
        assert_eq!(proposal["source_entity_id"], "source-person");
        assert_eq!(result.decisions[0]["action"], "facet_file_staged");
    }
}
