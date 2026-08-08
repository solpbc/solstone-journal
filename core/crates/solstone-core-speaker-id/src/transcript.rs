// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use serde_json::Value;

/// How a transcript row's sentence ID was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentenceIdSource {
    Persisted,
    Positional,
    PositionalAfterIgnoredId,
}

/// One parsed transcript row with its resolved sentence ID.
#[derive(Debug, Clone)]
pub struct TranscriptRow {
    pub sentence_id: i64,
    pub source: SentenceIdSource,
    pub value: Value,
}

/// Parsed transcript rows and corruption counters.
#[derive(Debug, Clone, Default)]
pub struct TranscriptRead {
    pub had_header: bool,
    pub rows: Vec<TranscriptRow>,
    pub malformed_lines: usize,
    pub ignored_ids: usize,
    pub disagreements: usize,
    pub duplicate_ids: usize,
}

/// Failure while reading transcript bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptError {
    InvalidUtf8,
}

impl fmt::Display for TranscriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 => formatter.write_str("transcript bytes are not valid UTF-8"),
        }
    }
}

impl Error for TranscriptError {}

/// Resolve a persisted sentence ID when valid, otherwise use the row ordinal.
pub fn sentence_id_for(ordinal_after_header: i64, row: &Value) -> (i64, SentenceIdSource) {
    let Some(value) = row.get("sentence_id") else {
        return (ordinal_after_header, SentenceIdSource::Positional);
    };
    let Some(sentence_id) = value.as_i64() else {
        return (
            ordinal_after_header,
            SentenceIdSource::PositionalAfterIgnoredId,
        );
    };
    if (1..210).contains(&sentence_id) {
        (sentence_id, SentenceIdSource::Persisted)
    } else {
        (
            ordinal_after_header,
            SentenceIdSource::PositionalAfterIgnoredId,
        )
    }
}

/// Read a headered transcript JSONL payload with universal newline handling.
pub fn read_transcript_rows(bytes: &[u8]) -> Result<TranscriptRead, TranscriptError> {
    let text = std::str::from_utf8(bytes).map_err(|_| TranscriptError::InvalidUtf8)?;
    let lines = split_universal_newlines(text);
    if lines.is_empty() {
        return Ok(TranscriptRead::default());
    }

    let mut read = TranscriptRead {
        had_header: true,
        ..TranscriptRead::default()
    };
    for (index, line) in lines.into_iter().skip(1).enumerate() {
        let ordinal_after_header = index as i64 + 1;
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            read.malformed_lines += 1;
            continue;
        };
        if !value.is_object() {
            read.malformed_lines += 1;
            continue;
        }

        let (sentence_id, source) = sentence_id_for(ordinal_after_header, &value);
        if source == SentenceIdSource::PositionalAfterIgnoredId {
            read.ignored_ids += 1;
        }
        if source == SentenceIdSource::Persisted && sentence_id != ordinal_after_header {
            read.disagreements += 1;
        }
        read.rows.push(TranscriptRow {
            sentence_id,
            source,
            value,
        });
    }

    let mut counts = HashMap::new();
    for row in &read.rows {
        *counts.entry(row.sentence_id).or_insert(0_usize) += 1;
    }
    read.duplicate_ids = counts
        .into_values()
        .map(|count| count.saturating_sub(1))
        .sum();
    Ok(read)
}

fn split_universal_newlines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\n' || bytes[index] == b'\r' {
            lines.push(&text[start..index]);
            if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
            index += 1;
            start = index;
            continue;
        }
        index += 1;
    }
    if start < bytes.len() {
        lines.push(&text[start..]);
    }
    lines
}
