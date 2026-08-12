// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure owner-facing rendering for the journal importer.

use std::path::Path;

use serde::Serialize;

use crate::{ImportPreview, ImportResult, RegistrySource};

#[derive(Clone, Copy, Serialize)]
pub struct ImporterRow {
    pub name: &'static str,
    pub display_name: &'static str,
    pub file_patterns: &'static [&'static str],
    pub description: &'static str,
}

pub const IMPORTERS: &[ImporterRow] = &[
    ImporterRow {
        name: "ics",
        display_name: "Google Calendar (ICS)",
        file_patterns: &["*.ics", "*.zip"],
        description: "Import events from ICS calendar files or Google Calendar export ZIP",
    },
    ImporterRow {
        name: "obsidian",
        display_name: "Obsidian / Logseq Vault",
        file_patterns: &["*.md"],
        description: "Import notes from an Obsidian or Logseq vault",
    },
    ImporterRow {
        name: "claude",
        display_name: "Claude Chat History",
        file_patterns: &["*.zip", "*.dms"],
        description: "Import conversations from Claude chat export",
    },
    ImporterRow {
        name: "chatgpt",
        display_name: "ChatGPT History",
        file_patterns: &["*.zip"],
        description: "Import conversations from ChatGPT export",
    },
    ImporterRow {
        name: "kindle",
        display_name: "Kindle Highlights",
        file_patterns: &["*.txt"],
        description: "Import highlights and notes from Kindle's My Clippings.txt",
    },
    ImporterRow {
        name: "gemini",
        display_name: "Gemini Activity History",
        file_patterns: &["*.zip", "*.json"],
        description: "Import activity from Google Takeout Gemini/Bard export",
    },
    ImporterRow {
        name: "document",
        display_name: "Documents",
        file_patterns: &["*.pdf"],
        description: "Import PDF documents with worker-backed text and raster extraction",
    },
    ImporterRow {
        name: "image",
        display_name: "Image",
        file_patterns: &["*.png", "*.jpg", "*.jpeg", "*.webp", "*.gif", "*.tiff"],
        description: "Import a single image and describe its contents with vision",
    },
    ImporterRow {
        name: "journal_archive",
        display_name: "Journal Archive",
        file_patterns: &["*.zip"],
        description: "Merge an exported journal archive into the current journal",
    },
    ImporterRow {
        name: "apple_health",
        display_name: "Apple Health",
        file_patterns: &["apple_health_export/", "export.xml", "*.zip"],
        description: "Preview Apple Health export.xml data without writing to the journal",
    },
    ImporterRow {
        name: "oura",
        display_name: "Oura",
        file_patterns: &["daily_sleep.json", "daily_readiness.json", "sleep.json"],
        description: "Preview Oura API v2 JSON documents (synthetic fixtures; save path is a later, gated phase)",
    },
];

pub const BACKENDS: &[&str] = &["plaud", "obsidian", "audio", "oura"];

pub const HELP: &str = concat!(
    "usage: journal importer [-h] [--timestamp TIMESTAMP] [--facet FACET]\n",
    "                        [--setting SETTING] [--source SOURCE] [--force]\n",
    "                        [--auto [AUTO]] [--dry-run] [--confirm-body-save]\n",
    "                        [--date-from DATE_FROM] [--date-to DATE_TO]\n",
    "                        [--deterministic-only] [--backends] [--sync BACKEND]\n",
    "                        [--save] [--path PATH] [--window-days WINDOW_DAYS]\n",
    "                        [--scheduled] [--connect BACKEND] [--list-importers]\n",
    "                        [--json] [-v] [-d] [media]\n\n",
    "Import a media file into the journal\n\n",
    "positional arguments:\n  media                 Path to audio or text file\n\n",
    "options:\n  -h, --help            show this help message and exit\n",
    "  --timestamp TIMESTAMP\n  --facet FACET\n  --setting SETTING\n  --source SOURCE\n",
    "  --force               Force re-import; body sources create a separate import\n",
    "  --auto [AUTO]         Auto-accept detected timestamp\n",
    "  --dry-run             Show what would be imported without writing to the journal\n",
    "  --confirm-body-save   Confirm this run may save sensitive body importer output\n",
    "  --date-from DATE_FROM\n  --date-to DATE_TO\n",
    "  --deterministic-only  Use only deterministic timestamp detection; skip model detection\n",
    "  --backends            List syncable importer backends\n  --sync BACKEND\n",
    "  --save                With --sync: download and import new files (default is dry-run)\n",
    "  --path PATH\n  --window-days WINDOW_DAYS\n  --scheduled\n  --connect BACKEND\n",
    "  --list-importers      List available file importers\n",
    "  --json                Output results as JSON (file importers only)\n",
    "  -v, --verbose         Enable verbose output\n  -d, --debug            Enable debug logging\n"
);

pub fn importers(json: bool) -> String {
    if json {
        return serde_json::to_string(IMPORTERS).expect("static importer rows serialize") + "\n";
    }
    let mut output = String::from("File importers:\n");
    for row in IMPORTERS {
        output.push_str(&format!(
            "  {:<12} {} ({})\n",
            row.name,
            row.display_name,
            row.file_patterns.join(", ")
        ));
        output.push_str(&format!("               {}\n", row.description));
    }
    output
}

pub fn backends() -> String {
    let mut output = String::from("Syncable backends:\n");
    for backend in BACKENDS {
        output.push_str(&format!("  {backend}\n"));
    }
    output
}

pub fn resolution_skipped(reason: &str) -> String {
    format!("Import skipped: {reason}\n")
}

pub fn generic_text_complete(segments: usize) -> String {
    format!("Generic text import complete: segments={segments}\n")
}

pub fn audio_sync_preview(source: &Path, files: usize, errors: usize) -> String {
    format!(
        "Audio sync preview complete: source={} files={files} errors={errors}\n",
        source.display()
    )
}

pub fn obsidian_sync_preview(source: Option<&Path>, files: usize, errors: usize) -> String {
    let source = source
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "configured vault".to_owned());
    format!("Obsidian sync preview complete: source={source} files={files} errors={errors}\n")
}

pub fn plaud_sync_preview(files: usize) -> String {
    format!("Plaud sync preview complete: files={files}\n")
}

pub fn source_preview(source: RegistrySource, preview: &ImportPreview) -> String {
    format!(
        "{} preview: date_range={}..{} items={} entities={} summary={}\n",
        source.name(),
        preview.date_range.0,
        preview.date_range.1,
        preview.item_count,
        preview.entity_count,
        preview.summary,
    )
}

pub fn source_preview_only_refusal(source: RegistrySource) -> String {
    format!(
        "{} import previews only and writes nothing; rerun with --dry-run\n",
        source.name()
    )
}

pub fn source_import_complete(source: RegistrySource, result: &ImportResult) -> String {
    format!(
        "{} import complete: entries_written={} files_created={} errors={} summary={}\n",
        source.name(),
        result.entries_written,
        result.files_created.len(),
        result.errors.len(),
        result.summary,
    )
}

pub fn source_import_failure(source: RegistrySource, result: &ImportResult) -> String {
    let detail = result
        .hard_failures
        .first()
        .or_else(|| result.errors.first())
        .map(String::as_str)
        .unwrap_or(result.summary.as_str());
    format!("{} import failed: {detail}\n", source.name())
}

pub fn source_dry_run_unsupported(source: RegistrySource) -> String {
    format!(
        "{} import does not support --dry-run; rerun without --dry-run to merge the archive\n",
        source.name()
    )
}

pub fn source_archive_merge_complete(
    source: RegistrySource,
    segments_copied: usize,
    imports_copied: usize,
    entities_created: usize,
    entities_merged: usize,
    facets_created: usize,
    facets_merged: usize,
) -> String {
    format!(
        "{} import complete: segments_copied={segments_copied} imports_copied={imports_copied} entities_created={entities_created} entities_merged={entities_merged} facets_created={facets_created} facets_merged={facets_merged}\n",
        source.name(),
    )
}

pub fn source_archive_already_present(source: RegistrySource) -> String {
    format!(
        "{} import did not merge anything: archive content is already present\n",
        source.name()
    )
}

pub fn source_archive_incomplete(source: RegistrySource, detail: &str) -> String {
    format!("{} import incomplete: {detail}\n", source.name())
}
