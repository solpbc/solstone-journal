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

fn source(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).expect("retention projection source reads")
}

fn constructs_retention_type(source: &str, type_name: &str) -> bool {
    let construction = format!("{type_name} {{");
    source.match_indices(&construction).any(|(offset, _)| {
        source[..offset]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_alphanumeric() && character != '_')
    })
}

#[test]
fn retention_policy_projection_has_one_owner() {
    let root = repository_root();
    let settings = source(
        &root,
        "core/crates/solstone-core-settings-web/src/retention.rs",
    );
    let maintenance = source(
        &root,
        "core/crates/solstone-core-maintenance/src/bodies/health.rs",
    );

    for (name, source) in [("settings", settings), ("maintenance", maintenance)] {
        for forbidden in [
            "fn policy_from_retention(",
            "fn policy_would_release(",
            "fn rule(",
            "struct RetentionRule",
            "struct RetentionPolicy",
        ] {
            assert!(
                !source.contains(forbidden),
                "{name} must not define retention projection helper `{forbidden}`"
            );
        }
        assert!(
            !constructs_retention_type(&source, "Policy"),
            "{name} must not construct retention Policy values"
        );
        assert!(
            !constructs_retention_type(&source, "Rule"),
            "{name} must not construct retention Rule values"
        );
        assert!(
            source.contains("policy_from_retention("),
            "{name} must call the retention-owned projection"
        );
        assert!(
            source.contains("policy_would_release("),
            "{name} must call the retention-owned release predicate"
        );
    }
}
