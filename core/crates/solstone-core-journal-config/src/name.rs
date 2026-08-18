// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// Return whether a trimmed value looks like a filesystem path.
///
/// True when the trimmed value contains `/` or `\`, or starts with `~`.
/// A path-shaped agent name is emitted verbatim as a chat speaker label,
/// so write paths refuse it and read paths treat it as missing.
pub fn is_path_shaped_name(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.contains('/') || trimmed.contains('\\') || trimmed.starts_with('~')
}

#[cfg(test)]
mod tests {
    use super::is_path_shaped_name;

    #[test]
    fn path_shaped_names_match_the_trim_slash_and_tilde_rule() {
        for value in ["~/x", "  ~/x", "a/b", "a\\b", "~"] {
            assert!(is_path_shaped_name(value), "{value:?}");
        }
        for value in ["Ada", "sol", "foo~bar", "", "   "] {
            assert!(!is_path_shaped_name(value), "{value:?}");
        }
    }
}
