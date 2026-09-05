// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::Response,
};
use serde_json::{Value, json};

use crate::{
    AppState,
    http::{error, import_not_found, json as json_response},
    imports::source_icon,
};

#[cfg(unix)]
const PRIVATE_IMPORT_FILE_MODE: u32 = 0o600;

fn read_jsonl(path: &Path) -> Result<Vec<Value>, std::io::Error> {
    let text = fs::read_to_string(path)?;
    Ok(text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            (!line.is_empty())
                .then(|| serde_json::from_str(line).ok())
                .flatten()
        })
        .collect())
}

fn source_type(directory: &Path) -> String {
    fs::read_to_string(directory.join("imported.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .get("source_type")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default()
}

fn backfill_type(source_type: &str) -> &'static str {
    match source_type {
        "ics" => "event",
        "kindle" => "highlight_group",
        "obsidian" => "note",
        _ => "conversation",
    }
}

fn atomic_private_write(path: &Path, data: &str) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_extension(format!("jsonl.{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(PRIVATE_IMPORT_FILE_MODE);
    use std::io::Write;
    let mut output = options.open(&temporary)?;
    output.write_all(data.as_bytes())?;
    output.sync_all()?;
    fs::rename(&temporary, path)
}

/// Backfill only the import-owned manifest; it never mutates imported payloads or chronicle data.
pub(crate) fn generate_content_manifest(
    root: &Path,
    timestamp: &str,
) -> Result<Option<PathBuf>, std::io::Error> {
    let directory = root.join("imports").join(timestamp);
    let imported_path = directory.join("imported.json");
    if !imported_path.exists() {
        return Ok(None);
    }
    let imported: Value = serde_json::from_str(&fs::read_to_string(&imported_path)?)
        .map_err(std::io::Error::other)?;
    let source_type = imported
        .get("source_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let files = imported
        .get("all_created_files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut entries = Vec::new();
    let mut index = 0usize;
    for original in files {
        let Some(original) = original.as_str() else {
            continue;
        };
        let candidate = PathBuf::from(original);
        let path = if candidate.exists() {
            candidate
        } else {
            root.join(original.trim_start_matches('/'))
        };
        if !path.exists() {
            continue;
        }
        let parts: Vec<_> = path
            .components()
            .map(|item| item.as_os_str().to_string_lossy().into_owned())
            .collect();
        let key = parts.iter().rev().nth(1).cloned().unwrap_or_default();
        let day = [parts.iter().rev().nth(3), parts.iter().rev().nth(2)]
            .into_iter()
            .flatten()
            .find(|item| item.len() == 8 && item.bytes().all(|byte| byte.is_ascii_digit()))
            .cloned()
            .unwrap_or_default();
        let segments = if !day.is_empty() && !key.is_empty() {
            json!([{"day": day, "key": key}])
        } else {
            json!([])
        };
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let lines: Vec<_> = text.trim().split('\n').collect();
            if lines.len() < 2 {
                continue;
            }
            let Ok(header) = serde_json::from_str::<Value>(lines[0]) else {
                continue;
            };
            let messages: Vec<Value> = lines[1..]
                .iter()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            if messages.is_empty() {
                continue;
            }
            let preview = messages
                .iter()
                .find(|item| item.get("speaker").and_then(Value::as_str) == Some("Human"))
                .and_then(|item| item.get("text").and_then(Value::as_str))
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>();
            let title = header
                .get("topics")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| {
                    if preview.is_empty() {
                        "Conversation segment".to_owned()
                    } else {
                        preview.chars().take(80).collect::<String>()
                    }
                });
            entries.push(json!({"id": format!("seg-{index}"), "title": title, "date": day, "type": "conversation", "preview": preview, "meta": {"message_count": messages.len()}, "segments": segments}));
            index += 1;
        } else if path.extension().is_some_and(|ext| ext == "md") {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let sections = text.strip_prefix("## ").unwrap_or(&text);
            for section in sections.split("\n## ") {
                let section = section.trim();
                if section.is_empty() {
                    continue;
                }
                let (title, body) = section.split_once('\n').unwrap_or((section, ""));
                entries.push(json!({"id": format!("item-{index}"), "title": title.trim(), "date": day, "type": backfill_type(source_type), "preview": body.trim().chars().take(200).collect::<String>(), "meta": {}, "segments": segments}));
                index += 1;
            }
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    let manifest = directory.join("content_manifest.jsonl");
    let data = entries
        .iter()
        .map(|entry| serde_json::to_string(entry).expect("manifest item serializes"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    atomic_private_write(&manifest, &data)?;
    Ok(Some(manifest))
}

fn content_manifest(root: &Path, timestamp: &str) -> Result<(PathBuf, Vec<Value>), Box<Response>> {
    let directory = root.join("imports").join(timestamp);
    if !directory.exists() {
        return Err(Box::new(import_not_found("Import not found")));
    }
    let manifest = directory.join("content_manifest.jsonl");
    if !manifest.exists() {
        match generate_content_manifest(root, timestamp) {
            Ok(Some(_)) => {}
            Ok(None) => return Err(Box::new(import_not_found("No content available"))),
            Err(_) => {
                return Err(Box::new(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "I couldn't read that import metadata.",
                    "import_metadata_failed",
                    "Failed to read manifest".to_owned(),
                )));
            }
        }
    }
    let items = read_jsonl(&manifest).map_err(|_| {
        Box::new(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't read that import metadata.",
            "import_metadata_failed",
            "Failed to read manifest".to_owned(),
        ))
    })?;
    Ok((directory, items))
}

pub(crate) async fn list(
    State(state): State<AppState>,
    AxumPath(timestamp): AxumPath<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let (directory, items) = match content_manifest(&state.root, &timestamp) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let source_type = source_type(&directory);
    let source_display =
        crate::imports::source_display(&source_type).unwrap_or_else(|| source_type.clone());
    let mut months = BTreeMap::<String, usize>::new();
    for item in &items {
        if let Some(date) = item
            .get("date")
            .and_then(Value::as_str)
            .filter(|date| date.len() >= 6)
        {
            *months.entry(date[..6].to_owned()).or_default() += 1;
        }
    }
    let month = query.get("month").map(String::as_str).unwrap_or("");
    let search = query
        .get("q")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let filtered: Vec<Value> = items
        .into_iter()
        .filter(|item| {
            let date = item.get("date").and_then(Value::as_str).unwrap_or("");
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let preview = item
                .get("preview")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            (month.is_empty() || date.starts_with(month))
                && (search.is_empty() || title.contains(&search) || preview.contains(&search))
        })
        .collect();
    let page = query
        .get("page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let per_page = query
        .get("per_page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 100);
    let total = filtered.len();
    let page_items: Vec<Value> = filtered
        .into_iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .collect();
    json_response(
        StatusCode::OK,
        json!({"items": page_items, "total": total, "page": page, "per_page": per_page, "pages": if total == 0 { 0 } else { total.div_ceil(per_page) }, "months": months, "source_type": source_type, "source_display": source_display, "source_icon_svg": source_icon(&source_type)}),
    )
}

pub(crate) async fn detail(
    State(state): State<AppState>,
    AxumPath((timestamp, item_id)): AxumPath<(String, String)>,
) -> Response {
    let (directory, items) = match content_manifest(&state.root, &timestamp) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let Some(item) = items
        .into_iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(item_id.as_str()))
    else {
        return import_not_found("Item not found");
    };
    let source_type = source_type(&directory);
    let mut content = Vec::<Value>::new();
    for segment in item
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let day = segment.get("day").and_then(Value::as_str).unwrap_or("");
        let key = segment.get("key").and_then(Value::as_str).unwrap_or("");
        if day.is_empty() || key.is_empty() {
            continue;
        }
        let path = state
            .root
            .join("chronicle")
            .join(day)
            .join(format!("import.{source_type}"))
            .join(key);
        let transcript = path.join("conversation_transcript.jsonl");
        if transcript.exists()
            && let Ok(lines) = fs::read_to_string(transcript)
        {
            content.extend(
                lines
                    .lines()
                    .skip(1)
                    .filter_map(|line| serde_json::from_str(line).ok()),
            );
        }
    }
    json_response(StatusCode::OK, json!({"item": item, "content": content}))
}
