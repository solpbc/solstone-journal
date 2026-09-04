// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-file facet ingest merges.  Each merge deliberately owns its reference contract.

use std::{fs, path::Path};

use chrono::Utc;
use serde_json::{Value, json};
use sha2::Digest;
use solstone_core_journal_io::{
    AtomicWriteOptions, append_jsonl as append_json_line, atomic_replace, contained_path,
};

#[cfg(test)]
use std::cell::Cell;
#[cfg(test)]
thread_local! {
    /// Test-only dispatch counter, thread-local rather than a process-global
    /// atomic. Cargo runs a crate's tests as parallel threads in one process, so
    /// a global counter let sibling tests increment between one test's reset and
    /// its assertion -- `received_hash_short_circuit_never_dispatches_a_merge`
    /// read 3 instead of 0 under parallel threads while passing serially. A
    /// thread-local is exclusive by construction, so no test needs to serialize.
    static DISPATCH_CALLS: Cell<usize> = const { Cell::new(0) };
}

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
    pub(crate) errors: Vec<Value>,
    pub(crate) decisions: Vec<Value>,
    pub(crate) wrote_files: bool,
}

/// Declared divergence from Python: Python resolves direct facet files from
/// `process_facet`'s explicit root but relationship/observation owner APIs
/// through an ambient journal resolver. Native has no ambient resolver;
/// `AppState.root` is its sole journal-root authority. The convey shell builds
/// this router with the same journal root that Python's ambient resolver would
/// return, so passing that root for both fields is sound. Keeping both fields
/// makes this single-root invariant visible rather than implying two live
/// native resolutions.
pub(crate) struct FacetRoots<'a> {
    pub(crate) direct: &'a Path,
    pub(crate) ambient: &'a Path,
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
    let path = staged_path(staged, facet, kind, relative)?;
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
    let path = staged_path(staged, facet, "facet_json", relative)?;
    write_value(
        &path,
        &json!({"reason":"facet_json_conflict","source_content":source,"target_content":target,"staged_at":Utc::now().to_rfc3339()}),
    )
}

fn staged_path(
    staged: &Path,
    facet: &str,
    kind: &str,
    relative: &str,
) -> Result<std::path::PathBuf, String> {
    contained_path(
        staged,
        &format!("{facet}/{kind}/{}.staged.json", relative.replace('/', "__")),
    )
    .map_err(|error| error.to_string())
}

fn contained_under(root: &Path, path: &Path) -> Result<std::path::PathBuf, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Staged facet path is outside the journal root".to_owned())?
        .to_str()
        .ok_or_else(|| "Staged facet path is not valid UTF-8".to_owned())?;
    contained_path(root, relative).map_err(|error| error.to_string())
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

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
    }
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
        .filter(|id| python_truthy(id))
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

fn parse_facet_path(path: &str, kind: &str) -> Result<(String, Option<String>), String> {
    let relative = path.trim();
    let parts = relative.split('/').collect::<Vec<_>>();
    let safe = !relative.is_empty()
        && !relative.starts_with('/')
        && parts.iter().all(|part| !matches!(*part, "" | "." | ".."));
    if !safe {
        return Err("Invalid path".into());
    }
    let day_jsonl = |part: &str| {
        part.len() == 14
            && part.ends_with(".jsonl")
            && part[..8].bytes().all(|byte| byte.is_ascii_digit())
    };
    let day_md = |part: &str| {
        part.len() == 11
            && part.ends_with(".md")
            && part[..8].bytes().all(|byte| byte.is_ascii_digit())
    };
    let output_dir = match kind {
        "facet_json" if parts == ["facet.json"] => None,
        "entity_relationship"
            if parts.len() == 3 && parts[0] == "entities" && parts[2] == "entity.json" =>
        {
            None
        }
        "entity_observations"
            if parts.len() == 3 && parts[0] == "entities" && parts[2] == "observations.jsonl" =>
        {
            None
        }
        "detected_entities"
            if parts.len() == 2 && parts[0] == "entities" && day_jsonl(parts[1]) =>
        {
            None
        }
        "activity_config" if parts == ["activities", "activities.jsonl"] => None,
        "activity_records"
            if parts.len() == 2 && parts[0] == "activities" && day_jsonl(parts[1]) =>
        {
            None
        }
        "activity_output"
            if parts.len() >= 4
                && parts[0] == "activities"
                && parts[1].len() == 8
                && parts[1].bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Some(parts[..3].join("/"))
        }
        "news" if parts.len() == 2 && parts[0] == "news" && day_md(parts[1]) => None,
        "logs" if parts.len() == 2 && parts[0] == "logs" && day_jsonl(parts[1]) => None,
        _ => return Err(format!("Invalid {kind} path")),
    };
    Ok((relative.to_owned(), output_dir))
}

fn ensure_facet_metadata(facet_dir: &Path, facet: &str) -> Result<(), String> {
    let path = contained_path(facet_dir, "facet.json").map_err(|error| error.to_string())?;
    if path.exists() {
        return Ok(());
    }
    let title = facet
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    write_value(
        &path,
        &json!({"title":title,"description":"","color":"#667eea","emoji":"📦"}),
    )
}

pub(crate) fn process_facet(
    roots: FacetRoots<'_>,
    facet: &str,
    items: &[FacetItem<'_>],
    staged: &Path,
    id_map: &serde_json::Map<String, Value>,
    received: &mut serde_json::Map<String, Value>,
) -> Result<ProcessResult, String> {
    let facet_dir = contained_path(roots.direct, &format!("facets/{facet}"))
        .map_err(|error| error.to_string())?;
    let ambient_facet_dir = contained_path(roots.ambient, &format!("facets/{facet}"))
        .map_err(|error| error.to_string())?;
    let staged = contained_under(roots.direct, staged)?;
    // This is intentionally the sole latch computation. Every merge receives this value.
    let new_facet = !facet_dir.exists();
    let mut result = ProcessResult {
        created: 0,
        merged: 0,
        skipped: 0,
        staged: 0,
        errors: Vec::new(),
        decisions: Vec::new(),
        wrote_files: false,
    };
    for item in items {
        let raw_path = item.path;
        let item_id = format!("{facet}/{raw_path}");
        let digest = format!("{:x}", sha2::Sha256::digest(item.bytes));
        // This guard is above dispatch: no target inspection or merge call can occur on replay.
        if received.get(&item_id).and_then(Value::as_str) == Some(&digest) {
            result.skipped += 1;
            result.decisions.push(json!({"ts":Utc::now().to_rfc3339(),"action":"facet_file_skipped","item_type":item.kind,"item_id":item_id,"facet":facet,"reason":"idempotent"}));
            continue;
        }
        #[cfg(test)]
        DISPATCH_CALLS.with(|calls| calls.set(calls.get() + 1));
        let item_result = (|| -> Result<(), String> {
            let (mut relative, output_dir) = parse_facet_path(raw_path, item.kind)?;
            let item_id = format!("{facet}/{relative}");
            let mut bytes = item.bytes.to_vec();
            if let Some(unmapped) = unmapped_entity(item.kind, &relative, &bytes, id_map)? {
                stage_unmapped_entity(
                    &staged,
                    facet,
                    item.kind,
                    &relative,
                    &unmapped,
                    std::str::from_utf8(&bytes).map_err(|error| error.to_string())?,
                )?;
                received.insert(item_id.clone(), json!(digest));
                result.staged += 1;
                result.decisions.push(json!({"ts":Utc::now().to_rfc3339(),"action":"facet_file_staged","item_type":item.kind,"item_id":item_id,"facet":facet,"reason":"unmapped_entity","staged_path":staged_path(&staged, facet, item.kind, &relative)?}));
                return Ok(());
            }
            remap_entity_ids(item.kind, &mut relative, &mut bytes, id_map)?;
            let target_root = if matches!(item.kind, "entity_relationship" | "entity_observations")
            {
                &ambient_facet_dir
            } else {
                &facet_dir
            };
            let target =
                contained_path(target_root, &relative).map_err(|error| error.to_string())?;
            let merge = match item.kind {
                "facet_json" => {
                    merge_facet_json(&target, &bytes, new_facet, &staged, facet, &relative)
                }
                "entity_relationship" => merge_entity_relationship(&target, &bytes, new_facet),
                "entity_observations" => merge_observations(&target, &bytes, new_facet),
                "detected_entities" => merge_detected_entities(&target, &bytes, new_facet),
                "activity_config" => merge_activity_config(&target, &bytes, new_facet),
                "activity_records" => merge_activity_records(&target, &bytes, new_facet),
                "activity_output" => merge_activity_output(
                    &target,
                    &bytes,
                    &contained_path(&facet_dir, &output_dir.expect("activity output grammar"))
                        .map_err(|error| error.to_string())?,
                    new_facet,
                ),
                "news" => merge_news(&target, &bytes, new_facet),
                "logs" => merge_logs(&target, &bytes, new_facet),
                _ => unreachable!("path grammar accepts only known kinds"),
            }?;
            received.insert(item_id.clone(), json!(digest));
            match merge.status {
                "staged" => result.staged += 1,
                "skipped" => result.skipped += 1,
                "written" if new_facet => result.created += 1,
                "written" => result.merged += 1,
                _ => {}
            }
            if merge.status == "written" {
                result.wrote_files = true;
            }
            let action = match merge.status {
                "staged" => "facet_file_staged",
                "skipped" => "facet_file_skipped",
                "written" if new_facet => "facet_file_created",
                "written" => "facet_file_merged",
                _ => unreachable!("facet merge status"),
            };
            result.decisions.push(json!({"ts":Utc::now().to_rfc3339(),"action":action,"item_type":item.kind,"item_id":item_id,"facet":facet,"reason":merge.reason}));
            Ok(())
        })();
        if let Err(reason) = item_result {
            result.decisions.push(json!({"ts":Utc::now().to_rfc3339(),"action":"facet_file_error","item_type":item.kind,"item_id":item_id,"facet":facet,"reason":reason}));
            result
                .errors
                .push(json!({"facet":facet,"path":raw_path,"error":reason}));
        }
    }
    if result.wrote_files {
        ensure_facet_metadata(&facet_dir, facet)?;
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
        DISPATCH_CALLS, FacetItem, FacetRoots, merge_activity_config, merge_activity_output,
        merge_activity_records, merge_detected_entities, merge_entity_relationship,
        merge_facet_json, merge_logs, merge_news, merge_observations, process_facet,
    };
    use serde_json::{Value, json};
    use sha2::Digest;
    use std::cell::Cell;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
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
        for falsy_id in ["null", "false", "0", "\"\""] {
            let bytes = format!("{{\"id\":{falsy_id},\"from\":\"source\"}}\n");
            let owner = format!("{{\"id\":{falsy_id},\"from\":\"owner\"}}\n");
            for new_facet in [false, true] {
                for (name, merge, expected) in [
                    (
                        "detected",
                        merge_detected_entities
                            as fn(
                                &std::path::Path,
                                &[u8],
                                bool,
                            )
                                -> Result<super::MergeResult, String>,
                        2,
                    ),
                    (
                        "config",
                        merge_activity_config
                            as fn(
                                &std::path::Path,
                                &[u8],
                                bool,
                            )
                                -> Result<super::MergeResult, String>,
                        1,
                    ),
                    (
                        "records",
                        merge_activity_records
                            as fn(
                                &std::path::Path,
                                &[u8],
                                bool,
                            )
                                -> Result<super::MergeResult, String>,
                        1,
                    ),
                ] {
                    let target = temp
                        .path()
                        .join(format!("{name}-{falsy_id:?}-{new_facet}.jsonl"));
                    if !new_facet {
                        fs::write(&target, &owner).unwrap();
                    }
                    merge(&target, bytes.as_bytes(), new_facet).unwrap();
                    let count = fs::read_to_string(target).unwrap().lines().count();
                    assert_eq!(
                        count,
                        if new_facet { 1 } else { expected },
                        "{name} {falsy_id} {new_facet}"
                    );
                }
            }
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
            FacetRoots {
                direct: temp.path(),
                ambient: temp.path(),
            },
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
        DISPATCH_CALLS.with(|calls| calls.set(0));
        let result = process_facet(
            FacetRoots {
                direct: temp.path(),
                ambient: temp.path(),
            },
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
        assert_eq!(DISPATCH_CALLS.with(Cell::get), 0);
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
            FacetRoots {
                direct: journal,
                ambient: journal,
            },
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
            FacetRoots {
                direct: temp.path(),
                ambient: temp.path(),
            },
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

    #[test]
    fn relationship_owner_merge_uses_the_explicit_ambient_journal_root() {
        let direct = TempDir::new().unwrap();
        let ambient = TempDir::new().unwrap();
        fs::create_dir_all(direct.path().join("facets/work")).unwrap();
        let mut received = serde_json::Map::new();
        process_facet(
            FacetRoots {
                direct: direct.path(),
                ambient: ambient.path(),
            },
            "work",
            &[FacetItem {
                path: "entities/ada/entity.json",
                kind: "entity_relationship",
                bytes: br#"{"source":true}"#,
            }],
            &direct.path().join("state/staged"),
            &serde_json::Map::from_iter([("ada".to_owned(), json!("ada"))]),
            &mut received,
        )
        .unwrap();
        assert!(
            ambient
                .path()
                .join("facets/work/entities/ada/entity.json")
                .exists()
        );
        assert!(
            !direct
                .path()
                .join("facets/work/entities/ada/entity.json")
                .exists()
        );
    }

    #[test]
    fn malformed_facet_file_logs_an_error_and_later_files_continue_with_default_metadata() {
        let root = TempDir::new().unwrap();
        let mut received = serde_json::Map::new();
        let result = process_facet(
            FacetRoots {
                direct: root.path(),
                ambient: root.path(),
            },
            "work-notes",
            &[
                FacetItem {
                    path: "logs/not-a-day.jsonl",
                    kind: "logs",
                    bytes: b"{\"message\":\"broken\"}\n",
                },
                FacetItem {
                    path: "logs/20260101.jsonl",
                    kind: "logs",
                    bytes: b"{\"message\":\"kept\"}\n",
                },
            ],
            &root.path().join("state/staged"),
            &serde_json::Map::new(),
            &mut received,
        )
        .unwrap();
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.decisions[0]["action"], "facet_file_error");
        assert_eq!(result.decisions[1]["action"], "facet_file_created");
        assert_eq!(
            fs::read_to_string(root.path().join("facets/work-notes/logs/20260101.jsonl")).unwrap(),
            "{\"message\":\"kept\"}\n"
        );
        let metadata: Value = serde_json::from_slice(
            &fs::read(root.path().join("facets/work-notes/facet.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(metadata["title"], "Work Notes");
        assert_eq!(metadata["emoji"], "📦");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_facet_directory_is_refused_without_touching_the_outside_tree() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("facets")).unwrap();
        symlink(outside.path(), root.path().join("facets/work")).unwrap();
        let mut received = serde_json::Map::new();

        let result = process_facet(
            FacetRoots {
                direct: root.path(),
                ambient: root.path(),
            },
            "work",
            &[FacetItem {
                path: "logs/20260101.jsonl",
                kind: "logs",
                bytes: b"{\"message\":\"must not escape\"}\n",
            }],
            &root.path().join("imports/prefix/facets/staged"),
            &serde_json::Map::new(),
            &mut received,
        );

        assert!(matches!(result, Err(error) if error.contains("escapes")));
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }
}
