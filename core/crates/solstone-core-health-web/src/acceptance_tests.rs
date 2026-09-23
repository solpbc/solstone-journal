// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use chrono::{Duration, TimeZone, Utc};
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::search_freshness::{
        IndexMetadata, SEARCH_NOTE_ATTEMPT_FAILED, SEARCH_TEXT_BEHIND_7_DAYS,
        SEARCH_TEXT_BEHIND_ATTEMPT_FAILED, SEARCH_TEXT_CURRENT, SEARCH_TEXT_UNCLEAR,
        evaluate_search_freshness,
    };
    use crate::{Clock, backlog, routes_with_clock};
    use solstone_core_system_health::{IndexerPhase, SummaryFreshness};

    struct StubIndexMetadata {
        entries: Mutex<BTreeMap<PathBuf, std::io::Result<SystemTime>>>,
    }

    impl StubIndexMetadata {
        fn new() -> Self {
            Self {
                entries: Mutex::new(BTreeMap::new()),
            }
        }

        fn insert(&self, path: impl Into<PathBuf>, res: std::io::Result<SystemTime>) {
            self.entries.lock().unwrap().insert(path.into(), res);
        }
    }

    impl IndexMetadata for StubIndexMetadata {
        fn modified(&self, path: &Path) -> std::io::Result<SystemTime> {
            if let Some(res) = self.entries.lock().unwrap().get(path) {
                match res {
                    Ok(t) => Ok(*t),
                    Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
                }
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "not found",
                ))
            }
        }
    }

    #[test]
    fn test_6_search_freshness_precedence_and_evaluation_rules() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let sqlite = root.join("indexer/journal.sqlite");
        let wal = root.join("indexer/journal.sqlite-wal");
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let now_sys = UNIX_EPOCH + StdDuration::from_secs(1_800_000_000);

        let stub = StubIndexMetadata::new();

        // 1. sqlite NotFound -> unknown, flag false, updated_at_ms null (ignore WAL)
        stub.insert(
            &sqlite,
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not found",
            )),
        );
        stub.insert(&wal, Ok(now_sys));
        let eval = evaluate_search_freshness(root, &stub, None, SummaryFreshness::Fresh, now);
        assert_eq!(eval.state, "unknown");
        assert_eq!(eval.updated_at_ms, None);
        assert!(!eval.last_attempt_failed);
        assert_eq!(eval.text, SEARCH_TEXT_UNCLEAR);

        // 2. Other metadata error on sqlite -> unknown, flag false, updated_at_ms null
        stub.insert(
            &sqlite,
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
        );
        let eval = evaluate_search_freshness(root, &stub, None, SummaryFreshness::Fresh, now);
        assert_eq!(eval.state, "unknown");
        assert_eq!(eval.updated_at_ms, None);
        assert!(!eval.last_attempt_failed);
        assert_eq!(eval.text, SEARCH_TEXT_UNCLEAR);

        // 3. Other metadata error on existing WAL -> unknown, flag false, do not return Err
        stub.insert(&sqlite, Ok(now_sys));
        stub.insert(
            &wal,
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
        );
        let eval = evaluate_search_freshness(root, &stub, None, SummaryFreshness::Fresh, now);
        assert_eq!(eval.state, "unknown");
        assert_eq!(eval.updated_at_ms, None);
        assert!(!eval.last_attempt_failed);
        assert_eq!(eval.text, SEARCH_TEXT_UNCLEAR);

        // 4. Newest mtime more than 5 minutes ahead -> unknown
        stub.insert(&sqlite, Ok(now_sys + StdDuration::from_secs(301)));
        stub.insert(
            &wal,
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not found",
            )),
        );
        let eval = evaluate_search_freshness(root, &stub, None, SummaryFreshness::Fresh, now);
        assert_eq!(eval.state, "unknown");
        assert_eq!(eval.updated_at_ms, Some(1_800_000_301 * 1000));
        assert!(!eval.last_attempt_failed);
        assert_eq!(eval.text, SEARCH_TEXT_UNCLEAR);

        // 5. Age strictly over 7 days -> stale, flag false (wins over summary and attempt)
        let eight_days_ago = now_sys - StdDuration::from_secs(8 * 86400);
        stub.insert(&sqlite, Ok(eight_days_ago));
        let failed_indexer = IndexerPhase {
            success: false,
            run_started_at_ms: 100,
            reason_code: Some("failed".to_owned()),
        };
        let eval = evaluate_search_freshness(
            root,
            &stub,
            Some(&failed_indexer),
            SummaryFreshness::Stale,
            now,
        );
        assert_eq!(eval.state, "stale");
        assert!(!eval.last_attempt_failed);
        assert_eq!(eval.text, SEARCH_TEXT_BEHIND_7_DAYS);

        // 6. Summary missing, unreadable, degraded, or not Fresh -> unknown, flag false
        let one_day_ago = now_sys - StdDuration::from_secs(86400);
        stub.insert(&sqlite, Ok(one_day_ago));
        let eval = evaluate_search_freshness(
            root,
            &stub,
            Some(&failed_indexer),
            SummaryFreshness::Stale,
            now,
        );
        assert_eq!(eval.state, "unknown");
        assert!(!eval.last_attempt_failed);
        assert_eq!(eval.text, SEARCH_TEXT_UNCLEAR);

        // 7. indexer_phase.success == false -> stale, flag true
        let eval = evaluate_search_freshness(
            root,
            &stub,
            Some(&failed_indexer),
            SummaryFreshness::Fresh,
            now,
        );
        assert_eq!(eval.state, "stale");
        assert!(eval.last_attempt_failed);
        assert_eq!(eval.text, SEARCH_TEXT_BEHIND_ATTEMPT_FAILED);

        // 8. Else fresh
        let eval = evaluate_search_freshness(root, &stub, None, SummaryFreshness::Fresh, now);
        assert_eq!(eval.state, "fresh");
        assert!(!eval.last_attempt_failed);
        assert_eq!(eval.text, SEARCH_TEXT_CURRENT);
    }

    #[test]
    fn test_7_synthesis_health_notes_and_metadata_behavior() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        let now = Utc::now();
        let fixed_now = chrono::DateTime::<chrono::FixedOffset>::from(now);

        // 1. Missing indexer database -> note pushed
        let (_synth, notes) = crate::journal_data::report::build_synthesis_health(
            root,
            &crate::journal_data::report::ScanAggregate::default(),
            fixed_now,
        )
        .unwrap();
        assert!(
            notes.iter().any(|n| n.message
                == "indexer database missing at journal/indexer/journal.sqlite; search-backed consumers may be stale.")
        );

        // 2. Indexer database exists and indexer attempt failed -> attempt note pushed
        fs::create_dir_all(root.join("indexer")).unwrap();
        fs::write(root.join("indexer/journal.sqlite"), b"sqlite").unwrap();

        let stats = json!({
            "generated_at": now.to_rfc3339(),
            "backlog": {
                "indexer_phase": {
                    "success": false,
                    "reason_code": "lock_failed",
                    "run_started_at_ms": 1_000
                }
            }
        });
        fs::write(root.join("stats.json"), stats.to_string()).unwrap();

        let (_synth, notes) = crate::journal_data::report::build_synthesis_health(
            root,
            &crate::journal_data::report::ScanAggregate::default(),
            fixed_now,
        )
        .unwrap();
        assert!(
            notes
                .iter()
                .any(|n| n.message == SEARCH_NOTE_ATTEMPT_FAILED)
        );
    }

    #[test]
    fn test_8_api_state_response_structure_and_verdict_wiring() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        let now = Utc::now();
        let stats = json!({
            "generated_at": now.to_rfc3339(),
            "backlog": {
                "pending_days": 1,
                "stuck_days": 0,
                "oldest_pending_day": "20990101",
                "days": [
                    {
                        "day": "20990101",
                        "state": "complete",
                        "unfinished_activities": {
                            "activities": 2,
                            "units": []
                        }
                    }
                ],
                "indexer_phase": {
                    "success": true,
                    "run_started_at_ms": 1_000
                }
            }
        });
        fs::write(root.join("stats.json"), stats.to_string()).unwrap();

        // Create indexer sqlite file
        fs::create_dir_all(root.join("indexer")).unwrap();
        fs::write(root.join("indexer/journal.sqlite"), b"sqlite").unwrap();

        let clock = Clock::fixed(now);
        let router = routes_with_clock(root.to_path_buf(), clock);

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let response = router
                    .oneshot(
                        Request::get("/app/health/api/state")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();

                assert_eq!(response.status(), 200);
                let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();

                let backlog = &body["backlog"];
                assert_eq!(backlog["verdict"], "1 day is still catching up.");
                assert_eq!(backlog["pending_days"], 1);
                assert_eq!(backlog["oldest_pending_day"], "20990101");
                assert_eq!(backlog["freshness"]["state"], "fresh");
                assert_eq!(backlog["freshness"]["generated_at"], now.to_rfc3339());
                assert_eq!(backlog["unfinished_activities"]["activities"], 2);
                assert_eq!(backlog["unfinished_activities"]["day_count"], 1);
                assert_eq!(backlog["unfinished_activities"]["oldest_day"], "20990101");
                assert!(backlog["copy"]["unfinished_template_one"].is_string());
                assert!(backlog["copy"]["search_current"].is_string());

                let search = &body["search_index"];
                assert_eq!(search["state"], "fresh");
                assert_eq!(search["last_attempt_failed"], false);
                assert_eq!(search["text"], SEARCH_TEXT_CURRENT);
            });
    }

    #[test]
    fn test_10_routes_with_clock_uses_injected_clock() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        // Stats generated 40 hours before `now` -> stale
        let old_time = now - Duration::hours(40);
        let stats = json!({
            "generated_at": old_time.to_rfc3339(),
            "backlog": {
                "pending_days": 0,
                "stuck_days": 0
            }
        });
        fs::write(root.join("stats.json"), stats.to_string()).unwrap();

        let clock = Clock::fixed(now);
        let router = routes_with_clock(root.to_path_buf(), clock);

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let response = router
                    .oneshot(
                        Request::get("/app/health/api/state")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();

                assert_eq!(response.status(), 200);
                let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();

                let backlog = &body["backlog"];
                assert_eq!(backlog["freshness"]["state"], "stale");
                assert_eq!(backlog["verdict"], "it's unclear whether your journal is caught up; the last update was 40 hours ago.");
            });
    }

    #[test]
    fn test_11_backlog_load_returns_top_level_generated_at_and_backlog() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // 1. Missing stats.json -> (None, None)
        assert_eq!(backlog::load(root), (None, None));

        // 2. stats.json with top-level generated_at and backlog
        let stats = json!({
            "generated_at": "2026-09-22T20:00:00Z",
            "backlog": {
                "generated_at": "nested-value-to-ignore",
                "stuck_days": 0
            }
        });
        fs::write(root.join("stats.json"), stats.to_string()).unwrap();

        let (gen_at, bl) = backlog::load(root);
        assert_eq!(gen_at.as_deref(), Some("2026-09-22T20:00:00Z"));
        let bl = bl.expect("backlog map");
        assert_eq!(bl["stuck_days"], 0);
    }

    fn configure_daily_work(journal: &Path, enabled: Option<&str>) {
        let (talent, apps) = solstone_core_system::daily_coverage::package_roots().unwrap();
        let configs =
            solstone_core_system::daily_coverage::daily_configs(journal, &talent, &apps).unwrap();
        let overrides = configs
            .into_iter()
            .map(|config| {
                let key = match config.key.split_once(':') {
                    Some((app, name)) => format!("talent.{app}.{name}"),
                    None => format!("talent.system.{}", config.key),
                };
                (
                    key,
                    json!({"disabled": enabled != Some(config.key.as_str())}),
                )
            })
            .collect::<serde_json::Map<String, serde_json::Value>>();
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            serde_json::to_vec(
                &json!({"identity":{"timezone":"UTC"},"talent_overrides":overrides}),
            )
            .unwrap(),
        )
        .unwrap();
    }

    fn write_run_log(root: &Path, day: &str, file: &str, lines: &[&str]) {
        let dir = root.join("chronicle").join(day).join("health");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), lines.join("\n") + "\n").unwrap();
    }

    #[test]
    fn test_backlog_run_cli_and_state_api_freshness() {
        use solstone_core_journal_stats_cli::{
            FilesystemBacklogViewReader, FilesystemDocumentWriter, run_cli,
        };

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        configure_daily_work(root, None);

        let day = "20990101";
        let day_dir = root.join("chronicle").join(day);
        fs::create_dir_all(&day_dir).unwrap();

        // Control journal without fail row
        let temp_control = TempDir::new().unwrap();
        let root_control = temp_control.path();
        configure_daily_work(root_control, None);
        fs::create_dir_all(root_control.join("chronicle").join(day)).unwrap();

        let t_secs = 1_800_000_000i64;
        let t = Utc.timestamp_opt(t_secs, 0).unwrap();
        let (talent_root, apps_root) =
            solstone_core_system::daily_coverage::package_roots().unwrap();

        // Run control journal CLI at T
        let run_res_control = run_cli(
            &[],
            root_control,
            t,
            &talent_root,
            &apps_root,
            &FilesystemBacklogViewReader,
            &FilesystemDocumentWriter,
        );
        assert_eq!(
            run_res_control.exit_code, 0,
            "control run_cli failed: {}",
            run_res_control.stderr
        );

        // Add activity failure to test journal
        let record = json!({
            "ts": 10_000,
            "event": "talent.fail",
            "mode": "activity",
            "name": "entities_observe",
            "facet": "work",
            "activity": "meeting-1",
            "reason_code": "timeout"
        });
        write_run_log(root, day, "001.jsonl", &[&record.to_string()]);

        // Run test journal CLI at T
        let run_res = run_cli(
            &[],
            root,
            t,
            &talent_root,
            &apps_root,
            &FilesystemBacklogViewReader,
            &FilesystemDocumentWriter,
        );
        assert_eq!(run_res.exit_code, 0, "run_cli failed: {}", run_res.stderr);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // Shortly after builder generated_at: T + 10s
        let now_shortly = t + Duration::seconds(10);

        rt.block_on(async {
            // Control check
            let ctrl_router =
                routes_with_clock(root_control.to_path_buf(), Clock::fixed(now_shortly));
            let resp = ctrl_router
                .oneshot(
                    Request::get("/app/health/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
            let ctrl_body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                ctrl_body["backlog"]["verdict"],
                "your journal's all caught up."
            );
            assert_eq!(
                ctrl_body["backlog"]["unfinished_activities"]["activities"],
                0
            );

            // Test journal check
            let router = routes_with_clock(root.to_path_buf(), Clock::fixed(now_shortly));
            let resp = router
                .oneshot(
                    Request::get("/app/health/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();

            let bl = &body["backlog"];
            assert_eq!(bl["freshness"]["state"], "fresh");
            assert_eq!(bl["pending_days"], ctrl_body["backlog"]["pending_days"]);
            assert_eq!(bl["stuck_rows"], ctrl_body["backlog"]["stuck_rows"]);
            assert_eq!(bl["unfinished_activities"]["activities"], 1);
            assert_eq!(bl["unfinished_activities"]["day_count"], 1);
            assert_eq!(bl["unfinished_activities"]["oldest_day"], "20990101");
            assert_eq!(bl["verdict"], "your journal's caught up.");

            // Aging tests:
            // +2h -> fresh
            let r2 = routes_with_clock(root.to_path_buf(), Clock::fixed(t + Duration::hours(2)));
            let resp = r2
                .oneshot(
                    Request::get("/app/health/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let b2: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(b2["backlog"]["freshness"]["state"], "fresh");

            // +40h -> stale, pending_days == 0, unfinished activities 0 (omitted or 0), verdict contains "40 hours" and does not contain "1 day is still catching up."
            let r40 = routes_with_clock(root.to_path_buf(), Clock::fixed(t + Duration::hours(40)));
            let resp = r40
                .oneshot(
                    Request::get("/app/health/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let b40: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(b40["backlog"]["freshness"]["state"], "stale");
            assert_eq!(b40["backlog"]["pending_days"], 0);
            let v40 = b40["backlog"]["verdict"].as_str().unwrap();
            assert!(v40.contains("40 hours"));
            assert!(!v40.contains("1 day is still catching up."));

            // Delete top-level generated_at and rewrite stats.json -> unknown age sentence
            let stats_path = root.join("stats.json");
            let mut stats_val: Value =
                serde_json::from_str(&fs::read_to_string(&stats_path).unwrap()).unwrap();
            stats_val.as_object_mut().unwrap().remove("generated_at");
            fs::write(&stats_path, stats_val.to_string()).unwrap();

            let r_nogen = routes_with_clock(root.to_path_buf(), Clock::fixed(now_shortly));
            let resp = r_nogen
                .oneshot(
                    Request::get("/app/health/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let bnogen: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(bnogen["backlog"]["freshness"]["state"], "unknown");
            assert_eq!(
                bnogen["backlog"]["verdict"],
                "it's unclear whether your journal is caught up; the last update age is unknown."
            );

            // Set generated_at one hour after now -> unknown future sentence
            stats_val["generated_at"] = json!((now_shortly + Duration::hours(1)).to_rfc3339());
            fs::write(&stats_path, stats_val.to_string()).unwrap();

            let r_fut = routes_with_clock(root.to_path_buf(), Clock::fixed(now_shortly));
            let resp = r_fut
                .oneshot(
                    Request::get("/app/health/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let bfut: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(bfut["backlog"]["freshness"]["state"], "unknown");
            assert_eq!(
                bfut["backlog"]["verdict"],
                "it's unclear whether your journal is caught up; the last update age is unknown."
            );

            // Delete stats.json -> still checking where your journal stands.
            fs::remove_file(&stats_path).unwrap();
            let r_del = routes_with_clock(root.to_path_buf(), Clock::fixed(now_shortly));
            let resp = r_del
                .oneshot(
                    Request::get("/app/health/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let bdel: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(
                bdel["backlog"]["verdict"],
                "still checking where your journal stands."
            );
        });
    }

    #[test]
    fn test_indexer_fail_and_success_flow() {
        use filetime::{FileTime, set_file_mtime};
        use solstone_core_journal_stats_cli::{
            FilesystemBacklogViewReader, FilesystemDocumentWriter, run_cli,
        };

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        configure_daily_work(root, None);

        let day = "20990101";
        fs::create_dir_all(root.join("chronicle").join(day)).unwrap();

        let t_secs = 1_800_000_000i64;
        let t = Utc.timestamp_opt(t_secs, 0).unwrap();
        let (talent_root, apps_root) =
            solstone_core_system::daily_coverage::package_roots().unwrap();

        // 1. Create indexer db
        let _conn = solstone_core_indexer_store::db::open_index(root).expect("open index");
        let _ = solstone_core_indexer_store::scan::scan_journal(root, false);

        // Ensure WAL exists and set mtimes so WAL is after T
        let wal_path = root.join("indexer/journal.sqlite-wal");
        if !wal_path.exists() {
            fs::write(&wal_path, b"").unwrap();
        }
        let sqlite_path = root.join("indexer/journal.sqlite");
        set_file_mtime(&sqlite_path, FileTime::from_unix_time(t_secs + 2, 0)).unwrap();
        set_file_mtime(&wal_path, FileTime::from_unix_time(t_secs + 5, 0)).unwrap();

        // Health log has indexer phase.complete failure whose ts is T
        let fail_record = json!({
            "ts": t_secs * 1000,
            "event": "phase.complete",
            "phase": "indexer",
            "success": false,
            "reason_code": "failed"
        });
        write_run_log(root, day, "001.jsonl", &[&fail_record.to_string()]);

        // Run CLI at T
        let run_res = run_cli(
            &[],
            root,
            t,
            &talent_root,
            &apps_root,
            &FilesystemBacklogViewReader,
            &FilesystemDocumentWriter,
        );
        assert_eq!(run_res.exit_code, 0);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let now_shortly = t + Duration::seconds(10);
        rt.block_on(async {
            let router = routes_with_clock(root.to_path_buf(), Clock::fixed(now_shortly));
            let resp = router
                .oneshot(
                    Request::get("/app/health/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let body: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();

            assert_eq!(body["search_index"]["state"], "stale");
            assert_eq!(body["search_index"]["last_attempt_failed"], true);
        });

        // Append a later success: true row
        let succ_record = json!({
            "ts": (t_secs + 5) * 1000,
            "event": "phase.complete",
            "phase": "indexer",
            "success": true
        });
        write_run_log(root, day, "002.jsonl", &[&succ_record.to_string()]);

        // Run CLI again at T + 10s
        let run_res2 = run_cli(
            &[],
            root,
            t + Duration::seconds(10),
            &talent_root,
            &apps_root,
            &FilesystemBacklogViewReader,
            &FilesystemDocumentWriter,
        );
        assert_eq!(run_res2.exit_code, 0);

        rt.block_on(async {
            let router = routes_with_clock(root.to_path_buf(), Clock::fixed(now_shortly));
            let resp = router
                .oneshot(
                    Request::get("/app/health/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let body: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();

            assert_eq!(body["search_index"]["state"], "fresh");
            assert_eq!(body["search_index"]["last_attempt_failed"], false);
        });
    }

    #[test]
    fn test_search_freshness_filesystem_real_index() {
        use filetime::{FileTime, set_file_mtime};

        let temp = TempDir::new().unwrap();
        let root = temp.path();

        let _conn = solstone_core_indexer_store::db::open_index(root).expect("open index");
        let _ = solstone_core_indexer_store::scan::scan_journal(root, false);

        let sqlite = root.join("indexer/journal.sqlite");
        let wal = root.join("indexer/journal.sqlite-wal");
        if !wal.exists() {
            fs::write(&wal, b"").unwrap();
        }

        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let eight_days_ago_secs = 1_800_000_000 - 8 * 86400;

        set_file_mtime(&sqlite, FileTime::from_unix_time(eight_days_ago_secs, 0)).unwrap();
        set_file_mtime(&wal, FileTime::from_unix_time(eight_days_ago_secs, 0)).unwrap();

        let meta = crate::search_freshness::FsIndexMetadata;

        // Call twice; both stale, mtimes unchanged
        let eval1 = evaluate_search_freshness(root, &meta, None, SummaryFreshness::Fresh, now);
        assert_eq!(eval1.state, "stale");
        assert_eq!(eval1.text, SEARCH_TEXT_BEHIND_7_DAYS);

        let eval2 = evaluate_search_freshness(root, &meta, None, SummaryFreshness::Fresh, now);
        assert_eq!(eval2.state, "stale");
        assert_eq!(eval2.text, SEARCH_TEXT_BEHIND_7_DAYS);

        let m1 = fs::metadata(&sqlite).unwrap().modified().unwrap();
        let m2 = fs::metadata(&wal).unwrap().modified().unwrap();
        assert_eq!(
            m1.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            eight_days_ago_secs as u64
        );
        assert_eq!(
            m2.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            eight_days_ago_secs as u64
        );

        // db now-1h is fresh
        set_file_mtime(&sqlite, FileTime::from_unix_time(1_800_000_000 - 3600, 0)).unwrap();
        set_file_mtime(&wal, FileTime::from_unix_time(1_800_000_000 - 3600, 0)).unwrap();
        let eval_fresh = evaluate_search_freshness(root, &meta, None, SummaryFreshness::Fresh, now);
        assert_eq!(eval_fresh.state, "fresh");

        // db now-8d with WAL now-1h is fresh
        set_file_mtime(&sqlite, FileTime::from_unix_time(eight_days_ago_secs, 0)).unwrap();
        set_file_mtime(&wal, FileTime::from_unix_time(1_800_000_000 - 3600, 0)).unwrap();
        let eval_wal_fresh =
            evaluate_search_freshness(root, &meta, None, SummaryFreshness::Fresh, now);
        assert_eq!(eval_wal_fresh.state, "fresh");

        // db now+1d is unknown
        set_file_mtime(&sqlite, FileTime::from_unix_time(1_800_000_000 + 86400, 0)).unwrap();
        set_file_mtime(&wal, FileTime::from_unix_time(1_800_000_000 - 3600, 0)).unwrap();
        let eval_future =
            evaluate_search_freshness(root, &meta, None, SummaryFreshness::Fresh, now);
        assert_eq!(eval_future.state, "unknown");

        // summary 40h old with a failed attempt and mtime now-1h is unknown with last_attempt_failed == false
        set_file_mtime(&sqlite, FileTime::from_unix_time(1_800_000_000 - 3600, 0)).unwrap();
        set_file_mtime(&wal, FileTime::from_unix_time(1_800_000_000 - 3600, 0)).unwrap();
        let failed_indexer = solstone_core_system_health::IndexerPhase {
            success: false,
            run_started_at_ms: 100,
            reason_code: Some("failed".to_owned()),
        };
        let eval_summary_old = evaluate_search_freshness(
            root,
            &meta,
            Some(&failed_indexer),
            SummaryFreshness::Stale,
            now,
        );
        assert_eq!(eval_summary_old.state, "unknown");
        assert_eq!(eval_summary_old.last_attempt_failed, false);

        // summary missing, both files now-30d, is stale
        let thirty_days_ago = 1_800_000_000 - 30 * 86400;
        set_file_mtime(&sqlite, FileTime::from_unix_time(thirty_days_ago, 0)).unwrap();
        set_file_mtime(&wal, FileTime::from_unix_time(thirty_days_ago, 0)).unwrap();
        let eval_30d = evaluate_search_freshness(root, &meta, None, SummaryFreshness::Unknown, now);
        assert_eq!(eval_30d.state, "stale");
    }

    #[tokio::test]
    async fn test_synthesis_and_summary_route_degraded_backlog_search_freshness() {
        use filetime::{FileTime, set_file_mtime};

        let temp = TempDir::new().unwrap();
        let root = temp.path();

        let now = Utc::now();
        let fixed_now = chrono::DateTime::<chrono::FixedOffset>::from(now);

        fs::create_dir_all(root.join("indexer")).unwrap();
        let sqlite = root.join("indexer/journal.sqlite");
        let wal = root.join("indexer/journal.sqlite-wal");
        fs::write(&sqlite, b"sqlite").unwrap();
        fs::write(&wal, b"wal").unwrap();

        let one_hour_ago = now.timestamp() - 3600;
        set_file_mtime(&sqlite, FileTime::from_unix_time(one_hour_ago, 0)).unwrap();
        set_file_mtime(&wal, FileTime::from_unix_time(one_hour_ago, 0)).unwrap();

        let stats = json!({
            "generated_at": now.to_rfc3339(),
            "backlog": {
                "degraded": true,
                "indexer_phase": {
                    "success": false,
                    "reason_code": "lock_failed",
                    "run_started_at_ms": 1_000
                }
            }
        });
        fs::write(root.join("stats.json"), stats.to_string()).unwrap();

        let (synth, notes) = crate::journal_data::report::build_synthesis_health(
            root,
            &crate::journal_data::report::ScanAggregate::default(),
            fixed_now,
        )
        .unwrap();

        assert!(
            !notes
                .iter()
                .any(|n| n.message == SEARCH_NOTE_ATTEMPT_FAILED),
            "degraded backlog must not emit attempt failed note"
        );
        assert_eq!(synth.indexer_last_rebuild_at, Some(one_hour_ago * 1000));

        let (gen_at_opt, backlog_opt) = crate::backlog::load(root);
        let indexer_phase = backlog_opt
            .as_ref()
            .and_then(|b| b.get("indexer_phase"))
            .and_then(solstone_core_system_health::IndexerPhase::from_json_value);
        let summary_freshness = if backlog_opt
            .as_ref()
            .is_none_or(|b| b.get("degraded") == Some(&serde_json::Value::Bool(true)))
        {
            solstone_core_system_health::SummaryFreshness::Unknown
        } else {
            solstone_core_system_health::summary_freshness(gen_at_opt.as_deref(), now)
        };
        let eval = evaluate_search_freshness(
            root,
            &crate::search_freshness::FsIndexMetadata,
            indexer_phase.as_ref(),
            summary_freshness,
            now,
        );
        assert_eq!(eval.state, "unknown");
        assert_eq!(eval.last_attempt_failed, false);

        let router = crate::routes_with_clock(root.to_path_buf(), Clock::new(move || now));
        let resp = router
            .oneshot(
                axum::http::Request::get("/api/health/summary")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let router2 = crate::routes_with_clock(root.to_path_buf(), Clock::new(move || now));
        let resp2 = router2
            .oneshot(
                axum::http::Request::get("/app/health/api/state")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), 200);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp2.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["search_index"]["state"], "unknown");
        assert_eq!(body["search_index"]["last_attempt_failed"], false);
    }
}
