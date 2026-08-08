// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Embedded category metadata shared by categorization and extraction selection.

use std::sync::LazyLock;

use serde::Deserialize;

const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Json,
    Markdown,
}

#[derive(Debug, Clone)]
pub struct CategoryMeta {
    pub name: &'static str,
    pub description: String,
    pub output: OutputKind,
    pub max_output_tokens: u64,
    pub context: String,
    pub extraction: Option<String>,
    pub importance: Option<String>,
    pub extractable: bool,
    pub instruction: String,
    pub schema: Option<&'static str>,
}

struct CategorySource {
    name: &'static str,
    markdown: &'static str,
    schema: Option<&'static str>,
}

#[derive(Deserialize)]
struct Frontmatter {
    description: String,
    output: Option<String>,
    max_output_tokens: Option<u64>,
    extraction: Option<String>,
    importance: Option<String>,
}

const SOURCES: [CategorySource; 11] = [
    CategorySource {
        name: "browsing",
        markdown: include_str!("../../../../solstone/observe/categories/browsing.md"),
        schema: None,
    },
    CategorySource {
        name: "calendar",
        markdown: include_str!("../../../../solstone/observe/categories/calendar.md"),
        schema: Some(include_str!(
            "../../../../solstone/observe/categories/calendar.schema.json"
        )),
    },
    CategorySource {
        name: "code",
        markdown: include_str!("../../../../solstone/observe/categories/code.md"),
        schema: None,
    },
    CategorySource {
        name: "gaming",
        markdown: include_str!("../../../../solstone/observe/categories/gaming.md"),
        schema: None,
    },
    CategorySource {
        name: "media",
        markdown: include_str!("../../../../solstone/observe/categories/media.md"),
        schema: None,
    },
    CategorySource {
        name: "meeting",
        markdown: include_str!("../../../../solstone/observe/categories/meeting.md"),
        schema: Some(include_str!(
            "../../../../solstone/observe/categories/meeting.schema.json"
        )),
    },
    CategorySource {
        name: "messaging",
        markdown: include_str!("../../../../solstone/observe/categories/messaging.md"),
        schema: Some(include_str!(
            "../../../../solstone/observe/categories/messaging.schema.json"
        )),
    },
    CategorySource {
        name: "productivity",
        markdown: include_str!("../../../../solstone/observe/categories/productivity.md"),
        schema: None,
    },
    CategorySource {
        name: "reading",
        markdown: include_str!("../../../../solstone/observe/categories/reading.md"),
        schema: None,
    },
    CategorySource {
        name: "social",
        markdown: include_str!("../../../../solstone/observe/categories/social.md"),
        schema: None,
    },
    CategorySource {
        name: "terminal",
        markdown: include_str!("../../../../solstone/observe/categories/terminal.md"),
        schema: None,
    },
];

pub static CATEGORIES_META: LazyLock<Vec<CategoryMeta>> =
    LazyLock::new(|| SOURCES.iter().map(parse).collect());

fn parse(source: &CategorySource) -> CategoryMeta {
    let (frontmatter, instruction) = split_frontmatter(source.markdown);
    let frontmatter: Frontmatter =
        serde_json::from_str(frontmatter).expect("embedded category frontmatter is valid JSON");
    let output = match frontmatter.output.as_deref().unwrap_or("markdown") {
        "json" => OutputKind::Json,
        "markdown" => OutputKind::Markdown,
        other => panic!("embedded category output kind is invalid: {other}"),
    };
    let instruction = instruction.trim().to_owned();
    CategoryMeta {
        name: source.name,
        description: frontmatter.description,
        output,
        max_output_tokens: frontmatter
            .max_output_tokens
            .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
        context: format!("observe.describe.{}", source.name),
        extraction: frontmatter.extraction,
        importance: frontmatter.importance,
        extractable: !instruction.is_empty(),
        instruction,
        schema: source.schema,
    }
}

fn split_frontmatter(source: &str) -> (&str, &str) {
    let end = source
        .find("\n}\n")
        .expect("embedded category markdown has JSON frontmatter")
        + 2;
    (&source[..end], &source[end..])
}

#[cfg(test)]
mod tests {
    use super::{CATEGORIES_META, OutputKind};

    #[test]
    fn embedded_categories_have_expected_metadata() {
        assert_eq!(CATEGORIES_META.len(), 11);
        let gaming = CATEGORIES_META
            .iter()
            .find(|category| category.name == "gaming")
            .expect("gaming category");
        assert_eq!(gaming.output, OutputKind::Markdown);
        assert_eq!(gaming.max_output_tokens, 4096);
        assert!(gaming.instruction.starts_with("# Game Text Extraction"));
        assert!(gaming.extraction.is_none());

        let calendar = CATEGORIES_META
            .iter()
            .find(|category| category.name == "calendar")
            .expect("calendar category");
        assert_eq!(calendar.importance.as_deref(), Some("high"));
        assert_eq!(gaming.importance.as_deref(), Some("ignore"));
        let browsing = CATEGORIES_META
            .iter()
            .find(|category| category.name == "browsing")
            .expect("browsing category");
        assert_eq!(browsing.importance, None);
    }
}
