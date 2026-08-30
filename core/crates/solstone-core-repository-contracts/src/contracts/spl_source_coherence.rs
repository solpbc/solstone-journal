// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, Value};

const SPL_DEPENDENCIES: [&str; 3] = ["spl-core", "spl-home", "spl-transport"];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn workspace_dependencies(document: &DocumentMut) -> Result<&Table, String> {
    document
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Item::as_table)
        .ok_or_else(|| "workspace dependencies table is missing".to_owned())
}

fn required_string(
    table: &toml_edit::InlineTable,
    key: &str,
    name: &str,
) -> Result<String, String> {
    table
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{name} must declare a string {key} source"))
}

fn assert_spl_source_coherence(text: &str) -> Result<(), String> {
    let document = text
        .parse::<DocumentMut>()
        .map_err(|error| format!("workspace manifest is not TOML: {error}"))?;
    let dependencies = workspace_dependencies(&document)?;
    let mut source: Option<(String, String)> = None;

    for name in SPL_DEPENDENCIES {
        let table = dependencies
            .get(name)
            .and_then(Item::as_value)
            .and_then(Value::as_inline_table)
            .ok_or_else(|| format!("{name} must be an inline dependency table"))?;
        if table.contains_key("rev") || table.contains_key("branch") || table.contains_key("path") {
            return Err(format!("{name} must use only a git URL and shared tag"));
        }
        let git = required_string(table, "git", name)?;
        let tag = required_string(table, "tag", name)?;
        if let Some((expected_git, expected_tag)) = &source {
            if git != *expected_git || tag != *expected_tag {
                return Err(format!(
                    "{name} source must match the other SPL dependencies"
                ));
            }
        } else {
            source = Some((git, tag));
        }
    }
    Ok(())
}

#[test]
fn spl_sources_are_coherent_in_the_workspace_manifest() {
    let manifest = repository_root().join("core/Cargo.toml");
    let text = fs::read_to_string(&manifest).expect("workspace manifest reads");
    assert_spl_source_coherence(&text).expect("SPL sources are coherently pinned");
}

fn coherent_fixture() -> String {
    r#"
[workspace.dependencies]
spl-core = { git = "https://example.invalid/spl", tag = "shared" }
spl-home = { git = "https://example.invalid/spl", tag = "shared" }
spl-transport = { git = "https://example.invalid/spl", tag = "shared" }
"#
    .to_owned()
}

#[test]
fn spl_source_contract_rejects_invalid_source_shapes_and_divergence() {
    for (needle, replacement) in [
        ("tag = \"shared\"", "rev = \"deadbeef\""),
        ("tag = \"shared\"", "branch = \"main\""),
        ("tag = \"shared\"", "tag = \"other\""),
        (
            "git = \"https://example.invalid/spl\"",
            "git = \"https://example.invalid/other\"",
        ),
    ] {
        let fixture = coherent_fixture().replacen(needle, replacement, 1);
        assert!(
            assert_spl_source_coherence(&fixture).is_err(),
            "fixture replacing {needle:?} with {replacement:?} must be rejected"
        );
    }
}
