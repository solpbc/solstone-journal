// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use serde_json::{Value, json};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::{Clock, routes};
    use solstone_core_journal_stats_cli::{
        FilesystemBacklogViewReader, FilesystemDocumentWriter, run_cli,
    };

    fn make_clock(utc: DateTime<Utc>) -> Clock {
        Clock::new(move || utc)
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

    /// A stuck day's reason on the stats page comes from the same mapping as
    /// /app/health, so two days with different codes read differently here.
    #[test]
    fn stats_backlog_days_carry_the_health_reason() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        configure_daily_work(root, None);
        let days = json!([
            {"day":"20990101","state":"stuck","reason":"failing_step","reason_code":"provider_key_missing"},
            {"day":"20990102","state":"stuck","reason":"corrupt_raw"},
        ]);
        fs::write(
            root.join("stats.json"),
            json!({"generated_at": Utc::now().to_rfc3339(), "backlog": {"stuck_days": 2, "days": days}}).to_string(),
        )
        .unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let body: Value = rt.block_on(async {
            let resp = routes(root.to_path_buf(), make_clock(Utc::now()))
                .oneshot(
                    Request::get("/app/stats/api/stats")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap()
        });
        let served = body["stats"]["backlog"]["days"].as_array().unwrap();
        let copies = served
            .iter()
            .zip(days.as_array().unwrap())
            .map(|(served, source)| {
                assert_eq!(
                    served["reason_copy"],
                    json!(solstone_core_system_health::backlog_day_reason_copy(
                        source.as_object().unwrap()
                    ))
                );
                served["reason_copy"].as_str().unwrap().to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(copies.len(), 2);
        assert_ne!(copies[0], copies[1]);
    }

    #[test]
    fn test_stats_api_status_evaluation_with_injected_clock() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        configure_daily_work(root, None);

        let day = "20990101";
        fs::create_dir_all(root.join("chronicle").join(day)).unwrap();

        // Control journal without fail row
        let temp_control = TempDir::new().unwrap();
        let root_control = temp_control.path();
        configure_daily_work(root_control, None);
        fs::create_dir_all(root_control.join("chronicle").join(day)).unwrap();

        let t_secs = 1_800_000_000i64;
        let t = Utc.timestamp_opt(t_secs, 0).unwrap();
        let (talent_root, apps_root) =
            solstone_core_system::daily_coverage::package_roots().unwrap();

        // Control run_cli
        let ctrl_res = run_cli(
            &[],
            root_control,
            t,
            &talent_root,
            &apps_root,
            &FilesystemBacklogViewReader,
            &FilesystemDocumentWriter,
        );
        assert_eq!(ctrl_res.exit_code, 0);

        // Activity failure in test journal
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
            // Control check
            let ctrl_router = routes(root_control.to_path_buf(), make_clock(now_shortly));
            let resp = ctrl_router
                .oneshot(
                    Request::get("/app/stats/api/stats")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let ctrl_body: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            let ctrl_status = &ctrl_body["stats"]["journal_status"];
            assert_eq!(ctrl_status["verdict"], "your journal's all caught up.");
            assert_eq!(ctrl_status["unfinished_activities"]["activities"], 0);

            // Fresh check with unfinished activities: your journal's caught up.
            let router = routes(root.to_path_buf(), make_clock(now_shortly));
            let resp = router
                .oneshot(
                    Request::get("/app/stats/api/stats")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let body: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            let status = &body["stats"]["journal_status"];
            assert_eq!(status["freshness"], "fresh");
            assert_eq!(status["verdict"], "your journal's caught up.");
            assert_eq!(status["unfinished_activities"]["activities"], 1);
            assert_eq!(status["unfinished_activities"]["day_count"], 1);
            assert_eq!(status["unfinished_activities"]["oldest_day"], "20990101");

            // +2h -> returns same verdict text
            let r2 = routes(root.to_path_buf(), make_clock(t + Duration::hours(2)));
            let resp = r2
                .oneshot(
                    Request::get("/app/stats/api/stats")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let b2: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(b2["stats"]["journal_status"]["freshness"], "fresh");
            assert_eq!(
                b2["stats"]["journal_status"]["verdict"],
                "your journal's caught up."
            );

            // +40h -> stale, unclear with 40 hours
            let r40 = routes(root.to_path_buf(), make_clock(t + Duration::hours(40)));
            let resp = r40
                .oneshot(
                    Request::get("/app/stats/api/stats")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let b40: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(b40["stats"]["journal_status"]["freshness"], "stale");
            assert_eq!(b40["stats"]["journal_status"]["pending_days"], 0);
            assert_eq!(
                b40["stats"]["journal_status"]["unfinished_activities"]["activities"],
                0
            );
            let v40 = b40["stats"]["journal_status"]["verdict"].as_str().unwrap();
            assert!(v40.contains("40 hours"));
            assert!(!v40.contains("1 day is still catching up."));

            // No top-level generated_at -> unknown age sentence
            let stats_path = root.join("stats.json");
            let mut stats_val: Value =
                serde_json::from_str(&fs::read_to_string(&stats_path).unwrap()).unwrap();
            stats_val.as_object_mut().unwrap().remove("generated_at");
            fs::write(&stats_path, stats_val.to_string()).unwrap();

            let rnogen = routes(root.to_path_buf(), make_clock(now_shortly));
            let resp = rnogen
                .oneshot(
                    Request::get("/app/stats/api/stats")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let bnogen: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(bnogen["stats"]["journal_status"]["freshness"], "unknown");
            assert_eq!(
                bnogen["stats"]["journal_status"]["verdict"],
                "it's unclear whether your journal is caught up; the last update age is unknown."
            );

            // One pending day in a 40-hour-old summary uses the unclear sentence with 40 hours, not 1 day is still catching up.
            let stats_40h = json!({
                "generated_at": (t - Duration::hours(40)).to_rfc3339(),
                "backlog": {
                    "pending_days": 1,
                    "oldest_pending_day": "20990101",
                    "stuck_days": 0
                }
            });
            fs::write(&stats_path, stats_40h.to_string()).unwrap();

            let r_pending_40h = routes(root.to_path_buf(), make_clock(t));
            let resp = r_pending_40h
                .oneshot(
                    Request::get("/app/stats/api/stats")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let b_pending_40h: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(
                b_pending_40h["stats"]["journal_status"]["freshness"],
                "stale"
            );
            let v_pending_40h = b_pending_40h["stats"]["journal_status"]["verdict"]
                .as_str()
                .unwrap();
            assert!(v_pending_40h.contains("40 hours"));
            assert!(!v_pending_40h.contains("1 day is still catching up."));
        });
    }

    #[test]
    fn usage_api_defaults_to_owner_zone_today() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            json!({ "identity": { "timezone": "Asia/Tokyo" } }).to_string(),
        )
        .unwrap();

        // 20:00 UTC on 2026-09-30 is 2026-10-01 in Tokyo
        let t = chrono::DateTime::parse_from_rfc3339("2026-09-30T20:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let tokens_dir = root.join("tokens");
        fs::create_dir_all(&tokens_dir).unwrap();
        fs::write(
            tokens_dir.join("20261001.jsonl"),
            json!({
                "ts": 1_700_000_000,
                "type": "generate",
                "model": "model",
                "context": "context",
                "input_tokens": 100
            })
            .to_string()
                + "\n",
        )
        .unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let router = routes(root.to_path_buf(), make_clock(t));
            let resp = router
                .oneshot(
                    Request::get("/app/stats/api/usage")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let body: Value =
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(body["day"], "20261001");
            assert_eq!(body["total"]["requests"], 1);
        });
    }

    #[test]
    fn journal_status_matches_system_health() {
        let now = DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let host_tz = solstone_core_journal_config::host_zone();
        let host_date = now.with_timezone(&host_tz).date_naive();
        let kiritimati: chrono_tz::Tz = "Pacific/Kiritimati".parse().unwrap();
        let gmt_minus_12: chrono_tz::Tz = "Etc/GMT+12".parse().unwrap();
        let chosen_tz = if now.with_timezone(&kiritimati).date_naive() != host_date {
            kiritimati
        } else {
            gmt_minus_12
        };

        for tz_name in [chosen_tz.name(), host_tz.name()] {
            let temp = TempDir::new().unwrap();
            let root = temp.path();
            fs::create_dir_all(root.join("config")).unwrap();
            fs::write(
                root.join("config/journal.json"),
                json!({ "identity": { "timezone": tz_name } }).to_string(),
            )
            .unwrap();

            let stats_val = json!({
                "generated_at": (now - Duration::hours(2)).to_rfc3339(),
                "backlog": {
                    "pending_days": 0,
                    "stuck_days": 0,
                    "days": []
                }
            });
            fs::write(root.join("stats.json"), stats_val.to_string()).unwrap();

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let body: Value = rt.block_on(async {
                let resp = routes(root.to_path_buf(), make_clock(now))
                    .oneshot(
                        Request::get("/app/stats/api/stats")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(resp.status(), 200);
                serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap()
            });

            let zone = solstone_core_journal_config::owner_zone(root);
            let owner_local = now.with_timezone(&zone).naive_local();
            let not_yet = solstone_core_system_health::summary_not_yet(root, owner_local);
            let expected_eval = match not_yet {
                Some(not_yet) => solstone_core_system_health::not_yet_evaluation(not_yet),
                None => {
                    let gen_at = stats_val.get("generated_at").and_then(Value::as_str);
                    let bl_obj = stats_val.get("backlog").and_then(Value::as_object);
                    solstone_core_system_health::evaluate_backlog_status(bl_obj, gen_at, now)
                }
            };

            assert_eq!(
                body["stats"]["journal_status"]["verdict"],
                expected_eval.verdict
            );
            assert_eq!(
                body["stats"]["journal_status"]["freshness"],
                expected_eval.freshness.as_str()
            );
        }
    }
}
