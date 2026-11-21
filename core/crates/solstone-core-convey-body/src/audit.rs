// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The overview's import audit: each new-path import, and each old bundle.
//!
//! This reads an import's summary and how far it got, and a bundle's manifest:
//! never a window.

use solstone_core_convey_http::window_catalog::{Value as CatalogValue, render, strings, text};
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_import::metadata::{
    AttemptHolder, AttemptRead, AttemptState, attempt_holder, read_attempt_facts, read_provenance,
};
use solstone_core_location::old_imports::{
    NewImport, PROGRESS_FILE, read_new_imports, read_old_bundles,
};

use crate::fmt::{cardinal_quantity, clock, grouped, month_abbr};
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
            if root
                .join("imports")
                .join(&import.id)
                .join("summary.md")
                .is_file()
            {
                "finished"
            } else {
                "stopped"
            }
        }
    }
}

/// How far a running or stopped import got, from `body-progress.json`, in the
/// owner's words. Never names a stage or a window.
pub(crate) fn progress_line(progress: &Value, running: bool, locale: &str) -> String {
    let days_done = progress.get("days_done").and_then(Value::as_u64);
    let days_total = progress.get("days_total").and_then(Value::as_u64);
    let rows_scanned = progress.get("rows_scanned").and_then(Value::as_u64);
    match (days_done, days_total, rows_scanned) {
        (Some(done), Some(total), _) if total > 0 => render(
            locale,
            if running {
                "journal.window.body.import.progress.running.days"
            } else {
                "journal.window.body.import.progress.stopped.days"
            },
            &[
                ("done_quantity", cardinal_quantity(done)),
                ("total_quantity", cardinal_quantity(total)),
                ("done_days", CatalogValue::String(&done.to_string())),
                ("total_days", CatalogValue::String(&total.to_string())),
            ],
        ),
        (_, _, Some(rows)) if rows > 0 => render(
            locale,
            if running {
                "journal.window.body.import.progress.running.entries"
            } else {
                "journal.window.body.import.progress.stopped.entries"
            },
            &[
                ("quantity", cardinal_quantity(rows)),
                ("entry_count", CatalogValue::String(&grouped(rows))),
            ],
        ),
        _ => text(
            locale,
            if running {
                "journal.window.body.import.progress.running.empty"
            } else {
                "journal.window.body.import.progress.stopped.empty"
            },
        ),
    }
}

fn when_label(id: &str, locale: &str) -> Option<String> {
    let at = jiff::civil::DateTime::strptime("%Y%m%d_%H%M%S", id).ok()?;
    Some(strings(
        locale,
        "journal.window.body.date.with.clock",
        [
            ("month", &month_abbr(at.month(), locale)),
            ("day", &at.day().to_string()),
            ("year", &at.year().to_string()),
            ("time", &clock(at, locale)),
        ],
    ))
}

fn new_import_item(root: &Path, import: &NewImport, locale: &str) -> Value {
    let dir = root.join("imports").join(&import.id);
    let state = import_state(root, import);
    let summary = (state == "finished")
        .then(|| fs::read_to_string(dir.join("summary.md")).ok())
        .flatten();
    let progress = (state != "finished").then(|| {
        let progress = fs::read(dir.join(PROGRESS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .unwrap_or(Value::Null);
        progress_line(&progress, state == "running", locale)
    });
    json!({
        "id": import.id,
        "source": import.source.words(),
        "when": when_label(&import.id, locale),
        "state": state,
        "summary": summary,
        "progress": progress,
    })
}

/// New-path imports, newest first, and old bundles.
pub(crate) fn imports_section(root: &Path, locale: &str) -> Value {
    let mut new_imports = read_new_imports(root);
    new_imports.reverse();
    let new_items = new_imports
        .iter()
        .map(|import| new_import_item(root, import, locale))
        .collect::<Vec<_>>();
    let old = read_old_bundles(root);
    let old_items = old
        .iter()
        .map(|bundle| listing_item(bundle, locale))
        .collect::<Vec<_>>();
    json!({
        "new": new_items,
        "old": old_items,
    })
}
