// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Permanent guard: no cargo compile input may resolve under `solstone/`.

use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn walk_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in
        fs::read_dir(dir).unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()))
    {
        let entry = entry
            .unwrap_or_else(|error| panic!("cannot read entry under {}: {error}", dir.display()));
        let path = entry.path();
        if path.is_dir() {
            walk_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn matching_close(text: &str, open_at: usize, open: char, close: char) -> Option<usize> {
    let bytes = text.as_bytes();
    if open_at >= bytes.len() || bytes[open_at] as char != open {
        return None;
    }
    let mut depth = 0usize;
    let mut index = open_at;
    while index < bytes.len() {
        let ch = bytes[index] as char;
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn include_spans(text: &str) -> Vec<(usize, String)> {
    let mut spans = Vec::new();
    let mut search = 0usize;
    while let Some(rel) = text[search..].find("include_") {
        let start = search + rel;
        let rest = &text[start..];
        let macro_name = if rest.starts_with("include_str!") {
            "include_str!"
        } else if rest.starts_with("include_bytes!") {
            "include_bytes!"
        } else {
            search = start + 1;
            continue;
        };
        let after = start + macro_name.len();
        let open = text[after..]
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace())
            .map(|(idx, _)| after + idx);
        let Some(open) = open else {
            break;
        };
        if !text[open..].starts_with('(') {
            search = start + 1;
            continue;
        }
        let Some(close) = matching_close(text, open, '(', ')') else {
            search = start + 1;
            continue;
        };
        let line = text[..start].bytes().filter(|byte| *byte == b'\n').count() + 1;
        spans.push((line, text[start..=close].to_owned()));
        search = close + 1;
    }
    search = 0;
    while let Some(rel) = text[search..].find("#[path") {
        let start = search + rel;
        let open = text[start..].find('[').map(|idx| start + idx);
        let Some(open) = open else {
            break;
        };
        let Some(close) = matching_close(text, open, '[', ']') else {
            search = start + 1;
            continue;
        };
        let line = text[..start].bytes().filter(|byte| *byte == b'\n').count() + 1;
        spans.push((line, text[start..=close].to_owned()));
        search = close + 1;
    }
    spans
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'/' {
            while index < bytes.len() && bytes[index] != b'\n' {
                out.push(' ');
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'*' {
            out.push(' ');
            out.push(' ');
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                out.push(if bytes[index] == b'\n' { '\n' } else { ' ' });
                index += 1;
            }
            if index + 1 < bytes.len() {
                out.push(' ');
                out.push(' ');
                index += 2;
            }
            continue;
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

fn build_rs_path_hits(text: &str) -> Vec<(usize, String)> {
    let stripped = strip_comments(text);
    let mut hits = Vec::new();
    let mut search = 0usize;
    while let Some(rel) = stripped[search..].find("solstone/") {
        let start = search + rel;
        let line = stripped[..start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1;
        let snippet_start = stripped[..start]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let snippet_end = stripped[start..]
            .find('\n')
            .map(|idx| start + idx)
            .unwrap_or(stripped.len());
        hits.push((line, stripped[snippet_start..snippet_end].trim().to_owned()));
        search = start + "solstone/".len();
    }
    hits
}

#[test]
fn no_cargo_compile_input_resolves_under_solstone() {
    let root = repository_root();
    let crates = root.join("core/crates");
    let mut files = Vec::new();
    walk_files(&crates, &mut files);
    let mut violations = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .expect("crate file is under repo")
            .to_string_lossy()
            .replace('\\', "/");
        let text =
            fs::read_to_string(path).unwrap_or_else(|error| panic!("{rel} is unreadable: {error}"));
        if path.file_name().and_then(|name| name.to_str()) == Some("build.rs") {
            for (line, snippet) in build_rs_path_hits(&text) {
                violations.push(format!(
                    "{rel}:{line}: build.rs path construction `{snippet}`"
                ));
            }
            continue;
        }
        for (line, span) in include_spans(&text) {
            if span.contains("solstone/") {
                violations.push(format!("{rel}:{line}: {span}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "cargo compile inputs still resolve under solstone/:\n{}",
        violations.join("\n")
    );
}

#[test]
fn build_rs_scan_ignores_comments_and_flags_path_joins() {
    assert!(build_rs_path_hits("// leftover: solstone/convey/static\nfn main() {}\n").is_empty());
    assert!(
        !build_rs_path_hits("let static_root = root.join(\"solstone/convey/static\");\n")
            .is_empty()
    );
}

fn include_spans_solstone(text: &str) -> Vec<String> {
    include_spans(text)
        .into_iter()
        .filter(|(_, span)| span.contains("solstone/"))
        .map(|(_, span)| span)
        .collect()
}

#[test]
fn include_spans_flag_multiline_and_concat_compile_inputs() {
    // Assemble the historical forms at runtime so this file is not itself a
    // compile-input hit for `no_cargo_compile_input_resolves_under_solstone`.
    let include_str = "include_str!";
    let include_bytes = "include_bytes!";
    let multiline = format!(
        "const PROMPT: &str = {include_str}(\n    \"../../../../solstone/observe/foo.md\"\n);\n"
    );
    let concat = format!(
        "const PROMPT: &[u8] = {include_bytes}(concat!(\n    env!(\"CARGO_MANIFEST_DIR\"),\n    \"/../../../solstone/observe/foo.md\"\n));\n"
    );
    let runtime = r#"
fn load(root: &Path) -> Vec<u8> {
    std::fs::read(root.join("solstone/apps/home/workspace.html")).unwrap()
}
"#;
    let multi_hits = include_spans_solstone(&multiline);
    assert_eq!(multi_hits.len(), 1, "{multi_hits:?}");
    assert!(multi_hits[0].contains("include_str!"), "{}", multi_hits[0]);
    let concat_hits = include_spans_solstone(&concat);
    assert_eq!(concat_hits.len(), 1, "{concat_hits:?}");
    assert!(concat_hits[0].contains("concat!"), "{}", concat_hits[0]);
    assert!(
        include_spans_solstone(runtime).is_empty(),
        "runtime root.join(solstone/…) must not look like a compile input"
    );
}
