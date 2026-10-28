// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_format::{
    ensure_ascii,
    markdown::{literal_json_syntax, literal_markup},
};

use crate::command::CommandOutput;
use crate::json_format::{json_compact_ascii, json_compact_utf8};

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
        // Keep the command's layout: one-line JSON stays on one line.
        let json = match (text.trim_end().contains('\n'), text.is_ascii()) {
            (true, true) => ensure_ascii(
                &serde_json::to_string_pretty(&value).expect("JSON output should serialize"),
            ),
            (true, false) => {
                serde_json::to_string_pretty(&value).expect("JSON output should serialize")
            }
            (false, true) => json_compact_ascii(&value),
            (false, false) => json_compact_utf8(&value),
        };
        return format!("{}\n", literal_json_syntax(&json));
    }
    literal_links(&text)
}

/// Escape one journal value inside a text rendering whose own layout uses
/// Markdown delimiters, so the layout itself is left as written.
pub(crate) fn literal_links(text: &str) -> String {
    literal_markup(text)
}

/// The response with each journal value made literal and its references kept,
/// for a text rendering whose own layout must stay as written.
pub(crate) fn literal_body(mut value: Value) -> Value {
    literal_values(&mut value);
    value
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
                        | "name_key"
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
