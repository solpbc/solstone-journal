// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-derived dry-run renderer for `thinking.py:3713-4057`.
//!
//! This module deliberately plans without creating a run-log or contacting
//! cortex.  Its output is an operator-facing explanation, not a substitute for
//! the run-mode sidecars.

use std::fmt::Write;

use serde_json::Value;
use solstone_core_system_health::{
    FilesystemHealthLogSource, FilesystemSegmentSource, read_completed_since, scan_day,
};
use solstone_core_talent_config::{
    TalentConfig, TalentFilter, get_output_path, load_talent_configs_in,
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
        let mut out = format!(
            "{}\n\n",
            solstone_core_cli_copy::display_for(
                context.locale(),
                "journal.cli.think.plan.weekly",
                [("day", &(display_day(&context.day)))]
            )
        );
        if configs.is_empty() {
            out.push_str(&format!(
                "{}\n",
                solstone_core_cli_copy::render_for(
                    context.locale(),
                    "journal.cli.think.plan.empty.weekly",
                    &[]
                )
            ));
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
        let (_, _, segments) = scan_day(
            &source,
            &context.journal,
            &context.day,
            jiff::Timestamp::now(),
        )
        .map_err(|error| error.render(context.locale()))?;
        if segments.is_empty() {
            return Ok(format!(
                "{}\n",
                solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.segment.day.empty",
                    [("day", &(context.day))]
                )
            ));
        }
        let mut out = format!(
            "{}\n\n",
            if context.terminal_locale.is_none() {
                solstone_core_cli_copy::display_for(
                    solstone_core_cli_copy::ENGLISH,
                    "journal.cli.think.plan.reprocess.record",
                    [
                        ("day", &(display_day(&context.day))),
                        ("count", &(segments.len())),
                    ],
                )
            } else {
                solstone_core_cli_copy::render_for(
                    context.locale(),
                    "journal.cli.think.plan.reprocess",
                    &[
                        (
                            "count",
                            solstone_core_cli_copy::Value::Cardinal((segments.len()) as i64),
                        ),
                        (
                            "day",
                            solstone_core_cli_copy::Value::String(
                                &(display_day(&context.day)).to_string(),
                            ),
                        ),
                    ],
                )
            }
        );
        for (index, segment) in segments.iter().enumerate() {
            writeln!(
                out,
                "  {}",
                solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.plan.segment.row",
                    [
                        ("index", &(index + 1)),
                        ("total", &(segments.len())),
                        ("segment", &(segment.key)),
                        ("start", &(segment.start)),
                        ("end", &(segment.end)),
                        ("stream", &(segment.stream))
                    ]
                )
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
    let heading = match (args.segment.as_deref(), args.refresh) {
        (None, false) => solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.plan.day",
            [("day", &(display_day(&context.day)))],
        ),
        (None, true) => solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.plan.day.refresh",
            [("day", &(display_day(&context.day)))],
        ),
        (Some(segment), false) => solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.plan.day.segment",
            [
                ("day", &(display_day(&context.day))),
                ("segment", &(segment)),
            ],
        ),
        (Some(segment), true) => solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.plan.day.segment.refresh",
            [
                ("day", &(display_day(&context.day))),
                ("segment", &(segment)),
            ],
        ),
    };
    let mut out = format!("{heading}\n\n");
    if args.segment.is_none() {
        writeln!(
            out,
            "{}",
            solstone_core_cli_copy::display_for(
                context.locale(),
                "journal.cli.think.plan.pre",
                [("day", &(context.day)), ("jobs", &(default_describe_jobs))]
            )
        )
        .expect("write string");
    }
    if configs.is_empty() {
        writeln!(
            out,
            "{}",
            solstone_core_cli_copy::display_for(
                context.locale(),
                "journal.cli.think.plan.schedule.empty",
                [("schedule", &(target))]
            )
        )
        .expect("write string");
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
        out.push_str(&format!(
            "{}\n",
            solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.plan.post.indexer",
                &[]
            )
        ));
        out.push_str(&format!(
            "{}\n",
            solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.plan.post.stats",
                &[]
            )
        ));
    }
    Ok(out)
}

fn configs(context: &ThinkContext, schedule: &str) -> Result<Vec<TalentConfig>, String> {
    load_talent_configs_in(
        context.locale(),
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
    writeln!(
        out,
        "{}",
        solstone_core_cli_copy::render_for(
            context.locale(),
            "journal.cli.think.plan.orchestrator",
            &[]
        )
    )
    .expect("write string");
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
        (
            solstone_core_talent_runtime::possible_names::TALENT,
            solstone_core_talent_runtime::possible_names::TALENT,
        ),
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
        let type_label = if is_gen { "gen" } else { "cog" };
        let row = match name {
            "sense" => solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.plan.step.sense",
                &[
                    ("presence", solstone_core_cli_copy::Value::String(status)),
                    (
                        "step",
                        solstone_core_cli_copy::Value::String(&(step).to_string()),
                    ),
                    ("name", solstone_core_cli_copy::Value::String(name)),
                    ("kind", solstone_core_cli_copy::Value::String(type_label)),
                    ("format", solstone_core_cli_copy::Value::String(format)),
                ],
            ),
            "entities" => solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.plan.step.entities",
                &[
                    ("presence", solstone_core_cli_copy::Value::String(status)),
                    (
                        "step",
                        solstone_core_cli_copy::Value::String(&(step).to_string()),
                    ),
                    ("name", solstone_core_cli_copy::Value::String(name)),
                    ("kind", solstone_core_cli_copy::Value::String(type_label)),
                    ("format", solstone_core_cli_copy::Value::String(format)),
                ],
            ),
            "screen" => solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.plan.step.screen",
                &[
                    ("presence", solstone_core_cli_copy::Value::String(status)),
                    (
                        "step",
                        solstone_core_cli_copy::Value::String(&(step).to_string()),
                    ),
                    ("name", solstone_core_cli_copy::Value::String(name)),
                    ("kind", solstone_core_cli_copy::Value::String(type_label)),
                    ("format", solstone_core_cli_copy::Value::String(format)),
                ],
            ),
            "speaker_attribution" => solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.plan.step.speaker.attribution",
                &[
                    ("presence", solstone_core_cli_copy::Value::String(status)),
                    (
                        "step",
                        solstone_core_cli_copy::Value::String(&(step).to_string()),
                    ),
                    ("name", solstone_core_cli_copy::Value::String(name)),
                    ("kind", solstone_core_cli_copy::Value::String(type_label)),
                    ("format", solstone_core_cli_copy::Value::String(format)),
                ],
            ),
            _ => solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.plan.step.possible",
                &[
                    ("presence", solstone_core_cli_copy::Value::String(status)),
                    (
                        "step",
                        solstone_core_cli_copy::Value::String(&(step).to_string()),
                    ),
                    ("name", solstone_core_cli_copy::Value::String(name)),
                    ("kind", solstone_core_cli_copy::Value::String(type_label)),
                    ("format", solstone_core_cli_copy::Value::String(format)),
                ],
            ),
        };
        writeln!(out, "  {row}").expect("write string");
        step += 1;
    }
    out.push_str(&format!(
        "\n  {}\n",
        solstone_core_cli_copy::render_for(context.locale(), "journal.cli.think.plan.idle", &[])
    ));
    out.push_str(&format!(
        "  {}\n",
        solstone_core_cli_copy::render_for(context.locale(), "journal.cli.think.plan.state", &[])
    ));
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
    let enabled =
        solstone_core_facets::list_declared_facet_names(&context.journal).map_err(|error| {
            solstone_core_talent_cli::preview::facet_detail(context.locale(), &error)
        })?;
    let active =
        solstone_core_system::activity_state::active_facets(&context.journal, &context.day);
    let mut total = 0;
    for (priority, items) in grouped(configs) {
        writeln!(
            out,
            "{}",
            solstone_core_cli_copy::display_for(
                context.locale(),
                "journal.cli.think.plan.priority",
                [("priority", &(priority))]
            )
        )
        .expect("write string");
        for config in items {
            let is_gen = is_generate(&config);
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
                        "plain"
                    };
                    writeln!(
                        out,
                        "  {}",
                        solstone_core_cli_copy::render_for(
                            context.locale(),
                            "journal.cli.think.plan.table.facet",
                            &[
                                (
                                    "row",
                                    solstone_core_cli_copy::Value::String(
                                        &(format!(
                                            "{}_{}",
                                            if is_gen { "generator" } else { "agent" },
                                            status
                                        ))
                                        .to_string()
                                    )
                                ),
                                (
                                    "name",
                                    solstone_core_cli_copy::Value::String(
                                        &(config.key).to_string()
                                    )
                                ),
                                (
                                    "facet",
                                    solstone_core_cli_copy::Value::String(&(facet).to_string())
                                )
                            ]
                        )
                    )
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
                        "  {}",
                        solstone_core_cli_copy::display_for(
                            context.locale(),
                            "journal.cli.think.plan.skip",
                            [("name", &(config.key)), ("facets", &(skipped.join(", ")))]
                        )
                    )
                    .expect("write string");
                }
            } else {
                let status = if is_gen {
                    output_status(context, &config.key, segment, Some(format), None, stream)
                } else {
                    "plain"
                };
                writeln!(
                    out,
                    "  {}",
                    solstone_core_cli_copy::render_for(
                        context.locale(),
                        "journal.cli.think.plan.table.talent",
                        &[
                            (
                                "row",
                                solstone_core_cli_copy::Value::String(
                                    &(format!(
                                        "{}_{}",
                                        if is_gen { "generator" } else { "agent" },
                                        status
                                    ))
                                    .to_string()
                                )
                            ),
                            (
                                "name",
                                solstone_core_cli_copy::Value::String(&(config.key).to_string())
                            )
                        ]
                    )
                )
                .expect("write string");
                total += 1;
            }
        }
        out.push('\n');
    }
    writeln!(out, "{}", plan_total(context, total)).expect("write string");
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
) -> &'static str {
    if segment == Some("<each>") {
        return "plain";
    }
    let path = if name == solstone_core_talent_runtime::possible_names::TALENT {
        // Its output is the possible-names sidecar, not a talent output file.
        let Some(segment) = segment else {
            return "plain";
        };
        get_output_path(&context.day_dir, name, Some(segment), format, facet, stream)
            .with_file_name("possible_names.json")
    } else if name == "weekly_reflection" {
        let start = crate::dispatch::snap_weekly_reflection_start(context);
        context
            .journal
            .join("reflections")
            .join("weekly")
            .join(format!("{start}.json"))
    } else {
        get_output_path(&context.day_dir, name, segment, format, facet, stream)
    };
    if path.exists() { "existing" } else { "new" }
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
            .map_err(|error| crate::activity::record_detail(context.locale(), &error))?
    else {
        return Ok(format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                context.locale(),
                "journal.cli.think.plan.activity.missing",
                [
                    ("activity", &(activity)),
                    ("facet", &(facet)),
                    ("day", &(context.day))
                ]
            )
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
        "{}\n\n",
        solstone_core_cli_copy::render_for(
            context.locale(),
            "journal.cli.think.plan.activity.heading",
            &[
                (
                    "operation",
                    solstone_core_cli_copy::Value::String(if refresh {
                        "refresh"
                    } else {
                        "ordinary"
                    })
                ),
                (
                    "day",
                    solstone_core_cli_copy::Value::String(&(display_day(&context.day)).to_string())
                ),
                ("activity", solstone_core_cli_copy::Value::String(activity)),
                ("facet", solstone_core_cli_copy::Value::String(facet))
            ]
        )
    );
    writeln!(
        out,
        "  {}",
        solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.plan.activity.type",
            [("kind", &(kind))]
        )
    )
    .expect("write string");
    writeln!(
        out,
        "  {}",
        solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.plan.activity.count",
            [("segments", &(segments))]
        )
    )
    .expect("write string");
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
        writeln!(
            out,
            "\n  {}",
            solstone_core_cli_copy::display_for(
                context.locale(),
                "journal.cli.think.plan.activity.empty",
                [("kind", &(kind))]
            )
        )
        .expect("write string");
        return Ok(out);
    }
    out.push('\n');
    let mut total = 0;
    for (priority, values) in grouped(matching) {
        writeln!(
            out,
            "{}",
            solstone_core_cli_copy::display_for(
                context.locale(),
                "journal.cli.think.plan.priority",
                [("priority", &(priority))]
            )
        )
        .expect("write string");
        for config in values {
            let is_gen = is_generate(&config);
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
                if path.exists() { "existing" } else { "new" }
            } else {
                "plain"
            };
            writeln!(
                out,
                "  {}",
                solstone_core_cli_copy::render_for(
                    context.locale(),
                    "journal.cli.think.plan.table.talent",
                    &[
                        (
                            "row",
                            solstone_core_cli_copy::Value::String(
                                &(format!(
                                    "{}_{}",
                                    if is_gen { "generator" } else { "agent" },
                                    status
                                ))
                                .to_string()
                            )
                        ),
                        (
                            "name",
                            solstone_core_cli_copy::Value::String(&(config.key).to_string())
                        )
                    ]
                )
            )
            .expect("write string");
            total += 1;
        }
        out.push('\n');
    }
    writeln!(out, "{}", plan_total(context, total)).expect("write string");
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
        "{}\n\n",
        solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.plan.flush",
            [
                ("day", &(display_day(&context.day))),
                ("segment", &(segment))
            ]
        )
    );
    if configs.is_empty() {
        out.push_str(&format!(
            "  {}\n",
            solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.plan.flush.empty",
                &[]
            )
        ));
        return Ok(out);
    }
    // Source-derived, not measured: the oracle captures only the no-eligible
    // branch; these rows are derived from thinking.py:4035-4055.
    for config in &configs {
        writeln!(
            out,
            "  {}",
            solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.plan.flush.talent",
                &[
                    (
                        "kind",
                        solstone_core_cli_copy::Value::String(if is_generate(config) {
                            "generate"
                        } else {
                            "agent"
                        })
                    ),
                    (
                        "name",
                        solstone_core_cli_copy::Value::String(&(config.key).to_string())
                    )
                ]
            )
        )
        .expect("write string");
    }
    let total = configs.len();
    writeln!(out, "\n{}", plan_total(context, total)).expect("write string");
    Ok(out)
}

/// Inline cadence renderer from `dry_run` (`thinking.py:3787-3814`).  Unlike
/// the prompt table it has fixed labels, no priority headings, and no total.
fn dry_run_cadence(context: &ThinkContext) -> Result<String, String> {
    let configs = configs(context, "cadence")?;
    let mut out = format!(
        "{}\n\n",
        solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.plan.cadence",
            [("day", &(display_day(&context.day)))]
        )
    );
    if configs.is_empty() {
        out.push_str(&format!(
            "{}\n",
            solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.plan.empty.cadence",
                &[]
            )
        ));
        return Ok(out);
    }
    let state = CadenceState::load(context.locale(), &context.journal);
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
                "  {}",
                solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.plan.cadence.skip",
                    [
                        ("name", &(config.key)),
                        ("seconds", &((context.now_ms - last) / 1000)),
                        ("minutes", &(minutes))
                    ]
                )
            )
            .expect("write string");
            continue;
        }
        let completed = read_completed_since(&source, &context.day, last.unwrap_or(0))
            .map_err(|error| error.render(context.locale()))?
            .value;
        if completed.segments.is_empty() && completed.activities.is_empty() {
            writeln!(
                out,
                "  {}",
                solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.plan.cadence.idle",
                    [("name", &(config.key))]
                )
            )
            .expect("write string");
        } else {
            // Source-derived, not measured: fixture coverage boundary 236-257
            // does not capture this `fire` row, so it is derived from
            // thinking.py:3808-3811 rather than replayed evidence.
            writeln!(
                out,
                "  {}",
                if context.terminal_locale.is_none() {
                    solstone_core_cli_copy::display_for(
                        solstone_core_cli_copy::ENGLISH,
                        "journal.cli.think.plan.cadence.fire.record",
                        [
                            ("name", &(config.key)),
                            ("segments", &(completed.segments.len())),
                            ("activities", &(completed.activities.len())),
                        ],
                    )
                } else {
                    solstone_core_cli_copy::render_for(
                        context.locale(),
                        "journal.cli.think.plan.cadence.fire",
                        &[
                            (
                                "segments",
                                solstone_core_cli_copy::Value::Cardinal(
                                    (completed.segments.len()) as i64,
                                ),
                            ),
                            (
                                "name",
                                solstone_core_cli_copy::Value::String(&(config.key).to_string()),
                            ),
                            (
                                "activities",
                                solstone_core_cli_copy::Value::Cardinal(
                                    (completed.activities.len()) as i64,
                                ),
                            ),
                        ],
                    )
                }
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
    solstone_core_time::parse_day(day)
        .map(|value| value.strftime("%Y-%m-%d").to_string())
        .unwrap_or_else(|| day.to_owned())
}

fn plan_total(context: &ThinkContext, total: usize) -> String {
    if context.terminal_locale.is_none() {
        solstone_core_cli_copy::display_for(
            solstone_core_cli_copy::ENGLISH,
            "journal.cli.think.plan.total.record",
            [("total", &(total))],
        )
    } else {
        solstone_core_cli_copy::render_for(
            context.locale(),
            "journal.cli.think.plan.total",
            &[(
                "total",
                solstone_core_cli_copy::Value::Cardinal((total) as i64),
            )],
        )
    }
}
