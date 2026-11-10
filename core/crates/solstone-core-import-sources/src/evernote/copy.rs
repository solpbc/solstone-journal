// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner-facing sentences for an Evernote export.
//!
//! The wording is the approved copy. Counts and names fill the braces.

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

pub fn preview_counts(notes: usize, first_year: i32, last_year: i32, attachments: usize) -> String {
    format!("{notes} notes, from {first_year} to {last_year}. {attachments} attachments.")
}

pub fn preview_counts_undated(notes: usize, attachments: usize) -> String {
    format!("{notes} notes. {attachments} attachments.")
}

pub fn encrypted_count(count: usize) -> String {
    format!("{count} notes have an encrypted part that will be left out.")
}

pub fn brought_in(count: u64) -> String {
    format!("brought in {count} notes.")
}

pub fn brought_in_new_and_changed(new: u64, changed: u64) -> String {
    format!("brought in {new} notes. brought in changes to {changed} notes that were already here.")
}

pub fn brought_in_changed(count: u64) -> String {
    format!("brought in changes to {count} notes that were already here.")
}

pub fn attachments_came_in(count: u64) -> String {
    format!("{count} attachments came in with them.")
}

pub fn dated_after_today(count: u64) -> String {
    format!("{count} notes are dated after today and were left out.")
}

pub fn no_date(count: u64) -> String {
    format!("{count} notes have no date and were left out.")
}

pub fn ended_early(count: u64) -> String {
    format!("this export ended early. {count} notes came in.")
}
