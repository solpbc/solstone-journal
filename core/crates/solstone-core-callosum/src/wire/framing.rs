// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::CallosumEnvelope;

use super::READ_BUFFER_CAPACITY;

/// Result of decoding one newline-delimited Callosum frame.
pub(crate) enum ReadFrame {
    Envelope(CallosumEnvelope),
    Whitespace,
    Malformed,
    InvalidUtf8,
    Eof,
}

/// Read one complete frame before decoding UTF-8 or JSON.
pub(crate) async fn read_frame<R>(
    reader: &mut BufReader<R>,
    buffer: &mut Vec<u8>,
) -> io::Result<ReadFrame>
where
    R: AsyncRead + Unpin,
{
    buffer.clear();
    let count = reader.read_until(b'\n', buffer).await?;
    if count == 0 {
        return Ok(ReadFrame::Eof);
    }
    if buffer.last() == Some(&b'\n') {
        buffer.pop();
    }
    let line = match std::str::from_utf8(buffer) {
        Ok(line) => line,
        Err(_) => return Ok(ReadFrame::InvalidUtf8),
    };
    if line.trim().is_empty() {
        return Ok(ReadFrame::Whitespace);
    }
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return Ok(ReadFrame::Malformed),
    };
    let Some(object) = value.as_object() else {
        return Ok(ReadFrame::Malformed);
    };
    if !object.contains_key("tract") || !object.contains_key("event") {
        return Ok(ReadFrame::Malformed);
    }
    match serde_json::from_value(value) {
        Ok(envelope) => Ok(ReadFrame::Envelope(envelope)),
        Err(_) => Ok(ReadFrame::Malformed),
    }
}

pub(crate) fn reader<R>(stream: R) -> BufReader<R>
where
    R: AsyncRead + Unpin,
{
    BufReader::with_capacity(READ_BUFFER_CAPACITY, stream)
}
