// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

/// Extract the `SOL_BIN` target from a managed source-install wrapper.
///
/// This intentionally recognizes only the marker grammar emitted by
/// `install_guard.py`: a version-1 through version-8 marker and a
/// single-quoted `SOL_BIN` assignment with shell-style embedded quote escapes.
pub(crate) fn parse_sol_bin(content: &str) -> Option<PathBuf> {
    let has_marker = content.lines().any(|line| {
        line.strip_prefix("# managed-version: ")
            .is_some_and(|version| matches!(version.as_bytes(), [b'1'..=b'8']))
    });
    if !has_marker {
        return None;
    }
    let value = content.lines().find_map(|line| {
        line.strip_prefix("SOL_BIN='")
            .and_then(|value| value.strip_suffix('\''))
    })?;
    unescape_single_quoted(value).map(PathBuf::from)
}

pub(crate) fn resolve_non_strict(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| lexical_absolute(path))
}

fn unescape_single_quoted(value: &str) -> Option<String> {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\'' {
            output.push(character);
            continue;
        }
        if characters.next() != Some('\\')
            || characters.next() != Some('\'')
            || characters.next() != Some('\'')
        {
            return None;
        }
        output.push('\'');
    }
    Some(output)
}

fn lexical_absolute(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_marked_wrappers_and_unescapes_sol_bin() {
        assert_eq!(
            parse_sol_bin("# managed-version: 7\nSOL_BIN='/tmp/it'\\''s/bin/journal'\n"),
            Some(PathBuf::from("/tmp/it's/bin/journal"))
        );
        assert_eq!(parse_sol_bin("SOL_BIN='/tmp/bin/journal'\n"), None);
        assert_eq!(
            parse_sol_bin("# managed-version: 8\nSOL_BIN='/tmp/bin/journal'\n"),
            Some(PathBuf::from("/tmp/bin/journal"))
        );
        assert_eq!(
            parse_sol_bin("# managed-version: 9\nSOL_BIN='/tmp/bin/journal'\n"),
            None
        );
    }
}
