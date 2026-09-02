// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded ASCII admission record at the start of one oplog leaf.

use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;

use super::name::{OplogName, OplogNameClassification, classify_oplog_name, format_oplog_name};

/// Inclusive maximum header size, terminator included.
pub const OPLOG_ADMISSION_MAX_BYTES: usize = 2048;

const VERSION: i64 = 1;
const RECORD_KEYS: [&str; 2] = ["_solstone_oplog_v", "candidates"];

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
/// bounds stay under [`OPLOG_ADMISSION_MAX_BYTES`] (1823 bytes at the cap).
pub(super) fn encode_oplog_admission(names: &[OplogName; 8]) -> Vec<u8> {
    debug_assert!(names.iter().all(|name| {
        name.source().display_slug() == names[0].source().display_slug()
            && name.source().identity_tag() == names[0].source().identity_tag()
            && name.run().display_slug() == names[0].run().display_slug()
            && name.run().identity_tag() == names[0].run().identity_tag()
            && name.opened_utc() == names[0].opened_utc()
            && name.format() == names[0].format()
    }));
    let mut header = format!("{{\"_solstone_oplog_v\":{VERSION},\"candidates\":[");
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            header.push(',');
        }
        header.push_str(
            &serde_json::to_string(&format_oplog_name(name))
                .expect("JSON string encode of canonical leaf"),
        );
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
    let version_value = object.get("_solstone_oplog_v").ok_or_else(malformed)?;
    if version_value.as_i64() != Some(VERSION) {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::WrongVersion));
    }
    let candidates_value = object.get("candidates").ok_or_else(malformed)?;
    let Some(candidate_values) = candidates_value.as_array() else {
        return Err(OplogAdmissionError::new(
            OplogAdmissionClass::WrongCandidateCardinality,
        ));
    };
    if candidate_values.len() != 8 {
        return Err(OplogAdmissionError::new(
            OplogAdmissionClass::WrongCandidateCardinality,
        ));
    }
    let mut candidate_strings = Vec::with_capacity(8);
    for entry in candidate_values {
        let Some(text) = entry.as_str() else {
            return Err(malformed());
        };
        candidate_strings.push(text);
    }
    let mut candidates_vec = Vec::with_capacity(8);
    for text in &candidate_strings {
        match classify_oplog_name(OsStr::new(text)) {
            OplogNameClassification::Candidate(Ok(name)) => candidates_vec.push(name),
            _ => {
                return Err(OplogAdmissionError::new(OplogAdmissionClass::UnlistedLeaf));
            }
        }
    }
    let mut unique_strings = HashSet::new();
    for text in &candidate_strings {
        if !unique_strings.insert(*text) {
            return Err(OplogAdmissionError::new(
                OplogAdmissionClass::NonUniqueFileId,
            ));
        }
    }
    let mut unique_ids = HashSet::new();
    for candidate in &candidates_vec {
        if !unique_ids.insert(candidate.file_id()) {
            return Err(OplogAdmissionError::new(
                OplogAdmissionClass::NonUniqueFileId,
            ));
        }
    }
    let first = &candidates_vec[0];
    for candidate in &candidates_vec[1..] {
        if candidate.source().display_slug() != first.source().display_slug()
            || candidate.source().identity_tag() != first.source().identity_tag()
            || candidate.opened_utc() != first.opened_utc()
            || candidate.run().display_slug() != first.run().display_slug()
            || candidate.run().identity_tag() != first.run().identity_tag()
            || candidate.format() != first.format()
        {
            return Err(OplogAdmissionError::new(
                OplogAdmissionClass::IncoherentCoordinates,
            ));
        }
    }
    let candidates: [OplogName; 8] = candidates_vec
        .try_into()
        .expect("exactly eight reconstructed candidates");
    let encoded = encode_oplog_admission(&candidates);
    if encoded.as_slice() != header {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::NonCanonical));
    }
    let observed = match classify_oplog_name(leaf) {
        OplogNameClassification::Candidate(Ok(name)) => name,
        _ => {
            return Err(OplogAdmissionError::new(OplogAdmissionClass::UnlistedLeaf));
        }
    };
    let observed_formatted = format_oplog_name(&observed);
    if candidates
        .iter()
        .filter(|candidate| format_oplog_name(candidate) == observed_formatted)
        .count()
        != 1
    {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::UnlistedLeaf));
    }
    Ok(OplogAdmissionRecord {
        leaf: observed,
        candidates,
        header_len: header.len(),
    })
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
    use crate::operational_log::name::{OplogFormat, oplog_name_from_parts};

    const FIXTURE: &[u8] = b"{\"_solstone_oplog_v\":1,\"candidates\":[\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--a1b2c3d4e5f60718293a4b5c6d7e8f90--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--b0c1d2e3f405162738495a6b7c8d9e0f--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--c1d2e3f405162738495a6b7c8d9e0f10--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--d2e3f405162738495a6b7c8d9e0f1011--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--e3f405162738495a6b7c8d9e0f101112--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--f405162738495a6b7c8d9e0f10111213--daily-think~7df259e6285645a5f9ea769caa484e07.log\",\"oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.log\"]}\n";

    const SUPERSEDED_A1C: &[u8] = b"{\"kind\":\"solstone.oplog.admission.v1\",\"source-slug\":\"cortex\",\"source-tag\":\"1ee11af4ed5d63caf142a30a96ba124b\",\"run-slug\":\"daily-think\",\"run-tag\":\"7df259e6285645a5f9ea769caa484e07\",\"opened-utc\":\"20260901T164233.381904Z\",\"format\":\"log\",\"file-ids\":[\"8f03cabead7e441d83f6c92b2d89a021\",\"a1b2c3d4e5f60718293a4b5c6d7e8f90\",\"b0c1d2e3f405162738495a6b7c8d9e0f\",\"c1d2e3f405162738495a6b7c8d9e0f10\",\"d2e3f405162738495a6b7c8d9e0f1011\",\"e3f405162738495a6b7c8d9e0f101112\",\"f405162738495a6b7c8d9e0f10111213\",\"05162738495a6b7c8d9e0f1011121314\"]}\n";

    const FIXTURE_LEAF: &str = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log";

    const EIGHTH_LEAF: &str = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.log";

    const SECOND_LEAF: &str = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--a1b2c3d4e5f60718293a4b5c6d7e8f90--daily-think~7df259e6285645a5f9ea769caa484e07.log";

    const CANDIDATE_LEAVES: [&str; 8] = [
        FIXTURE_LEAF,
        SECOND_LEAF,
        "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--b0c1d2e3f405162738495a6b7c8d9e0f--daily-think~7df259e6285645a5f9ea769caa484e07.log",
        "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--c1d2e3f405162738495a6b7c8d9e0f10--daily-think~7df259e6285645a5f9ea769caa484e07.log",
        "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--d2e3f405162738495a6b7c8d9e0f1011--daily-think~7df259e6285645a5f9ea769caa484e07.log",
        "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--e3f405162738495a6b7c8d9e0f101112--daily-think~7df259e6285645a5f9ea769caa484e07.log",
        "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--f405162738495a6b7c8d9e0f10111213--daily-think~7df259e6285645a5f9ea769caa484e07.log",
        EIGHTH_LEAF,
    ];

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
        reject(bytes, FIXTURE_LEAF, token);
    }

    fn swap_key_pair(object: &str, left: &str, right: &str) -> String {
        let left_pat = format!("\"{left}\":");
        let right_pat = format!("\"{right}\":");
        let left_at = object.find(&left_pat).unwrap();
        let right_at = object.find(&right_pat).unwrap();
        assert!(left_at < right_at);
        let left_end = if left == "candidates" {
            object.len() - 1
        } else {
            object[left_at + left_pat.len()..]
                .find(",\"")
                .map(|offset| left_at + left_pat.len() + offset)
                .unwrap()
        };
        let right_end = if right == "candidates" {
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

    fn replace_eighth(object: &str, replacement: &str) -> String {
        object.replace(EIGHTH_LEAF, replacement)
    }

    #[test]
    fn encode_matches_the_pinned_fixture_and_round_trips() {
        let names = [
            oplog_name_from_parts(
                "cortex",
                "daily-think",
                "20260901T164233.381904Z".to_owned(),
                "8f03cabead7e441d83f6c92b2d89a021".to_owned(),
                OplogFormat::Log,
            ),
            oplog_name_from_parts(
                "cortex",
                "daily-think",
                "20260901T164233.381904Z".to_owned(),
                "a1b2c3d4e5f60718293a4b5c6d7e8f90".to_owned(),
                OplogFormat::Log,
            ),
            oplog_name_from_parts(
                "cortex",
                "daily-think",
                "20260901T164233.381904Z".to_owned(),
                "b0c1d2e3f405162738495a6b7c8d9e0f".to_owned(),
                OplogFormat::Log,
            ),
            oplog_name_from_parts(
                "cortex",
                "daily-think",
                "20260901T164233.381904Z".to_owned(),
                "c1d2e3f405162738495a6b7c8d9e0f10".to_owned(),
                OplogFormat::Log,
            ),
            oplog_name_from_parts(
                "cortex",
                "daily-think",
                "20260901T164233.381904Z".to_owned(),
                "d2e3f405162738495a6b7c8d9e0f1011".to_owned(),
                OplogFormat::Log,
            ),
            oplog_name_from_parts(
                "cortex",
                "daily-think",
                "20260901T164233.381904Z".to_owned(),
                "e3f405162738495a6b7c8d9e0f101112".to_owned(),
                OplogFormat::Log,
            ),
            oplog_name_from_parts(
                "cortex",
                "daily-think",
                "20260901T164233.381904Z".to_owned(),
                "f405162738495a6b7c8d9e0f10111213".to_owned(),
                OplogFormat::Log,
            ),
            oplog_name_from_parts(
                "cortex",
                "daily-think",
                "20260901T164233.381904Z".to_owned(),
                "05162738495a6b7c8d9e0f1011121314".to_owned(),
                OplogFormat::Log,
            ),
        ];
        let encoded = encode_oplog_admission(&names);
        assert_eq!(encoded, FIXTURE);
        assert!(encoded.len() <= OPLOG_ADMISSION_MAX_BYTES);
        assert!(encoded.ends_with(b"\n"));
        let mut bytes = encoded.clone();
        bytes.extend_from_slice(b"payload-one\npayload-two\n");
        let record = validate_oplog_admission(OsStr::new(FIXTURE_LEAF), &bytes).unwrap();
        assert_eq!(record.header_len(), encoded.len());
        assert_eq!(&bytes[record.header_len()..], b"payload-one\npayload-two\n");
        assert_eq!(format_oplog_name(record.leaf()), FIXTURE_LEAF);
        assert_eq!(record.candidates().len(), 8);
        assert_eq!(record.leaf().file_id(), names[0].file_id());
        assert!(!format!("{record:?}").contains("payload-one"));
    }

    #[test]
    fn every_fixture_leaf_validates_the_same_offset_and_candidate_order() {
        for leaf in CANDIDATE_LEAVES {
            let record = validate_oplog_admission(OsStr::new(leaf), FIXTURE).unwrap();
            assert_eq!(record.header_len(), FIXTURE.len());
            assert_eq!(format_oplog_name(record.leaf()), leaf);
            assert_eq!(record.candidates().len(), 8);
            let formatted: Vec<String> =
                record.candidates().iter().map(format_oplog_name).collect();
            assert_eq!(formatted, CANDIDATE_LEAVES);
        }
    }

    #[test]
    fn maximal_encoded_name_stays_under_the_byte_cap() {
        let ids = [
            "8f03cabead7e441d83f6c92b2d89a021",
            "a1b2c3d4e5f60718293a4b5c6d7e8f90",
            "b0c1d2e3f405162738495a6b7c8d9e0f",
            "c1d2e3f405162738495a6b7c8d9e0f10",
            "d2e3f405162738495a6b7c8d9e0f1011",
            "e3f405162738495a6b7c8d9e0f101112",
            "f405162738495a6b7c8d9e0f10111213",
            "05162738495a6b7c8d9e0f1011121314",
        ];
        let names = ids.map(|file_id| {
            oplog_name_from_parts(
                &"a".repeat(40),
                &"b".repeat(40),
                "20260901T164233.381904Z".to_owned(),
                file_id.to_owned(),
                OplogFormat::Jsonl,
            )
        });
        let encoded = encode_oplog_admission(&names);
        assert_eq!(encoded.len(), 1823);
        assert!(encoded.len() <= OPLOG_ADMISSION_MAX_BYTES);
        let leaf = format_oplog_name(&names[0]);
        assert_eq!(leaf.len(), 220);
        validate_oplog_admission(OsStr::new(&leaf), &encoded).unwrap();
    }

    #[test]
    fn twins_reject_with_distinct_tokens() {
        let object = std::str::from_utf8(&FIXTURE[..FIXTURE.len() - 1]).unwrap();

        reject(b"", FIXTURE_LEAF, "oplog_admission_missing");
        reject(
            b"{\"_solstone_oplog_v\":1",
            FIXTURE_LEAF,
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
            "\"_solstone_oplog_v\":1,",
            "\"_solstone_oplog_v\":1,\"_solstone_oplog_v\":1,",
            1,
        );
        reject_fixture_leaf(
            format!("{duplicate}\n").as_bytes(),
            "oplog_admission_duplicate_field_key",
        );

        let missing_key = object.replacen("\"_solstone_oplog_v\":1,", "", 1);
        reject_fixture_leaf(
            format!("{missing_key}\n").as_bytes(),
            "oplog_admission_wrong_key_set",
        );
        let extra_key = format!("{},\"extra\":\"1\"}}\n", &object[..object.len() - 1]);
        reject_fixture_leaf(extra_key.as_bytes(), "oplog_admission_wrong_key_set");
        reject_fixture_leaf(SUPERSEDED_A1C, "oplog_admission_wrong_key_set");
        let mut superseded_with_payload = SUPERSEDED_A1C.to_vec();
        superseded_with_payload.extend_from_slice(b"payload-one\n");
        reject_fixture_leaf(&superseded_with_payload, "oplog_admission_wrong_key_set");

        let reordered = swap_key_pair(object, "_solstone_oplog_v", "candidates");
        reject_fixture_leaf(
            format!("{reordered}\n").as_bytes(),
            "oplog_admission_wrong_key_order",
        );

        let wrong_version =
            object.replacen("\"_solstone_oplog_v\":1,", "\"_solstone_oplog_v\":2,", 1);
        reject_fixture_leaf(
            format!("{wrong_version}\n").as_bytes(),
            "oplog_admission_wrong_version",
        );
        let version_string = object.replacen(
            "\"_solstone_oplog_v\":1,",
            "\"_solstone_oplog_v\":\"1\",",
            1,
        );
        reject_fixture_leaf(
            format!("{version_string}\n").as_bytes(),
            "oplog_admission_wrong_version",
        );
        let version_float =
            object.replacen("\"_solstone_oplog_v\":1,", "\"_solstone_oplog_v\":1.0,", 1);
        reject_fixture_leaf(
            format!("{version_float}\n").as_bytes(),
            "oplog_admission_wrong_version",
        );
        let version_exp =
            object.replacen("\"_solstone_oplog_v\":1,", "\"_solstone_oplog_v\":1e0,", 1);
        reject_fixture_leaf(
            format!("{version_exp}\n").as_bytes(),
            "oplog_admission_wrong_version",
        );

        let seven = object.replace(&format!(",\"{EIGHTH_LEAF}\""), "");
        reject_fixture_leaf(
            format!("{seven}\n").as_bytes(),
            "oplog_admission_wrong_candidate_cardinality",
        );
        reject_fixture_leaf(
            b"{\"_solstone_oplog_v\":1,\"candidates\":\"not-an-array\"}\n",
            "oplog_admission_wrong_candidate_cardinality",
        );
        let numbers = object.replacen(&format!("\"{FIXTURE_LEAF}\""), "1", 1);
        reject_fixture_leaf(
            format!("{numbers}\n").as_bytes(),
            "oplog_admission_malformed",
        );

        let invalid_utc = replace_eighth(
            object,
            "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260231T250099.000000Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.log",
        );
        reject_fixture_leaf(
            format!("{invalid_utc}\n").as_bytes(),
            "oplog_admission_unlisted_leaf",
        );

        let escaped = object.replacen("oplog--cortex~", "oplog--\\u0063ortex~", 1);
        reject_fixture_leaf(
            format!("{escaped}\n").as_bytes(),
            "oplog_admission_non_canonical",
        );

        let colliding = object.replace(SECOND_LEAF, FIXTURE_LEAF);
        reject_fixture_leaf(
            format!("{colliding}\n").as_bytes(),
            "oplog_admission_non_unique_file_id",
        );
        let duplicate_id_hidden_slug = object.replace(
            SECOND_LEAF,
            "oplog--other~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log",
        );
        reject_fixture_leaf(
            format!("{duplicate_id_hidden_slug}\n").as_bytes(),
            "oplog_admission_non_unique_file_id",
        );

        reject(FIXTURE, "not-an-oplog", "oplog_admission_unlisted_leaf");
        reject(
            FIXTURE,
            "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa--daily-think~7df259e6285645a5f9ea769caa484e07.log",
            "oplog_admission_unlisted_leaf",
        );
        reject(
            FIXTURE,
            "oplog--other~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log",
            "oplog_admission_unlisted_leaf",
        );

        let source_slug = replace_eighth(
            object,
            "oplog--other~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.log",
        );
        reject_fixture_leaf(
            format!("{source_slug}\n").as_bytes(),
            "oplog_admission_incoherent_coordinates",
        );
        let source_tag = replace_eighth(
            object,
            "oplog--cortex~aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.log",
        );
        reject_fixture_leaf(
            format!("{source_tag}\n").as_bytes(),
            "oplog_admission_incoherent_coordinates",
        );
        let opened_utc = replace_eighth(
            object,
            "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260902T000000.000000Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.log",
        );
        reject_fixture_leaf(
            format!("{opened_utc}\n").as_bytes(),
            "oplog_admission_incoherent_coordinates",
        );
        let run_slug = replace_eighth(
            object,
            "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--other-run~7df259e6285645a5f9ea769caa484e07.log",
        );
        reject_fixture_leaf(
            format!("{run_slug}\n").as_bytes(),
            "oplog_admission_incoherent_coordinates",
        );
        let run_tag = replace_eighth(
            object,
            "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--daily-think~aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.log",
        );
        reject_fixture_leaf(
            format!("{run_tag}\n").as_bytes(),
            "oplog_admission_incoherent_coordinates",
        );
        let format = replace_eighth(
            object,
            "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--05162738495a6b7c8d9e0f1011121314--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl",
        );
        reject_fixture_leaf(
            format!("{format}\n").as_bytes(),
            "oplog_admission_incoherent_coordinates",
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_invalid_native_bytes_are_unlisted_without_payload_offset() {
        use std::os::unix::ffi::OsStrExt;

        let unrelated = OsStr::from_bytes(b"not-oplog-\xff");
        expect_token(
            validate_oplog_admission(unrelated, FIXTURE).unwrap_err(),
            "oplog_admission_unlisted_leaf",
        );
        let prefixed = OsStr::from_bytes(b"oplog--\xff");
        expect_token(
            validate_oplog_admission(prefixed, FIXTURE).unwrap_err(),
            "oplog_admission_unlisted_leaf",
        );
        let non_ascii = OsStr::from_bytes("oplog--caf\u{e9}".as_bytes());
        expect_token(
            validate_oplog_admission(non_ascii, FIXTURE).unwrap_err(),
            "oplog_admission_unlisted_leaf",
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_unpaired_surrogates_are_unlisted_without_payload_offset() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let unrelated = OsString::from_wide(&[b'x' as u16, 0xd800]);
        expect_token(
            validate_oplog_admission(&unrelated, FIXTURE).unwrap_err(),
            "oplog_admission_unlisted_leaf",
        );
        let mut prefixed = Vec::from(b"oplog--".map(u16::from));
        prefixed.push(0xd800);
        let prefixed = OsString::from_wide(&prefixed);
        expect_token(
            validate_oplog_admission(&prefixed, FIXTURE).unwrap_err(),
            "oplog_admission_unlisted_leaf",
        );
    }
}
