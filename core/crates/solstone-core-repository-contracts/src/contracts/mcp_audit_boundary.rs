// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

const APPROVED_DEPENDENCIES: [&str; 4] =
    ["chrono", "serde", "serde_json", "solstone-core-journal-io"];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn dependency_names(document: &DocumentMut, section: &str) -> BTreeSet<String> {
    document
        .get(section)
        .and_then(Item::as_table)
        .map(|table| table.iter().map(|(name, _)| name.to_owned()).collect())
        .unwrap_or_default()
}

fn manifest_violations(manifest: &Path) -> Vec<String> {
    let text = fs::read_to_string(manifest).expect("MCP audit manifest reads");
    let document = text
        .parse::<DocumentMut>()
        .unwrap_or_else(|error| panic!("parse {}: {error}", manifest.display()));
    let expected: BTreeSet<_> = APPROVED_DEPENDENCIES
        .into_iter()
        .map(str::to_owned)
        .collect();
    let dependencies = dependency_names(&document, "dependencies");
    let development = dependency_names(&document, "dev-dependencies");
    let mut violations = Vec::new();
    if dependencies != expected {
        violations.push(format!(
            "{} dependencies were {dependencies:?}, expected {expected:?}",
            manifest.display()
        ));
    }
    if !development.is_empty() {
        violations.push(format!(
            "{} has unapproved dev-dependencies: {development:?}",
            manifest.display()
        ));
    }
    violations
}

fn public_write_surface_is_coordinate_only(source: &str) -> bool {
    let Some(write) = source.find("pub fn write_interaction_record(") else {
        return false;
    };
    let signature_end = source[write..]
        .find("{")
        .map_or(source.len(), |offset| write + offset);
    let signature = &source[write..signature_end];
    let Some(coordinates) = source.find("pub struct AuditCoordinates {") else {
        return false;
    };
    let definition_end = source[coordinates..]
        .find("}\n")
        .map_or(source.len(), |offset| coordinates + offset);
    let definition = &source[coordinates..definition_end];

    signature.contains("Result<AuditCoordinates, AuditWriteError>")
        && !signature.contains("InteractionRecord")
        && !definition.contains("InteractionRecord")
}

#[test]
fn mcp_audit_boundary_has_only_approved_dependencies_and_coordinate_output() {
    let root = repository_root();
    let crate_root = root.join("core/crates/solstone-core-mcp-audit");
    let manifest = crate_root.join("Cargo.toml");
    let violations = manifest_violations(&manifest);
    assert!(
        violations.is_empty(),
        "MCP audit must remain a dependency-pure leaf: {violations:?}"
    );

    let source = fs::read_to_string(crate_root.join("src/lib.rs")).expect("MCP audit source reads");
    assert!(
        public_write_surface_is_coordinate_only(&source),
        "MCP audit writes must return only AuditCoordinates, never interaction contents"
    );
}

#[test]
fn mcp_audit_boundary_rejects_the_seeded_forbidden_dependency_fixture() {
    let fixture = repository_root().join(
        "core/crates/solstone-core-repository-contracts/src/contracts/fixtures/mcp_audit/forbidden_dependency.Cargo.toml",
    );
    assert!(
        !manifest_violations(&fixture).is_empty(),
        "the seeded forbidden-dependency fixture must prove this guard rejects a network-capable edge"
    );
}

#[test]
fn coordinate_surface_rejects_a_record_return_type() {
    let fixture = "pub struct AuditCoordinates { pub record: InteractionRecord }\n\
                   pub fn write_interaction_record() -> Result<InteractionRecord, AuditWriteError> {\n\
                   unreachable!()\n\
                   }\n";
    assert!(
        !public_write_surface_is_coordinate_only(fixture),
        "the source guard must reject a public record-returning write surface"
    );
}
