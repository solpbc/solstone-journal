// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// How the dashboard finds static references and names hashed asset trees.
// Included by the shell's build script, which rewrites and hashes, and by the
// shell itself, which serves and tests the result, so both read references the
// same way.
//
// A tree is one directory of build-only files that is hashed as a unit:
// `/static` (the shell), `/static/vendor/<library>` (each vendor library) and
// `/app/<app>/static` (each view). A file at `<root>/<path>` is served for good
// at `<root>/<hash>/<path>`, so references relative to it keep working.

/// Hex digits of a tree hash in its URL.
#[allow(dead_code)]
pub const TREE_HASH_LEN: usize = 16;

/// The tree a served path belongs to, if any. HTML pages are documents, served
/// at their own stable address, and never part of a tree. The shell's manual
/// test pages are left out entirely.
#[allow(dead_code)]
pub fn tree_root(path: &str) -> Option<String> {
    if path.ends_with(".html") || path.starts_with("/static/tests/") {
        return None;
    }
    if let Some((library, file)) = path
        .strip_prefix("/static/vendor/")
        .and_then(|rest| rest.split_once('/'))
        && !library.is_empty()
        && !file.is_empty()
    {
        return Some(format!("/static/vendor/{library}"));
    }
    if path.starts_with("/static/") {
        return Some("/static".to_owned());
    }
    let rest = path.strip_prefix("/app/")?;
    let (app, rest) = rest.split_once('/')?;
    let file = rest.strip_prefix("static/")?;
    (!app.is_empty() && !file.is_empty()).then(|| format!("/app/{app}/static"))
}

#[allow(dead_code)]
pub fn is_tree_hash(segment: &str) -> bool {
    segment.len() == TREE_HASH_LEN
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The address of a tree file under its tree's hash.
#[allow(dead_code)]
pub fn hashed_path(root: &str, hash: &str, path: &str) -> String {
    let relative = path
        .strip_prefix(root)
        .and_then(|rest| rest.strip_prefix('/'))
        .expect("path is inside its tree");
    format!("{root}/{hash}/{relative}")
}

/// Whether a path has the shape of a hashed tree address, whatever the hash.
#[allow(dead_code)]
pub fn has_tree_hash_shape(path: &str) -> bool {
    let candidates: [Option<&str>; 3] = [
        path.strip_prefix("/static/vendor/")
            .and_then(|rest| rest.split_once('/'))
            .map(|(_, rest)| rest),
        path.strip_prefix("/static/"),
        path.strip_prefix("/app/")
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(_, rest)| rest.strip_prefix("static/")),
    ];
    candidates.into_iter().flatten().any(|rest| {
        rest.split_once('/')
            .is_some_and(|(segment, file)| is_tree_hash(segment) && !file.is_empty())
    })
}

/// A tree's hash: SHA-256 over each file's path inside the tree and the SHA-256
/// of the bytes served for it, in path order.
#[allow(dead_code)]
pub fn tree_hash<'a>(root: &str, files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    use sha2::{Digest, Sha256};
    let mut entries: Vec<(String, String)> = files
        .into_iter()
        .map(|(path, bytes)| {
            let relative = path
                .strip_prefix(root)
                .and_then(|rest| rest.strip_prefix('/'))
                .expect("file is inside its tree")
                .to_owned();
            (relative, format!("{:x}", Sha256::digest(bytes)))
        })
        .collect();
    entries.sort();
    let mut digest = Sha256::new();
    for (relative, file_hash) in entries {
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(file_hash.as_bytes());
        digest.update(b"\n");
    }
    let hash = format!("{:x}", digest.finalize());
    hash[..TREE_HASH_LEN].to_owned()
}

/// Byte ranges of every static reference in a text asset: `src=` and `href=`
/// attribute values, CSS `url()` values, and quoted absolute static paths.
#[allow(dead_code)]
pub fn reference_spans(source: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if let Some(span) = attribute_reference(bytes, index)
            .or_else(|| url_reference(bytes, index))
            .or_else(|| quoted_static_reference(bytes, index))
        {
            spans.push(span);
            index = span.1;
        } else {
            index += 1;
        }
    }
    spans
}

fn starts_with_ignore_case(bytes: &[u8], index: usize, word: &[u8]) -> bool {
    bytes
        .get(index..index + word.len())
        .is_some_and(|slice| slice.eq_ignore_ascii_case(word))
}

fn skip_space(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn attribute_reference(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    if index > 0 && is_word(bytes[index - 1]) {
        return None;
    }
    let name = [b"src".as_slice(), b"href".as_slice()]
        .into_iter()
        .find(|name| starts_with_ignore_case(bytes, index, name))?;
    let mut cursor = skip_space(bytes, index + name.len());
    if bytes.get(cursor) != Some(&b'=') {
        return None;
    }
    cursor = skip_space(bytes, cursor + 1);
    if !matches!(bytes.get(cursor), Some(b'"' | b'\'')) {
        return None;
    }
    let start = cursor + 1;
    let end = start + bytes[start..].iter().position(|b| matches!(b, b'"' | b'\''))?;
    (end > start).then_some((start, end))
}

fn url_reference(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    if !starts_with_ignore_case(bytes, index, b"url(") {
        return None;
    }
    let mut cursor = skip_space(bytes, index + 4);
    if matches!(bytes.get(cursor), Some(b'"' | b'\'')) {
        cursor += 1;
    }
    let start = cursor;
    while bytes
        .get(cursor)
        .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b')' | b'"' | b'\''))
    {
        cursor += 1;
    }
    let end = cursor;
    if end == start {
        return None;
    }
    if matches!(bytes.get(cursor), Some(b'"' | b'\'')) {
        cursor += 1;
    }
    cursor = skip_space(bytes, cursor);
    (bytes.get(cursor) == Some(&b')')).then_some((start, end))
}

fn quoted_static_reference(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    if !matches!(bytes[index], b'"' | b'\'' | b'`') {
        return None;
    }
    let start = index + 1;
    let quote = |b: &u8| matches!(b, b'"' | b'\'' | b'`');
    let rest = &bytes[start..];
    let prefix = if rest.starts_with(b"/static/") {
        8
    } else if rest.starts_with(b"/app/") {
        let app = rest[5..]
            .iter()
            .position(|b| *b == b'/' || quote(b))
            .filter(|length| *length > 0)?;
        if !rest[5 + app..].starts_with(b"/static/") {
            return None;
        }
        5 + app + 8
    } else {
        return None;
    };
    let length = rest[prefix..].iter().position(quote)?;
    (length > 0).then_some((start, start + prefix + length))
}

/// The served path a reference names from `base`, without query or fragment.
/// Returns `None` for anything that is not a same-origin path.
#[allow(dead_code)]
pub fn resolve_reference(base: &str, reference: &str) -> Option<String> {
    let reference = reference.split(['?', '#']).next().unwrap_or_default();
    if reference.is_empty() || reference.contains(':') || reference.starts_with("//") {
        return None;
    }
    let joined = if reference.starts_with('/') {
        reference.to_owned()
    } else {
        format!("{}/{reference}", base.rsplit_once('/')?.0)
    };
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    Some(format!("/{}", parts.join("/")))
}

/// Apply byte-range replacements in order. Each edit is (start, end, text).
#[allow(dead_code)]
pub fn apply_edits(source: &[u8], edits: &[(usize, usize, &str)]) -> Vec<u8> {
    let mut output = Vec::with_capacity(source.len() + edits.len() * (TREE_HASH_LEN + 1));
    let mut cursor = 0;
    for (start, end, text) in edits {
        output.extend_from_slice(&source[cursor..*start]);
        output.extend_from_slice(text.as_bytes());
        cursor = *end;
    }
    output.extend_from_slice(&source[cursor..]);
    output
}
