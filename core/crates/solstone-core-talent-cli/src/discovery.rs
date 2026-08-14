// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use regex::Regex;
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub(crate) struct TalentConfig {
    pub(crate) key: String,
    pub(crate) file: String,
    pub(crate) metadata: Map<String, Value>,
    pub(crate) body: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedFrontmatter {
    pub(crate) metadata: Map<String, Value>,
    pub(crate) body: String,
}

pub(crate) fn discover(talent_root: &Path, apps_root: &Path) -> Result<Vec<TalentConfig>, String> {
    let mut configs = Vec::new();
    collect_system(talent_root, &mut configs)?;
    collect_apps(apps_root, &mut configs)?;
    Ok(configs)
}

fn collect_system(root: &Path, configs: &mut Vec<TalentConfig>) -> Result<(), String> {
    for path in markdown_entries(root)? {
        let name = stem(&path)?;
        configs.push(config(
            &path,
            name.clone(),
            format!("talent/{name}.md"),
            "system",
            None,
        )?);
    }
    Ok(())
}

fn collect_apps(root: &Path, configs: &mut Vec<TalentConfig>) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut apps = fs::read_dir(root)
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?;
    apps.sort_by_key(|entry| entry.file_name());
    for app_entry in apps {
        let app_path = app_entry.path();
        if !app_path.is_dir() {
            continue;
        }
        let app = app_entry.file_name().to_string_lossy().into_owned();
        if app.starts_with('_') {
            continue;
        }
        for path in markdown_entries(&app_path.join("talent"))? {
            let name = stem(&path)?;
            let key = format!("{app}:{name}");
            let file = format!("apps/{app}/talent/{name}.md");
            configs.push(config(&path, key, file, "app", Some(app.clone()))?);
        }
    }
    Ok(())
}

fn config(
    path: &Path,
    key: String,
    file: String,
    source: &str,
    app: Option<String>,
) -> Result<TalentConfig, String> {
    let parsed = read_frontmatter(path)?;
    let modified = fs::metadata(path)
        .map_err(|error| format!("failed to stat {}: {error}", path.display()))?
        .modified()
        .map_err(|error| format!("failed to stat {}: {error}", path.display()))?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("failed to stat {}: {error}", path.display()))?
        .as_secs();
    let mut metadata = Map::new();
    metadata.insert("path".to_owned(), Value::String(path.display().to_string()));
    metadata.insert("mtime".to_owned(), Value::Number(modified.into()));
    metadata.extend(parsed.metadata);
    if !metadata.contains_key("color") {
        metadata.insert("color".to_owned(), Value::String("#6c757d".to_owned()));
    }
    metadata.insert("source".to_owned(), Value::String(source.to_owned()));
    if let Some(app) = app {
        metadata.insert("app".to_owned(), Value::String(app));
    }
    Ok(TalentConfig {
        key,
        file,
        metadata,
        body: parsed.body,
    })
}

pub(crate) fn read_frontmatter(path: &Path) -> Result<ParsedFrontmatter, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let text = text.trim();
    let boundary = Regex::new(r"(?m)^(?:\{|\})$").expect("static regex");
    let Some(first) = boundary.find(text) else {
        return Ok(ParsedFrontmatter {
            metadata: Map::new(),
            body: text.to_owned(),
        });
    };
    if first.start() != 0 || first.as_str() != "{" {
        return Ok(ParsedFrontmatter {
            metadata: Map::new(),
            body: text.to_owned(),
        });
    }
    let mut parts = boundary.splitn(text, 3);
    let _ = parts.next();
    let Some(frontmatter) = parts.next() else {
        return Ok(ParsedFrontmatter {
            metadata: Map::new(),
            body: text.to_owned(),
        });
    };
    let Some(body) = parts.next() else {
        return Ok(ParsedFrontmatter {
            metadata: Map::new(),
            body: text.to_owned(),
        });
    };
    let value: Value = serde_json::from_str(&format!("{{{frontmatter}}}"))
        .map_err(|_| format!("failed to parse frontmatter from {}", path.display()))?;
    Ok(ParsedFrontmatter {
        metadata: value.as_object().cloned().unwrap_or_default(),
        body: body.trim().to_owned(),
    })
}

fn markdown_entries(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = fs::read_dir(root)
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("md")
        })
        .collect::<Vec<_>>();
    entries.sort();
    Ok(entries)
}

fn stem(path: &Path) -> Result<String, String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("talent filename has no UTF-8 stem: {}", path.display()))
}
