// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! This repository contract is a unit test so the ordinary `make ci`
//! `--lib --bins` selection executes it.

use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

#[test]
fn ac7_python_settings_and_observer_management_surfaces_are_absent() {
    let root = repository_root();
    for path in [
        "solstone/apps/settings/routes.py",
        "solstone/apps/settings/workspace.html",
        "solstone/apps/settings/static/settings.js",
        "solstone/apps/settings/copy.py",
        "solstone/apps/settings/transcribe_resource.py",
        "solstone/apps/settings/app.json",
        "solstone/apps/observer/routes.py",
        "solstone/apps/observer/workspace.html",
        "solstone/apps/observer/app.json",
    ] {
        assert!(
            !root.join(path).exists(),
            "retired Python web path remains: {path}"
        );
    }

    assert!(
        root.join("core/crates/solstone-core-settings-web/src/lib.rs")
            .exists(),
        "native Settings web owner is present"
    );
    assert!(
        root.join("core/crates/solstone-core-convey-shell/src/clients.rs")
            .exists(),
        "native Clients owner is present"
    );
}

#[test]
fn retired_python_app_json_and_merged_workspace_html_are_absent() {
    let root = repository_root();
    // Control: the instrument can see a file we deliberately kept.
    assert!(
        root.join("solstone/think/detect_created.md").exists(),
        "control path solstone/think/detect_created.md must remain visible"
    );
    for path in [
        "solstone/apps/README.md",
        "solstone/apps/backup/app.json",
        "solstone/apps/body/app.json",
        "solstone/apps/chat/app.json",
        "solstone/apps/entities/app.json",
        "solstone/apps/health/app.json",
        "solstone/apps/home/app.json",
        "solstone/apps/import/app.json",
        "solstone/apps/network/app.json",
        "solstone/apps/search/app.json",
        "solstone/apps/sol/app.json",
        "solstone/apps/sol/workspace.html",
        "solstone/apps/speakers/app.json",
        "solstone/apps/stats/app.json",
        "solstone/apps/support/app.json",
        "solstone/apps/thinking/app.json",
        "solstone/apps/tokens/app.json",
        "solstone/apps/tokens/workspace.html",
        "solstone/apps/transcripts/app.json",
    ] {
        assert!(
            !root.join(path).exists(),
            "retired Python web path remains: {path}"
        );
    }
}
