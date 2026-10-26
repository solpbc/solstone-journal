// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_format::{ensure_ascii, markdown::literal_text};

use crate::command::CommandOutput;

/// Protect the final agent-facing projection, after filtering and truncation.
/// HTTP consumers and the identifiers used to make requests keep their bytes.
pub(crate) fn literal_output(mut output: CommandOutput) -> CommandOutput {
    output.stdout = literal_stream(output.stdout);
    output.stderr = literal_stream(output.stderr);
    output
}

fn literal_stream(text: String) -> String {
    if let Ok(mut value) = serde_json::from_str::<Value>(&text) {
        literal_values(&mut value);
        let json = serde_json::to_string_pretty(&value).expect("JSON output should serialize");
        let json = if text.is_ascii() {
            ensure_ascii(&json)
        } else {
            json
        };
        return format!("{}\n", literal_json_syntax(&json));
    }
    literal_links(&text)
}

fn literal_links(text: &str) -> String {
    // Markdown links/images and HTML require at least one of these delimiters.
    // Leave ordinary text, paths, IDs and URLs unchanged.
    if text.contains(['[', '<', '&']) {
        literal_text(text)
    } else {
        text.to_owned()
    }
}

fn literal_values(value: &mut Value) {
    match value {
        Value::String(text) => *text = literal_links(text),
        Value::Array(items) => items.iter_mut().for_each(literal_values),
        Value::Object(object) => {
            for (key, value) in object {
                // These are opaque references used in subsequent calls, not
                // prose. In particular, indexed filenames may contain '&' or '['.
                if !matches!(
                    key.as_str(),
                    "id" | "entity_id"
                        | "peer_id"
                        | "src"
                        | "dst"
                        | "from_entity_id"
                        | "to_entity_id"
                        | "path"
                        | "source_path"
                        | "stream"
                        | "facet"
                        | "agent"
                        | "segment"
                        | "segments"
                        | "source_key"
                        | "anchor"
                        | "url"
                        | "week_url"
                ) {
                    literal_values(value);
                }
            }
        }
        _ => {}
    }
}

/// JSON doubles Markdown's protective backslashes. Escape string delimiters in
/// JSON's own syntax too, so raw stdout cannot join a link across two fields.
fn literal_json_syntax(json: &str) -> String {
    let mut output = String::with_capacity(json.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in json.chars() {
        if ch == '"' && !escaped {
            in_string = !in_string;
        }
        if in_string && matches!(ch, '[' | ']' | '(' | ')' | '!' | '<' | '>' | '&' | '`') {
            use std::fmt::Write;
            write!(output, "\\u{:04x}", u32::from(ch)).expect("string write");
        } else {
            output.push(ch);
        }
        escaped = ch == '\\' && !escaped;
    }
    output
}
