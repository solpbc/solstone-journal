// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const OWNER: &str = "core/crates/solstone-core-journal/src/lib.rs";

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

fn allowed(relative: &str) -> bool {
    relative == OWNER
        || relative.starts_with("core/crates/solstone-core-doctor/")
        || relative == "core/crates/solstone-core-transcribe/src/model_assets.rs"
        || relative == "core/crates/solstone-core-journal-cli/src/layout.rs"
        || relative == "core/crates/solstone-core/src/config.rs"
        || relative == "core/crates/solstone-core-setup/src/wrapper.rs"
        || relative == "core/crates/solstone-core-depict/src/lib.rs"
        || relative.contains("/tests/")
        || relative.contains("/contracts/")
}

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, found);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            found.push(path);
        }
    }
}

fn strip_tests_and_comments(text: &str) -> String {
    let without_line_comments = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let marker = "#[cfg(test)]";
    let mut kept = String::new();
    let mut rest = without_line_comments.as_str();
    while let Some(index) = rest.find(marker) {
        kept.push_str(&rest[..index]);
        rest = &rest[index + marker.len()..];
        if let Some(brace) = rest.find('{') {
            let mut depth = 0_i32;
            let mut end = None;
            for (offset, ch) in rest[brace..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(brace + offset + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = match end {
                Some(end) => &rest[end..],
                None => "",
            };
        }
    }
    kept.push_str(rest);
    kept
}

fn private_copy(text: &str) -> bool {
    text.contains("fn installed_site_packages_from_executable_dir")
        || text.contains("fn is_solstone_checkout_root")
        || text.contains("fn resolve_canonical_site_packages")
}

fn checkout_conjunction(text: &str) -> bool {
    text.contains("pyproject.toml") && text.contains(".git") && text.contains("join(\"solstone\")")
}

fn talent_ancestor_scan(text: &str) -> bool {
    text.contains(".ancestors()")
        && (text.contains("\"solstone/talent\"") || text.contains("\"solstone/apps\""))
}

pub(crate) fn scan_text(text: &str) -> BTreeSet<&'static str> {
    let mut hits = BTreeSet::new();
    if private_copy(text) {
        hits.insert("private-site-packages-or-checkout-copy");
    }
    if checkout_conjunction(text) {
        hits.insert("pyproject-git-solstone-conjunction");
    }
    if talent_ancestor_scan(text) {
        hits.insert("ancestors-talent-or-apps-scan");
    }
    hits
}

fn scan_repository(root: &Path) -> BTreeSet<String> {
    let mut files = Vec::new();
    rust_files(&root.join("core/crates"), &mut files);
    let mut unexpected = BTreeSet::new();
    for file in files {
        let Ok(relative) = file.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if allowed(&relative) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let text = strip_tests_and_comments(&text);
        for hit in scan_text(&text) {
            unexpected.insert(format!("{relative}::{hit}"));
        }
    }
    unexpected
}

#[test]
fn planted_violation_is_detected() {
    let site_packages_copy = concat!(
        "fn ",
        "installed_site_packages_from_executable_dir",
        "(dir: &Path) {}"
    );
    assert!(
        scan_text(site_packages_copy).contains("private-site-packages-or-checkout-copy"),
        "planted private copy must fail"
    );
    let conjunction = "candidate.join(\"pyproject.toml\").is_file() && candidate.join(\".git\").exists() && candidate.join(\"solstone\").is_dir()";
    assert!(
        scan_text(conjunction).contains("pyproject-git-solstone-conjunction"),
        "planted checkout conjunction must fail"
    );
    let walk = "for ancestor in start.ancestors() { let _ = ancestor.join(\"solstone/talent\"); }";
    assert!(
        scan_text(walk).contains("ancestors-talent-or-apps-scan"),
        "planted ancestor talent scan must fail"
    );
}

#[test]
fn production_has_no_independent_resolvers() {
    let unexpected = scan_repository(&repository_root());
    assert!(
        unexpected.is_empty(),
        "{}",
        format_named_list("unexpected", &unexpected)
    );
}
