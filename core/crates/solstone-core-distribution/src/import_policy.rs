// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Ordinary-import membership and allowlist checks over a PE import report.

use crate::pe::ImportedLibrary;

#[must_use]
pub fn is_ordinary_import(imports: &[ImportedLibrary], name: &str) -> bool {
    imports
        .iter()
        .any(|lib| lib.name.eq_ignore_ascii_case(name))
}

#[must_use]
pub fn disallowed_imports<'a>(imports: &'a [ImportedLibrary], allowlist: &[&str]) -> Vec<&'a str> {
    imports
        .iter()
        .filter(|lib| {
            !allowlist
                .iter()
                .any(|allowed| lib.name.eq_ignore_ascii_case(allowed))
        })
        .map(|lib| lib.name.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(name: &str) -> ImportedLibrary {
        ImportedLibrary {
            name: name.to_string(),
            symbols: vec![],
        }
    }

    #[test]
    fn ordinary_import_is_case_insensitive() {
        let imports = [library("KERNEL32.dll")];
        assert!(is_ordinary_import(&imports, "kernel32.dll"));
    }

    #[test]
    fn absent_name_is_not_an_ordinary_import() {
        let imports = [library("KERNEL32.dll")];
        assert!(!is_ordinary_import(&imports, "user32.dll"));
    }

    #[test]
    fn empty_symbols_row_still_counts_as_present() {
        let imports = [library("empty.dll")];
        assert!(imports[0].symbols.is_empty());
        assert!(is_ordinary_import(&imports, "empty.dll"));
    }

    #[test]
    fn disallowed_imports_preserve_original_casing_and_ignore_allowlist_case() {
        let imports = [library("KERNEL32.dll"), library("evil.dll")];
        assert_eq!(
            disallowed_imports(&imports, &["kernel32.dll"]),
            vec!["evil.dll"]
        );
    }

    #[test]
    fn fully_allowlisted_imports_are_empty() {
        let imports = [library("KERNEL32.dll"), library("user32.dll")];
        assert!(disallowed_imports(&imports, &["kernel32.dll", "USER32.dll"]).is_empty());
    }

    #[test]
    fn empty_allowlist_returns_every_import_name() {
        let imports = [library("KERNEL32.dll"), library("evil.dll")];
        assert_eq!(
            disallowed_imports(&imports, &[]),
            vec!["KERNEL32.dll", "evil.dll"]
        );
    }

    #[test]
    fn empty_imports_return_empty() {
        assert!(disallowed_imports(&[], &["kernel32.dll"]).is_empty());
    }
}
