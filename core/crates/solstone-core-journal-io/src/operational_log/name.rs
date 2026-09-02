// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical `oplog--` filename grammar, slug, tag, and ordering.

use std::cmp::Ordering;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, FixedOffset, Utc};
use sha2::{Digest, Sha256};

use crate::name_admission::check_portable_component;

const PREFIX: &str = "oplog--";
const PREFIX_BYTES: &[u8] = b"oplog--";
const MAX_NAME_BYTES: usize = 220;
const MAX_SLUG_BYTES: usize = 40;
const TAG_HEX_LEN: usize = 32;
const FILE_ID_HEX_LEN: usize = 32;
const OPENED_FIELD_LEN: usize = 23;

/// Leaf format suffix.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OplogFormat {
    /// `.log`
    Log,
    /// `.jsonl`
    Jsonl,
}

impl OplogFormat {
    fn from_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "log" => Some(Self::Log),
            "jsonl" => Some(Self::Jsonl),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Jsonl => "jsonl",
        }
    }
}

/// Display slug plus identity tag for one original field.
///
/// Equality and hashing use the tag only. The slug is display text.
#[derive(Clone, Debug)]
pub struct OplogIdentity {
    display_slug: String,
    identity_tag: String,
}

impl OplogIdentity {
    fn new(display_slug: String, identity_tag: String) -> Self {
        Self {
            display_slug,
            identity_tag,
        }
    }

    /// Display slug stored in the filename.
    pub fn display_slug(&self) -> &str {
        &self.display_slug
    }

    /// Identity tag (first 128 bits of SHA-256 of the original).
    pub fn identity_tag(&self) -> &str {
        &self.identity_tag
    }
}

impl PartialEq for OplogIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.identity_tag == other.identity_tag
    }
}

impl Eq for OplogIdentity {}

impl Hash for OplogIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.identity_tag.hash(state);
    }
}

/// Parsed canonical operational-log leaf.
#[derive(Clone, Debug)]
pub struct OplogName {
    source: OplogIdentity,
    opened_utc: String,
    file_id: String,
    run: OplogIdentity,
    format: OplogFormat,
}

impl OplogName {
    /// Source identity (tag) and display slug.
    pub fn source(&self) -> &OplogIdentity {
        &self.source
    }

    /// Fixed-width UTC opened field.
    pub fn opened_utc(&self) -> &str {
        &self.opened_utc
    }

    /// 128-bit file id as lowercase hex.
    pub fn file_id(&self) -> &str {
        &self.file_id
    }

    /// Run identity (tag) and display slug.
    pub fn run(&self) -> &OplogIdentity {
        &self.run
    }

    /// Leaf format.
    pub fn format(&self) -> OplogFormat {
        self.format
    }
}

impl PartialEq for OplogName {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for OplogName {}

impl PartialOrd for OplogName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OplogName {
    fn cmp(&self, other: &Self) -> Ordering {
        self.opened_utc
            .cmp(&other.opened_utc)
            .then_with(|| self.file_id.cmp(&other.file_id))
            .then_with(|| format_oplog_name(self).cmp(&format_oplog_name(other)))
    }
}

/// Closed failure while classifying an `oplog--` candidate.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OplogNameError {
    class: OplogNameClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogNameClass {
    InvalidEncoding,
    TooLong,
    MalformedSeparator,
    MalformedSourceCompound,
    MalformedSourceSlug,
    MalformedSourceTag,
    MalformedUtc,
    MalformedFileId,
    MalformedRunCompound,
    MalformedRunSlug,
    MalformedRunTag,
    MalformedSuffix,
    UnrecognizedSuffix,
}

impl OplogNameClass {
    const fn token(self) -> &'static str {
        match self {
            Self::InvalidEncoding => "oplog_name_invalid_encoding",
            Self::TooLong => "oplog_name_too_long",
            Self::MalformedSeparator => "oplog_name_malformed_separator",
            Self::MalformedSourceCompound => "oplog_name_malformed_source_compound",
            Self::MalformedSourceSlug => "oplog_name_malformed_source_slug",
            Self::MalformedSourceTag => "oplog_name_malformed_source_tag",
            Self::MalformedUtc => "oplog_name_malformed_utc",
            Self::MalformedFileId => "oplog_name_malformed_file_id",
            Self::MalformedRunCompound => "oplog_name_malformed_run_compound",
            Self::MalformedRunSlug => "oplog_name_malformed_run_slug",
            Self::MalformedRunTag => "oplog_name_malformed_run_tag",
            Self::MalformedSuffix => "oplog_name_malformed_suffix",
            Self::UnrecognizedSuffix => "oplog_name_unrecognized_suffix",
        }
    }
}

impl OplogNameError {
    const fn new(class: OplogNameClass) -> Self {
        Self { class }
    }

    fn token(self) -> &'static str {
        self.class.token()
    }
}

impl fmt::Display for OplogNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

impl fmt::Debug for OplogNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for OplogNameError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Native-name classification: unrelated leaves are not errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OplogNameClassification {
    /// The native name does not start with ASCII `oplog--`.
    Unrelated,
    /// The native name starts with `oplog--` and was parsed or refused.
    Candidate(Result<OplogName, OplogNameError>),
}

/// Local day key and UTC opened field from one instant.
pub fn derive_day_key_and_opened_field(instant: DateTime<FixedOffset>) -> (String, String) {
    let day = instant.format("%Y%m%d").to_string();
    let opened = format!(
        "{}Z",
        instant.with_timezone(&Utc).format("%Y%m%dT%H%M%S%.6f")
    );
    (day, opened)
}

/// Assemble the canonical leaf spelling.
pub fn format_oplog_name(name: &OplogName) -> String {
    let assembled = format!(
        "oplog--{}~{}--{}--{}--{}~{}.{}",
        name.source.display_slug,
        name.source.identity_tag,
        name.opened_utc,
        name.file_id,
        name.run.display_slug,
        name.run.identity_tag,
        name.format.as_str()
    );
    debug_assert!(assembled.len() <= MAX_NAME_BYTES);
    debug_assert!(check_portable_component(&assembled).is_ok());
    assembled
}

/// Classify a native directory leaf without lossy UTF-8 conversion.
pub fn classify_oplog_name(leaf: &OsStr) -> OplogNameClassification {
    if !native_has_oplog_prefix(leaf) {
        return OplogNameClassification::Unrelated;
    }
    let Some(text) = os_str_strict_ascii_utf8(leaf) else {
        return OplogNameClassification::Candidate(Err(OplogNameError::new(
            OplogNameClass::InvalidEncoding,
        )));
    };
    OplogNameClassification::Candidate(parse_decoded(text))
}

pub(super) fn slug_and_tag(original: &str, fallback: &str) -> (String, String) {
    (slug_field(original, fallback), tag_hex(original.as_bytes()))
}

pub(super) fn original_is_admissible(original: &str) -> bool {
    !original.is_empty() && !original.chars().any(char::is_control)
}

pub(super) fn file_id_hex(bytes: &[u8; 16]) -> String {
    hex_lower(bytes)
}

pub(super) fn oplog_name_from_parts(
    source_original: &str,
    run_original: &str,
    opened_utc: String,
    file_id: String,
    format: OplogFormat,
) -> OplogName {
    let (source_slug, source_tag) = slug_and_tag(source_original, "source");
    let (run_slug, run_tag) = slug_and_tag(run_original, "run");
    OplogName {
        source: OplogIdentity::new(source_slug, source_tag),
        opened_utc,
        file_id,
        run: OplogIdentity::new(run_slug, run_tag),
        format,
    }
}

fn native_has_oplog_prefix(leaf: &OsStr) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        leaf.as_bytes().starts_with(PREFIX_BYTES)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut wide = leaf.encode_wide();
        PREFIX_BYTES
            .iter()
            .all(|byte| wide.next() == Some(u16::from(*byte)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        leaf.to_str()
            .is_some_and(|text| text.as_bytes().starts_with(PREFIX_BYTES))
    }
}

fn os_str_strict_ascii_utf8(leaf: &OsStr) -> Option<&str> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let text = std::str::from_utf8(leaf.as_bytes()).ok()?;
        text.is_ascii().then_some(text)
    }
    #[cfg(windows)]
    {
        let text = leaf.to_str()?;
        text.is_ascii().then_some(text)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let text = leaf.to_str()?;
        text.is_ascii().then_some(text)
    }
}

fn parse_decoded(text: &str) -> Result<OplogName, OplogNameError> {
    if text.len() > MAX_NAME_BYTES {
        return Err(OplogNameError::new(OplogNameClass::TooLong));
    }
    let fields: Vec<&str> = text.split("--").collect();
    if fields.len() != 5 || fields[0] != "oplog" {
        return Err(OplogNameError::new(OplogNameClass::MalformedSeparator));
    }
    let source = parse_compound(
        fields[1],
        OplogNameClass::MalformedSourceCompound,
        OplogNameClass::MalformedSourceSlug,
        OplogNameClass::MalformedSourceTag,
    )?;
    if !is_opened_field(fields[2]) {
        return Err(OplogNameError::new(OplogNameClass::MalformedUtc));
    }
    if !is_hex_field(fields[3], FILE_ID_HEX_LEN) {
        return Err(OplogNameError::new(OplogNameClass::MalformedFileId));
    }
    let Some((run_compound, suffix)) = fields[4].rsplit_once('.') else {
        return Err(OplogNameError::new(OplogNameClass::MalformedSuffix));
    };
    let Some(format) = OplogFormat::from_suffix(suffix) else {
        return Err(OplogNameError::new(OplogNameClass::UnrecognizedSuffix));
    };
    let run = parse_compound(
        run_compound,
        OplogNameClass::MalformedRunCompound,
        OplogNameClass::MalformedRunSlug,
        OplogNameClass::MalformedRunTag,
    )?;
    Ok(OplogName {
        source,
        opened_utc: fields[2].to_owned(),
        file_id: fields[3].to_owned(),
        run,
        format,
    })
}

fn parse_compound(
    field: &str,
    compound: OplogNameClass,
    slug_class: OplogNameClass,
    tag_class: OplogNameClass,
) -> Result<OplogIdentity, OplogNameError> {
    let Some((slug, tag)) = field.split_once('~') else {
        return Err(OplogNameError::new(compound));
    };
    if tag.contains('~') {
        return Err(OplogNameError::new(compound));
    }
    if !is_valid_slug(slug) {
        return Err(OplogNameError::new(slug_class));
    }
    if !is_hex_field(tag, TAG_HEX_LEN) {
        return Err(OplogNameError::new(tag_class));
    }
    Ok(OplogIdentity::new(slug.to_owned(), tag.to_owned()))
}

fn is_valid_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_SLUG_BYTES
        && bytes[0] != b'-'
        && bytes[bytes.len() - 1] != b'-'
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && !slug.contains("--")
}

fn is_opened_field(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == OPENED_FIELD_LEN
        && bytes[8] == b'T'
        && bytes[15] == b'.'
        && bytes[22] == b'Z'
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[16..22].iter().all(u8::is_ascii_digit)
}

fn is_hex_field(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn slug_field(original: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut in_other_run = false;
    for character in original.chars() {
        if let Some(mapped) = map_ascii_keep(character) {
            in_other_run = false;
            out.push(mapped);
        } else if !in_other_run {
            out.push('-');
            in_other_run = true;
        }
    }
    let trimmed = out.trim_matches('-');
    let mut slug = if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    };
    if slug.len() > MAX_SLUG_BYTES {
        slug.truncate(MAX_SLUG_BYTES);
        while slug.ends_with('-') {
            slug.pop();
        }
    }
    slug
}

fn map_ascii_keep(character: char) -> Option<char> {
    if !character.is_ascii() {
        return None;
    }
    if character.is_ascii_uppercase() {
        Some(character.to_ascii_lowercase())
    } else if character.is_ascii_lowercase() || character.is_ascii_digit() {
        Some(character)
    } else {
        None
    }
}

fn tag_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_lower(&digest[..16])
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

const _: () = assert!(PREFIX.len() == PREFIX_BYTES.len());

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::error::Error;
    use std::ffi::OsStr;

    use chrono::DateTime;

    use super::*;

    const REFERENCE: &str = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log";

    fn expect_token(error: OplogNameError, token: &str) {
        assert_eq!(error.to_string(), token);
        assert_eq!(format!("{error:?}"), token);
        assert!(error.source().is_none());
    }

    fn classify_ok(leaf: &str) -> OplogName {
        match classify_oplog_name(OsStr::new(leaf)) {
            OplogNameClassification::Candidate(Ok(name)) => name,
            other => panic!("expected candidate ok, got {other:?}"),
        }
    }

    fn classify_err(leaf: &str) -> OplogNameError {
        match classify_oplog_name(OsStr::new(leaf)) {
            OplogNameClassification::Candidate(Err(error)) => error,
            other => panic!("expected candidate err, got {other:?}"),
        }
    }

    #[test]
    fn reference_vector_round_trips_typed_identity_and_log_format() {
        let instant = DateTime::parse_from_rfc3339("2026-09-01T16:42:33.381904Z").unwrap();
        let (day, opened) = derive_day_key_and_opened_field(instant);
        assert_eq!(day, "20260901");
        assert_eq!(opened, "20260901T164233.381904Z");
        assert_eq!(opened.len(), 23);

        let (source_slug, source_tag) = slug_and_tag("cortex", "source");
        let (run_slug, run_tag) = slug_and_tag("daily-think", "run");
        assert_eq!(source_slug, "cortex");
        assert_eq!(source_tag, "1ee11af4ed5d63caf142a30a96ba124b");
        assert_eq!(run_slug, "daily-think");
        assert_eq!(run_tag, "7df259e6285645a5f9ea769caa484e07");

        let name = oplog_name_from_parts(
            "cortex",
            "daily-think",
            opened,
            "8f03cabead7e441d83f6c92b2d89a021".to_owned(),
            OplogFormat::Log,
        );
        assert_eq!(format_oplog_name(&name), REFERENCE);
        let parsed = classify_ok(REFERENCE);
        assert_eq!(parsed.source().display_slug(), "cortex");
        assert_eq!(
            parsed.source().identity_tag(),
            "1ee11af4ed5d63caf142a30a96ba124b"
        );
        assert_eq!(parsed.run().display_slug(), "daily-think");
        assert_eq!(
            parsed.run().identity_tag(),
            "7df259e6285645a5f9ea769caa484e07"
        );
        assert_eq!(parsed.format(), OplogFormat::Log);
        assert_eq!(format_oplog_name(&parsed), REFERENCE);
    }

    #[test]
    fn fixture_table_covers_slug_fallback_cap_formats_and_malformed_shapes() {
        let fixture = include_str!("../../tests/fixtures/oplog-name-grammar.md");
        let block = fixture
            .split("```")
            .nth(1)
            .expect("fixture has a mapping code block");
        for line in block.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("original|") {
                continue;
            }
            let mut parts = line.split('|');
            let original = parts.next().expect("original");
            let fallback = parts.next().expect("fallback");
            let expected = parts.next().expect("expected slug");
            assert_eq!(slug_and_tag(original, fallback).0, expected, "{original:?}");
        }

        let long = "A".repeat(80);
        let (slug, _) = slug_and_tag(&long, "source");
        assert_eq!(slug.len(), 40);
        assert!(!slug.ends_with('-'));

        let (left, left_tag) = slug_and_tag("Cortex", "source");
        let (right, right_tag) = slug_and_tag("cortex", "source");
        assert_eq!(left, right);
        assert_ne!(left_tag, right_tag);

        let jsonl = REFERENCE.replacen(".log", ".jsonl", 1);
        assert_eq!(classify_ok(&jsonl).format(), OplogFormat::Jsonl);
    }

    #[test]
    fn non_ascii_letters_collapse_independently_of_distinct_tags() {
        for original in ["İ", "é", "e\u{301}"] {
            let (slug, tag) = slug_and_tag(original, "source");
            assert!(slug.bytes().all(|byte| byte.is_ascii()), "{original:?}");
            assert_eq!(tag, tag_hex(original.as_bytes()));
            assert_ne!(tag, tag_hex(b"i"));
        }
    }

    #[test]
    fn same_identity_tag_groups_despite_distinct_display_slugs() {
        let mut first = classify_ok(REFERENCE);
        first.source.display_slug = "other-display".to_owned();
        let second = classify_ok(REFERENCE);
        assert_eq!(first.source(), second.source());
        let mut set = HashSet::new();
        set.insert(first.source().clone());
        set.insert(second.source().clone());
        assert_eq!(set.len(), 1);
        assert_ne!(
            first.source().display_slug(),
            second.source().display_slug()
        );
    }

    #[test]
    fn unrelated_and_every_malformed_token_round_trip() {
        assert_eq!(
            classify_oplog_name(OsStr::new("stream.updated")),
            OplogNameClassification::Unrelated
        );
        assert_eq!(
            classify_oplog_name(OsStr::new(".oplog-namespace.lock")),
            OplogNameClassification::Unrelated
        );

        let tag = "1ee11af4ed5d63caf142a30a96ba124b";
        let run = "daily-think~7df259e6285645a5f9ea769caa484e07";
        let utc = "20260901T164233.381904Z";
        let id = "8f03cabead7e441d83f6c92b2d89a021";
        let cases = [
            (
                format!(
                    "oplog--{}~{tag}--{utc}--{id}--{}~7df259e6285645a5f9ea769caa484e07.jsonlx",
                    "a".repeat(40),
                    "b".repeat(40)
                ),
                "oplog_name_too_long",
            ),
            (
                format!("oplog--cortex~{tag}-{utc}--{id}--{run}.log"),
                "oplog_name_malformed_separator",
            ),
            (
                format!("oplog--cortex{tag}--{utc}--{id}--{run}.log"),
                "oplog_name_malformed_source_compound",
            ),
            (
                format!("oplog--cortex~{tag}~extra--{utc}--{id}--{run}.log"),
                "oplog_name_malformed_source_compound",
            ),
            (
                format!("oplog--~{tag}--{utc}--{id}--{run}.log"),
                "oplog_name_malformed_source_slug",
            ),
            (
                format!("oplog--{}~{tag}--{utc}--{id}--{run}.log", "a".repeat(41)),
                "oplog_name_malformed_source_slug",
            ),
            (
                format!("oplog--Cortex~{tag}--{utc}--{id}--{run}.log"),
                "oplog_name_malformed_source_slug",
            ),
            (
                format!(
                    "oplog--cortex~{}--{utc}--{id}--{run}.log",
                    tag.to_uppercase()
                ),
                "oplog_name_malformed_source_tag",
            ),
            (
                format!("oplog--cortex~abcd--{utc}--{id}--{run}.log"),
                "oplog_name_malformed_source_tag",
            ),
            (
                format!("oplog--cortex~{tag}--2026-09-01T16:42:33Z--{id}--{run}.log"),
                "oplog_name_malformed_utc",
            ),
            (
                format!("oplog--cortex~{tag}--20260901T164233Z--{id}--{run}.log"),
                "oplog_name_malformed_utc",
            ),
            (
                format!("oplog--cortex~{tag}--{utc}--ABCD--{run}.log"),
                "oplog_name_malformed_file_id",
            ),
            (
                format!("oplog--cortex~{tag}--{utc}--{}--{run}.log", &id[..16]),
                "oplog_name_malformed_file_id",
            ),
            (
                format!("oplog--cortex~{tag}--{utc}--{id}{}--{run}.log", "aa"),
                "oplog_name_malformed_file_id",
            ),
            (
                format!(
                    "oplog--cortex~{tag}--{utc}--{id}--daily-think7df259e6285645a5f9ea769caa484e07.log"
                ),
                "oplog_name_malformed_run_compound",
            ),
            (
                format!("oplog--cortex~{tag}--{utc}--{id}--~7df259e6285645a5f9ea769caa484e07.log"),
                "oplog_name_malformed_run_slug",
            ),
            (
                format!("oplog--cortex~{tag}--{utc}--{id}--daily-think~ABCD.log"),
                "oplog_name_malformed_run_tag",
            ),
            (
                format!("oplog--cortex~{tag}--{utc}--{id}--{run}"),
                "oplog_name_malformed_suffix",
            ),
            (
                format!("oplog--cortex~{tag}--{utc}--{id}--{run}.txt"),
                "oplog_name_unrecognized_suffix",
            ),
        ];
        for (leaf, token) in cases {
            expect_token(classify_err(&leaf), token);
        }
    }

    #[test]
    fn exact_220_byte_name_is_valid_and_overlong_is_rejected() {
        let source_slug = "a".repeat(40);
        let run_slug = "b".repeat(40);
        let tag = "1ee11af4ed5d63caf142a30a96ba124b";
        let run_tag = "7df259e6285645a5f9ea769caa484e07";
        let utc = "20260901T164233.381904Z";
        let id = "8f03cabead7e441d83f6c92b2d89a021";
        let max = format!("oplog--{source_slug}~{tag}--{utc}--{id}--{run_slug}~{run_tag}.jsonl");
        assert_eq!(max.len(), 220);
        classify_ok(&max);
        let overlong = format!("{max}x");
        expect_token(classify_err(&overlong), "oplog_name_too_long");
    }

    #[test]
    fn equal_source_and_opened_utc_with_reused_file_id_orders_by_filename_bytes() {
        let left = classify_ok(REFERENCE);
        let mut right = left.clone();
        right.format = OplogFormat::Jsonl;
        let mut names = [right.clone(), left.clone()];
        names.sort();
        assert_eq!(format_oplog_name(&names[0]), format_oplog_name(&right));
        assert_eq!(format_oplog_name(&names[1]), format_oplog_name(&left));
        assert!(right < left);
    }

    #[test]
    fn midnight_offset_instant_shares_one_derivation() {
        let instant = DateTime::parse_from_rfc3339("2026-09-01T23:30:00-05:00").unwrap();
        let (day, opened) = derive_day_key_and_opened_field(instant);
        assert_eq!(day, "20260901");
        assert_eq!(opened, "20260902T043000.000000Z");
    }

    #[test]
    fn this_module_is_the_only_oplog_name_formatter() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        fn walk(dir: &std::path::Path, offenders: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().and_then(OsStr::to_str) == Some("operational_log") {
                        continue;
                    }
                    walk(&path, offenders);
                    continue;
                }
                if path.extension().and_then(OsStr::to_str) != Some("rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains("oplog--") {
                    offenders.push(path.display().to_string());
                }
            }
        }
        walk(&root, &mut offenders);
        assert!(
            offenders.is_empty(),
            "only operational_log may mention oplog-- names: {offenders:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_invalid_native_bytes_are_unrelated_unless_prefixed() {
        use std::os::unix::ffi::OsStrExt;
        let unrelated = OsStr::from_bytes(b"not-oplog-\xff");
        assert_eq!(
            classify_oplog_name(unrelated),
            OplogNameClassification::Unrelated
        );
        let prefixed = OsStr::from_bytes(b"oplog--\xff");
        match classify_oplog_name(prefixed) {
            OplogNameClassification::Candidate(Err(error)) => {
                expect_token(error, "oplog_name_invalid_encoding");
            }
            other => panic!("expected malformed encoding, got {other:?}"),
        }
        let non_ascii = OsStr::from_bytes("oplog--caf\u{e9}".as_bytes());
        match classify_oplog_name(non_ascii) {
            OplogNameClassification::Candidate(Err(error)) => {
                expect_token(error, "oplog_name_invalid_encoding");
            }
            other => panic!("expected malformed encoding, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_unpaired_surrogates_are_unrelated_unless_prefixed() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        let unrelated = OsString::from_wide(&[b'x' as u16, 0xd800]);
        assert_eq!(
            classify_oplog_name(&unrelated),
            OplogNameClassification::Unrelated
        );
        let mut prefixed = Vec::from(b"oplog--".map(u16::from));
        prefixed.push(0xd800);
        let prefixed = OsString::from_wide(&prefixed);
        match classify_oplog_name(&prefixed) {
            OplogNameClassification::Candidate(Err(error)) => {
                expect_token(error, "oplog_name_invalid_encoding");
            }
            other => panic!("expected malformed encoding, got {other:?}"),
        }
    }
}
