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

    use crate::search_index::{IndexMetadata, evaluate_search_index};
    use crate::{Clock, backlog, routes_with_clock};
    use solstone_core_system_health::INDEX_TEXT_OK;

    #[cfg(feature = "full-tests")]
    #[tokio::test]
    async fn media_backlog_route_preserves_owner_time_and_avoids_unrelated_reports() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        for name in ["config", "facets", "entities", "chronicle", "talents"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        fs::write(root.join("config/journal.json"), serde_json::to_vec(&json!({
            "identity":{"timezone":"America/Denver"},
            "providers":{"active":{"provider":"test"}},
            "processing":{"mode":"deferred","gate":{"time_window":{"enabled":true,"start":"02:00","end":"06:00"}}}
        })).unwrap()).unwrap();
        let old_segment = root.join("chronicle/20200101/120000_60");
        fs::create_dir_all(&old_segment).unwrap();
        fs::write(old_segment.join("screen.jsonl"), "{}\n").unwrap();
        fs::create_dir_all(root.join("chronicle/20200101/health")).unwrap();
        fs::write(
            root.join("chronicle/20200101/health/stream.updated"),
            "updated\n",
        )
        .unwrap();
        for (hour, expected) in [(9, "window_open"), (13, "waiting_for_window")] {
            let now = Utc.with_ymd_and_hms(2026, 10, 20, hour, 0, 0).unwrap();
            let router = routes_with_clock(root.to_path_buf(), Clock::fixed(now));
            let response = router
                .oneshot(
                    Request::get("/app/health/api/media-backlog")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(body["segment_backlog"]["drain_state"], expected);
            assert_eq!(body["segment_backlog"]["not_sensed"], 1);
            let local = now.with_timezone(&chrono_tz::America::Denver);
            let full = crate::journal_data::build_health_report(
                root,
                (local.date_naive(), local.date_naive()),
                local,
            )
            .unwrap();
            assert_eq!(
                body["segment_backlog"],
                serde_json::to_value(full).unwrap()["segment_backlog"]
            );
        }
        #[cfg(unix)]
        {
            // A media-only read has no dependency on the full report's facet inventory.
            fs::remove_dir(root.join("facets")).unwrap();
            std::os::unix::fs::symlink("facets", root.join("facets")).unwrap();
            let now = Utc.with_ymd_and_hms(2026, 10, 20, 9, 0, 0).unwrap();
            let router = routes_with_clock(root.to_path_buf(), Clock::fixed(now));
            let narrow = router
                .clone()
                .oneshot(
                    Request::get("/app/health/api/media-backlog")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(narrow.status(), 200);
            let full = router
                .oneshot(
                    Request::get("/api/health/summary")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(full.status(), 500);
        }
    }

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

        // 1. sqlite NotFound -> no activity time, observed_failure false, index_activity_at_ms null (ignore WAL)
        stub.insert(
            &sqlite,
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not found",
            )),
        );
        stub.insert(&wal, Ok(now_sys));
        let eval = evaluate_search_index(root, &stub, now);
        assert_eq!(eval.coverage, "complete");
        assert_eq!(eval.state, "ok");
        assert_eq!(eval.index_activity_at_ms, None);
        assert!(!eval.observed_failure);
        assert_eq!(eval.text, INDEX_TEXT_OK);

        // 2. Other metadata error on sqlite -> unknown, observed_failure false, index_activity_at_ms null
        stub.insert(
            &sqlite,
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
        );
        let eval = evaluate_search_index(root, &stub, now);
        assert_eq!(eval.state, "ok");
        assert_eq!(eval.index_activity_at_ms, None);
        assert!(!eval.observed_failure);
        assert_eq!(eval.text, INDEX_TEXT_OK);

        // 3. Other metadata error on existing WAL -> unknown, observed_failure false, index_activity_at_ms null
        stub.insert(&sqlite, Ok(now_sys));
        stub.insert(
            &wal,
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
        );
        let eval = evaluate_search_index(root, &stub, now);
        assert_eq!(eval.state, "ok");
        assert_eq!(eval.index_activity_at_ms, None);
        assert!(!eval.observed_failure);
        assert_eq!(eval.text, INDEX_TEXT_OK);

        // 4. Newest mtime more than 5 minutes ahead -> unknown
        stub.insert(&sqlite, Ok(now_sys + StdDuration::from_secs(301)));
        stub.insert(
            &wal,
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not found",
            )),
        );
        let eval = evaluate_search_index(root, &stub, now);
        assert_eq!(eval.state, "ok");
        assert_eq!(eval.index_activity_at_ms, None);
        assert!(!eval.observed_failure);
        assert_eq!(eval.text, INDEX_TEXT_OK);

        // 5. Valid mtime with no failure -> state unknown, index_activity_at_ms Some, rendered text
        let one_hour_ago = now_sys - StdDuration::from_secs(3600);
        stub.insert(&sqlite, Ok(one_hour_ago));
        stub.insert(
            &wal,
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not found",
            )),
        );
        let eval = evaluate_search_index(root, &stub, now);
        assert_eq!(eval.state, "ok");
        assert_eq!(
            eval.index_activity_at_ms,
            Some((1_800_000_000 - 3600) * 1000)
        );
        assert!(!eval.observed_failure);
        assert_eq!(eval.text, INDEX_TEXT_OK);

        // 6. A recorded failure for a file that is not on disk is observed, and
        // is history: nothing is behind, so the index is still ok.
        let health_dir = root.join("chronicle/20261005/health");
        fs::create_dir_all(&health_dir).unwrap();
        let record = json!({
            "ts": 1_800_000_000_000i64 - 1000,
            "event": "index.attempt",
            "path": "20261005/test.md",
            "outcome": "failed",
        });
        fs::write(health_dir.join("001.jsonl"), record.to_string() + "\n").unwrap();
        let eval = evaluate_search_index(root, &stub, now);
        assert_eq!(eval.state, "ok");
        assert!(eval.observed_failure);
        assert_eq!(eval.index.failed, 0);
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

        // 2. An index file that is not a database -> the failing line is the note
        fs::create_dir_all(root.join("indexer")).unwrap();
        fs::write(root.join("indexer/journal.sqlite"), b"sqlite").unwrap();
        let (synth, notes) = crate::journal_data::report::build_synthesis_health(
            root,
            &crate::journal_data::report::ScanAggregate::default(),
            fixed_now,
        )
        .unwrap();
        assert_eq!(synth.search_index.state, "failing");
        assert!(
            notes
                .iter()
                .any(|n| n.message == solstone_core_system_health::INDEX_TEXT_UNREADABLE)
        );

        // 3. A real index, and a recorded failed update for a file that is still
        // not in it -> the attempt note
        fs::remove_file(root.join("indexer/journal.sqlite")).unwrap();
        drop(solstone_core_indexer_store::db::open_index(root).expect("open index"));
        let day = now.format("%Y%m%d").to_string();
        fs::create_dir_all(root.join("chronicle").join(&day).join("talents")).unwrap();
        fs::write(
            root.join("chronicle").join(&day).join("talents/test.md"),
            "# Test",
        )
        .unwrap();
        let health_dir = root.join("chronicle").join(&day).join("health");
        fs::create_dir_all(&health_dir).unwrap();
        let record = json!({
            "ts": now.timestamp_millis(),
            "event": "index.attempt",
            "path": format!("{day}/talents/test.md"),
            "outcome": "failed",
        });
        fs::write(health_dir.join("001.jsonl"), record.to_string() + "\n").unwrap();

        let (synth, notes) = crate::journal_data::report::build_synthesis_health(
            root,
            &crate::journal_data::report::ScanAggregate::default(),
            fixed_now,
        )
        .unwrap();
        assert_eq!(synth.search_index.failed, 1);
        assert!(
            notes
                .iter()
                .any(|n| n.message == solstone_core_system_health::INDEX_TEXT_FAILED_FILES)
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
                ]
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

                // The fixture's index file is not a database.
                let search = &body["search_index"];
                assert_eq!(search["coverage"], "unknown");
                assert_eq!(search["state"], "failing");
                assert_eq!(search["observed_failure"], false);
                assert_eq!(
                    search["text"],
                    solstone_core_system_health::INDEX_TEXT_UNREADABLE
                );
            });
    }

    fn state_body(root: &Path, now: chrono::DateTime<Utc>) -> Value {
        let router = routes_with_clock(root.to_path_buf(), Clock::fixed(now));
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
                serde_json::from_slice(&bytes).unwrap()
            })
    }

    /// Day 0 on /app/health says what home says, from the same day-0 rule.
    #[test]
    fn a_summary_that_has_not_come_up_yet_is_calm_on_health() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let now = Utc::now();

        // No way to think chosen: the nightly run is off, and search makes no claim.
        let body = state_body(root, now);
        assert_eq!(body["backlog"]["not_yet"], "awaiting_engine");
        assert_eq!(
            body["backlog"]["verdict"],
            solstone_core_system_health::NOT_YET_ENGINE
        );
        assert_eq!(body["search_index"]["text"], "");

        // A way to think, and the journal began today: its first night is ahead.
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"local"}}}"#,
        )
        .unwrap();
        let today = now
            .with_timezone(&chrono::Local)
            .format("%Y%m%d")
            .to_string();
        fs::create_dir_all(root.join("chronicle").join(&today)).unwrap();
        let body = state_body(root, now);
        assert_eq!(body["backlog"]["not_yet"], "first_night");
        assert_eq!(
            body["backlog"]["verdict"],
            solstone_core_system_health::NOT_YET_FIRST_NIGHT
        );
        assert_eq!(
            body["search_index"]["text"],
            solstone_core_system_health::NOT_YET_SEARCH
        );

        // The injected clock decides: three days on, the first night has passed
        // with no summary, so it is unclear again.
        let later = state_body(root, now + Duration::days(3));
        assert!(later["backlog"]["not_yet"].is_null());
        assert_eq!(
            later["backlog"]["verdict"],
            solstone_core_system_health::VERDICT_UNCLEAR_NOW
        );

        // Any summary file at all keeps today's rules.
        fs::write(root.join("stats.json"), "not json").unwrap();
        let body = state_body(root, now);
        assert!(body["backlog"]["not_yet"].is_null());
        assert_eq!(
            body["backlog"]["verdict"],
            solstone_core_system_health::VERDICT_UNCLEAR_NOW
        );
    }

    #[test]
    fn saved_thinking_lane_is_reflected_on_health() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let now = Utc::now();
        fs::create_dir_all(root.join("config")).unwrap();

        for (choice, lane) in [
            ("byo", solstone_core_journal_config::SavedLane::Byo),
            ("spp", solstone_core_journal_config::SavedLane::Spp),
            ("local", solstone_core_journal_config::SavedLane::Local),
        ] {
            fs::write(
                root.join("config/journal.json"),
                format!(r#"{{"setup":{{"thinking_choice":"{choice}"}}}}"#),
            )
            .unwrap();
            let body = state_body(root, now);
            assert_eq!(body["backlog"]["not_yet"], "awaiting_engine");
            assert_eq!(
                body["backlog"]["verdict"],
                solstone_core_system_health::NotYet::AwaitingEngine(Some(lane)).text()
            );
            assert_eq!(body["search_index"]["text"], "");
        }
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

            // Delete stats.json -> unclear whether the journal is caught up,
            // once a way to think is chosen and the first night has passed
            // (before that it is not yet due: the day-0 rule).
            fs::remove_file(&stats_path).unwrap();
            fs::create_dir_all(root.join("config")).unwrap();
            fs::write(
                root.join("config/journal.json"),
                br#"{"providers":{"active":{"provider":"local"}}}"#,
            )
            .unwrap();
            fs::create_dir_all(root.join("chronicle/20200101")).unwrap();
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
                "it's unclear whether your journal is caught up right now."
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

        // Health log has index.attempt failure whose ts is T
        let fail_record = json!({
            "ts": t_secs * 1000,
            "event": "index.attempt",
            "path": format!("{day}/seg.md"),
            "outcome": "failed"
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

            // The failed path is not an indexable file, so nothing is behind.
            assert_eq!(body["search_index"]["state"], "ok");
            assert_eq!(body["search_index"]["observed_failure"], true);
        });

        // Append a later success/indexed attempt for the same path
        let succ_record = json!({
            "ts": (t_secs + 5) * 1000,
            "event": "index.attempt",
            "path": format!("{day}/seg.md"),
            "outcome": "indexed"
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

            assert_eq!(body["search_index"]["state"], "ok");
            assert_eq!(body["search_index"]["observed_failure"], false);
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

        let meta = crate::search_index::FsIndexMetadata;

        // Call twice; an index that has not changed in eight days is not stale
        // when nothing on disk changed either. Mtimes unchanged.
        let eval1 = evaluate_search_index(root, &meta, now);
        assert_eq!(eval1.state, "ok");
        assert_eq!(eval1.coverage, "complete");
        assert!(!eval1.observed_failure);
        assert_eq!(eval1.text, INDEX_TEXT_OK);
        assert_eq!(eval1.index_activity_at_ms, Some(eight_days_ago_secs * 1000));

        let eval2 = evaluate_search_index(root, &meta, now);
        assert_eq!(eval2.state, "ok");

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

        // db now-1h
        set_file_mtime(&sqlite, FileTime::from_unix_time(1_800_000_000 - 3600, 0)).unwrap();
        set_file_mtime(&wal, FileTime::from_unix_time(1_800_000_000 - 3600, 0)).unwrap();
        let eval_fresh = evaluate_search_index(root, &meta, now);
        assert_eq!(eval_fresh.state, "ok");
        assert_eq!(
            eval_fresh.index_activity_at_ms,
            Some((1_800_000_000 - 3600) * 1000)
        );

        // db now-8d with WAL now-1h
        set_file_mtime(&sqlite, FileTime::from_unix_time(eight_days_ago_secs, 0)).unwrap();
        set_file_mtime(&wal, FileTime::from_unix_time(1_800_000_000 - 3600, 0)).unwrap();
        let eval_wal_fresh = evaluate_search_index(root, &meta, now);
        assert_eq!(eval_wal_fresh.state, "ok");
        assert_eq!(
            eval_wal_fresh.index_activity_at_ms,
            Some((1_800_000_000 - 3600) * 1000)
        );

        // db now+1d: no activity time (future mtime suppressed)
        set_file_mtime(&sqlite, FileTime::from_unix_time(1_800_000_000 + 86400, 0)).unwrap();
        set_file_mtime(&wal, FileTime::from_unix_time(1_800_000_000 - 3600, 0)).unwrap();
        let eval_future = evaluate_search_index(root, &meta, now);
        assert_eq!(eval_future.state, "ok");
        assert_eq!(eval_future.index_activity_at_ms, None);
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
                .any(|n| n.message == solstone_core_system_health::INDEX_TEXT_FAILED_FILES),
            "no index attempt failure in chronicle logs"
        );
        assert_eq!(synth.index_activity_at, Some(one_hour_ago * 1000));

        // The fixture's index files are not a database.
        let eval = evaluate_search_index(root, &crate::search_index::FsIndexMetadata, now);
        assert_eq!(eval.state, "failing");
        assert!(!eval.observed_failure);

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
        assert_eq!(body["search_index"]["state"], "failing");
        assert_eq!(body["search_index"]["observed_failure"], false);
    }

    #[tokio::test]
    async fn test_speech_setup_endpoint_and_scenarios() {
        use solstone_core_journal_config::parakeet_coreml::{
            ParakeetCoremlSentinel, default_parakeet_coreml_cache_dir, parakeet_coreml_model_root,
            parakeet_coreml_sentinel_path,
        };
        use solstone_core_system::speech_setup::{
            SpeechSetupPresentation, SpeechSetupState, SpeechSetupTone, read_speech_setup,
        };

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();

        // 1. Oneshot equality test
        let app = crate::routes_with_clock(root.to_path_buf(), Clock::fixed(now));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/app/health/api/speech-setup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body_bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let from_endpoint: SpeechSetupPresentation = serde_json::from_slice(&body_bytes).unwrap();

        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute());
        let expected = read_speech_setup(
            root,
            std::env::consts::OS,
            std::env::consts::ARCH,
            home.as_deref(),
            now,
        );
        assert_eq!(from_endpoint, expected);

        // The first failed browser read still knows whether this host has setup to check.
        let workspace = app
            .oneshot(
                Request::builder()
                    .uri("/app/health/workspace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(workspace.status(), 200);
        let workspace_bytes = to_bytes(workspace.into_body(), 1024 * 1024).await.unwrap();
        let workspace_html = std::str::from_utf8(&workspace_bytes).unwrap();
        assert!(workspace_html.contains(&format!("data-applicable=\"{}\"", expected.applicable)));

        // 2. LinuxCpp scenarios (injected platform)
        let status_path = root.join("health/providers/parakeet.json");
        fs::create_dir_all(status_path.parent().unwrap()).unwrap();
        let downloading_record = json!({
            "schema_version": 1,
            "provider": "parakeet",
            "revision": 1,
            "install_state": "downloading",
            "attempt_id": "attempt-1",
            "target_fingerprint_json": "fp",
            "target_fingerprint_sha256": "sha",
            "started_at": null,
            "last_transition_at": null,
            "last_progress_at": null,
            "completed_at": null,
            "progress_bytes_received": 123456,
            "progress_bytes_total": 987654,
            "install_error": null,
            "error_code": null,
            "owner": null
        });
        fs::write(
            &status_path,
            serde_json::to_vec(&downloading_record).unwrap(),
        )
        .unwrap();

        let lease = solstone_core_local::install::lease::acquire(root, "parakeet").unwrap();
        assert!(lease.is_some());

        let pres_linux_held = read_speech_setup(root, "linux", "x86_64", None, now);
        assert!(pres_linux_held.applicable);
        assert!(pres_linux_held.visible);
        assert_eq!(pres_linux_held.state, Some(SpeechSetupState::Downloading));
        assert_eq!(pres_linux_held.tone, Some(SpeechSetupTone::Neutral));
        assert_eq!(pres_linux_held.bytes_received, Some(123456));
        assert_eq!(pres_linux_held.bytes_total, Some(987654));
        assert_eq!(pres_linux_held.command, None);
        assert!(pres_linux_held.recovery_representable);

        if solstone_core_system::stt_backend_choice::stt_host_policy(
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .runtime_kind
            == Some(solstone_core_system::stt_backend_choice::SttRuntimeKind::LinuxCpp)
        {
            let app2 = crate::routes_with_clock(root.to_path_buf(), Clock::fixed(now));
            let response = app2
                .oneshot(
                    Request::builder()
                        .uri("/app/health/api/speech-setup")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            let from_ep: SpeechSetupPresentation = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(from_ep.state, Some(SpeechSetupState::Downloading));
            assert_eq!(from_ep.bytes_received, Some(123456));
            assert_eq!(from_ep.bytes_total, Some(987654));
        }

        drop(lease);

        let pres_linux_dropped = read_speech_setup(root, "linux", "x86_64", None, now);
        assert!(pres_linux_dropped.applicable);
        assert!(pres_linux_dropped.visible);
        assert_eq!(pres_linux_dropped.state, Some(SpeechSetupState::Attention));
        assert_eq!(pres_linux_dropped.tone, Some(SpeechSetupTone::Attention));
        assert_eq!(
            pres_linux_dropped.cause,
            Some("install_interrupted".to_string())
        );
        assert_eq!(pres_linux_dropped.bytes_received, None);
        assert_eq!(pres_linux_dropped.bytes_total, None);
        assert_eq!(
            pres_linux_dropped.command,
            Some("solstone journal install-provider parakeet".to_string())
        );

        // 3. MacosCoreml scenario: sentinel accepted + parakeet.json failed -> visible: false
        let fake_home = temp.path().join("mac_home");
        let cache_dir = default_parakeet_coreml_cache_dir(&fake_home);
        let model_root = parakeet_coreml_model_root(&cache_dir);
        fs::create_dir_all(&model_root).unwrap();
        fs::write(model_root.join("model_file.bin"), b"test").unwrap();

        let sentinel =
            ParakeetCoremlSentinel::new(cache_dir, "darwin", "arm64", "0.14.0", "1.0", vec![]);
        let sentinel_path = parakeet_coreml_sentinel_path(&fake_home);
        fs::create_dir_all(sentinel_path.parent().unwrap()).unwrap();
        fs::write(sentinel_path, serde_json::to_vec(&sentinel).unwrap()).unwrap();

        let failed_record = json!({
            "schema_version": 1,
            "provider": "parakeet",
            "revision": 2,
            "install_state": "failed",
            "attempt_id": "attempt-2",
            "target_fingerprint_json": "fp",
            "target_fingerprint_sha256": "sha",
            "started_at": null,
            "last_transition_at": null,
            "last_progress_at": null,
            "completed_at": null,
            "progress_bytes_received": null,
            "progress_bytes_total": null,
            "install_error": "network error",
            "error_code": "download_failed",
            "owner": null
        });
        fs::write(&status_path, serde_json::to_vec(&failed_record).unwrap()).unwrap();

        let pres_macos = read_speech_setup(root, "darwin", "arm64", Some(&fake_home), now);
        assert!(pres_macos.applicable);
        assert!(!pres_macos.visible);
        assert_eq!(pres_macos.command, None);
    }
}
