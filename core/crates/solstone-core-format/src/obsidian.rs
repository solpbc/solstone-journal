// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Note changes a linked device reports from an Obsidian vault.
//!
//! A line says a note changed, moved or went away, and when. Ingest turns each
//! file into one `note_transcript.md` for its window, holding only what changed.
//! This module is the one reading of the line contract ([`validate_obsidian_file`])
//! and the one writing of the memory ([`render_edit`], [`render_transcript`]).

use std::fmt;

use chrono::{DateTime, FixedOffset, NaiveDateTime, Timelike};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::calendar::segment_window;

/// The ingest source an Obsidian stream is bound to.
pub const OBSIDIAN_SOURCE: &str = "obsidian";

/// The name every note file starts with.
pub const OBSIDIAN_FILE_PREFIX: &str = "obsidian_";

/// The memory a window of note changes becomes.
pub const NOTE_TRANSCRIPT_FILE: &str = "note_transcript.md";

/// Most lines in one file.
pub const MAX_FILE_LINES: usize = 5_000;

/// Most bytes in one file.
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

/// Most bytes of text in one note.
pub const MAX_NOTE_BYTES: usize = 4 * 1024 * 1024;

const MAX_VAULT_BYTES: usize = 128;

const REQUIRED_KEYS: [&str; 9] = [
    "v",
    "change",
    "vault",
    "path",
    "previous_path",
    "changed_at",
    "observed_at",
    "text",
    "content_hash",
];

/// Whether a stream's name is that of an Obsidian stream, for the few labels
/// decided from the name alone. Behaviour is decided by the bound source.
pub fn is_obsidian_stream_name(stream: &str) -> bool {
    stream.ends_with("_obsidian")
}

/// Whether `name` is an Obsidian note file name: `obsidian_<64 lowercase hex>.jsonl`.
pub fn is_obsidian_file_name(name: &str) -> bool {
    name.strip_prefix(OBSIDIAN_FILE_PREFIX)
        .and_then(|rest| rest.strip_suffix(".jsonl"))
        .is_some_and(|id| {
            id.len() == 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// Whether `name` claims to be a note file, well formed or not.
pub fn names_obsidian_file(name: &str) -> bool {
    name.starts_with(OBSIDIAN_FILE_PREFIX) && name.ends_with(".jsonl")
}

/// What happened to a note.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    Changed,
    Renamed,
    Removed,
}

/// One checked line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteChange {
    pub change: Change,
    pub vault: String,
    pub path: String,
    pub previous_path: Option<String>,
    pub changed_at: DateTime<FixedOffset>,
    pub observed_at: DateTime<FixedOffset>,
    pub text: Option<String>,
    pub content_hash: Option<String>,
}

impl NoteChange {
    pub fn changed_at_ms(&self) -> i64 {
        self.changed_at.timestamp_millis()
    }
}

/// Why a note file was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObsidianFileError {
    /// The 1-based line, or `None` for the file as a whole.
    pub line: Option<usize>,
    pub reason: String,
}

impl fmt::Display for ObsidianFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "line {line}: {}", self.reason),
            None => formatter.write_str(&self.reason),
        }
    }
}

impl std::error::Error for ObsidianFileError {}

fn whole_file(reason: impl Into<String>) -> ObsidianFileError {
    ObsidianFileError {
        line: None,
        reason: reason.into(),
    }
}

/// Check a whole note file against the contract. A file that breaks any rule
/// is refused whole.
///
/// `day` and `segment` are the envelope's: every line's `changed_at`, read as
/// wall-clock time in its own offset, must fall inside that window.
pub fn validate_obsidian_file(
    name: &str,
    bytes: &[u8],
    day: &str,
    segment: &str,
) -> Result<Vec<NoteChange>, ObsidianFileError> {
    if !is_obsidian_file_name(name) {
        return Err(whole_file(
            "file name must be obsidian_ followed by 64 lowercase hex digits and .jsonl",
        ));
    }
    if format!("{:x}", Sha256::digest(bytes)) != name[OBSIDIAN_FILE_PREFIX.len()..name.len() - 6] {
        return Err(whole_file("file name must be the SHA-256 of the file"));
    }
    let window = segment_window(day, segment).ok_or_else(|| {
        whole_file("segment must name one 300-second window on the journal's grid")
    })?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(whole_file("file holds more than 8 MiB"));
    }
    if bytes.last() != Some(&b'\n') {
        return Err(whole_file("file must end with a newline"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| whole_file("file is not UTF-8"))?;
    let mut changes = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if index == MAX_FILE_LINES {
            return Err(whole_file("file holds more than 5,000 lines"));
        }
        let change = parse_line(line, window).map_err(|reason| ObsidianFileError {
            line: Some(index + 1),
            reason,
        })?;
        changes.push(change);
    }
    if changes.is_empty() {
        return Err(whole_file("file holds no lines"));
    }
    Ok(changes)
}

fn parse_line(
    line: &str,
    (window_start, window_end): (NaiveDateTime, NaiveDateTime),
) -> Result<NoteChange, String> {
    let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
        return Err("line is not a JSON object".to_owned());
    };
    for key in REQUIRED_KEYS {
        if !record.contains_key(key) {
            return Err(format!("{key} is missing"));
        }
    }
    if record.get("v").and_then(Value::as_i64) != Some(1) {
        return Err("v must be 1".to_owned());
    }
    let change = match record.get("change").and_then(Value::as_str) {
        Some("changed") => Change::Changed,
        Some("renamed") => Change::Renamed,
        Some("removed") => Change::Removed,
        _ => return Err("change must be changed, renamed or removed".to_owned()),
    };
    let vault = string(&record, "vault")?;
    if vault.is_empty() || vault.len() > MAX_VAULT_BYTES || vault.chars().any(char::is_control) {
        return Err("vault must be 1 to 128 bytes with no control characters".to_owned());
    }
    let path = string(&record, "path")?;
    check_path(&path).map_err(|reason| format!("path {reason}"))?;
    let previous_path = optional_string(&record, "previous_path")?;
    let changed_at = time(&record, "changed_at")?;
    let observed_at = time(&record, "observed_at")?;
    let local = changed_at.naive_local();
    if local < window_start || local >= window_end {
        return Err("changed_at falls outside the segment window".to_owned());
    }
    let text = optional_string(&record, "text")?;
    let content_hash = optional_string(&record, "content_hash")?;
    match change {
        Change::Removed => {
            if previous_path.is_some() || text.is_some() || content_hash.is_some() {
                return Err("a removed line carries no previous_path, text or hash".to_owned());
            }
        }
        Change::Changed | Change::Renamed => {
            match (&text, &content_hash) {
                (Some(text), Some(hash)) => {
                    if text.len() > MAX_NOTE_BYTES {
                        return Err("text is over 4 MiB".to_owned());
                    }
                    if &format!("{:x}", Sha256::digest(text.as_bytes())) != hash {
                        return Err("content_hash is not the SHA-256 of text".to_owned());
                    }
                }
                _ => return Err("text and content_hash are required".to_owned()),
            }
            match (change, &previous_path) {
                (Change::Renamed, Some(previous)) => {
                    check_path(previous).map_err(|reason| format!("previous_path {reason}"))?;
                }
                (Change::Renamed, None) => {
                    return Err("a renamed line needs previous_path".to_owned());
                }
                (_, Some(_)) => return Err("only a renamed line has previous_path".to_owned()),
                _ => {}
            }
        }
    }
    Ok(NoteChange {
        change,
        vault,
        path,
        previous_path,
        changed_at,
        observed_at,
        text,
        content_hash,
    })
}

fn string(record: &Map<String, Value>, key: &str) -> Result<String, String> {
    record
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{key} must be a string"))
}

fn optional_string(record: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match record.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{key} must be a string or null")),
    }
}

fn time(record: &Map<String, Value>, key: &str) -> Result<DateTime<FixedOffset>, String> {
    let text = string(record, key)?;
    let parsed = DateTime::parse_from_rfc3339(&text)
        .map_err(|_| format!("{key} must be RFC 3339 with an offset"))?;
    if parsed.nanosecond() != 0 {
        return Err(format!("{key} must be whole seconds"));
    }
    Ok(parsed)
}

fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return Err("must be a relative path with / separators".to_owned());
    }
    if path.chars().any(char::is_control) {
        return Err("must have no control characters".to_owned());
    }
    if !path.to_lowercase().ends_with(".md") {
        return Err("must end .md".to_owned());
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment.starts_with('.') {
            return Err("must have no empty segment or segment starting with a dot".to_owned());
        }
        let lower = segment.to_lowercase();
        if lower == "templates" || lower == "_templates" {
            return Err("must not be inside a templates folder".to_owned());
        }
    }
    Ok(())
}

/// One note's entry in a window's memory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteEntry {
    pub path: String,
    /// What happened, when it needs saying. A note seen for the first time
    /// simply reads as the note.
    pub status: Option<String>,
    pub body: String,
}

/// The memory for a window: each entry as a titled note, in the order given.
pub fn render_transcript(entries: &[NoteEntry]) -> String {
    let mut out = String::new();
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n");
        }
        out.push_str("## ");
        out.push_str(note_title(&entry.path));
        out.push_str("\nSource: ");
        out.push_str(&entry.path);
        if let Some(status) = &entry.status {
            out.push('\n');
            out.push_str(status);
        }
        let body = entry.body.trim_end();
        if !body.is_empty() {
            out.push_str("\n\n");
            out.push_str(body);
        }
    }
    out.push('\n');
    out
}

/// A note's title: its file name without the extension.
pub fn note_title(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    }
}

/// What an edit wrote: the text inserted, and a count of the words taken out.
/// Nothing written before the edit appears. `None` means no change of substance.
pub fn render_edit(old: &str, new: &str) -> Option<String> {
    let old = old.replace("\r\n", "\n");
    let new = new.replace("\r\n", "\n");
    if old == new {
        return None;
    }
    let old_lines = old.split('\n').collect::<Vec<_>>();
    let new_lines = new.split('\n').collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut removed_words = 0usize;
    for hunk in hunks(&old_lines, &new_lines) {
        let taken = &old_lines[hunk.old.clone()];
        let given = &new_lines[hunk.new.clone()];
        if taken.iter().all(|line| line.trim().is_empty()) {
            let block = given.join("\n");
            if !block.trim().is_empty() {
                blocks.push(block.trim_matches('\n').to_owned());
            }
            continue;
        }
        if given.iter().all(|line| line.trim().is_empty()) {
            removed_words += count_words(taken);
            continue;
        }
        let (inserted, removed) = word_changes(&taken.join("\n"), &given.join("\n"));
        removed_words += removed;
        if !inserted.is_empty() {
            blocks.push(inserted.join(" … "));
        }
    }
    let mut out = blocks.join("\n\n");
    if removed_words > 0 {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&match removed_words {
            1 => "removed 1 word.".to_owned(),
            count => format!("removed {count} words."),
        });
    }
    (!out.is_empty()).then_some(out)
}

fn count_words(lines: &[&str]) -> usize {
    lines
        .iter()
        .map(|line| line.split_whitespace().count())
        .sum()
}

#[derive(Clone, Debug)]
struct Hunk {
    old: std::ops::Range<usize>,
    new: std::ops::Range<usize>,
}

/// The most cells a diff table may hold; past it a hunk reads as replaced.
const MAX_DIFF_CELLS: usize = 4_000_000;

/// The changed regions between two token lists, by longest common subsequence.
fn hunks<T: PartialEq>(old: &[T], new: &[T]) -> Vec<Hunk> {
    let mut start = 0;
    while start < old.len() && start < new.len() && old[start] == new[start] {
        start += 1;
    }
    let (mut old_end, mut new_end) = (old.len(), new.len());
    while old_end > start && new_end > start && old[old_end - 1] == new[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }
    if start == old_end && start == new_end {
        return Vec::new();
    }
    let (old_middle, new_middle) = (&old[start..old_end], &new[start..new_end]);
    let (rows, columns) = (old_middle.len(), new_middle.len());
    if rows == 0 || columns == 0 || rows.saturating_mul(columns) > MAX_DIFF_CELLS {
        return vec![Hunk {
            old: start..old_end,
            new: start..new_end,
        }];
    }
    let mut table = vec![0u32; (rows + 1) * (columns + 1)];
    let at = |row: usize, column: usize| row * (columns + 1) + column;
    for row in (0..rows).rev() {
        for column in (0..columns).rev() {
            table[at(row, column)] = if old_middle[row] == new_middle[column] {
                table[at(row + 1, column + 1)] + 1
            } else {
                table[at(row + 1, column)].max(table[at(row, column + 1)])
            };
        }
    }
    let mut result = Vec::new();
    let (mut row, mut column) = (0, 0);
    let mut open: Option<Hunk> = None;
    while row < rows || column < columns {
        if row < rows && column < columns && old_middle[row] == new_middle[column] {
            if let Some(hunk) = open.take() {
                result.push(hunk);
            }
            row += 1;
            column += 1;
            continue;
        }
        let hunk = open.get_or_insert(Hunk {
            old: start + row..start + row,
            new: start + column..start + column,
        });
        if column < columns
            && (row == rows || table[at(row, column + 1)] >= table[at(row + 1, column)])
        {
            column += 1;
            hunk.new.end = start + column;
        } else {
            row += 1;
            hunk.old.end = start + row;
        }
    }
    if let Some(hunk) = open.take() {
        result.push(hunk);
    }
    result
}

/// Words and the spaces between them, so a diff keeps the spacing of what it
/// keeps.
fn tokens(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut in_space = None;
    for (index, character) in text.char_indices() {
        let space = character.is_whitespace();
        if in_space.is_some_and(|previous| previous != space) {
            result.push(&text[start..index]);
            start = index;
        }
        in_space = Some(space);
    }
    if start < text.len() {
        result.push(&text[start..]);
    }
    result
}

/// The text inserted between `old` and `new`, and how many words left.
///
/// A sentence mostly written anew is kept whole, so a rewrite reads as the
/// sentence it is rather than as scattered words around the few it kept.
/// Otherwise only the inserted runs are kept, in order.
fn word_changes(old: &str, new: &str) -> (Vec<String>, usize) {
    let old_tokens = tokens(old);
    let new_tokens = tokens(new);
    let mut inserted = vec![false; new_tokens.len()];
    let mut removed = 0;
    for hunk in hunks(&old_tokens, &new_tokens) {
        removed += old_tokens[hunk.old.clone()]
            .iter()
            .filter(|token| !token.trim().is_empty())
            .count();
        inserted[hunk.new.clone()].fill(true);
    }
    let is_word = |index: usize| !new_tokens[index].trim().is_empty();
    let mut runs: Vec<String> = Vec::new();
    let mut last_end = None;
    let mut start = 0;
    while start < new_tokens.len() {
        let mut end = start;
        while end < new_tokens.len() && !ends_sentence(new_tokens[end]) {
            end += 1;
        }
        let end = (end + 1).min(new_tokens.len());
        let words = (start..end).filter(|&index| is_word(index)).count();
        let written = (start..end)
            .filter(|&index| is_word(index) && inserted[index])
            .count();
        if written > 0 && written * 2 > words {
            push_run(&mut runs, &mut last_end, start, end, &new_tokens);
        } else if written > 0 {
            let mut index = start;
            while index < end {
                if !inserted[index] {
                    index += 1;
                    continue;
                }
                let run_start = index;
                while index < end && inserted[index] {
                    index += 1;
                }
                if (run_start..index).any(is_word) {
                    push_run(&mut runs, &mut last_end, run_start, index, &new_tokens);
                }
            }
        }
        start = end;
    }
    (runs, removed)
}

/// Whether a sentence stops with this token: its closing punctuation, or a
/// line break.
fn ends_sentence(token: &str) -> bool {
    token.contains('\n') || token.ends_with(['.', '!', '?'])
}

/// Add the tokens in `start..end`, joining a run that follows straight on from
/// the last so only a real gap reads as one.
fn push_run(
    runs: &mut Vec<String>,
    last_end: &mut Option<usize>,
    start: usize,
    end: usize,
    tokens: &[&str],
) {
    let text = tokens[start..end].concat();
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let follows = (*last_end).is_some_and(|last| {
        tokens[last..start]
            .iter()
            .all(|token| token.trim().is_empty())
    });
    match runs.last_mut() {
        Some(previous) if follows => {
            previous.push(' ');
            previous.push_str(text);
        }
        _ => runs.push(text.to_owned()),
    }
    *last_end = Some(end);
}

/// A rename known to the journal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnownRename {
    pub from: String,
    pub to: String,
}

/// Whether the only difference between two texts is link targets that follow
/// a known rename, which is Obsidian keeping links working rather than the
/// owner writing.
pub fn only_link_rewrites(old: &str, new: &str, renames: &[KnownRename]) -> bool {
    if renames.is_empty() {
        return false;
    }
    let (old_masked, old_targets) = mask_links(old);
    let (new_masked, new_targets) = mask_links(new);
    if old_masked != new_masked || old_targets.len() != new_targets.len() {
        return false;
    }
    let mut changed = false;
    for (before, after) in old_targets.iter().zip(&new_targets) {
        if before == after {
            continue;
        }
        changed = true;
        if !renames
            .iter()
            .any(|rename| names_note(before, &rename.from) && names_note(after, &rename.to))
        {
            return false;
        }
    }
    changed
}

/// Whether a link target points at the note at `path`: its whole path or a
/// trailing part of it, with or without the extension.
fn names_note(target: &str, path: &str) -> bool {
    let normalize = |value: &str| {
        let value = value.replace("%20", " ").replace('\\', "/").to_lowercase();
        let value = value.trim_start_matches("./").trim_start_matches('/');
        value.strip_suffix(".md").unwrap_or(value).to_owned()
    };
    let target = normalize(target);
    let path = normalize(path);
    !target.is_empty() && (target == path || path.ends_with(&format!("/{target}")))
}

/// The text with every link target taken out, and the targets in order.
fn mask_links(text: &str) -> (String, Vec<String>) {
    let mut masked = String::with_capacity(text.len());
    let mut targets = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("[[")
            && let Some(close) = after.find("]]")
        {
            let inner = &after[..close];
            let end = inner.find(['|', '#']).unwrap_or(inner.len());
            targets.push(inner[..end].to_owned());
            masked.push_str("[[\u{0}");
            masked.push_str(&inner[end..]);
            masked.push_str("]]");
            rest = &after[close + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("](")
            && let Some(close) = after.find(')')
        {
            let inner = &after[..close];
            let end = inner.find('#').unwrap_or(inner.len());
            if inner[..end].to_lowercase().ends_with(".md") {
                targets.push(inner[..end].to_owned());
                masked.push_str("](\u{0}");
                masked.push_str(&inner[end..]);
                masked.push(')');
                rest = &after[close + 1..];
                continue;
            }
        }
        let next = rest.chars().next().map_or(1, char::len_utf8);
        masked.push_str(&rest[..next]);
        rest = &rest[next..];
    }
    (masked, targets)
}

#[cfg(test)]
#[path = "obsidian_tests.rs"]
mod tests;
