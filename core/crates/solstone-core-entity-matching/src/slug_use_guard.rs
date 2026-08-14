// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Guard: a slug derived from a name may label a directory, never find one.
//!
//! Written identity means the directory an entity lives in is a label, not its
//! identity. The two diverge whenever a name changes after creation, and for
//! every name carrying one of the pinned transliteration divergences. So a slug
//! computed from a name is a correct way to *choose* a new directory and never a
//! correct way to *locate* an existing one -- a lookup keyed that way misses
//! entities that exist, and the caller then creates a duplicate or reports the
//! entity missing.
//!
//! This has been introduced independently on four separate surfaces, each time
//! caught only by a human reading the code. The allowlist below makes a new call
//! site fail loudly instead: adding one is a deliberate act with a stated reason.

#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};

/// Every call site permitted to derive a slug from a name, with why.
///
/// Add an entry only when the derived value **names something new** or is
/// compared, never when it is used to find something that already exists.
const ALLOWED: &[(&str, &str)] = &[
    (
        "solstone-core-entity-matching/src/matcher.rs",
        "the slug tier compares a derived query against stored ids -- comparison, not lookup",
    ),
    (
        "solstone-core-facets/src/store/detected_entities.rs",
        "the detected-entity day file has no written identity and no map; its row key is \
         derived by contract, and un-deriving it makes the upsert append instead of update",
    ),
    (
        "solstone-core-facets/src/store/facet_entities.rs",
        "names a NEW journal entity and a NEW relationship directory on the create path",
    ),
    (
        "solstone-core-facets/src/store/legacy_entity_migration.rs",
        "names a NEW journal entity directory and a NEW facet-relationship directory for each \
         canonical the one-time migration builds -- there is no prior identity to resolve, \
         because creating the first one is what this migration does",
    ),
    (
        "solstone-core-facets/src/store/facet_entity_move.rs",
        "read-compat fallback only, after resolution through the stored link identity fails \
         -- a facet directory can legitimately have no resolvable identity yet",
    ),
    (
        "solstone-core-facets/src/store/observations.rs",
        "read-compat fallback only, after stored facet identity resolution finds no match \
         -- legacy directory labels remain readable",
    ),
    (
        "solstone-core-facets/src/store/seeding.rs",
        "names a NEW entity id on the create path and a NEW facet-relationship directory when \
         attaching a resolved entity for the first time -- never used to look one up",
    ),
    (
        "solstone-core-speaker-resolve/src/identify_target.rs",
        "proposes a new-create id, then immediately checks the resolved identity map and \
         literal destination directory before returning -- refuses any collision before \
         Ready is produced; never used to look up or adopt an existing entity",
    ),
    (
        "solstone-core-entities/src/router.rs",
        "names merge-candidate key components and is a compatibility fallback only after a \
         stored attached-facet identity lookup fails -- never used to locate an existing entity",
    ),
    (
        "solstone-core-convey-shell/src/speakers_review.rs",
        "labels a synthetic principal entity from the principal_identity_or_none fallback when no \
         journal entity exists yet -- never used to look one up",
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

/// Source files that ship, excluding test modules and this guard.
fn shipping_sources() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let root = crates_root();
    let Ok(entries) = fs::read_dir(&root) else {
        return found;
    };
    for entry in entries.flatten() {
        let source = entry.path().join("src");
        if source.is_dir() {
            collect(&source, &mut found);
        }
    }
    found.retain(|path| {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        !name.ends_with("_tests.rs") && name != "slug_use_guard.rs" && name != "slug.rs"
    });
    found.sort();
    found
}

fn relative(path: &Path) -> String {
    let root = crates_root();
    path.strip_prefix(&root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// A call, not a doc comment or an import.
fn call_lines(source: &str) -> Vec<String> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("entity_slug("))
        .filter(|line| !line.starts_with("//") && !line.starts_with("use "))
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_slug_derivation_is_allowlisted_with_a_reason() {
    let mut seen: Vec<String> = Vec::new();
    let mut unallowlisted: Vec<String> = Vec::new();
    for path in shipping_sources() {
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        if call_lines(&source).is_empty() {
            continue;
        }
        let file = relative(&path);
        if !ALLOWED.iter().any(|(allowed, _)| *allowed == file) {
            unallowlisted.push(file);
            continue;
        }
        seen.push(file);
    }
    unallowlisted.sort();
    assert!(
        unallowlisted.is_empty(),
        "these files derive a slug from a name and are not allowlisted:\n- {}\n\
         A slug may LABEL a new directory or be COMPARED. It may never be used to \
         FIND an existing entity -- the directory is a label and diverges from the \
         written identity. Resolve through the identity map or the stored link \
         instead. If this really is a labelling or comparison use, add it to \
         ALLOWED with the reason.",
        unallowlisted.join("\n- ")
    );
    seen.sort();
    seen.dedup();
    let mut expected: Vec<String> = ALLOWED.iter().map(|(file, _)| (*file).to_owned()).collect();
    expected.sort();
    assert_eq!(
        seen, expected,
        "the allowlist must name exactly the files that derive a slug -- a stale entry \
         hides a surface that stopped deriving, and outdated reasons stop being read"
    );
    assert!(
        ALLOWED.iter().all(|(_, reason)| reason.len() > 30),
        "every entry states why deriving is correct there"
    );
}
