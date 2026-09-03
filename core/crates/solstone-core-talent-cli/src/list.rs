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
        return "No prompts found matching filters.\n".to_owned();
    }
    let name_width = filtered
        .iter()
        .map(|config| config.key.len())
        .max()
        .unwrap_or(20)
        .max(10);
    let mut output = format!(
        "  {:<name_width$}  {:<28}  {:<18}  TAGS\n\n",
        "NAME", "TITLE", "LAST RUN"
    );
    for group in ["segment", "daily", "weekly", "activity", "unscheduled"] {
        let items = filtered
            .iter()
            .copied()
            .filter(|config| group_for(config) == group)
            .collect::<Vec<_>>();
        if items.is_empty() {
            continue;
        }
        if options.schedule.is_none() {
            output.push_str(&format!("{group}:\n"));
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
                LastRunOutcome::Unavailable => ("unavailable".to_owned(), false),
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
            output.push_str(&format!(
                "{} prompts ({} disabled hidden, use --disabled)\n",
                filtered.len(),
                disabled_count
            ));
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
    let mut tags = Vec::new();
    match info.get("output").and_then(Value::as_str) {
        Some("json") => tags.push("json"),
        Some(_) => tags.push("md"),
        None => {}
    }
    if let Some(hook) = info.get("hook") {
        if let Some(hook) = hook.as_object() {
            if hook.get("pre").is_some_and(is_truthy) {
                tags.push("pre");
            }
            if hook.get("post").is_some_and(is_truthy) {
                tags.push("post");
            }
        } else {
            tags.push("hook");
        }
    }
    if info.get("disabled").is_some_and(is_truthy) {
        tags.push("disabled");
    }
    if failed {
        tags.push("FAIL");
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
