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

fn read_jsonl(path: &Path) -> Result<Vec<Value>, std::io::Error> {
    let text = fs::read_to_string(path)?;
    let mut items = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let val: Value = serde_json::from_str(line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        items.push(val);
    }
    Ok(items)
}

fn source_type(directory: &Path) -> String {
    if let Ok(text) = fs::read_to_string(directory.join("imported.json"))
        && let Ok(value) = serde_json::from_str::<Value>(&text)
    {
        if let Some(st) = value
            .get("source_type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return st.to_owned();
        }
        if let Some(imp) = value
            .get("importer")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return imp.to_owned();
        }
    }
    if let Ok(text) = fs::read_to_string(directory.join("manifest.json"))
        && let Ok(value) = serde_json::from_str::<Value>(&text)
        && let Some(st) = value
            .get("source_type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    {
        return st.to_owned();
    }
    if let Ok(text) = fs::read_to_string(directory.join("import.json"))
        && let Ok(value) = serde_json::from_str::<Value>(&text)
        && let Some(st) = value
            .get("source_type")
            .or_else(|| value.get("source"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    {
        return st.to_owned();
    }
    String::new()
}

fn backfill_type(source_type: &str) -> &'static str {
    match source_type {
        "ics" => "event",
        "kindle" => "highlight_group",
        "obsidian" => "note",
        _ => "conversation",
    }
}

/// Project manifest rows from imported metadata without persisting a cache on read.
fn derive_content_items(
    root: &Path,
    timestamp: &str,
) -> Result<Option<Vec<Value>>, std::io::Error> {
    let directory = root.join("imports").join(timestamp);
    let imported_path = directory.join("imported.json");
    if !imported_path.exists() {
        return Ok(None);
    }
    let imported_path = contained_file(root, &imported_path)?;
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
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("referenced created file does not exist: {}", path.display()),
            ));
        }
        let path = contained_file(root, &path)?;
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
            let text = fs::read_to_string(&path)?;
            let lines: Vec<_> = text.trim().split('\n').collect();
            if lines.len() < 2 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "jsonl file has less than 2 lines",
                ));
            }
            let header: Value = serde_json::from_str(lines[0])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let mut messages: Vec<Value> = Vec::new();
            for line in &lines[1..] {
                let msg: Value = serde_json::from_str(line)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                messages.push(msg);
            }
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
            let text = fs::read_to_string(&path)?;
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
    Ok(Some(entries))
}

fn json_from_file(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn image_content_items(_root: &Path, directory: &Path) -> Option<Vec<Value>> {
    let imported_path = directory.join("imported.json");
    let imported: Value = json_from_file(&imported_path).unwrap_or(Value::Null);
    let manifest_path = directory.join("manifest.json");
    let manifest: Value = json_from_file(&manifest_path).unwrap_or(Value::Null);
    let import_meta: Value = json_from_file(&directory.join("import.json")).unwrap_or(Value::Null);

    let is_image = imported.get("importer").and_then(Value::as_str) == Some("image")
        || manifest.get("source_type").and_then(Value::as_str) == Some("image")
        || import_meta.get("source").and_then(Value::as_str) == Some("image");

    if !is_image {
        return None;
    }

    let title = import_meta
        .get("original_filename")
        .and_then(Value::as_str)
        .unwrap_or("Image import");

    let segments = imported.get("segments").and_then(Value::as_array)?.clone();

    let mut items = Vec::new();
    for (idx, seg) in segments.into_iter().enumerate() {
        let day = seg.get("day").and_then(Value::as_str).unwrap_or("");
        let key = seg.get("segment").and_then(Value::as_str).unwrap_or("");
        let stream = seg
            .get("stream")
            .and_then(Value::as_str)
            .unwrap_or("import.image");
        if !day.is_empty() && !key.is_empty() {
            items.push(json!({
                "id": format!("item_{idx}"),
                "title": title,
                "date": day,
                "type": "image",
                "stream": stream,
                "segments": [{"day": day, "key": key, "stream": stream}],
            }));
        }
    }
    if items.is_empty() { None } else { Some(items) }
}

fn content_manifest(root: &Path, timestamp: &str) -> Result<(PathBuf, Vec<Value>), Box<Response>> {
    if timestamp.is_empty() || timestamp.contains(['/', '\\']) || matches!(timestamp, "." | "..") {
        return Err(Box::new(import_not_found("Import not found")));
    }
    let directory = root.join("imports").join(timestamp);
    if !directory.exists() {
        return Err(Box::new(import_not_found("Import not found")));
    }
    let directory = contained_file(root, &directory)
        .map_err(|_| Box::new(import_not_found("Import not found")))?;
    let manifest = directory.join("content_manifest.jsonl");
    if manifest.exists() {
        let mut items = contained_file(root, &manifest)
            .and_then(|path| read_jsonl(&path))
            .map_err(|_| {
                Box::new(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "that import metadata couldn't be read.",
                    "import_metadata_failed",
                    "Failed to read manifest".to_owned(),
                ))
            })?;
        for item in &mut items {
            let item_type = item
                .get("type")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let has_stream = item.get("stream").is_some();
            let is_doc = matches!(item_type.as_deref(), Some("document" | "pdf"));

            if is_doc {
                if !has_stream {
                    item["stream"] = json!("import.document");
                }
                if let Some(segments) = item.get_mut("segments").and_then(Value::as_array_mut) {
                    for seg in segments {
                        if seg.get("stream").is_none() {
                            seg["stream"] = json!("import.document");
                        }
                    }
                }
            }
        }
        return Ok((directory, items));
    }

    if let Some(items) = image_content_items(root, &directory) {
        return Ok((directory, items));
    }

    match derive_content_items(root, timestamp) {
        Ok(Some(items)) => Ok((directory, items)),
        Ok(None) => Err(Box::new(import_not_found("No content available"))),
        Err(error_detail) => Err(Box::new(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "that import metadata couldn't be read.",
            "import_metadata_failed",
            error_detail.to_string(),
        ))),
    }
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
    let mut source_type = source_type(&directory);
    if source_type.is_empty()
        && let Some(first) = items.first()
    {
        if let Some(t) = first
            .get("type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            source_type = if t == "pdf" {
                "document".to_owned()
            } else {
                t.to_owned()
            };
        } else if let Some(stream) = first.get("stream").and_then(Value::as_str) {
            if let Some(stripped) = stream.strip_prefix("import.") {
                source_type = stripped.to_owned();
            } else {
                source_type = stream.to_owned();
            }
        }
    }
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
    let content = match read_item_content(&state.root, &source_type, &item) {
        Ok(content) => content,
        Err(detail) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "that imported content couldn't be read.",
                "import_content_failed",
                detail.to_string(),
            );
        }
    };
    json_response(StatusCode::OK, json!({"item": item, "content": content}))
}

fn contained_file(root: &Path, path: &Path) -> Result<PathBuf, std::io::Error> {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let rel_str = rel
        .to_str()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "non-utf8 path"))?;
    solstone_core_journal_io::contained_path(root, rel_str)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))
}

fn read_item_content(
    root: &Path,
    source_type: &str,
    item: &Value,
) -> Result<Vec<Value>, std::io::Error> {
    let mut content = Vec::new();
    for segment in item
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let day = segment.get("day").and_then(Value::as_str).unwrap_or("");
        let key = segment.get("key").and_then(Value::as_str).unwrap_or("");
        let stream_name = segment
            .get("stream")
            .and_then(Value::as_str)
            .or_else(|| item.get("stream").and_then(Value::as_str))
            .map(|s| {
                if s.starts_with("import.") {
                    s.to_owned()
                } else {
                    format!("import.{s}")
                }
            })
            .unwrap_or_else(|| {
                if source_type.starts_with("import.") {
                    source_type.to_owned()
                } else {
                    format!("import.{source_type}")
                }
            });

        if day.len() != 8
            || !day.bytes().all(|b| b.is_ascii_digit())
            || key.is_empty()
            || key.contains(['/', '\\'])
            || matches!(key, "." | "..")
            || stream_name.contains(['/', '\\'])
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid import segment",
            ));
        }
        let directory = root
            .join("chronicle")
            .join(day)
            .join(&stream_name)
            .join(key);
        let directory = contained_file(root, &directory)?;
        if !directory.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "missing retained content directory: {}",
                    directory.display()
                ),
            ));
        }
        let transcript = directory.join("conversation_transcript.jsonl");
        if transcript.exists() {
            let transcript = contained_file(root, &transcript)?;
            let text = fs::read_to_string(transcript)?;
            for line in text.lines().skip(1).filter(|line| !line.trim().is_empty()) {
                content.push(serde_json::from_str(line).map_err(std::io::Error::other)?);
            }
        }
        let mut markdown = fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
        markdown.sort_by_key(|entry| entry.file_name());
        for entry in markdown {
            if entry
                .file_name()
                .to_string_lossy()
                .ends_with("_transcript.md")
            {
                let path = contained_file(root, &entry.path())?;
                content.push(json!({"type": "markdown", "content": fs::read_to_string(path)?}));
            }
        }
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_derive_content_items_fails_on_missing_created_file() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let timestamp = "20260101_120000";
        let import_dir = root.join("imports").join(timestamp);
        fs::create_dir_all(&import_dir).unwrap();
        fs::write(
            import_dir.join("imported.json"),
            json!({
                "source_type": "chatgpt",
                "all_created_files": ["/chronicle/20260101/import.chatgpt/convo/item.jsonl"]
            })
            .to_string(),
        )
        .unwrap();
        let result = derive_content_items(root, timestamp);
        assert!(result.is_err(), "missing created file should fail closed");
    }

    #[test]
    fn test_derive_content_items_fails_on_corrupt_created_file() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let timestamp = "20260101_120000";
        let import_dir = root.join("imports").join(timestamp);
        fs::create_dir_all(&import_dir).unwrap();
        let file_path = root.join("chronicle/20260101/import.chatgpt/convo/item.jsonl");
        fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        fs::write(&file_path, "not valid json lines\n").unwrap();
        fs::write(
            import_dir.join("imported.json"),
            json!({
                "source_type": "chatgpt",
                "all_created_files": [file_path.to_str().unwrap()]
            })
            .to_string(),
        )
        .unwrap();
        let result = derive_content_items(root, timestamp);
        assert!(result.is_err(), "corrupt jsonl should fail closed");
    }

    #[test]
    fn a_corrupt_message_line_after_a_valid_header_also_fails_closed() {
        // The previous test is corrupt on its first line; this one is corrupt only in the
        // message lines after a valid header, which is where a lenient parser hides it.
        let temp = tempdir().unwrap();
        let root = temp.path();
        let timestamp = "20260101_120001";
        let import_dir = root.join("imports").join(timestamp);
        fs::create_dir_all(&import_dir).unwrap();
        let file_path = root.join("chronicle/20260101/import.chatgpt/convo/item.jsonl");
        fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        fs::write(
            &file_path,
            "{\"imported\":{\"id\":\"x\"}}\n{\"role\":\"user\",\"text\":\"kept\"}\n{ not json\n",
        )
        .unwrap();
        fs::write(
            import_dir.join("imported.json"),
            json!({
                "source_type": "chatgpt",
                "all_created_files": [file_path.to_str().unwrap()]
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            derive_content_items(root, timestamp).is_err(),
            "a corrupt message line must not be skipped"
        );
    }

    #[test]
    fn the_content_manifest_reader_refuses_a_corrupt_row_instead_of_dropping_it() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("content_manifest.jsonl");
        fs::write(&path, "{\"id\":\"a\"}\n{ not json\n{\"id\":\"c\"}\n").unwrap();
        let error = read_jsonl(&path).expect_err("a corrupt row must be an error");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        fs::write(&path, "{\"id\":\"a\"}\n\n{\"id\":\"c\"}\n").unwrap();
        assert_eq!(read_jsonl(&path).unwrap().len(), 2, "blank lines are fine");
    }
}
