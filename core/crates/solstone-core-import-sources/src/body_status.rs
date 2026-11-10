// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Why an Apple Health import stopped or was refused, as a code the run records
//! and the page turns into words.
//!
//! An Apple run writes `imports/<id>/body-stop.json` before the write that ends
//! its attempt, naming the attempt's generation. It only annotates: what an
//! import reads as comes from its attempt, and a code from another generation is
//! ignored.

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

/// What the card says of an export too large for the page to upload.
#[must_use]
pub fn too_big_to_upload_words() -> String {
    format!(
        "this export is larger than 4.3 GB, the most this page uploads. bring it in from the command line instead: {WHERE_TO_RUN}: solstone journal importer <your export> --confirm-body-save"
    )
}

/// The size bound in the units the card shows sizes in. `too_large_words` keeps
/// it in step with the bound itself.
const TOO_LARGE: &str = "this export unpacks to more than 68.7 GB, the most one import reads.";

/// What a too-large export is told, at the terminal as on the page.
#[must_use]
pub fn too_large_words() -> String {
    TOO_LARGE.trim_end_matches('.').to_owned()
}

/// The incomplete-export words as a sentence beside the others.
const EXPORT_INCOMPLETE_SENTENCE: &str = "this export looks incomplete, so none of it was saved. export it from Apple Health again and import the new copy.";

/// What a stopped import's row says: what happened, then what to do. Importing
/// again finishes most stops; a full disk needs room first, and a refused
/// permission isn't fixed by importing again, so it says only what happened.
#[must_use]
pub fn stopped_words(code: &str) -> String {
    match code {
        "disk_full" => format!(
            "{} free up space, then import the export again.",
            words(code)
        ),
        "permission" => words(code).to_owned(),
        _ => format!("{} import the export again to finish.", words(code)),
    }
}

/// What the owner reads for a code: what happened, without the system's own words.
#[must_use]
pub fn words(code: &str) -> &'static str {
    match code {
        "disk_full" => "the disk filled up before it finished.",
        "permission" => "the journal wasn't allowed to open the export.",
        "windows_not_saving" => {
            "too many parts of your journal in a row couldn't be saved, so it stopped."
        }
        "busy" => "another Apple Health import was running, so this one didn't start.",
        "export_incomplete" => EXPORT_INCOMPLETE_SENTENCE,
        "export_too_large" => TOO_LARGE,
        "export_unreadable" => {
            "this export couldn't be read. export it from Apple Health again and import the new copy."
        }
        "not_this_import" => "this import doesn't hold that export, so nothing was brought in.",
        "facet" => "the facet chosen for it is muted or gone, so nothing was brought in.",
        "go_ahead" => "it didn't have your go-ahead, so nothing was brought in.",
        "whole_export" => {
            "an Apple Health import always reads the whole export, so nothing was brought in."
        }
        "preview_again" => {
            "the facet, the time zone or the export changed after its preview, so nothing was brought in. import the export again from the Apple Health card."
        }
        "not_started" => "it didn't start, so nothing was brought in.",
        "no_room" => {
            "there wasn't room on this disk to start it, so nothing was brought in. free up space, then import the export again."
        }
        _ => "it stopped before it finished.",
    }
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
        assert!(TOO_LARGE.contains(&bound), "{TOO_LARGE} against {bound}");
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
