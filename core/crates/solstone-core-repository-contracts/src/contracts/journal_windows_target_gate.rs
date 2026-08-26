// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! journal-io and archive retain their supported Windows source surfaces.

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn read_repo_file(relative: &str) -> String {
    let path = repository_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn parse_manifest(relative: &str) -> DocumentMut {
    read_repo_file(relative)
        .parse()
        .unwrap_or_else(|error| panic!("parse {relative}: {error}"))
}

fn table<'a>(doc: &'a DocumentMut, key: &str) -> Option<&'a Table> {
    doc.get(key).and_then(Item::as_table)
}

fn target_unix<'a>(doc: &'a DocumentMut, kind: &str) -> Option<&'a Table> {
    doc.get("target")
        .and_then(|item| item.get("cfg(unix)"))
        .and_then(|item| item.get(kind))
        .and_then(Item::as_table)
}

fn target_windows<'a>(doc: &'a DocumentMut, kind: &str) -> Option<&'a Table> {
    doc.get("target")
        .and_then(|item| item.get("cfg(windows)"))
        .and_then(|item| item.get(kind))
        .and_then(Item::as_table)
}

fn workspace_dependencies(doc: &DocumentMut) -> &Table {
    doc.get("workspace")
        .and_then(|item| item.get("dependencies"))
        .and_then(Item::as_table)
        .expect("workspace dependencies")
}

fn has_key(table: Option<&Table>, key: &str) -> bool {
    table.is_some_and(|table| table.contains_key(key))
}

fn features(table: &Table, crate_name: &str) -> Vec<String> {
    table
        .get(crate_name)
        .and_then(Item::as_value)
        .and_then(|value| value.as_inline_table())
        .and_then(|table| table.get("features"))
        .and_then(|value| value.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn compile_error_literal(lib: &str, cfg: &str) -> String {
    const MACRO: &str = "compile_error!(";
    let cfg_at = lib
        .find(cfg)
        .unwrap_or_else(|| panic!("lib.rs must declare {cfg}"));
    let after_cfg = &lib[cfg_at + cfg.len()..];
    let macro_rel = after_cfg
        .find(MACRO)
        .expect("lib.rs must declare compile_error! after its target gate");
    let between = after_cfg[..macro_rel].trim();
    assert!(
        between.is_empty(),
        "compile_error! must follow its target gate immediately, found {between:?}"
    );
    let rest = &after_cfg[macro_rel + MACRO.len()..];
    let quote = rest
        .find('"')
        .expect("compile_error! must contain a string literal");
    let body = &rest[quote + 1..];
    let end = body
        .find('"')
        .expect("compile_error! string literal must close");
    body[..end].to_owned()
}

#[test]
fn journal_io_nix_edges_are_unix_target_gated() {
    let doc = parse_manifest("core/crates/solstone-core-journal-io/Cargo.toml");
    assert!(!has_key(table(&doc, "dependencies"), "nix"));
    assert!(!has_key(table(&doc, "dev-dependencies"), "nix"));
    let unix_deps = target_unix(&doc, "dependencies").expect("unix dependencies");
    assert_eq!(features(unix_deps, "nix"), ["dir", "fs"]);
    let unix_dev = target_unix(&doc, "dev-dependencies").expect("unix dev-dependencies");
    assert_eq!(features(unix_dev, "nix"), ["signal"]);
    let windows = target_windows(&doc, "dependencies").expect("windows dependencies");
    assert!(windows.contains_key("windows-sys"));
    let workspace = parse_manifest("core/Cargo.toml");
    assert_eq!(
        features(workspace_dependencies(&workspace), "windows-sys"),
        [
            "Wdk_Foundation",
            "Wdk_Storage_FileSystem",
            "Win32_Globalization",
            "Win32_Security",
            "Win32_Storage_CloudFilters",
            "Win32_Storage_FileSystem",
            "Win32_System_IO",
            "Win32_System_Ioctl",
            "Win32_System_SystemServices",
            "Win32_System_Threading",
            "Win32_System_WindowsProgramming",
        ]
    );
}

#[test]
fn journal_archive_unix_edges_are_target_gated() {
    let doc = parse_manifest("core/crates/solstone-core-journal-archive/Cargo.toml");
    let deps = table(&doc, "dependencies").expect("dependencies");
    assert!(!deps.contains_key("nix"));
    assert!(!deps.contains_key("solstone-core-journal-io"));
    assert!(deps.contains_key("zip"));
    let unix_deps = target_unix(&doc, "dependencies").expect("unix dependencies");
    assert_eq!(features(unix_deps, "nix"), ["dir", "user"]);
    assert!(unix_deps.contains_key("solstone-core-journal-io"));
    let windows_deps = target_windows(&doc, "dependencies").expect("windows dependencies");
    assert!(windows_deps.contains_key("solstone-core-journal-io"));
    let dev = table(&doc, "dev-dependencies").expect("dev-dependencies");
    assert!(!dev.contains_key("nix"));
    assert!(dev.contains_key("zip"));
    let unix_dev = target_unix(&doc, "dev-dependencies").expect("unix dev-dependencies");
    assert_eq!(features(unix_dev, "nix"), ["fs"]);
}

#[test]
fn journal_io_lib_declares_non_unix_non_windows_compile_error() {
    let lib = read_repo_file("core/crates/solstone-core-journal-io/src/lib.rs");
    assert_eq!(
        compile_error_literal(&lib, "#[cfg(not(any(unix, windows)))]"),
        "solstone-core-journal-io requires a Unix or Windows target: atomic write, locking, and lease durability guarantees have no portable backend"
    );
}

#[test]
fn journal_archive_lib_declares_non_unix_non_windows_compile_error() {
    let lib = read_repo_file("core/crates/solstone-core-journal-archive/src/lib.rs");
    assert_eq!(
        compile_error_literal(&lib, "#[cfg(not(any(unix, windows)))]"),
        "solstone-core-journal-archive requires a Unix or Windows target: archive source traversal has no portable backend"
    );
}

#[test]
fn windows_crosscheck_has_no_journal_archive_exclusion() {
    let doc: DocumentMut = read_repo_file("core/ci/windows-crosscheck.toml")
        .parse()
        .expect("parse windows-crosscheck.toml");
    let exclusions = doc["exclusions"]
        .as_array_of_tables()
        .expect("exclusions array of tables");
    assert!(exclusions.iter().all(|exclusion| {
        exclusion.get("package").and_then(Item::as_str) != Some("solstone-core-journal-archive")
    }));
}
