// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::SystemTime;

use serde_json::Value;

use crate::args::ListOptions;
use crate::emit;
use crate::last_run::{LastRunOutcome, format_last_run};
use solstone_core_talent_config::TalentConfig;
use solstone_core_talent_config::is_truthy;

pub(crate) fn render(
    configs: &[TalentConfig],
    options: &ListOptions,
    journal_root: &Path,
    now: SystemTime,
) -> String {
    let mut filtered = emit::filtered(configs, options);
    filtered.sort_by(|left, right| left.key.cmp(&right.key));
    if filtered.is_empty() {
        return format!(
            "{}\n",
            solstone_core_cli_copy::render_for(
                solstone_core_cli_copy::cli_locale(),
                "journal.cli.talent.list.empty",
                &[]
            )
        );
    }
    let name_width = filtered
        .iter()
        .map(|config| config.key.len())
        .max()
        .unwrap_or(20)
        .max(10);
    let mut output = format!(
        "  {:<name_width$}  {:<28}  {:<18}  {}\n\n",
        solstone_core_cli_copy::render_for(
            solstone_core_cli_copy::cli_locale(),
            "journal.cli.talent.list.header.name",
            &[]
        ),
        solstone_core_cli_copy::render_for(
            solstone_core_cli_copy::cli_locale(),
            "journal.cli.talent.list.header.title",
            &[]
        ),
        solstone_core_cli_copy::render_for(
            solstone_core_cli_copy::cli_locale(),
            "journal.cli.talent.list.header.last",
            &[]
        ),
        solstone_core_cli_copy::render_for(
            solstone_core_cli_copy::cli_locale(),
            "journal.cli.talent.list.header.tags",
            &[]
        )
    );
    for group in [
        "segment",
        "daily",
        "weekly",
        "activity",
        "cadence",
        "unscheduled",
    ] {
        let items = filtered
            .iter()
            .copied()
            .filter(|config| group_for(config) == group)
            .collect::<Vec<_>>();
        if items.is_empty() {
            continue;
        }
        if options.schedule.is_none() {
            let heading = match group {
                "segment" => solstone_core_cli_copy::render_for(
                    solstone_core_cli_copy::cli_locale(),
                    "journal.cli.talent.list.group.segment",
                    &[],
                ),
                "daily" => solstone_core_cli_copy::render_for(
                    solstone_core_cli_copy::cli_locale(),
                    "journal.cli.talent.list.group.daily",
                    &[],
                ),
                "weekly" => solstone_core_cli_copy::render_for(
                    solstone_core_cli_copy::cli_locale(),
                    "journal.cli.talent.list.group.weekly",
                    &[],
                ),
                "activity" => solstone_core_cli_copy::render_for(
                    solstone_core_cli_copy::cli_locale(),
                    "journal.cli.talent.list.group.activity",
                    &[],
                ),
                "cadence" => solstone_core_cli_copy::render_for(
                    solstone_core_cli_copy::cli_locale(),
                    "journal.cli.talent.list.group.cadence",
                    &[],
                ),
                "unscheduled" => solstone_core_cli_copy::render_for(
                    solstone_core_cli_copy::cli_locale(),
                    "journal.cli.talent.list.group.unscheduled",
                    &[],
                ),
                _ => unreachable!(),
            };
            output.push_str(&heading);
            output.push('\n');
        }
        for config in items {
            let title = truncate(
                config
                    .metadata
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                28,
            );
            let (last_run, failed) = match format_last_run(&config.key, journal_root, now) {
                LastRunOutcome::NoRuns => ("-".to_owned(), false),
                LastRunOutcome::Unavailable => (
                    solstone_core_window_catalog::text(
                        solstone_core_cli_copy::cli_locale(),
                        "journal.window.thinking.copy.unavailable",
                    ),
                    false,
                ),
                LastRunOutcome::Found { display, failed } => (display, failed),
            };
            let tags = tags(&config.metadata, failed);
            let source = if config.metadata.get("source").and_then(Value::as_str) == Some("app") {
                format!(
                    " [{}]",
                    config
                        .metadata
                        .get("app")
                        .and_then(Value::as_str)
                        .unwrap_or("app")
                )
            } else {
                String::new()
            };
            let tag_part = if tags.is_empty() {
                String::new()
            } else {
                format!("  {tags}")
            };
            let line = format!(
                "  {:<name_width$}  {:<28}  {:<18}{}{}",
                config.key,
                title,
                truncate(&last_run, 18),
                tag_part,
                source
            );
            output.push_str(line.trim_end());
            output.push('\n');
        }
        if options.schedule.is_none() {
            output.push('\n');
        }
    }
    if !options.disabled {
        let before_disabled = configs
            .iter()
            .filter(|config| matches_filters(config, options))
            .count();
        let disabled_count = before_disabled.saturating_sub(filtered.len());
        if disabled_count > 0 {
            let count = filtered.len();
            output.push_str(&solstone_core_cli_copy::render_for(
                solstone_core_cli_copy::cli_locale(),
                "journal.cli.talent.list.visible",
                &[
                    (
                        "count",
                        solstone_core_cli_copy::Value::Cardinal(count as i64),
                    ),
                    (
                        "hidden",
                        solstone_core_cli_copy::Value::String(&disabled_count.to_string()),
                    ),
                ],
            ));
            output.push('\n');
        }
    }
    output
}

fn group_for(config: &TalentConfig) -> &str {
    match config.metadata.get("schedule").and_then(Value::as_str) {
        Some("segment") => "segment",
        Some("daily") => "daily",
        Some("weekly") => "weekly",
        Some("activity") => "activity",
        Some("cadence") => "cadence",
        _ => "unscheduled",
    }
}

fn matches_filters(config: &TalentConfig, options: &ListOptions) -> bool {
    options.schedule.as_deref().is_none_or(|schedule| {
        config.metadata.get("schedule").and_then(Value::as_str) == Some(schedule)
    }) && options
        .source
        .as_deref()
        .is_none_or(|source| config.metadata.get("source").and_then(Value::as_str) == Some(source))
}

fn tags(info: &serde_json::Map<String, Value>, failed: bool) -> String {
    let mut tags: Vec<std::borrow::Cow<'_, str>> = Vec::new();
    match info.get("output").and_then(Value::as_str) {
        Some("json") => tags.push("json".into()),
        Some(_) => tags.push("md".into()),
        None => {}
    }
    if let Some(hook) = info.get("hook") {
        if let Some(hook) = hook.as_object() {
            if hook.get("pre").is_some_and(is_truthy) {
                tags.push("pre".into());
            }
            if hook.get("post").is_some_and(is_truthy) {
                tags.push("post".into());
            }
        } else {
            tags.push("hook".into());
        }
    }
    if info.get("disabled").is_some_and(is_truthy) {
        tags.push(std::borrow::Cow::Owned(solstone_core_cli_copy::render_for(
            solstone_core_cli_copy::cli_locale(),
            "journal.cli.talent.list.tag.disabled",
            &[],
        )));
    }
    if failed {
        tags.push(std::borrow::Cow::Owned(solstone_core_cli_copy::render_for(
            solstone_core_cli_copy::cli_locale(),
            "journal.cli.talent.list.tag.fail",
            &[],
        )));
    }
    tags.join(" ")
}

fn truncate(value: &str, width: usize) -> String {
    value.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_follow_reference_order() {
        let mut info = serde_json::Map::new();
        info.insert("output".to_owned(), Value::String("md".to_owned()));
        info.insert(
            "hook".to_owned(),
            serde_json::json!({"pre": "x", "post": "y"}),
        );
        info.insert("disabled".to_owned(), Value::Bool(true));
        assert_eq!(tags(&info, true), "md pre post disabled FAIL");
    }
}
