// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

const ARCHIVE_RS: &str = "core/crates/solstone-core-distribution/src/archive.rs";
const INSTALL_SH: &str = "core/distribution/install.sh";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn format_named_list(label: &str, names: &BTreeSet<String>) -> String {
    let mut lines = vec![format!("{label}:")];
    for name in names {
        lines.push(format!("  {name}"));
    }
    lines.join("\n")
}

fn archive_escape_names(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("Self::")
                .and_then(|rest| rest.split_once(" => \""))
                .and_then(|(_, rest)| rest.strip_suffix("\","))
                .map(str::to_owned)
        })
        .filter(|name| name.starts_with("archive-"))
        .collect()
}

fn comment_block_names(text: &str, header: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut take = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == header {
            take = true;
            continue;
        }
        if take {
            if let Some(name) = trimmed.strip_prefix("#   ")
                && !name.is_empty()
            {
                names.insert(name.to_owned());
                continue;
            }
            if trimmed.starts_with('#') && trimmed.contains("REFUSALS:") {
                break;
            }
            if !trimmed.starts_with('#') {
                break;
            }
        }
    }
    names
}

fn refuse_call_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in text.lines() {
        let Some((_, rest)) = line.split_once("refuse ") else {
            continue;
        };
        let token = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches('"');
        if !token.is_empty() && token.chars().all(|ch| ch == '-' || ch.is_ascii_lowercase()) {
            names.insert(token.to_owned());
        }
    }
    names
}

#[test]
fn install_archive_refusals_match_archive_escape_enum() {
    let root = repository_root();
    let rust = fs::read_to_string(root.join(ARCHIVE_RS)).expect("read archive.rs");
    let shell = fs::read_to_string(root.join(INSTALL_SH)).expect("read install.sh");
    let rust_names = archive_escape_names(&rust);
    let shell_archive = comment_block_names(&shell, "# ARCHIVE_REFUSALS:");
    assert_eq!(
        rust_names,
        shell_archive,
        "{}\n{}",
        format_named_list(
            "missing required",
            &rust_names.difference(&shell_archive).cloned().collect()
        ),
        format_named_list(
            "unexpected",
            &shell_archive.difference(&rust_names).cloned().collect()
        )
    );
    let called = refuse_call_names(&shell);
    let missing_calls = rust_names
        .difference(&called)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(
        missing_calls.is_empty(),
        "{}",
        format_named_list("missing required", &missing_calls)
    );
}

#[test]
fn planted_archive_refusal_mismatch_is_detected() {
    let rust = "Self::AbsolutePath => \"archive-absolute-path\",\nSelf::ParentTraversal => \"archive-parent-traversal\",\n";
    let shell = "# ARCHIVE_REFUSALS:\n#   archive-absolute-path\n";
    let rust_names = archive_escape_names(rust);
    let shell_names = comment_block_names(shell, "# ARCHIVE_REFUSALS:");
    assert_ne!(
        rust_names, shell_names,
        "planted mismatch must fail equality"
    );
    let missing = rust_names
        .difference(&shell_names)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(missing.contains("archive-parent-traversal"));
}
