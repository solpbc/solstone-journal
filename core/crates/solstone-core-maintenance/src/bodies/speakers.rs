// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_speaker_resolve::candidate_tracker::CandidateTracker;

use crate::CliRun;

pub(crate) fn consolidate(args: &[String], journal: &Path) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: journal maintenance run speakers:consolidate-pool\n".to_owned(),
            exit_code: 2,
        };
    }
    match CandidateTracker::new(journal).consolidate_dense_candidates() {
        Ok(report) => CliRun {
            stdout: format!("{report}\n"),
            stderr: String::new(),
            exit_code: 0,
        },
        Err(error) => CliRun {
            stdout: String::new(),
            stderr: format!("speaker candidate consolidation failed: {error}\n"),
            exit_code: 1,
        },
    }
}

pub(crate) fn name_variants(args: &[String], journal: &Path) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: journal maintenance run speakers:name-variants\n".to_owned(),
            exit_code: 2,
        };
    }
    let result = (|| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let scan =
            solstone_core_speaker_resolve::name_variant_scan::detect_name_variant_candidates(
                journal,
            )?;
        let (mut created, mut updated, mut suppressed) = (0, 0, 0);
        for candidate in &scan.candidates {
            let (_, was_created, was_suppressed) = solstone_core_speaker_resolve::speaker_review_candidates::record_name_variant_candidate(journal, candidate)?;
            if was_created {
                created += 1;
            } else {
                updated += 1;
            }
            if was_suppressed {
                suppressed += 1;
            }
        }
        Ok(
            serde_json::json!({"found": scan.candidates.len(), "created": created, "updated": updated, "suppressed": suppressed}),
        )
    })();
    match result {
        Ok(report) => CliRun {
            stdout: format!("{report}\n"),
            stderr: String::new(),
            exit_code: 0,
        },
        Err(error) => CliRun {
            stdout: String::new(),
            stderr: format!("speaker name-variant refresh failed: {error}\n"),
            exit_code: 1,
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn registered_name_variants_run_without_creating_empty_suggestions() {
        let root = tempfile::tempdir().unwrap();
        let args = vec!["run".to_owned(), "speakers:name-variants".to_owned()];
        let result = crate::run_cli(&args, root.path());
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result.stdout).unwrap()["found"],
            0
        );
        assert!(
            !root
                .path()
                .join("speakers/review-candidates.jsonl")
                .exists()
        );
        let mut bad = args;
        bad.push("--commit".into());
        assert_eq!(crate::run_cli(&bad, root.path()).exit_code, 2);
    }

    #[test]
    fn registered_consolidation_runs_and_refuses_unexpected_arguments() {
        let root = tempfile::tempdir().unwrap();
        let mut args = vec!["run".to_owned(), "speakers:consolidate-pool".to_owned()];
        let result = crate::run_cli(&args, root.path());
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result.stdout).unwrap()["merged"],
            0
        );
        assert!(
            !root
                .path()
                .join("awareness/speaker_candidates.json")
                .exists()
        );
        args.push("--commit".to_owned());
        assert_eq!(crate::run_cli(&args, root.path()).exit_code, 2);
        std::fs::write(
            root.path().join("awareness/speaker_candidates.json"),
            "invalid",
        )
        .unwrap();
        assert_eq!(crate::run_cli(&args[..2], root.path()).exit_code, 1);
    }
}
