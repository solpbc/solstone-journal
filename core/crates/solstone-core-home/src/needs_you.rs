// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure needs-you classification.

use serde_json::{Value, json};

pub fn classify_needs_you(attention: &Value, pulse_needs: &[Value]) -> Vec<Value> {
    let mut items = Vec::new();
    if !attention.is_null()
        && !attention.as_bool().is_some_and(|value| !value)
        && let Some(item) = classify_attention(attention)
    {
        items.push(item);
    }
    for item in pulse_needs {
        if let Some(item) = classify_pulse(item) {
            items.push(item);
        }
    }
    items
}

pub fn needs_dedup_key(item: &Value) -> String {
    if let Some(source) = item
        .get("source_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|source| !source.is_empty())
    {
        return source.to_owned();
    }
    let text = display_text(item);
    if let Some(href) = item
        .pointer("/payload/href")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|href| !href.is_empty())
    {
        return href.to_owned();
    }
    format!(
        "text:{}",
        text.to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    )
}

pub fn format_degraded_capture_line(capture: &Value) -> Option<String> {
    (capture.is_object() && capture.get("status").and_then(Value::as_str) == Some("degraded")).then(
        || {
            "the solstone app on one of your devices is having trouble adding to your journal."
                .to_owned()
        },
    )
}

fn classify_attention(value: &Value) -> Option<Value> {
    let text = value
        .get("placeholder_text")
        .and_then(Value::as_str)?
        .trim();
    (!text.is_empty()).then(|| note(text))
}
fn classify_pulse(value: &Value) -> Option<Value> {
    if let Some(object) = value.as_object() {
        let text = object.get("text").and_then(Value::as_str)?.trim();
        if text.is_empty() {
            return None;
        }
        match object.get("kind").and_then(Value::as_str) {
            Some("chat") | Some("confirm") => Some(note(text)),
            Some("route") => object.get("payload").and_then(|value| value.get("href")).and_then(Value::as_str).filter(|href| href.starts_with('/') && !href.starts_with("//")).map(|href| json!({"text":text,"kind":"route","payload":{"href":href},"disabled":false,"reason":""})).or_else(|| Some(disabled(text, "route", "this link isn't available from here."))),
            _ => None,
        }
    } else {
        let text = value.as_str()?.trim();
        (!text.is_empty()).then(|| note(text))
    }
}
fn note(text: &str) -> Value {
    json!({"text":text,"kind":"note","disabled":false,"reason":""})
}
fn disabled(text: &str, kind: &str, reason: &str) -> Value {
    json!({"text":text,"kind":kind,"payload":{},"disabled":true,"reason":reason})
}
fn display_text(item: &Value) -> String {
    item.get("text")
        .or_else(|| item.get("placeholder_text"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if item.is_string() {
                item.as_str().unwrap_or("")
            } else {
                ""
            }
        })
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn injected_needs_cover_note_and_disabled_route() {
        let items = classify_needs_you(
            &json!({"placeholder_text":"the invoice"}),
            &[json!({"text":"unsafe","kind":"route","payload":{"href":"//elsewhere"}})],
        );
        assert_eq!(items[0]["kind"], "note");
        assert_eq!(items[0]["disabled"], false);
        assert!(items[0].get("payload").is_none());
        assert_eq!(items[1]["disabled"], true);
        assert_eq!(items[1]["reason"], "this link isn't available from here.");
        assert_eq!(
            needs_dedup_key(&json!({"text":"  A   Need "})),
            "text:a need"
        );
    }
}
