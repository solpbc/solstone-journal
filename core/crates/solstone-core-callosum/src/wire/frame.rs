// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;

use serde::Serialize;
use serde_json::ser::Formatter;

use crate::CallosumEnvelope;

/// Serialize a Callosum envelope as compact JSONL with Python's ASCII escaping.
pub(crate) fn encode_envelope(envelope: &CallosumEnvelope) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = Vec::new();
    {
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, PythonJsonFormatter);
        envelope.serialize(&mut serializer)?;
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Compact serde_json formatting with `json.dumps(ensure_ascii=True)` string escaping.
pub(crate) struct PythonJsonFormatter;

impl Formatter for PythonJsonFormatter {
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        let mut ascii_start = 0;
        for (index, character) in fragment.char_indices() {
            if character.is_ascii() {
                continue;
            }
            writer.write_all(&fragment.as_bytes()[ascii_start..index])?;
            write_unicode_escape(writer, character)?;
            ascii_start = index + character.len_utf8();
        }
        writer.write_all(&fragment.as_bytes()[ascii_start..])
    }
}

fn write_unicode_escape<W>(writer: &mut W, character: char) -> io::Result<()>
where
    W: ?Sized + io::Write,
{
    let code_point = character as u32;
    if code_point <= 0xffff {
        let mut escaped = [0_u8; 6];
        escaped[0..2].copy_from_slice(b"\\u");
        write_hex4(&mut escaped[2..], code_point as u16);
        return writer.write_all(&escaped);
    }
    let value = code_point - 0x1_0000;
    let high = 0xd800 + (value >> 10);
    let low = 0xdc00 + (value & 0x03ff);
    let mut escaped = [0_u8; 12];
    escaped[0..2].copy_from_slice(b"\\u");
    write_hex4(&mut escaped[2..6], high as u16);
    escaped[6..8].copy_from_slice(b"\\u");
    write_hex4(&mut escaped[8..], low as u16);
    writer.write_all(&escaped)
}

fn write_hex4(output: &mut [u8], value: u16) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (index, shift) in [12, 8, 4, 0].into_iter().enumerate() {
        output[index] = HEX[((value >> shift) & 0x0f) as usize];
    }
}
