// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-derived dry-run renderer for `thinking.py:3713-4057`.
//!
//! This module deliberately plans without creating a run-log or contacting
//! cortex.  Its output is an operator-facing explanation, not a substitute for
//! the run-mode sidecars.

use std::fmt::Write;

use chrono::NaiveDate;
use serde_json::Value;
use solstone_core_system_health::{
    FilesystemHealthLogSource, FilesystemSegmentSource, read_completed_since, scan_day,
};
use solstone_core_talent_config::{
    TalentConfig, TalentFilter, get_output_path, load_talent_configs,
};

use crate::args::ThinkArgs;
use crate::cadence_state::CadenceState;
use crate::context::ThinkContext;
use crate::dispatch::grouped;

/// Port of `thinking.py:3713-3872`.  Every row is derived from the same native
/// config and durable journal readers that its matching execution mode uses.
pub(crate) fn run(
    context: &ThinkContext,
    args: &ThinkArgs,
    default_describe_jobs: usize,
) -> Result<String, String> {
    if let Some(activity) = args.activity.as_deref() {
        return dry_run_activity(
            context,
            activity,
            args.facet.as_deref().unwrap_or_default(),
            args.refresh,
        );
    }
    if args.flush {
        return dry_run_flush(context, args.segment.as_deref().unwrap_or_default());
    }
    if args.weekly {
        let configs = configs(context, "weekly")?;
        let mut out = format!("Day {} — weekly agents\n\n", display_day(&context.day));
        if configs.is_empty() {
            out.push_str("No prompts for schedule: weekly\n");
        } else {
            print_prompt_table(
                &mut out,
                context,
                configs,
                None,
                args.refresh,
                args.stream.as_deref(),
            )?;
        }
        return Ok(out);
    }
    if args.cadence {
        return dry_run_cadence(context);
    }
    if args.segments {
        let source = FilesystemSegmentSource;
        let (_, _, segments) =
            scan_day(&source, &context.journal, &context.day, chrono::Utc::now())
                .map_err(|error| error.to_string())?;
        if segments.is_empty() {
            return Ok(format!("No segments found for {}\n", context.day));
        }
        let mut out = format!(
            "Day {} — re-process {} segments\n\n",
            display_day(&context.day),
            segments.len()
        );
        for (index, segment) in segments.iter().enumerate() {
            writeln!(
                out,
                "  [{}/{}] {} ({}-{}) stream={}",
                index + 1,
                segments.len(),
                segment.key,
                segment.start,
                segment.end,
                segment.stream,
            )
            .expect("write string");
        }
        out.push('\n');
        let configs = configs(context, "segment")?;
        if !configs.is_empty() {
            print_segment_orchestrator(
                &mut out,
                context,
                &configs,
                "<each>",
                args.stream.as_deref(),
            )?;
        }
        return Ok(out);
    }

    let target = if args.segment.is_some() {
        "segment"
    } else {
        "daily"
    };
    let configs = configs(context, target)?;
    let mut out = format!("Day {}", display_day(&context.day));
    if let Some(segment) = args.segment.as_deref() {
        write!(out, " segment {segment}").expect("write string");
    }
    if args.refresh {
        out.push_str(" (refresh)");
    }
    out.push_str("\n\n");
    if args.segment.is_none() {
        writeln!(
            out,
            "Pre-phase:  journal sense --day {} -j {default_describe_jobs}",
            context.day
        )
        .expect("write string");
    }
    if configs.is_empty() {
        writeln!(out, "No prompts for schedule: {target}").expect("write string");
    } else if let Some(segment) = args.segment.as_deref() {
        print_segment_orchestrator(&mut out, context, &configs, segment, args.stream.as_deref())?;
    } else {
        print_prompt_table(
            &mut out,
            context,
            configs,
            None,
            args.refresh,
            args.stream.as_deref(),
        )?;
    }
    if args.segment.is_none() {
        out.push_str("Post-phase: journal indexer --rescan\nPost-phase: journal journal-stats\n");
    }
    Ok(out)
}

fn configs(context: &ThinkContext, schedule: &str) -> Result<Vec<TalentConfig>, String> {
    load_talent_configs(
        &context.talent_root,
        &context.apps_root,
        None,
        TalentFilter {
            r#type: None,
            schedule: Some(schedule),
            include_disabled: false,
        },
    )
}

/// Nested `_print_segment_orchestrator` from `thinking.py:3729-3765`, lifted
/// only to keep this Rust module readable.
fn print_segment_orchestrator(
    out: &mut String,
    context: &ThinkContext,
    configs: &[TalentConfig],
    target_segment: &str,
    stream: Option<&str>,
) -> Result<(), String> {
    writeln!(out, "Sense orchestrator (linear):").expect("write string");
    let mut by_name = configs
        .iter()
        .map(|config| (config.key.as_str(), config))
        .collect::<std::collections::BTreeMap<_, _>>();
    // Unknown optional orchestrator steps simply do not render.
    let aliases = [
        ("sense", "sense"),
        ("entities", "entities"),
        ("screen", "screen"),
        ("speaker_attribution", "speaker_attribution"),
    ];
    let mut step = 1;
    for (name, _label) in aliases {
        let Some(config) = by_name.remove(name) else {
            continue;
        };
        let is_gen = is_generate(config);
        let format = config
            .metadata
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or(if is_gen { "md" } else { "" });
        let status = output_status(
            context,
            name,
            Some(target_segment),
            is_gen.then_some(format),
            None,
            stream,
        );
        let label = match name {
            "sense" => "mandatory",
            "entities" => "always for non-idle",
            "screen" => "if recommend.screen_record",
            _ => "if recommend.speaker_attribution + audio embeddings",
        };
        let type_label = if is_gen { "gen" } else { "cog" };
        writeln!(
            out,
            "  {step}. {name} ({type_label}/{format}){status} — {label}"
        )
        .expect("write string");
        step += 1;
    }
    out.push_str("\n  idle segments: write stubs only (unless --refresh)\n  activity state machine: updates per segment\n");
    Ok(())
}

/// Port of `_print_prompt_table` at `thinking.py:3875-3938`.
fn print_prompt_table(
    out: &mut String,
    context: &ThinkContext,
    configs: Vec<TalentConfig>,
    segment: Option<&str>,
    _refresh: bool,
    stream: Option<&str>,
) -> Result<(), String> {
    let enabled = solstone_core_facets::list_declared_facet_names(&context.journal)
        .map_err(|error| error.to_string())?;
    let active =
        solstone_core_system::activity_state::active_facets(&context.journal, &context.day);
    let mut total = 0;
    for (priority, items) in grouped(configs) {
        writeln!(out, "Priority {priority}:").expect("write string");
        for config in items {
            let is_gen = is_generate(&config);
            let kind = if is_gen { "gen" } else { "agent" };
            let format = config
                .metadata
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or("md");
            if config.metadata.get("multi_facet").and_then(Value::as_bool) == Some(true) {
                // Source-derived, not measured: fixture coverage boundary
                // 236-257 omits the active multi-facet branch; this is the
                // direct port of thinking.py:3901-3920.
                let always = config.metadata.get("always").and_then(Value::as_bool) == Some(true);
                let targets = enabled
                    .iter()
                    .filter(|facet| always || active.contains(*facet))
                    .collect::<Vec<_>>();
                for facet in &targets {
                    let status = if is_gen {
                        output_status(
                            context,
                            &config.key,
                            segment,
                            Some(format),
                            Some(facet),
                            stream,
                        )
                    } else {
                        String::new()
                    };
                    writeln!(out, "  {kind}  {}/{facet}{status}", config.key)
                        .expect("write string");
                    total += 1;
                }
                let skipped = enabled
                    .iter()
                    .filter(|facet| !targets.contains(facet))
                    .cloned()
                    .collect::<Vec<_>>();
                if !skipped.is_empty() {
                    writeln!(
                        out,
                        "  skip {} — no activity: {}",
                        config.key,
                        skipped.join(", ")
                    )
                    .expect("write string");
                }
            } else {
                let status = if is_gen {
                    output_status(context, &config.key, segment, Some(format), None, stream)
                } else {
                    String::new()
                };
                writeln!(out, "  {kind}  {}{status}", config.key).expect("write string");
                total += 1;
            }
        }
        out.push('\n');
    }
    writeln!(out, "Total: {total} agents").expect("write string");
    Ok(())
}

/// Port of `_output_status` at `thinking.py:3941-3963`.
fn output_status(
    context: &ThinkContext,
    name: &str,
    segment: Option<&str>,
    format: Option<&str>,
    facet: Option<&str>,
    stream: Option<&str>,
) -> String {
    if segment == Some("<each>") {
        return String::new();
    }
    let path = get_output_path(&context.day_dir, name, segment, format, facet, stream);
    if path.exists() {
        " (exists)".to_owned()
    } else {
        " (new)".to_owned()
    }
}

/// Port of `_dry_run_activity` at `thinking.py:3966-4022`.
fn dry_run_activity(
    context: &ThinkContext,
    activity: &str,
    facet: &str,
    refresh: bool,
) -> Result<String, String> {
    let Some(record) =
        solstone_core_facets::get_activity_record(&context.journal, facet, &context.day, activity)
            .map_err(|error| error.to_string())?
    else {
        return Ok(format!(
            "Activity not found: {activity} in facet '{facet}' on {}\n",
            context.day
        ));
    };
    let kind = record
        .get("activity")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let segments = record
        .get("segments")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let mut out = format!(
        "Day {} --activity {activity} --facet {facet}{}\n\n  type:     {kind}\n  segments: {segments}\n",
        display_day(&context.day),
        if refresh { " (refresh)" } else { "" }
    );
    let matching = configs(context, "activity")?
        .into_iter()
        .filter(|config| {
            config
                .metadata
                .get("activities")
                .and_then(Value::as_array)
                .is_some_and(|types| {
                    types
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|value| value == "*" || value == kind)
                })
        })
        .collect::<Vec<_>>();
    if matching.is_empty() {
        writeln!(out, "\n  No agents match activity type '{kind}'").expect("write string");
        return Ok(out);
    }
    out.push('\n');
    let mut total = 0;
    for (priority, values) in grouped(matching) {
        writeln!(out, "Priority {priority}:").expect("write string");
        for config in values {
            let is_gen = is_generate(&config);
            let kind_label = if is_gen { "gen" } else { "agent" };
            let status = if is_gen {
                let format = config
                    .metadata
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or("md");
                let extension = if format == "json" { "json" } else { "md" };
                let filename = format!(
                    "{}.{}",
                    solstone_core_talent_config::get_output_name(&config.key),
                    extension
                );
                let path = context
                    .journal
                    .join("facets")
                    .join(facet)
                    .join("activities")
                    .join(&context.day)
                    .join(activity)
                    .join(filename);
                if path.exists() { " (exists)" } else { " (new)" }
            } else {
                ""
            };
            writeln!(out, "  {kind_label}  {}{status}", config.key).expect("write string");
            total += 1;
        }
        out.push('\n');
    }
    writeln!(out, "Total: {total} agents").expect("write string");
    Ok(out)
}

/// Port of `_dry_run_flush` at `thinking.py:4025-4057`.
fn dry_run_flush(context: &ThinkContext, segment: &str) -> Result<String, String> {
    let configs = configs(context, "segment")?
        .into_iter()
        .filter(|config| {
            config
                .metadata
                .get("hook")
                .and_then(Value::as_object)
                .and_then(|hook| hook.get("flush"))
                .and_then(Value::as_bool)
                == Some(true)
        })
        .collect::<Vec<_>>();
    let mut out = format!(
        "Day {} --flush segment {segment}\n\n",
        display_day(&context.day)
    );
    if configs.is_empty() {
        out.push_str("  No flush-eligible agents\n");
        return Ok(out);
    }
    // Source-derived, not measured: the oracle captures only the no-eligible
    // branch; these rows are derived from thinking.py:4035-4055.
    for config in &configs {
        writeln!(
            out,
            "  {}  {}",
            if is_generate(config) { "gen" } else { "agent" },
            config.key
        )
        .expect("write string");
    }
    writeln!(out, "\nTotal: {} agents", configs.len()).expect("write string");
    Ok(out)
}

/// Inline cadence renderer from `dry_run` (`thinking.py:3787-3814`).  Unlike
/// the prompt table it has fixed labels, no priority headings, and no total.
fn dry_run_cadence(context: &ThinkContext) -> Result<String, String> {
    let configs = configs(context, "cadence")?;
    let mut out = format!("Day {} — cadence agents\n\n", display_day(&context.day));
    if configs.is_empty() {
        out.push_str("No prompts for schedule: cadence\n");
        return Ok(out);
    }
    let state = CadenceState::load(&context.journal);
    let source = FilesystemHealthLogSource::new(&context.journal);
    let mut configs = configs;
    configs.sort_by_key(|config| {
        (
            config
                .metadata
                .get("priority")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            config.key.clone(),
        )
    });
    for config in configs {
        let minutes = config
            .metadata
            .get("cadence_minutes")
            .and_then(Value::as_i64)
            .unwrap_or(5);
        let last = state.timestamp(&config.key);
        if let Some(last) = last.filter(|last| context.now_ms - *last < minutes * 60_000) {
            writeln!(
                out,
                "  skip  {} — interval not elapsed ({}s < {minutes}m)",
                config.key,
                (context.now_ms - last) / 1_000
            )
            .expect("write string");
            continue;
        }
        let completed = read_completed_since(&source, &context.day, last.unwrap_or(0))
            .map_err(|error| error.to_string())?
            .value;
        if completed.segments.is_empty() && completed.activities.is_empty() {
            writeln!(
                out,
                "  no-op {} — no new work since last cadence run",
                config.key
            )
            .expect("write string");
        } else {
            // Source-derived, not measured: fixture coverage boundary 236-257
            // does not capture this `fire` row, so it is derived from
            // thinking.py:3808-3811 rather than replayed evidence.
            writeln!(
                out,
                "  fire  {} — window: {} segment(s), {} activity(ies)",
                config.key,
                completed.segments.len(),
                completed.activities.len()
            )
            .expect("write string");
        }
    }
    Ok(out)
}

fn is_generate(config: &TalentConfig) -> bool {
    config.metadata.get("type").and_then(Value::as_str) == Some("generate")
}

fn display_day(day: &str) -> String {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| day.to_owned())
}
