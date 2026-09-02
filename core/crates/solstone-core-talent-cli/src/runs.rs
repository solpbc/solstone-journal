// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

pub(crate) fn find_run_file(talents_dir: &Path, use_id: &str) -> Option<PathBuf> {
    for suffix in [".jsonl", "_active.jsonl"] {
        let entries = fs::read_dir(talents_dir).ok()?;
        for entry in entries.flatten() {
            let subdir = entry.path();
            if !subdir.is_dir() {
                continue;
            }
            let candidate = subdir.join(format!("{use_id}{suffix}"));
            if candidate.is_file() && candidate_matches_use_id(&candidate, use_id) {
                return Some(candidate);
            }
        }
    }
    None
}

fn candidate_matches_use_id(path: &Path, use_id: &str) -> bool {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let Some(line) = BufReader::new(file).lines().next() else {
        return false;
    };
    let Ok(line) = line else {
        return false;
    };
    let line = line.trim();
    if line.is_empty() {
        return false;
    }
    match serde_json::from_str::<Value>(line) {
        Ok(event) => event.get("use_id").and_then(Value::as_str) == Some(use_id),
        Err(_) => false,
    }
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
        fs::write(
            talents.join("complete-only/run-a.jsonl"),
            r#"{"event":"request","use_id":"run-a"}"#,
        )
        .expect("complete run");
        assert_eq!(
            find_run_file(&talents, "run-a"),
            Some(talents.join("complete-only/run-a.jsonl"))
        );

        fs::create_dir_all(talents.join("active-only")).expect("active directory");
        fs::write(
            talents.join("active-only/run-b_active.jsonl"),
            r#"{"event":"request","use_id":"run-b"}"#,
        )
        .expect("active run");
        assert_eq!(
            find_run_file(&talents, "run-b"),
            Some(talents.join("active-only/run-b_active.jsonl"))
        );

        fs::create_dir_all(talents.join("active-first")).expect("active priority directory");
        fs::create_dir_all(talents.join("complete-second")).expect("complete priority directory");
        fs::write(
            talents.join("active-first/run-c_active.jsonl"),
            r#"{"event":"request","use_id":"run-c"}"#,
        )
        .expect("active run");
        fs::write(
            talents.join("complete-second/run-c.jsonl"),
            r#"{"event":"request","use_id":"run-c"}"#,
        )
        .expect("complete run");
        assert_eq!(
            find_run_file(&talents, "run-c"),
            Some(talents.join("complete-second/run-c.jsonl"))
        );
    }

    fn write_run(talents: &Path, dir: &str, name: &str, body: impl AsRef<[u8]>) {
        let path = talents.join(dir).join(name);
        fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        fs::write(path, body).expect("run");
    }

    #[test]
    fn candidate_matches_use_id_classifies_content() {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("tempdir");
        let path = root.path().join("run.jsonl");
        fs::write(&path, r#"{"event":"request","use_id":"hit"}"#).expect("match");
        assert!(candidate_matches_use_id(&path, "hit"));
        fs::write(&path, r#"{"event":"request","use_id":"other"}"#).expect("mismatch");
        assert!(!candidate_matches_use_id(&path, "hit"));
        fs::write(&path, r#"{"event":"request"}"#).expect("missing");
        assert!(!candidate_matches_use_id(&path, "hit"));
        fs::write(&path, "").expect("empty");
        assert!(!candidate_matches_use_id(&path, "hit"));
        fs::write(&path, "not-json\n").expect("invalid");
        assert!(!candidate_matches_use_id(&path, "hit"));
        fs::write(&path, [0xff, 0xfe]).expect("non-utf8");
        assert!(!candidate_matches_use_id(&path, "hit"));
        fs::remove_file(&path).expect("vanish");
        assert!(!candidate_matches_use_id(&path, "hit"));
    }

    #[test]
    fn find_run_file_skips_non_matching_content() {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("tempdir");
        let talents = root.path().join("talents");
        write_run(
            &talents,
            "wrong",
            "run-a.jsonl",
            r#"{"event":"request","use_id":"other"}"#,
        );
        assert_eq!(find_run_file(&talents, "run-a"), None);
        write_run(&talents, "missing", "run-b.jsonl", r#"{"event":"request"}"#);
        assert_eq!(find_run_file(&talents, "run-b"), None);
        for (dir, body) in [
            ("empty", &b""[..]),
            ("invalid", b"not-json\n".as_slice()),
            ("utf8", [0xff, 0xfe].as_slice()),
        ] {
            write_run(&talents, dir, "run-c.jsonl", body);
            assert_eq!(find_run_file(&talents, "run-c"), None, "{dir}");
            fs::remove_file(talents.join(dir).join("run-c.jsonl")).expect("cleanup");
        }
    }

    #[test]
    fn wrong_use_id_decoy_does_not_mask_genuine_match_in_either_directory_order() {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("tempdir");
        let talents = root.path().join("talents");
        for (decoy, genuine) in [("aaa", "zzz"), ("zzz", "aaa")] {
            let _ = fs::remove_dir_all(&talents);
            write_run(
                &talents,
                decoy,
                "run-d.jsonl",
                r#"{"event":"request","use_id":"decoy-wrong"}"#,
            );
            write_run(
                &talents,
                genuine,
                "run-d.jsonl",
                r#"{"event":"request","use_id":"run-d"}"#,
            );
            assert_eq!(
                find_run_file(&talents, "run-d"),
                Some(talents.join(genuine).join("run-d.jsonl")),
                "decoy={decoy} genuine={genuine}"
            );
        }
    }
}
