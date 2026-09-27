// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Rendering a planned conversation export (ChatGPT, Claude, Gemini) for saving.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use solstone_core_import::RegistrySource;

use crate::ImportPlan;
use crate::save::{RenderedImport, SegmentFile};

/// The transcript file a conversation segment carries.
pub const TRANSCRIPT_FILE: &str = "conversation_transcript.jsonl";

/// Render each planned segment as a transcript, and each conversation as one browsable item.
#[must_use]
pub fn render(source: RegistrySource, plan: ImportPlan, import_id: &str) -> RenderedImport {
    let mut threads: BTreeMap<usize, Thread> = BTreeMap::new();
    let mut files = Vec::with_capacity(plan.segments.len());
    for segment in &plan.segments {
        let mut header = Map::new();
        header.insert("imported".to_owned(), json!({ "id": import_id }));
        if let Some(model) = &segment.model_slug {
            header.insert("model".to_owned(), Value::String(model.clone()));
        }
        let mut contents = Value::Object(header).to_string();
        contents.push('\n');
        for entry in &segment.entries {
            let row = json!({
                "start": entry.start,
                "speaker": entry.speaker,
                "text": entry.text,
                "source": "import",
            });
            contents.push_str(&row.to_string());
            contents.push('\n');

            let thread = threads.entry(entry.thread).or_insert_with(|| Thread {
                date: segment.day.clone(),
                ..Thread::default()
            });
            thread.messages += 1;
            if thread.preview.is_none() && entry.speaker == "Human" {
                thread.preview = Some(entry.text.chars().take(200).collect());
            }
            let key = (segment.day.clone(), segment.segment_key.clone());
            if thread.segments.last() != Some(&key) {
                thread.segments.push(key);
            }
        }
        files.push(SegmentFile {
            day: segment.day.clone(),
            segment: segment.segment_key.clone(),
            name: TRANSCRIPT_FILE,
            contents,
        });
    }

    let stream = format!("import.{}", source.name());
    let mut ordered = threads.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|(_, left), (_, right)| left.segments.first().cmp(&right.segments.first()));
    let items = ordered
        .into_iter()
        .map(|(index, thread)| {
            json!({
                "id": format!("conv-{index}"),
                "title": plan.threads.get(index).cloned().unwrap_or_default(),
                "date": thread.date,
                "type": "conversation",
                "preview": thread.preview.unwrap_or_default(),
                "meta": { "message_count": thread.messages },
                "segments": thread
                    .segments
                    .iter()
                    .map(|(day, key)| json!({ "day": day, "key": key, "stream": stream }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    RenderedImport {
        source,
        summary: format!(
            "imported {} messages from {} conversations across {} days",
            plan.item_count,
            items.len(),
            plan.affected_days.len()
        ),
        entries: plan.item_count,
        files,
        items,
    }
}

#[derive(Default)]
struct Thread {
    date: String,
    messages: u64,
    preview: Option<String>,
    segments: Vec<(String, String)>,
}
