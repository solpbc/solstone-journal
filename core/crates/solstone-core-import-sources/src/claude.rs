// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Claude chat export detection, preview, and planning.

use std::path::Path;

use jiff::tz::TimeZone;
use serde_json::{Map, Value};
use solstone_core_import::ImportPreview;

use crate::shared::{
    ParsedEntry, PlannedThread, SourcePathKind, ThreadTitleFallback, has_extension, parse_iso_utc,
    plan_entries, read_zip_json, skip_conversation, skip_message, source_path_kind,
};
use crate::{ImportPlan, SkipReason, SkippedEntry, SourceError};

const CONVERSATIONS: &str = "conversations.json";

/// Detect a Claude archive by its content, not its filename.
pub fn detect(path: &Path) -> Result<bool, SourceError> {
    if source_path_kind(path)? != SourcePathKind::File
        || !(has_extension(path, "zip") || has_extension(path, "dms"))
    {
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

/// Preview the atomic message count and local date range for a Claude archive.
pub fn preview(path: &Path, zone: &TimeZone) -> Result<ImportPreview, SourceError> {
    let plan = plan(path, zone)?;
    let locale = solstone_core_cli_copy::cli_locale();
    let messages = crate::save::cardinal_count(
        locale,
        "journal.cli.importer.count.message",
        plan.item_count,
    );
    Ok(ImportPreview {
        date_range: plan.date_range,
        item_count: plan.item_count,
        entity_count: 0,
        summary: solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.importer.preview.claude",
            [("messages", &messages)],
        ),
    })
}

/// Parse a Claude archive into a write-free segment plan on the given zone's days.
pub fn plan(path: &Path, zone: &TimeZone) -> Result<ImportPlan, SourceError> {
    if source_path_kind(path)? != SourcePathKind::File {
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
    let mut threads = Vec::with_capacity(conversations.len());
    for (conversation_index, conversation) in conversations.iter().enumerate() {
        let (title, title_fallback) = conversation_title(conversation, "name", conversation_index);
        threads.push(PlannedThread::from_export(
            conversation,
            title,
            title_fallback,
            &["uuid", "id"],
            &["conversation_url", "url"],
        ));
        parse_conversation(conversation, conversation_index, &mut entries, &mut skipped);
    }
    Ok(plan_entries(entries, skipped, threads, zone))
}

/// A conversation's title, or its position when the export names none.
fn conversation_title(
    conversation: &Value,
    key: &str,
    index: usize,
) -> (String, Option<ThreadTitleFallback>) {
    match conversation
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        Some(title) => (title.to_owned(), None),
        None => (
            format!("Conversation {}", index + 1),
            Some(ThreadTitleFallback::Conversation(index + 1)),
        ),
    }
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
        let sent_at = string_at(message, "created_at").and_then(parse_iso_utc);
        let Some(timestamp) = sent_at.or(conversation_timestamp) else {
            skip_message(
                skipped,
                conversation_index,
                message_index,
                SkipReason::NoUsableTimestamp,
            );
            continue;
        };
        entries.push(ParsedEntry {
            sent_at,
            timestamp,
            speaker: match string_at(message, "sender") {
                Some("human") => "Human",
                Some("assistant") => "Assistant",
                _ => "Unknown",
            }
            .to_owned(),
            text: text.to_owned(),
            model_slug: None,
            thread: conversation_index,
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
