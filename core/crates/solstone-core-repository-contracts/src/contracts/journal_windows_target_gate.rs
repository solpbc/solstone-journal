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

fn batch_label_body<'a>(script: &'a str, label: &str) -> &'a str {
    let heading = format!(":{label}\r\n");
    let body_at = script
        .find(&heading)
        .unwrap_or_else(|| panic!("batch script must define :{label}"))
        + heading.len();
    let body_end = script[body_at..]
        .find("\r\n:")
        .map(|offset| body_at + offset)
        .unwrap_or(script.len());
    &script[body_at..body_end]
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
            "Win32_System_Com",
            "Win32_System_IO",
            "Win32_System_Ioctl",
            "Win32_System_SystemServices",
            "Win32_System_Threading",
            "Win32_System_WindowsProgramming",
            "Win32_UI_Shell",
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

#[test]
fn native_launch_preparation_receipts_are_source_bound_and_exactly_once() {
    let win_ci = read_repo_file("scripts/win-ci.cmd");
    let host = read_repo_file("scripts/win-host-ci.sh");
    let receipt =
        read_repo_file("core/crates/solstone-core-system/tests/windows_lifecycle_receipt.rs");

    for (selector, marker) in [
        (
            "windows_launch_environment_preparation_receipt",
            "JOURNAL_WIN_CI_LAUNCH_ENVIRONMENT_PREPARATION",
        ),
        (
            "windows_launch_path_preparation_receipt",
            "JOURNAL_WIN_CI_LAUNCH_PATH_PREPARATION",
        ),
    ] {
        let invocation = format!(
            "call :run_platform_receipt \"Windows launch {} preparation\" \"solstone-core-system\" \"windows_lifecycle_receipt\" \"{selector}\" \"{marker}\"",
            if selector.contains("environment") {
                "environment"
            } else {
                "path"
            }
        );
        assert_eq!(
            win_ci.matches(&invocation).count(),
            1,
            "win-ci must run {selector} exactly once through the marker-validating receipt helper"
        );
        assert_eq!(
            receipt.matches(&format!("{marker}=executed/pass")).count(),
            1,
            "the source receipt must emit one canonical {marker} pass marker"
        );
        assert_eq!(
            host.lines()
                .filter(|line| *line == format!("require_platform_receipt {marker}"))
                .count(),
            1,
            "the host must require {marker} exactly once"
        );
    }

    let source_binding = win_ci
        .find("call :verify_source_binding || exit /b 1")
        .expect("win-ci verifies the transferred source before receipts");
    let first_receipt = win_ci
        .find("call :run_platform_receipt")
        .expect("win-ci runs launch-preparation receipts");
    let final_binding = win_ci
        .rfind("call :verify_source_binding || exit /b 1")
        .expect("win-ci re-verifies the transferred source after receipts");
    let acknowledgement = win_ci
        .find("=== JOURNAL_WIN_CI_OK:")
        .expect("win-ci emits its final acknowledgement");
    assert!(source_binding < first_receipt);
    assert!(first_receipt < final_binding);
    assert!(final_binding < acknowledgement);
}

#[test]
fn native_foundation_targets_run_before_source_markers_and_are_host_required() {
    let win_ci = read_repo_file("scripts/win-ci.cmd");
    let host = read_repo_file("scripts/win-host-ci.sh");
    assert_eq!(win_ci.matches("JOURNAL_WIN_CI_OK:").count(), 1);
    let final_ok = win_ci
        .find("JOURNAL_WIN_CI_OK:")
        .expect("final success marker");
    let targets = [
        (
            "signed Windows payload verifier",
            "solstone-core-distribution",
            "windows_payload",
            "test-fixture-pin",
            "journal_win_ci_windows_payload_marker",
            "JOURNAL_WIN_CI_TARGET_WINDOWS_PAYLOAD",
            "core/crates/solstone-core-distribution/tests/windows_payload.rs",
            true,
        ),
        (
            "journal-io create-only publication",
            "solstone-core-journal-io",
            "windows_create_only",
            "",
            "journal_win_ci_windows_create_only_marker",
            "JOURNAL_WIN_CI_TARGET_WINDOWS_CREATE_ONLY",
            "core/crates/solstone-core-journal-io/tests/windows_create_only.rs",
            true,
        ),
        (
            "journal-io create-only publication protocol",
            "solstone-core-journal-io",
            "windows_create_only_protocol",
            "test-hooks",
            "journal_win_ci_windows_create_only_protocol_marker",
            "JOURNAL_WIN_CI_TARGET_WINDOWS_CREATE_ONLY_PROTOCOL",
            "core/crates/solstone-core-journal-io/tests/windows_create_only_protocol.rs",
            true,
        ),
        (
            "journal-io install-file publication",
            "solstone-core-journal-io",
            "windows_install_file",
            "",
            "journal_win_ci_windows_install_file_marker",
            "JOURNAL_WIN_CI_TARGET_WINDOWS_INSTALL_FILE",
            "core/crates/solstone-core-journal-io/tests/windows_install_file.rs",
            true,
        ),
        (
            "journal-io install-file publication protocol",
            "solstone-core-journal-io",
            "windows_install_file_protocol",
            "test-hooks",
            "journal_win_ci_windows_install_file_protocol_marker",
            "JOURNAL_WIN_CI_TARGET_WINDOWS_INSTALL_FILE_PROTOCOL",
            "core/crates/solstone-core-journal-io/tests/windows_install_file_protocol.rs",
            false,
        ),
        (
            "journal-io operational-log namespace",
            "solstone-core-journal-io",
            "windows_oplog_namespace",
            "test-hooks",
            "journal_win_ci_windows_oplog_namespace_marker",
            "JOURNAL_WIN_CI_TARGET_WINDOWS_OPLOG_NAMESPACE",
            "core/crates/solstone-core-journal-io/tests/windows_oplog_namespace.rs",
            true,
        ),
    ];

    for (label, package, target, features, marker_test, marker, source_path, runs_target) in targets
    {
        let helper = if runs_target {
            "run_source_marked_target"
        } else {
            "run_source_marker"
        };
        let invocation = format!(
            "call :{helper} \"{label}\" \"{package}\" \"{target}\" \"{features}\" \"{marker_test}\" \"{marker}\""
        );
        assert_eq!(
            win_ci.matches(&invocation).count(),
            1,
            "native batch lost the exact {target} execution/marker route"
        );
        assert!(
            win_ci
                .find(&invocation)
                .expect("top-level target invocation")
                < final_ok,
            "native batch must complete {target} before its final success marker"
        );
        let source = read_repo_file(source_path);
        assert_eq!(
            source.matches(&format!("{marker}=executed/pass")).count(),
            1
        );
        let host_requirement = format!("require_platform_receipt {marker}");
        assert_eq!(
            host.lines()
                .filter(|line| *line == host_requirement)
                .count(),
            1,
            "native driver must independently require {marker}"
        );
    }

    let target_body = batch_label_body(&win_ci, "run_source_marked_target");
    let plain_target = "cargo test --manifest-path core\\Cargo.toml --locked -p \"%~2\" --test \"%~3\" || exit /b 1";
    let featured_target = "cargo test --manifest-path core\\Cargo.toml --locked -p \"%~2\" --test \"%~3\" --features \"%~4\" || exit /b 1";
    let marker_call =
        "call :run_source_marker \"%~1\" \"%~2\" \"%~3\" \"%~4\" \"%~5\" \"%~6\" || exit /b 1";
    for command in [plain_target, featured_target, marker_call] {
        assert_eq!(
            target_body.matches(command).count(),
            1,
            "full-target helper lost its exact guarded command: {command}"
        );
    }
    assert!(
        target_body.find(plain_target) < target_body.find(marker_call),
        "plain full-target execution must precede its marker"
    );
    assert!(
        target_body.find(featured_target) < target_body.find(marker_call),
        "feature-qualified full-target execution must precede its marker"
    );

    let install_target = "cargo test --manifest-path core\\Cargo.toml --locked -p solstone-core-journal-io --test windows_install_file_protocol --features test-hooks -- --nocapture > \"%JOURNAL_WIN_CI_INSTALL_PROTOCOL_LOG%\" 2>&1";
    let install_target_guard = "if not \"%ERRORLEVEL%\"==\"0\" ( del /q \"%JOURNAL_WIN_CI_INSTALL_PROTOCOL_LOG%\" >nul 2>&1 & echo ERROR: journal-io install-file publication protocol failed & exit /b 1 )";
    let install_receipt = "$marker = 'JOURNAL_WIN_CI_INSTALL_FILE_PROTOCOL';";
    let install_receipt_guard = "if not \"%ERRORLEVEL%\"==\"0\" ( del /q \"%JOURNAL_WIN_CI_INSTALL_PROTOCOL_LOG%\" >nul 2>&1 & echo ERROR: journal-io install-file protocol did not emit exactly one source-originated pass marker & exit /b 1 )";
    let install_marker = "call :run_source_marker \"journal-io install-file publication protocol\" \"solstone-core-journal-io\" \"windows_install_file_protocol\" \"test-hooks\" \"journal_win_ci_windows_install_file_protocol_marker\" \"JOURNAL_WIN_CI_TARGET_WINDOWS_INSTALL_FILE_PROTOCOL\" || exit /b 1";
    for command in [
        install_target,
        install_target_guard,
        install_receipt,
        install_receipt_guard,
        install_marker,
    ] {
        assert_eq!(
            win_ci.matches(command).count(),
            1,
            "install-file protocol lost its exact full-target/receipt/marker command: {command}"
        );
    }
    let install_target_at = win_ci
        .find(install_target)
        .expect("install protocol target");
    let install_target_guard_at = win_ci
        .find(install_target_guard)
        .expect("install protocol target guard");
    let install_receipt_at = win_ci
        .find(install_receipt)
        .expect("install protocol receipt");
    let install_receipt_guard_at = win_ci
        .find(install_receipt_guard)
        .expect("install protocol receipt guard");
    let install_marker_at = win_ci
        .find(install_marker)
        .expect("install protocol marker");
    assert!(
        install_target_at < install_target_guard_at
            && install_target_guard_at < install_receipt_at
            && install_receipt_at < install_receipt_guard_at
            && install_receipt_guard_at < install_marker_at
            && install_marker_at < final_ok,
        "install-file protocol must guard its full target and receipt before its source marker and final success"
    );

    let marker_body = batch_label_body(&win_ci, "run_source_marker");
    assert!(marker_body.contains("-- --ignored --exact --nocapture"));
    assert!(marker_body.contains("did not emit exactly one source-originated target marker"));
}
