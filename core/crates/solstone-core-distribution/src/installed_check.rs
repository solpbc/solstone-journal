// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only check of one installed package root.
//!
//! The root is the operator's subject. This command does not select a root
//! for a running journal and does not write.

use std::path::Path;

use solstone_core_installed_payload::{InstalledPayloadRefusal, verify_installed_package};

pub fn check_installed(
    root: &Path,
    version: &str,
    target: &str,
) -> Result<(), InstalledPayloadRefusal> {
    verify_installed_package(root, version, target)
}

#[must_use]
pub fn refusal_line(refusal: &InstalledPayloadRefusal) -> String {
    match &refusal.path {
        Some(path) => format!("{} {path}\n", refusal.code),
        None => format!("{}\n", refusal.code),
    }
}

#[must_use]
pub fn admitted_line() -> &'static str {
    "admitted\n"
}
