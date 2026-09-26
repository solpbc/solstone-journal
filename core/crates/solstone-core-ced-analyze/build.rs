// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

include!("../../build-support/windows_version_resource.rs");

fn main() {
    windows_version_resource("solstone-core-ced-analyze", "solstone-core-ced-analyze.exe");
}
