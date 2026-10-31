// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_speaker_resolve::candidate_tracker::CandidateTracker;

use crate::CliRun;

pub(crate) fn backfill_pool(args: &[String], journal: &Path) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: solstone journal maintenance run speakers:backfill-pool\n".to_owned(),
            exit_code: 2,
        };
    }
    match solstone_core_speaker_resolve::candidate_tracker::backfill_speaker_pool(journal) {
        // Once the pool has caught up, look for the owner's voice the way the
        // Speakers page does, so a candidate is found without the owner opening
        // any particular page.
        solstone_core_speaker_resolve::candidate_tracker::SpeakerPoolCatchUp::Done(report) => {
            match solstone_core_speaker_resolve::owner_voice::refresh_owner_voice(journal) {
                Ok(owner_voice) => CliRun {
                    stdout: format!(
                        "{}\n",
                        serde_json::json!({"pool":report,"owner_voice":owner_voice})
                    ),
                    stderr: String::new(),
                    exit_code: 0,
                },
                Err(error) => CliRun {
                    stdout: format!("{}\n", serde_json::json!({"pool":report})),
                    stderr: format!("owner voice look failed: {error}\n"),
                    exit_code: 1,
                },
            }
        }
        solstone_core_speaker_resolve::candidate_tracker::SpeakerPoolCatchUp::Refuse(refusal) => {
            CliRun {
                stdout: String::new(),
                stderr: format!("{refusal}\n"),
                exit_code: 1,
            }
        }
    }
}

/// Resolve speakers, without the model, in every segment with voice embeddings that has
/// no speaker labels yet or only a stub: segments thought before speakers were resolved
/// everywhere, and any a live run missed. Labelled segments are never touched again here,
/// and no voiceprint learns from this pass.
pub(crate) fn backfill_labels(args: &[String], journal: &Path) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: solstone journal maintenance run speakers:backfill-labels\n".to_owned(),
            exit_code: 2,
        };
    }
    let request = solstone_core_speaker_resolve::backfill_coordinator::StartBackfillRequest {
        operation_id: None,
        commit: true,
        reattribute: false,
        accumulation: false,
        now_ms: chrono::Utc::now().timestamp_millis(),
    };
    match solstone_core_speaker_resolve::backfill_coordinator::run_backfill_blocking(
        journal, &request,
    ) {
        Ok(status) => CliRun {
            stdout: format!(
                "{}\n",
                serde_json::to_value(&status).unwrap_or(serde_json::Value::Null)
            ),
            stderr: String::new(),
            exit_code: 0,
        },
        Err(error) => CliRun {
            stdout: String::new(),
            stderr: format!("speaker label backfill failed: {error}\n"),
            exit_code: 1,
        },
    }
}

pub(crate) fn consolidate(args: &[String], journal: &Path) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: solstone journal maintenance run speakers:consolidate-pool\n"
                .to_owned(),
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
            stderr: "usage: solstone journal maintenance run speakers:name-variants\n".to_owned(),
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
            stderr: "usage: solstone journal maintenance run speakers:candidate-pair-suggestions\n"
                .to_owned(),
            exit_code: 2,
        };
    }
    match solstone_core_speaker_resolve::candidate_pair_suggestions::refresh_candidate_pair_suggestions(journal) {
        Ok(report) => CliRun { stdout:format!("{report}\n"), stderr:String::new(), exit_code:0 },
        Err(error) => CliRun { stdout:String::new(), stderr:format!("speaker candidate-pair refresh failed: {error}\n"), exit_code:1 },
    }
}

pub(crate) fn discovery(
    args: &[String],
    journal: &Path,
    #[cfg(windows)] generation: &solstone_core_system::process::ChildLaunchContext,
) -> CliRun {
    use solstone_core_speaker_resolve::discovery_scan::{
        DiscoveryRefresh, DiscoveryRefreshError, refresh_discovery_cache,
    };
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: solstone journal maintenance run speakers:discover-voices\n".to_owned(),
            exit_code: 2,
        };
    }
    let report = match refresh_discovery_cache(
        journal,
        None,
        #[cfg(windows)]
        generation,
    ) {
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
    fn run_discovery(args: &[String], root: &std::path::Path) -> crate::CliOutcome {
        #[cfg(windows)]
        {
            // These no-helper cases need no launch grants. An unexpected helper
            // launch still refuses with this empty context.
            crate::CliOutcome::Rendered(crate::run_cli_with_discovery_generation(
                args,
                root,
                &solstone_core_system::process::ChildLaunchContext::default(),
            ))
        }
        #[cfg(not(windows))]
        crate::run_cli(args, root)
    }

    #[test]
    fn registered_discovery_preserves_cache_until_owner_voice_is_available() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("awareness")).unwrap();
        let cache = root.path().join("awareness/discovery_clusters.json");
        std::fs::write(&cache, "prior cache").unwrap();
        let mut args = vec!["run".to_owned(), "speakers:discover-voices".to_owned()];
        let result = run_discovery(&args, root.path());
        assert_eq!(result.output().2, 0, "{}", result.output().1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(result.output().0).unwrap()["status"],
            "skipped"
        );
        std::fs::create_dir_all(root.path().join("entities/owner")).unwrap();
        std::fs::write(
            root.path().join("entities/owner/entity.json"),
            r#"{"id":"owner","name":"Owner","type":"Person","is_principal":true}"#,
        )
        .unwrap();
        let result = run_discovery(&args, root.path());
        assert_eq!(result.output().2, 0, "{}", result.output().1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(result.output().0).unwrap()["reason_code"],
            "speaker_discovery_owner_voice_unavailable"
        );
        assert_eq!(std::fs::read(&cache).unwrap(), b"prior cache");
        args.push("--commit".to_owned());
        assert_eq!(run_discovery(&args, root.path()).output().2, 2);
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
        let result = run_discovery(
            &["run".to_owned(), "speakers:discover-voices".to_owned()],
            root.path(),
        );
        assert_eq!(result.output().2, 0, "{}", result.output().1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(result.output().0).unwrap()["clusters"],
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
        assert_eq!(result.output().2, 0, "{}", result.output().1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(result.output().0).unwrap()["found"],
            0
        );
        assert!(
            !root
                .path()
                .join("speakers/candidate-pair-review-candidates.jsonl")
                .exists()
        );
        args.push("--commit".to_owned());
        assert_eq!(crate::run_cli(&args, root.path()).output().2, 2);
    }

    #[test]
    fn registered_name_variants_run_without_creating_empty_suggestions() {
        let root = tempfile::tempdir().unwrap();
        let args = vec!["run".to_owned(), "speakers:name-variants".to_owned()];
        let result = crate::run_cli(&args, root.path());
        assert_eq!(result.output().2, 0, "{}", result.output().1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(result.output().0).unwrap()["found"],
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
        assert_eq!(crate::run_cli(&bad, root.path()).output().2, 2);
    }

    #[test]
    fn registered_consolidation_runs_and_refuses_unexpected_arguments() {
        let root = tempfile::tempdir().unwrap();
        let mut args = vec!["run".to_owned(), "speakers:consolidate-pool".to_owned()];
        let result = crate::run_cli(&args, root.path());
        assert_eq!(result.output().2, 0, "{}", result.output().1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(result.output().0).unwrap()["merged"],
            0
        );
        assert!(
            !root
                .path()
                .join("awareness/speaker_candidates.json")
                .exists()
        );
        args.push("--commit".to_owned());
        assert_eq!(crate::run_cli(&args, root.path()).output().2, 2);
        std::fs::write(
            root.path().join("awareness/speaker_candidates.json"),
            "invalid",
        )
        .unwrap();
        assert_eq!(crate::run_cli(&args[..2], root.path()).output().2, 1);
    }

    #[test]
    fn registered_backfill_pool_runs_and_refuses_unexpected_arguments() {
        let root = tempfile::tempdir().unwrap();
        let mut args = vec!["run".to_owned(), "speakers:backfill-pool".to_owned()];
        let result = crate::run_cli(&args, root.path());
        assert_eq!(result.output().2, 0, "{}", result.output().1);
        assert!(result.output().1.is_empty());
        let report = serde_json::from_str::<serde_json::Value>(result.output().0).unwrap();
        assert_eq!(report["pool"]["complete"], false);
        assert_eq!(report["pool"]["sources_read"], 0);
        assert_eq!(report["owner_voice"]["reason"], "identity_invalid");

        args.push("--commit".to_owned());
        let bad_arg = crate::run_cli(&args, root.path());
        assert_eq!(bad_arg.output().2, 2);
        assert!(bad_arg.output().0.is_empty());
        assert_eq!(
            bad_arg.output().1,
            "usage: solstone journal maintenance run speakers:backfill-pool\n"
        );

        let awareness = root.path().join("awareness");
        std::fs::create_dir_all(&awareness).unwrap();
        std::fs::write(awareness.join("speaker_candidates_backfill.json"), b"{").unwrap();
        let invalid = crate::run_cli(&args[..2], root.path());
        assert_eq!(invalid.output().2, 1);
        assert!(invalid.output().0.is_empty());
        assert_eq!(
            invalid.output().1,
            "voice list catch-up refused: catch-up progress file is invalid; removing awareness/speaker_candidates_backfill.json from your journal restarts the catch-up safely\n"
        );
    }

    fn snapshot(root: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        fn walk(
            root: &std::path::Path,
            dir: &std::path::Path,
            files: &mut std::collections::BTreeMap<String, Vec<u8>>,
        ) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, files);
                } else {
                    let key = path
                        .strip_prefix(root)
                        .unwrap()
                        .iter()
                        .map(|part| part.to_str().unwrap())
                        .collect::<Vec<_>>()
                        .join("/");
                    files.insert(key, std::fs::read(&path).unwrap());
                }
            }
        }
        let mut files = std::collections::BTreeMap::new();
        walk(root, root, &mut files);
        files
    }

    /// A long-confirmed owner: a voice confirmed in August in the pre-2.x
    /// format (seven members, no evidence hash, never refreshed), and a pool
    /// large enough that detection could otherwise offer a candidate. The
    /// scheduled look must change nothing: no candidate, no past-label update,
    /// no awareness write.
    #[test]
    fn scheduled_look_leaves_a_confirmed_pre_2x_voice_alone() {
        use serde_json::json;
        use solstone_core_speaker_resolve::owner_centroid::{
            OwnerCentroidWriteInput, load_owner_centroid, write_owner_centroid,
        };
        use solstone_core_speaker_resolve::owner_voice::{OwnerVoiceSetup, owner_voice_setup};

        let root = tempfile::tempdir().unwrap();
        let journal = root.path();
        std::fs::create_dir_all(journal.join("entities/owner")).unwrap();
        std::fs::write(
            journal.join("entities/owner/entity.json"),
            r#"{"id":"owner","name":"Owner","type":"Person","is_principal":true}"#,
        )
        .unwrap();
        let mut voice = vec![0.0_f32; 256];
        voice[0] = 1.0;
        write_owner_centroid(
            journal,
            "owner",
            &OwnerCentroidWriteInput {
                centroid: voice.clone(),
                cluster_size: 3000,
                timestamp: "2026-08-05T15:49:00+00:00".to_owned(),
                evidence_tier: "strong".to_owned(),
            },
        )
        .unwrap();
        let centroid = load_owner_centroid(journal, "owner").unwrap().unwrap();
        assert_eq!(centroid.evidence_hash, None);
        assert_eq!(
            centroid.last_refreshed_at.as_deref(),
            Some("2026-08-05T15:49:00+00:00")
        );
        std::fs::create_dir_all(journal.join("awareness")).unwrap();
        std::fs::write(
            journal.join("awareness/current.json"),
            json!({"voiceprint":{"status":"confirmed","cluster_size":3000,"confirmed_at":"2026-08-05T15:49:00+00:00","detected_at":"2026-08-05T15:40:00+00:00","evidence_tier":"strong","recommendation":"ready","samples":[],"streams_represented":6}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            journal.join("awareness/speaker_candidates.json"),
            json!({"next_id":2,"candidates":[{"cand_id":1,"centroid":voice,"n_segments":40,"n_intervals":400,"total_duration_s":800.0,"source_segments":[],"confirmed_entity":null,"status":"pending","merge_events":[]}]}).to_string(),
        )
        .unwrap();
        let segment = journal.join("chronicle/20260806/default/093000_300/talents");
        std::fs::create_dir_all(&segment).unwrap();
        std::fs::write(
            segment.join("speaker_labels.json"),
            json!({"labels":[{"sentence_id":1,"speaker":"owner","confidence":"high","method":"owner_centroid"}],"owner_centroid_last_refreshed_at":"2026-08-05T15:49:00+00:00"}).to_string(),
        )
        .unwrap();
        let before = snapshot(journal);

        let args = vec!["run".to_owned(), "speakers:backfill-pool".to_owned()];
        let result = crate::run_cli(&args, journal);
        assert_eq!(result.output().2, 0, "{}", result.output().1);
        let report = serde_json::from_str::<serde_json::Value>(result.output().0).unwrap();
        assert_eq!(report["owner_voice"]["status"], "skipped", "{report}");
        assert_eq!(report["owner_voice"]["reason"], "voice_saved", "{report}");

        let after = snapshot(journal);
        let changed = before
            .keys()
            .chain(after.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|path| before.get(*path) != after.get(*path))
            // The pool catch-up keeps its own progress record and pool lock;
            // neither is owner-voice state. The pool itself must not change.
            .filter(|path| {
                !path.starts_with("awareness/speaker_candidates_backfill")
                    && path.as_str() != "awareness/speaker_candidates.json.lock"
            })
            .cloned()
            .collect::<Vec<_>>();
        assert!(changed.is_empty(), "the scheduled look changed {changed:?}");
        assert!(!journal.join("awareness/owner_candidate.npz").exists());
        assert!(!journal.join("speakers/backfill-operations.jsonl").exists());
        assert_eq!(owner_voice_setup(journal), OwnerVoiceSetup::Saved);
    }
}
