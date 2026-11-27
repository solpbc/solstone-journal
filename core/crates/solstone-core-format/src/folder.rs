// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Watched-folder activity and content uploads from a linked device.
//!
//! A watched folder sends one activity file (`folder_<sha256>.jsonl`) per
//! 5-minute window (`HHMMSS_300` on the journal's 300-second grid), optionally
//! accompanied by content parts referenced by `part` fields on activity lines.
//! This module is the single definition of the wire metadata, activity-line
//! contract ([`validate_folder_meta`], [`validate_folder_file`]), bound stream
//! naming ([`folder_stream_source`], [`is_folder_source_name`]), and
//! deterministic `screen_text.md` activity rendering
//! ([`render_folder_activity_markdown`]).

use std::fmt;

use jiff::ZonedDateTime;
use jiff::civil::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::calendar::segment_window;
use crate::content::screen_text::{append_screen_text_line, redact_inline, screen_text_minute};

/// The wire ingest source every watched-folder upload declares.
pub const FOLDER_SOURCE: &str = "folder";

/// The capability name listed by `/api/system/status` when this journal
/// understands watched-folder uploads.
pub const FOLDER_CAPABILITY: &str = "folder";

/// The prefix of every folder activity file and bound per-folder stream source.
pub const FOLDER_FILE_PREFIX: &str = "folder_";

/// The `meta` key carrying watched-folder identity and preferences.
pub const FOLDER_META_KEY: &str = "folder";

/// Structured per-segment record of folder activity events and owner facet.
pub const FOLDER_ACTIVITY_FILE: &str = "folder_activity.json";

/// Stored transcript filename for a transcript file delivered by a folder.
pub const FOLDER_TRANSCRIPT_FILE: &str = "folder_transcript.jsonl";

/// Stored document transcript filename for a document delivered by a folder.
pub const DOCUMENT_TRANSCRIPT_FILE: &str = "document_transcript.md";

/// The segment window length for watched folders, in seconds.
pub const FOLDER_WINDOW_SECONDS: u32 = 300;

/// Default quiet period before a revised document or transcript is re-sent, in
/// seconds.
pub const DOCUMENT_REVISION_COOLDOWN_SECONDS: u64 = 3_600;

/// Most activity lines in one folder activity file.
pub const MAX_FILE_LINES: usize = 5_000;

/// Most bytes in one folder activity file.
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

/// Most bytes of text in one folder note.
pub const MAX_NOTE_BYTES: usize = 4 * 1024 * 1024;

/// Canonical content kinds a folder may turn on (`"other"` covers
/// metadata-only activity recording for all other files).
pub const SUPPORTED_FOLDER_KINDS: [&str; 6] = [
    "notes",
    "documents",
    "transcripts",
    "screenshots",
    "photos",
    "other",
];

const MAX_FOLDER_ID_BYTES: usize = 64;
const MAX_FOLDER_NAME_BYTES: usize = 128;
const MAX_FACET_SLUG_BYTES: usize = 64;
const MAX_PATH_BYTES: usize = 1024;
const MAX_FILE_TYPE_BYTES: usize = 64;
const MAX_KIND_HINT_BYTES: usize = 64;
const MAX_PART_NAME_BYTES: usize = 128;
const MAX_PLATFORM_XATTRS: usize = 32;
const MAX_PLATFORM_VALUE_BYTES: usize = 2048;

const REQUIRED_LINE_KEYS: [&str; 5] = ["v", "event", "path", "at", "observed_at"];

/// Derive the internal bound stream source for a folder ID (`folder_<12 hex>`).
///
/// Every folder binds its own stream so two folders with activity in the same
/// 5-minute window each keep their own segment without colliding.
#[must_use]
pub fn folder_stream_source(folder_id: &str) -> String {
    let hash = format!("{:x}", Sha256::digest(folder_id.as_bytes()));
    format!("{FOLDER_FILE_PREFIX}{}", &hash[..12])
}

/// Whether `source` is the wire `"folder"` source or a bound per-folder stream
/// source (`folder_<12 lowercase hex>`).
#[must_use]
pub fn is_folder_source_name(source: &str) -> bool {
    if source == FOLDER_SOURCE {
        return true;
    }
    source.strip_prefix(FOLDER_FILE_PREFIX).is_some_and(|hex| {
        hex.len() == 12
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Whether `name` is a valid folder activity filename: `folder_<64 hex>.jsonl`.
#[must_use]
pub fn is_folder_file_name(name: &str) -> bool {
    name.strip_prefix(FOLDER_FILE_PREFIX)
        .and_then(|rest| rest.strip_suffix(".jsonl"))
        .is_some_and(is_sha256_hex)
}

/// Whether `name` claims to be a folder activity file, well-formed or not.
#[must_use]
pub fn names_folder_file(name: &str) -> bool {
    name.starts_with(FOLDER_FILE_PREFIX) && name.ends_with(".jsonl")
}

/// Whether `id` is a valid stable client-assigned folder identifier.
#[must_use]
pub fn is_valid_folder_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_FOLDER_ID_BYTES
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
}

/// Whether `slug` is a syntactically valid facet slug.
#[must_use]
pub fn is_valid_facet_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_FACET_SLUG_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
}

/// Whether `value` is 64 lowercase ASCII hex digits.
#[must_use]
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Checked `meta.folder` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FolderMeta {
    pub id: String,
    pub name: String,
    pub facet: Option<String>,
    pub kinds: Vec<String>,
}

/// What happened to a file inside a watched folder.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderEventKind {
    Added,
    Changed,
    Renamed,
    Removed,
}

impl FolderEventKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Changed => "changed",
            Self::Renamed => "renamed",
            Self::Removed => "removed",
        }
    }

    #[must_use]
    const fn display_verb(self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Changed => "Edited",
            Self::Renamed => "Renamed",
            Self::Removed => "Removed",
        }
    }
}

/// One checked line from a `folder_<sha256>.jsonl` activity file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FolderActivityLine {
    pub event: FolderEventKind,
    pub path: String,
    pub previous_path: Option<String>,
    pub at: ZonedDateTime,
    pub observed_at: ZonedDateTime,
    pub file_type: Option<String>,
    pub size: Option<u64>,
    pub created_at: Option<ZonedDateTime>,
    pub modified_at: Option<ZonedDateTime>,
    pub content_hash: Option<String>,
    pub part: Option<String>,
    pub kind_hint: Option<String>,
    pub platform: Option<Map<String, Value>>,
}

impl FolderActivityLine {
    #[must_use]
    pub fn at_ms(&self) -> i64 {
        self.at.timestamp().as_millisecond()
    }

    #[must_use]
    pub fn observed_at_ms(&self) -> i64 {
        self.observed_at.timestamp().as_millisecond()
    }

    /// The primary origin URL carried in platform metadata, if any.
    #[must_use]
    pub fn origin_url(&self) -> Option<&str> {
        let platform = self.platform.as_ref()?;
        platform
            .get("origin_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .or_else(|| {
                platform
                    .get("where_froms")
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.iter().find_map(Value::as_str))
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
            })
    }

    /// The browser or app that downloaded or created the file, if reported.
    #[must_use]
    pub fn downloaded_by(&self) -> Option<&str> {
        let platform = self.platform.as_ref()?;
        platform
            .get("downloaded_by")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    }

    /// The application that produced the file, if reported.
    #[must_use]
    pub fn source_app(&self) -> Option<&str> {
        let platform = self.platform.as_ref()?;
        platform
            .get("source_app")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    }
}

/// Why a folder upload's metadata or activity file broke the contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FolderError {
    /// 1-based line number inside the activity file, or `None` for whole-file
    /// or metadata errors.
    pub line: Option<usize>,
    pub reason: String,
}

impl fmt::Display for FolderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "line {line}: {}", self.reason),
            None => formatter.write_str(&self.reason),
        }
    }
}

impl std::error::Error for FolderError {}

fn whole_error(reason: impl Into<String>) -> FolderError {
    FolderError {
        line: None,
        reason: reason.into(),
    }
}

/// Read and validate `meta.folder`.
pub fn validate_folder_meta(meta: &Map<String, Value>) -> Result<FolderMeta, FolderError> {
    let Some(Value::Object(folder)) = meta.get(FOLDER_META_KEY) else {
        return Err(whole_error("meta.folder is required and must be an object"));
    };
    match folder.get("v") {
        Some(Value::Number(v)) if v.as_u64() == Some(1) => {}
        _ => return Err(whole_error("meta.folder.v must be 1")),
    }
    let Some(Value::String(id)) = folder.get("id") else {
        return Err(whole_error("meta.folder.id is required"));
    };
    if !is_valid_folder_id(id) {
        return Err(whole_error(
            "meta.folder.id must be 1 to 64 lowercase ASCII letters, digits, hyphens or underscores starting with a letter or digit",
        ));
    }
    let Some(Value::String(name)) = folder.get("name") else {
        return Err(whole_error("meta.folder.name is required"));
    };
    if name.trim().is_empty()
        || name.len() > MAX_FOLDER_NAME_BYTES
        || name.chars().any(char::is_control)
    {
        return Err(whole_error(
            "meta.folder.name must be 1 to 128 non-blank UTF-8 bytes with no control characters",
        ));
    }
    let facet = match folder.get("facet") {
        None | Some(Value::Null) => None,
        Some(Value::String(slug)) if is_valid_facet_slug(slug) => Some(slug.clone()),
        Some(Value::String(_)) => {
            return Err(whole_error(
                "meta.folder.facet must be a lowercase facet slug",
            ));
        }
        Some(_) => {
            return Err(whole_error("meta.folder.facet must be a string or null"));
        }
    };
    let kinds = match folder.get("kinds") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let Some(kind) = item.as_str() else {
                    return Err(whole_error("meta.folder.kinds entries must be strings"));
                };
                if !SUPPORTED_FOLDER_KINDS.contains(&kind) {
                    return Err(whole_error(format!(
                        "meta.folder.kinds contains unsupported kind '{kind}'"
                    )));
                }
                if !out.iter().any(|existing: &String| existing == kind) {
                    out.push(kind.to_owned());
                }
            }
            out
        }
        Some(_) => return Err(whole_error("meta.folder.kinds must be an array")),
    };
    Ok(FolderMeta {
        id: id.clone(),
        name: name.trim().to_owned(),
        facet,
        kinds,
    })
}

/// Validate a `folder_<sha256>.jsonl` file against the wire contract.
///
/// Every line's `at`, read as wall-clock time in its own offset, must fall
/// within the 300-second window named by `(day, segment)`.
pub fn validate_folder_file(
    name: &str,
    bytes: &[u8],
    day: &str,
    segment: &str,
) -> Result<Vec<FolderActivityLine>, FolderError> {
    if !is_folder_file_name(name) {
        return Err(whole_error(
            "activity file name must be folder_ followed by 64 lowercase hex digits and .jsonl",
        ));
    }
    let expected_hash = &name[FOLDER_FILE_PREFIX.len()..name.len() - 6];
    if format!("{:x}", Sha256::digest(bytes)) != expected_hash {
        return Err(whole_error(
            "activity file name must be the SHA-256 of the file",
        ));
    }
    let window = segment_window(day, segment).ok_or_else(|| {
        whole_error("segment must name one 300-second window on the journal's grid")
    })?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(whole_error("activity file holds more than 8 MiB"));
    }
    if bytes.last() != Some(&b'\n') {
        return Err(whole_error("activity file must end with a newline"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| whole_error("activity file is not UTF-8"))?;
    let mut lines = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if index == MAX_FILE_LINES {
            return Err(whole_error("activity file holds more than 5,000 lines"));
        }
        let parsed = parse_line(line, window).map_err(|reason| FolderError {
            line: Some(index + 1),
            reason,
        })?;
        lines.push(parsed);
    }
    if lines.is_empty() {
        return Err(whole_error("activity file holds no lines"));
    }
    Ok(lines)
}

fn parse_line(
    line: &str,
    (window_start, window_end): (DateTime, DateTime),
) -> Result<FolderActivityLine, String> {
    let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
        return Err("line is not a JSON object".to_owned());
    };
    for key in REQUIRED_LINE_KEYS {
        if !record.contains_key(key) {
            return Err(format!("{key} is missing"));
        }
    }
    if record.get("v").and_then(Value::as_i64) != Some(1) {
        return Err("v must be 1".to_owned());
    }
    let event = match record.get("event").and_then(Value::as_str) {
        Some("added") => FolderEventKind::Added,
        Some("changed") => FolderEventKind::Changed,
        Some("renamed") => FolderEventKind::Renamed,
        Some("removed") => FolderEventKind::Removed,
        _ => return Err("event must be added, changed, renamed or removed".to_owned()),
    };
    let path = req_string(&record, "path")?;
    check_relative_path(&path).map_err(|reason| format!("path {reason}"))?;
    let previous_path = opt_string(&record, "previous_path")?;
    match (event, &previous_path) {
        (FolderEventKind::Renamed, Some(prev)) => {
            check_relative_path(prev).map_err(|reason| format!("previous_path {reason}"))?;
        }
        (FolderEventKind::Renamed, None) => {
            return Err("a renamed line requires previous_path".to_owned());
        }
        (_, Some(_)) => return Err("only a renamed line carries previous_path".to_owned()),
        _ => {}
    }
    let at = req_time(&record, "at")?;
    let observed_at = req_time(&record, "observed_at")?;
    let local = at.datetime();
    if local < window_start || local >= window_end {
        return Err("at falls outside the segment window".to_owned());
    }
    let file_type = opt_string(&record, "file_type")?;
    if let Some(ft) = &file_type
        && (ft.is_empty() || ft.len() > MAX_FILE_TYPE_BYTES || ft.chars().any(char::is_control))
    {
        return Err("file_type must be 1 to 64 bytes with no control characters".to_owned());
    }
    let size = match record.get("size") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => Some(
            n.as_u64()
                .ok_or_else(|| "size must be a non-negative integer".to_owned())?,
        ),
        Some(_) => return Err("size must be a non-negative integer or null".to_owned()),
    };
    let created_at = opt_time(&record, "created_at")?;
    let modified_at = opt_time(&record, "modified_at")?;
    let content_hash = opt_string(&record, "content_hash")?;
    if let Some(hash) = &content_hash
        && !is_sha256_hex(hash)
    {
        return Err("content_hash must be 64 lowercase hex digits".to_owned());
    }
    let part = opt_string(&record, "part")?;
    if let Some(part_name) = &part {
        if part_name.is_empty()
            || part_name.len() > MAX_PART_NAME_BYTES
            || part_name.chars().any(char::is_control)
            || part_name.contains(['/', '\\'])
        {
            return Err("part must be a simple file name".to_owned());
        }
        if event == FolderEventKind::Removed {
            return Err("a removed line carries no part".to_owned());
        }
        if content_hash.is_none() {
            return Err("content_hash is required when part is set".to_owned());
        }
    }
    let kind_hint = opt_string(&record, "kind_hint")?;
    if let Some(hint) = &kind_hint
        && (hint.is_empty()
            || hint.len() > MAX_KIND_HINT_BYTES
            || hint.chars().any(char::is_control))
    {
        return Err("kind_hint must be 1 to 64 bytes with no control characters".to_owned());
    }
    let platform = match record.get("platform") {
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) => {
            validate_platform_metadata(map)?;
            Some(map.clone())
        }
        Some(_) => return Err("platform must be an object or null".to_owned()),
    };
    Ok(FolderActivityLine {
        event,
        path,
        previous_path,
        at,
        observed_at,
        file_type,
        size,
        created_at,
        modified_at,
        content_hash,
        part,
        kind_hint,
        platform,
    })
}

fn validate_platform_metadata(platform: &Map<String, Value>) -> Result<(), String> {
    if let Some(xattrs) = platform.get("xattrs")
        && !xattrs.is_null()
    {
        let Some(map) = xattrs.as_object() else {
            return Err("platform.xattrs must be an object".to_owned());
        };
        if map.len() > MAX_PLATFORM_XATTRS {
            return Err("platform.xattrs holds too many entries".to_owned());
        }
        for (key, value) in map {
            if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
                return Err("platform.xattrs key is invalid".to_owned());
            }
            match value {
                Value::String(s) if s.len() <= MAX_PLATFORM_VALUE_BYTES => {}
                Value::String(_) => {
                    return Err("platform.xattrs value is too long".to_owned());
                }
                _ => return Err("platform.xattrs values must be strings".to_owned()),
            }
        }
    }
    for key in [
        "origin_url",
        "referrer_url",
        "quarantine",
        "downloaded_by",
        "source_app",
    ] {
        if let Some(val) = platform.get(key)
            && !val.is_null()
        {
            let Some(s) = val.as_str() else {
                return Err(format!("platform.{key} must be a string or null"));
            };
            if s.len() > MAX_PLATFORM_VALUE_BYTES || s.chars().any(char::is_control) {
                return Err(format!("platform.{key} is invalid"));
            }
        }
    }
    if let Some(where_froms) = platform.get("where_froms")
        && !where_froms.is_null()
    {
        let Some(arr) = where_froms.as_array() else {
            return Err("platform.where_froms must be an array or null".to_owned());
        };
        if arr.len() > 16 {
            return Err("platform.where_froms holds too many entries".to_owned());
        }
        for entry in arr {
            let Some(s) = entry.as_str() else {
                return Err("platform.where_froms entries must be strings".to_owned());
            };
            if s.len() > MAX_PLATFORM_VALUE_BYTES || s.chars().any(char::is_control) {
                return Err("platform.where_froms entry is invalid".to_owned());
            }
        }
    }
    if let Some(zone_id) = platform.get("zone_id")
        && !zone_id.is_null()
        && zone_id.as_u64().is_none()
    {
        return Err("platform.zone_id must be a non-negative integer or null".to_owned());
    }
    Ok(())
}

fn req_string(record: &Map<String, Value>, key: &str) -> Result<String, String> {
    match record.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(format!("{key} must be a string")),
    }
}

fn opt_string(record: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match record.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(format!("{key} must be a string or null")),
    }
}

fn req_time(record: &Map<String, Value>, key: &str) -> Result<ZonedDateTime, String> {
    let raw = req_string(record, key)?;
    solstone_core_time::parse_rfc3339(&raw)
        .map_err(|_| format!("{key} must be RFC 3339 with an offset"))
}

fn opt_time(record: &Map<String, Value>, key: &str) -> Result<Option<ZonedDateTime>, String> {
    match opt_string(record, key)? {
        None => Ok(None),
        Some(raw) => solstone_core_time::parse_rfc3339(&raw)
            .map(Some)
            .map_err(|_| format!("{key} must be RFC 3339 with an offset")),
    }
}

fn check_relative_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return Err("must be 1 to 1024 bytes");
    }
    if path.starts_with('/') || path.ends_with('/') {
        return Err("must be relative with no leading or trailing slash");
    }
    if path.contains('\\') || path.chars().any(char::is_control) {
        return Err("must use forward slashes with no control characters");
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("must have no empty or dot path segments");
    }
    Ok(())
}

/// Persisted structured activity record stored at
/// `chronicle/<day>/<stream>/<segment>/talents/folder_activity.json`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredFolderActivity {
    pub v: u32,
    pub folder_id: String,
    pub folder_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_facet: Option<String>,
    #[serde(default)]
    pub events: Vec<StoredFolderEvent>,
}

/// One persisted file activity event inside a 5-minute folder segment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredFolderEvent {
    pub key: String,
    pub event: FolderEventKind,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    pub at: String,
    pub at_ms: i64,
    pub offset_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloaded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed_file: Option<String>,
}

/// Compute an event's offset in milliseconds from the start of its 5-minute
/// window (`day`, `segment`), clamped to `[0, 299_999]`.
#[must_use]
pub fn event_offset_ms(day: &str, segment: &str, at: &ZonedDateTime) -> i64 {
    let Some((window_start, _)) = segment_window(day, segment) else {
        return 0;
    };
    let start_sec = i64::from(window_start.hour()) * 3600
        + i64::from(window_start.minute()) * 60
        + i64::from(window_start.second());
    let local = at.datetime();
    let local_sec =
        i64::from(local.hour()) * 3600 + i64::from(local.minute()) * 60 + i64::from(local.second());
    let delta_ms =
        (local_sec - start_sec) * 1_000 + i64::from(local.subsec_nanosecond() / 1_000_000);
    delta_ms.clamp(0, i64::from(FOLDER_WINDOW_SECONDS) * 1_000 - 1)
}

/// Deterministic `talents/screen_text.md` rendering for a folder activity record.
///
/// Emits column-0 timed headings (`## MM:SS · <folder_name>`) so the screen_text
/// indexer and `screen_text_with_clock` resolve each group to wall-clock time,
/// and escapes/redacts every bullet via `append_screen_text_line` and
/// `redact_inline`.
#[must_use]
pub fn render_folder_activity_markdown(activity: &StoredFolderActivity) -> String {
    if activity.events.is_empty() {
        return String::new();
    }
    let mut events = activity.events.clone();
    events.sort_by(|a, b| {
        a.offset_ms
            .cmp(&b.offset_ms)
            .then_with(|| a.at_ms.cmp(&b.at_ms))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.key.cmp(&b.key))
    });
    let safe_folder = sanitize_heading_label(&activity.folder_name);
    let mut out = String::new();
    let mut current_minute: Option<String> = None;
    for ev in &events {
        let minute = screen_text_minute(ev.offset_ms.max(0));
        if current_minute.as_deref() != Some(minute.as_str()) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("## ");
            out.push_str(&minute);
            out.push_str(" · Folder: ");
            out.push_str(&safe_folder);
            out.push('\n');
            current_minute = Some(minute);
        }
        let line = format_event_line(ev);
        let redacted = redact_inline(&line);
        append_screen_text_line(&mut out, &redacted);
    }
    out
}

/// Combine folder activity Markdown with screen OCR Markdown for a segment that
/// holds both folder events and screenshots.
#[must_use]
pub fn merge_folder_activity_with_screen_text(
    activity: &StoredFolderActivity,
    ocr_markdown: &str,
) -> String {
    let folder_md = render_folder_activity_markdown(activity);
    let ocr = ocr_markdown.trim();
    match (folder_md.is_empty(), ocr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => folder_md,
        (true, false) => format!("{ocr}\n"),
        (false, false) => format!("{folder_md}\n{ocr}\n"),
    }
}

/// Extract non-folder sections (for example OCR blocks written by `describe`)
/// from an existing `talents/screen_text.md` file.
#[must_use]
pub fn extract_non_folder_screen_text(existing: &str) -> String {
    let mut out = String::new();
    let mut in_folder_section = false;
    for line in existing.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.starts_with("## ") {
            in_folder_section = trimmed.contains(" · Folder: ");
        }
        if !in_folder_section {
            out.push_str(line);
        }
    }
    out.trim().to_owned()
}

fn sanitize_heading_label(label: &str) -> String {
    label
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

fn format_event_line(ev: &StoredFolderEvent) -> String {
    let mut parts = Vec::new();
    let verb = ev.event.display_verb();
    match (ev.event, ev.previous_path.as_deref()) {
        (FolderEventKind::Renamed, Some(prev)) => {
            parts.push(format!("{verb} {prev} -> {}", ev.path));
        }
        _ => {
            parts.push(format!("{verb} {}", ev.path));
        }
    }
    let mut details = Vec::new();
    if let Some(size) = ev.size {
        details.push(human_size(size));
    }
    if let Some(kind) = ev.kind.as_deref() {
        details.push(kind.to_owned());
    } else if let Some(ft) = ev.file_type.as_deref() {
        details.push(ft.to_owned());
    }
    if !details.is_empty() {
        parts.push(format!("({})", details.join(", ")));
    }
    if let Some(origin) = ev.origin_url.as_deref() {
        parts.push(format!("from {origin}"));
    }
    if let Some(app) = ev.downloaded_by.as_deref().or(ev.source_app.as_deref()) {
        parts.push(format!("via {app}"));
    }
    parts.join(" ")
}

fn human_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    if bytes >= GIB {
        format!("{:.1} GB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{} KB", (bytes + KIB / 2) / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
#[path = "folder_tests.rs"]
mod tests;
