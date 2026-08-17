// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

#[test]
fn service_legacy_evidence_is_standalone_and_has_one_named_gate() {
    let root = repository_root();
    let workspace =
        fs::read_to_string(root.join("core/Cargo.toml")).expect("root Cargo manifest reads");
    let members = workspace
        .split_once("members = [")
        .and_then(|(_, tail)| tail.split_once("]\n"))
        .map(|(members, _)| members)
        .expect("workspace members block is recognizable");
    assert!(
        !members.contains("solstone-core-service-legacy-evidence"),
        "heavy evidence crate must not be a root workspace member"
    );

    assert!(
        workspace.contains("exclude = [\"crates/solstone-core-service-legacy-evidence\"]"),
        "root workspace must explicitly exclude the standalone evidence crate"
    );

    let standalone = root.join("core/crates/solstone-core-service-legacy-evidence/Cargo.toml");
    let standalone_text = fs::read_to_string(&standalone).expect("standalone manifest reads");
    assert!(standalone_text.contains("[workspace]"));
    assert!(!standalone_text.contains("workspace = true"));

    let makefile = fs::read_to_string(root.join("Makefile")).expect("Makefile reads");
    assert!(makefile.contains("check-service-legacy-evidence:"));
    assert!(makefile.contains(
        "cargo test --manifest-path core/crates/solstone-core-service-legacy-evidence/Cargo.toml"
    ));
}
