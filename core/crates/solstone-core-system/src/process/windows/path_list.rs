// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows PATH-list parsing used by executable resolution.

const QUOTE: u16 = b'"' as u16;
const SEMICOLON: u16 = b';' as u16;

/// Split a Windows PATH value into its unquoted components.
///
/// Derived from Rust 1.97.1 `sys/paths/windows.rs::split_paths`.
pub(super) fn split_windows_paths(raw: &[u16]) -> Vec<Vec<u16>> {
    let mut paths = Vec::new();
    let mut component = Vec::new();
    let mut in_quote = false;

    for &unit in raw {
        match unit {
            QUOTE => in_quote = !in_quote,
            SEMICOLON if !in_quote => {
                paths.push(std::mem::take(&mut component));
            }
            _ => component.push(unit),
        }
    }
    paths.push(component);
    paths
}

#[cfg(test)]
mod tests {
    use super::split_windows_paths;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    #[test]
    fn quoted_semicolons_are_literal_and_quotes_are_removed() {
        assert_eq!(
            split_windows_paths(&wide(r#"C:\one;"C:\two;three";C:\four"#)),
            vec![wide(r"C:\one"), wide(r"C:\two;three"), wide(r"C:\four")]
        );
    }

    #[test]
    fn paired_empty_quotes_produce_an_empty_component() {
        assert_eq!(
            split_windows_paths(&wide(r#"C:\one;"";C:\two"#)),
            vec![wide(r"C:\one"), Vec::new(), wide(r"C:\two")]
        );
    }

    #[test]
    fn unmatched_quote_makes_later_semicolons_literal() {
        assert_eq!(
            split_windows_paths(&wide(r#"C:\one;"C:\two;C:\three"#)),
            vec![wide(r"C:\one"), wide(r"C:\two;C:\three")]
        );
    }

    #[test]
    fn preserves_empty_components_for_the_resolver_to_skip() {
        assert_eq!(
            split_windows_paths(&wide(r#";C:\one;;"";"#)),
            vec![
                Vec::new(),
                wide(r"C:\one"),
                Vec::new(),
                Vec::new(),
                Vec::new()
            ]
        );
    }
}
