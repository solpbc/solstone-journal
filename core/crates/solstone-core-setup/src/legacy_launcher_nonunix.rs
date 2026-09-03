// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Non-Unix boundary for retired shell launcher recognition.
//!
//! The legacy launchers are executable shell scripts and have no Windows
//! equivalent. Windows setup neither recognizes nor replaces them.

use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LegacyLauncher;

pub(crate) fn validate_effective_path(
    _home: &Path,
    _current_dir: &Path,
    _executable_dir: &Path,
) -> Result<(), String> {
    Ok(())
}

pub(crate) fn classify(
    _home: &Path,
    _public_path: &Path,
    _command: &str,
) -> Result<Option<LegacyLauncher>, String> {
    Ok(None)
}
