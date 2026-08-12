// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only ChatGPT export detection, preview, and planning.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use solstone_core_import::ImportPreview;

use crate::shared::{ParsedEntry, has_extension, is_file, plan_entries, read_zip_json};
use crate::{ImportPlan, SkipLocator, SkipReason, SkippedEntry, SourceError};

const CONVERSATIONS: &str = "conversations.json";

/// Detect a ChatGPT archive by its content, not its filename.
pub fn detect(path: &Path) -> Result<bool, SourceError> {
    if !is_file(path) || !has_extension(path, "zip") {
        return Ok(false);
    }
    let value = match read_zip_json(path, CONVERSATIONS, "ChatGPT conversations") {
        Ok(value) => value,
        Err(SourceError::ArchiveMemberMissing { .. }) => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(conversations) = value.as_array() else {
        return Ok(false);
    };
    let Some(first) = conversations.first().and_then(Value::as_object) else {
        return Ok(false);
    };
    Ok(first.contains_key("mapping"))
}

/// Preview the atomic message count and UTC date range for a ChatGPT archive.
pub fn preview(path: &Path) -> Result<ImportPreview, SourceError> {
    let plan = plan(path)?;
    Ok(ImportPreview {
        date_range: plan.date_range,
        item_count: plan.item_count,
        entity_count: 0,
        summary: format!("{} messages from ChatGPT export", plan.item_count),
    })
}

/// Parse a ChatGPT archive into a write-free UTC segment plan.
pub fn plan(path: &Path) -> Result<ImportPlan, SourceError> {
    if !is_file(path) {
        return Err(SourceError::UnsupportedPathKind {
            path: path.to_owned(),
        });
    }
    if !has_extension(path, "zip") {
        return Err(SourceError::UnsupportedExtension {
            path: path.to_owned(),
        });
    }
    let value = read_zip_json(path, CONVERSATIONS, "ChatGPT conversations")?;
    let conversations = value
        .as_array()
        .ok_or_else(|| SourceError::InvalidJsonShape {
            path: path.to_owned(),
            context: "ChatGPT conversations",
        })?;
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    for (conversation_index, conversation) in conversations.iter().enumerate() {
        parse_conversation(conversation, conversation_index, &mut entries, &mut skipped);
    }
    Ok(plan_entries(entries, skipped))
}

fn parse_conversation(
    conversation: &Value,
    conversation_index: usize,
    entries: &mut Vec<ParsedEntry>,
    skipped: &mut Vec<SkippedEntry>,
) {
    let Some(conversation) = conversation.as_object() else {
        skip_conversation(
            skipped,
            conversation_index,
            SkipReason::NoImportableConversationContent,
        );
        return;
    };
    let Some(mapping) = conversation.get("mapping").and_then(Value::as_object) else {
        skip_conversation(
            skipped,
            conversation_index,
            SkipReason::MissingConversationMapping,
        );
        return;
    };
    if mapping.is_empty() {
        skip_conversation(
            skipped,
            conversation_index,
            SkipReason::MissingConversationMapping,
        );
        return;
    }
    let Some(messages) = message_path(conversation, mapping) else {
        skip_conversation(
            skipped,
            conversation_index,
            SkipReason::InvalidConversationPath,
        );
        return;
    };
    let entries_before = entries.len();
    let skipped_before = skipped.len();
    for (message_index, message) in messages.into_iter().enumerate() {
        parse_message(message, conversation_index, message_index, entries, skipped);
    }
    if entries.len() == entries_before && skipped.len() == skipped_before {
        skip_conversation(
            skipped,
            conversation_index,
            SkipReason::NoImportableConversationContent,
        );
    }
}

fn message_path<'a>(
    conversation: &'a Map<String, Value>,
    mapping: &'a Map<String, Value>,
) -> Option<Vec<&'a Value>> {
    let mut node_id = conversation.get("current_node")?.as_str()?.to_owned();
    let mut seen = BTreeSet::new();
    let mut messages = Vec::new();
    while !node_id.is_empty() {
        if !seen.insert(node_id.clone()) {
            return None;
        }
        let node = mapping.get(&node_id)?.as_object()?;
        if let Some(message) = node.get("message") {
            messages.push(message);
        }
        match node.get("parent") {
            None | Some(Value::Null) => break,
            Some(Value::String(parent)) => node_id = parent.clone(),
            Some(_) => return None,
        }
    }
    messages.reverse();
    Some(messages)
}

fn parse_message(
    message: &Value,
    conversation_index: usize,
    message_index: usize,
    entries: &mut Vec<ParsedEntry>,
    skipped: &mut Vec<SkippedEntry>,
) {
    let Some(message) = message.as_object() else {
        skip_message(
            skipped,
            conversation_index,
            message_index,
            SkipReason::NoImportableConversationContent,
        );
        return;
    };
    let role = message
        .get("author")
        .and_then(Value::as_object)
        .and_then(|author| author.get("role"))
        .and_then(Value::as_str);
    if !matches!(role, Some("user" | "assistant")) {
        skip_message(
            skipped,
            conversation_index,
            message_index,
            SkipReason::UnsupportedMessageRole,
        );
        return;
    }
    let text = message
        .get("content")
        .and_then(Value::as_object)
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        skip_message(
            skipped,
            conversation_index,
            message_index,
            SkipReason::EmptyMessageContent,
        );
        return;
    }
    let timestamp = message.get("create_time").and_then(Value::as_f64);
    let Some(timestamp) = timestamp.and_then(epoch_utc) else {
        skip_message(
            skipped,
            conversation_index,
            message_index,
            SkipReason::InvalidMessageTimestamp,
        );
        return;
    };
    let model_slug = if role == Some("assistant") {
        message
            .get("metadata")
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get("model_slug"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    } else {
        None
    };
    entries.push(ParsedEntry {
        timestamp,
        speaker: if role == Some("user") {
            "Human".to_owned()
        } else {
            "Assistant".to_owned()
        },
        text: text.to_owned(),
        model_slug,
    });
}

fn epoch_utc(value: f64) -> Option<DateTime<Utc>> {
    if !value.is_finite() {
        return None;
    }
    let seconds = value.trunc() as i64;
    let nanos = ((value.fract().abs()) * 1_000_000_000.0).round() as u32;
    DateTime::from_timestamp(seconds, nanos)
}

fn skip_conversation(
    skipped: &mut Vec<SkippedEntry>,
    conversation_index: usize,
    reason: SkipReason,
) {
    skipped.push(SkippedEntry {
        locator: SkipLocator::Conversation {
            conversation_index,
            message_index: None,
        },
        reason,
    });
}

fn skip_message(
    skipped: &mut Vec<SkippedEntry>,
    conversation_index: usize,
    message_index: usize,
    reason: SkipReason,
) {
    skipped.push(SkippedEntry {
        locator: SkipLocator::Conversation {
            conversation_index,
            message_index: Some(message_index),
        },
        reason,
    });
}
