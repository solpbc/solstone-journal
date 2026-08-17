// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

const INSTALL_SH: &str = "core/distribution/install.sh";
const INSTALL_TEST_SH: &str = "core/distribution/install.test.sh";

#[derive(Debug, PartialEq, Eq)]
struct Violation {
    file: String,
    line: usize,
    text: String,
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn has_unapproved_tmp_path(line: &str) -> bool {
    line.match_indices("/tmp").any(|(offset, _)| {
        let escaped = line[..offset].ends_with('\\');
        let suffix_is_name = line[offset + "/tmp".len()..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        !escaped && !suffix_is_name && !line[..offset].ends_with("/var")
    })
}

fn has_template_operand(mut text: &str) -> bool {
    loop {
        text = text.trim_start();
        match text.chars().next() {
            None | Some(')') | Some(';') | Some('|') | Some('&') => return false,
            Some('-') => {
                let end = text
                    .find(|ch: char| ch.is_whitespace() || matches!(ch, ')' | ';' | '|' | '&'))
                    .unwrap_or(text.len());
                text = &text[end..];
            }
            Some(_) => return true,
        }
    }
}

fn is_mktemp_boundary(text: &str) -> bool {
    match text.chars().next() {
        None => true,
        Some(ch) => ch.is_whitespace() || ch == ')',
    }
}

fn has_templateless_mktemp(line: &str) -> bool {
    let mut remaining = line;
    while let Some(offset) = remaining.find("$(mktemp") {
        let after = &remaining[offset + "$(mktemp".len()..];
        if is_mktemp_boundary(after) && !has_template_operand(after) {
            return true;
        }
        remaining = after;
    }

    let trimmed = line.trim_start();
    let Some(after) = trimmed.strip_prefix("mktemp") else {
        return false;
    };
    is_mktemp_boundary(after) && !has_template_operand(after)
}

fn detect_violations(file: &str, text: &str) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if has_unapproved_tmp_path(line) {
            violations.push(Violation {
                file: file.to_owned(),
                line: index + 1,
                text: line.to_owned(),
            });
        }
        if has_templateless_mktemp(line) {
            violations.push(Violation {
                file: file.to_owned(),
                line: index + 1,
                text: line.to_owned(),
            });
        }
    }
    violations
}

fn format_violations(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|violation| format!("{}:{}: {}", violation.file, violation.line, violation.text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn distribution_install_temporary_paths_are_explicit() {
    let root = repository_root();
    let mut violations = Vec::new();
    for file in [INSTALL_SH, INSTALL_TEST_SH] {
        let text = fs::read_to_string(root.join(file)).expect("read distribution shell source");
        violations.extend(detect_violations(file, &text));
    }
    assert!(violations.is_empty(), "{}", format_violations(&violations));
}

#[test]
fn planted_temporary_path_violation_is_detected() {
    let text = concat!(
        "for cmd in sh tar sha256sum uname mktemp mkdir cp mv ln cat awk chmod wc tr cut printf dirname rm ls env; do\n",
        "exec \"/usr/bin/mktemp\" \"$@\"\n",
        "GOOD=$(mktemp -d \"$TMP_ROOT/solstone-good-XXXXXX\")\n",
        "LOG=/var/tmp/solstone-good.log\n",
        "BAD=/tmp/x\n",
        "NO_TEMPLATE=$(mktemp)\n",
        "FLAGS_ONLY=$(mktemp -d)\n",
    );
    let violations = detect_violations("planted.sh", text);
    assert_eq!(violations.len(), 3, "{}", format_violations(&violations));
    assert!(
        violations
            .iter()
            .any(|violation| violation.text == "BAD=/tmp/x")
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.text == "NO_TEMPLATE=$(mktemp)")
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.text == "FLAGS_ONLY=$(mktemp -d)")
    );
}
