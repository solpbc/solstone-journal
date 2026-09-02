// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded ASCII admission record at the start of one oplog leaf.

use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;

use super::name::{
    OplogFormat, OplogName, OplogNameClassification, classify_oplog_name, format_oplog_name,
};

/// Inclusive maximum header size, terminator included.
pub const OPLOG_ADMISSION_MAX_BYTES: usize = 2048;

const VERSION: &str = "solstone.oplog.admission.v1";
const FIELD_KEYS: [&str; 8] = [
    "leaf",
    "source-slug",
    "source-tag",
    "opened-utc",
    "file-id",
    "run-slug",
    "run-tag",
    "format",
];

/// Parsed admission header bound to one canonical leaf.
pub struct OplogAdmissionRecord {
    name: OplogName,
    header_len: usize,
}

impl OplogAdmissionRecord {
    /// Reconstructed canonical name.
    pub fn name(&self) -> &OplogName {
        &self.name
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
            .field("name", &self.name)
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
    Malformed,
    Overlong,
    InvalidUtf8,
    WrongVersion,
    WrongFieldCardinality,
    DuplicateFieldKey,
    UnlistedLeaf,
    Incoherent,
    NonUniqueFileId,
}

impl OplogAdmissionClass {
    const fn token(self) -> &'static str {
        match self {
            Self::Missing => "oplog_admission_missing",
            Self::Malformed => "oplog_admission_malformed",
            Self::Overlong => "oplog_admission_overlong",
            Self::InvalidUtf8 => "oplog_admission_invalid_utf8",
            Self::WrongVersion => "oplog_admission_wrong_version",
            Self::WrongFieldCardinality => "oplog_admission_wrong_field_cardinality",
            Self::DuplicateFieldKey => "oplog_admission_duplicate_field_key",
            Self::UnlistedLeaf => "oplog_admission_unlisted_leaf",
            Self::Incoherent => "oplog_admission_incoherent",
            Self::NonUniqueFileId => "oplog_admission_non_unique_file_id",
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

/// Encode the canonical admission header for `name`.
///
/// Maximal slug/tag/leaf bounds from the name grammar keep this under
/// [`OPLOG_ADMISSION_MAX_BYTES`]: version line 27 bytes, `leaf=` plus 220-byte
/// name 226, two 40-byte slugs 103, three 32-byte hex fields 129, 23-byte UTC
/// 35, `format=jsonl` 13, terminator 1 — 534 bytes.
pub(super) fn encode_oplog_admission(name: &OplogName) -> Vec<u8> {
    let leaf = format_oplog_name(name);
    let format = match name.format() {
        OplogFormat::Log => "log",
        OplogFormat::Jsonl => "jsonl",
    };
    let header = format!(
        "{VERSION}\nleaf={leaf}\nsource-slug={}\nsource-tag={}\nopened-utc={}\nfile-id={}\nrun-slug={}\nrun-tag={}\nformat={format}\n\n",
        name.source().display_slug(),
        name.source().identity_tag(),
        name.opened_utc(),
        name.file_id(),
        name.run().display_slug(),
        name.run().identity_tag(),
    );
    debug_assert!(header.len() <= OPLOG_ADMISSION_MAX_BYTES);
    debug_assert!(header.bytes().all(is_grammar_byte));
    header.into_bytes()
}

/// Validate one admission header at the start of `bytes` for on-disk `leaf`.
pub fn validate_oplog_admission(
    leaf: &OsStr,
    bytes: &[u8],
) -> Result<OplogAdmissionRecord, OplogAdmissionError> {
    if bytes.is_empty() {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::Missing));
    }
    let search_len = bytes.len().min(OPLOG_ADMISSION_MAX_BYTES);
    let Some(offset) = find_terminator(&bytes[..search_len]) else {
        return if bytes.len() >= OPLOG_ADMISSION_MAX_BYTES {
            Err(OplogAdmissionError::new(OplogAdmissionClass::Overlong))
        } else {
            Err(OplogAdmissionError::new(OplogAdmissionClass::Missing))
        };
    };
    let header_len = offset + 2;
    let header = &bytes[..header_len];
    let text = std::str::from_utf8(header)
        .map_err(|_| OplogAdmissionError::new(OplogAdmissionClass::InvalidUtf8))?;
    if !header.iter().copied().all(is_grammar_byte) {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::Malformed));
    }
    let body = &text[..text.len() - 2];
    let mut lines = body.split('\n');
    let Some(version) = lines.next() else {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::WrongVersion));
    };
    if version != VERSION {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::WrongVersion));
    }
    let mut keys = Vec::with_capacity(FIELD_KEYS.len());
    let mut values = Vec::with_capacity(FIELD_KEYS.len());
    let mut seen = HashSet::new();
    for line in lines {
        let Some((key, value)) = line.split_once('=') else {
            return Err(OplogAdmissionError::new(OplogAdmissionClass::Malformed));
        };
        if key.is_empty() {
            return Err(OplogAdmissionError::new(OplogAdmissionClass::Malformed));
        }
        if !seen.insert(key) {
            return Err(OplogAdmissionError::new(
                OplogAdmissionClass::DuplicateFieldKey,
            ));
        }
        keys.push(key);
        values.push(value);
    }
    if keys.as_slice() != FIELD_KEYS {
        return Err(OplogAdmissionError::new(
            OplogAdmissionClass::WrongFieldCardinality,
        ));
    }
    let reconstructed = reconstruct_name(&values)?;
    let formatted = format_oplog_name(&reconstructed);
    if formatted != values[0] || OsStr::new(&formatted) != leaf {
        return Err(OplogAdmissionError::new(OplogAdmissionClass::Incoherent));
    }
    Ok(OplogAdmissionRecord {
        name: reconstructed,
        header_len,
    })
}

/// Validate each pair, then refuse a repeated `file-id` across the set.
pub fn validate_oplog_admission_set(
    entries: &[(&OsStr, &[u8])],
) -> Result<Vec<OplogAdmissionRecord>, OplogAdmissionError> {
    let mut records = Vec::with_capacity(entries.len());
    for (leaf, bytes) in entries {
        records.push(validate_oplog_admission(leaf, bytes)?);
    }
    let mut file_ids = HashSet::new();
    for record in &records {
        if !file_ids.insert(record.name.file_id()) {
            return Err(OplogAdmissionError::new(
                OplogAdmissionClass::NonUniqueFileId,
            ));
        }
    }
    Ok(records)
}

fn reconstruct_name(values: &[&str]) -> Result<OplogName, OplogAdmissionError> {
    let suffix = match values[7] {
        "log" | "jsonl" => values[7],
        _ => {
            return Err(OplogAdmissionError::new(OplogAdmissionClass::UnlistedLeaf));
        }
    };
    let assembled = format!(
        "oplog--{}~{}--{}--{}--{}~{}.{suffix}",
        values[1], values[2], values[3], values[4], values[5], values[6],
    );
    match classify_oplog_name(OsStr::new(&assembled)) {
        OplogNameClassification::Candidate(Ok(name)) => Ok(name),
        _ => Err(OplogAdmissionError::new(OplogAdmissionClass::UnlistedLeaf)),
    }
}

fn find_terminator(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\n\n")
}

fn is_grammar_byte(byte: u8) -> bool {
    byte == b'\n' || (0x20..=0x7E).contains(&byte)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::ffi::OsStr;

    use super::*;
    use crate::operational_log::name::oplog_name_from_parts;

    fn reference_name() -> OplogName {
        oplog_name_from_parts(
            "cortex",
            "daily-think",
            "20260901T164233.381904Z".to_owned(),
            "8f03cabead7e441d83f6c92b2d89a021".to_owned(),
            OplogFormat::Log,
        )
    }

    fn expect_token(error: OplogAdmissionError, token: &str) {
        assert_eq!(error.to_string(), token);
        assert_eq!(format!("{error:?}"), token);
        assert!(error.source().is_none());
    }

    fn header_with_fields(fields: &[(&str, &str)]) -> Vec<u8> {
        let mut out = format!("{VERSION}\n");
        for (key, value) in fields {
            out.push_str(key);
            out.push('=');
            out.push_str(value);
            out.push('\n');
        }
        out.push('\n');
        out.into_bytes()
    }

    fn valid_fields(name: &OplogName) -> Vec<(String, String)> {
        let format = match name.format() {
            OplogFormat::Log => "log",
            OplogFormat::Jsonl => "jsonl",
        };
        vec![
            ("leaf".to_owned(), format_oplog_name(name)),
            (
                "source-slug".to_owned(),
                name.source().display_slug().to_owned(),
            ),
            (
                "source-tag".to_owned(),
                name.source().identity_tag().to_owned(),
            ),
            ("opened-utc".to_owned(), name.opened_utc().to_owned()),
            ("file-id".to_owned(), name.file_id().to_owned()),
            ("run-slug".to_owned(), name.run().display_slug().to_owned()),
            ("run-tag".to_owned(), name.run().identity_tag().to_owned()),
            ("format".to_owned(), format.to_owned()),
        ]
    }

    #[test]
    fn encode_round_trips_and_payload_starts_after_terminator() {
        let name = reference_name();
        let mut bytes = encode_oplog_admission(&name);
        let header_len = bytes.len();
        assert!(header_len <= OPLOG_ADMISSION_MAX_BYTES);
        assert!(bytes.ends_with(b"\n\n"));
        bytes.extend_from_slice(b"payload-one\npayload-two\n");
        let leaf = format_oplog_name(&name);
        let record = validate_oplog_admission(OsStr::new(&leaf), &bytes).unwrap();
        assert_eq!(record.header_len(), header_len);
        assert_eq!(&bytes[record.header_len()..], b"payload-one\npayload-two\n");
        assert_eq!(format_oplog_name(record.name()), leaf);
        assert_eq!(record.name().file_id(), name.file_id());
        assert!(!format!("{record:?}").contains("payload-one"));
    }

    #[test]
    fn maximal_encoded_name_stays_under_the_byte_cap() {
        let name = oplog_name_from_parts(
            &"a".repeat(40),
            &"b".repeat(40),
            "20260901T164233.381904Z".to_owned(),
            "ffffffffffffffffffffffffffffffff".to_owned(),
            OplogFormat::Jsonl,
        );
        let encoded = encode_oplog_admission(&name);
        assert!(encoded.len() <= OPLOG_ADMISSION_MAX_BYTES);
        let leaf = format_oplog_name(&name);
        assert_eq!(leaf.len(), 220);
        validate_oplog_admission(OsStr::new(&leaf), &encoded).unwrap();
    }

    #[test]
    fn missing_empty_or_unterminated_short_input() {
        let name = reference_name();
        let leaf = format_oplog_name(&name);
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), b"").unwrap_err(),
            "oplog_admission_missing",
        );
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), b"solstone.oplog.admission.v1\nleaf=")
                .unwrap_err(),
            "oplog_admission_missing",
        );
    }

    #[test]
    fn malformed_cr_or_non_key_value_line() {
        let name = reference_name();
        let leaf = format_oplog_name(&name);
        let mut cr = encode_oplog_admission(&name);
        cr[VERSION.len()] = b'\r';
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &cr).unwrap_err(),
            "oplog_admission_malformed",
        );
        let mut broken = encode_oplog_admission(&name);
        let needle = b"format=log\n";
        let at = broken
            .windows(needle.len())
            .position(|window| window == needle)
            .unwrap();
        broken.splice(at..at + needle.len(), b"format log\n".iter().copied());
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &broken).unwrap_err(),
            "oplog_admission_malformed",
        );
    }

    #[test]
    fn overlong_scan_without_terminator() {
        let name = reference_name();
        let leaf = format_oplog_name(&name);
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &[b'x'; OPLOG_ADMISSION_MAX_BYTES])
                .unwrap_err(),
            "oplog_admission_overlong",
        );
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &[b'x'; OPLOG_ADMISSION_MAX_BYTES + 8])
                .unwrap_err(),
            "oplog_admission_overlong",
        );
    }

    #[test]
    fn invalid_utf8_in_header() {
        let name = reference_name();
        let leaf = format_oplog_name(&name);
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), b"solstone.oplog.admission.v1\n\xff\n\n")
                .unwrap_err(),
            "oplog_admission_invalid_utf8",
        );
    }

    #[test]
    fn wrong_version_token() {
        let name = reference_name();
        let leaf = format_oplog_name(&name);
        let mut bytes = encode_oplog_admission(&name);
        bytes.splice(
            ..VERSION.len(),
            b"solstone.oplog.admission.v2".iter().copied(),
        );
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &bytes).unwrap_err(),
            "oplog_admission_wrong_version",
        );
    }

    #[test]
    fn wrong_field_cardinality_missing_extra_or_reordered() {
        let name = reference_name();
        let leaf = format_oplog_name(&name);
        let fields = valid_fields(&name);
        let pairs: Vec<(&str, &str)> = fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &header_with_fields(&pairs[..7]))
                .unwrap_err(),
            "oplog_admission_wrong_field_cardinality",
        );
        let mut extra = pairs.clone();
        extra.push(("extra", "1"));
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &header_with_fields(&extra)).unwrap_err(),
            "oplog_admission_wrong_field_cardinality",
        );
        let mut reordered = pairs.clone();
        reordered.swap(4, 3);
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &header_with_fields(&reordered))
                .unwrap_err(),
            "oplog_admission_wrong_field_cardinality",
        );
    }

    #[test]
    fn duplicate_field_key() {
        let name = reference_name();
        let leaf = format_oplog_name(&name);
        let mut fields = valid_fields(&name);
        fields[7] = ("leaf".to_owned(), fields[0].1.clone());
        let pairs: Vec<(&str, &str)> = fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &header_with_fields(&pairs)).unwrap_err(),
            "oplog_admission_duplicate_field_key",
        );
    }

    #[test]
    fn unlisted_leaf_from_invalid_component() {
        let name = reference_name();
        let leaf = format_oplog_name(&name);
        let mut fields = valid_fields(&name);
        fields[4].1 = "not-hex".to_owned();
        let pairs: Vec<(&str, &str)> = fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &header_with_fields(&pairs)).unwrap_err(),
            "oplog_admission_unlisted_leaf",
        );
        fields = valid_fields(&name);
        fields[7].1 = "txt".to_owned();
        let pairs: Vec<(&str, &str)> = fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        expect_token(
            validate_oplog_admission(OsStr::new(&leaf), &header_with_fields(&pairs)).unwrap_err(),
            "oplog_admission_unlisted_leaf",
        );
    }

    #[test]
    fn incoherent_when_leaf_field_or_on_disk_name_diverges() {
        let name = reference_name();
        let other = oplog_name_from_parts(
            "cortex",
            "daily-think",
            "20260901T164233.381904Z".to_owned(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            OplogFormat::Log,
        );
        let mut fields = valid_fields(&name);
        fields[0].1 = format_oplog_name(&other);
        let pairs: Vec<(&str, &str)> = fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        expect_token(
            validate_oplog_admission(
                OsStr::new(&format_oplog_name(&name)),
                &header_with_fields(&pairs),
            )
            .unwrap_err(),
            "oplog_admission_incoherent",
        );
        let encoded = encode_oplog_admission(&name);
        expect_token(
            validate_oplog_admission(OsStr::new(&format_oplog_name(&other)), &encoded).unwrap_err(),
            "oplog_admission_incoherent",
        );
    }

    #[test]
    fn set_rejects_colliding_file_ids_and_propagates_first_record_error() {
        let opened = "20260901T164233.381904Z".to_owned();
        let file_id = "8f03cabead7e441d83f6c92b2d89a021".to_owned();
        let first = oplog_name_from_parts(
            "cortex",
            "run-a",
            opened.clone(),
            file_id.clone(),
            OplogFormat::Log,
        );
        let second = oplog_name_from_parts("cortex", "run-b", opened, file_id, OplogFormat::Log);
        let first_bytes = encode_oplog_admission(&first);
        let second_bytes = encode_oplog_admission(&second);
        let first_leaf = format_oplog_name(&first);
        let second_leaf = format_oplog_name(&second);
        expect_token(
            validate_oplog_admission_set(&[
                (OsStr::new(&first_leaf), first_bytes.as_slice()),
                (OsStr::new(&second_leaf), second_bytes.as_slice()),
            ])
            .unwrap_err(),
            "oplog_admission_non_unique_file_id",
        );
        let distinct = oplog_name_from_parts(
            "cortex",
            "run-b",
            "20260901T164233.381904Z".to_owned(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            OplogFormat::Log,
        );
        let distinct_bytes = encode_oplog_admission(&distinct);
        let distinct_leaf = format_oplog_name(&distinct);
        let records = validate_oplog_admission_set(&[
            (OsStr::new(&first_leaf), first_bytes.as_slice()),
            (OsStr::new(&distinct_leaf), distinct_bytes.as_slice()),
        ])
        .unwrap();
        assert_eq!(records.len(), 2);
        expect_token(
            validate_oplog_admission_set(&[(OsStr::new(&first_leaf), b"".as_slice())]).unwrap_err(),
            "oplog_admission_missing",
        );
    }
}
