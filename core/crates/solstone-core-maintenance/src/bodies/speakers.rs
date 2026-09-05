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

pub(crate) fn candidate_pairs(args: &[String], journal: &Path) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: journal maintenance run speakers:candidate-pair-suggestions\n"
                .to_owned(),
            exit_code: 2,
        };
    }
    match solstone_core_speaker_resolve::candidate_pair_suggestions::refresh_candidate_pair_suggestions(journal) {
        Ok(report) => CliRun { stdout:format!("{report}\n"), stderr:String::new(), exit_code:0 },
        Err(error) => CliRun { stdout:String::new(), stderr:format!("speaker candidate-pair refresh failed: {error}\n"), exit_code:1 },
    }
}

pub(crate) fn discovery(args: &[String], journal: &Path) -> CliRun {
    use solstone_core_speaker_resolve::discovery_scan::{
        DiscoveryRefresh, DiscoveryRefreshError, refresh_discovery_cache,
    };
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: journal maintenance run speakers:discover-voices\n".to_owned(),
            exit_code: 2,
        };
    }
    let report = match refresh_discovery_cache(journal, None) {
        Ok(DiscoveryRefresh::IdentityInvalid) => {
            serde_json::json!({"status":"skipped","reason_code":"speaker_owner_identity_invalid"})
        }
        Ok(DiscoveryRefresh::NoConfirmedOwner) => {
            serde_json::json!({"status":"skipped","reason_code":"speaker_discovery_owner_voice_unavailable"})
        }
        Ok(DiscoveryRefresh::Refreshed {
            clusters,
            dropped_invalid,
        }) => {
            serde_json::json!({"status":if dropped_invalid==0 {"ok"} else {"degraded"},"clusters":clusters.len(),"dropped_invalid":dropped_invalid})
        }
        Err(error) => {
            let exit_code = if matches!(error, DiscoveryRefreshError::Helper(_)) {
                2
            } else {
                1
            };
            return CliRun {
                stdout: String::new(),
                stderr: format!("{error}\n"),
                exit_code,
            };
        }
    };
    CliRun {
        stdout: format!("{report}\n"),
        stderr: String::new(),
        exit_code: 0,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn registered_discovery_preserves_cache_until_owner_voice_is_available() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("awareness")).unwrap();
        let cache = root.path().join("awareness/discovery_clusters.json");
        std::fs::write(&cache, "prior cache").unwrap();
        let mut args = vec!["run".to_owned(), "speakers:discover-voices".to_owned()];
        let result = crate::run_cli(&args, root.path());
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result.stdout).unwrap()["status"],
            "skipped"
        );
        std::fs::create_dir_all(root.path().join("entities/owner")).unwrap();
        std::fs::write(
            root.path().join("entities/owner/entity.json"),
            r#"{"id":"owner","name":"Owner","type":"Person","is_principal":true}"#,
        )
        .unwrap();
        let result = crate::run_cli(&args, root.path());
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result.stdout).unwrap()["reason_code"],
            "speaker_discovery_owner_voice_unavailable"
        );
        assert_eq!(std::fs::read(&cache).unwrap(), b"prior cache");
        args.push("--commit".to_owned());
        assert_eq!(crate::run_cli(&args, root.path()).exit_code, 2);
        assert_eq!(std::fs::read(&cache).unwrap(), b"prior cache");
    }

    #[test]
    fn registered_discovery_clears_empty_cache_with_confirmed_owner_without_invoking_helper() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("entities/owner")).unwrap();
        std::fs::write(
            root.path().join("entities/owner/entity.json"),
            r#"{"id":"owner","name":"Owner","type":"Person","is_principal":true}"#,
        )
        .unwrap();
        let mut centroid = vec![0.0; 256];
        centroid[0] = 1.0;
        solstone_core_speaker_resolve::owner_centroid::write_owner_centroid(
            root.path(),
            "owner",
            &solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteInput {
                centroid,
                cluster_size: 5,
                timestamp: "2026-08-08T00:00:00Z".to_owned(),
                evidence_tier: "standard".to_owned(),
            },
        )
        .unwrap();
        std::fs::create_dir_all(root.path().join("awareness")).unwrap();
        for name in [
            "discovery_clusters.json",
            "discovery_clusters.resolved.json",
        ] {
            std::fs::write(root.path().join("awareness").join(name), "prior cache").unwrap();
        }
        let result = crate::run_cli(
            &["run".to_owned(), "speakers:discover-voices".to_owned()],
            root.path(),
        );
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result.stdout).unwrap()["clusters"],
            0
        );
        for name in [
            "discovery_clusters.json",
            "discovery_clusters.resolved.json",
        ] {
            assert!(!root.path().join("awareness").join(name).exists());
        }
    }

    #[test]
    fn registered_candidate_pairs_run_and_reject_arguments() {
        let root = tempfile::tempdir().unwrap();
        let mut args = vec![
            "run".to_owned(),
            "speakers:candidate-pair-suggestions".to_owned(),
        ];
        let result = crate::run_cli(&args, root.path());
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result.stdout).unwrap()["found"],
            0
        );
        assert!(
            !root
                .path()
                .join("speakers/candidate-pair-review-candidates.jsonl")
                .exists()
        );
        args.push("--commit".to_owned());
        assert_eq!(crate::run_cli(&args, root.path()).exit_code, 2);
    }

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
