// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;

use serde_json::Value;

use super::{JsonObject, ProducedChunks, display_value, json_falsy, recorded_chunk};

pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let mut chunks = Vec::new();
    let mut attention_memory: HashMap<(Option<String>, Option<String>), String> = HashMap::new();

    for record in records {
        let kind = clean_text(record.get("t"));
        let markdown = match kind.as_str() {
            "segment_start" => {
                let (md, attention) = format_snapshot(record);
                if !md.is_empty() {
                    let key = memory_key(record);
                    // Snapshot resets attention memory for this key; empty attention removes it.
                    if attention.is_empty() {
                        attention_memory.remove(&key);
                    } else {
                        attention_memory.insert(key, attention);
                    }
                }
                md
            }
            "delta" => {
                let block_text = format_delta(record);
                if block_text.is_empty() {
                    String::new()
                } else {
                    let attention = parse_attention_text(record);
                    if attention.is_empty() {
                        block_text
                    } else {
                        let key = memory_key(record);
                        let remembered = attention_memory.get(&key);
                        if remembered.map(String::as_str) != Some(attention.as_str()) {
                            let prefixed = format!("context: {attention}\n{block_text}");
                            attention_memory.insert(key, attention);
                            prefixed
                        } else {
                            block_text
                        }
                    }
                }
            }
            _ => String::new(),
        };
        if !markdown.is_empty() {
            let occurrence = record.get("ts").and_then(integer_milliseconds).unwrap_or(0);
            chunks.push(recorded_chunk(markdown, occurrence, record));
        }
    }

    ProducedChunks {
        chunks,
        agent_override: Some("browser".to_string()),
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

pub(super) fn integer_milliseconds(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        let number = value.as_f64()?;
        (number.is_finite()
            && number.fract() == 0.0
            && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&number))
        .then_some(number as i64)
    })
}

fn memory_key(row: &JsonObject) -> (Option<String>, Option<String>) {
    let inst = row.get("inst").and_then(Value::as_str).map(str::to_string);
    let ctx = row.get("ctx").and_then(Value::as_str).map(str::to_string);
    (inst, ctx)
}

fn collapsed_bounded_string(value: Option<&Value>) -> Option<String> {
    let raw = value?.as_str()?;
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() && collapsed.chars().count() <= 64 {
        Some(collapsed)
    } else {
        None
    }
}

fn format_duration(ms: i64) -> String {
    // Duration: largest whole unit plus next non-zero smaller unit; seconds only under one minute.
    let s = ms / 1000;
    if s < 60 {
        return format!("{s}s");
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    let rem_m = m % 60;
    if h < 24 {
        if rem_m > 0 {
            return format!("{h}h {rem_m}m");
        } else {
            return format!("{h}h");
        }
    }
    let d = h / 24;
    let rem_h = h % 24;
    if rem_h > 0 {
        format!("{d}d {rem_h}h")
    } else {
        format!("{d}d")
    }
}

fn parse_attention_parts(row: &JsonObject) -> Vec<&'static str> {
    let mut parts = Vec::new();
    if let Some(Value::Object(tab)) = row.get("tab") {
        let front = tab.get("front").and_then(Value::as_bool);
        let focused = tab.get("focused").and_then(Value::as_bool);
        match front {
            Some(true) => match focused {
                Some(true) => parts.push("front tab, window focused"),
                Some(false) => parts.push("front tab, window not focused"),
                None => parts.push("front tab"),
            },
            Some(false) => parts.push("background tab"),
            None => {}
        }
    }
    if let Some(Value::Object(media)) = row.get("media")
        && media.get("playing").and_then(Value::as_bool) == Some(true)
    {
        if media.get("muted").and_then(Value::as_bool) == Some(true) {
            parts.push("media playing, muted");
        } else {
            parts.push("media playing");
        }
    }
    if row.get("solstone_paused").and_then(Value::as_bool) == Some(true) {
        parts.push("solstone app paused");
    }
    parts
}

fn parse_attention_text(row: &JsonObject) -> String {
    parse_attention_parts(row).join(" · ")
}

fn parse_snapshot_context(row: &JsonObject) -> (Option<String>, String) {
    let mut parts: Vec<String> = parse_attention_parts(row)
        .into_iter()
        .map(str::to_string)
        .collect();
    let attention_text = parts.join(" · ");

    if let Some(Value::Object(tab)) = row.get("tab")
        && let Some(ms) = tab.get("age_ms").and_then(integer_milliseconds)
        && ms >= 0
    {
        parts.push(format!("tab open at least {}", format_duration(ms)));
    }
    if let Some(lang) = collapsed_bounded_string(row.get("lang")) {
        parts.push(format!("language {lang}"));
    }
    if let Some(Value::Object(browser)) = row.get("browser") {
        let vendor = collapsed_bounded_string(browser.get("vendor"));
        let version = collapsed_bounded_string(browser.get("version"));
        match (vendor, version) {
            (Some(v), Some(ver)) => parts.push(format!("{v} {ver}")),
            (Some(v), None) => parts.push(v),
            (None, _) => {}
        }
    }

    let context_line = if parts.is_empty() {
        None
    } else {
        Some(format!("context: {}", parts.join(" · ")))
    };

    (context_line, attention_text)
}

fn format_snapshot(row: &JsonObject) -> (String, String) {
    let title = clean_text(row.get("title"));
    let site = clean_text(row.get("site"));
    let url = clean_text(row.get("url"));
    let heading =
        first_non_empty(&[title.as_str(), site.as_str(), url.as_str()]).unwrap_or("Browser Page");
    let mut lines = vec![format!("## {heading}")];

    let adapter = clean_text(row.get("adapter"));
    let subline = [adapter.as_str(), site.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if !subline.is_empty() {
        lines.push(String::new());
        lines.push(subline);
    }

    let (context_line, attention_text) = parse_snapshot_context(row);
    if let Some(context_line) = context_line {
        lines.push(context_line);
    }

    let block_lines = format_blocks(row.get("blocks"));
    if !block_lines.is_empty() {
        lines.push(String::new());
        lines.extend(block_lines);
    }
    (lines.join("\n").trim().to_string(), attention_text)
}

fn format_blocks(blocks: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(blocks)) = blocks else {
        return Vec::new();
    };

    let mut lines = Vec::new();
    for block in blocks {
        let Value::Object(block) = block else {
            continue;
        };
        let text = clean_text(block.get("text"));
        if text.is_empty() {
            continue;
        }
        if block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            == "heading"
        {
            lines.push(format!("### {text}"));
        } else {
            lines.push(text);
        }
    }
    lines
}

fn format_delta(row: &JsonObject) -> String {
    match clean_text(row.get("op")).as_str() {
        "add" | "update" => {}
        _ => return String::new(),
    }
    let Some(Value::Object(block)) = row.get("block") else {
        return String::new();
    };
    clean_text(block.get("text"))
}

fn first_non_empty<'a>(values: &[&'a str]) -> Option<&'a str> {
    values.iter().copied().find(|value| !value.is_empty())
}

fn clean_text(value: Option<&Value>) -> String {
    if json_falsy(value) {
        String::new()
    } else {
        value
            .map(display_value)
            .unwrap_or_default()
            .trim()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_jsonl_objects;

    #[test]
    fn integral_timestamp_representations_preserve_occurrence() {
        for (raw, expected) in [
            ("100", 100),
            ("100.0", 100),
            ("1e3", 1000),
            ("9007199254740991.0", 9_007_199_254_740_991),
        ] {
            let text = format!(r#"{{"t":"segment_start","ts":{raw},"blocks":[]}}"#);
            let produced = render(&parse_jsonl_objects(&text));
            assert_eq!(
                produced.chunks[0].occurrence_time_ms,
                Some(crate::content::OccurrenceTimeMs(expected))
            );
        }
        for raw in ["0.5", "9223372036854775808.0", "-9223372036854777856.0"] {
            let value: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(integer_milliseconds(&value), None);
        }
    }

    #[test]
    fn renders_snapshots_and_deltas() {
        let records = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"site":"mail.google.com","url":"https://mail.google.com/mail/u/0/#inbox","title":"Inbox - Gmail","adapter":"gmail","blocks":[{"type":"heading","text":"Inbox"},{"type":"row","text":"Ari Patel - Browser stream contract review"},{"type":"row","text":"   "},{"type":"link","text":"Open pull request"}]}
{"t":"delta","ts":2,"op":"add","block":{"type":"row","text":"New: Casey Morgan - Lunch moved"}}
{"t":"delta","ts":3,"op":"update","block":{"type":"row","text":"Ari Patel - Browser stream contract review (2 replies)"}}
{"t":"delta","ts":4,"op":"remove","block":{"type":"row","text":"Promotions tab collapsed"}}
{"t":"ignored","ts":5,"text":"skip me"}
"#,
        );
        let produced = render(&records);
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();

        assert_eq!(produced.agent_override.as_deref(), Some("browser"));
        assert_eq!(
            contents,
            vec![
                "## Inbox - Gmail\n\ngmail · mail.google.com\n\n### Inbox\nAri Patel - Browser stream contract review\nOpen pull request",
                "New: Casey Morgan - Lunch moved",
                "Ari Patel - Browser stream contract review (2 replies)",
            ]
        );
    }

    #[test]
    fn snapshot_uses_url_or_default_heading_and_delta_skip_rules() {
        let records = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"url":"https://example.com/fallback","blocks":[{"type":"text","text":"Fallback page text"}]}
{"t":"segment_start","ts":2,"blocks":[]}
{"t":"delta","ts":3,"op":"add","block":{"type":"row","text":"   "}}
{"t":"delta","ts":4,"op":"noop","block":{"type":"row","text":"skip"}}
{"t":"delta","ts":5,"op":"update","block":"not an object"}
"#,
        );
        let produced = render(&records);
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();

        assert_eq!(
            contents,
            vec![
                "## https://example.com/fallback\n\nFallback page text",
                "## Browser Page",
            ]
        );
    }

    #[test]
    fn produce_chunks_integrates_browser_jsonl() {
        use crate::content::{Family, OccurrenceTimeMs, produce_chunks};

        let path = "chronicle/20260804/suze_browser/120000_10/browser_pages.jsonl";
        let content = "{\"t\":\"segment_start\",\"ts\":1000,\"title\":\"Doc\",\"blocks\":[{\"text\":\"Line 1\"}]}\n";
        let produced = produce_chunks(Family::Browser, path, content);
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(produced.chunks[0].content, "## Doc\n\nLine 1");
        assert_eq!(
            produced.chunks[0].occurrence_time_ms,
            Some(OccurrenceTimeMs(1000))
        );
        assert_eq!(produced.agent_override.as_deref(), Some("browser"));
    }

    #[test]
    fn snapshot_context_rendering_table() {
        let cases = [
            (
                r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":true}}"#,
                "## Browser Page\ncontext: front tab, window focused",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":false}}"#,
                "## Browser Page\ncontext: front tab, window not focused",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"front":false,"focused":true}}"#,
                "## Browser Page\ncontext: background tab",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"front":true}}"#,
                "## Browser Page\ncontext: front tab",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"focused":true}}"#,
                "## Browser Page",
            ),
            (
                r#"{"t":"segment_start","ts":1,"media":{"playing":true}}"#,
                "## Browser Page\ncontext: media playing",
            ),
            (
                r#"{"t":"segment_start","ts":1,"media":{"playing":true,"muted":true}}"#,
                "## Browser Page\ncontext: media playing, muted",
            ),
            (
                r#"{"t":"segment_start","ts":1,"media":{"playing":false,"muted":true}}"#,
                "## Browser Page",
            ),
            (
                r#"{"t":"segment_start","ts":1,"solstone_paused":true}"#,
                "## Browser Page\ncontext: solstone app paused",
            ),
            (
                r#"{"t":"segment_start","ts":1,"solstone_paused":false}"#,
                "## Browser Page",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":0}}"#,
                "## Browser Page\ncontext: tab open at least 0s",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":45000}}"#,
                "## Browser Page\ncontext: tab open at least 45s",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":750000}}"#,
                "## Browser Page\ncontext: tab open at least 12m",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":10800000}}"#,
                "## Browser Page\ncontext: tab open at least 3h",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":11100000}}"#,
                "## Browser Page\ncontext: tab open at least 3h 5m",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":174600000}}"#,
                "## Browser Page\ncontext: tab open at least 2d",
            ),
            (
                r#"{"t":"segment_start","ts":1,"tab":{"age_ms":187200000}}"#,
                "## Browser Page\ncontext: tab open at least 2d 4h",
            ),
            (
                r#"{"t":"segment_start","ts":1,"lang":"en-US"}"#,
                "## Browser Page\ncontext: language en-US",
            ),
            (
                r#"{"t":"segment_start","ts":1,"lang":"fr"}"#,
                "## Browser Page\ncontext: language fr",
            ),
            (
                r#"{"t":"segment_start","ts":1,"browser":{"vendor":"safari","version":"18.1"}}"#,
                "## Browser Page\ncontext: safari 18.1",
            ),
            (
                r#"{"t":"segment_start","ts":1,"browser":{"vendor":"firefox"}}"#,
                "## Browser Page\ncontext: firefox",
            ),
            (
                r#"{"t":"segment_start","ts":1,"browser":{"version":"18.1"}}"#,
                "## Browser Page",
            ),
            (
                r#"{"t":"segment_start","ts":1,"title":"Example","tab":{"front":true},"blocks":[{"type":"text","text":"Hello"}]}"#,
                "## Example\ncontext: front tab\n\nHello",
            ),
            (
                r#"{"t":"segment_start","ts":1,"adapter":"gmail","site":"mail.google.com","title":"Inbox","ext":"EXT-SECRET-99","tab":{"id":"TAB-ID-77","front":false,"focused":true,"age_ms":11100000},"media":{"playing":true},"solstone_paused":true,"lang":"en-US","browser":{"vendor":"safari","version":"18.1"},"blocks":[{"type":"heading","text":"Inbox"}]}"#,
                "## Inbox\n\ngmail · mail.google.com\ncontext: background tab · media playing · solstone app paused · tab open at least 3h 5m · language en-US · safari 18.1\n\n### Inbox",
            ),
        ];

        for (json, expected) in cases {
            let records = parse_jsonl_objects(json);
            let produced = render(&records);
            assert!(produced.warnings.is_empty());
            assert_eq!(produced.error, None);
            assert_eq!(produced.chunks.len(), 1);
            assert_eq!(produced.chunks[0].content, expected);
            if json.contains("EXT-SECRET-99") {
                assert!(!produced.chunks[0].content.contains("EXT-SECRET-99"));
                assert!(!produced.chunks[0].content.contains("TAB-ID-77"));
            }
            if json.contains(r#""front":false,"focused":true"#) {
                assert!(!produced.chunks[0].content.contains("window focused"));
            }
        }
    }

    #[test]
    fn delta_attention_memory_and_reset() {
        let records_b = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"inst":"shared","ctx":"shared","tab":{"front":true,"focused":true,"age_ms":10800000},"blocks":[{"text":"Hello"}]}
{"t":"delta","ts":2,"op":"add","inst":"shared","ctx":"shared","tab":{"front":true,"focused":true,"age_ms":10800000},"lang":"en-US","block":{"text":"World"}}
"#,
        );
        let produced_b = render(&records_b);
        assert!(produced_b.warnings.is_empty());
        assert_eq!(produced_b.error, None);
        assert_eq!(produced_b.chunks.len(), 2);
        assert!(
            produced_b.chunks[0]
                .content
                .contains("tab open at least 3h")
        );
        assert!(produced_b.chunks[0].content.contains("context:"));
        assert_eq!(produced_b.chunks[1].content, "World");

        let records_a = parse_jsonl_objects(
            r#"{"t":"delta","ts":1,"op":"add","inst":"shared","ctx":"shared","tab":{"front":true,"focused":true,"age_ms":10800000},"lang":"en-US","browser":{"vendor":"safari"},"block":{"text":"Added"}}"#,
        );
        let produced_a = render(&records_a);
        assert!(produced_a.warnings.is_empty());
        assert_eq!(produced_a.error, None);
        assert_eq!(
            produced_a
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec!["context: front tab, window focused\nAdded"]
        );
        assert!(!produced_a.chunks[0].content.contains("3h"));
        assert!(!produced_a.chunks[0].content.contains("language"));
        assert!(!produced_a.chunks[0].content.contains("safari"));

        let records_c = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"inst":"c","ctx":"c","tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"update","inst":"c","ctx":"c","tab":{"front":false},"block":{"text":"Edited"}}
"#,
        );
        let produced_c = render(&records_c);
        assert!(produced_c.warnings.is_empty());
        assert_eq!(produced_c.error, None);
        assert_eq!(
            produced_c
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: background tab\nEdited",
            ]
        );

        let records_d1 = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"inst":"one","ctx":"alpha","tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"add","inst":"two","ctx":"alpha","tab":{"front":true,"focused":true},"block":{"text":"Other"}}
"#,
        );
        let produced_d1 = render(&records_d1);
        assert!(produced_d1.warnings.is_empty());
        assert_eq!(produced_d1.error, None);
        assert_eq!(
            produced_d1
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: front tab, window focused\nOther",
            ]
        );

        let records_d2 = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"inst":"","ctx":"x","tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"add","ctx":"x","tab":{"front":true,"focused":true},"block":{"text":"After1"}}
"#,
        );
        let produced_d2 = render(&records_d2);
        assert!(produced_d2.warnings.is_empty());
        assert_eq!(produced_d2.error, None);
        assert_eq!(
            produced_d2
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: front tab, window focused\nAfter1",
            ]
        );

        let records_d3 = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"inst":"a ","ctx":"x","tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"add","inst":"a","ctx":"x","tab":{"front":true,"focused":true},"block":{"text":"After2"}}
"#,
        );
        let produced_d3 = render(&records_d3);
        assert!(produced_d3.warnings.is_empty());
        assert_eq!(produced_d3.error, None);
        assert_eq!(
            produced_d3
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: front tab, window focused\nAfter2",
            ]
        );

        let records_e = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":true},"lang":"en-US"}
{"t":"delta","ts":2,"op":"add","block":{"text":"Legacy line"}}
{"t":"delta","ts":3,"op":"add","tab":{"front":true,"focused":true},"block":{"text":"Still"}}
"#,
        );
        let produced_e = render(&records_e);
        assert!(produced_e.warnings.is_empty());
        assert_eq!(produced_e.error, None);
        assert_eq!(
            produced_e
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused · language en-US",
                "Legacy line",
                "Still",
            ]
        );

        let records_f1 = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"remove","tab":{"front":false},"block":{"text":"gone"}}
{"t":"delta","ts":3,"op":"add","tab":{"front":false},"block":{"text":"kept"}}
"#,
        );
        let produced_f1 = render(&records_f1);
        assert!(produced_f1.warnings.is_empty());
        assert_eq!(produced_f1.error, None);
        assert_eq!(
            produced_f1
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: background tab\nkept",
            ]
        );

        let records_f2 = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":true}}
{"t":"delta","ts":2,"op":"add","tab":{"front":false},"block":{"text":"   "}}
{"t":"delta","ts":3,"op":"add","tab":{"front":false},"block":{"text":"kept"}}
"#,
        );
        let produced_f2 = render(&records_f2);
        assert!(produced_f2.warnings.is_empty());
        assert_eq!(produced_f2.error, None);
        assert_eq!(
            produced_f2
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "context: background tab\nkept",
            ]
        );

        let records_g = parse_jsonl_objects(
            r#"{"t":"delta","ts":1,"op":"add","tab":{"front":false},"block":{"text":"First"}}
{"t":"segment_start","ts":2,"tab":{"front":true,"focused":true}}
{"t":"delta","ts":3,"op":"add","tab":{"front":false},"block":{"text":"Again"}}
"#,
        );
        let produced_g = render(&records_g);
        assert!(produced_g.warnings.is_empty());
        assert_eq!(produced_g.error, None);
        assert_eq!(
            produced_g
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "context: background tab\nFirst",
                "## Browser Page\ncontext: front tab, window focused",
                "context: background tab\nAgain",
            ]
        );

        let records_h = parse_jsonl_objects(
            r#"{"t":"delta","ts":1,"op":"add","tab":{"front":true,"focused":true},"block":{"text":"   "}}"#,
        );
        let produced_h = render(&records_h);
        assert!(produced_h.warnings.is_empty());
        assert_eq!(produced_h.error, None);
        assert!(produced_h.chunks.is_empty());

        let records_reset = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"tab":{"front":true,"focused":true}}
{"t":"segment_start","ts":2,"lang":"en"}
{"t":"delta","ts":3,"op":"add","tab":{"front":true,"focused":true},"block":{"text":"After"}}
"#,
        );
        let produced_reset = render(&records_reset);
        assert!(produced_reset.warnings.is_empty());
        assert_eq!(produced_reset.error, None);
        assert_eq!(
            produced_reset
                .chunks
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "## Browser Page\ncontext: front tab, window focused",
                "## Browser Page\ncontext: language en",
                "context: front tab, window focused\nAfter",
            ]
        );
    }

    #[test]
    fn malformed_field_handling() {
        let with_string_tab =
            parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"tab":"x","lang":"en"}"#);
        let without_tab = parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"lang":"en"}"#);
        assert_eq!(
            render(&with_string_tab).chunks[0].content,
            render(&without_tab).chunks[0].content
        );

        let with_string_media = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"media":{"playing":"yes"},"lang":"en"}"#,
        );
        let without_media = parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"lang":"en"}"#);
        assert_eq!(
            render(&with_string_media).chunks[0].content,
            render(&without_media).chunks[0].content
        );

        let neg_age =
            parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"tab":{"front":true,"age_ms":-5}}"#);
        let pos_tab = parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"tab":{"front":true}}"#);
        let prod_neg = render(&neg_age);
        assert_eq!(
            prod_neg.chunks[0].content,
            render(&pos_tab).chunks[0].content
        );
        assert_eq!(
            prod_neg.chunks[0].content,
            "## Browser Page\ncontext: front tab"
        );

        let multiline_lang =
            parse_jsonl_objects(r#"{"t":"segment_start","ts":1,"lang":"en\n## x"}"#);
        let prod_lang = render(&multiline_lang);
        assert_eq!(
            prod_lang.chunks[0].content,
            "## Browser Page\ncontext: language en ## x"
        );
        assert_eq!(
            prod_lang.chunks[0]
                .content
                .lines()
                .filter(|l| l.starts_with("context:"))
                .count(),
            1
        );

        let v65 = "a".repeat(65);
        let v200 = "b".repeat(200);
        for v in [v65, v200] {
            let json = format!(
                r#"{{"t":"segment_start","ts":1,"lang":"en","browser":{{"vendor":"safari","version":"{v}"}}}}"#
            );
            let prod = render(&parse_jsonl_objects(&json));
            assert_eq!(
                prod.chunks[0].content,
                "## Browser Page\ncontext: language en · safari"
            );
            assert!(!prod.chunks[0].content.contains(&v));
        }

        let v64 = "c".repeat(64);
        let json64 = format!(
            r#"{{"t":"segment_start","ts":1,"lang":"en","browser":{{"vendor":"safari","version":"{v64}"}}}}"#
        );
        let prod64 = render(&parse_jsonl_objects(&json64));
        assert_eq!(
            prod64.chunks[0].content,
            format!("## Browser Page\ncontext: language en · safari {v64}")
        );
    }
}
