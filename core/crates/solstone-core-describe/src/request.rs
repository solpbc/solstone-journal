// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The closed phase-one request shape for native screen description.

use base64::Engine;
use solstone_core_generate::{ContentPart, GenerateRequest};

const PROMPT: &str = include_str!("../../../../solstone/observe/describe.md");
const SCHEMA: &str = include_str!("../../../../solstone/observe/describe.schema.json");
const CATEGORIES: [(&str, &str); 11] = [
    (
        "browsing",
        include_str!("../../../../solstone/observe/categories/browsing.md"),
    ),
    (
        "calendar",
        include_str!("../../../../solstone/observe/categories/calendar.md"),
    ),
    (
        "code",
        include_str!("../../../../solstone/observe/categories/code.md"),
    ),
    (
        "gaming",
        include_str!("../../../../solstone/observe/categories/gaming.md"),
    ),
    (
        "media",
        include_str!("../../../../solstone/observe/categories/media.md"),
    ),
    (
        "meeting",
        include_str!("../../../../solstone/observe/categories/meeting.md"),
    ),
    (
        "messaging",
        include_str!("../../../../solstone/observe/categories/messaging.md"),
    ),
    (
        "productivity",
        include_str!("../../../../solstone/observe/categories/productivity.md"),
    ),
    (
        "reading",
        include_str!("../../../../solstone/observe/categories/reading.md"),
    ),
    (
        "social",
        include_str!("../../../../solstone/observe/categories/social.md"),
    ),
    (
        "terminal",
        include_str!("../../../../solstone/observe/categories/terminal.md"),
    ),
];

pub fn system_instruction(redact_rules: &[String]) -> String {
    let categories = render_categories(&CATEGORIES);
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

/// Category prompt files are JSON-frontmatter markdown. The Python counterpart
/// obtains this exact field through `load_prompt`; keeping extraction here tiny
/// avoids making describe's core depend on Python prompt machinery.
pub fn render_categories(categories: &[(&str, &str)]) -> String {
    categories
        .iter()
        .filter_map(|(name, source)| {
            description(source).map(|description| format!("- {name}: {description}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn description(source: &str) -> Option<&str> {
    let line = source
        .lines()
        .find(|line| line.contains("\"description\""))?;
    let (_, value) = line.split_once(':')?;
    let value = value.trim().trim_end_matches(',').trim();
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        Some(&value[1..value.len() - 1])
    } else {
        None
    }
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
    fn category_rendering_skips_missing_or_malformed_descriptions() {
        let rendered = render_categories(&[
            ("kept", "{\n  \"description\": \"Kept category\"\n}"),
            ("missing", "{\n  \"output\": \"markdown\"\n}"),
            ("malformed", "{\n  \"description\": 12\n}"),
        ]);
        assert_eq!(rendered, "- kept: Kept category");
    }
}
