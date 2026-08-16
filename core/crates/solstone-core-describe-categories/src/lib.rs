// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Embedded category metadata shared by categorization and extraction selection.

use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Map, Value, json};

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
    pub label: String,
    pub group: String,
    pub importance: Option<String>,
    pub context: String,
    pub extraction: Option<String>,
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
    label: Option<String>,
    group: Option<String>,
    importance: Option<String>,
}

const SOURCES: [CategorySource; 11] = [
    CategorySource {
        name: "browsing",
        markdown: include_str!("../assets/categories/browsing.md"),
        schema: None,
    },
    CategorySource {
        name: "calendar",
        markdown: include_str!("../assets/categories/calendar.md"),
        schema: Some(include_str!("../assets/categories/calendar.schema.json")),
    },
    CategorySource {
        name: "code",
        markdown: include_str!("../assets/categories/code.md"),
        schema: None,
    },
    CategorySource {
        name: "gaming",
        markdown: include_str!("../assets/categories/gaming.md"),
        schema: None,
    },
    CategorySource {
        name: "media",
        markdown: include_str!("../assets/categories/media.md"),
        schema: None,
    },
    CategorySource {
        name: "meeting",
        markdown: include_str!("../assets/categories/meeting.md"),
        schema: Some(include_str!("../assets/categories/meeting.schema.json")),
    },
    CategorySource {
        name: "messaging",
        markdown: include_str!("../assets/categories/messaging.md"),
        schema: Some(include_str!("../assets/categories/messaging.schema.json")),
    },
    CategorySource {
        name: "productivity",
        markdown: include_str!("../assets/categories/productivity.md"),
        schema: None,
    },
    CategorySource {
        name: "reading",
        markdown: include_str!("../assets/categories/reading.md"),
        schema: None,
    },
    CategorySource {
        name: "social",
        markdown: include_str!("../assets/categories/social.md"),
        schema: None,
    },
    CategorySource {
        name: "terminal",
        markdown: include_str!("../assets/categories/terminal.md"),
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
        label: frontmatter
            .label
            .unwrap_or_else(|| default_label(source.name)),
        group: frontmatter
            .group
            .unwrap_or_else(|| "Screen Analysis".to_owned()),
        importance: frontmatter.importance,
        context: format!("observe.describe.{}", source.name),
        extraction: frontmatter.extraction,
        extractable: !instruction.is_empty(),
        instruction,
        schema: source.schema,
    }
}

/// Render the complete Python-facing category mapping from native definitions.
pub fn category_registry() -> Value {
    let mut registry = Map::new();
    for category in CATEGORIES_META.iter() {
        let mut metadata = Map::new();
        metadata.insert("description".to_owned(), json!(category.description));
        metadata.insert(
            "output".to_owned(),
            json!(match category.output {
                OutputKind::Json => "json",
                OutputKind::Markdown => "markdown",
            }),
        );
        metadata.insert(
            "max_output_tokens".to_owned(),
            json!(category.max_output_tokens),
        );
        metadata.insert("label".to_owned(), json!(category.label));
        metadata.insert("group".to_owned(), json!(category.group));
        metadata.insert("context".to_owned(), json!(category.context));
        if let Some(extraction) = &category.extraction {
            metadata.insert("extraction".to_owned(), json!(extraction));
        }
        if let Some(importance) = &category.importance {
            metadata.insert("importance".to_owned(), json!(importance));
        }
        if category.extractable {
            metadata.insert("prompt".to_owned(), json!(category.instruction));
        }
        if let Some(schema) = category.schema {
            metadata.insert(
                "json_schema".to_owned(),
                serde_json::from_str(schema).expect("category schema is valid JSON"),
            );
        }
        registry.insert(category.name.to_owned(), Value::Object(metadata));
    }
    Value::Object(registry)
}

fn split_frontmatter(source: &str) -> (&str, &str) {
    let end = source
        .find("\n}\n")
        .expect("embedded category markdown has JSON frontmatter")
        + 2;
    (&source[..end], &source[end..])
}

fn default_label(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut characters = word.chars();
            let Some(first) = characters.next() else {
                return String::new();
            };
            format!("{}{}", first.to_ascii_uppercase(), characters.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{CATEGORIES_META, OutputKind, category_registry};

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
    }

    #[test]
    fn registry_preserves_each_category_context() {
        let registry = category_registry();
        let categories = registry.as_object().expect("category registry object");
        assert_eq!(categories.len(), CATEGORIES_META.len());
        for category in CATEGORIES_META.iter() {
            assert_eq!(
                categories[category.name]["context"],
                format!("observe.describe.{}", category.name),
                "{} context",
                category.name
            );
        }
    }
}
