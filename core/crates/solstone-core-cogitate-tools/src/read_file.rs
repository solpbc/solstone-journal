// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::Path;

use crate::denylist::{classify, refusal_for};
use crate::paths::{ContainedPathError, journal_root_real, resolve_target};
use crate::refusals::{
    NOTICE_READ_FILE_TRUNCATED, REFUSAL_BAD_PATH, REFUSAL_BINARY, REFUSAL_MISSING,
    REFUSAL_NOT_FILE, REFUSAL_PATH_ESCAPE, REFUSAL_PERMISSION_DENIED, REFUSAL_SPECIAL_FILE, charge,
    ok, refused,
};
use crate::{ReadBudget, ReadFileOptions, ReadPayload, ReadResult, ToolName};

pub fn read_file(
    journal: &Path,
    path: &str,
    options: &ReadFileOptions,
    budget: Option<&mut ReadBudget>,
) -> ReadResult {
    if let Some(result) = charge(ToolName::ReadFile, budget) {
        return result;
    }
    let root = match journal_root_real(journal) {
        Ok(root) => root,
        Err(_) => return refused(ToolName::ReadFile, REFUSAL_PERMISSION_DENIED),
    };
    let resolved = match resolve_target(journal, path) {
        Ok(path) => path,
        Err(ContainedPathError::Invalid) => return refused(ToolName::ReadFile, REFUSAL_BAD_PATH),
        Err(ContainedPathError::Escape) => return refused(ToolName::ReadFile, REFUSAL_PATH_ESCAPE),
        Err(ContainedPathError::Io) => {
            return refused(ToolName::ReadFile, REFUSAL_PERMISSION_DENIED);
        }
    };
    if let Some(reason) = refusal_for(classify(&resolved, &root)) {
        return refused(ToolName::ReadFile, reason);
    }
    let metadata = match fs::metadata(&resolved) {
        Ok(metadata) => metadata,
        Err(error) => return refused(ToolName::ReadFile, stat_refusal(&error)),
    };
    if is_special(&metadata) {
        return refused(ToolName::ReadFile, REFUSAL_SPECIAL_FILE);
    }
    if !metadata.is_file() {
        return refused(ToolName::ReadFile, REFUSAL_NOT_FILE);
    }
    let limit = nonnegative(options.max_bytes);
    let mut raw = Vec::new();
    let read = fs::File::open(&resolved).and_then(|mut file| {
        file.by_ref()
            .take(limit.saturating_add(1) as u64)
            .read_to_end(&mut raw)
    });
    if let Err(error) = read {
        return refused(ToolName::ReadFile, stat_refusal(&error));
    }
    if raw[..raw.len().min(8192)].contains(&0) {
        return refused(ToolName::ReadFile, REFUSAL_BINARY);
    }
    let byte_truncated = raw.len() > limit;
    let Some(text) = decode_clipped(&raw[..raw.len().min(limit)]) else {
        return refused(ToolName::ReadFile, REFUSAL_BINARY);
    };
    let lines = splitlines(&text);
    let start = options.start_line.max(1).saturating_sub(1) as usize;
    let line_limit = nonnegative(options.max_lines);
    let selected = if start >= lines.len() {
        Vec::new()
    } else {
        lines[start..lines.len().min(start.saturating_add(line_limit))].to_vec()
    };
    let line_truncated = start < lines.len() && start.saturating_add(selected.len()) < lines.len();
    ok(
        ToolName::ReadFile,
        ReadPayload::Text(selected.join("\n")),
        byte_truncated || line_truncated,
        NOTICE_READ_FILE_TRUNCATED,
    )
}

pub(crate) fn nonnegative(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(usize::MAX)
}
pub(crate) fn decode_clipped(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .ok()
        .or_else(|| {
            (1..=3).find_map(|trim| {
                (bytes.len() >= trim)
                    .then(|| {
                        std::str::from_utf8(&bytes[..bytes.len() - trim])
                            .ok()
                            .map(str::to_owned)
                    })
                    .flatten()
            })
        })
}
/// Match Python `str.splitlines()` without retaining line-boundary characters.
pub(crate) fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        let boundary = matches!(
            character,
            '\n' | '\r'
                | '\u{000B}'
                | '\u{000C}'
                | '\u{001C}'
                | '\u{001D}'
                | '\u{001E}'
                | '\u{0085}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !boundary {
            index += character.len_utf8();
            continue;
        }
        lines.push(&text[start..index]);
        index += character.len_utf8();
        if character == '\r' && text[index..].starts_with('\n') {
            index += '\n'.len_utf8();
        }
        start = index;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}
pub(crate) fn stat_refusal(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => REFUSAL_MISSING,
        std::io::ErrorKind::PermissionDenied => REFUSAL_PERMISSION_DENIED,
        std::io::ErrorKind::IsADirectory => REFUSAL_NOT_FILE,
        _ => REFUSAL_PERMISSION_DENIED,
    }
}
pub(crate) fn is_special(metadata: &fs::Metadata) -> bool {
    #[cfg(not(unix))]
    {
        let _ = metadata;
        return false;
    }
    #[cfg(unix)]
    {
        let kind = metadata.file_type();
        kind.is_socket() || kind.is_block_device() || kind.is_char_device() || kind.is_fifo()
    }
}

#[cfg(test)]
mod tests {
    use super::splitlines;

    #[test]
    fn splitlines_matches_python_lone_cr_and_unicode_boundaries() {
        assert_eq!(splitlines("one\rtwo\u{2028}three"), ["one", "two", "three"]);
        assert_eq!(splitlines("one\r\ntwo\n"), ["one", "two"]);
    }
}
