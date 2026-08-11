// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::JournalStatsError;

#[derive(Debug, Default)]
pub(crate) struct DailyOutputCounts {
    pub processed: u64,
    pub pending: u64,
}

pub(crate) fn daily_output_counts(
    day_dir: &Path,
    system_root: &Path,
    apps_root: &Path,
) -> Result<DailyOutputCounts, JournalStatsError> {
    let mut configs = Vec::new();
    discover_system_configs(system_root, &mut configs)?;
    discover_app_configs(apps_root, &mut configs)?;
    let mut counts = DailyOutputCounts::default();
    for config in configs {
        validate_config(&config.path, &config.metadata)?;
        if config.metadata.get("type").and_then(Value::as_str) != Some("generate")
            || config.metadata.get("schedule").and_then(Value::as_str) != Some("daily")
        {
            continue;
        }
        let output = config.metadata.get("output").and_then(Value::as_str);
        let extension = if output == Some("json") { "json" } else { "md" };
        let output_name = match &config.app {
            Some(app) => format!("_{app}_{}", config.name),
            None => config.name.clone(),
        };
        if day_dir
            .join("talents")
            .join(format!("{output_name}.{extension}"))
            .exists()
        {
            counts.processed += 1;
        } else {
            counts.pending += 1;
        }
    }
    Ok(counts)
}

struct TalentConfig {
    path: PathBuf,
    app: Option<String>,
    name: String,
    metadata: Map<String, Value>,
}

fn discover_system_configs(
    root: &Path,
    configs: &mut Vec<TalentConfig>,
) -> Result<(), JournalStatsError> {
    if !root.is_dir() {
        return Ok(());
    }
    for path in markdown_entries(root)? {
        let Some(metadata) = read_frontmatter(&path)? else {
            continue;
        };
        configs.push(TalentConfig {
            name: stem(&path)?,
            path,
            app: None,
            metadata,
        });
    }
    Ok(())
}

fn discover_app_configs(
    root: &Path,
    configs: &mut Vec<TalentConfig>,
) -> Result<(), JournalStatsError> {
    if !root.is_dir() {
        return Ok(());
    }
    for app_path in directories(root)? {
        let app = app_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid(&app_path, "app directory name is not UTF-8"))?
            .to_owned();
        if app.starts_with('_') {
            continue;
        }
        let talent_root = app_path.join("talent");
        for path in markdown_entries(&talent_root)? {
            let Some(metadata) = read_frontmatter(&path)? else {
                continue;
            };
            configs.push(TalentConfig {
                name: stem(&path)?,
                path,
                app: Some(app.clone()),
                metadata,
            });
        }
    }
    Ok(())
}

fn read_frontmatter(path: &Path) -> Result<Option<Map<String, Value>>, JournalStatsError> {
    let text = fs::read_to_string(path).map_err(|error| JournalStatsError::io(path, error))?;
    let mut lines = text.lines();
    if lines.next() != Some("{") {
        return Ok(None);
    }
    let mut frontmatter = String::from("{\n");
    let mut closed = false;
    for line in lines {
        frontmatter.push_str(line);
        frontmatter.push('\n');
        if line == "}" {
            closed = true;
            break;
        }
    }
    if !closed {
        return Err(invalid(
            path,
            "frontmatter opening brace has no closing brace",
        ));
    }
    let value: Value =
        serde_json::from_str(&frontmatter).map_err(|error| JournalStatsError::json(path, error))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| invalid(path, "frontmatter must be a JSON object"))
        .map(Some)
}

fn validate_config(path: &Path, metadata: &Map<String, Value>) -> Result<(), JournalStatsError> {
    let config_type = metadata.get("type");
    let type_name = match config_type {
        None => None,
        Some(Value::String(value)) if matches!(value.as_str(), "generate" | "cogitate") => {
            Some(value.as_str())
        }
        Some(_) => return Err(invalid(path, "type must be 'generate' or 'cogitate'")),
    };
    if metadata.get("schedule").is_some_and(is_truthy) && !metadata.contains_key("priority") {
        return Err(invalid(path, "scheduled prompt is missing priority"));
    }
    if metadata.contains_key("output") && type_name.is_none() {
        return Err(invalid(path, "prompt with output is missing type"));
    }
    if type_name == Some("generate") && !metadata.contains_key("output") {
        return Err(invalid(path, "generate prompt is missing output"));
    }
    if metadata.get("schedule").and_then(Value::as_str) == Some("activity")
        && !metadata
            .get("activities")
            .and_then(Value::as_array)
            .is_some_and(|activities| !activities.is_empty())
    {
        return Err(invalid(
            path,
            "activity prompt has no non-empty activities list",
        ));
    }
    let has_cogitate_field = ["write", "access_tier", "cwd"]
        .into_iter()
        .any(|field| metadata.contains_key(field));
    if type_name != Some("cogitate") && has_cogitate_field {
        return Err(invalid(
            path,
            "cogitate-only field set on non-cogitate prompt",
        ));
    }
    if type_name == Some("cogitate") {
        if metadata.get("write").is_some_and(is_truthy) {
            return Err(invalid(path, "cogitate prompt cannot set write"));
        }
        if let Some(access_tier) = metadata.get("access_tier").and_then(Value::as_str)
            && !matches!(
                access_tier,
                "normal" | "system-read" | "outbound" | "synthesis"
            )
        {
            return Err(invalid(path, "cogitate prompt has invalid access_tier"));
        }
        if let Some(cwd) = metadata.get("cwd").and_then(Value::as_str)
            && cwd != "journal"
        {
            return Err(invalid(path, "cogitate prompt has invalid cwd"));
        }
        if metadata
            .get("access_tier")
            .is_some_and(|value| !value.is_string())
        {
            return Err(invalid(path, "cogitate prompt has invalid access_tier"));
        }
        if metadata.get("cwd").is_some_and(|value| !value.is_string()) {
            return Err(invalid(path, "cogitate prompt has invalid cwd"));
        }
    }
    Ok(())
}

fn markdown_entries(root: &Path) -> Result<Vec<PathBuf>, JournalStatsError> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = fs::read_dir(root)
        .map_err(|error| JournalStatsError::io(root, error))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| JournalStatsError::io(root, error))?;
    entries.retain(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "md"));
    entries.sort();
    Ok(entries)
}

fn directories(root: &Path) -> Result<Vec<PathBuf>, JournalStatsError> {
    let mut entries = fs::read_dir(root)
        .map_err(|error| JournalStatsError::io(root, error))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| JournalStatsError::io(root, error))?;
    entries.retain(|path| path.is_dir());
    entries.sort();
    Ok(entries)
}

fn stem(path: &Path) -> Result<String, JournalStatsError> {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid(path, "talent filename has no UTF-8 stem"))
}

fn invalid(path: &Path, message: &str) -> JournalStatsError {
    JournalStatsError::TalentConfig {
        path: path.to_path_buf(),
        message: message.to_owned(),
    }
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
