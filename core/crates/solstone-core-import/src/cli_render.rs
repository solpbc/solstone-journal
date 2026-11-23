// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure owner-facing rendering for the journal importer.

use serde::Serialize;

use crate::{ImportPreview, ImportResult, RegistrySource};

/// What the phone says of an import that stays on the desktop importer.
pub const EMBEDDED_BODY_REFUSAL: &str = "Apple Health exports and Oura data are brought in by a journal on a computer, so nothing was brought in.";

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

const BACKENDS: &[&str] = &["plaud", "oura", "granola"];

/// What a retired notes-folder form tells the owner instead of importing.
pub const NOTES_FOLDER_IMPORT_RETIRED: &str = "importing a folder of notes is gone; an Obsidian vault comes into your journal when you turn it on in the solstone app on the computer where the vault lives";

pub const HELP: &str = concat!(
    "usage: solstone journal importer [-h] [--timestamp TIMESTAMP] [--facet FACET] [--no-facet]\n",
    "                        [--setting SETTING] [--source SOURCE] [--force]\n",
    "                        [--auto [AUTO]] [--dry-run] [--confirm-body-save]\n",
    "                        [--date-from DATE_FROM] [--date-to DATE_TO]\n",
    "                        [--deterministic-only] [--backends] [--sync BACKEND]\n",
    "                        [--save] [--window-days WINDOW_DAYS]\n",
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
    "  --window-days WINDOW_DAYS\n  --scheduled\n  --connect BACKEND\n",
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

pub fn embedded_body_refusal() -> String {
    solstone_core_cli_copy::text("journal.cli.importer.embedded.body.refusal")
}

pub fn notes_folder_import_retired() -> String {
    solstone_core_cli_copy::text("journal.cli.importer.notes.folder.retired")
}

pub fn help() -> String {
    help_for(solstone_core_cli_copy::cli_locale())
}

pub fn help_for(locale: &str) -> String {
    let r = |id: &str| solstone_core_cli_copy::render_for(locale, id, &[]);
    let ind = |id: &str, indent: &str| {
        solstone_core_cli_copy::display_for(locale, id, [("indent", &indent)])
    };
    let manifest_example =
        r#"{"members":[{"path":"clip.ogg","instant":"2025-11-02T01:30:00-06:00"}]}"#;
    [
        r("journal.cli.importer.help.usage"),
        "                        [--setting SETTING] [--source SOURCE] [--force]".to_owned(),
        "                        [--auto [AUTO]] [--dry-run] [--confirm-body-save]".to_owned(),
        "                        [--date-from DATE_FROM] [--date-to DATE_TO]".to_owned(),
        "                        [--deterministic-only] [--backends] [--sync BACKEND]".to_owned(),
        "                        [--save] [--window-days WINDOW_DAYS]".to_owned(),
        "                        [--scheduled] [--connect BACKEND] [--list-importers]".to_owned(),
        "                        [--audio-archive PATH] [--source-zone IANA] [--resume IMPORT_ID]"
            .to_owned(),
        "                        [--audio-archive-status IMPORT_ID] [--audio-archive-members IMPORT_ID]"
            .to_owned(),
        "                        [--offset OFFSET] [--limit LIMIT]".to_owned(),
        "                        [--json] [-v] [-d] [media]".to_owned(),
        String::new(),
        r("journal.cli.importer.help.summary"),
        String::new(),
        r("journal.cli.importer.help.positional.header"),
        ind("journal.cli.importer.help.positional.media", "  "),
        String::new(),
        r("journal.cli.importer.help.options.header"),
        ind("journal.cli.importer.help.opt.help", "  "),
        "  --timestamp TIMESTAMP".to_owned(),
        ind(
            "journal.cli.importer.help.opt.timestamp",
            "                        ",
        ),
        ind("journal.cli.importer.help.opt.facet.line1", "  "),
        ind(
            "journal.cli.importer.help.opt.facet.line2",
            "                        ",
        ),
        ind(
            "journal.cli.importer.help.opt.facet.line3",
            "                        ",
        ),
        ind("journal.cli.importer.help.opt.no.facet.line1", "  "),
        ind(
            "journal.cli.importer.help.opt.no.facet.line2",
            "                        ",
        ),
        "  --setting SETTING".to_owned(),
        ind("journal.cli.importer.help.opt.source", "  "),
        ind("journal.cli.importer.help.opt.force", "  "),
        ind("journal.cli.importer.help.opt.auto", "  "),
        ind("journal.cli.importer.help.opt.dry.run", "  "),
        ind("journal.cli.importer.help.opt.confirm.body", "  "),
        "  --date-from DATE_FROM".to_owned(),
        "  --date-to DATE_TO".to_owned(),
        ind("journal.cli.importer.help.opt.deterministic.line1", "  "),
        ind(
            "journal.cli.importer.help.opt.deterministic.line2",
            "                        ",
        ),
        ind("journal.cli.importer.help.opt.backends", "  "),
        "  --sync BACKEND".to_owned(),
        ind("journal.cli.importer.help.opt.save", "  "),
        "  --window-days WINDOW_DAYS".to_owned(),
        "  --scheduled".to_owned(),
        "  --connect BACKEND".to_owned(),
        ind("journal.cli.importer.help.opt.list.importers", "  "),
        ind("journal.cli.importer.help.opt.audio.archive.line1", "  "),
        ind(
            "journal.cli.importer.help.opt.audio.archive.line2",
            "                        ",
        ),
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.importer.help.opt.audio.archive.line3",
            [
                ("indent", &"                        "),
                ("example", &manifest_example),
            ],
        ),
        ind(
            "journal.cli.importer.help.opt.audio.archive.line4",
            "                        ",
        ),
        ind(
            "journal.cli.importer.help.opt.audio.archive.line5",
            "                        ",
        ),
        ind(
            "journal.cli.importer.help.opt.audio.archive.line6",
            "                        ",
        ),
        ind(
            "journal.cli.importer.help.opt.audio.archive.line7",
            "                        ",
        ),
        ind(
            "journal.cli.importer.help.opt.audio.archive.line8",
            "                        ",
        ),
        ind("journal.cli.importer.help.opt.source.zone", "  "),
        ind("journal.cli.importer.help.opt.resume", "  "),
        "  --audio-archive-status IMPORT_ID".to_owned(),
        ind(
            "journal.cli.importer.help.opt.archive.status",
            "                        ",
        ),
        "  --audio-archive-members IMPORT_ID".to_owned(),
        ind(
            "journal.cli.importer.help.opt.archive.members",
            "                        ",
        ),
        ind("journal.cli.importer.help.opt.offset", "  "),
        ind("journal.cli.importer.help.opt.limit", "  "),
        ind("journal.cli.importer.help.opt.json.line1", "  "),
        ind(
            "journal.cli.importer.help.opt.json.line2",
            "                        ",
        ),
        ind("journal.cli.importer.help.opt.verbose", "  "),
        ind("journal.cli.importer.help.opt.debug", "  "),
    ]
    .join("\n")
        + "\n"
}

fn importer_description_id(name: &str) -> Option<&'static str> {
    match name {
        "ics" => Some("journal.cli.importer.list.desc.ics"),
        "claude" => Some("journal.cli.importer.list.desc.claude"),
        "chatgpt" => Some("journal.cli.importer.list.desc.chatgpt"),
        "gemini" => Some("journal.cli.importer.list.desc.gemini"),
        "document" => Some("journal.cli.importer.list.desc.document"),
        "image" => Some("journal.cli.importer.list.desc.image"),
        "journal_archive" => Some("journal.cli.importer.list.desc.journal.archive"),
        "apple_health" => Some("journal.cli.importer.list.desc.apple.health"),
        "oura" => Some("journal.cli.importer.list.desc.oura"),
        "strava" => Some("journal.cli.importer.list.desc.strava"),
        "evernote" => Some("journal.cli.importer.list.desc.evernote"),
        _ => None,
    }
}

pub fn importers(json: bool) -> String {
    if json {
        return serde_json::to_string(IMPORTERS).expect("static importer rows serialize") + "\n";
    }
    let locale = solstone_core_cli_copy::cli_locale();
    let mut output = format!(
        "{}\n",
        solstone_core_cli_copy::render_for(locale, "journal.cli.importer.list.header", &[])
    );
    for row in IMPORTERS {
        output.push_str(&format!(
            "  {:<12} {} ({})\n",
            row.name,
            row.display_name,
            row.file_patterns.join(", ")
        ));
        let desc = importer_description_id(row.name)
            .map(|id| solstone_core_cli_copy::render_for(locale, id, &[]))
            .unwrap_or_else(|| row.description.to_owned());
        output.push_str(&format!("               {desc}\n"));
    }
    output
}

pub fn sync_backends() -> impl Iterator<Item = &'static str> {
    BACKENDS.iter().copied()
}

pub fn backends() -> String {
    let mut output = format!(
        "{}\n",
        solstone_core_cli_copy::text("journal.cli.importer.backends.header")
    );
    for backend in sync_backends() {
        output.push_str(&format!("  {backend}\n"));
    }
    output
}

pub fn resolution_skipped(reason: crate::SkipReason) -> String {
    let id = match reason {
        crate::SkipReason::AlreadyImported => "journal.cli.importer.skipped.already",
        crate::SkipReason::NoDeterministicMatch => "journal.cli.importer.skipped.no.date",
        crate::SkipReason::TimestampRequired => "journal.cli.importer.skipped.confirm.date",
    };
    format!("{}\n", solstone_core_cli_copy::text(id))
}

pub fn timestamp_confirmation(timestamp: &str) -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::strings(
            "journal.cli.importer.timestamp.confirm",
            [("timestamp", timestamp)],
        )
    )
}

/// Where an owner asks for a transcript layout the journal does not read yet: the support
/// form, filled in as a report from the import app. The import page links the same place.
pub const UNTIMED_TRANSCRIPT_SUPPORT_URL: &str = "https://support.solstone.app/#report=v1&app=import&state=transcript%20times%20not%20recognized";

pub fn generic_text_complete(segments: usize, untimed: bool) -> String {
    let locale = solstone_core_cli_copy::cli_locale();
    let mut output = format!(
        "{}\n",
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.importer.generic.text.complete",
            [("segments", &segments)],
        )
    );
    if untimed {
        output.push_str(&format!(
            "{}\n{}\n",
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.importer.generic.text.untimed.line1",
                &[],
            ),
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.importer.generic.text.untimed.line2",
                [("url", &UNTIMED_TRANSCRIPT_SUPPORT_URL)],
            ),
        ));
    }
    output
}

/// The outcome of a sync that saved: how many items reached the journal, and how many failed.
pub fn sync_save_complete(backend: &str, saved: u64, errors: usize) -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::display(
            "journal.cli.importer.sync.complete",
            [
                ("backend", &backend),
                ("saved", &saved),
                ("errors", &errors),
            ],
        )
    )
}

pub fn plaud_sync_preview(files: usize) -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::display(
            "journal.cli.importer.plaud.preview.complete",
            [("files", &files)],
        )
    )
}

pub fn source_preview(source: RegistrySource, preview: &ImportPreview) -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::display(
            "journal.cli.importer.source.preview",
            [
                ("source", &source.name()),
                ("start", &preview.date_range.0),
                ("end", &preview.date_range.1),
                ("items", &preview.item_count),
                ("entities", &preview.entity_count),
                ("summary", &preview.summary),
            ],
        )
    )
}

pub fn source_import_complete(source: RegistrySource, result: &ImportResult) -> String {
    let files_created = result.files_created.len();
    let errors = result.errors.len();
    format!(
        "{}\n",
        solstone_core_cli_copy::display(
            "journal.cli.importer.source.complete",
            [
                ("source", &source.name()),
                ("entries", &result.entries_written),
                ("files", &files_created),
                ("errors", &errors),
                ("summary", &result.summary),
            ],
        )
    )
}

pub fn source_import_failure(source: RegistrySource, result: &ImportResult) -> String {
    let detail = result
        .hard_failures
        .first()
        .or_else(|| result.errors.first())
        .map(String::as_str)
        .unwrap_or(result.summary.as_str());
    format!(
        "{}\n",
        solstone_core_cli_copy::display(
            "journal.cli.importer.source.failed",
            [("source", &source.name()), ("detail", &detail)],
        )
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
        "{}\n",
        solstone_core_cli_copy::display(
            "journal.cli.importer.source.archive.complete",
            [
                ("source", &source.name()),
                ("segments_copied", &segments_copied),
                ("imports_copied", &imports_copied),
                ("entities_created", &entities_created),
                ("entities_merged", &entities_merged),
                ("facets_created", &facets_created),
                ("facets_merged", &facets_merged),
            ],
        )
    )
}

/// What the owner reads about body data that came in on another time zone.
pub fn body_zones_differ(source: &str, hours: u64) -> String {
    let locale = solstone_core_cli_copy::cli_locale();
    if hours == 0 {
        return format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.importer.body.zones.under.hour",
                [("source", &source)],
            )
        );
    }
    let count = hours.to_string();
    format!(
        "{}\n",
        solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.importer.body.zones.hours",
            &[
                (
                    "quantity",
                    solstone_core_cli_copy::Value::Cardinal(
                        i64::try_from(hours).unwrap_or(i64::MAX)
                    ),
                ),
                ("count", solstone_core_cli_copy::Value::String(&count)),
                ("source", solstone_core_cli_copy::Value::String(source)),
            ],
        )
    )
}

/// What the owner reads about days of body data an archive brought hidden.
pub fn body_days_hidden(days: usize) -> String {
    let count = days.to_string();
    format!(
        "{}\n",
        solstone_core_cli_copy::render(
            "journal.cli.importer.body.days.hidden",
            &[
                (
                    "quantity",
                    solstone_core_cli_copy::Value::Cardinal(
                        i64::try_from(days).unwrap_or(i64::MAX)
                    ),
                ),
                ("count", solstone_core_cli_copy::Value::String(&count)),
            ],
        )
    )
}

pub fn source_archive_already_present(source: RegistrySource) -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::display(
            "journal.cli.importer.source.archive.already.present",
            [("source", &source.name())],
        )
    )
}

pub fn source_archive_incomplete(source: RegistrySource, detail: &str) -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::display(
            "journal.cli.importer.source.archive.incomplete",
            [("source", &source.name()), ("detail", &detail)],
        )
    )
}

#[cfg(test)]
mod tests {
    use super::{HELP, body_zones_differ, help_for};

    #[test]
    fn help_matches_english_const() {
        assert_eq!(help_for(solstone_core_cli_copy::ENGLISH), HELP);
    }

    #[test]
    fn a_zone_line_claims_no_more_of_the_clock_than_it_covers() {
        assert!(body_zones_differ("Oura", 0).starts_with("less than an hour of Oura"));
        assert!(body_zones_differ("Oura", 1).starts_with("1 hour of Oura"));
        assert!(body_zones_differ("Oura", 3).starts_with("3 hours of Oura"));
    }
}
