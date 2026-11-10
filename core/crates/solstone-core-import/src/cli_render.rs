// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure owner-facing rendering for the journal importer.

use std::path::Path;

use serde::Serialize;

use crate::{ImportPreview, ImportResult, RegistrySource};

/// What the phone says of an import that stays on the desktop importer.
pub const EMBEDDED_BODY_REFUSAL: &str =
    "Apple Health import, Oura sync, and Oura connect stay on the desktop importer.";

/// Observable result of a library-hosted journal importer invocation.
#[derive(Debug, Eq, PartialEq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

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
        description: "Import events from ICS calendar files or a Google Calendar export ZIP",
    },
    ImporterRow {
        name: "claude",
        display_name: "Claude Chat History",
        file_patterns: &["*.zip", "*.dms"],
        description: "Import conversations from a Claude chat export",
    },
    ImporterRow {
        name: "chatgpt",
        display_name: "ChatGPT History",
        file_patterns: &["*.zip"],
        description: "Import conversations from a ChatGPT export",
    },
    ImporterRow {
        name: "gemini",
        display_name: "Gemini Activity History",
        file_patterns: &["*.zip", "*.json"],
        description: "Import conversations from a Google Takeout Gemini or Bard export",
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
        file_patterns: &["*.png", "*.jpg", "*.jpeg", "*.webp", "*.gif"],
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
    ImporterRow {
        name: "strava",
        display_name: "Strava",
        file_patterns: &["*.zip", "*.csv"],
        description: "Workouts from a Strava download (only its activities.csv is read)",
    },
    ImporterRow {
        name: "evernote",
        display_name: "Evernote",
        file_patterns: &["*.enex"],
        description: "Notes from an Evernote export",
    },
];

pub const BACKENDS: &[&str] = &["plaud", "audio", "oura"];

/// What a retired notes-folder form tells the owner instead of importing.
pub const NOTES_FOLDER_IMPORT_RETIRED: &str = "importing a folder of notes is gone; an Obsidian vault comes into your journal when you turn it on in the solstone app on the computer where the vault lives";

pub const HELP: &str = concat!(
    "usage: solstone journal importer [-h] [--timestamp TIMESTAMP] [--facet FACET] [--no-facet]\n",
    "                        [--setting SETTING] [--source SOURCE] [--force]\n",
    "                        [--auto [AUTO]] [--dry-run] [--confirm-body-save]\n",
    "                        [--date-from DATE_FROM] [--date-to DATE_TO]\n",
    "                        [--deterministic-only] [--backends] [--sync BACKEND]\n",
    "                        [--save] [--path PATH] [--window-days WINDOW_DAYS]\n",
    "                        [--scheduled] [--connect BACKEND] [--list-importers]\n",
    "                        [--audio-archive PATH] [--source-zone IANA] [--resume IMPORT_ID]\n",
    "                        [--audio-archive-status IMPORT_ID] [--audio-archive-members IMPORT_ID]\n",
    "                        [--offset OFFSET] [--limit LIMIT]\n",
    "                        [--json] [-v] [-d] [media]\n\n",
    "Import a media file into the journal\n\n",
    "positional arguments:\n  media                 Path to audio or text file\n\n",
    "options:\n  -h, --help            show this help message and exit\n",
    "  --timestamp TIMESTAMP\n                        Generic .txt/.md use this, or --auto to adopt a detected timestamp (--timestamp YYYYMMDD_HHMMSS, or a date and time with its offset, such as 2025-11-02T01:30:00-07:00)\n",
    "  --facet FACET         File this audio or text file, or this body import, under FACET,\n",
    "                        one of your unmuted facets, instead of letting your journal choose\n",
    "                        (for a body import: your remembered facet, or none)\n",
    "  --no-facet            File this body import under no facet, whatever the remembered\n",
    "                        default is\n",
    "  --setting SETTING\n",
    "  --source SOURCE       Required for classified formats (ics, chat export, image, PDF)\n",
    "  --force               Force re-import; body sources create a separate import\n",
    "  --auto [AUTO]         Adopt the detected timestamp; AUTO is guidance, not the file path\n",
    "  --dry-run             Show what would be imported without writing to the journal\n",
    "  --confirm-body-save   Confirm this run may save sensitive body importer output\n",
    "  --date-from DATE_FROM\n  --date-to DATE_TO\n",
    "  --deterministic-only  When no date can be read from the file, skip it (exit 0)\n",
    "                        instead of failing\n",
    "  --backends            List syncable importer backends\n  --sync BACKEND\n",
    "  --save                With --sync: download and import new files (default is dry-run)\n",
    "  --path PATH\n  --window-days WINDOW_DAYS\n  --scheduled\n  --connect BACKEND\n",
    "  --list-importers      List available file importers\n",
    "  --audio-archive PATH  Import a folder of Ogg Opus recordings or a JSON manifest\n",
    "                        Folder: Limitless filenames and --source-zone are required.\n",
    "                        Manifest: {\"members\":[{\"path\":\"clip.ogg\",\"instant\":\"2025-11-02T01:30:00-06:00\"}]}\n",
    "                        Or use local_civil and source_zone for a member's time.\n",
    "                        Paths are absolute or relative to the manifest.\n",
    "                        Explicit manifest times take precedence over filenames.\n",
    "                        Supports single-stream mono/stereo Ogg Opus; other containers are left out.\n",
    "                        Shows size before saving; --dry-run saves nothing.\n",
    "  --source-zone IANA    Time zone for folder audio archives or civil times (e.g. America/New_York)\n",
    "  --resume IMPORT_ID    Fill unsaved parts of an audio archive's frozen plan\n",
    "  --audio-archive-status IMPORT_ID\n                        Print compact status for an audio archive run\n",
    "  --audio-archive-members IMPORT_ID\n                        List members for an audio archive run (use with --offset and --limit)\n",
    "  --offset OFFSET       Starting member offset for member listing\n",
    "  --limit LIMIT         Maximum members to list (default 100, max 1000)\n",
    "  --json                Output results as JSON; archive saves emit progress objects\n",
    "                        and a final result, one JSON object per line.\n",
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

pub fn resolution_skipped(reason: crate::SkipReason) -> String {
    let reason = match reason {
        crate::SkipReason::AlreadyImported => {
            "this file was already imported; use --force to import it again"
        }
        crate::SkipReason::NoDeterministicMatch => {
            "no date could be read from the file; name one with --timestamp YYYYMMDD_HHMMSS"
        }
        crate::SkipReason::TimestampRequired => {
            "the file's date needs confirming; rerun with --timestamp or --auto"
        }
    };
    format!("Import skipped: {reason}\n")
}

pub fn timestamp_confirmation(timestamp: &str) -> String {
    format!("detected timestamp {timestamp}; rerun with --timestamp {timestamp} or --auto\n")
}

/// Where an owner asks for a transcript layout the journal does not read yet: the support
/// form, filled in as a report from the import app. The import page links the same place.
pub const UNTIMED_TRANSCRIPT_SUPPORT_URL: &str = "https://support.solstone.app/#report=v1&app=import&state=transcript%20times%20not%20recognized";

pub fn generic_text_complete(segments: usize, untimed: bool) -> String {
    let mut output = format!("Generic text import complete: segments={segments}\n");
    if untimed {
        output.push_str(&format!(
            "no turn times found in this file, so it landed as one segment at the import's start time, each line as written.\n\
             if your transcripts mark times in a way the journal doesn't read yet, ask us to add it: {UNTIMED_TRANSCRIPT_SUPPORT_URL}\n"
        ));
    }
    output
}

pub fn audio_sync_preview(source: &Path, files: usize, errors: usize) -> String {
    format!(
        "Audio sync preview complete: source={} files={files} errors={errors}\n",
        source.display()
    )
}

/// The outcome of a sync that saved: how many items reached the journal, and how many failed.
pub fn sync_save_complete(backend: &str, saved: u64, errors: usize) -> String {
    format!("{backend} sync complete: saved={saved} errors={errors}\n")
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

/// What the owner reads about body data that came in on another time zone.
pub fn body_zones_differ(source: &str, hours: u64) -> String {
    if hours == 0 {
        return format!(
            "less than an hour of {source} may show at the wrong time: it was saved in a different time zone from the one this journal holds for {source}.\n"
        );
    }
    let (unit, were) = if hours == 1 {
        ("hour", "it was")
    } else {
        ("hours", "they were")
    };
    format!(
        "{hours} {unit} of {source} may show at the wrong time: {were} saved in a different time zone from the one this journal holds for {source}.\n"
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

#[cfg(test)]
mod tests {
    use super::body_zones_differ;

    #[test]
    fn a_zone_line_claims_no_more_of_the_clock_than_it_covers() {
        assert!(body_zones_differ("Oura", 0).starts_with("less than an hour of Oura"));
        assert!(body_zones_differ("Oura", 1).starts_with("1 hour of Oura"));
        assert!(body_zones_differ("Oura", 3).starts_with("3 hours of Oura"));
    }
}
