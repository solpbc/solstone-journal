// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Every native binary `journal` forwards a process verb to must complete the
//! Windows launch handshake from its own `main`.
//!
//! On Windows a hosted `journal <verb>` cannot replace itself the way Unix
//! `exec` does: it creates the native binary named in `NATIVE_PROCESS_SPECS`
//! as a new process and waits for that process to connect, take its launch
//! authority and acknowledge it. A binary that never calls
//! `receive_windows_launch()` leaves the forwarder waiting out its deadline,
//! after which the child is stopped and `journal` exits 70 without the verb's
//! own exit code. That is how `journal describe` and `journal depict` failed
//! under Sense on Windows while every Linux and macOS gate stayed green, so the
//! rule is checked here, from source, on every platform.

use std::collections::BTreeSet;

use syn::visit::Visit;

#[allow(dead_code)]
#[path = "../../../solstone-core-journal-cli/src/processes.rs"]
mod production_processes;

use production_processes::NATIVE_PROCESS_SPECS;

const RECEIVING_HALF: &str = "receive_windows_launch";

struct NativeEntry {
    binary: &'static str,
    manifest: &'static str,
    main: &'static str,
}

// One row per distinct binary in NATIVE_PROCESS_SPECS. The coverage test below
// is bidirectional, so a binary added to the table without a row here fails.
const NATIVE_ENTRIES: &[NativeEntry] = &[
    NativeEntry {
        binary: "solstone-core",
        manifest: include_str!("../../../solstone-core/Cargo.toml"),
        main: include_str!("../../../solstone-core/src/main.rs"),
    },
    NativeEntry {
        binary: "solstone-core-depict",
        manifest: include_str!("../../../solstone-core-depict/Cargo.toml"),
        main: include_str!("../../../solstone-core-depict/src/main.rs"),
    },
    NativeEntry {
        binary: "solstone-core-describe",
        manifest: include_str!("../../../solstone-core-describe/Cargo.toml"),
        main: include_str!("../../../solstone-core-describe/src/main.rs"),
    },
];

#[derive(Default)]
struct ReceivingCall {
    found: bool,
}

impl<'ast> Visit<'ast> for ReceivingCall {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref()
            && path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == RECEIVING_HALF)
        {
            self.found = true;
        }
        syn::visit::visit_expr_call(self, call);
    }
}

/// Whether the crate-root `fn main` itself calls the receiving half. A call in
/// a comment, a string, a helper `main` never reaches, or a test does not count.
fn main_receives_windows_launch(source: &str) -> bool {
    let file = syn::parse_file(source).expect("native entry source parses");
    file.items.iter().any(|item| {
        let syn::Item::Fn(function) = item else {
            return false;
        };
        if function.sig.ident != "main" {
            return false;
        }
        let mut visitor = ReceivingCall::default();
        visitor.visit_block(&function.block);
        visitor.found
    })
}

/// The binary Cargo builds from `src/main.rs`: the package name, unless a
/// `[[bin]]` renames that source.
fn main_binary_name(manifest: &str) -> String {
    let manifest = manifest
        .parse::<toml_edit::DocumentMut>()
        .expect("native entry manifest parses");
    let renamed = manifest
        .get("bin")
        .and_then(toml_edit::Item::as_array_of_tables)
        .into_iter()
        .flatten()
        .find(|bin| bin.get("path").and_then(toml_edit::Item::as_str) == Some("src/main.rs"))
        .and_then(|bin| bin.get("name").and_then(toml_edit::Item::as_str));
    renamed
        .or_else(|| {
            manifest
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(toml_edit::Item::as_str)
        })
        .expect("native entry manifest names its package")
        .to_owned()
}

#[test]
fn every_native_process_binary_has_a_checked_entry() {
    let forwarded = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.binary)
        .collect::<BTreeSet<_>>();
    let checked = NATIVE_ENTRIES
        .iter()
        .map(|entry| entry.binary)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        forwarded, checked,
        "NATIVE_PROCESS_SPECS binaries and the launch-admission entries diverged"
    );
    for entry in NATIVE_ENTRIES {
        assert_eq!(
            main_binary_name(entry.manifest),
            entry.binary,
            "the checked main.rs does not build {}",
            entry.binary
        );
    }
}

#[test]
fn every_native_process_binary_completes_the_windows_launch_handshake() {
    let missing = NATIVE_ENTRIES
        .iter()
        .filter(|entry| !main_receives_windows_launch(entry.main))
        .map(|entry| entry.binary)
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "journal forwards to these binaries on Windows, but their main never calls \
         {RECEIVING_HALF}(), so a hosted launch times out and exits 70: {missing:?}"
    );
}

#[test]
fn handshake_detection_is_load_bearing() {
    assert!(main_receives_windows_launch(
        "fn main() { let _a = solstone_core_system::process::receive_windows_launch(); }"
    ));
    assert!(main_receives_windows_launch(
        "fn main() { #[cfg(windows)] let _a = match receive_windows_launch() { Ok(a) => a, Err(_) => return }; }"
    ));
    for source in [
        "fn main() {}",
        "fn main() { /* receive_windows_launch() */ let _ = \"receive_windows_launch()\"; }",
        "fn main() {} fn admit() { let _ = receive_windows_launch(); }",
        "fn main() { let _ = receive_windows_installed_task_launch(&request); }",
    ] {
        assert!(!main_receives_windows_launch(source), "{source}");
    }
    assert_eq!(
        main_binary_name("[package]\nname = \"solstone-core-describe\"\n"),
        "solstone-core-describe"
    );
    assert_eq!(
        main_binary_name(
            "[package]\nname = \"solstone-core-journal-bin\"\n\n[[bin]]\nname = \"solstone-core-journal\"\npath = \"src/main.rs\"\n"
        ),
        "solstone-core-journal"
    );
}
