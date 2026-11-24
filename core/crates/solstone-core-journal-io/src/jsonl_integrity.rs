// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! JSONL files read line by line from raw bytes: what a damaged file still
//! holds, setting its damaged lines aside, and how often the journal has
//! quietly healed its own logs.
//!
//! A line is damaged when it is not one valid JSON value: cut short at the end
//! of the file (a stop or power loss mid-append), invalid UTF-8, or malformed
//! JSON between good lines. One damaged line never hides the others.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::IgnoredAny;

use crate::durability::{JOURNAL_ARTIFACTS, ParserKind, SET_ASIDE_MARKER};
use crate::{AtomicWriteOptions, LockOptions};

/// How one JSONL line is damaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonlDamageKind {
    /// The file's last line, with no newline after it, does not parse: a write
    /// that stopped part way.
    TruncatedTail,
    /// A line between others that is not valid UTF-8.
    InvalidUtf8,
    /// A line between others that is valid text but not one JSON value.
    MalformedJson,
}

impl JsonlDamageKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TruncatedTail => "truncated_tail",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::MalformedJson => "malformed_json",
        }
    }
}

/// One damaged line, numbered from 1 as an editor shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DamagedJsonlLine {
    pub line: usize,
    pub kind: JsonlDamageKind,
}

/// What a JSONL file holds, read without changing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JsonlInspection {
    /// Journal-relative, `/`-separated. Never a host path.
    pub path: String,
    /// Lines that parse as one JSON value. Blank lines are not counted.
    pub valid_lines: usize,
    pub damaged: Vec<DamagedJsonlLine>,
}

impl JsonlInspection {
    pub fn is_damaged(&self) -> bool {
        !self.damaged.is_empty()
    }

    /// Whether the last line was cut short at the end of the file.
    pub fn truncated_tail(&self) -> bool {
        self.damaged
            .iter()
            .any(|line| line.kind == JsonlDamageKind::TruncatedTail)
    }

    /// Damaged lines other than a cut-short last line.
    pub fn interior_damaged(&self) -> usize {
        self.damaged
            .iter()
            .filter(|line| line.kind != JsonlDamageKind::TruncatedTail)
            .count()
    }
}

/// One line of a JSONL byte buffer, without its newline.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JsonlLine<'a> {
    pub(crate) number: usize,
    pub(crate) bytes: &'a [u8],
    pub(crate) terminated: bool,
}

impl<'a> JsonlLine<'a> {
    /// The line without surrounding ASCII whitespace (a `\r` included).
    pub(crate) fn content(&self) -> &'a [u8] {
        self.bytes.trim_ascii()
    }

    pub(crate) fn is_blank(&self) -> bool {
        self.content().is_empty()
    }
}

/// Split raw bytes into lines on `\n`, never decoding them as a whole.
pub(crate) fn jsonl_lines(bytes: &[u8]) -> impl Iterator<Item = JsonlLine<'_>> {
    let ends_with_newline = bytes.last() == Some(&b'\n');
    let body = if ends_with_newline {
        &bytes[..bytes.len() - 1]
    } else {
        bytes
    };
    let mut lines = if bytes.is_empty() {
        None
    } else {
        Some(body.split(|byte| *byte == b'\n'))
    }
    .into_iter()
    .flatten()
    .enumerate()
    .peekable();
    std::iter::from_fn(move || {
        let (index, line) = lines.next()?;
        let terminated = lines.peek().is_some() || ends_with_newline;
        Some(JsonlLine {
            number: index + 1,
            bytes: line,
            terminated,
        })
    })
}

/// Why a non-blank line can't be read as one JSON value, if it can't.
pub(crate) fn classify_line(line: &JsonlLine<'_>) -> Option<JsonlDamageKind> {
    if line.is_blank() {
        return None;
    }
    let damage = match std::str::from_utf8(line.content()) {
        Err(_) => JsonlDamageKind::InvalidUtf8,
        Ok(text) => match serde_json::from_str::<IgnoredAny>(text) {
            Ok(_) => return None,
            Err(_) => JsonlDamageKind::MalformedJson,
        },
    };
    Some(if line.terminated {
        damage
    } else {
        JsonlDamageKind::TruncatedTail
    })
}

/// Count valid lines and name damaged ones in a JSONL byte buffer.
pub fn inspect_jsonl_bytes(bytes: &[u8]) -> (usize, Vec<DamagedJsonlLine>) {
    let mut valid = 0;
    let mut damaged = Vec::new();
    for line in jsonl_lines(bytes) {
        if line.is_blank() {
            continue;
        }
        match classify_line(&line) {
            None => valid += 1,
            Some(kind) => damaged.push(DamagedJsonlLine {
                line: line.number,
                kind,
            }),
        }
    }
    (valid, damaged)
}

/// The `/`-separated path of `path` inside `journal_root`, if it is inside it.
pub fn journal_relative_path(journal_root: &Path, path: &Path) -> Option<String> {
    let inside = path.strip_prefix(journal_root).ok()?;
    let mut parts = Vec::new();
    for component in inside.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn outside_journal(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "{} is not inside the journal",
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
    )
}

/// Inspect one JSONL file inside the journal without changing anything.
pub fn inspect_jsonl(journal_root: &Path, path: &Path) -> io::Result<JsonlInspection> {
    let relative =
        journal_relative_path(journal_root, path).ok_or_else(|| outside_journal(path))?;
    let bytes = fs::read(path)?;
    let (valid_lines, damaged) = inspect_jsonl_bytes(&bytes);
    Ok(JsonlInspection {
        path: relative,
        valid_lines,
        damaged,
    })
}

/// What setting a file's damaged lines aside did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonlQuarantine {
    /// Non-blank lines still in the file, each byte-for-byte as it was.
    pub kept_lines: usize,
    pub set_aside: Vec<DamagedJsonlLine>,
    /// The dated side file holding the damaged lines; `None` when nothing was damaged.
    pub aside_path: Option<PathBuf>,
}

/// The stamp a set-aside file carries: whole seconds since the Unix epoch.
pub(crate) fn set_aside_stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Move a JSONL file's damaged lines into one dated side file beside it.
///
/// Runs under the file's canonical lock (reused if this thread holds it). The
/// side file, `<stem>.wedged-<stamp>-lines<ext>`, is written and synced before
/// the file is rewritten without the damaged lines, so a stop between the two
/// leaves the damaged lines in both places and loses none. Every other line,
/// blank ones included, is kept byte for byte. For a damaged owner transcript
/// this is the owner-confirmed set-aside; passive reads never call it.
pub fn quarantine_damaged_jsonl_lines(path: &Path) -> io::Result<JsonlQuarantine> {
    let _lock = crate::hold_or_reuse_lock(
        path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(io::Error::other)?;
    let bytes = fs::read(path)?;
    let mut kept = Vec::with_capacity(bytes.len());
    let mut aside = Vec::new();
    let mut set_aside = Vec::new();
    let mut kept_lines = 0;
    for line in jsonl_lines(&bytes) {
        match classify_line(&line) {
            Some(kind) => {
                aside.extend_from_slice(line.bytes);
                aside.push(b'\n');
                set_aside.push(DamagedJsonlLine {
                    line: line.number,
                    kind,
                });
            }
            None => {
                kept_lines += usize::from(!line.is_blank());
                kept.extend_from_slice(line.bytes);
                kept.push(b'\n');
            }
        }
    }
    if set_aside.is_empty() {
        return Ok(JsonlQuarantine {
            kept_lines,
            set_aside,
            aside_path: None,
        });
    }
    let aside_path = write_new_aside(path, "-lines", &aside)?;
    crate::atomic_replace(
        path,
        &kept,
        AtomicWriteOptions {
            mode: existing_mode(path),
        },
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(JsonlQuarantine {
        kept_lines,
        set_aside,
        aside_path: Some(aside_path),
    })
}

#[cfg(unix)]
fn existing_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn existing_mode(_path: &Path) -> Option<u32> {
    None
}

/// `<stem>.wedged-<stamp><suffix><ext>` beside `path`, never replacing a file.
pub(crate) fn aside_name(path: &Path, stamp: u64, suffix: &str, sequence: usize) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("artifact");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_else(|| ".jsonl".to_owned());
    let sequence = if sequence == 0 {
        String::new()
    } else {
        format!("-{sequence}")
    };
    path.with_file_name(format!(
        "{stem}{SET_ASIDE_MARKER}{stamp}{suffix}{sequence}{extension}"
    ))
}

/// Create and sync a new side file beside `path`, choosing an unused name.
pub(crate) fn write_new_aside(path: &Path, suffix: &str, contents: &[u8]) -> io::Result<PathBuf> {
    let stamp = set_aside_stamp();
    for sequence in 0.. {
        let candidate = aside_name(path, stamp, suffix, sequence);
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        file.write_all(contents)?;
        file.sync_all()?;
        return Ok(candidate);
    }
    unreachable!("an unbounded sequence always finds an unused name")
}

/// How long a quiet heal counts toward a recurring one.
pub const OPERATIONAL_HEAL_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How many quiet heals within [`OPERATIONAL_HEAL_WINDOW`] make a recurring one.
pub const OPERATIONAL_HEAL_RECURRENCE: usize = 2;

/// One quiet heal of an operational JSONL log: a durable read that set one or
/// more damaged lines aside at one moment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct OperationalHeal {
    /// When the heal ran, in whole seconds since the Unix epoch.
    pub healed_at: u64,
    /// The healed log, journal-relative.
    pub log: String,
    /// The side files it wrote, journal-relative.
    pub set_aside: Vec<String>,
}

/// Quiet heals in the recurrence window, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationalHealRecurrence {
    pub heals: Vec<OperationalHeal>,
    /// At least [`OPERATIONAL_HEAL_RECURRENCE`] heals in the window: a sign of
    /// a failing disk, a read-only mount or a sync folder rather than a one-off stop.
    pub recurring: bool,
}

/// Quiet heals within [`OPERATIONAL_HEAL_WINDOW`] before `now`, and whether they recur.
pub fn operational_heal_recurrence(
    journal_root: &Path,
    now: SystemTime,
) -> io::Result<OperationalHealRecurrence> {
    let since = now
        .checked_sub(OPERATIONAL_HEAL_WINDOW)
        .unwrap_or(UNIX_EPOCH);
    let mut heals = operational_heals_since(journal_root, since)?;
    heals.reverse();
    let recurring = heals.len() >= OPERATIONAL_HEAL_RECURRENCE;
    Ok(OperationalHealRecurrence { heals, recurring })
}

/// Every quiet heal of a registered operational JSONL log at or after `since`,
/// oldest first, found from the side files the heals wrote.
///
/// Reads only directory listings; a directory whose modification time is
/// before `since` gained no side file since then and is not listed. A
/// directory that can't be read is an error: an unknown count is not zero.
pub fn operational_heals_since(
    journal_root: &Path,
    since: SystemTime,
) -> io::Result<Vec<OperationalHeal>> {
    let since_secs = since
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let mut directories = BTreeMap::<PathBuf, Vec<&'static str>>::new();
    for artifact in JOURNAL_ARTIFACTS
        .iter()
        .filter(|artifact| artifact.parser == ParserKind::RecordFramedJsonl)
    {
        let Some((parent, leaf)) = artifact.path.rsplit_once('/') else {
            continue;
        };
        if parent.contains("**") {
            continue;
        }
        for directory in expand_directories(journal_root, parent)? {
            directories.entry(directory).or_default().push(leaf);
        }
    }
    let mut grouped = BTreeMap::<(u64, String), BTreeSet<String>>::new();
    for (directory, leaves) in directories {
        let modified = match fs::metadata(&directory) {
            Ok(metadata) => metadata.modified()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if modified < since {
            continue;
        }
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some((log_name, stamp)) = parse_line_aside(&name, &leaves) else {
                continue;
            };
            if stamp < since_secs {
                continue;
            }
            let (Some(log), Some(aside)) = (
                journal_relative_path(journal_root, &directory.join(&log_name)),
                journal_relative_path(journal_root, &entry.path()),
            ) else {
                continue;
            };
            grouped.entry((stamp, log)).or_default().insert(aside);
        }
    }
    Ok(grouped
        .into_iter()
        .map(|((healed_at, log), set_aside)| OperationalHeal {
            healed_at,
            log,
            set_aside: set_aside.into_iter().collect(),
        })
        .collect())
}

/// Directories matching a `/`-separated pattern whose `*` stands for one name.
fn expand_directories(root: &Path, pattern: &str) -> io::Result<Vec<PathBuf>> {
    let mut current = vec![root.to_path_buf()];
    for component in pattern.split('/') {
        let mut next = Vec::new();
        for directory in current {
            if !component.contains('*') {
                next.push(directory.join(component));
                continue;
            }
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if !name.starts_with('.')
                    && glob_matches(component, name)
                    && entry.file_type()?.is_dir()
                {
                    next.push(entry.path());
                }
            }
        }
        current = next;
    }
    Ok(current)
}

/// Whether `name` matches a one-component pattern in which `*` stands for any run.
fn glob_matches(pattern: &str, name: &str) -> bool {
    let mut pieces = pattern.split('*');
    let first = pieces.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let pieces = pieces.collect::<Vec<_>>();
    let Some((last, middle)) = pieces.split_last() else {
        return rest.is_empty();
    };
    for piece in middle {
        let Some(found) = rest.find(piece) else {
            return false;
        };
        rest = &rest[found + piece.len()..];
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// A durable read's per-line side file, `<stem>.wedged-<stamp>-line-<n>[-<seq>]<ext>`:
/// the healed log's name among `leaves` and the heal's stamp.
fn parse_line_aside(name: &str, leaves: &[&str]) -> Option<(String, u64)> {
    let (stem, rest) = name.split_once(SET_ASIDE_MARKER)?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let stamp = rest[..digits].parse::<u64>().ok()?;
    let after = rest[digits..].strip_prefix("-line-")?;
    let extension = after.find('.').map_or("", |dot| &after[dot..]);
    // A log without an extension (`oplog--*`) gets `.jsonl` on its side files.
    let with_extension = format!("{stem}{extension}");
    leaves
        .iter()
        .find_map(|leaf| {
            let log = if leaf.contains('.') {
                &with_extension
            } else {
                stem
            };
            glob_matches(leaf, log).then(|| log.to_owned())
        })
        .map(|log| (log, stamp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn inspection_keeps_every_valid_line_and_names_each_kind_of_damage() {
        let bytes = b"{\"n\":1}\n{bad}\n\n{\"s\":\"\xff\"}\n\"plain\"\n{\"n\":2";
        let (valid, damaged) = inspect_jsonl_bytes(bytes);
        assert_eq!(valid, 2);
        assert_eq!(
            damaged,
            vec![
                DamagedJsonlLine {
                    line: 2,
                    kind: JsonlDamageKind::MalformedJson
                },
                DamagedJsonlLine {
                    line: 4,
                    kind: JsonlDamageKind::InvalidUtf8
                },
                DamagedJsonlLine {
                    line: 6,
                    kind: JsonlDamageKind::TruncatedTail
                },
            ]
        );
        assert_eq!(inspect_jsonl_bytes(b"{\"n\":1}"), (1, Vec::new()));
        assert_eq!(inspect_jsonl_bytes(b""), (0, Vec::new()));
        assert_eq!(
            inspect_jsonl_bytes(b"\x00\x00\x00\n{\"n\":1}\n").1[0].kind,
            JsonlDamageKind::MalformedJson
        );
    }

    #[test]
    fn inspection_reports_a_journal_relative_path_and_changes_nothing() {
        let journal = TempDir::new();
        let path = journal
            .path()
            .join("chronicle/20260101/_default/120000_60/audio.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = b"{\"start\":\"00:00:01\"}\n{\"start\":\"00:0";
        fs::write(&path, bytes).unwrap();
        let inspection = inspect_jsonl(journal.path(), &path).unwrap();
        assert_eq!(
            inspection.path,
            "chronicle/20260101/_default/120000_60/audio.jsonl"
        );
        assert_eq!(inspection.valid_lines, 1);
        assert!(inspection.truncated_tail());
        assert_eq!(inspection.interior_damaged(), 0);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        let outside = TempDir::new();
        let elsewhere = outside.path().join("audio.jsonl");
        fs::write(&elsewhere, b"{}\n").unwrap();
        let error = inspect_jsonl(journal.path(), &elsewhere).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(
            !error
                .to_string()
                .contains(&outside.path().display().to_string())
        );
    }

    #[test]
    fn quarantine_moves_damaged_lines_into_one_side_file_and_keeps_the_rest_verbatim() {
        let journal = TempDir::new();
        let path = journal.path().join("screen.jsonl");
        fs::write(
            &path,
            b"{\"a\": 1}\r\n{torn\n\n{\"b\":\"\xe2\x9c\x93\"}\n\xff\xfe\n{\"c\":3",
        )
        .unwrap();
        let outcome = quarantine_damaged_jsonl_lines(&path).unwrap();
        assert_eq!(outcome.kept_lines, 2);
        assert_eq!(
            outcome
                .set_aside
                .iter()
                .map(|line| (line.line, line.kind))
                .collect::<Vec<_>>(),
            vec![
                (2, JsonlDamageKind::MalformedJson),
                (5, JsonlDamageKind::InvalidUtf8),
                (6, JsonlDamageKind::TruncatedTail),
            ]
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            b"{\"a\": 1}\r\n\n{\"b\":\"\xe2\x9c\x93\"}\n"
        );
        let aside = outcome.aside_path.unwrap();
        let name = aside.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("screen.wedged-") && name.ends_with("-lines.jsonl"));
        assert_eq!(fs::read(&aside).unwrap(), b"{torn\n\xff\xfe\n{\"c\":3\n");
        let again = quarantine_damaged_jsonl_lines(&path).unwrap();
        assert!(again.set_aside.is_empty() && again.aside_path.is_none());
    }

    #[test]
    fn quarantine_reuses_a_lock_this_thread_already_holds() {
        let journal = TempDir::new();
        let path = journal.path().join("audio.jsonl");
        fs::write(&path, b"{\"ok\":true}\n{bad\n").unwrap();
        let _held = crate::hold_lock(&path, LockOptions::default()).unwrap();
        let outcome = quarantine_damaged_jsonl_lines(&path).unwrap();
        assert_eq!(outcome.set_aside.len(), 1);
        assert_eq!(fs::read(&path).unwrap(), b"{\"ok\":true}\n");
    }

    #[test]
    fn recurrence_counts_heals_of_registered_logs_inside_the_window() {
        let journal = TempDir::new();
        let root = journal.path();
        let now = SystemTime::now();
        let secs = |at: SystemTime| at.duration_since(UNIX_EPOCH).unwrap().as_secs();
        let recent = secs(now) - 60;
        let older = secs(now) - 8 * 24 * 60 * 60;
        let awareness = root.join("awareness");
        let talents = root.join("chronicle/20260101/talents");
        let segment = root.join("chronicle/20260101/_default/120000_60");
        for directory in [&awareness, &talents, &segment] {
            fs::create_dir_all(directory).unwrap();
        }
        // One heal that set two lines aside at once counts once.
        fs::write(
            awareness.join(format!("20260101.wedged-{recent}-line-2.jsonl")),
            b"x",
        )
        .unwrap();
        fs::write(
            awareness.join(format!("20260101.wedged-{recent}-line-5.jsonl")),
            b"x",
        )
        .unwrap();
        fs::write(
            awareness.join(format!("20260101.wedged-{older}-line-1.jsonl")),
            b"x",
        )
        .unwrap();
        // An owner-confirmed transcript set-aside is not a quiet heal.
        fs::write(
            segment.join(format!("audio.wedged-{recent}-lines.jsonl")),
            b"x",
        )
        .unwrap();
        let once = operational_heal_recurrence(root, now).unwrap();
        assert_eq!(once.heals.len(), 1);
        assert_eq!(once.heals[0].log, "awareness/20260101.jsonl");
        assert_eq!(once.heals[0].set_aside.len(), 2);
        assert!(!once.recurring);

        fs::write(
            talents.join(format!("pulse.wedged-{}-line-3.jsonl", recent + 1)),
            b"x",
        )
        .unwrap();
        let twice = operational_heal_recurrence(root, now).unwrap();
        assert_eq!(twice.heals.len(), 2);
        assert_eq!(twice.heals[0].log, "chronicle/20260101/talents/pulse.jsonl");
        assert!(twice.recurring);
    }

    #[test]
    fn glob_matching_covers_the_registered_leaves() {
        assert!(glob_matches("*.jsonl", "20260101.jsonl"));
        assert!(!glob_matches("*.jsonl", "20260101.json"));
        assert!(glob_matches("oplog--*", "oplog--segment"));
        assert!(glob_matches("indexer.jsonl", "indexer.jsonl"));
        assert!(!glob_matches("indexer.jsonl", "indexer.jsonl.lock"));
    }
}
