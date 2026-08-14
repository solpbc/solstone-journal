// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

fn defined_function_name(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let line = line
        .strip_prefix("fn ")
        .or_else(|| line.strip_prefix("pub fn "))
        .or_else(|| line.strip_prefix("pub(crate) fn "))?;
    line.split_once('(').map(|(name, _)| name.trim())
}

/// Package-root discovery is deliberately out of scope: it resolves locations and reads no
/// frontmatter.
fn reimplements_talent_reader(source: &str) -> bool {
    let mentions_talent_domain = source.contains("talent");
    let defines_frontmatter_fn = source
        .lines()
        .any(|line| defined_function_name(line).is_some_and(|name| name.contains("frontmatter")));
    let detects_brace_line = source
        .lines()
        .any(|line| line.contains("== \"{\"") || line.contains("!= Some(\"{\")"));
    let collects_talent_markdown = source.contains("join(\"talent\")")
        && source.contains("read_dir")
        && source.contains("extension")
        && source.contains("\"md\"");
    let defines_talent_validation_fn = source.lines().any(|line| {
        matches!(
            defined_function_name(line),
            Some("validate_access_tier" | "validate_cwd" | "validate_write")
        )
    });
    (mentions_talent_domain
        && (defines_frontmatter_fn || detects_brace_line || collects_talent_markdown))
        || defines_talent_validation_fn
}

fn visit(root: &Path, violations: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, violations);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = fs::read_to_string(&path).expect("repository source is readable");
            if reimplements_talent_reader(&source) {
                violations.push(path.display().to_string());
            }
        }
    }
}

#[test]
fn talent_reader_has_one_native_owner() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory");
    let mut violations = Vec::new();
    for entry in fs::read_dir(crates)
        .expect("crates directory is readable")
        .flatten()
    {
        let name = entry.file_name();
        if name == "solstone-core-talent-config" || name == "solstone-core-repository-contracts" {
            continue;
        }
        visit(&entry.path(), &mut violations);
    }
    assert!(
        violations.is_empty(),
        "duplicate talent config reader(s): {violations:?}"
    );
}

#[test]
fn predicate_catches_reader_shapes_and_clears_non_talent_shapes() {
    assert!(reimplements_talent_reader(
        "fn talent_frontmatter(source: &str) {}"
    ));
    assert!(reimplements_talent_reader(
        "let path = \"talent/case.md\";\nif first != Some(\"{\") {}"
    ));
    assert!(reimplements_talent_reader(
        "let root = source.join(\"talent\");\nfor entry in fs::read_dir(root)? { if entry.path().extension() == Some(\"md\") {} }"
    ));
    assert!(reimplements_talent_reader("fn validate_access_tier() {}"));
    assert!(!reimplements_talent_reader(
        "fn split_frontmatter(source: &str) { source.find(\"\\n}\\n\") }"
    ));
    assert!(!reimplements_talent_reader(
        "pub fn split_frontmatter(raw: &str) -> Result<&str, ()> { Ok(raw) }"
    ));
    assert!(!reimplements_talent_reader(
        "fn frontmatter_re() -> &'static Regex { Regex::new(\"---\").unwrap() }"
    ));
    assert!(!reimplements_talent_reader(
        "use solstone_core_talent_config::read_frontmatter;\nfn caller() { read_frontmatter(path); }"
    ));
}
