// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The one remaining legacy input: the explicitly requested supervisor log.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const REVERSE_TAIL_CHUNK_SIZE: usize = 65_536;

/// Open-by-path seam for the non-canonical supervisor log only.
pub trait TailFileOpener {
    fn open(&self, path: &Path) -> io::Result<fs::File>;
}

/// Production [`TailFileOpener`] implementation.
#[derive(Debug, Default)]
pub struct StdTailFileOpener;

impl TailFileOpener for StdTailFileOpener {
    fn open(&self, path: &Path) -> io::Result<fs::File> {
        fs::File::open(path)
    }
}

/// Read trailing text with the retained forgiving supervisor-log semantics.
///
/// I/O and decoding failures are intentionally an empty result, as before.
pub fn tail_reverse_text(path: &Path, count: i64, opener: &dyn TailFileOpener) -> Vec<String> {
    let mut file = match opener.open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    if file.seek(SeekFrom::End(0)).is_err() {
        return Vec::new();
    }
    let size = match file.stream_position() {
        Ok(size) => size,
        Err(_) => return Vec::new(),
    };
    if size == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut remaining = size;
    while remaining > 0 && count >= 0 && (lines.len() as u128) <= count as u128 {
        let read_size = REVERSE_TAIL_CHUNK_SIZE.min(remaining as usize);
        remaining -= read_size as u64;
        if file.seek(SeekFrom::Start(remaining)).is_err() {
            return Vec::new();
        }
        let mut chunk = vec![0; read_size];
        if file.read_exact(&mut chunk).is_err() {
            return Vec::new();
        }
        let decoded = String::from_utf8_lossy(&chunk);
        let mut chunk_lines = splitlines(&decoded);
        chunk_lines.append(&mut lines);
        lines = chunk_lines;
    }
    tail_slice(lines, count)
}

pub(crate) fn tail_slice<T>(mut lines: Vec<T>, count: i64) -> Vec<T> {
    let start = if count == 0 {
        0
    } else if count > 0 {
        lines.len().saturating_sub(count as usize)
    } else {
        lines.len().min(count.unsigned_abs() as usize)
    };
    lines.drain(..start);
    lines
}

fn splitlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut scalars = text.char_indices().peekable();
    while let Some((index, scalar)) = scalars.next() {
        let boundary_len = match scalar {
            '\n' | '\x0b' | '\x0c' | '\x1c'..='\x1e' | '\u{0085}' | '\u{2028}' | '\u{2029}' => {
                scalar.len_utf8()
            }
            '\r' => {
                if let Some(&(next, '\n')) = scalars.peek() {
                    scalars.next();
                    next + 1 - index
                } else {
                    1
                }
            }
            _ => continue,
        };
        lines.push(text[start..index].to_owned());
        start = index + boundary_len;
    }
    if start < text.len() {
        lines.push(text[start..].to_owned());
    }
    lines
}
