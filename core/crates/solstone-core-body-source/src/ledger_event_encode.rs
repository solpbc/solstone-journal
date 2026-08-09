// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::canonicalize::{
    CanonicalSink, CappedVecSink, write_integer, write_object_end, write_object_key,
    write_object_start, write_quoted_code_points, write_separator,
};
use crate::{
    BodyInteger, BodyLedgerEvent, BodyString, LedgerEventError, LedgerEventErrorCode,
    LedgerEventErrorField,
};

pub(crate) const MAX_LEDGER_EVENT_OBJECT_BYTES: usize = 65_536;
pub(crate) const MAX_LEDGER_EVENT_FRAME_BYTES: usize = 65_537;

/// Encodes a checked body-ledger event as canonical JSONL bytes.
pub fn encode_body_ledger_event(event: &BodyLedgerEvent) -> Result<Vec<u8>, LedgerEventError> {
    let mut buffer = Vec::new();
    {
        let mut sink = CappedVecSink::new(&mut buffer, MAX_LEDGER_EVENT_OBJECT_BYTES);
        write_ledger_event(&mut sink, event).map_err(|_| overflow(event))?;
    }
    {
        let mut sink = CappedVecSink::new(&mut buffer, MAX_LEDGER_EVENT_FRAME_BYTES);
        sink.write_bytes(b"\n").map_err(|_| overflow(event))?;
    }
    Ok(buffer)
}

fn overflow(event: &BodyLedgerEvent) -> LedgerEventError {
    LedgerEventError::new(
        Some(event.bundle_id().clone()),
        LedgerEventErrorCode::InputTooLarge,
        LedgerEventErrorField::Ledger,
        event.sequence(),
    )
}

fn write_ledger_event<S: CanonicalSink>(
    sink: &mut S,
    event: &BodyLedgerEvent,
) -> Result<(), S::Error> {
    write_object_start(sink)?;
    write_key(sink, "bundle_id")?;
    write_ascii_string(sink, event.bundle_id().as_str())?;
    write_separator(sink)?;
    write_key(sink, "day")?;
    write_ascii_string(sink, event.day().as_str())?;
    write_separator(sink)?;
    write_key(sink, "dedupe_key")?;
    write_ascii_string(sink, event.dedupe_key().as_str())?;
    write_separator(sink)?;
    write_key(sink, "end_time")?;
    write_optional_body_string(sink, event.end_time())?;
    write_separator(sink)?;
    write_key(sink, "line")?;
    write_u64(sink, event.line())?;
    write_separator(sink)?;
    write_key(sink, "normalized_ref")?;
    write_body_string(sink, event.normalized_ref())?;
    write_separator(sink)?;
    write_key(sink, "raw_ref")?;
    write_optional_body_string(sink, event.raw_ref())?;
    write_separator(sink)?;
    write_key(sink, "record_type")?;
    write_body_string(sink, event.record_type())?;
    write_separator(sink)?;
    write_key(sink, "row_schema")?;
    write_ascii_string(sink, event.row_schema().as_str())?;
    write_separator(sink)?;
    write_key(sink, "row_sha256")?;
    write_ascii_string(sink, event.row_sha256().as_str())?;
    write_separator(sink)?;
    write_key(sink, "schema")?;
    write_ascii_string(sink, event.schema())?;
    write_separator(sink)?;
    write_key(sink, "sequence")?;
    write_u64(sink, event.sequence())?;
    write_separator(sink)?;
    write_key(sink, "shard")?;
    write_ascii_string(sink, event.shard())?;
    write_separator(sink)?;
    write_key(sink, "source_family")?;
    write_ascii_string(sink, event.source_family().as_str())?;
    write_separator(sink)?;
    write_key(sink, "source_record_id")?;
    write_optional_body_string(sink, event.source_record_id())?;
    write_separator(sink)?;
    write_key(sink, "start_time")?;
    write_body_string(sink, event.start_time())?;
    write_separator(sink)?;
    write_key(sink, "value_hash")?;
    write_ascii_string(sink, event.value_hash().as_str())?;
    write_object_end(sink)
}

fn write_key<S: CanonicalSink>(sink: &mut S, key: &str) -> Result<(), S::Error> {
    write_object_key(sink, key.bytes().map(u32::from))
}

fn write_ascii_string<S: CanonicalSink>(sink: &mut S, value: &str) -> Result<(), S::Error> {
    write_quoted_code_points(sink, value.bytes().map(u32::from))
}

fn write_body_string<S: CanonicalSink>(sink: &mut S, value: &BodyString) -> Result<(), S::Error> {
    write_quoted_code_points(sink, value.code_points().iter().copied())
}

fn write_optional_body_string<S: CanonicalSink>(
    sink: &mut S,
    value: Option<&BodyString>,
) -> Result<(), S::Error> {
    match value {
        Some(value) => write_body_string(sink, value),
        None => sink.write_bytes(b"null"),
    }
}

fn write_u64<S: CanonicalSink>(sink: &mut S, value: u64) -> Result<(), S::Error> {
    let integer = BodyInteger::from_u64(value);
    write_integer(sink, integer.is_negative(), integer.digits())
}
