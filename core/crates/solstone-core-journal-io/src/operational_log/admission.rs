// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded ASCII admission record at the start of one oplog leaf.

use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;

use super::name::{
    OplogFormat, OplogName, OplogNameClassification, classify_oplog_name, format_oplog_name,
    is_opened_field,
};

/// Inclusive maximum header size, terminator included.
pub const OPLOG_ADMISSION_MAX_BYTES: usize = 2048;

const VERSION: &str = "solstone.oplog.admission.v1";
const RECORD_KEYS: [&str; 8] = [
    "kind",
    "source-slug",
    "source-tag",
    "run-slug",
    "run-tag",
    "opened-utc",
    "format",
    "file-ids",
];

/// Parsed admission header bound to one canonical leaf of an eight-candidate set.
///
/// ```compile_fail,E0599
/// fn old_accessor(record: &solstone_core_journal_io::operational_log::OplogAdmissionRecord) {
///     let _ = record.name();
/// }
/// ```
///
/// ```compile_fail,E0425
/// let _ = solstone_core_journal_io::operational_log::validate_oplog_admission_set;
/// ```
pub struct OplogAdmissionRecord {
    leaf: OplogName,
    candidates: [OplogName; 8],
    header_len: usize,
}

impl OplogAdmissionRecord {
    /// Canonical name of the observed on-disk leaf.
    pub fn leaf(&self) -> &OplogName {
        &self.leaf
    }

    /// The eight reconstructed collision candidates, record order.
    pub fn candidates(&self) -> &[OplogName; 8] {
        &self.candidates
    }

    /// Byte offset where payload begins (header including terminator).
    pub fn header_len(&self) -> usize {
        self.header_len
    }
}

impl fmt::Debug for OplogAdmissionRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OplogAdmissionRecord")
            .field("leaf", &self.leaf)
            .field("candidates", &self.candidates)
            .field("header_len", &self.header_len)
            .finish()
    }
}

/// Bounded failure while validating an admission record.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OplogAdmissionError {
    class: OplogAdmissionClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogAdmissionClass {
    Missing,
    Overlong,
    InvalidUtf8,
    Crlf,
    TrailingBytes,
    Malformed,
    ExtraWhitespace,
    DuplicateFieldKey,
    WrongKeySet,
    WrongKeyOrder,
    WrongVersion,
    WrongCandidateCardinality,
    InvalidOpenedUtc,
    NonCanonical,
    UnlistedLeaf,
    NonUniqueFileId,
    IncoherentCoordinates,
}

impl OplogAdmissionClass {
    const fn token(self) -> &'static str {
        match self {
            Self::Missing => "oplog_admission_missing",
            Self::Overlong => "oplog_admission_overlong",
            Self::InvalidUtf8 => "oplog_admission_invalid_utf8",
            Self::Crlf => "oplog_admission_crlf",
            Self::TrailingBytes => "oplog_admission_trailing_bytes",
            Self::Malformed => "oplog_admission_malformed",
            Self::ExtraWhitespace => "oplog_admission_extra_whitespace",
            Self::DuplicateFieldKey => "oplog_admission_duplicate_field_key",
            Self::WrongKeySet => "oplog_admission_wrong_key_set",
            Self::WrongKeyOrder => "oplog_admission_wrong_key_order",
            Self::WrongVersion => "oplog_admission_wrong_version",
            Self::WrongCandidateCardinality => "oplog_admission_wrong_candidate_cardinality",
            Self::InvalidOpenedUtc => "oplog_admission_invalid_opened_utc",
            Self::NonCanonical => "oplog_admission_non_canonical",
            Self::UnlistedLeaf => "oplog_admission_unlisted_leaf",
            Self::NonUniqueFileId => "oplog_admission_non_unique_file_id",
            Self::IncoherentCoordinates => "oplog_admission_incoherent_coordinates",
        }
    }
}

impl OplogAdmissionError {
    const fn new(class: OplogAdmissionClass) -> Self {
        Self { class }
    }

    fn token(self) -> &'static str {
        self.class.token()
    }
}

impl fmt::Display for OplogAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

impl fmt::Debug for OplogAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for OplogAdmissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

fn missing() -> OplogAdmissionError {
    OplogAdmissionError::new(OplogAdmissionClass::Missing)
}

fn malformed() -> OplogAdmissionError {
    OplogAdmissionError::new(OplogAdmissionClass::Malformed)
}

/// Encode the canonical admission header for eight pre-drawn candidates.
///
/// Compact JSON, no extra whitespace, one trailing LF. Maximal slug/tag/id
/// bounds stay under [`OPLOG_ADMISSION_MAX_BYTES`] (592 bytes at the cap).
pub(super) fn encode_oplog_admission(names: &[OplogName; 8]) -> Vec<u8> {
    let first = &names[0];
    debug_assert!(names.iter().all(|name| {
        name.source().display_slug() == first.source().display_slug()
            && name.source().identity_tag() == first.source().identity_tag()
            && name.run().display_slug() == first.run().display_slug()
            && name.run().identity_tag() == first.run().identity_tag()
            && name.opened_utc() == first.opened_utc()
            && name.format() == first.format()
    }));
    let format = match first.format() {
        OplogFormat::Log => "log",
        OplogFormat::Jsonl => "jsonl",
    };
    let mut header = format!(
        "{{\"kind\":\"{VERSION}\",\"source-slug\":\"{}\",\"source-tag\":\"{}\",\"run-slug\":\"{}\",\"run-tag\":\"{}\",\"opened-utc\":\"{}\",\"format\":\"{format}\",\"file-ids\":[",
        first.source().display_slug(),
        first.source().identity_tag(),
        first.run().display_slug(),
        first.run().identity_tag(),
        first.opened_utc(),
    );
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            header.push(',');
        }
        header.push('"');
        header.push_str(name.file_id());
        header.push('"');
    }
    header.push_str("]}\n");
    debug_assert!(header.len() <= OPLOG_ADMISSION_MAX_BYTES);
    header.into_bytes()
}

/// Validate one admission header at the start of `bytes` for on-disk `leaf`.
///
/// Proves the full eight-candidate set from this one observed leaf.
pub fn validate_oplog_admission(
    leaf: &OsStr,
    bytes: &[u8],
) -> Result<OplogAdmissionRecord, OplogAdmissionError> {
    if bytes.is_empty() {
        return Err(missing());
    }
    let search_len = bytes.len().min(OPLOG_ADMISSION_MAX_BYTES);
    let Some(newline) = bytes[..search_len].iter().position(|byte| *byte == b'\n') else {
        return if bytes.len() >= OPLOG_ADMISSION_MAX_BYTES {
            Err(OplogAdmissionError::new(OplogAdmissionClass::Overlong))
        } else {
            Err(missing())
        };
    };
    let header = &bytes[..=newline];
    let text = std::str::from_utf8(header)
        .map_err(|_| OplogAdmissionError::new(OplogAdmissionClass::InvalidUtf8))?;
    if header.contains(&b'\r') {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::Crlf));
    }
    let object_text = &text[..text.len() - 1];
    if !object_text.starts_with('{') {
        return Err(malformed());
    }
    if !object_text.ends_with('}') {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::TrailingBytes));
    }
    if has_extra_whitespace(object_text) {
        return Err(OplogAdmissionError::new(
            OplogAdmissionClass::ExtraWhitespace,
        ));
    }
    let keys = scan_top_level_keys(object_text)?;
    let mut seen = HashSet::new();
    for key in &keys {
        if !seen.insert(*key) {
            return Err(OplogAdmissionError::new(
                OplogAdmissionClass::DuplicateFieldKey,
            ));
        }
    }
    if key_set_mismatch(&keys) {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::WrongKeySet));
    }
    if keys.as_slice() != RECORD_KEYS {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::WrongKeyOrder));
    }
    let value: serde_json::Value = serde_json::from_str(object_text).map_err(|_| malformed())?;
    let object = value.as_object().ok_or_else(malformed)?;
    let kind = object
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(malformed)?;
    if kind != VERSION {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::WrongVersion));
    }
    let file_ids_value = object.get("file-ids").ok_or_else(malformed)?;
    let Some(file_ids) = file_ids_value.as_array() else {
        return Err(OplogAdmissionError::new(
            OplogAdmissionClass::WrongCandidateCardinality,
        ));
    };
    if file_ids.len() != 8 {
        return Err(OplogAdmissionError::new(
            OplogAdmissionClass::WrongCandidateCardinality,
        ));
    }
    let mut file_id_strings = Vec::with_capacity(8);
    for entry in file_ids {
        let Some(text) = entry.as_str() else {
            return Err(malformed());
        };
        file_id_strings.push(text);
    }
    let source_slug = object
        .get("source-slug")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(malformed)?;
    let source_tag = object
        .get("source-tag")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(malformed)?;
    let run_slug = object
        .get("run-slug")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(malformed)?;
    let run_tag = object
        .get("run-tag")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(malformed)?;
    let opened_utc = object
        .get("opened-utc")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(malformed)?;
    let format = object
        .get("format")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(malformed)?;
    if !is_opened_field(opened_utc) {
        return Err(OplogAdmissionError::new(
            OplogAdmissionClass::InvalidOpenedUtc,
        ));
    }
    let mut candidates_vec = Vec::with_capacity(8);
    for file_id in &file_id_strings {
        candidates_vec.push(reconstruct_candidate(
            source_slug,
            source_tag,
            opened_utc,
            file_id,
            run_slug,
            run_tag,
            format,
        )?);
    }
    let candidates: [OplogName; 8] = candidates_vec
        .try_into()
        .expect("exactly eight reconstructed candidates");
    let encoded = encode_oplog_admission(&candidates);
    if encoded.as_slice() != header {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::NonCanonical));
    }
    let mut unique = HashSet::new();
    for candidate in &candidates {
        if !unique.insert(candidate.file_id()) {
            return Err(OplogAdmissionError::new(
                OplogAdmissionClass::NonUniqueFileId,
            ));
        }
    }
    let observed = match classify_oplog_name(leaf) {
        OplogNameClassification::Candidate(Ok(name)) => name,
        _ => {
            return Err(OplogAdmissionError::new(OplogAdmissionClass::UnlistedLeaf));
        }
    };
    let observed_formatted = format_oplog_name(&observed);
    if let Some(matched) = candidates
        .iter()
        .find(|candidate| candidate.file_id() == observed.file_id())
    {
        if format_oplog_name(matched) != observed_formatted {
            return Err(OplogAdmissionError::new(
                OplogAdmissionClass::IncoherentCoordinates,
            ));
        }
    } else {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::UnlistedLeaf));
    }
    if !candidates
        .iter()
        .any(|candidate| format_oplog_name(candidate) == observed_formatted)
    {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::UnlistedLeaf));
    }
    Ok(OplogAdmissionRecord {
        leaf: observed,
        candidates,
        header_len: header.len(),
    })
}

fn reconstruct_candidate(
    source_slug: &str,
    source_tag: &str,
    opened_utc: &str,
    file_id: &str,
    run_slug: &str,
    run_tag: &str,
    format: &str,
) -> Result<OplogName, OplogAdmissionError> {
    let assembled = format!(
        "oplog--{source_slug}~{source_tag}--{opened_utc}--{file_id}--{run_slug}~{run_tag}.{format}"
    );
    match classify_oplog_name(OsStr::new(&assembled)) {
        OplogNameClassification::Candidate(Ok(name)) => Ok(name),
        _ => Err(OplogAdmissionError::new(OplogAdmissionClass::UnlistedLeaf)),
    }
}

fn key_set_mismatch(keys: &[&str]) -> bool {
    if keys.len() != RECORD_KEYS.len() {
        return true;
    }
    let expected: HashSet<&str> = RECORD_KEYS.iter().copied().collect();
    let observed: HashSet<&str> = keys.iter().copied().collect();
    observed != expected
}

fn has_extra_whitespace(object_text: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    for byte in object_text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        if byte == b'"' {
            in_string = true;
            continue;
        }
        if byte == b' ' || byte == b'\t' || byte == b'\n' {
            return true;
        }
    }
    false
}

fn scan_top_level_keys(object_text: &str) -> Result<Vec<&str>, OplogAdmissionError> {
    let bytes = object_text.as_bytes();
    let mut index = 1;
    let end = bytes.len() - 1;
    let mut keys = Vec::new();
    if index == end {
        return Ok(keys);
    }
    loop {
        let (key, next) = parse_object_key(object_text, index)?;
        index = next;
        if bytes.get(index) != Some(&b':') {
            return Err(malformed());
        }
        index += 1;
        index = skip_compact_value(bytes, index)?;
        keys.push(key);
        if index == end {
            return Ok(keys);
        }
        if bytes.get(index) != Some(&b',') {
            return Err(malformed());
        }
        index += 1;
        if index == end {
            return Err(malformed());
        }
    }
}

fn parse_object_key(object_text: &str, start: usize) -> Result<(&str, usize), OplogAdmissionError> {
    let bytes = object_text.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return Err(malformed());
    }
    let mut index = start + 1;
    while index < bytes.len() && bytes[index] != b'"' {
        if bytes[index] == b'\\' {
            return Err(malformed());
        }
        index += 1;
    }
    if index >= bytes.len() || bytes[index] != b'"' {
        return Err(malformed());
    }
    Ok((&object_text[start + 1..index], index + 1))
}

fn skip_compact_value(bytes: &[u8], start: usize) -> Result<usize, OplogAdmissionError> {
    match bytes.get(start) {
        Some(b'"') => skip_compact_string(bytes, start),
        Some(b'[') => skip_compact_array(bytes, start),
        Some(b'{') => skip_compact_object(bytes, start),
        Some(b't') => skip_literal(bytes, start, b"true"),
        Some(b'f') => skip_literal(bytes, start, b"false"),
        Some(b'n') => skip_literal(bytes, start, b"null"),
        Some(b'-') | Some(b'0'..=b'9') => skip_number(bytes, start),
        _ => Err(malformed()),
    }
}

fn skip_compact_string(bytes: &[u8], start: usize) -> Result<usize, OplogAdmissionError> {
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index += 1;
                if index >= bytes.len() {
                    return Err(malformed());
                }
                if bytes[index] == b'u' {
                    index += 1;
                    index = index.checked_add(4).ok_or_else(malformed)?;
                    if index > bytes.len() {
                        return Err(malformed());
                    }
                } else {
                    index += 1;
                }
            }
            b'"' => return Ok(index + 1),
            _ => index += 1,
        }
    }
    Err(malformed())
}

fn skip_compact_array(bytes: &[u8], start: usize) -> Result<usize, OplogAdmissionError> {
    let mut index = start + 1;
    if bytes.get(index) == Some(&b']') {
        return Ok(index + 1);
    }
    loop {
        index = skip_compact_value(bytes, index)?;
        match bytes.get(index) {
            Some(b']') => return Ok(index + 1),
            Some(b',') => index += 1,
            _ => return Err(malformed()),
        }
    }
}

fn skip_compact_object(bytes: &[u8], start: usize) -> Result<usize, OplogAdmissionError> {
    let mut index = start + 1;
    if bytes.get(index) == Some(&b'}') {
        return Ok(index + 1);
    }
    loop {
        index = skip_compact_string(bytes, index)?;
        if bytes.get(index) != Some(&b':') {
            return Err(malformed());
        }
        index += 1;
        index = skip_compact_value(bytes, index)?;
        match bytes.get(index) {
            Some(b'}') => return Ok(index + 1),
            Some(b',') => index += 1,
            _ => return Err(malformed()),
        }
    }
}

fn skip_literal(bytes: &[u8], start: usize, literal: &[u8]) -> Result<usize, OplogAdmissionError> {
    if bytes.get(start..start + literal.len()) != Some(literal) {
        return Err(malformed());
    }
    Ok(start + literal.len())
}

fn skip_number(bytes: &[u8], start: usize) -> Result<usize, OplogAdmissionError> {
    let mut index = start;
    if bytes.get(index) == Some(&b'-') {
        index += 1;
    }
    let digits_start = index;
    while matches!(bytes.get(index), Some(b'0'..=b'9')) {
        index += 1;
    }
    if index == digits_start {
        return Err(malformed());
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let frac = index;
        while matches!(bytes.get(index), Some(b'0'..=b'9')) {
            index += 1;
        }
        if index == frac {
            return Err(malformed());
        }
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exp = index;
        while matches!(bytes.get(index), Some(b'0'..=b'9')) {
            index += 1;
        }
        if index == exp {
            return Err(malformed());
        }
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::ffi::OsStr;

    use super::*;
    use crate::operational_log::name::oplog_name_from_parts;

    const FIXTURE: &[u8] = b"{\"kind\":\"solstone.oplog.admission.v1\",\"source-slug\":\"cortex\",\"source-tag\":\"1ee11af4ed5d63caf142a30a96ba124b\",\"run-slug\":\"daily-think\",\"run-tag\":\"7df259e6285645a5f9ea769caa484e07\",\"opened-utc\":\"20260901T164233.381904Z\",\"format\":\"log\",\"file-ids\":[\"8f03cabead7e441d83f6c92b2d89a021\",\"a1b2c3d4e5f60718293a4b5c6d7e8f90\",\"b0c1d2e3f405162738495a6b7c8d9e0f\",\"c1d2e3f405162738495a6b7c8d9e0f10\",\"d2e3f405162738495a6b7c8d9e0f1011\",\"e3f405162738495a6b7c8d9e0f101112\",\"f405162738495a6b7c8d9e0f10111213\",\"05162738495a6b7c8d9e0f1011121314\"]}\n";

    const FILE_IDS: [&str; 8] = [
        "8f03cabead7e441d83f6c92b2d89a021",
        "a1b2c3d4e5f60718293a4b5c6d7e8f90",
        "b0c1d2e3f405162738495a6b7c8d9e0f",
        "c1d2e3f405162738495a6b7c8d9e0f10",
        "d2e3f405162738495a6b7c8d9e0f1011",
        "e3f405162738495a6b7c8d9e0f101112",
        "f405162738495a6b7c8d9e0f10111213",
        "05162738495a6b7c8d9e0f1011121314",
    ];

    fn reference_names() -> [OplogName; 8] {
        FILE_IDS.map(|file_id| {
            oplog_name_from_parts(
                "cortex",
                "daily-think",
                "20260901T164233.381904Z".to_owned(),
                file_id.to_owned(),
                OplogFormat::Log,
            )
        })
    }

    fn fixture_leaf() -> String {
        format_oplog_name(&reference_names()[0])
    }

    fn expect_token(error: OplogAdmissionError, token: &str) {
        assert_eq!(error.to_string(), token);
        assert_eq!(format!("{error:?}"), token);
        assert!(error.source().is_none());
    }

    fn reject(bytes: &[u8], leaf: &str, token: &str) {
        expect_token(
            validate_oplog_admission(OsStr::new(leaf), bytes).unwrap_err(),
            token,
        );
    }

    fn reject_fixture_leaf(bytes: &[u8], token: &str) {
        reject(bytes, &fixture_leaf(), token);
    }

    fn swap_key_pair(object: &str, left: &str, right: &str) -> String {
        let left_pat = format!("\"{left}\":");
        let right_pat = format!("\"{right}\":");
        let left_at = object.find(&left_pat).unwrap();
        let right_at = object.find(&right_pat).unwrap();
        assert!(left_at < right_at);
        let left_end = if left == "file-ids" {
            object.len() - 1
        } else {
            object[left_at + left_pat.len()..]
                .find(",\"")
                .map(|offset| left_at + left_pat.len() + offset)
                .unwrap()
        };
        let right_end = if right == "file-ids" {
            object.len() - 1
        } else {
            object[right_at + right_pat.len()..]
                .find(",\"")
                .map(|offset| right_at + right_pat.len() + offset)
                .unwrap()
        };
        let mut out = String::new();
        out.push_str(&object[..left_at]);
        out.push_str(&object[right_at..right_end]);
        out.push_str(&object[left_end..right_at]);
        out.push_str(&object[left_at..left_end]);
        out.push_str(&object[right_end..]);
        out
    }

    #[test]
    fn encode_matches_the_pinned_fixture_and_round_trips() {
        let names = reference_names();
        let encoded = encode_oplog_admission(&names);
        assert_eq!(encoded, FIXTURE);
        assert!(encoded.len() <= OPLOG_ADMISSION_MAX_BYTES);
        assert!(encoded.ends_with(b"\n"));
        let mut bytes = encoded.clone();
        bytes.extend_from_slice(b"payload-one\npayload-two\n");
        let leaf = fixture_leaf();
        let record = validate_oplog_admission(OsStr::new(&leaf), &bytes).unwrap();
        assert_eq!(record.header_len(), encoded.len());
        assert_eq!(&bytes[record.header_len()..], b"payload-one\npayload-two\n");
        assert_eq!(format_oplog_name(record.leaf()), leaf);
        assert_eq!(record.candidates().len(), 8);
        assert_eq!(record.leaf().file_id(), names[0].file_id());
        assert!(!format!("{record:?}").contains("payload-one"));
    }

    #[test]
    fn maximal_encoded_name_stays_under_the_byte_cap() {
        let names = FILE_IDS.map(|file_id| {
            oplog_name_from_parts(
                &"a".repeat(40),
                &"b".repeat(40),
                "20260901T164233.381904Z".to_owned(),
                file_id.to_owned(),
                OplogFormat::Jsonl,
            )
        });
        let encoded = encode_oplog_admission(&names);
        assert_eq!(encoded.len(), 592);
        assert!(encoded.len() <= OPLOG_ADMISSION_MAX_BYTES);
        let leaf = format_oplog_name(&names[0]);
        assert_eq!(leaf.len(), 220);
        validate_oplog_admission(OsStr::new(&leaf), &encoded).unwrap();
    }

    #[test]
    fn twins_reject_with_distinct_tokens() {
        let leaf = fixture_leaf();
        let object = std::str::from_utf8(&FIXTURE[..FIXTURE.len() - 1]).unwrap();

        reject(b"", &leaf, "oplog_admission_missing");
        reject(
            b"{\"kind\":\"solstone.oplog.admission.v1\"",
            &leaf,
            "oplog_admission_missing",
        );
        reject_fixture_leaf(
            &[b'x'; OPLOG_ADMISSION_MAX_BYTES],
            "oplog_admission_overlong",
        );
        reject_fixture_leaf(
            &[b'x'; OPLOG_ADMISSION_MAX_BYTES + 8],
            "oplog_admission_overlong",
        );

        let mut invalid_utf8 = FIXTURE.to_vec();
        invalid_utf8[2] = 0xff;
        reject_fixture_leaf(&invalid_utf8, "oplog_admission_invalid_utf8");

        let mut crlf = FIXTURE.to_vec();
        crlf.pop();
        crlf.extend_from_slice(b"\r\n");
        reject_fixture_leaf(&crlf, "oplog_admission_crlf");

        let trailing = format!("{object}x\n");
        reject_fixture_leaf(trailing.as_bytes(), "oplog_admission_trailing_bytes");

        let spaced = object.replace("\":", "\": ");
        let spaced = format!("{spaced}\n");
        reject_fixture_leaf(spaced.as_bytes(), "oplog_admission_extra_whitespace");

        reject_fixture_leaf(b"{not-json}\n", "oplog_admission_malformed");

        let duplicate = object.replacen(
            "\"kind\":\"solstone.oplog.admission.v1\",",
            "\"kind\":\"solstone.oplog.admission.v1\",\"kind\":\"solstone.oplog.admission.v1\",",
            1,
        );
        reject_fixture_leaf(
            format!("{duplicate}\n").as_bytes(),
            "oplog_admission_duplicate_field_key",
        );

        let missing_key = object.replace(",\"format\":\"log\"", "");
        reject_fixture_leaf(
            format!("{missing_key}\n").as_bytes(),
            "oplog_admission_wrong_key_set",
        );
        let extra_key =
            object.replace(",\"format\":\"log\"", ",\"format\":\"log\",\"extra\":\"1\"");
        reject_fixture_leaf(
            format!("{extra_key}\n").as_bytes(),
            "oplog_admission_wrong_key_set",
        );

        let reordered = swap_key_pair(object, "source-slug", "source-tag");
        reject_fixture_leaf(
            format!("{reordered}\n").as_bytes(),
            "oplog_admission_wrong_key_order",
        );

        let wrong_version =
            object.replace("solstone.oplog.admission.v1", "solstone.oplog.admission.v2");
        reject_fixture_leaf(
            format!("{wrong_version}\n").as_bytes(),
            "oplog_admission_wrong_version",
        );

        let seven = object.replace(",\"05162738495a6b7c8d9e0f1011121314\"", "");
        reject_fixture_leaf(
            format!("{seven}\n").as_bytes(),
            "oplog_admission_wrong_candidate_cardinality",
        );
        let file_ids_string = object.replace(
            &format!(
                "\"file-ids\":[\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\"]",
                FILE_IDS[0],
                FILE_IDS[1],
                FILE_IDS[2],
                FILE_IDS[3],
                FILE_IDS[4],
                FILE_IDS[5],
                FILE_IDS[6],
                FILE_IDS[7],
            ),
            "\"file-ids\":\"not-an-array\"",
        );
        reject_fixture_leaf(
            format!("{file_ids_string}\n").as_bytes(),
            "oplog_admission_wrong_candidate_cardinality",
        );
        let numbers = object.replace(&format!("\"{}\"", FILE_IDS[0]), "1");
        reject_fixture_leaf(
            format!("{numbers}\n").as_bytes(),
            "oplog_admission_malformed",
        );

        let invalid_utc = object.replace("20260901T164233.381904Z", "20260231T250099.000000Z");
        reject_fixture_leaf(
            format!("{invalid_utc}\n").as_bytes(),
            "oplog_admission_invalid_opened_utc",
        );

        let escaped = object.replace("\"cortex\"", "\"\\u0063ortex\"");
        reject_fixture_leaf(
            format!("{escaped}\n").as_bytes(),
            "oplog_admission_non_canonical",
        );

        let colliding = object.replace(FILE_IDS[1], FILE_IDS[0]);
        reject_fixture_leaf(
            format!("{colliding}\n").as_bytes(),
            "oplog_admission_non_unique_file_id",
        );

        reject(FIXTURE, "not-an-oplog", "oplog_admission_unlisted_leaf");
        let other_id = oplog_name_from_parts(
            "cortex",
            "daily-think",
            "20260901T164233.381904Z".to_owned(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            OplogFormat::Log,
        );
        reject(
            FIXTURE,
            &format_oplog_name(&other_id),
            "oplog_admission_unlisted_leaf",
        );

        let other_leaf = format!(
            "oplog--other~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--{}--daily-think~7df259e6285645a5f9ea769caa484e07.log",
            FILE_IDS[0]
        );
        reject(
            FIXTURE,
            &other_leaf,
            "oplog_admission_incoherent_coordinates",
        );
    }
}
