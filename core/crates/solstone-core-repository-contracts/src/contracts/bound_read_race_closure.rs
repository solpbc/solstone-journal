// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keep descriptor-bound read hardening, caller coverage, and feature gates aligned.

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

const JOURNAL_IO_TEST_HOOKS: &str = "solstone-core-journal-io/test-hooks";
const SOL_LINK_TEST_HOOKS: &str = "solstone-core-sol-link/test-hooks";
const OPTIONAL_JOURNAL_IO_TEST_HOOKS: &str = "solstone-core-journal-io?/test-hooks";

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn read_repo_file(relative: &str) -> String {
    let path = repository_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn production_source(source: &str) -> &str {
    source
        .split_once("\n#[cfg(test)]\nmod tests")
        .map(|(production, _)| production)
        .unwrap_or(source)
}

fn parse_manifest(relative: &str) -> DocumentMut {
    read_repo_file(relative)
        .parse::<DocumentMut>()
        .unwrap_or_else(|error| panic!("parse {relative}: {error}"))
}

fn feature_members(document: &DocumentMut, feature: &str) -> Option<Vec<String>> {
    document
        .get("features")
        .and_then(Item::as_table)
        .and_then(|features| features.get(feature))
        .and_then(Item::as_array)
        .map(|members| {
            members
                .iter()
                .filter_map(toml_edit::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
}

fn target_dev_dependencies(document: &DocumentMut) -> &Table {
    document["target"]["cfg(unix)"]["dev-dependencies"]
        .as_table()
        .expect("Unix dev-dependencies table exists")
}

fn inline_feature_members(item: &Item) -> Vec<String> {
    item.as_inline_table()
        .and_then(|dependency| dependency.get("features"))
        .and_then(toml_edit::Value::as_array)
        .expect("dependency feature array exists")
        .iter()
        .filter_map(toml_edit::Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn test_table<'a>(document: &'a DocumentMut, name: &str) -> &'a Table {
    document
        .get("test")
        .and_then(Item::as_array_of_tables)
        .expect("test target array exists")
        .iter()
        .find(|table| table.get("name").and_then(Item::as_str) == Some(name))
        .unwrap_or_else(|| panic!("{name} test target exists"))
}

#[test]
fn bound_read_callers_cover_the_five_committed_leaf_paths() {
    let config_source = read_repo_file("core/crates/solstone-core-journal-config/src/read.rs");
    let committed_source = read_repo_file("core/crates/solstone-core-sol-link/src/committed.rs");
    let config = production_source(&config_source);
    let committed = production_source(&committed_source);

    assert_eq!(
        config.matches("read_bytes_bound(").count()
            + committed.matches("read_bytes_bound(").count(),
        3,
        "exactly the config, required-link-file, and optional-link-string callers use bound bytes"
    );

    assert!(
        config.contains("root.canonical_path().join(\"config/journal.json\")")
            && config.contains("read_bytes_bound(&config_directory, OsStr::new(\"journal.json\"))"),
        "the config caller remains bound to config/journal.json"
    );
    assert!(
        committed.contains("link_path.join(\"ca/cert.pem\")")
            && committed.contains("read_required_bound_file(&ca, OsStr::new(\"cert.pem\"))"),
        "the required-file caller remains bound to link/ca/cert.pem"
    );
    assert!(
        committed.contains("link_path.join(\"ca/private.pem\")")
            && committed.contains("read_required_bound_file(&ca, OsStr::new(\"private.pem\"))"),
        "the required-file caller remains bound to link/ca/private.pem"
    );
    assert!(
        committed.contains("link_path.join(\"state.json\")")
            && committed.contains("read_optional_bound_string(link, OsStr::new(\"state.json\"))"),
        "the optional-string caller remains bound to link/state.json"
    );
    assert!(
        committed.contains("ca_path.join(\"state.json\")")
            && committed.contains("read_optional_bound_string(ca, OsStr::new(\"state.json\"))"),
        "the optional-string caller remains bound to link/ca/state.json"
    );
}

#[test]
fn bound_reader_retains_the_six_step_identity_stability_protocol() {
    let source = read_repo_file("core/crates/solstone-core-journal-io/src/readers.rs");
    let implementation = source
        .split_once("pub fn read_bytes_bound(")
        .map(|(_, body)| body)
        .and_then(|body| body.split_once("\n}\n\n#[cfg(unix)]\nfn bound_read_identity_changed"))
        .map(|(body, _)| body)
        .expect("bound reader implementation is delimited");
    for primitive in [
        "InitialNameObserve",
        "Open",
        "OpenedHandleObserve",
        "Read",
        "FinalHandleObserve",
        "FinalNameObserve",
    ] {
        assert!(
            implementation.contains(&format!(
                "checkpoint_error(BoundReadPrimitive::{primitive})"
            )),
            "bound reader retains {primitive} checkpoint"
        );
    }
    assert!(
        implementation.contains("OFlag::O_NONBLOCK"),
        "bound reader's no-follow open remains nonblocking"
    );
}

#[test]
fn bound_read_feature_closures_and_leaf_process_target_remain_narrow() {
    let core = parse_manifest("core/crates/solstone-core/Cargo.toml");
    assert_eq!(
        feature_members(&core, "test-hooks"),
        Some(vec!["solstone-core-system/test-hooks".to_owned()])
    );
    assert_eq!(
        feature_members(&core, "journal-mcp-endpoint"),
        Some(vec!["dep:solstone-core-mcp-endpoint".to_owned()])
    );

    let endpoint = parse_manifest("core/crates/solstone-core-mcp-endpoint/Cargo.toml");
    assert_eq!(
        feature_members(&endpoint, "test-hooks"),
        Some(vec![
            JOURNAL_IO_TEST_HOOKS.to_owned(),
            SOL_LINK_TEST_HOOKS.to_owned(),
        ])
    );
    let leaf_process = test_table(&endpoint, "mcp_endpoint_bound_leaf_process");
    assert_eq!(
        leaf_process.get("path").and_then(Item::as_str),
        Some("tests/mcp_endpoint_bound_leaf_process.rs")
    );
    let required = leaf_process
        .get("required-features")
        .and_then(Item::as_array)
        .expect("leaf process required features exist")
        .iter()
        .filter_map(toml_edit::Value::as_str)
        .collect::<Vec<_>>();
    assert_eq!(required, ["test-hooks"]);

    let sol_link = parse_manifest("core/crates/solstone-core-sol-link/Cargo.toml");
    assert_eq!(
        feature_members(&sol_link, "test-hooks"),
        Some(vec![OPTIONAL_JOURNAL_IO_TEST_HOOKS.to_owned()])
    );

    let journal_config = parse_manifest("core/crates/solstone-core-journal-config/Cargo.toml");
    assert!(
        journal_config.get("features").is_none(),
        "journal-config must not expose a feature table"
    );
    let dependency = target_dev_dependencies(&journal_config)
        .get("solstone-core-journal-io")
        .expect("journal-io is a Unix dev dependency");
    assert_eq!(inline_feature_members(dependency), ["test-hooks"]);
}
