// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The closed phase-one request shape for native screen description.

use base64::Engine;
use solstone_core_generate::{ContentPart, GenerateRequest};

use crate::categories::CATEGORIES_META;

const PROMPT: &str = include_str!("../../../../solstone/observe/describe.md");
const SCHEMA: &str = include_str!("../../../../solstone/observe/describe.schema.json");
pub fn system_instruction(redact_rules: &[String]) -> String {
    let categories = render_categories();
    let mut prompt = PROMPT.replace("$categories", &categories).trim().to_owned();
    if !redact_rules.is_empty() {
        prompt
            .push_str("\n\nRedaction rules (apply these exactly as written, do not generalize):\n");
        for rule in redact_rules {
            prompt.push_str("- ");
            prompt.push_str(rule);
            prompt.push('\n');
        }
    }
    prompt
}

pub fn render_categories() -> String {
    CATEGORIES_META
        .iter()
        .map(|category| format!("- {}: {}", category.name, category.description))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn request(
    frame_id: u64,
    attempt_index: u64,
    png: &[u8],
    instruction: &str,
) -> GenerateRequest {
    GenerateRequest {
        id: Some(format!("frame:{frame_id}:attempt:{attempt_index}")),
        context: "observe.describe.frame".to_owned(),
        contents: vec![
            ContentPart::Text {
                text: "Analyze this screenshot frame from a screencast recording.".to_owned(),
            },
            ContentPart::Image {
                mime_type: "image/png".to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(png),
            },
        ],
        system_instruction: Some(instruction.to_owned()),
        temperature: 0.7,
        max_output_tokens: 512,
        thinking_budget: Some(1024),
        timeout_s: None,
        json_output: true,
        json_schema: Some(serde_json::from_str(SCHEMA).expect("describe schema is valid JSON")),
        enforce_responsiveness: true,
        attempt_index,
        exclusive_admission: false,
        transport_retries: None,
    }
}

#[cfg(test)]
mod tests {
    use super::render_categories;

    #[test]
    fn category_rendering_uses_embedded_metadata() {
        let rendered = render_categories();
        assert!(rendered.contains("- browsing: General web browsing"));
        assert!(rendered.contains("- terminal: Command line interfaces, logs, shell"));
    }
}
