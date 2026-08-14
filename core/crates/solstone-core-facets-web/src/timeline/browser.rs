// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

const MAX_EXTRACTION_CHARS: usize = 32_768;
const EXTRACTION_BOUND_MARKER: &str = "\n\n[solstone: extraction output bounded before journaling - degenerate length sanitized/truncated]";

pub fn format_browser(entries: &[Value]) -> (Vec<Value>, Option<String>) {
    if entries.is_empty() {
        return (Vec::new(), Some("browser stream has no rows".to_owned()));
    }
    let budget = MAX_EXTRACTION_CHARS - EXTRACTION_BOUND_MARKER.chars().count();
    let mut chunks = Vec::new();
    let mut emitted = 0;
    let mut saw_start = false;
    for row in entries.iter().filter(|row| row.is_object()) {
        let kind = text(row.get("t"));
        let markdown = match kind.as_str() {
            "segment_start" => {
                saw_start = true;
                format_snapshot(row)
            }
            "delta" => format_delta(row),
            _ => String::new(),
        };
        if markdown.is_empty() {
            continue;
        }
        let remaining = budget.saturating_sub(emitted);
        let (markdown, stop) = if remaining == 0 {
            (EXTRACTION_BOUND_MARKER.to_owned(), true)
        } else if markdown.chars().count() > remaining {
            (
                format!(
                    "{}{}",
                    markdown.chars().take(remaining).collect::<String>(),
                    EXTRACTION_BOUND_MARKER
                ),
                true,
            )
        } else {
            (markdown, false)
        };
        emitted += markdown.chars().count();
        // Deliberate divergence: missing `ts` becomes JSON null here; Python indexes
        // `row["ts"]` and raises. Observer-produced rows always carry integer timestamps.
        chunks.push(json!({"timestamp": row["ts"], "markdown": markdown, "source": row}));
        if stop {
            break;
        }
    }
    let error = (!saw_start).then(|| "browser stream has no segment_start rows".to_owned());
    (chunks, error)
}

fn format_snapshot(row: &Value) -> String {
    let title = text(row.get("title"));
    let site = text(row.get("site"));
    let url = text(row.get("url"));
    let heading = if !title.is_empty() {
        title
    } else if !site.is_empty() {
        site.clone()
    } else if !url.is_empty() {
        url
    } else {
        "Browser Page".to_owned()
    };
    let mut parts = vec![format!("## {heading}")];
    let adapter = text(row.get("adapter"));
    let subline = [adapter, site]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if !subline.is_empty() {
        parts.extend([String::new(), subline]);
    }
    let blocks = format_blocks(row.get("blocks"));
    if !blocks.is_empty() {
        parts.push(String::new());
        parts.extend(blocks);
    }
    parts.join("\n").trim().to_owned()
}

fn format_blocks(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |blocks| {
            blocks
                .iter()
                .filter_map(|block| {
                    let text = text(block.get("text"));
                    (!text.is_empty()).then(|| {
                        if text_of(block.get("type")) == "heading" {
                            format!("### {text}")
                        } else {
                            text
                        }
                    })
                })
                .collect()
        })
}

fn format_delta(row: &Value) -> String {
    let operation = text(row.get("op"));
    if !matches!(operation.as_str(), "add" | "update") {
        return String::new();
    }
    row.get("block")
        .filter(|block| block.is_object())
        .map_or_else(String::new, |block| text(block.get("text")))
}

fn text(value: Option<&Value>) -> String {
    // Deliberate divergence: Python stringifies non-string values; this internal
    // observer contract treats non-string title/site/url/adapter/block text as empty.
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}
fn text_of(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or_default().trim()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{EXTRACTION_BOUND_MARKER, MAX_EXTRACTION_CHARS, format_browser};

    #[test]
    fn ac2_unicode_bounding_uses_character_not_byte_budget() {
        let rows = vec![json!({
            "t": "segment_start",
            "ts": 1,
            "title": "é",
            "blocks": [{"text": "é".repeat(MAX_EXTRACTION_CHARS), "type": "text"}],
        })];
        let (chunks, error) = format_browser(&rows);
        assert!(error.is_none());
        let markdown = chunks[0]["markdown"].as_str().expect("markdown");
        assert!(markdown.ends_with(EXTRACTION_BOUND_MARKER));
        assert_eq!(markdown.chars().count(), MAX_EXTRACTION_CHARS);
        assert!(markdown.is_char_boundary(markdown.len()));
    }
}
