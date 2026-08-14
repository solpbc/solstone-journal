// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Guard: names are compared under one normalization, and exceptions are named.
//!
//! Name comparison is unified on `NFKC -> collapse whitespace -> full Unicode
//! case fold`. Plain lowercasing is not that: it leaves `Straße` and `STRASSE`
//! distinct, along with every ligature, the Greek final sigma, and the Cherokee
//! syllabary. A comparison that lowercases therefore decides two spellings of
//! one person are two people -- and the surfaces that do it were each found by
//! someone reading the code, three times over.
//!
//! Not every lowercase call is a defect. Emails are stored lowercased on
//! purpose, the slug pipeline lowercases as part of building a label, and some
//! dedupe keys reproduce the reference's own. The allowlist separates those from
//! the ones that are genuinely deferred, so a deferred one stays visible instead
//! of reading as settled.

#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};

/// Files permitted to lowercase, and why it is not a name comparison there.
///
/// A `deferred` reason means the site *is* a name comparison still on plain
/// lowercasing, kept for reference parity and owed a decision -- not settled.
const ALLOWED: &[(&str, &str)] = &[
    (
        "solstone-core-entity-matching/src/slug.rs",
        "the slug pipeline lowercases while building a filesystem label; it compares nothing",
    ),
    (
        "solstone-core-entity-matching/src/batch.rs",
        "the email tier, which the reference compares plain-lowercased on purpose and a \
         committed test pins",
    ),
    (
        "solstone-core-entity/src/store/create.rs",
        "emails are persisted lowercased, matching the reference's stored form",
    ),
    (
        "solstone-core-entity/src/store/merge.rs",
        "alias and email dedupe keys, reproducing the reference's own dedupe",
    ),
    (
        "solstone-core-entity/src/archive_dedupe.rs",
        "narrow archive alias and email dedupe keys, deliberately reproducing merge reference semantics",
    ),
    (
        "solstone-core-entity/src/store/undo.rs",
        "the inverse of that same dedupe, so undo removes exactly what merge added",
    ),
    (
        "solstone-core-facets/src/store/legacy_entity_migration.rs",
        "the retired one-time facet-entity migration's own merge keys, which decided historical \
         canonical grouping on plain `.lower()`; changing the comparison would change which \
         legacy records merge, so it reproduces the reference exactly",
    ),
    (
        "solstone-core-facets/src/store/seeding.rs",
        "the email lookup and a title-casing helper; the email half is pinned by a committed \
         test asserting it does NOT full-case-fold",
    ),
    (
        "solstone-core-entity/src/store/derived.rs",
        "DEFERRED -- this one IS a name comparison, matching a configured identity name \
         against an entity name and its aliases on plain lowercasing. Kept for reference \
         parity and owed a decision: unify it and measure what changes, or declare the \
         divergence. It is listed here so it stays visible, not because it is settled",
    ),
    (
        "solstone-core-entities/src/router.rs",
        "classifies free-text store-error messages and boolean-ish request values; it never \
         compares entity names",
    ),
    (
        "solstone-core-facets/src/store/review_candidates.rs",
        "the facet-slug pipeline lowercases while building a filesystem label; it compares \
         nothing",
    ),
];

fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate directory has a parent")
        .to_path_buf()
}

fn collect(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// Store and route crates that may compare entity names.
const GUARDED_CRATES: &[&str] = &[
    "solstone-core-entity",
    "solstone-core-entity-matching",
    "solstone-core-facets",
    "solstone-core-entities",
];

fn shipping_sources() -> Vec<PathBuf> {
    let root = crates_root();
    let mut found = Vec::new();
    for crate_name in GUARDED_CRATES {
        let source = root.join(crate_name).join("src");
        if source.is_dir() {
            collect(&source, &mut found);
        }
    }
    found.retain(|path| {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        !name.ends_with("_tests.rs") && name != "comparison_guard.rs" && name != "slug_use_guard.rs"
    });
    found.sort();
    found
}

fn relative(path: &Path) -> String {
    path.strip_prefix(crates_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn lowercases(source: &str) -> bool {
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .any(|line| line.contains("to_lowercase()") || line.contains("to_ascii_lowercase()"))
}

#[test]
fn every_lowercase_comparison_is_allowlisted_with_a_reason() {
    let mut seen: Vec<String> = Vec::new();
    for path in shipping_sources() {
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        if !lowercases(&source) {
            continue;
        }
        let file = relative(&path);
        assert!(
            ALLOWED.iter().any(|(allowed, _)| *allowed == file),
            "{file} lowercases a string and is not allowlisted.\n\
             Names are compared under NFKC + whitespace collapse + full Unicode case fold. \
             Plain lowercasing leaves two spellings of one person distinct. If this is a \
             name comparison, use the unified normalization. If it is an email, a label, or \
             a dedupe key reproducing the reference, add it to ALLOWED with that reason."
        );
        seen.push(file);
    }
    seen.sort();
    seen.dedup();
    let mut expected: Vec<String> = ALLOWED.iter().map(|(file, _)| (*file).to_owned()).collect();
    expected.sort();
    assert_eq!(
        seen, expected,
        "the allowlist must name exactly the files that lowercase -- a stale entry hides a \
         surface that stopped, and an unread reason stops being a decision"
    );
    assert!(
        ALLOWED.iter().all(|(_, reason)| reason.len() > 30),
        "every entry states why lowercasing is correct, or that it is deferred"
    );
}

#[test]
fn a_deferred_name_comparison_is_still_declared() {
    let deferred = ALLOWED
        .iter()
        .filter(|(_, reason)| reason.starts_with("DEFERRED"))
        .count();
    assert!(
        deferred <= 1,
        "more than one name comparison is deferred; each needs its own decision rather than \
         accumulating in the allowlist"
    );
}
