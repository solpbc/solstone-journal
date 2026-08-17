// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

// This integration test directly exercises only a subset of warm module items.
#[allow(dead_code)]
#[path = "../src/warm.rs"]
mod warm;

use warm::{
    Classification, Host, InventoryRow, PlatformApplicability, collect_for_executable,
    inventory_rows,
};

const FIXTURE_ROW: InventoryRow = InventoryRow {
    binary_name: "warm-fixture",
    distribution: "warm-fixture",
    crate_name: "warm-fixture",
    argv: &[],
    expected: "fixture reaches main",
    applicability: PlatformApplicability::All,
};

struct TempDir(tempfile::TempDir);

impl TempDir {
    fn new() -> Self {
        Self(tempfile::tempdir().expect("create temporary directory"))
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

fn make_executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make executable");
}

fn write_shim(path: &Path, script: &str) {
    fs::write(path, script).expect("write shim");
    make_executable(path);
}

fn executable_in(directory: &Path) -> PathBuf {
    directory.join("solstone-core")
}

fn build_linked_fixture(directory: &Path) {
    fs::write(
        directory.join("lib.rs"),
        "#[no_mangle]\npub extern \"C\" fn warm_fixture() {}\n",
    )
    .expect("write cdylib source");
    fs::write(
        directory.join("bin.rs"),
        "extern \"C\" { fn warm_fixture(); }\nfn main() { unsafe { warm_fixture(); } }\n",
    )
    .expect("write binary source");

    let library = directory.join("libwarm_fixture.so");
    let status = Command::new("rustc")
        .current_dir(directory)
        .args(["--crate-type", "cdylib", "lib.rs", "-o"])
        .arg(&library)
        .status()
        .expect("rustc must be available to build the cdylib fixture");
    assert!(status.success(), "rustc must build the cdylib fixture");

    let binary = directory.join(FIXTURE_ROW.binary_name);
    let status = Command::new("rustc")
        .current_dir(directory)
        .args(["bin.rs", "-L", "native=.", "-l", "dylib=warm_fixture", "-C"])
        .arg("link-arg=-Wl,-rpath,$ORIGIN")
        .arg("-o")
        .arg(&binary)
        .status()
        .expect("rustc must be available to build the linked fixture");
    assert!(status.success(), "rustc must build the linked fixture");
}

#[test]
fn warm_spawns_sibling_by_independent_witness() {
    let temp = TempDir::new();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).expect("create bin directory");
    let copied_core = executable_in(&bin);
    fs::copy(env!("CARGO_BIN_EXE_solstone-core"), &copied_core).expect("copy core binary");
    make_executable(&copied_core);

    let marker = temp.path().join("witness");
    for row in inventory_rows() {
        if row.binary_name != "solstone-core" {
            write_shim(
                &bin.join(row.binary_name),
                "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> \"$WARM_WITNESS\"\nexit 0\n",
            );
        }
    }

    // The copied core probes itself with --version; that terminates and cannot recurse into warm.
    let output = Command::new(&copied_core)
        .args(["warm", "--json"])
        .env("WARM_WITNESS", &marker)
        .output()
        .expect("run copied core warm");
    assert!(
        output.status.success(),
        "warm stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let witness = fs::read_to_string(marker).expect("sibling shim wrote witness");
    assert!(witness.contains("solstone-core-journal --version"));
}

#[test]
fn linked_fixture_runs_then_reports_missing_library() {
    let temp = TempDir::new();
    build_linked_fixture(temp.path());
    let executable = executable_in(temp.path());
    let report = collect_for_executable(&executable, &[FIXTURE_ROW], Host::Linux);
    assert_eq!(report.records[0].classification, Classification::Ran);
    assert_eq!(report.records[0].reason_code, "reached-own-code");

    fs::rename(
        temp.path().join("libwarm_fixture.so"),
        temp.path().join("libwarm_fixture.so.hidden"),
    )
    .expect("hide fixture library");
    let report = collect_for_executable(&executable, &[FIXTURE_ROW], Host::Linux);
    let record = &report.records[0];
    assert_eq!(record.classification, Classification::CannotLoad);
    assert_eq!(record.reason_code, "loader-library-missing");
    assert_eq!(
        record.unresolved_library.as_deref(),
        Some("libwarm_fixture.so")
    );
    #[cfg(target_os = "linux")]
    assert_eq!(record.exit_code, Some(127));
    #[cfg(target_os = "macos")]
    {
        assert_eq!(record.exit_code, None);
        assert_eq!(
            record.signal,
            Some(nix::sys::signal::Signal::SIGABRT as i32)
        );
    }
}

#[test]
fn signal_death_is_cannot_load() {
    let temp = TempDir::new();
    write_shim(
        &temp.path().join(FIXTURE_ROW.binary_name),
        "#!/bin/sh\nkill -TERM $$\n",
    );
    let report = collect_for_executable(&executable_in(temp.path()), &[FIXTURE_ROW], Host::Linux);
    let record = &report.records[0];
    assert_eq!(record.classification, Classification::CannotLoad);
    assert_eq!(record.reason_code, "terminated-by-signal");
    assert_eq!(record.exit_code, None);
    assert_eq!(record.signal, Some(15));
}

#[test]
fn foreign_loader_diagnostic_does_not_override_a_numeric_exit() {
    let temp = TempDir::new();
    #[cfg(target_os = "linux")]
    let diagnostic = "Library not loaded: libwarm-foreign.so";
    #[cfg(target_os = "macos")]
    let diagnostic =
        "error while loading shared libraries: libwarm-foreign.so: cannot open shared object file";
    write_shim(
        &temp.path().join(FIXTURE_ROW.binary_name),
        &format!("#!/bin/sh\nprintf '%s\\n' '{diagnostic}' >&2\nexit 127\n"),
    );
    let report = collect_for_executable(&executable_in(temp.path()), &[FIXTURE_ROW], Host::Linux);
    let record = &report.records[0];
    assert_eq!(record.classification, Classification::Ran);
    assert_eq!(record.reason_code, "reached-own-code");
    assert_eq!(record.unresolved_library, None);
    assert_eq!(record.exit_code, Some(127));
}

#[test]
fn native_loader_diagnostic_requires_the_host_loader_status() {
    let temp = TempDir::new();
    #[cfg(target_os = "linux")]
    let (diagnostic, termination, reason_code, exit_code) = (
        "error while loading shared libraries: libwarm-status.so: cannot open shared object file",
        "kill -ABRT $$",
        "terminated-by-signal",
        None,
    );
    #[cfg(target_os = "macos")]
    let (diagnostic, termination, reason_code, exit_code) = (
        "Library not loaded: /fixture/libwarm-status.dylib",
        "exit 127",
        "reached-own-code",
        Some(127),
    );
    write_shim(
        &temp.path().join(FIXTURE_ROW.binary_name),
        &format!("#!/bin/sh\nprintf '%s\\n' '{diagnostic}' >&2\n{termination}\n"),
    );
    let report = collect_for_executable(&executable_in(temp.path()), &[FIXTURE_ROW], Host::Linux);
    let record = &report.records[0];
    assert_eq!(record.reason_code, reason_code);
    assert_eq!(record.unresolved_library, None);
    assert_eq!(record.exit_code, exit_code);
}

#[test]
fn loader_failure_json_has_reason_library_and_house_fields() {
    let temp = TempDir::new();
    build_linked_fixture(temp.path());
    fs::rename(
        temp.path().join("libwarm_fixture.so"),
        temp.path().join("libwarm_fixture.so.hidden"),
    )
    .expect("hide fixture library");
    let report = collect_for_executable(&executable_in(temp.path()), &[FIXTURE_ROW], Host::Linux);
    let value = report.as_json();
    let row = &value["binaries"][0];
    assert_eq!(row["classification"], "cannot-load");
    assert_eq!(row["reason-code"], "loader-library-missing");
    assert_eq!(row["unresolved-library"], "libwarm_fixture.so");
    for field in ["subject", "error", "expected", "actual", "repair-command"] {
        assert!(row.get(field).is_some(), "missing {field}");
    }
    assert!(row["repair-command"].is_null());
}

#[test]
fn oversized_loader_stderr_still_classifies_cannot_load() {
    let temp = TempDir::new();
    let noise_bytes = warm::STDERR_LIMIT + 1;
    #[cfg(target_os = "linux")]
    let failure = "printf '%s: error while loading shared libraries: libwarm-oversize.so: cannot open shared object file: No such file or directory\\n' \"$0\" >&2\nexit 127";
    #[cfg(target_os = "macos")]
    let failure = "printf 'dyld[%s]: Library not loaded: /fixture/libwarm-oversize.so\\n  Reason: tried fixture paths\\n' \"$$\" >&2\nkill -ABRT $$";
    write_shim(
        &temp.path().join(FIXTURE_ROW.binary_name),
        &format!(
            "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt \"{noise_bytes}\" ]; do\n  printf x >&2\n  i=$((i + 1))\ndone\n{failure}\n"
        ),
    );
    let report = collect_for_executable(&executable_in(temp.path()), &[FIXTURE_ROW], Host::Linux);
    let record = &report.records[0];
    assert_eq!(record.classification, Classification::CannotLoad);
    assert_eq!(record.reason_code, "loader-library-missing");
    assert_eq!(
        record.unresolved_library.as_deref(),
        Some("libwarm-oversize.so")
    );
    assert!(record.stderr_truncated);
}

#[test]
fn cargo_marker_makes_absent_sibling_a_named_gap() {
    let temp = TempDir::new();
    let target = temp.path().join("cargo-target");
    let debug = target.join("debug");
    fs::create_dir_all(&debug).expect("create target debug directory");
    fs::write(
        target.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n",
    )
    .expect("write Cargo target marker");
    let report = collect_for_executable(&debug.join("solstone-core"), &[FIXTURE_ROW], Host::Linux);
    let record = &report.records[0];
    assert_eq!(record.classification, Classification::Unexercised);
    assert_eq!(record.reason_code, "development-sibling-not-built");
    assert!(!report.failed());
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

#[test]
fn linux_only_row_is_named_and_non_failing_on_simulated_macos() {
    let temp = TempDir::new();
    let report =
        collect_for_executable(&executable_in(temp.path()), &[describe_row()], Host::MacOs);
    let record = &report.records[0];

    assert_eq!(record.classification, Classification::NotApplicable);
    assert_eq!(record.reason_code, "platform-not-applicable");
    assert!(!report.failed());
}

#[test]
fn linux_only_row_is_missing_and_failing_on_simulated_linux() {
    let temp = TempDir::new();
    let report =
        collect_for_executable(&executable_in(temp.path()), &[describe_row()], Host::Linux);
    let record = &report.records[0];

    assert_eq!(record.classification, Classification::Missing);
    assert_eq!(record.reason_code, "binary-missing");
    assert!(report.failed());
}
