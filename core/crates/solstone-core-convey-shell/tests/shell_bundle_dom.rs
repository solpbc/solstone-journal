// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The shell's script bundle under Node: a full parse of the bundle and of
//! each file in it (the build's lexer can't parse), and how the bundle's
//! wrappers behave, on a bundle of throwaway files built by the same code.
//! Runs in `make ci-full`, which every journal release runs on its final
//! source; keep it there.

use std::process::Command;

#[allow(dead_code)]
mod bundle {
    include!("../../../build-support/convey_shell_bundle.rs");
}

fn tag(path: &str) -> bundle::BundledTag {
    bundle::BundledTag {
        start: 0,
        end: 0,
        path: path.to_owned(),
        media: None,
        part: "boot".to_owned(),
    }
}

fn isolation_bundle() -> String {
    let files = [
        (
            tag("/static/a.js"),
            "var first = 1;\nvar earlyCall = helper();\nfunction helper() { return 'h'; }\n\
             let counter = 0;\nconst LIMIT = 3;\nclass Box {}\n\
             function bump() { counter += 1; return counter; }\n\
             var sloppyThisIsGlobal = (function () { return this === globalThis; })();\n"
                .to_owned(),
        ),
        (
            tag("/static/b.js"),
            "// b breaks on its first statement\nthrow new Error('b broke');\nfunction afterThrow() {}\n".to_owned(),
        ),
        (
            tag("/static/c.js"),
            "// c is strict\n'use strict';\nvar strictThis = (function () { return this === undefined; })();\n\
             globalThis.cRan = true;\nbump();\nglobalThis.sawCounterLive = counter === 1;\ncounter = 5;\n"
                .to_owned(),
        ),
        (tag("/static/d.js"), "globalThis.dRan = true;\n".to_owned()),
    ];
    bundle::script_bundle(&files, "", "shell.js.map")
        .expect("the throwaway files bundle")
        .text
}

fn split_bundles() -> (String, String) {
    let boot = [(tag("/static/boot.js"), "var bootRan = true;\n".to_owned())];
    let rest = [(
        tag("/static/rest.js"),
        "// rest breaks on its first statement\nthrow new Error('rest broke');\n".to_owned(),
    )];
    (
        bundle::script_bundle(
            &boot,
            "globalThis.SOL_SHELL_REGISTRY = {};\n",
            "shell.js.map",
        )
        .expect("boot bundle")
        .text,
        bundle::script_bundle(&rest, "", "shell-rest.js.map")
            .expect("rest bundle")
            .text,
    )
}

#[test]
fn shell_bundle_dom_contract() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let scratch = tempfile::tempdir().expect("scratch directory");
    let shipped = solstone_core_convey_shell::assets::lookup("/static/shell.js")
        .expect("the shell serves its boot script");
    let shipped_rest = solstone_core_convey_shell::assets::lookup("/static/shell-rest.js")
        .expect("the shell serves its rest script");
    let bundle_path = scratch.path().join("shell.js");
    let rest_path = scratch.path().join("shell-rest.js");
    std::fs::write(&bundle_path, shipped.bytes).expect("bundle writes");
    std::fs::write(&rest_path, shipped_rest.bytes).expect("rest bundle writes");
    let isolation_path = scratch.path().join("isolation.js");
    std::fs::write(&isolation_path, isolation_bundle()).expect("isolation bundle writes");
    let (split_boot, split_rest) = split_bundles();
    let split_boot_path = scratch.path().join("split-boot.js");
    let split_rest_path = scratch.path().join("split-rest.js");
    std::fs::write(&split_boot_path, split_boot).expect("split boot writes");
    std::fs::write(&split_rest_path, split_rest).expect("split rest writes");

    let output = Command::new("node")
        .arg(format!("{manifest}/tests/shell_bundle_dom.js"))
        .arg(&bundle_path)
        .arg(&rest_path)
        .arg(format!("{manifest}/assets/static"))
        .arg(&isolation_path)
        .arg(&split_boot_path)
        .arg(&split_rest_path)
        .output()
        .expect("shell bundle DOM harness starts (requires node)");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "shell bundle DOM harness failed:\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.starts_with("DOM CASES: ") && stdout.contains(" passed"),
        "shell bundle DOM harness did not report its case count:\n{stdout}"
    );
    println!("{}", stdout.trim());
}
