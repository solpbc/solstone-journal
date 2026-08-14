// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, path::Path};

use serde_json::{Value, json};

use crate::segments::is_day;

// Reads never create the journal tree. The deliberate no-mkdir-on-read ruling has no
// observable: a permission-denied root reads as an empty journal rather than 500, and
// no check distinguishes that case. This follows the reference's read-as-empty shape.
pub fn list_newsletters(root: &Path) -> Vec<NewsRow> {
    let Ok(facets) = fs::read_dir(root.join("facets")) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for facet in facets.filter_map(Result::ok) {
        let name = facet.file_name().to_string_lossy().into_owned();
        if !facet.path().is_dir() || !valid_facet(&name) {
            continue;
        }
        let Ok(news) = fs::read_dir(facet.path().join("news")) else {
            continue;
        };
        for entry in news.filter_map(Result::ok) {
            let path = entry.path();
            let day = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if path.is_file()
                && path.extension().is_some_and(|extension| extension == "md")
                && is_day(day)
            {
                rows.push(NewsRow {
                    facet: name.clone(),
                    day: day.to_owned(),
                });
            }
        }
    }
    rows.sort_by(|left, right| right.day.cmp(&left.day).then(left.facet.cmp(&right.facet)));
    rows
}

#[derive(Clone, Debug)]
pub struct NewsRow {
    pub facet: String,
    pub day: String,
}

pub fn valid_facet(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub fn get_facet_news(
    root: &Path,
    facet: &str,
    cursor: Option<&str>,
    limit: usize,
    day: Option<&str>,
) -> Value {
    let news_dir = root.join("facets").join(facet).join("news");
    let Ok(entries) = fs::read_dir(&news_dir) else {
        return json!({"days": [], "next_cursor": null, "has_more": false});
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.extension().is_some_and(|extension| extension == "md")
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(is_day)
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| right.file_stem().cmp(&left.file_stem()));
    let selected = if let Some(day) = day {
        files
            .into_iter()
            .filter(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(day))
            .collect::<Vec<_>>()
    } else {
        if let Some(cursor) = cursor {
            files.retain(|path| path.file_stem().is_some_and(|stem| stem < cursor));
        }
        files.truncate(limit);
        files
    };
    let total_available = if day.is_some() {
        selected.len()
    } else {
        let mut all = fs::read_dir(&news_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path.extension().is_some_and(|extension| extension == "md")
                    && path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(is_day)
            })
            .collect::<Vec<_>>();
        if let Some(cursor) = cursor {
            all.retain(|path| path.file_stem().is_some_and(|stem| stem < cursor));
        }
        all.len()
    };
    let days = selected
        .iter()
        .map(|path| {
            let date = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("day file");
            // Deliberately port get_facet_news's facets.py:524-539 `except Exception: pass`:
            // an unreadable file contributes an empty raw body rather than changing the feed.
            let raw_content = fs::read_to_string(path).unwrap_or_default();
            json!({"date": date, "raw_content": raw_content})
        })
        .collect::<Vec<_>>();
    let has_more = day.is_none() && total_available > selected.len();
    let next_cursor = has_more.then(|| {
        selected
            .last()
            .and_then(|path| path.file_stem())
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
    });
    json!({"days": days, "next_cursor": next_cursor, "has_more": has_more})
}

// This is deliberately narrower than PyYAML's rejection set: it may accept what Python
// rejects, but can never reject what Python accepts. A false 500 on a valid newsletter is
// an owner-visible regression; a missed 500 on exotic malformed YAML is not.
pub fn split_frontmatter(raw: &str) -> Result<&str, ()> {
    let Some(first_end) = raw.find('\n') else {
        return Ok(raw);
    };
    if raw[..first_end].trim_end_matches('\r') != "---" {
        return Ok(raw.trim_end_matches(['\r', '\n']));
    }
    let remainder = &raw[first_end + 1..];
    let mut offset = 0;
    for line in remainder.split_inclusive('\n') {
        let plain = line.trim_end_matches(['\r', '\n']);
        offset += line.len();
        if plain == "---" || plain == "..." {
            let header = &remainder[..offset - line.len()];
            if header
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\t' | '\r' | '\n'))
            {
                return Err(());
            }
            return Ok(remainder[offset..].trim_matches(['\r', '\n']));
        }
    }
    // An unclosed delimiter is not frontmatter: leave the complete document as Markdown.
    Ok(raw)
}
