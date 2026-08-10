// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

pub fn observers_dir(journal_root: &Path) -> PathBuf {
    journal_root.join("apps/observer/observers")
}

pub fn history_dir(journal_root: &Path, prefix: &str) -> PathBuf {
    observers_dir(journal_root).join(prefix).join("hist")
}

pub fn observer_path(journal_root: &Path, prefix: &str) -> PathBuf {
    observers_dir(journal_root).join(format!("{prefix}.json"))
}

pub fn history_path(journal_root: &Path, prefix: &str, day: &str) -> PathBuf {
    history_dir(journal_root, prefix).join(format!("{day}.jsonl"))
}
