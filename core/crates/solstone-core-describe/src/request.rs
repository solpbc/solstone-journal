// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The closed phase-one request shape for native screen description.

use base64::Engine;
use solstone_core_generate::{ContentPart, GenerateRequest};

use crate::categories::CATEGORIES_META;
use crate::phone::PhoneScreen;

const PROMPT: &str = include_str!("../assets/describe.md");
const SCHEMA: &str = include_str!("../assets/describe.schema.json");
pub fn system_instruction(redact_rules: &[String], phone: Option<PhoneScreen>) -> String {
    let categories = render_categories();
    let mut prompt = PROMPT.replace("$categories", &categories).trim().to_owned();
    if let Some(phone) = phone {
        prompt.push_str("\n\n");
        prompt.push_str(&phone.describe_note());
    }
    append_redaction(&mut prompt, redact_rules);
    prompt
}

pub fn append_redaction(base: &mut String, redact_rules: &[String]) {
    if redact_rules.is_empty() {
        return;
    }
    base.push_str("\n\nRedaction rules (apply these exactly as written, do not generalize):\n");
    for rule in redact_rules {
        base.push_str("- ");
        base.push_str(rule);
        base.push('\n');
    }
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
    phone: Option<PhoneScreen>,
) -> GenerateRequest {
    GenerateRequest {
        id: Some(format!("frame:{frame_id}:attempt:{attempt_index}")),
        context: "observe.describe.frame".to_owned(),
        contents: vec![
            ContentPart::Text {
                text: phone.map_or_else(
                    || "Analyze this screenshot frame from a screencast recording.".to_owned(),
                    |phone| phone.frame_text(),
                ),
            },
            ContentPart::Image {
                mime_type: "image/png".to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(png),
            },
        ],
        system_instruction: Some(instruction.to_owned()),
        // Categorization is a closed choice that picks the extractor. Sampled at
        // 0.7 the same screen changed categories between tries; greedy matched or
        // beat it on a labelled sample.
        temperature: 0.0,
        max_output_tokens: 256,
        timeout_s: None,
        json_output: true,
        json_schema: Some(serde_json::from_str(SCHEMA).expect("describe schema is valid JSON")),
        attempt_index,
        exclusive_admission: false,
        overflow_text: None,
        transport_retries: None,
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::render_categories;

    #[test]
    fn category_rendering_uses_embedded_metadata() {
        let rendered = render_categories();
        assert!(rendered.contains("- browsing: General web browsing"));
        assert!(rendered.contains("- terminal: Command line interfaces, logs, shell"));
    }
}
