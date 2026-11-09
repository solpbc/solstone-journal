// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The overview's import audit: each new-path import, and each old bundle.
//!
//! This reads an import's summary and how far it got, and a bundle's manifest:
//! never a window.

use std::fs;
use std::path::Path;

use chrono::NaiveDateTime;
use serde_json::{Value, json};
use solstone_core_import::metadata::{
    AttemptHolder, AttemptRead, AttemptState, attempt_holder, read_attempt_facts, read_provenance,
};
use solstone_core_location::old_imports::{NewImport, PROGRESS_FILE, read_new_imports, read_old_bundles};

use crate::fmt::{clock, count_noun, month_abbr};
use crate::old_lines::listing_item;

/// How a new-path import reads now.
fn import_state(root: &Path, import: &NewImport) -> &'static str {
    let attempt = read_provenance(root, &import.id)
        .ok()
        .flatten()
        .map(|metadata| read_attempt_facts(&metadata));
    match attempt {
        Some(AttemptRead::Present(facts)) => match facts.state {
            AttemptState::Completed => "finished",
            AttemptState::Unconfirmed => "stopped",
            AttemptState::Running => match attempt_holder(root, &import.id) {
                AttemptHolder::Held => "running",
                AttemptHolder::Released | AttemptHolder::Unknown => "stopped",
            },
        },
        // An import with no attempt on record has only its summary to go by.
        _ => {
            if root.join("imports").join(&import.id).join("summary.md").is_file() {
                "finished"
            } else {
                "stopped"
            }
        }
    }
}

/// How far a running or stopped import got, from `body-progress.json`, in the
/// owner's words. Never names a stage or a window.
pub(crate) fn progress_line(progress: &Value, running: bool) -> String {
    let lead = if running { "still running" } else { "stopped partway" };
    let days_done = progress.get("days_done").and_then(Value::as_u64);
    let days_total = progress.get("days_total").and_then(Value::as_u64);
    let rows_scanned = progress.get("rows_scanned").and_then(Value::as_u64);
    match (days_done, days_total, rows_scanned) {
        (Some(done), Some(total), _) if total > 0 => {
            format!("{lead}: {done} of {total} days done.")
        }
        (_, _, Some(rows)) if rows > 0 => {
            format!("{lead}: {} read so far.", count_noun(rows, "entry", "entries"))
        }
        _ => format!("{lead}, with nothing saved yet."),
    }
}

fn when_label(id: &str) -> Option<String> {
    let at = NaiveDateTime::parse_from_str(id, "%Y%m%d_%H%M%S").ok()?;
    use chrono::Datelike;
    Some(format!(
        "{} {}, {}, {}",
        month_abbr(at.month()),
        at.day(),
        at.year(),
        clock(at)
    ))
}

fn new_import_item(root: &Path, import: &NewImport) -> Value {
    let dir = root.join("imports").join(&import.id);
    let state = import_state(root, import);
    let summary = (state == "finished")
        .then(|| fs::read_to_string(dir.join("summary.md")).ok())
        .flatten();
    let progress = (state != "finished")
        .then(|| {
            let progress = fs::read(dir.join(PROGRESS_FILE))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .unwrap_or(Value::Null);
            progress_line(&progress, state == "running")
        });
    json!({
        "id": import.id,
        "source": import.source.words(),
        "when": when_label(&import.id),
        "state": state,
        "summary": summary,
        "progress": progress,
    })
}

/// New-path imports, newest first, and old bundles.
pub(crate) fn imports_section(root: &Path) -> Value {
    let mut new_imports = read_new_imports(root);
    new_imports.reverse();
    let new_items = new_imports
        .iter()
        .map(|import| new_import_item(root, import))
        .collect::<Vec<_>>();
    let old = read_old_bundles(root);
    let sources_with_old = crate::old_lines::sources_with_old(&old);
    let old_items = old.iter().map(listing_item).collect::<Vec<_>>();
    json!({
        "new": new_items,
        "old": old_items,
        "old_sources": sources_with_old,
    })
}
