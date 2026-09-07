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

/// X-02: health's banner is the single source for this sentence, and it names
/// the device. Home hid the name behind "one of your devices" while health and
/// the status pane both said which one, so the same red fact read three ways.
/// Health's strings are `HEALTH_GLANCE_DEVICE_FAILING` and
/// `HEALTH_GLANCE_DEVICES_FAILING`; this echoes them.
pub fn format_degraded_capture_line(capture: &Value) -> Option<String> {
    (capture.is_object() && capture.get("status").and_then(Value::as_str) == Some("degraded")).then(
        || {
            let (count, labels, named) = failing_clients(capture);
            match (count, labels.as_slice(), named_attention_sources(capture)) {
                (1, [label], _) => format!("{label} isn't reaching your journal."),
                // Nameless devices are counted, never listed: "an unnamed
                // device, an unnamed device" told the owner nothing twice.
                (many, _, _) if many > 1 && !named => {
                    format!("{many} devices aren't reaching your journal.")
                }
                (many, labels, _) if many > 1 => format!(
                    "{many} devices aren't reaching your journal: {}.",
                    labels.join(", ")
                ),
                // Nothing here names a device. The sources it is refusing are
                // still more than "something is wrong", so they keep the line.
                (_, _, Some(sources)) => format!(
                    "the solstone app on one of your devices is having trouble adding {sources} to your journal."
                ),
                _ => "a device isn't reaching your journal.".to_owned(),
            }
        },
    )
}

/// Rewritten so the caller can count the devices separately from the labels it
/// can print: a run of nameless devices is one label and several devices.
fn failing_clients(capture: &Value) -> (usize, Vec<String>, bool) {
    let labels = failing_client_names(capture);
    let count = capture
        .get("clients")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|client| client.get("failing").and_then(Value::as_bool) == Some(true))
        .count();
    let named = labels
        .iter()
        .any(|label| !label.ends_with("unnamed device") && !label.ends_with("unnamed devices"));
    (count, labels, named)
}

/// The devices whose deliveries are being refused, named the way health names
/// them: the same `failing` flag, in the same quietest-first order, so the two
/// surfaces name the same devices in the same sequence (F-8). A device with no
/// name of its own is still one of the devices the sentence is about, so it
/// counts even though it cannot be listed; several of them collapse into one
/// count rather than repeating "an unnamed device, an unnamed device".
fn failing_client_names(capture: &Value) -> Vec<String> {
    let mut failing = capture
        .get("clients")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|client| client.get("failing").and_then(Value::as_bool) == Some(true))
        .collect::<Vec<_>>();
    // health's byQuietestFirst: longest since its last capture comes first, and
    // a device with no elapsed reading sorts last.
    failing.sort_by_key(|client| {
        std::cmp::Reverse(
            client
                .get("capture_elapsed_ms")
                .and_then(Value::as_i64)
                .unwrap_or(-1),
        )
    });
    let mut names = Vec::new();
    let mut unnamed = 0usize;
    for client in failing {
        match client
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            Some(name) => names.push(name.to_owned()),
            None => unnamed += 1,
        }
    }
    match unnamed {
        0 => {}
        1 => names.push("an unnamed device".to_owned()),
        many => names.push(format!("{many} unnamed devices")),
    }
    names
}

pub(crate) fn source_display_name(source: &str) -> &str {
    if source.is_empty() { "default" } else { source }
}

pub(crate) fn named_attention_sources(capture: &Value) -> Option<String> {
    let clients = capture.get("clients")?.as_array()?;
    let mut names = Vec::new();
    for client in clients {
        let Some(map) = client.get("source_delivery").and_then(Value::as_object) else {
            continue;
        };
        if map.len() <= 1 {
            continue;
        }
        for (source, row) in map {
            if row.get("state").and_then(Value::as_str) == Some("needs_attention") {
                names.push(source_display_name(source).to_owned());
            }
        }
    }
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
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

    /// X-02: a degraded device is named, the way health's banner names it. The
    /// source-named line survives only for the case that has no device name to
    /// give, which is the one place it was carrying real information.
    #[test]
    fn degraded_capture_line_names_the_device_health_names() {
        assert_eq!(
            format_degraded_capture_line(&json!({
                "status": "degraded",
                "clients": [{"name": "iPhone's iPhone", "failing": true}]
            }))
            .as_deref(),
            Some("iPhone's iPhone isn't reaching your journal.")
        );
        // F-8: quietest first, the order health's byQuietestFirst produces.
        assert_eq!(
            format_degraded_capture_line(&json!({
                "status": "degraded",
                "clients": [
                    {"name": "iPhone's iPhone", "failing": true, "capture_elapsed_ms": 60_000},
                    {"name": "suze", "failing": true, "capture_elapsed_ms": 3_600_000}
                ]
            }))
            .as_deref(),
            Some("2 devices aren't reaching your journal: suze, iPhone's iPhone.")
        );
        // F-8: an old rejection on a device health does not call failing is not
        // a second failing device. Neither surface names it, or both do.
        assert_eq!(
            format_degraded_capture_line(&json!({
                "status": "degraded",
                "clients": [
                    {"name": "suze", "failing": true},
                    {"name": "iPhone's iPhone", "failing": false, "ingest_rejection": {"active_count": 1}}
                ]
            }))
            .as_deref(),
            Some("suze isn't reaching your journal.")
        );
        assert_eq!(
            format_degraded_capture_line(&json!({
                "status": "degraded",
                "clients": [{"failing": true}]
            }))
            .as_deref(),
            Some("an unnamed device isn't reaching your journal.")
        );
        // F-8: several nameless devices are a count, not the same phrase twice.
        assert_eq!(
            format_degraded_capture_line(&json!({
                "status": "degraded",
                "clients": [{"failing": true}, {"failing": true}, {"failing": true}]
            }))
            .as_deref(),
            Some("3 devices aren't reaching your journal.")
        );
        assert_eq!(
            format_degraded_capture_line(&json!({
                "status": "degraded",
                "clients": [
                    {"name": "suze", "failing": true},
                    {"failing": true},
                    {"failing": true}
                ]
            }))
            .as_deref(),
            Some("3 devices aren't reaching your journal: suze, 2 unnamed devices.")
        );
        assert_eq!(
            format_degraded_capture_line(&json!({"status": "active"})),
            None
        );
    }

    #[test]
    fn degraded_capture_line_stays_unnamed_for_single_source() {
        let unnamed = "a device isn't reaching your journal.";
        for capture in [
            json!({"status": "degraded"}),
            json!({
                "status": "degraded",
                "clients": [{
                    "source_delivery": {
                        "audio": {"state": "needs_attention"}
                    }
                }]
            }),
            json!({
                "status": "degraded",
                "clients": [{
                    "source_delivery": {
                        "": {"state": "needs_attention"}
                    }
                }]
            }),
        ] {
            assert_eq!(
                format_degraded_capture_line(&capture).as_deref(),
                Some(unnamed)
            );
        }
        assert_eq!(
            format_degraded_capture_line(&json!({
                "status": "degraded",
                "clients": [{
                    "source_delivery": {
                        "audio": {"state": "current"},
                        "location": {"state": "needs_attention"}
                    }
                }]
            }))
            .as_deref(),
            Some(
                "the solstone app on one of your devices is having trouble adding location to your journal."
            )
        );
        assert_eq!(
            format_degraded_capture_line(&json!({
                "status": "degraded",
                "clients": [{
                    "source_delivery": {
                        "audio": {"state": "current"},
                        "": {"state": "needs_attention"}
                    }
                }]
            }))
            .as_deref(),
            Some(
                "the solstone app on one of your devices is having trouble adding default to your journal."
            )
        );
    }
}
