// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Discovery and read-only rendering of scheduled generators.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// Discover packaged prompt roots in the same production order as
/// `solstone-core`'s `discover_package_roots`: executable layout first, then
/// the invoking directory, and finally source-relative paths.
pub fn discover_package_roots() -> (PathBuf, PathBuf) {
    for start in [
        env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf)),
        env::current_dir().ok(),
    ]
    .into_iter()
    .flatten()
    {
        for ancestor in start.ancestors() {
            let candidate = ancestor.join("solstone");
            if candidate.join("talent").is_dir() && candidate.join("apps").is_dir() {
                return (candidate.join("talent"), candidate.join("apps"));
            }
        }
    }
    (
        PathBuf::from("solstone/talent"),
        PathBuf::from("solstone/apps"),
    )
}

pub fn generators(config: &Map<String, Value>) -> Result<Value, String> {
    let (system_root, apps_root) = discover_package_roots();
    generators_from_roots(config, &system_root, &apps_root)
}

pub fn generators_from_roots(
    config: &Map<String, Value>,
    system_root: &Path,
    apps_root: &Path,
) -> Result<Value, String> {
    let mut found = Vec::new();
    collect_system(system_root, &mut found)?;
    collect_apps(apps_root, &mut found)?;
    let overrides = config.get("talent_overrides").and_then(Value::as_object);
    let mut segment = Vec::new();
    let mut daily = Vec::new();
    for item in found {
        if item.metadata.get("type").and_then(Value::as_str) != Some("generate") {
            continue;
        }
        let context = context_key(&item.key);
        let disabled = overrides
            .and_then(|entries| entries.get(&context))
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("disabled"))
            .and_then(Value::as_bool)
            .or_else(|| item.metadata.get("disabled").and_then(Value::as_bool))
            .unwrap_or(false);
        let rendered = json!({
            "key": item.key,
            "title": item.metadata.get("title").and_then(Value::as_str)
                .or_else(|| item.metadata.get("label").and_then(Value::as_str))
                .unwrap_or(&item.fallback_title),
            "description": item.metadata.get("description").and_then(Value::as_str).unwrap_or(""),
            "source": item.source,
            "app": item.app,
            "disabled": disabled,
        });
        match item.metadata.get("schedule").and_then(Value::as_str) {
            Some("segment") => segment.push(rendered),
            Some("daily") => daily.push(rendered),
            _ => {}
        }
    }
    Ok(json!({"segment": segment, "daily": daily}))
}

struct Generator {
    key: String,
    fallback_title: String,
    source: &'static str,
    app: Option<String>,
    metadata: Map<String, Value>,
}

fn collect_system(root: &Path, into: &mut Vec<Generator>) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    for path in markdown_entries(root)? {
        if let Some(metadata) = read_frontmatter(&path)? {
            let key = stem(&path)?;
            into.push(Generator {
                fallback_title: key.clone(),
                key,
                source: "system",
                app: None,
                metadata,
            });
        }
    }
    Ok(())
}

fn collect_apps(root: &Path, into: &mut Vec<Generator>) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut apps: Vec<_> = fs::read_dir(root)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    apps.sort_by_key(|entry| entry.file_name());
    for app_path in apps {
        let path = app_path.path();
        if !path.is_dir() {
            continue;
        }
        let app = app_path.file_name().to_string_lossy().into_owned();
        if app.starts_with('_') {
            continue;
        }
        for prompt in markdown_entries(&path.join("talent"))? {
            if let Some(metadata) = read_frontmatter(&prompt)? {
                let name = stem(&prompt)?;
                into.push(Generator {
                    fallback_title: name.clone(),
                    key: format!("{app}:{name}"),
                    source: "app",
                    app: Some(app.clone()),
                    metadata,
                });
            }
        }
    }
    Ok(())
}

fn markdown_entries(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<_> = fs::read_dir(root)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .collect();
    entries.sort();
    Ok(entries)
}

fn read_frontmatter(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut lines = text.lines();
    if lines.next() != Some("{") {
        return Ok(None);
    }
    let mut json_text = String::from("{\n");
    let mut closed = false;
    for line in lines {
        json_text.push_str(line);
        json_text.push('\n');
        if line == "}" {
            closed = true;
            break;
        }
    }
    if !closed {
        return Err(format!(
            "{}: frontmatter opening brace has no closing brace",
            path.display()
        ));
    }
    serde_json::from_str::<Value>(&json_text)
        .map_err(|error| error.to_string())?
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{}: frontmatter must be a JSON object", path.display()))
        .map(Some)
}

fn stem(path: &Path) -> Result<String, String> {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{}: non-UTF-8 file stem", path.display()))
}

fn context_key(key: &str) -> String {
    match key.split_once(':') {
        Some((app, name)) => format!("talent.{app}.{name}"),
        None => format!("talent.system.{key}"),
    }
}
