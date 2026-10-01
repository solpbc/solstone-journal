// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

include!("../../build-support/windows_version_resource.rs");

fn main() {
    // The windowed program: Windows starts it without a console.
    windows_app_resource(
        "journal-app",
        "journal-app.exe",
        std::path::Path::new("assets/journal-generic.ico"),
    );
}
