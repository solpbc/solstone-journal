// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use chrono::{DateTime, Duration, Local, TimeZone, Utc};
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
        Clock::new(move || utc.with_timezone(&Local).naive_local())
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
        let (talent_root, apps_root) = solstone_core_system::daily_coverage::package_roots().unwrap();

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
                .oneshot(Request::get("/app/stats/api/stats").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let ctrl_body: Value = serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
            let ctrl_status = &ctrl_body["stats"]["journal_status"];
            assert_eq!(ctrl_status["verdict"], "your journal's all caught up.");
            assert_eq!(ctrl_status["unfinished_activities"]["activities"], 0);

            // Fresh check with unfinished activities: your journal's caught up.
            let router = routes(root.to_path_buf(), make_clock(now_shortly));
            let resp = router
                .oneshot(Request::get("/app/stats/api/stats").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let body: Value = serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
            let status = &body["stats"]["journal_status"];
            assert_eq!(status["freshness"], "fresh");
            assert_eq!(status["verdict"], "your journal's caught up.");
            assert_eq!(status["unfinished_activities"]["activities"], 1);
            assert_eq!(status["unfinished_activities"]["day_count"], 1);
            assert_eq!(status["unfinished_activities"]["oldest_day"], "20990101");

            // +2h -> returns same verdict text
            let r2 = routes(root.to_path_buf(), make_clock(t + Duration::hours(2)));
            let resp = r2.oneshot(Request::get("/app/stats/api/stats").body(Body::empty()).unwrap()).await.unwrap();
            let b2: Value = serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
            assert_eq!(b2["stats"]["journal_status"]["freshness"], "fresh");
            assert_eq!(b2["stats"]["journal_status"]["verdict"], "your journal's caught up.");

            // +40h -> stale, unclear with 40 hours
            let r40 = routes(root.to_path_buf(), make_clock(t + Duration::hours(40)));
            let resp = r40.oneshot(Request::get("/app/stats/api/stats").body(Body::empty()).unwrap()).await.unwrap();
            let b40: Value = serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
            assert_eq!(b40["stats"]["journal_status"]["freshness"], "stale");
            assert_eq!(b40["stats"]["journal_status"]["pending_days"], 0);
            assert_eq!(b40["stats"]["journal_status"]["unfinished_activities"]["activities"], 0);
            let v40 = b40["stats"]["journal_status"]["verdict"].as_str().unwrap();
            assert!(v40.contains("40 hours"));
            assert!(!v40.contains("1 day is still catching up."));

            // No top-level generated_at -> unknown age sentence
            let stats_path = root.join("stats.json");
            let mut stats_val: Value = serde_json::from_str(&fs::read_to_string(&stats_path).unwrap()).unwrap();
            stats_val.as_object_mut().unwrap().remove("generated_at");
            fs::write(&stats_path, stats_val.to_string()).unwrap();

            let rnogen = routes(root.to_path_buf(), make_clock(now_shortly));
            let resp = rnogen.oneshot(Request::get("/app/stats/api/stats").body(Body::empty()).unwrap()).await.unwrap();
            let bnogen: Value = serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
            assert_eq!(bnogen["stats"]["journal_status"]["freshness"], "unknown");
            assert_eq!(bnogen["stats"]["journal_status"]["verdict"], "it's unclear whether your journal is caught up; the last update age is unknown.");

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
            let resp = r_pending_40h.oneshot(Request::get("/app/stats/api/stats").body(Body::empty()).unwrap()).await.unwrap();
            let b_pending_40h: Value = serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
            assert_eq!(b_pending_40h["stats"]["journal_status"]["freshness"], "stale");
            let v_pending_40h = b_pending_40h["stats"]["journal_status"]["verdict"].as_str().unwrap();
            assert!(v_pending_40h.contains("40 hours"));
            assert!(!v_pending_40h.contains("1 day is still catching up."));
        });
    }
}
