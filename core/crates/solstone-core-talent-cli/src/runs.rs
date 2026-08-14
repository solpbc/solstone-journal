// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn find_run_file(talents_dir: &Path, use_id: &str) -> Option<PathBuf> {
    for suffix in [".jsonl", "_active.jsonl"] {
        let entries = fs::read_dir(talents_dir).ok()?;
        for entry in entries.flatten() {
            let subdir = entry.path();
            if !subdir.is_dir() {
                continue;
            }
            let candidate = subdir.join(format!("{use_id}{suffix}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn finds_completed_then_active_runs_in_two_passes() {
        let root = tempfile::tempdir().expect("tempdir");
        let talents = root.path().join("talents");
        fs::create_dir_all(talents.join("complete-only")).expect("complete directory");
        fs::write(talents.join("complete-only/run-a.jsonl"), "").expect("complete run");
        assert_eq!(
            find_run_file(&talents, "run-a"),
            Some(talents.join("complete-only/run-a.jsonl"))
        );

        fs::create_dir_all(talents.join("active-only")).expect("active directory");
        fs::write(talents.join("active-only/run-b_active.jsonl"), "").expect("active run");
        assert_eq!(
            find_run_file(&talents, "run-b"),
            Some(talents.join("active-only/run-b_active.jsonl"))
        );

        fs::create_dir_all(talents.join("active-first")).expect("active priority directory");
        fs::create_dir_all(talents.join("complete-second")).expect("complete priority directory");
        fs::write(talents.join("active-first/run-c_active.jsonl"), "").expect("active run");
        fs::write(talents.join("complete-second/run-c.jsonl"), "").expect("complete run");
        assert_eq!(
            find_run_file(&talents, "run-c"),
            Some(talents.join("complete-second/run-c.jsonl"))
        );
    }
}
