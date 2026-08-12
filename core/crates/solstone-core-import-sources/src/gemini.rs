// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Gemini Takeout detection, preview, and planning.

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_import::ImportPreview;

use crate::shared::{
    ParsedEntry, has_extension, is_file, parse_iso_utc, plan_entries, read_json_file,
    read_zip_json, read_zip_member,
};
use crate::{ImportPlan, SkipLocator, SkipReason, SkippedEntry, SourceError};

const ACTIVITY_PATHS: [&str; 4] = [
    "Takeout/My Activity/Gemini Apps/MyActivity.json",
    "My Activity/Gemini Apps/MyActivity.json",
    "Takeout/My Activity/Bard/MyActivity.json",
    "My Activity/Bard/MyActivity.json",
];

/// Detect a Gemini Takeout archive, JSON export, or export directory.
pub fn detect(path: &Path) -> Result<bool, SourceError> {
    if is_file(path) && has_extension(path, "zip") {
        for activity_path in ACTIVITY_PATHS {
            if read_zip_member(path, activity_path)?.is_some() {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if is_file(path) && has_extension(path, "json") {
        let value = read_json_file(path, "Gemini activities")?;
        let Some(first) = value.as_array().and_then(|activities| activities.first()) else {
            return Ok(false);
        };
        let Some(first) = first.as_object() else {
            return Ok(false);
        };
        return Ok(first.contains_key("header") && first.contains_key("time"));
    }
    if path.is_dir() {
        return Ok(ACTIVITY_PATHS
            .iter()
            .any(|activity_path| path.join(activity_path).is_file())
            || path.join("MyActivity.json").is_file());
    }
    Ok(false)
}

/// Preview the atomic message count and UTC date range for Gemini activities.
pub fn preview(path: &Path) -> Result<ImportPreview, SourceError> {
    let plan = plan(path)?;
    Ok(ImportPreview {
        date_range: plan.date_range,
        item_count: plan.item_count,
        entity_count: 0,
        summary: format!("{} messages from Gemini export", plan.item_count),
    })
}

/// Parse Gemini activities into a write-free UTC segment plan.
pub fn plan(path: &Path) -> Result<ImportPlan, SourceError> {
    let value = load_activities(path)?;
    let activities = value
        .as_array()
        .ok_or_else(|| SourceError::InvalidJsonShape {
            path: path.to_owned(),
            context: "Gemini activities",
        })?;
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    for (activity_index, activity) in activities.iter().enumerate() {
        parse_activity(activity, activity_index, &mut entries, &mut skipped);
    }
    Ok(plan_entries(entries, skipped))
}

fn load_activities(path: &Path) -> Result<Value, SourceError> {
    if is_file(path) && has_extension(path, "zip") {
        for activity_path in ACTIVITY_PATHS {
            match read_zip_json(path, activity_path, "Gemini activities") {
                Ok(value) => return Ok(value),
                Err(SourceError::ArchiveMemberMissing { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        return Err(SourceError::ArchiveMemberMissing {
            path: path.to_owned(),
            member: ACTIVITY_PATHS[0],
        });
    }
    if is_file(path) && has_extension(path, "json") {
        return read_json_file(path, "Gemini activities");
    }
    if path.is_dir() {
        for activity_path in ACTIVITY_PATHS {
            let candidate = path.join(activity_path);
            if candidate.is_file() {
                return read_json_file(&candidate, "Gemini activities");
            }
        }
        let candidate = path.join("MyActivity.json");
        if candidate.is_file() {
            return read_json_file(&candidate, "Gemini activities");
        }
        return Err(SourceError::ArchiveMemberMissing {
            path: path.to_owned(),
            member: ACTIVITY_PATHS[0],
        });
    }
    if path.exists() {
        Err(SourceError::UnsupportedExtension {
            path: path.to_owned(),
        })
    } else {
        Err(SourceError::Io {
            path: path.to_owned(),
            operation: "open source",
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "source does not exist"),
        })
    }
}

fn parse_activity(
    activity: &Value,
    activity_index: usize,
    entries: &mut Vec<ParsedEntry>,
    skipped: &mut Vec<SkippedEntry>,
) {
    let Some(activity) = activity.as_object() else {
        skip_activity(skipped, activity_index, SkipReason::NoActivityContent);
        return;
    };
    let prompt = prompt(activity);
    let response = response(activity);
    if prompt.is_empty() && response.is_empty() {
        skip_activity(skipped, activity_index, SkipReason::NoActivityContent);
        return;
    }
    let Some(time) = activity
        .get("time")
        .and_then(Value::as_str)
        .filter(|time| !time.is_empty())
    else {
        skip_activity(
            skipped,
            activity_index,
            SkipReason::MissingActivityTimestamp,
        );
        return;
    };
    let Some(timestamp) = parse_iso_utc(&time.replace('Z', "+00:00")) else {
        skip_activity(
            skipped,
            activity_index,
            SkipReason::InvalidActivityTimestamp,
        );
        return;
    };
    if !prompt.is_empty() {
        entries.push(ParsedEntry {
            timestamp,
            speaker: "Human".to_owned(),
            text: prompt,
            model_slug: None,
        });
    }
    if !response.is_empty() {
        entries.push(ParsedEntry {
            timestamp,
            speaker: "Assistant".to_owned(),
            text: response,
            model_slug: None,
        });
    }
}

fn prompt(activity: &Map<String, Value>) -> String {
    activity
        .get("subtitles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .find_map(|subtitle| {
            subtitle
                .get("value")
                .or_else(|| subtitle.get("name"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_default()
        .to_owned()
}

fn response(activity: &Map<String, Value>) -> String {
    let Some(items) = activity.get("safeHtmlItem").and_then(Value::as_array) else {
        return String::new();
    };
    for item in items {
        let Some(html) = item
            .as_object()
            .and_then(|item| item.get("html"))
            .and_then(Value::as_str)
            .filter(|html| !html.is_empty())
        else {
            continue;
        };
        return strip_html(html);
    }
    String::new()
}

fn strip_html(html: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    output
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_owned()
}

#[cfg(test)]
fn is_bard_era(activity: &Map<String, Value>) -> bool {
    activity
        .get("products")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|product| product.to_lowercase().contains("bard"))
        || activity
            .get("header")
            .and_then(Value::as_str)
            .is_some_and(|header| header.to_lowercase().contains("bard"))
}

fn skip_activity(skipped: &mut Vec<SkippedEntry>, activity_index: usize, reason: SkipReason) {
    skipped.push(SkippedEntry {
        locator: SkipLocator::Activity { activity_index },
        reason,
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::is_bard_era;

    #[test]
    fn bard_era_recognizes_products_and_header() {
        assert!(is_bard_era(
            json!({ "products": ["Bard"] }).as_object().unwrap()
        ));
        assert!(is_bard_era(
            json!({ "header": "Bard activity" }).as_object().unwrap()
        ));
        assert!(!is_bard_era(
            json!({ "products": ["Gemini"] }).as_object().unwrap()
        ));
    }
}
