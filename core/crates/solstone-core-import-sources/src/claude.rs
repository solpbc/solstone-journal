// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Claude chat export detection, preview, and planning.

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_import::ImportPreview;

use crate::shared::{
    ParsedEntry, has_extension, is_file, parse_iso_utc, plan_entries, read_zip_json,
};
use crate::{ImportPlan, SkipLocator, SkipReason, SkippedEntry, SourceError};

const CONVERSATIONS: &str = "conversations.json";

/// Detect a Claude archive by its content, not its filename.
pub fn detect(path: &Path) -> Result<bool, SourceError> {
    if !is_file(path) || !(has_extension(path, "zip") || has_extension(path, "dms")) {
        return Ok(false);
    }
    let value = match read_zip_json(path, CONVERSATIONS, "Claude conversations") {
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
    Ok(first.contains_key("chat_messages") && !first.contains_key("mapping"))
}

/// Preview the atomic message count and UTC date range for a Claude archive.
pub fn preview(path: &Path) -> Result<ImportPreview, SourceError> {
    let plan = plan(path)?;
    Ok(ImportPreview {
        date_range: plan.date_range,
        item_count: plan.item_count,
        entity_count: 0,
        summary: format!("{} messages from Claude chat export", plan.item_count),
    })
}

/// Parse a Claude archive into a write-free UTC segment plan.
pub fn plan(path: &Path) -> Result<ImportPlan, SourceError> {
    if !is_file(path) {
        return Err(SourceError::UnsupportedPathKind {
            path: path.to_owned(),
        });
    }
    if !(has_extension(path, "zip") || has_extension(path, "dms")) {
        return Err(SourceError::UnsupportedExtension {
            path: path.to_owned(),
        });
    }
    let value = read_zip_json(path, CONVERSATIONS, "Claude conversations")?;
    let conversations = value
        .as_array()
        .ok_or_else(|| SourceError::InvalidJsonShape {
            path: path.to_owned(),
            context: "Claude conversations",
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
    let Some(messages) = conversation.get("chat_messages").and_then(Value::as_array) else {
        skip_conversation(skipped, conversation_index, SkipReason::EmptyConversation);
        return;
    };
    if messages.is_empty() {
        skip_conversation(skipped, conversation_index, SkipReason::EmptyConversation);
        return;
    }
    let conversation_timestamp = string_at(conversation, "created_at").and_then(parse_iso_utc);
    let entries_before = entries.len();
    let skipped_before = skipped.len();
    for (message_index, message) in messages.iter().enumerate() {
        let Some(message) = message.as_object() else {
            skip_message(
                skipped,
                conversation_index,
                message_index,
                SkipReason::NoImportableConversationContent,
            );
            continue;
        };
        let text = string_at(message, "text").unwrap_or_default();
        if text.is_empty() {
            skip_message(
                skipped,
                conversation_index,
                message_index,
                SkipReason::EmptyMessageText,
            );
            continue;
        }
        let timestamp = string_at(message, "created_at")
            .and_then(parse_iso_utc)
            .or(conversation_timestamp);
        let Some(timestamp) = timestamp else {
            skip_message(
                skipped,
                conversation_index,
                message_index,
                SkipReason::NoUsableTimestamp,
            );
            continue;
        };
        entries.push(ParsedEntry {
            timestamp,
            speaker: if string_at(message, "sender") == Some("human") {
                "Human".to_owned()
            } else {
                "Assistant".to_owned()
            },
            text: text.to_owned(),
            model_slug: None,
        });
    }
    if entries.len() == entries_before && skipped.len() == skipped_before {
        skip_conversation(
            skipped,
            conversation_index,
            SkipReason::NoImportableConversationContent,
        );
    }
}

fn string_at<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
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
