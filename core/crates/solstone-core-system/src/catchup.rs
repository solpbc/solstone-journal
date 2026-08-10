// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only catchup-day selection shared by the supervisor tick.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{PathError, PathOrDay, day_dirs, iter_segments};
use thiserror::Error;

pub const MAX_UPDATED_CATCHUP: usize = 4;

const RAW_HASHED_NAMES: [&str; 5] = [
    "audio.json",
    "audio.jsonl",
    "screen.jsonl",
    "conversation_transcript.jsonl",
    "chat.jsonl",
];
const RAW_HASHED_SUFFIXES: [&str; 3] = ["_audio.jsonl", "_screen.jsonl", "_transcript.md"];
const MEDIA_EXTENSIONS: [&str; 17] = [
    ".flac", ".opus", ".ogg", ".m4a", ".mp3", ".wav", ".webm", ".mp4", ".mov", ".png", ".jpg",
    ".jpeg", ".heic", ".heif", ".gif", ".webp", ".tiff",
];
const PDF_EXTENSIONS: [&str; 1] = [".pdf"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchupKind {
    DailyCatchup,
    SegmentRepair,
}

impl CatchupKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DailyCatchup => "daily-catchup",
            Self::SegmentRepair => "segment-repair",
        }
    }
}

#[derive(Debug, Error)]
pub enum CatchupError {
    #[error("catchup journal path error: {0}")]
    Path(#[from] PathError),
    #[error("catchup I/O failed at {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("catchup state is malformed: {0}")]
    State(String),
}

/// Return ascending day keys whose stream marker is newer than their daily marker.
pub fn updated_days(
    journal: &Path,
    exclude: &BTreeSet<String>,
) -> Result<Vec<String>, CatchupError> {
    let days = day_dirs(journal)?;
    let mut updated = Vec::new();
    for (day, path) in days {
        if exclude.contains(&day) {
            continue;
        }
        let stream = path.join("health/stream.updated");
        if !stream.is_file() {
            continue;
        }
        let daily = path.join("health/daily.updated");
        if !daily.is_file() || modified(&daily)? < modified(&stream)? {
            updated.push(day);
        }
    }
    updated.sort();
    Ok(updated)
}

/// Return whether one day/kind record may be drained now.
pub fn day_eligible_to_drain(
    journal: &Path,
    day: &str,
    kind: CatchupKind,
    now: SystemTime,
) -> Result<bool, CatchupError> {
    let entries = read_entries(journal)?;
    let key = format!("{day}:{}", kind.as_str());
    let Some(entry) = entries.get(&key) else {
        return Ok(true);
    };
    let entry = entry
        .as_object()
        .ok_or_else(|| CatchupError::State(format!("entry {key} is not an object")))?;
    if entry.get("active").is_some_and(json_truthy) {
        return Ok(false);
    }
    let retry_at = entry.get("next_retry_at").map_or(Ok(0.0), json_number)?;
    let now = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    if now >= retry_at {
        return Ok(true);
    }
    let fingerprint = entry
        .get("fingerprint")
        .and_then(Value::as_str)
        .ok_or_else(|| CatchupError::State(format!("entry {key} has no string fingerprint")))?;
    Ok(read_raw_input_fingerprint(journal, day)? != fingerprint)
}

/// Return the Python-compatible raw-input fingerprint for a chronicle day.
pub fn read_raw_input_fingerprint(journal: &Path, day: &str) -> Result<String, CatchupError> {
    let day_dir = journal.join("chronicle").join(day);
    let mut entries = Vec::new();
    for segment in iter_segments(journal, PathOrDay::Day(day))? {
        for entry in read_dir(&segment.path)? {
            let entry = entry.map_err(|source| CatchupError::Io {
                path: segment.path.clone(),
                source,
            })?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let marker = if is_raw_hashed(&name) {
                sha256_file(&path)?
            } else if is_sized_media(&path) {
                format!("size:{}", metadata(&path)?.len())
            } else {
                continue;
            };
            let relative = path
                .strip_prefix(&day_dir)
                .map_err(|_| {
                    CatchupError::State(format!("segment path escaped day: {}", path.display()))
                })?
                .to_string_lossy()
                .replace('\\', "/");
            entries.push((relative, marker));
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(hex_digest(Sha256::digest(
        compact_ascii_entries(&entries).as_bytes(),
    )))
}

/// Select ascending natural and forced days that pass both catchup gates.
pub fn eligible_catchup_days(
    journal: &Path,
    force_days: &[String],
    exclude: &BTreeSet<String>,
    now: SystemTime,
) -> Result<Vec<String>, CatchupError> {
    let natural = updated_days(journal, exclude)?;
    let eligible_natural = natural
        .into_iter()
        .filter(|day| eligible_or_fail_open(journal, day, now))
        .collect::<Vec<_>>();
    let freshest = eligible_natural
        .into_iter()
        .rev()
        .take(MAX_UPDATED_CATCHUP)
        .collect::<BTreeSet<_>>();
    let mut merged = freshest;
    for day in force_days {
        if eligible_or_fail_open(journal, day, now) {
            merged.insert(day.clone());
        }
    }
    Ok(merged.into_iter().collect())
}

fn eligible_or_fail_open(journal: &Path, day: &str, now: SystemTime) -> bool {
    match (|| {
        Ok::<_, CatchupError>(
            day_eligible_to_drain(journal, day, CatchupKind::DailyCatchup, now)?
                && day_eligible_to_drain(journal, day, CatchupKind::SegmentRepair, now)?,
        )
    })() {
        Ok(eligible) => eligible,
        Err(error) => {
            eprintln!(
                "supervisor: catchup eligibility check failed for {day}; treating as eligible: {error}"
            );
            true
        }
    }
}

fn read_entries(journal: &Path) -> Result<HashMap<String, Value>, CatchupError> {
    let path = journal.join("health/catchup-state.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(source) => return Err(CatchupError::Io { path, source }),
    };
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CatchupError::State(format!("invalid JSON: {error}")))?;
    let entries = value
        .as_object()
        .and_then(|value| value.get("entries"))
        .and_then(Value::as_object)
        .ok_or_else(|| CatchupError::State("entries is not an object".to_owned()))?;
    Ok(entries
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect())
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn json_number(value: &Value) -> Result<f64, CatchupError> {
    value
        .as_f64()
        .ok_or_else(|| CatchupError::State("next_retry_at is not numeric".to_owned()))
}

fn is_raw_hashed(name: &str) -> bool {
    RAW_HASHED_NAMES.contains(&name)
        || RAW_HASHED_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
        || (name.starts_with("monitor_")
            && (name.ends_with("_diff.json") || name.ends_with("_diff_box.json")))
}

fn is_sized_media(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
        .unwrap_or_default();
    MEDIA_EXTENSIONS.contains(&extension.as_str()) || PDF_EXTENSIONS.contains(&extension.as_str())
}

fn compact_ascii_entries(entries: &[(String, String)]) -> String {
    let body = entries
        .iter()
        .map(|(path, marker)| format!("[{},{}]", quote_ascii(path), quote_ascii(marker)))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{body}]")
}

fn quote_ascii(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('"');
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0c}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character <= '\u{1f}' => {
                use std::fmt::Write;
                write!(&mut result, "\\u{:04x}", character as u32).expect("String write");
            }
            character if character.is_ascii() => result.push(character),
            character => {
                for unit in character.encode_utf16(&mut [0; 2]) {
                    use std::fmt::Write;
                    write!(&mut result, "\\u{unit:04x}").expect("String write");
                }
            }
        }
    }
    result.push('"');
    result
}

fn sha256_file(path: &Path) -> Result<String, CatchupError> {
    let bytes = fs::read(path).map_err(|source| CatchupError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(hex_digest(Sha256::digest(&bytes)))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_dir(path: &Path) -> Result<fs::ReadDir, CatchupError> {
    fs::read_dir(path).map_err(|source| CatchupError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn metadata(path: &Path) -> Result<fs::Metadata, CatchupError> {
    fs::metadata(path).map_err(|source| CatchupError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn modified(path: &Path) -> Result<SystemTime, CatchupError> {
    metadata(path)?
        .modified()
        .map_err(|source| CatchupError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use sha2::{Digest, Sha256};

    use super::*;

    static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

    struct Bed {
        root: PathBuf,
    }

    impl Bed {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "solstone-catchup-{name}-{}",
                NEXT_PATH.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).expect("temporary journal");
            Self { root }
        }

        fn write(&self, relative: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            fs::write(path, contents).expect("write fixture");
        }

        fn segment_file(&self, day: &str, segment: &str, name: &str, contents: &[u8]) {
            self.write(
                Path::new("chronicle").join(day).join(segment).join(name),
                contents,
            );
        }

        fn updated_day(&self, day: &str) {
            self.write(
                Path::new("chronicle")
                    .join(day)
                    .join("health/stream.updated"),
                b"stream",
            );
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn state_entry(day: &str, kind: CatchupKind, entry: &str) -> String {
        format!(
            r#"{{"version":1,"entries":{{"{day}:{}":{entry}}}}}"#,
            kind.as_str()
        )
    }

    fn digest(bytes: &[u8]) -> String {
        hex_digest(Sha256::digest(bytes))
    }

    fn empty_fingerprint() -> String {
        digest(b"[]")
    }

    #[test]
    fn missing_catchup_state_is_eligible() {
        let bed = Bed::new("missing-state");
        assert!(
            day_eligible_to_drain(&bed.root, "20260101", CatchupKind::DailyCatchup, UNIX_EPOCH)
                .expect("missing state")
        );
    }

    #[test]
    fn active_catchup_state_is_not_eligible() {
        let bed = Bed::new("active");
        bed.write(
            "health/catchup-state.json",
            state_entry("20260101", CatchupKind::DailyCatchup, r#"{"active":true}"#),
        );
        assert!(
            !day_eligible_to_drain(&bed.root, "20260101", CatchupKind::DailyCatchup, UNIX_EPOCH)
                .expect("active state")
        );
    }

    #[test]
    fn past_retry_is_eligible() {
        let bed = Bed::new("past-retry");
        bed.write(
            "health/catchup-state.json",
            state_entry(
                "20260101",
                CatchupKind::DailyCatchup,
                r#"{"next_retry_at":10,"fingerprint":"ignored"}"#,
            ),
        );
        assert!(
            day_eligible_to_drain(
                &bed.root,
                "20260101",
                CatchupKind::DailyCatchup,
                UNIX_EPOCH + Duration::from_secs(10)
            )
            .expect("past retry")
        );
    }

    #[test]
    fn unchanged_fingerprint_before_retry_is_not_eligible() {
        let bed = Bed::new("unchanged-fingerprint");
        let fingerprint = empty_fingerprint();
        bed.write(
            "health/catchup-state.json",
            state_entry(
                "20260101",
                CatchupKind::DailyCatchup,
                &format!(r#"{{"next_retry_at":10,"fingerprint":"{fingerprint}"}}"#),
            ),
        );
        assert!(
            !day_eligible_to_drain(
                &bed.root,
                "20260101",
                CatchupKind::DailyCatchup,
                UNIX_EPOCH + Duration::from_secs(9)
            )
            .expect("unchanged fingerprint")
        );
    }

    #[test]
    fn changed_fingerprint_before_retry_is_eligible() {
        let bed = Bed::new("changed-fingerprint");
        bed.segment_file("20260101", "120000_1", "audio.json", b"new raw input");
        bed.write(
            "health/catchup-state.json",
            state_entry(
                "20260101",
                CatchupKind::DailyCatchup,
                &format!(
                    r#"{{"next_retry_at":10,"fingerprint":"{}"}}"#,
                    empty_fingerprint()
                ),
            ),
        );
        assert!(
            day_eligible_to_drain(
                &bed.root,
                "20260101",
                CatchupKind::DailyCatchup,
                UNIX_EPOCH + Duration::from_secs(9)
            )
            .expect("changed fingerprint")
        );
    }

    #[test]
    fn raw_hash_conditions_use_sha256_markers() {
        let bed = Bed::new("raw-names");
        let files = [
            ("audio.json", b"exact".as_slice()),
            ("capture_audio.jsonl", b"suffix".as_slice()),
            ("monitor_12_diff_box.json", b"glob".as_slice()),
        ];
        for (name, contents) in &files {
            bed.segment_file("20260101", "120000_1", name, contents);
        }
        let entries = files
            .iter()
            .map(|(name, contents)| (format!("120000_1/{name}"), digest(contents)))
            .collect::<Vec<_>>();
        let mut entries = entries;
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            read_raw_input_fingerprint(&bed.root, "20260101").expect("fingerprint"),
            digest(compact_ascii_entries(&entries).as_bytes())
        );
        assert!(is_raw_hashed("audio.json"));
        assert!(is_raw_hashed("capture_audio.jsonl"));
        assert!(is_raw_hashed("monitor_12_diff_box.json"));
    }

    #[test]
    fn media_and_pdf_use_sizes_and_skip_unrelated_files() {
        let bed = Bed::new("media");
        bed.segment_file("20260101", "120000_1", "image.PNG", b"1234");
        bed.segment_file("20260101", "120000_1", "report.pdf", b"123456");
        bed.segment_file("20260101", "120000_1", "notes.txt", b"must not count");
        let entries = vec![
            ("120000_1/image.PNG".to_owned(), "size:4".to_owned()),
            ("120000_1/report.pdf".to_owned(), "size:6".to_owned()),
        ];
        assert_eq!(
            read_raw_input_fingerprint(&bed.root, "20260101").expect("fingerprint"),
            digest(compact_ascii_entries(&entries).as_bytes())
        );
    }

    #[test]
    fn fingerprint_sorts_entries_by_path_not_marker() {
        let bed = Bed::new("path-sort");
        bed.segment_file("20260101", "130000_1", "audio.json", b"a");
        bed.segment_file("20260101", "120000_1", "audio.json", b"z");
        let entries = vec![
            ("120000_1/audio.json".to_owned(), digest(b"z")),
            ("130000_1/audio.json".to_owned(), digest(b"a")),
        ];
        assert_eq!(
            read_raw_input_fingerprint(&bed.root, "20260101").expect("fingerprint"),
            digest(compact_ascii_entries(&entries).as_bytes())
        );
    }

    #[test]
    fn eligible_days_caps_natural_days_but_keeps_forced_days() {
        let bed = Bed::new("cap");
        for day in [
            "20260101", "20260102", "20260103", "20260104", "20260105", "20260106",
        ] {
            bed.updated_day(day);
        }
        assert_eq!(
            eligible_catchup_days(
                &bed.root,
                &["20260101".to_owned()],
                &BTreeSet::new(),
                SystemTime::now(),
            )
            .expect("eligible days"),
            vec![
                "20260101".to_owned(),
                "20260103".to_owned(),
                "20260104".to_owned(),
                "20260105".to_owned(),
                "20260106".to_owned(),
            ]
        );
    }

    #[test]
    fn updated_days_honors_exclusion_and_marker_order() {
        let bed = Bed::new("updated-days");
        bed.updated_day("20260101");
        bed.write("chronicle/20260102/health/daily.updated", b"daily first");
        thread::sleep(Duration::from_millis(20));
        bed.updated_day("20260102");
        bed.updated_day("20260103");
        thread::sleep(Duration::from_millis(20));
        bed.write("chronicle/20260103/health/daily.updated", b"daily second");
        assert_eq!(
            updated_days(&bed.root, &BTreeSet::new()).expect("updated days"),
            vec!["20260101".to_owned(), "20260102".to_owned()]
        );
        assert_eq!(
            updated_days(&bed.root, &BTreeSet::from(["20260101".to_owned()]))
                .expect("updated days"),
            vec!["20260102".to_owned()]
        );
    }
}
