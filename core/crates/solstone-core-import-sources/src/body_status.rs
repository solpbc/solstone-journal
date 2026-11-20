// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Why an Apple Health import stopped or was refused, as a code the run records
//! and the page turns into words.
//!
//! An Apple run writes `imports/<id>/body-stop.json` before the write that ends
//! its attempt, naming the attempt's generation. It only annotates: what an
//! import reads as comes from its attempt, and a code from another generation is
//! ignored.

use solstone_core_window_catalog::{ENGLISH, strings, text};
use std::{collections::HashMap, sync::OnceLock};

use crate::apple_export::EXPORT_INCOMPLETE_TEXT;

/// The file an Apple run leaves when it stops or is refused.
pub const STOP_FILE: &str = "body-stop.json";

/// Whether a code means the export or the request was refused, so nothing was
/// saved and starting again the same way won't help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopClass {
    /// It stopped part way; importing the export again finishes it.
    Stopped,
    /// It never started, because another import was running.
    DidntStart,
    /// Nothing was saved, and the export or request has to change.
    Refused,
}

/// Every code a run records, with its class.
pub const CODES: &[(&str, StopClass)] = &[
    ("disk_full", StopClass::Stopped),
    ("permission", StopClass::Stopped),
    ("windows_not_saving", StopClass::Stopped),
    ("other", StopClass::Stopped),
    ("busy", StopClass::DidntStart),
    ("export_incomplete", StopClass::Refused),
    ("export_too_large", StopClass::Refused),
    ("export_unreadable", StopClass::Refused),
    ("not_this_import", StopClass::Refused),
    ("facet", StopClass::Refused),
    ("go_ahead", StopClass::Refused),
    ("whole_export", StopClass::Refused),
    ("preview_again", StopClass::Refused),
    ("not_started", StopClass::Refused),
    ("no_room", StopClass::Refused),
];

/// The class of a recorded code. A code this build doesn't know reads as a stop.
#[must_use]
pub fn class_of(code: &str) -> StopClass {
    CODES
        .iter()
        .find_map(|(known, class)| (*known == code).then_some(*class))
        .unwrap_or(StopClass::Stopped)
}

/// The code for a stage an Apple export's reading refused at.
#[must_use]
pub fn code_for_source_stage(stage: &str) -> &'static str {
    match stage {
        "permission" => "permission",
        "export_incomplete" => "export_incomplete",
        "export_size_limit" | "source_size_limit" => "export_too_large",
        _ => "export_unreadable",
    }
}

/// The code for a run's error, from its text. Operating-system errors carry
/// their raw number in `(os error N)`, which isn't translated, so a full disk
/// reads the same whatever the system's language.
#[must_use]
pub fn code_for_error_text(text: &str) -> &'static str {
    if text.contains(crate::apple_export::EXPORT_INCOMPLETE)
        || text.contains(EXPORT_INCOMPLETE_TEXT)
    {
        return "export_incomplete";
    }
    if text.contains("export_size_limit") || text.contains("the most one import reads") {
        return "export_too_large";
    }
    // A full disk, and a disk quota used up (EDQUOT), which the owner fixes the
    // same way.
    let full: &[&str] = if cfg!(windows) {
        &["(os error 39)", "(os error 112)"]
    } else if cfg!(target_vendor = "apple") {
        &["(os error 28)", "(os error 69)"]
    } else {
        &["(os error 28)", "(os error 122)"]
    };
    if full.iter().any(|marker| text.contains(marker)) {
        return "disk_full";
    }
    "other"
}

/// What the stop for 64 windows in a row says; the run's message carries it.
pub const STUCK_MARKER: &str = "in a row couldn't be finished";

/// The code for a body run's own error text: a refused export by its stage, the
/// stuck stop by its marker, a full disk by its number.
#[must_use]
pub fn code_for_run_error(text: &str) -> &'static str {
    if text.contains(STUCK_MARKER) {
        return "windows_not_saving";
    }
    if let Some(stage) = text
        .split("body-ingest source: ")
        .nth(1)
        .map(|rest| rest.split_whitespace().next().unwrap_or_default())
    {
        return code_for_source_stage(stage);
    }
    code_for_error_text(text)
}

/// Record why the attempt `generation` of import `import_id` ended, before the
/// write that ends it. Best effort: a file that can't be written leaves the
/// owner the general words.
pub fn record(journal: &std::path::Path, import_id: &str, code: &str, generation: u64) {
    let path = journal.join("imports").join(import_id).join(STOP_FILE);
    if !path.parent().is_some_and(std::path::Path::is_dir) {
        return;
    }
    let body = serde_json::json!({"reason": code, "generation": generation}).to_string();
    let _ = solstone_core_journal_io::atomic_replace(
        &path,
        body.as_bytes(),
        solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
    );
}

/// The recorded reason and the generation it explains, if any.
#[must_use]
pub fn read(journal: &std::path::Path, import_id: &str) -> Option<(String, u64)> {
    let bytes = std::fs::read(journal.join("imports").join(import_id).join(STOP_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some((
        value.get("reason")?.as_str()?.to_owned(),
        value.get("generation")?.as_u64()?,
    ))
}

/// Where an owner on this platform can type an importer command.
pub const WHERE_TO_RUN: &str = if cfg!(target_os = "macos") {
    "in the journal app's journal menu, choose open admin terminal, and run"
} else {
    "from a terminal on the computer your journal runs on, run"
};

/// The go-ahead refusal and its literal terminal command are kept in English.
#[must_use]
pub fn apple_go_ahead_words() -> String {
    strings(
        ENGLISH,
        "journal.window.import.response.preview.apple.go.ahead",
        [("where", WHERE_TO_RUN)],
    )
}

/// What the card says of an export too large for the page to upload.
#[must_use]
pub fn too_big_to_upload_words() -> String {
    too_big_to_upload_words_localized(ENGLISH)
}

pub fn too_big_to_upload_words_localized(locale: &str) -> String {
    let id = if cfg!(target_os = "macos") {
        "journal.window.import.body.status.upload.limit.mac"
    } else {
        "journal.window.import.body.status.upload.limit.other"
    };
    text(locale, id)
}

/// What a too-large export is told, at the terminal as on the page.
#[must_use]
pub fn too_large_words() -> String {
    words("export_too_large").trim_end_matches('.').to_owned()
}

/// What a stopped import's row says: what happened, then what to do. Importing
/// again finishes most stops; a full disk needs room first, and a refused
/// permission isn't fixed by importing again, so it says only what happened.
#[must_use]
pub fn stopped_words(code: &str) -> String {
    stopped_words_localized(ENGLISH, code)
}

pub fn stopped_words_localized(locale: &str, code: &str) -> String {
    let reason = words_localized(locale, code);
    match code {
        "disk_full" => strings(
            locale,
            "journal.window.import.body.status.stop.disk",
            [("reason", &reason)],
        ),
        "permission" => reason,
        _ => strings(
            locale,
            "journal.window.import.body.status.stop.resume",
            [("reason", &reason)],
        ),
    }
}

const REASONS: &[(&str, &str)] = &[
    (
        "disk_full",
        "journal.window.import.body.status.reason.disk.full",
    ),
    (
        "permission",
        "journal.window.import.body.status.reason.permission",
    ),
    (
        "windows_not_saving",
        "journal.window.import.body.status.reason.windows.not.saving",
    ),
    ("busy", "journal.window.import.body.status.reason.busy"),
    (
        "export_incomplete",
        "journal.window.import.body.status.reason.export.incomplete",
    ),
    (
        "export_too_large",
        "journal.window.import.body.status.reason.export.too.large",
    ),
    (
        "export_unreadable",
        "journal.window.import.body.status.reason.export.unreadable",
    ),
    (
        "not_this_import",
        "journal.window.import.body.status.reason.not.this.import",
    ),
    ("facet", "journal.window.import.body.status.reason.facet"),
    (
        "go_ahead",
        "journal.window.import.body.status.reason.go.ahead",
    ),
    (
        "whole_export",
        "journal.window.import.body.status.reason.whole.export",
    ),
    (
        "preview_again",
        "journal.window.import.body.status.reason.preview.again",
    ),
    (
        "not_started",
        "journal.window.import.body.status.reason.not.started",
    ),
    (
        "no_room",
        "journal.window.import.body.status.reason.no.room",
    ),
    ("other", "journal.window.import.body.status.reason.other"),
];

pub fn words_id(code: &str) -> &'static str {
    REASONS
        .iter()
        .find_map(|(known, id)| (*known == code).then_some(*id))
        .unwrap_or("journal.window.import.body.status.reason.other")
}

/// Source-English copy for command output.
#[must_use]
pub fn words(code: &str) -> &'static str {
    static ENGLISH_COPY: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    &ENGLISH_COPY.get_or_init(|| {
        REASONS
            .iter()
            .map(|(_, id)| (*id, text(ENGLISH, id)))
            .collect()
    })[words_id(code)]
}

/// Project the recorded reason in the language selected by the reader.
pub fn words_localized(locale: &str, code: &str) -> String {
    text(locale, words_id(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_has_its_own_words_and_an_unknown_one_reads_as_a_stop() {
        for (code, _) in CODES {
            if *code != "other" {
                assert_ne!(words(code), words("other"), "{code}");
            }
        }
        assert_eq!(class_of("something_new"), StopClass::Stopped);
    }

    #[test]
    fn a_runs_own_error_is_known_by_its_stage_its_marker_or_its_number() {
        assert_eq!(
            code_for_run_error("body-ingest source: permission"),
            "permission"
        );
        assert_eq!(
            code_for_run_error("body-ingest source: archive"),
            "export_unreadable"
        );
        assert_eq!(
            code_for_run_error(&format!(
                "stopped: 64 segments {STUCK_MARKER}; import again"
            )),
            "windows_not_saving"
        );
        assert_eq!(code_for_run_error("something else"), "other");
    }

    #[test]
    fn a_stop_is_recorded_beside_its_import_and_read_back() {
        let journal = tempfile::tempdir().unwrap();
        record(journal.path(), "20261010_120000", "busy", 1);
        assert_eq!(
            read(journal.path(), "20261010_120000"),
            None,
            "no import, nothing written"
        );
        std::fs::create_dir_all(journal.path().join("imports/20261010_120000")).unwrap();
        record(journal.path(), "20261010_120000", "busy", 1);
        assert_eq!(
            read(journal.path(), "20261010_120000"),
            Some(("busy".to_owned(), 1))
        );
    }

    #[test]
    fn the_too_large_words_name_the_bound_the_reader_uses() {
        let bound = crate::body_space::size_words(crate::apple_export::apple_scan_byte_bound());
        let words = words("export_too_large");
        assert!(words.contains(&bound), "{words} against {bound}");
        assert_eq!(code_for_error_text(&too_large_words()), "export_too_large");
    }

    #[test]
    fn the_too_large_words_name_the_pages_bound_and_where_to_type_the_command() {
        let words = too_big_to_upload_words();
        let bound = 4.0 * 1024.0 * 1024.0 * 1024.0 / 1e9;
        assert!(
            words.contains(&format!("larger than {bound:.1} GB")),
            "{words}"
        );
        assert!(words.contains(WHERE_TO_RUN), "{words}");
    }

    #[test]
    fn a_full_disk_is_known_by_its_number_not_its_wording() {
        let full = if cfg!(windows) {
            "write: There is not enough space on the disk. (os error 112)"
        } else {
            "write: No space left on device (os error 28)"
        };
        assert_eq!(code_for_error_text(full), "disk_full");
        if !cfg!(windows) {
            let quota = if cfg!(target_vendor = "apple") {
                "write: Disc quota exceeded (os error 69)"
            } else {
                "write: Disk quota exceeded (os error 122)"
            };
            assert_eq!(
                code_for_error_text(quota),
                "disk_full",
                "a used-up quota too"
            );
        }
        assert_eq!(
            code_for_error_text("write: Permission denied (os error 13)"),
            "other"
        );
        // Each stop says what to do next only where importing again does it.
        assert!(
            stopped_words("disk_full").ends_with("free up space, then import the export again.")
        );
        assert_eq!(stopped_words("permission"), words("permission"));
        assert!(stopped_words("other").ends_with("import the export again to finish."));
    }
}
