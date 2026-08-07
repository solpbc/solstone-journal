// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

/// Return the canonical config path below `journal_path`.
pub fn get_journal_config_path(journal_path: &Path) -> PathBuf {
    journal_path.join("config").join("journal.json")
}
