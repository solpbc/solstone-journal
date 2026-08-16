// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! System-prefix mapping shared by the deb and rpm writers.
//!
//! Tree dest → archive dest (no leading slash):
//!   `bin/`   → `usr/bin/`
//!   `lib/`   → `usr/lib/`
//!   `share/` → `usr/share/`
//!
//! Installed path is `/` + archive dest. Both container writers must call
//! [`to_system_path`] so normalized manifests stay identical.

pub const SYSTEM_PREFIX: &str = "usr";

#[must_use]
pub fn to_system_path(tree_dest: &str) -> Option<String> {
    for top in ["bin/", "lib/", "share/"] {
        if tree_dest.starts_with(top) || tree_dest == top.trim_end_matches('/') {
            return Some(format!("{SYSTEM_PREFIX}/{tree_dest}"));
        }
    }
    None
}

#[must_use]
pub fn from_system_path(archive_dest: &str) -> Option<String> {
    archive_dest
        .strip_prefix(&format!("{SYSTEM_PREFIX}/"))
        .map(str::to_owned)
}
