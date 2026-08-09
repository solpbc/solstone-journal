// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// ⚠ Included by several test binaries via `mod support;`. An item used by
// one of them is dead code in the others, which `-D warnings` rejects.
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The wire these tests drive.
///
/// It used to be `.venv/bin/solstone-generate-wire`, a Python console script.
/// The conversion deleted it, so these targets now drive the native binary and
/// need no interpreter — which is why they are no longer feature-gated.
///
/// ⚠ `CARGO_BIN_EXE_*` does not cross packages, and `solstone-core` is not built
/// as a side effect of testing this crate — the dependency runs the other way.
/// A workspace test run builds it; a bare `-p solstone-core-generate` run does
/// not, and the assertion below says so rather than failing obscurely.
#[allow(dead_code)]
pub fn core_binary() -> PathBuf {
    if let Some(path) = env::var_os("SOLSTONE_CORE") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "SOLSTONE_CORE is not a file: {}",
            path.display()
        );
        return path;
    }
    let exe = env::current_exe().expect("test executable path");
    // Integration test binaries live in `target/<profile>/deps/`; workspace
    // binaries sit one directory up.
    let path = exe
        .parent()
        .and_then(Path::parent)
        .expect("test executable has a target directory")
        .join("solstone-core");
    assert!(
        path.is_file(),
        "missing {}. These tests drive the native binary; build it with \
         `cargo build -p solstone-core`, or set SOLSTONE_CORE. A bare \
         `-p solstone-core-generate` run does not build it because the \
         dependency runs the other way.",
        path.display()
    );
    path
}

/// The subcommand that reaches the wire.
///
/// ⛔ Native verbs ship as `solstone-core` subcommands, never as standalone
/// binaries — the wheel check builds an exact member set from a one-name script
/// list, so a separate `solstone-generate-wire` executable is unreachable on an
/// installed host.
#[allow(dead_code)]
pub fn prefix() -> Vec<OsString> {
    vec![OsString::from("generate")]
}

/// A `Command` already pointed at the wire, for the tests that drive it as a
/// process rather than through a client.
#[allow(dead_code)]
pub fn generate_command() -> Command {
    let mut command = Command::new(core_binary());
    command.arg("generate");
    command
}
