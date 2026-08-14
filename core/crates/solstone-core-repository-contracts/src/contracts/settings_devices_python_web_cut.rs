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
        "solstone/apps/observer/workspace.html",
        "solstone/apps/observer/app.json",
    ] {
        assert!(
            !root.join(path).exists(),
            "retired Python web path remains: {path}"
        );
    }

    let observer_routes = std::fs::read_to_string(root.join("solstone/apps/observer/routes.py"))
        .expect("retained observer wire routes read");
    for retired in [
        "def api_list",
        "def api_create",
        "def api_delete",
        "def api_get_key",
        "\"/api/list\"",
        "\"/api/create\"",
        "\"/api/<key_prefix>\"",
        "\"/api/<key_prefix>/key\"",
    ] {
        assert!(
            !observer_routes.contains(retired),
            "retired observer management surface remains: {retired}"
        );
    }

    for retained in [
        "OBSERVER_CALLOSUM_SSE_ROUTE",
        "@observer_bp.route(\"/register\", methods=[\"POST\"])",
        "@observer_bp.route(\"/ingest\", methods=[\"POST\"])",
        "@observer_bp.route(\"/ingest/manifest\", methods=[\"GET\"])",
        "@observer_bp.route(\"/ingest/manifest/<day>\", methods=[\"GET\"])",
        "@observer_bp.route(\"/ingest/event\", methods=[\"POST\"])",
        "@observer_bp.route(\"/health\", methods=[\"POST\"])",
        "@observer_bp.route(\"/ingest/segments/<day>\")",
        "@observer_bp.route(\"/source/<stream>\", methods=[\"DELETE\"])",
    ] {
        assert!(
            observer_routes.contains(retained),
            "retained observer wire surface is missing: {retained}"
        );
    }

    assert!(
        root.join("core/crates/solstone-core-settings-web/src/lib.rs")
            .exists(),
        "native Settings web owner is present"
    );
    assert!(
        root.join("core/crates/solstone-core-convey-shell/src/devices.rs")
            .exists(),
        "native Devices owner is present"
    );
}
