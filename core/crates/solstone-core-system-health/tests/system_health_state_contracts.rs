// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "backlog_view.rs"]
mod backlog_view;
#[path = "catchup_state.rs"]
mod catchup_state;
#[path = "support/corpus.rs"]
mod corpus;
#[path = "day_segment_scan.rs"]
mod day_segment_scan;
#[path = "fold_error.rs"]
mod fold_error;
#[path = "fold_fixture.rs"]
mod fold_fixture;
#[path = "pipeline_health_empty_fixture.rs"]
mod pipeline_health_empty_fixture;
#[path = "pipeline_health_folds.rs"]
mod pipeline_health_folds;
#[path = "run_log_fixture.rs"]
mod run_log_fixture;

/// These raw-marker and segment fixtures explicitly select their daily workload.
fn configure_daily_work(journal: &std::path::Path, enabled: Option<&str>) {
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
                serde_json::json!({"disabled": enabled != Some(config.key.as_str())}),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();
    std::fs::create_dir_all(journal.join("config")).unwrap();
    std::fs::write(
        journal.join("config/journal.json"),
        serde_json::to_vec(
            &serde_json::json!({"identity":{"timezone":"UTC"},"talent_overrides":overrides}),
        )
        .unwrap(),
    )
    .unwrap();
}
