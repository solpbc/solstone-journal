// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner-facing sentences for an Evernote export.
//!
//! The wording is the approved copy. Counts and names fill the braces.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_window_catalog::{ENGLISH, Value, packaged_locale, render, strings, text};

pub const ENCRYPTED_LINE: &str =
    "a part of this note is encrypted in Evernote. it wasn't brought in.";
pub const MISSING_ATTACHMENT: &str = "an attachment is missing from this export.";
pub const UNMATCHED_ATTACHMENT: &str = "an attachment didn't match this note, so it was left out.";
pub const OVERSIZED_ATTACHMENT: &str = "an attachment is over 25 MB, so it was left out.";

pub const NOT_AN_EXPORT: &str = "this file isn't an Evernote export.";
pub const EMPTY_EXPORT: &str = "this export has no notes.";
pub const TOO_BIG: &str =
    "this export is over 2 GB. in Evernote, export it again in smaller parts.";
pub const NOTEBOOK_HINT: &str = "notes that share this name stay together.";
pub const ALREADY_IN_JOURNAL: &str = "these notes are already in your journal.";

/// The sidecar file beside `imports/<id>/finish.txt` that carries verified
/// presentation counts for non-English owner views.
pub const FINISH_DISPLAY_FILE: &str = "finish.display.json";

/// Structured counts that compose an Evernote import's `finish.txt`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FinishCounts {
    pub new_count: u64,
    pub changed: u64,
    pub attachments: u64,
    pub future: u64,
    pub undated: u64,
    pub ended_early: bool,
    pub matched: u64,
}

impl FinishCounts {
    #[must_use]
    pub fn render(&self, locale: &str) -> String {
        let locale = packaged_locale(Some(locale));
        let mut lines = Vec::new();
        if self.new_count > 0 && self.changed > 0 {
            lines.push(brought_in_new_and_changed_in(
                locale,
                self.new_count,
                self.changed,
            ));
        } else if self.new_count > 0 {
            lines.push(brought_in_in(locale, self.new_count));
        } else if self.changed > 0 {
            lines.push(brought_in_changed_in(locale, self.changed));
        }
        if self.attachments > 0 {
            lines.push(attachments_came_in_for_notes_in(
                locale,
                self.attachments,
                self.new_count == 1 && self.changed == 0,
            ));
        }
        if self.future > 0 {
            lines.push(dated_after_today_in(locale, self.future));
        }
        if self.undated > 0 {
            lines.push(no_date_in(locale, self.undated));
        }
        if self.ended_early {
            lines.push(ended_early_in(locale, self.new_count));
        }
        if self.new_count == 0 && self.changed == 0 && self.matched > 0 {
            lines.push(text(
                locale,
                "journal.window.import.response.evernote.finish.already.in.journal",
            ));
        }
        lines.join("\n")
    }
}

/// Verified presentation metadata for `imports/<id>/finish.txt`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FinishDisplay {
    pub version: u32,
    pub source_sha256: String,
    pub counts: FinishCounts,
}

impl FinishDisplay {
    #[must_use]
    pub fn new(counts: FinishCounts, source_text: &str) -> Self {
        Self {
            version: 1,
            source_sha256: format!("{:x}", Sha256::digest(source_text.as_bytes())),
            counts,
        }
    }

    /// Project `finish_raw` into `locale` only when the receipt parses cleanly,
    /// matches `finish_raw`'s exact SHA-256 digest, and reproduces the English text.
    #[must_use]
    pub fn project_bytes(locale: &str, finish_raw: &str, receipt_bytes: &[u8]) -> Option<String> {
        let locale = packaged_locale(Some(locale));
        if locale == ENGLISH {
            return None;
        }
        let receipt: Self = serde_json::from_slice(receipt_bytes).ok()?;
        if receipt.version != 1 {
            return None;
        }
        let digest = format!("{:x}", Sha256::digest(finish_raw.as_bytes()));
        if receipt.source_sha256 != digest {
            return None;
        }
        if receipt.counts.render(ENGLISH) != finish_raw {
            return None;
        }
        Some(receipt.counts.render(locale))
    }
}

fn cardinal(locale: &str, id: &str, count: u64) -> String {
    let count_str = count.to_string();
    render(
        locale,
        id,
        &[
            ("count", Value::String(&count_str)),
            (
                "quantity",
                Value::Cardinal(i64::try_from(count).unwrap_or(i64::MAX)),
            ),
        ],
    )
}

fn preview_counts_in(
    locale: &str,
    notes: usize,
    first_year: i32,
    last_year: i32,
    attachments: usize,
) -> String {
    let locale = packaged_locale(Some(locale));
    let notes_str = cardinal(
        locale,
        "journal.window.import.response.evernote.count.note",
        notes as u64,
    );
    let attachments_str = cardinal(
        locale,
        "journal.window.import.response.evernote.count.attachment",
        attachments as u64,
    );
    let first_str = first_year.to_string();
    let last_str = last_year.to_string();
    strings(
        locale,
        "journal.window.import.response.evernote.preview.dated",
        [
            ("notes", &notes_str),
            ("first", &first_str),
            ("last", &last_str),
            ("attachments", &attachments_str),
        ],
    )
}

fn preview_counts_undated_in(locale: &str, notes: usize, attachments: usize) -> String {
    let locale = packaged_locale(Some(locale));
    let notes_str = cardinal(
        locale,
        "journal.window.import.response.evernote.count.note",
        notes as u64,
    );
    let attachments_str = cardinal(
        locale,
        "journal.window.import.response.evernote.count.attachment",
        attachments as u64,
    );
    strings(
        locale,
        "journal.window.import.response.evernote.preview.undated",
        [("notes", &notes_str), ("attachments", &attachments_str)],
    )
}

fn encrypted_count_in(locale: &str, count: usize) -> String {
    cardinal(
        packaged_locale(Some(locale)),
        "journal.window.import.response.evernote.preview.encrypted",
        count as u64,
    )
}

fn brought_in_in(locale: &str, count: u64) -> String {
    cardinal(
        packaged_locale(Some(locale)),
        "journal.window.import.response.evernote.finish.brought.in",
        count,
    )
}

fn brought_in_changed_in(locale: &str, count: u64) -> String {
    cardinal(
        packaged_locale(Some(locale)),
        "journal.window.import.response.evernote.finish.brought.in.changed",
        count,
    )
}

fn brought_in_new_and_changed_in(locale: &str, new: u64, changed: u64) -> String {
    let locale = packaged_locale(Some(locale));
    let new_part = brought_in_in(locale, new);
    let changed_part = brought_in_changed_in(locale, changed);
    strings(
        locale,
        "journal.window.import.response.evernote.finish.brought.in.both",
        [("new_part", &new_part), ("changed_part", &changed_part)],
    )
}

fn attachments_came_in_for_notes_in(locale: &str, count: u64, single_note: bool) -> String {
    let id = if single_note {
        "journal.window.import.response.evernote.finish.attachments.single.note"
    } else {
        "journal.window.import.response.evernote.finish.attachments"
    };
    cardinal(packaged_locale(Some(locale)), id, count)
}

fn dated_after_today_in(locale: &str, count: u64) -> String {
    cardinal(
        packaged_locale(Some(locale)),
        "journal.window.import.response.evernote.finish.dated.after.today",
        count,
    )
}

fn no_date_in(locale: &str, count: u64) -> String {
    cardinal(
        packaged_locale(Some(locale)),
        "journal.window.import.response.evernote.finish.no.date",
        count,
    )
}

fn ended_early_in(locale: &str, count: u64) -> String {
    cardinal(
        packaged_locale(Some(locale)),
        "journal.window.import.response.evernote.finish.ended.early",
        count,
    )
}

pub fn preview_counts(notes: usize, first_year: i32, last_year: i32, attachments: usize) -> String {
    preview_counts_in(ENGLISH, notes, first_year, last_year, attachments)
}

pub fn preview_counts_undated(notes: usize, attachments: usize) -> String {
    preview_counts_undated_in(ENGLISH, notes, attachments)
}

pub fn encrypted_count(count: usize) -> String {
    encrypted_count_in(ENGLISH, count)
}

pub fn brought_in(count: u64) -> String {
    brought_in_in(ENGLISH, count)
}

pub fn brought_in_new_and_changed(new: u64, changed: u64) -> String {
    brought_in_new_and_changed_in(ENGLISH, new, changed)
}

pub fn brought_in_changed(count: u64) -> String {
    brought_in_changed_in(ENGLISH, count)
}

pub fn attachments_came_in(count: u64) -> String {
    attachments_came_in_for_notes_in(ENGLISH, count, false)
}

pub fn attachments_came_in_for_notes(count: u64, single_note: bool) -> String {
    attachments_came_in_for_notes_in(ENGLISH, count, single_note)
}

pub fn dated_after_today(count: u64) -> String {
    dated_after_today_in(ENGLISH, count)
}

pub fn no_date(count: u64) -> String {
    no_date_in(ENGLISH, count)
}

pub fn ended_early(count: u64) -> String {
    ended_early_in(ENGLISH, count)
}

/// A refusal from the current or an older importer, identified by its own JSON copy.
pub(crate) fn preview_refusal_id(line: &str) -> Option<&'static str> {
    match line {
        NOT_AN_EXPORT => Some("journal.window.import.response.evernote.type"),
        EMPTY_EXPORT => Some("journal.window.import.response.evernote.empty"),
        line if line == ended_early(0) => {
            Some("journal.window.import.response.evernote.ended.empty")
        }
        _ => None,
    }
}

/// Project known preview lines from the importer JSON. Counts round-trip through
/// the source formatter (and also accept the legacy plural-only spelling from
/// older importer binaries), so an unknown line stays intact.
pub(crate) fn project_preview_line(locale: &str, line: &str) -> Option<String> {
    let locale = packaged_locale(Some(locale));
    if line == NOTEBOOK_HINT {
        return Some(text(
            locale,
            "journal.window.import.response.evernote.notebook",
        ));
    }
    let numeric: Vec<_> = line
        .split_whitespace()
        .map(|word| word.trim_end_matches('.'))
        .filter(|word| word.parse::<i64>().is_ok())
        .collect();
    match numeric.as_slice() {
        [notes, first, last, attachments] => {
            let n: usize = notes.parse().ok()?;
            let f: i32 = first.parse().ok()?;
            let l: i32 = last.parse().ok()?;
            let a: usize = attachments.parse().ok()?;
            let legacy = format!("{n} notes, from {f} to {l}. {a} attachments.");
            if line == preview_counts(n, f, l, a) || line == legacy {
                Some(preview_counts_in(locale, n, f, l, a))
            } else {
                None
            }
        }
        [notes, attachments] => {
            let n: usize = notes.parse().ok()?;
            let a: usize = attachments.parse().ok()?;
            let legacy = format!("{n} notes. {a} attachments.");
            if line == preview_counts_undated(n, a) || line == legacy {
                Some(preview_counts_undated_in(locale, n, a))
            } else {
                None
            }
        }
        [count] => {
            let c: usize = count.parse().ok()?;
            let legacy = format!("{c} notes have an encrypted part that will be left out.");
            if line == encrypted_count(c) || line == legacy {
                Some(encrypted_count_in(locale, c))
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evernote_singular_and_plural_copy_and_finish_display_round_trip() {
        assert_eq!(
            preview_counts(1, 2024, 2024, 1),
            "1 note, from 2024 to 2024. 1 attachment."
        );
        assert_eq!(
            preview_counts(4, 2024, 2025, 0),
            "4 notes, from 2024 to 2025. 0 attachments."
        );
        assert_eq!(preview_counts_undated(1, 1), "1 note. 1 attachment.");
        assert_eq!(
            encrypted_count(1),
            "1 note has an encrypted part that will be left out."
        );
        assert_eq!(brought_in(1), "brought in 1 note.");
        assert_eq!(
            brought_in_new_and_changed(1, 1),
            "brought in 1 note. brought in changes to 1 note that was already here."
        );
        assert_eq!(
            brought_in_changed(1),
            "brought in changes to 1 note that was already here."
        );
        assert_eq!(
            attachments_came_in_for_notes(1, true),
            "1 attachment came in with it."
        );
        assert_eq!(
            attachments_came_in_for_notes(2, true),
            "2 attachments came in with it."
        );
        assert_eq!(
            attachments_came_in_for_notes(1, false),
            "1 attachment came in with them."
        );
        assert_eq!(
            dated_after_today(1),
            "1 note is dated after today and was left out."
        );
        assert_eq!(no_date(1), "1 note has no date and was left out.");
        assert_eq!(ended_early(1), "this export ended early. 1 note came in.");

        // Legacy plural-only preview lines still project cleanly.
        assert_eq!(
            project_preview_line(ENGLISH, "1 notes, from 2024 to 2024. 1 attachments.").as_deref(),
            Some("1 note, from 2024 to 2024. 1 attachment.")
        );
        assert_eq!(
            project_preview_line(
                ENGLISH,
                "1 notes have an encrypted part that will be left out."
            )
            .as_deref(),
            Some("1 note has an encrypted part that will be left out.")
        );

        let counts = FinishCounts {
            new_count: 1,
            changed: 0,
            attachments: 1,
            future: 1,
            undated: 1,
            ended_early: true,
            matched: 0,
        };
        let english = counts.render(ENGLISH);
        assert!(english.contains("brought in 1 note."));
        assert!(english.contains("1 attachment came in with it."));
        assert!(english.contains("1 note is dated after today and was left out."));
        assert!(english.contains("1 note has no date and was left out."));
        assert!(english.contains("this export ended early. 1 note came in."));

        let receipt = FinishDisplay::new(counts, &english);
        let bytes = serde_json::to_vec(&receipt).unwrap();
        let locale = packaged_locale(Some("en-XA"));
        if locale == ENGLISH {
            assert!(FinishDisplay::project_bytes(locale, &english, &bytes).is_none());
        } else {
            let localized = FinishDisplay::project_bytes(locale, &english, &bytes).unwrap();
            assert_ne!(localized, english);
            // Tampering with finish.txt rejects projection cleanly.
            assert!(
                FinishDisplay::project_bytes(locale, &format!("{english} tampered"), &bytes)
                    .is_none()
            );
        }
    }
}
