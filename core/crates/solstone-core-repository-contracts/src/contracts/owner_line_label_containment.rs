// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keep the owner's line answers out of voice and evidence code.
//!
//! Owner-voice mode removes audio based on voice evidence. The owner's own
//! answers about who spoke a line (`talents/owner_line_labels.json`) are not
//! voice evidence, so only the code listed here may name the file, its path
//! constant, its path helper or its readers. Readers that show who spoke get
//! the answers only through the who-spoke loader, and only the readers listed
//! here may call it; nothing else is pre-allowed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The tokens that reach the file.
const TOKENS: [&str; 5] = [
    "owner_line_labels.json",
    "OWNER_LINE_LABELS_FILE",
    "owner_line_labels_path",
    "read_owner_line_labels",
    "decode_owner_line_labels",
];

/// Production sources allowed to reach the file, relative to `core/crates`.
const ALLOWED: [&str; 8] = [
    // The store itself: format and read, then set, remove and restore.
    "solstone-core-speaker-id/src/owner_line_labels.rs",
    "solstone-core-speaker-resolve/src/owner_line_labels.rs",
    // Its durability registration.
    "solstone-core-journal-io/src/durability.rs",
    // The speaker-artifact wipe, which leaves the answers in place.
    "solstone-core-speaker-resolve/src/artifact_wipe.rs",
    // Entity merge rewrites, rollback and interrupted-merge recovery.
    "solstone-core-entity/src/store/merge.rs",
    "solstone-core-entity/src/store/merge_rollback.rs",
    // Entity reference scans.
    "solstone-core-facets/src/store/reference_scan.rs",
    // The who-spoke loader.
    "solstone-core-transcripts/src/who_spoke.rs",
];

/// The tokens that reach the who-spoke loader.
const LOADER_TOKENS: [&str; 1] = ["load_who_spoke"];

/// Production sources allowed to call the who-spoke loader: the loader itself
/// and the readers that show who spoke, relative to `core/crates`.
const LOADER_ALLOWED: [&str; 6] = [
    "solstone-core-transcripts/src/who_spoke.rs",
    // The talent transcript view and the owner's own lines.
    "solstone-core-transcripts/src/lib.rs",
    // The agent and connected-agent segment page.
    "solstone-core-transcripts/src/segment_page.rs",
    // The transcript page's speaker chips.
    "solstone-core-transcripts-web/src/segment_speakers.rs",
    // Participation.
    "solstone-core-talent-runtime/src/participation.rs",
    // Linked sharing.
    "solstone-core-linked-share/src/source.rs",
];

fn is_test_source(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    path.components()
        .any(|component| matches!(component.as_os_str().to_str(), Some("tests" | "benches")))
        || name == "tests.rs"
        || name.ends_with("_tests.rs")
        || name.contains("test_support")
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                collect(&path, files);
            }
        } else if name.ends_with(".rs") {
            files.push(path);
        }
    }
}

/// The production part of a source: every item not compiled only for tests.
/// A file that doesn't parse is taken whole.
fn production_part(source: &str) -> String {
    fn retain(items: &mut Vec<syn::Item>) {
        items.retain(|item| !crate::owner_origin_host::is_test_item(item));
        for item in items {
            match item {
                syn::Item::Mod(module) => {
                    if let Some((_, items)) = &mut module.content {
                        retain(items);
                    }
                }
                syn::Item::Impl(block) => block
                    .items
                    .retain(|item| !crate::owner_origin_host::is_test_impl_item(item)),
                _ => {}
            }
        }
    }
    match syn::parse_file(source) {
        Ok(mut file) => {
            retain(&mut file.items);
            quote::quote!(#file).to_string()
        }
        Err(_) => source.to_owned(),
    }
}

/// Every production source under `crates` that names one of `tokens`,
/// relative to `crates`. This census never scans its own crate.
fn reaching_sources(crates: &Path, tokens: &[&str]) -> BTreeSet<String> {
    let mut files = Vec::new();
    collect(crates, &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(crates).ok()?.to_path_buf();
            if relative.starts_with("solstone-core-repository-contracts")
                || is_test_source(&relative)
            {
                return None;
            }
            let source = fs::read_to_string(&path).ok()?;
            let production = production_part(&source);
            let production = production.as_str();
            tokens
                .iter()
                .any(|token| production.contains(token))
                .then(|| relative.to_string_lossy().replace('\\', "/"))
        })
        .collect()
}

/// What breaks the census: a source that reaches the file or calls the
/// loader without being allowed, or an allowed source that no longer does.
fn census_violations(crates: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    for (tokens, allowed, what) in [
        (
            &TOKENS[..],
            &ALLOWED[..],
            "reaches the owner's line answers",
        ),
        (
            &LOADER_TOKENS[..],
            &LOADER_ALLOWED[..],
            "calls the who-spoke loader",
        ),
    ] {
        let reaching = reaching_sources(crates, tokens);
        let allowed = allowed.iter().map(|path| (*path).to_owned()).collect();
        violations.extend(
            reaching
                .difference(&allowed)
                .map(|path| format!("{path} {what} but isn't allowed to")),
        );
        violations.extend(
            allowed
                .difference(&reaching)
                .map(|path| format!("{path} is allowed but no longer {what}; remove it")),
        );
    }
    violations
}

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .to_path_buf()
}

#[test]
fn only_allowed_code_reaches_the_owners_line_answers() {
    let violations = census_violations(&crates_dir());
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn the_census_fails_on_an_unlisted_reference_and_ignores_tests() {
    let root = tempfile::tempdir().unwrap();
    let crates = root.path();
    for allowed in ALLOWED {
        let path = crates.join(allowed);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "const FILE: &str = \"owner_line_labels.json\";\n").unwrap();
    }
    for allowed in LOADER_ALLOWED {
        let path = crates.join(allowed);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut contents = fs::read_to_string(&path).unwrap_or_default();
        contents.push_str("use x::load_who_spoke;\n");
        fs::write(path, contents).unwrap();
    }
    let write = |relative: &str, contents: &str| {
        let path = crates.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    };
    write(
        "solstone-core-voice/src/evidence.rs",
        "fn evidence() {}\n#[cfg(test)]\nmod tests { const F: &str = \"owner_line_labels.json\"; }\n",
    );
    write(
        "solstone-core-voice/src/evidence_tests.rs",
        "use x::read_owner_line_labels;\n",
    );
    write(
        "solstone-core-voice/tests/contract.rs",
        "use x::owner_line_labels_path;\n",
    );
    assert_eq!(census_violations(crates), Vec::<String>::new());

    write(
        "solstone-core-voice/src/evidence.rs",
        "use solstone_core_speaker_id::owner_line_labels::read_owner_line_labels;\n",
    );
    assert_eq!(
        census_violations(crates),
        vec![
            "solstone-core-voice/src/evidence.rs reaches the owner's line answers but isn't allowed to"
                .to_owned()
        ]
    );

    write(
        "solstone-core-voice/src/evidence.rs",
        "fn evidence(dir: &Path) { let _ = solstone_core_transcripts::load_who_spoke(dir, None, None); }\n",
    );
    assert_eq!(
        census_violations(crates),
        vec![
            "solstone-core-voice/src/evidence.rs calls the who-spoke loader but isn't allowed to"
                .to_owned()
        ]
    );

    write("solstone-core-voice/src/evidence.rs", "fn evidence() {}\n");
    write("solstone-core-entity/src/store/merge.rs", "fn merge() {}\n");
    assert_eq!(census_violations(crates).len(), 1);
    write(
        "solstone-core-linked-share/src/source.rs",
        "fn segment_labels() {}\n",
    );
    assert_eq!(census_violations(crates).len(), 2);
}
