// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Why an Apple Health import from the import page stopped or was refused, as
//! a code the run records and the page turns into words.
//!
//! The run writes `imports/<id>/body-stop.json` before the write that ends its
//! attempt. It only annotates: what an import reads as comes from its attempt.

use crate::apple_export::EXPORT_INCOMPLETE_TEXT;

/// The file a page-started body run leaves when it stops or is refused.
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
    if text.contains(crate::apple_export::EXPORT_INCOMPLETE) || text.contains(EXPORT_INCOMPLETE_TEXT)
    {
        return "export_incomplete";
    }
    if text.contains("export_size_limit") || text.contains("the most one import reads") {
        return "export_too_large";
    }
    let full: &[&str] = if cfg!(windows) {
        &["(os error 39)", "(os error 112)"]
    } else {
        &["(os error 28)"]
    };
    if full.iter().any(|marker| text.contains(marker)) {
        return "disk_full";
    }
    "other"
}

/// What the owner reads for a code: what happened, without the system's own words.
#[must_use]
pub fn words(code: &str) -> &'static str {
    match code {
        "disk_full" => "the disk filled up before it finished.",
        "permission" => "the journal wasn't allowed to open the export.",
        "windows_not_saving" => "64 hours in a row couldn't be saved, so it stopped.",
        "busy" => "another Apple Health import was running, so this one didn't start.",
        "export_incomplete" => EXPORT_INCOMPLETE_TEXT,
        "export_too_large" => "this export is larger than 64 GiB, the most one import reads.",
        "export_unreadable" => {
            "this export couldn't be read. export it from Apple Health again and import the new copy."
        }
        "not_this_import" => "this import doesn't hold that export, so nothing was brought in.",
        "facet" => "the facet chosen for it is muted or gone, so nothing was brought in.",
        "go_ahead" => "it didn't have your go-ahead, so nothing was brought in.",
        "whole_export" => "an Apple Health import always reads the whole export, so nothing was brought in.",
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
    fn a_full_disk_is_known_by_its_number_not_its_wording() {
        let full = if cfg!(windows) {
            "write: There is not enough space on the disk. (os error 112)"
        } else {
            "write: No space left on device (os error 28)"
        };
        assert_eq!(code_for_error_text(full), "disk_full");
        assert_eq!(
            code_for_error_text("write: Permission denied (os error 13)"),
            "other"
        );
    }
}
