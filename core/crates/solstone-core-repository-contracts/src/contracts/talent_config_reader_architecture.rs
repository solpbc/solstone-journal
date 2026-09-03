// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::ops::Range;
use std::path::Path;

fn defined_function_name(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let line = line
        .strip_prefix("fn ")
        .or_else(|| line.strip_prefix("pub fn "))
        .or_else(|| line.strip_prefix("pub(crate) fn "))?;
    line.split_once('(').map(|(name, _)| name.trim())
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

fn find_byte_from(haystack: &[u8], byte: u8, from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .iter()
        .position(|candidate| *candidate == byte)
        .map(|offset| from + offset)
}

/// A raw string opener at `index`, as `(quote position, hash count)`. Handles `r"`, `r#"`, and the
/// byte-string `br"` forms; the caller guarantees `index` is not inside an identifier.
fn raw_string_quote(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let mut cursor = match bytes.get(index)? {
        b'r' => index + 1,
        b'b' if bytes.get(index + 1) == Some(&b'r') => index + 2,
        _ => return None,
    };
    let mut hashes = 0;
    while bytes.get(cursor) == Some(&b'#') {
        hashes += 1;
        cursor += 1;
    }
    if bytes.get(cursor) == Some(&b'"') {
        Some((cursor, hashes))
    } else {
        None
    }
}

fn skip_raw_string(bytes: &[u8], mut index: usize, hashes: usize, masked: &mut [u8]) -> usize {
    while index < bytes.len() {
        if bytes[index] == b'"'
            && bytes[index + 1..]
                .iter()
                .take(hashes)
                .filter(|byte| **byte == b'#')
                .count()
                == hashes
        {
            return index + 1 + hashes;
        }
        if bytes[index] == b'\n' {
            masked[index] = b'\n';
        }
        index += 1;
    }
    index
}

fn skip_quoted(bytes: &[u8], mut index: usize, terminator: u8, masked: &mut [u8]) -> usize {
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            byte if byte == terminator => return index + 1,
            b'\n' => {
                masked[index] = b'\n';
                index += 1;
            }
            _ => index += 1,
        }
    }
    index
}

/// A `'` opens a char literal only when it closes immediately; otherwise it is a lifetime.
fn is_char_literal(bytes: &[u8], index: usize) -> bool {
    match bytes.get(index + 1) {
        Some(b'\\') => true,
        Some(_) => bytes.get(index + 2) == Some(&b'\''),
        None => false,
    }
}

/// Blank every comment, string, and char literal to same-length spaces, keeping newlines, so brace
/// matching cannot be derailed by a `"{"` literal or a commented-out block. Byte offsets are
/// preserved, so ranges computed against the mask slice the original source directly.
pub(super) fn mask_literals_and_comments(source: &str) -> Vec<u8> {
    let bytes = source.as_bytes();
    let mut masked = vec![b' '; bytes.len()];
    let mut index = 0;
    let mut previous_is_ident = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            previous_is_ident = false;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            let mut depth = 1usize;
            index += 2;
            while index < bytes.len() && depth > 0 {
                if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
                    depth += 1;
                    index += 2;
                } else if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    depth -= 1;
                    index += 2;
                } else {
                    if bytes[index] == b'\n' {
                        masked[index] = b'\n';
                    }
                    index += 1;
                }
            }
            previous_is_ident = false;
            continue;
        }
        let raw_string = if previous_is_ident {
            None
        } else {
            raw_string_quote(bytes, index)
        };
        if let Some((quote, hashes)) = raw_string {
            masked[index..quote].copy_from_slice(&bytes[index..quote]);
            index = skip_raw_string(bytes, quote + 1, hashes, &mut masked);
            previous_is_ident = false;
            continue;
        }
        if byte == b'"' {
            index = skip_quoted(bytes, index + 1, b'"', &mut masked);
            previous_is_ident = false;
            continue;
        }
        if byte == b'\'' && is_char_literal(bytes, index) {
            index = skip_quoted(bytes, index + 1, b'\'', &mut masked);
            previous_is_ident = false;
            continue;
        }
        masked[index] = byte;
        previous_is_ident = is_ident_byte(byte);
        index += 1;
    }
    masked
}

/// Byte range of the brace-balanced block that starts at `open`, which must be a `{`.
fn block_end(masked: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    let mut index = open;
    while index < masked.len() {
        match masked[index] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
        index += 1;
    }
    masked.len()
}

/// Ranges covered by a `#[cfg(test)]` item. Test fixtures are not production talent reads, and
/// conflating them with unrelated production code is what made the whole-file scan misfire.
fn cfg_test_ranges(masked: &[u8]) -> Vec<Range<usize>> {
    const ATTRIBUTE: &[u8] = b"#[cfg(test)]";
    let mut ranges = Vec::new();
    let mut index = 0;
    while let Some(found) = find_from(masked, ATTRIBUTE, index) {
        let after = found + ATTRIBUTE.len();
        let brace = find_byte_from(masked, b'{', after);
        let semicolon = find_byte_from(masked, b';', after);
        let end = match (brace, semicolon) {
            (Some(brace), Some(semicolon)) if semicolon < brace => semicolon + 1,
            (Some(brace), _) => block_end(masked, brace),
            (None, Some(semicolon)) => semicolon + 1,
            (None, None) => masked.len(),
        };
        ranges.push(found..end);
        index = end.max(after);
    }
    ranges
}

fn find_fn_keyword(masked: &[u8], from: usize) -> Option<usize> {
    let mut index = from;
    while let Some(found) = find_from(masked, b"fn", index) {
        let before_ok = found == 0 || !is_ident_byte(masked[found - 1]);
        let after_ok = match masked.get(found + 2) {
            Some(byte) => !is_ident_byte(*byte),
            None => true,
        };
        if before_ok && after_ok {
            return Some(found);
        }
        index = found + 2;
    }
    None
}

/// The bounded production scopes a collector has to live inside: each outermost function body,
/// plus the residual top-level text outside every function and every `#[cfg(test)]` item.
///
/// Scoping is the whole point. The four collector signals are individually unremarkable — a run-log
/// `read_dir`, an `extension`/`"md"` response-format label, and a `join("talent")` in a test
/// fixture co-occur in files that never touch talent frontmatter. Only their co-occurrence *within
/// one production scope* is evidence of a reimplemented reader.
pub(super) fn production_scopes(source: &str) -> Vec<String> {
    let masked = mask_literals_and_comments(source);
    let excluded = cfg_test_ranges(&masked);
    let mut bodies: Vec<Range<usize>> = Vec::new();
    let mut index = 0;
    while let Some(found) = find_fn_keyword(&masked, index) {
        let after = found + 2;
        let brace = find_byte_from(&masked, b'{', after);
        let semicolon = find_byte_from(&masked, b';', after);
        let has_body = match (brace, semicolon) {
            (Some(brace), Some(semicolon)) => brace < semicolon,
            (Some(_), None) => true,
            (None, _) => false,
        };
        match brace {
            Some(brace) if has_body => {
                let body = brace..block_end(&masked, brace);
                index = body.end;
                if !excluded.iter().any(|skip| skip.contains(&found)) {
                    bodies.push(body);
                }
            }
            _ => index = after,
        }
    }

    let mut scopes: Vec<String> = bodies
        .iter()
        .map(|body| source[body.clone()].to_string())
        .collect();

    let mut removed = excluded;
    removed.extend(bodies);
    removed.sort_by_key(|range| range.start);
    let mut residual = String::new();
    let mut cursor = 0;
    for range in &removed {
        if range.start > cursor {
            residual.push_str(&source[cursor..range.start]);
        }
        cursor = cursor.max(range.end);
    }
    if cursor < source.len() {
        residual.push_str(&source[cursor..]);
    }
    scopes.push(residual);
    scopes
}

fn collects_talent_markdown(source: &str) -> bool {
    production_scopes(source).iter().any(|scope| {
        scope.contains("join(\"talent\")")
            && scope.contains("read_dir")
            && scope.contains("extension")
            && scope.contains("\"md\"")
    })
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
    let defines_talent_validation_fn = source.lines().any(|line| {
        matches!(
            defined_function_name(line),
            Some("validate_access_tier" | "validate_cwd" | "validate_write")
        )
    });
    (mentions_talent_domain
        && (defines_frontmatter_fn || detects_brace_line || collects_talent_markdown(source)))
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

#[test]
fn collector_signals_must_co_occur_in_one_production_scope() {
    // A real collector: all four signals inside a single production function body.
    assert!(reimplements_talent_reader(concat!(
        "fn load_talents(source: &Path) -> Vec<PathBuf> {\n",
        "    let root = source.join(\"talent\");\n",
        "    fs::read_dir(root)\n",
        "        .into_iter()\n",
        "        .flatten()\n",
        "        .flatten()\n",
        "        .filter(|entry| entry.path().extension() == Some(\"md\".as_ref()))\n",
        "        .map(|entry| entry.path())\n",
        "        .collect()\n",
        "}\n",
    )));

    // The exact false positive this contract used to produce: a run-log `read_dir`, an unrelated
    // response-format label, and a `join("talent")` that only exists in a `#[cfg(test)]` fixture.
    // Whole-file scanning conflated these three into a reader that does not exist.
    assert!(!reimplements_talent_reader(concat!(
        "fn talent_run_logs(runs: &Path) -> Vec<PathBuf> {\n",
        "    fs::read_dir(runs).into_iter().flatten().flatten().map(|e| e.path()).collect()\n",
        "}\n",
        "\n",
        "fn response_format(path: &Path) -> &'static str {\n",
        "    if path.extension().is_some_and(|ext| ext == \"json\") { \"json\" } else { \"md\" }\n",
        "}\n",
        "\n",
        "#[cfg(test)]\n",
        "mod tests {\n",
        "    use super::*;\n",
        "\n",
        "    fn fixture(root: &Path) {\n",
        "        fs::create_dir_all(root.join(\"talent\")).expect(\"talent root\");\n",
        "    }\n",
        "}\n",
    )));

    // Signals split across two production functions are not a reader either.
    assert!(!reimplements_talent_reader(concat!(
        "fn talent_root(source: &Path) -> PathBuf { source.join(\"talent\") }\n",
        "\n",
        "fn markdown_entries(root: &Path) -> Vec<PathBuf> {\n",
        "    fs::read_dir(root)\n",
        "        .into_iter()\n",
        "        .flatten()\n",
        "        .flatten()\n",
        "        .filter(|entry| entry.path().extension() == Some(\"md\".as_ref()))\n",
        "        .map(|entry| entry.path())\n",
        "        .collect()\n",
        "}\n",
    )));
}

#[test]
fn scope_bounding_survives_literals_and_comments() {
    // A `"{"` literal and a commented-out block must not unbalance the scope walker; the collector
    // below is still a single-body true positive.
    assert!(reimplements_talent_reader(concat!(
        "fn load(source: &Path) -> Vec<PathBuf> {\n",
        "    let opener = \"{\";\n",
        "    /* let stale = source.join(\"other\"); { */\n",
        "    let root = source.join(\"talent\");\n",
        "    fs::read_dir(root).into_iter().flatten().flatten()\n",
        "        .filter(|e| e.path().extension() == Some(\"md\".as_ref()))\n",
        "        .map(|e| e.path()).collect()\n",
        "}\n",
    )));

    // The same `"{"` literal must not merge two separate bodies into one scope.
    assert!(!reimplements_talent_reader(concat!(
        "fn opener() -> &'static str {\n",
        "    let brace = \"{\";\n",
        "    let root = PathBuf::from(\".\").join(\"talent\");\n",
        "    brace\n",
        "}\n",
        "\n",
        "fn entries(root: &Path) -> Vec<PathBuf> {\n",
        "    fs::read_dir(root).into_iter().flatten().flatten()\n",
        "        .filter(|e| e.path().extension() == Some(\"md\".as_ref()))\n",
        "        .map(|e| e.path()).collect()\n",
        "}\n",
    )));
}
