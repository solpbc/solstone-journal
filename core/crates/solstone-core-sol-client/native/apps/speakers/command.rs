// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cmp::Reverse;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::ClientError;
use crate::json_format::{json_pretty_ascii, json_pretty_utf8};
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};
use solstone_core_cli_copy::{Value as CopyValue, display_for, render_for};

const IDENTIFY_FAILURE_CODES: &[&str] = &[
    "speaker_identify_recoverable",
    "speaker_identify_repair_required",
    "speaker_identify_conflict",
    "speaker_identify_operation_not_found",
];

#[must_use]
pub fn status(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(locale, ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        "/app/speakers/api/status",
        vec![],
        None,
    ) {
        Ok(body) => body,
        Err(error) => return speaker_error(locale, error),
    };
    let valid = [
        "embeddings",
        "owner",
        "speakers",
        "pool",
        "clusters",
        "imports",
        "attribution",
        "quality",
    ];
    let result = if let Some(section) = parsed.positionals.first() {
        if valid.contains(&section.as_str()) {
            body.get(section).cloned().unwrap_or(Value::Null)
        } else {
            json!({"error": format!("Unknown section '{section}'. Valid: {}", valid.join(", "))})
        }
    } else {
        body
    };
    crate::agent_output::literal_output(stdout_json(&result))
}

#[must_use]
pub fn bootstrap(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = parse_json_commit(locale, ctx.args);
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(
            &mut out,
            render_for(locale, "journal.cli.sol.speakers.report.only", &[]),
        );
    }
    if !json_output {
        emit(
            &mut out,
            render_for(
                locale,
                "journal.cli.sol.speakers.bootstrap.bootstrapping.voiceprints.from.single.speaker",
                &[],
            ),
        );
    }
    let stats = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/bootstrap",
        vec![],
        Some(json!({"commit": commit})),
    ) {
        Ok(stats) => stats,
        Err(error) if error.reason_code() == Some("speaker_owner_centroid_required") => {
            return CommandOutput {
                stdout: out,
                stderr: format!(
                    "{}\n",
                    display_for(
                        locale,
                        "journal.cli.client.error.detail",
                        [(
                            "detail",
                            &(&error
                                .detail_for(locale)
                                .unwrap_or_else(|| error.message_for(locale)))
                        )]
                    )
                ),
                exit: 1,
            };
        }
        Err(error) => return speaker_error_preserving_stdout(locale, out, error),
    };
    if json_output {
        return crate::agent_output::literal_output(stdout_json(&stats));
    }
    let stats = crate::agent_output::literal_body(stats);
    render_bootstrap_stats(locale, &mut out, &stats);
    CommandOutput::success(out)
}

#[must_use]
pub fn resolve_names(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(
        locale,
        ctx.args,
        &[],
        &[
            FlagSpec::true_flag("--commit"),
            FlagSpec::true_flag("--json"),
            FlagSpec::true_flag("--yes"),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    // Resolving name variants with --commit merges entities, and merges are permanent.
    if commit && !parsed.flag("--yes") {
        return stderr(render_for(
            locale,
            "!journal.cli.sol.speakers.resolve.names.merging.name.variants.can.t",
            &[],
        ));
    }
    let mut out = String::new();
    if !commit && !json_output {
        emit(
            &mut out,
            render_for(locale, "journal.cli.sol.speakers.report.only", &[]),
        );
    }
    if !json_output {
        emit(
            &mut out,
            render_for(
                locale,
                "journal.cli.sol.speakers.resolve.names.resolving.speaker.name.variants",
                &[],
            ),
        );
    }
    let stats = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/resolve-names",
        vec![],
        Some(json!({"commit": commit})),
    ) {
        Ok(stats) => stats,
        Err(error) => return speaker_error_preserving_stdout(locale, out, error),
    };
    if json_output {
        return crate::agent_output::literal_output(stdout_json(&stats));
    }
    let stats = crate::agent_output::literal_body(stats);
    render_resolve_names(locale, &mut out, &stats);
    CommandOutput::success(out)
}

#[must_use]
pub fn attribute_segment(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--stream-layout", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[
            FlagSpec::true_flag("--commit"),
            FlagSpec::paired("--save", "--no-save", true),
            FlagSpec::paired("--accumulate", "--no-accumulate", true),
            FlagSpec::true_flag("--json"),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("DAY"))],
        ));
    };
    let Some(stream) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("STREAM"))],
        ));
    };
    let Some(segment) = parsed.positionals.get(2) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SEGMENT"))],
        ));
    };
    let commit = parsed.flag("--commit");
    let save = parsed.bool_value("--save").unwrap_or(true);
    let accumulate = parsed.bool_value("--accumulate").unwrap_or(true);
    let stream_layout = match parse_stream_layout_option(locale, &parsed) {
        Ok(layout) => layout,
        Err(err) => return err,
    };
    let json_output = parsed.flag("--json");
    let wrap = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/attribute-segment",
        vec![],
        Some(json!({
            "day": day,
            "stream_layout": stream_layout,
            "stream": stream,
            "segment": segment,
            "commit": commit,
            "save": save,
            "accumulate": accumulate,
        })),
    ) {
        Ok(wrap) => wrap,
        Err(error) if error.reason_code() == Some("speaker_owner_centroid_required") => {
            let mut out = String::new();
            if !commit && !json_output {
                emit(
                    &mut out,
                    render_for(locale, "journal.cli.sol.speakers.report.only", &[]),
                );
            }
            return CommandOutput {
                stdout: out,
                stderr: format!(
                    "{}\n",
                    display_for(
                        locale,
                        "journal.cli.client.error.detail",
                        [(
                            "detail",
                            &(&error
                                .detail_for(locale)
                                .unwrap_or_else(|| error.message_for(locale)))
                        )]
                    )
                ),
                exit: 1,
            };
        }
        Err(error) => return speaker_error(locale, error),
    };
    let mut out = String::new();
    if !commit && !json_output {
        emit(
            &mut out,
            render_for(locale, "journal.cli.sol.speakers.report.only", &[]),
        );
    }
    let result = wrap.get("result").cloned().unwrap_or(Value::Null);
    if json_output {
        return crate::agent_output::literal_output(stdout_json(&result));
    }
    render_attribute_segment(locale, &mut out, &wrap, &result, commit, save, accumulate);
    CommandOutput::success(out)
}

#[must_use]
pub fn correct(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--stream-layout", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[FlagSpec::true_flag("--json")],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("DAY"))],
        ));
    };
    let Some(stream) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("STREAM"))],
        ));
    };
    let Some(segment) = parsed.positionals.get(2) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SEGMENT"))],
        ));
    };
    let Some(source) = parsed.positionals.get(3) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SOURCE"))],
        ));
    };
    let Some(sentence_id) = parsed
        .positionals
        .get(4)
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SENTENCE_ID"))],
        ));
    };
    let Some(new_speaker) = parsed.positionals.get(5) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("NEW_SPEAKER"))],
        ));
    };
    let stream_layout = match parse_stream_layout_option(locale, &parsed) {
        Ok(layout) => layout,
        Err(err) => return err,
    };
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/correct-attribution",
        vec![],
        Some(json!({
            "day": day,
            "stream_layout": stream_layout,
            "stream": stream,
            "segment_key": segment,
            "source": source,
            "sentence_id": sentence_id,
            "new_speaker": new_speaker,
        })),
    ) {
        Ok(result) => result,
        Err(error) => return speaker_error(locale, error),
    };
    if parsed.flag("--json") {
        return stdout_json(&result);
    }
    let mut out = String::new();
    if string_field(&result, "status").as_deref() == Some("already_correct") {
        emit(
            &mut out,
            display_for(
                locale,
                "journal.cli.sol.speakers.correct.already.correct",
                [
                    ("day", &(day)),
                    ("stream_layout", &(stream_layout)),
                    ("stream", &(stream)),
                    ("segment", &(segment)),
                    ("sentence_id", &(sentence_id)),
                ],
            ),
        );
        return CommandOutput::success(out);
    }
    let old = string_field(&result, "old_speaker").unwrap_or_else(|| {
        render_for(
            locale,
            "journal.cli.sol.speakers.correct.unassigned.unassigned",
            &[],
        )
    });
    emit(
        &mut out,
        display_for(
            locale,
            "journal.cli.sol.speakers.correct.corrected",
            [
                ("day", &(day)),
                ("stream_layout", &(stream_layout)),
                ("stream", &(stream)),
                ("segment", &(segment)),
                ("sentence_id", &(sentence_id)),
                ("old", &(old)),
                ("new_speaker", &(value_to_string(result.get("new_speaker")))),
            ],
        ),
    );
    let removal = result.get("voiceprint_removal").and_then(Value::as_object);
    emit(
        &mut out,
        display_for(
            locale,
            "journal.cli.sol.speakers.correct.voiceprint.removal",
            [(
                "outcome",
                &(value_to_string(removal.and_then(|item| item.get("outcome")))),
            )],
        ),
    );
    let offer = result.get("propagation_offer").and_then(Value::as_object);
    if offer
        .and_then(|item| item.get("available"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        emit(&mut out, {
            let display_statements =
                (value_or_zero(offer.and_then(|item| item.get("statement_count")))).to_string();
            let display_segments =
                (value_or_zero(offer.and_then(|item| item.get("segment_count")))).to_string();
            render_for(
                locale,
                "journal.cli.sol.speakers.correct.propagation.available.statements.in.segments",
                &[
                    ("statements", CopyValue::String(&display_statements)),
                    ("segments", CopyValue::String(&display_segments)),
                    (
                        "quantity",
                        crate::command::count_selector(display_statements.parse().unwrap_or(0)),
                    ),
                    (
                        "segments_quantity",
                        crate::command::count_selector(display_segments.parse().unwrap_or(0)),
                    ),
                ],
            )
        });
        emit(
            &mut out,
            display_for(
                locale,
                "journal.cli.sol.speakers.correct.preview.with.solstone.call.speakers",
                [
                    ("old_speaker", &(value_to_string(result.get("old_speaker")))),
                    ("new_speaker", &(value_to_string(result.get("new_speaker")))),
                ],
            ),
        );
    } else {
        emit(
            &mut out,
            render_for(
                locale,
                "journal.cli.sol.speakers.correct.propagation.nothing.else.would.change",
                &[],
            ),
        );
    }
    CommandOutput::success(out)
}

#[must_use]
pub fn propagate_correction(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_json_commit(locale, ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(old_speaker) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("OLD_SPEAKER"))],
        ));
    };
    let Some(new_speaker) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("NEW_SPEAKER"))],
        ));
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(
            &mut out,
            render_for(locale, "journal.cli.sol.speakers.report.only", &[]),
        );
    }
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/propagate-correction",
        vec![],
        Some(json!({"old_speaker": old_speaker, "new_speaker": new_speaker, "commit": commit})),
    ) {
        Ok(result) => result,
        Err(error) => return speaker_error_preserving_stdout(locale, out, error),
    };
    if json_output {
        return stdout_json(&result);
    }
    render_propagate(locale, &mut out, &result, commit);
    CommandOutput::success(out)
}

#[must_use]
pub fn backfill(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--operation-id", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[
            FlagSpec::true_flag("--commit"),
            FlagSpec::true_flag("--reattribute"),
            FlagSpec::true_flag("--accumulation"),
            FlagSpec::true_flag("--json"),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let reattribute = parsed.flag("--reattribute");
    let accumulation = parsed.flag("--accumulation");
    let operation_id = parsed.value("--operation-id").map(str::to_owned);
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(
            &mut out,
            render_for(locale, "journal.cli.sol.speakers.report.only", &[]),
        );
    }
    if !json_output {
        emit(
            &mut out,
            render_for(
                locale,
                "journal.cli.sol.speakers.backfill.starting.speaker.attribution.backfill",
                &[],
            ),
        );
    }
    let start = monotonic_seconds(ctx);
    let mut body = json!({
        "commit": commit,
        "reattribute": reattribute,
        "accumulation": accumulation,
    });
    if let Some(id) = &operation_id {
        body["operation_id"] = json!(id);
    }
    let stats = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/backfill",
        vec![],
        Some(body),
    ) {
        Ok(stats) => stats,
        Err(error) => {
            if let Some(id) = &operation_id {
                let err_msg = error.to_string();
                if err_msg.contains("flags mismatch") || err_msg.contains("conflict") {
                    let err_msg = error.message_for(locale);
                    let extra = display_for(
                        locale,
                        "journal.cli.sol.speakers.backfill.note.backfill.operation.has.immutable",
                        [("id", &(id))],
                    );
                    return CommandOutput {
                        stdout: out,
                        stderr: format!("{err_msg}\n{extra}"),
                        exit: 1,
                    };
                }
            }
            return speaker_error_preserving_stdout(locale, out, error);
        }
    };
    let op_id = string_field(&stats, "operation_id").unwrap_or_default();
    follow_backfill(ctx, &op_id, out, json_output, start)
}

#[must_use]
pub fn backfill_status(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(operation_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("OPERATION_ID"))],
        ));
    };
    let json_output = parsed.flag("--json");
    let stats = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        &format!("/app/speakers/api/backfill/operations/{operation_id}"),
        vec![],
        None,
    ) {
        Ok(stats) => stats,
        Err(error) => return speaker_error(locale, error),
    };
    if json_output {
        return stdout_json(&stats);
    }
    let mut out = String::new();
    render_backfill(locale, &mut out, &stats, 0.0);
    CommandOutput::success(out)
}

#[must_use]
pub fn backfill_resume(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(operation_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("OPERATION_ID"))],
        ));
    };
    let json_output = parsed.flag("--json");
    let out = String::new();
    let start = monotonic_seconds(ctx);
    let _ = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        &format!("/app/speakers/api/backfill/operations/{operation_id}/resume"),
        vec![],
        None,
    ) {
        Ok(stats) => stats,
        Err(error) => return speaker_error_preserving_stdout(locale, out, error),
    };
    follow_backfill(ctx, operation_id, out, json_output, start)
}

const BACKFILL_POLL_INTERVAL: Duration = Duration::from_secs(30);

fn sleep(ctx: CommandContext<'_>, duration: Duration) {
    if let Some(clock) = ctx.clock {
        clock.sleep(duration);
    } else {
        std::thread::sleep(duration);
    }
}

fn follow_backfill(
    ctx: CommandContext<'_>,
    operation_id: &str,
    mut out: String,
    json_output: bool,
    start: f64,
) -> CommandOutput {
    let locale = ctx.human_locale();
    loop {
        let stats = match request_json(
            locale,
            ctx,
            HttpMethod::Get,
            &format!("/app/speakers/api/backfill/operations/{operation_id}"),
            vec![],
            None,
        ) {
            Ok(stats) => stats,
            Err(error) => {
                if json_output {
                    return CommandOutput {
                        stdout: String::new(),
                        stderr: error.to_string(),
                        exit: 1,
                    };
                }
                return speaker_error_preserving_stdout(locale, out, error);
            }
        };

        let status_kind = string_field(&stats, "status").unwrap_or_default();
        let done =
            stats.get("done").and_then(Value::as_bool).unwrap_or(false) || status_kind == "done";
        let is_active = status_kind == "active_preparing" || status_kind == "active_running";

        if done {
            let elapsed = monotonic_seconds(ctx) - start;
            if json_output {
                return stdout_json(&stats);
            }
            render_backfill(locale, &mut out, &stats, elapsed);
            return CommandOutput::success(out);
        }

        if !is_active {
            let elapsed = monotonic_seconds(ctx) - start;
            let advice = display_for(
                locale,
                "journal.cli.sol.speakers.follow.backfill.backfill.not.completed.status.to",
                [
                    ("status_kind", &(status_kind)),
                    ("operation_id", &(operation_id)),
                ],
            );
            if json_output {
                return CommandOutput {
                    stdout: serde_json::to_string(&stats).unwrap_or_default(),
                    stderr: advice,
                    exit: 1,
                };
            }
            render_backfill(locale, &mut out, &stats, elapsed);
            emit(&mut out, advice);
            return CommandOutput {
                stdout: out,
                stderr: String::new(),
                exit: 1,
            };
        }

        if !json_output {
            let total = value_to_string(stats.get("total_count"));
            let completed = value_to_string(stats.get("completed_count"));
            let pending = value_to_string(stats.get("pending_count"));
            emit(
                &mut out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.follow.backfill.backfill.in.progress.status.completed",
                    [
                        ("status_kind", &(status_kind)),
                        ("completed", &(completed)),
                        ("total", &(total)),
                        ("pending", &(pending)),
                    ],
                ),
            );
        }

        sleep(ctx, BACKFILL_POLL_INTERVAL);
    }
}

#[must_use]
pub fn repair(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--operation-id", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[
            FlagSpec::true_flag("--commit"),
            FlagSpec::true_flag("--json"),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    // A committed repair is named here, so a request that times out can still say which
    // operation to follow.
    let operation_id = match parsed.value("--operation-id") {
        Some(id) => Some(id.to_owned()),
        None if commit => Some(mint_repair_operation_id(ctx)),
        None => None,
    };
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(
            &mut out,
            render_for(locale, "journal.cli.sol.speakers.report.only", &[]),
        );
    }
    if !json_output {
        emit(
            &mut out,
            render_for(
                locale,
                "journal.cli.sol.speakers.repair.starting.speaker.contamination.repair",
                &[],
            ),
        );
    }
    let mut body = json!({
        "commit": commit,
    });
    if let Some(id) = &operation_id {
        body["operation_id"] = json!(id);
    }
    let stats = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/repair",
        vec![],
        Some(body),
    ) {
        Ok(stats) => stats,
        Err(error) => {
            let timed_out = matches!(error, ClientError::Timeout { .. });
            let mut output = speaker_error_preserving_stdout(locale, out, error);
            if let Some(id) = operation_id.as_deref().filter(|_| commit && timed_out) {
                output
                    .stderr
                    .push_str(&repair_still_running_advice(locale, id));
            }
            return output;
        }
    };
    if commit {
        return follow_repair_start(locale, ctx, &stats, out, json_output);
    }
    let complete = stats
        .get("complete")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);

    if json_output {
        let json_res = stdout_json(&stats);
        if !complete {
            return CommandOutput::failure(json_res.stdout, 1);
        }
        return json_res;
    }
    render_repair_dry_run(locale, &mut out, &stats);
    if !complete {
        return CommandOutput::failure(out, 1);
    }
    CommandOutput::success(out)
}

#[must_use]
pub fn repair_status(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(operation_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("OPERATION_ID"))],
        ));
    };
    let json_output = parsed.flag("--json");
    let stats = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        &format!("/app/speakers/api/repair/operations/{operation_id}"),
        vec![],
        None,
    ) {
        Ok(stats) => stats,
        Err(error) => return speaker_error(locale, error),
    };
    if json_output {
        return stdout_json(&stats);
    }
    let mut out = String::new();
    render_repair_status(locale, &mut out, &stats);
    if matches!(
        string_field(&stats, "status").as_deref(),
        Some("failed" | "interrupted")
    ) {
        out.push_str(&repair_stopped_advice(locale, operation_id, &stats));
    }
    CommandOutput::success(out)
}

#[must_use]
pub fn repair_resume(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(operation_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("OPERATION_ID"))],
        ));
    };
    let json_output = parsed.flag("--json");
    let out = String::new();
    let stats = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        &format!("/app/speakers/api/repair/operations/{operation_id}/resume"),
        vec![],
        None,
    ) {
        Ok(stats) => stats,
        Err(error) => {
            let timed_out = matches!(error, ClientError::Timeout { .. });
            let mut output = speaker_error_preserving_stdout(locale, out, error);
            if timed_out {
                output
                    .stderr
                    .push_str(&repair_still_running_advice(locale, operation_id));
            }
            return output;
        }
    };
    follow_repair_start(locale, ctx, &stats, out, json_output)
}

const REPAIR_POLL_INTERVAL: Duration = Duration::from_secs(30);

fn mint_repair_operation_id(ctx: CommandContext<'_>) -> String {
    let now = ctx.clock.map_or_else(SystemTime::now, |clock| clock.now());
    let millis = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("repair_{millis}")
}

fn repair_check_status_advice(locale: &str, operation_id: &str) -> String {
    display_for(
        locale,
        "journal.cli.sol.speakers.repair.check.status.advice.to.check.status.solstone.call",
        [("operation_id", &(operation_id))],
    )
}

/// For a repair whose request timed out: it may have started, and may still be going.
fn repair_still_running_advice(locale: &str, operation_id: &str) -> String {
    display_for(
        locale,
        "journal.cli.sol.speakers.repair.in.progress.status.advice",
        [("operation_id", &(operation_id))],
    )
}

/// For a repair that has stopped: resuming is offered only when its last failure allows it.
fn repair_stopped_advice(locale: &str, operation_id: &str, status: &Value) -> String {
    let resumable = status
        .get("latest_failure")
        .and_then(|failure| failure.get("retryable"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if resumable {
        display_for(
            locale,
            "journal.cli.sol.speakers.repair.stopped.advice.to.resume.solstone.call.speakers",
            [("operation_id", &(operation_id))],
        )
    } else {
        String::new()
    }
}

/// A committed or resumed repair answers once its attempt starts; follow it to the end.
fn follow_repair_start(
    locale: &str,
    ctx: CommandContext<'_>,
    started: &Value,
    mut out: String,
    json_output: bool,
) -> CommandOutput {
    let operation_id = string_field(started, "operation_id").unwrap_or_default();
    if string_field(started, "status").as_deref() != Some("running") {
        if json_output {
            return stdout_json(started);
        }
        render_repair_status(locale, &mut out, started);
        return CommandOutput::success(out);
    }
    if !json_output {
        emit(
            &mut out,
            display_for(
                locale,
                "journal.cli.sol.speakers.follow.repair.start.repair.running",
                [("operation_id", &(operation_id))],
            ),
        );
    }
    loop {
        sleep(ctx, REPAIR_POLL_INTERVAL);
        let status = match request_json(
            locale,
            ctx,
            HttpMethod::Get,
            &format!("/app/speakers/api/repair/operations/{operation_id}"),
            vec![],
            None,
        ) {
            Ok(status) => status,
            Err(error) => {
                let advice = if matches!(error, ClientError::Timeout { .. }) {
                    repair_still_running_advice(locale, &operation_id)
                } else {
                    repair_check_status_advice(locale, &operation_id)
                };
                let stdout = if json_output { String::new() } else { out };
                let mut output = speaker_error_preserving_stdout(locale, stdout, error);
                output.stderr.push_str(&advice);
                return output;
            }
        };
        match string_field(&status, "status").as_deref() {
            Some("completed") => {
                if json_output {
                    return stdout_json(&status);
                }
                render_repair_status(locale, &mut out, &status);
                return CommandOutput::success(out);
            }
            Some("running") => {
                if !json_output {
                    emit(
                        &mut out,
                        display_for(
                            locale,
                            "journal.cli.sol.speakers.follow.repair.start.repair.in.progress.segments.repaired",
                            [(
                                "count",
                                &(value_to_string(status.get("checkpointed_count"))),
                            )],
                        ),
                    );
                }
            }
            _ => {
                let advice = repair_stopped_advice(locale, &operation_id, &status);
                if json_output {
                    return CommandOutput {
                        stdout: format!("{}\n", json_pretty_ascii(&status)),
                        stderr: advice,
                        exit: 1,
                    };
                }
                render_repair_status(locale, &mut out, &status);
                emit(&mut out, advice);
                return CommandOutput {
                    stdout: out,
                    stderr: String::new(),
                    exit: 1,
                };
            }
        }
    }
}

fn render_repair_status(locale: &str, out: &mut String, status: &Value) {
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.help.operation",
            [(
                "operation",
                &(string_field(status, "operation_id").unwrap_or_default()),
            )],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.repair.status.status",
            [(
                "status",
                &(string_field(status, "status").unwrap_or_default()),
            )],
        ),
    );
    if let Some(count) = status.get("checkpointed_count").filter(|v| !v.is_null()) {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.repair.status.segments.repaired",
                [("count", &(count))],
            ),
        );
    }
    if let Some(clean) = status
        .get("summary")
        .and_then(|summary| summary.get("clean"))
        .and_then(Value::as_bool)
    {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.repair.status.clean",
                [("clean", &(clean))],
            ),
        );
    }
    if let Some(failure) = status.get("latest_failure").filter(|v| !v.is_null()) {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.repair.status.last.failure",
                [
                    (
                        "stage",
                        &(string_field(failure, "stage").unwrap_or_default()),
                    ),
                    (
                        "code",
                        &(string_field(failure, "detail").unwrap_or_default()),
                    ),
                ],
            ),
        );
    }
}

/// Render the dry-run inventory; a committed repair is followed by operation.
fn render_repair_dry_run(locale: &str, out: &mut String, stats: &Value) {
    let complete = stats
        .get("complete")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let clean = stats.get("clean").and_then(Value::as_bool).unwrap_or(false);

    emit(
        out,
        render_for(
            locale,
            "journal.cli.sol.speakers.repair.dry.run.mode.dry.run",
            &[],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.repair.dry.run.complete",
            [("complete", &(complete))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.repair.status.clean",
            [("clean", &(clean))],
        ),
    );

    if let Some(removals) = stats.get("planned_removals").and_then(Value::as_array)
        && !removals.is_empty()
    {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.repair.dry.run.entities.with.voiceprints.to.remove",
                [("entities", &(removals.len()))],
            ),
        );
        for removal in removals {
            if let Some(id) = removal.get("entity_id").and_then(Value::as_str) {
                let rows = removal
                    .get("voiceprint_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                emit(out, {
                    let display_id = (id).to_string();
                    let display_rows = (rows).to_string();
                    render_for(
                        locale,
                        "journal.cli.sol.speakers.repair.dry.run.rows",
                        &[
                            ("id", CopyValue::String(&display_id)),
                            ("rows", CopyValue::String(&display_rows)),
                            (
                                "quantity",
                                crate::command::count_selector(display_rows.parse().unwrap_or(0)),
                            ),
                        ],
                    )
                });
            }
        }
    }
    if let Some(segments) = stats.get("planned_segments").and_then(Value::as_array)
        && !segments.is_empty()
    {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.repair.dry.run.segments.to.repair",
                [("segments", &(segments.len()))],
            ),
        );
    }
    if let Some(gaps) = stats.get("gaps").and_then(Value::as_array)
        && !gaps.is_empty()
    {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.repair.dry.run.gaps.detected",
                [("gaps", &(gaps.len()))],
            ),
        );
        for g in gaps {
            if let Some(reason) = g.get("reason").and_then(Value::as_str) {
                emit(out, format!("  - {reason}"));
            }
        }
    }
}

#[must_use]
pub fn backfill_last_seen(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_json_commit(locale, ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(
            &mut out,
            render_for(locale, "journal.cli.sol.speakers.report.only", &[]),
        );
    }
    let stats = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/backfill-last-seen",
        vec![],
        Some(json!({"commit": commit})),
    ) {
        Ok(stats) => stats,
        Err(error) => return speaker_error_preserving_stdout(locale, out, error),
    };
    if json_output {
        return stdout_json(&stats);
    }
    render_backfill_last_seen(locale, &mut out, &stats);
    CommandOutput::success(out)
}

#[must_use]
pub fn wipe(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(
        locale,
        ctx.args,
        &[],
        &[
            FlagSpec::true_flag("--commit"),
            FlagSpec::true_flag("--erase"),
            FlagSpec::true_flag("--from-backups"),
            FlagSpec::true_flag("--not-from-backups"),
            FlagSpec::true_flag("--json"),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let erase = parsed.flag("--erase");
    if erase && !commit {
        return stderr(render_for(
            locale,
            "journal.cli.sol.speakers.wipe.erase.needs.commit",
            &[],
        ));
    }
    let backups = if erase {
        match crate::command::backups_choice_for(
            locale,
            ctx,
            parsed.flag("--from-backups"),
            parsed.flag("--not-from-backups"),
        ) {
            Ok(choice) => choice,
            Err(error) => return stderr(error),
        }
    } else {
        crate::command::BackupsChoice::default()
    };
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(
            &mut out,
            render_for(locale, "journal.cli.sol.speakers.report.only", &[]),
        );
    }
    let report = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/wipe",
        vec![],
        Some(if erase {
            json!({"commit": commit, "erase": true, "backups": backups.chosen})
        } else {
            json!({"commit": commit, "erase": false})
        }),
    ) {
        Ok(report) => report,
        Err(error) => return speaker_error_preserving_stdout(locale, out, error),
    };
    if json_output {
        return stdout_json(&report);
    }
    let erase_day = crate::command::erase_day_of_for(locale, ctx, &report);
    render_wipe(locale, &mut out, &report, erase_day.as_deref());
    if report.get("erased").and_then(Value::as_bool) == Some(true)
        && let Some(next) = crate::command::backups_next_line_for(locale, backups)
    {
        emit(&mut out, next);
    }
    CommandOutput::success(out)
}

#[must_use]
pub fn discover(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let json_output = parsed.flag("--json");
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/discovery/scan",
        vec![],
        None,
    ) {
        Ok(result) => result,
        Err(error) => return discovery_error(locale, error, json_output),
    };
    if json_output {
        return crate::agent_output::literal_output(stdout_json(&result));
    }
    let result = crate::agent_output::literal_body(result);
    let clusters = array_field(&result, "clusters");
    let mut out = String::new();
    if clusters.is_empty() {
        emit(
            &mut out,
            render_for(
                locale,
                "journal.cli.sol.speakers.discover.no.recurring.unknown.speakers.found",
                &[],
            ),
        );
    } else {
        emit(&mut out, {
            let display_clusters = (clusters.len()).to_string();
            render_for(
                locale,
                "journal.cli.sol.speakers.discover.found.unknown.speaker.cluster.s",
                &[
                    ("clusters", CopyValue::String(&display_clusters)),
                    (
                        "quantity",
                        crate::command::count_selector(display_clusters.parse().unwrap_or(0)),
                    ),
                ],
            )
        });
        for cluster in clusters {
            emit(&mut out, {
                let display_cluster = (value_to_string(cluster.get("cluster_id"))).to_string();
                let display_samples = (value_to_string(cluster.get("size"))).to_string();
                let display_segments = (value_to_string(cluster.get("segment_count"))).to_string();
                render_for(
                    locale,
                    "journal.cli.sol.speakers.discover.cluster.samples.across.segments",
                    &[
                        ("cluster", CopyValue::String(&display_cluster)),
                        ("samples", CopyValue::String(&display_samples)),
                        ("segments", CopyValue::String(&display_segments)),
                        (
                            "quantity",
                            crate::command::count_selector(display_samples.parse().unwrap_or(0)),
                        ),
                        (
                            "segments_quantity",
                            crate::command::count_selector(display_segments.parse().unwrap_or(0)),
                        ),
                    ],
                )
            });
            for sample in array_field(cluster, "samples") {
                let text = string_field(sample, "text").unwrap_or_default();
                let preview = text.chars().take(60).collect::<String>();
                emit(
                    &mut out,
                    display_for(
                        locale,
                        "journal.cli.sol.speakers.discover.sid",
                        [
                            ("day", &(value_to_string(sample.get("day")))),
                            ("stream", &(value_to_string(sample.get("stream")))),
                            ("segment", &(value_to_string(sample.get("segment_key")))),
                            ("sentence_id", &(value_to_string(sample.get("sentence_id")))),
                            ("text", &(preview)),
                        ],
                    ),
                );
            }
            emit(&mut out, "");
        }
    }
    render_discovery_warnings(locale, &mut out, &result);
    CommandOutput::success(out)
}

#[must_use]
pub fn presence(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(cluster_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("CLUSTER_ID"))],
        ));
    };
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        &format!("/app/speakers/api/discovery/cluster/{cluster_id}/presence"),
        vec![],
        None,
    ) {
        Ok(result) => result,
        Err(error) if error.reason_code() == Some("speaker_review_unavailable") => {
            return CommandOutput::failure(
                display_for(
                    locale,
                    "journal.cli.sol.speakers.presence.cluster.was.not.found.run",
                    [("cluster_id", &(cluster_id))],
                ),
                1,
            );
        }
        Err(error) => return speaker_error(locale, error),
    };
    if parsed.flag("--json") {
        return crate::agent_output::literal_output(stdout_json(&result));
    }
    let result = crate::agent_output::literal_body(result);
    let mut out = String::new();
    render_presence(locale, &mut out, &result);
    CommandOutput::success(out)
}

#[must_use]
pub fn identify(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(
        locale,
        ctx.args,
        &[
            ("--entity-id", None),
            ("--entity-type", None),
            ("--request-id", None),
            ("--reviewed-near-match-entity-id", None),
        ],
        &[
            FlagSpec::true_flag("--create"),
            FlagSpec::true_flag("--resolve-only"),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(cluster_id) = parsed
        .positionals
        .first()
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("CLUSTER_ID"))],
        ));
    };
    let name = parsed.positionals.get(1).cloned();
    let entity_id = parsed.value("--entity-id");
    if name.is_none() && entity_id.is_none() {
        return CommandOutput::failure(
            render_for(
                locale,
                "journal.cli.sol.speakers.identify.name.required.usage",
                &[],
            ),
            2,
        );
    }
    let request_id = parsed.value("--request-id").map(str::to_string);
    let reviewed_ids = parsed.values("--reviewed-near-match-entity-id");
    let reviewed_ids_json = if reviewed_ids.is_empty() {
        Value::Null
    } else {
        Value::Array(reviewed_ids.into_iter().map(Value::String).collect())
    };
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/discovery/identify-cli",
        vec![],
        Some(json!({
            "cluster_id": cluster_id,
            "name": name,
            "entity_id": entity_id,
            "create_new": parsed.flag("--create"),
            "entity_type": parsed.value("--entity-type").unwrap_or("Person"),
            "resolve_only": parsed.flag("--resolve-only"),
            "request_id": request_id,
            "reviewed_near_match_entity_ids": reviewed_ids_json,
        })),
    ) {
        Ok(result) => result,
        Err(error) if is_identify_failure(&error) => {
            return identify_error(locale, error, request_id.as_deref());
        }
        Err(error) if error.reason_code() == Some("speaker_command_failed") => {
            return detail_error(locale, error);
        }
        Err(error) => return speaker_error(locale, error),
    };
    crate::agent_output::literal_output(stdout_json(&result))
}

#[must_use]
pub fn identify_undo(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(locale, ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(operation_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("OPERATION_ID"))],
        ));
    };
    match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/discovery/identify/undo",
        vec![],
        Some(json!({"operation_id": operation_id})),
    ) {
        Ok(result) => stdout_json(&result),
        Err(error) if is_identify_failure(&error) => identify_error(locale, error, None),
        Err(error) if error.reason_code() == Some("speaker_command_failed") => {
            detail_error(locale, error)
        }
        Err(error) => speaker_error(locale, error),
    }
}

#[must_use]
pub fn identify_operations(ctx: CommandContext<'_>) -> CommandOutput {
    request_pretty_get(ctx, "/app/speakers/api/discovery/identify/operations")
}

#[must_use]
pub fn identify_operation(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(locale, ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(operation_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("OPERATION_ID"))],
        ));
    };
    match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        &format!("/app/speakers/api/discovery/identify/operations/{operation_id}"),
        vec![],
        None,
    ) {
        Ok(result) => crate::agent_output::literal_output(stdout_json(&result)),
        Err(error) if is_identify_failure(&error) => identify_error(locale, error, None),
        Err(error) => speaker_error(locale, error),
    }
}

#[must_use]
pub fn dismiss_cluster(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(locale, ctx.args, &[("--disposition", None)], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(cluster_id) = parsed
        .positionals
        .first()
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("CLUSTER_ID"))],
        ));
    };
    let Some(disposition) = parsed.value("--disposition") else {
        return stderr(display_for(
            locale,
            "journal.cli.client.option.required",
            [("option", &("--disposition"))],
        ));
    };
    match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/discovery/dismiss",
        vec![],
        Some(json!({"cluster_id": cluster_id, "disposition": disposition})),
    ) {
        Ok(result) => stdout_json(&result),
        Err(error) if error.reason_code() == Some("speaker_command_failed") => {
            detail_error(locale, error)
        }
        Err(error) => speaker_error(locale, error),
    }
}

#[must_use]
pub fn dismissals(ctx: CommandContext<'_>) -> CommandOutput {
    request_pretty_get(ctx, "/app/speakers/api/discovery/dismissals")
}

#[must_use]
pub fn keep_separate_list(ctx: CommandContext<'_>) -> CommandOutput {
    request_pretty_get(ctx, "/app/speakers/api/name-variants/keep-separate")
}

#[must_use]
pub fn merge_names(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(locale, ctx.args, &[], &[FlagSpec::true_flag("--yes")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(alias) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ALIAS"))],
        ));
    };
    let Some(canonical) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("CANONICAL"))],
        ));
    };
    match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/merge-names",
        vec![],
        Some(json!({"alias": alias, "canonical": canonical, "commit": parsed.flag("--yes")})),
    ) {
        Ok(result) => crate::agent_output::literal_output(stdout_json(&result)),
        Err(error) if error.reason_code() == Some("speaker_command_failed") => {
            detail_error(locale, error)
        }
        Err(error) => speaker_error(locale, error),
    }
}

#[must_use]
pub fn link_import(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(locale, ctx.args, &[("--entity-id", None)], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("NAME"))],
        ));
    };
    let Some(entity_id) = parsed.value("--entity-id") else {
        return stderr(display_for(
            locale,
            "journal.cli.client.option.required",
            [("option", &("--entity-id"))],
        ));
    };
    match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/link-import",
        vec![],
        Some(json!({"name": name, "entity_id": entity_id})),
    ) {
        Ok(result) => stdout_json(&result),
        Err(error) if error.reason_code() == Some("speaker_command_failed") => {
            detail_error(locale, error)
        }
        Err(error) => speaker_error(locale, error),
    }
}

#[must_use]
pub fn seed_from_imports(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_json_commit(locale, ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(
            &mut out,
            render_for(locale, "journal.cli.sol.speakers.report.only", &[]),
        );
    }
    if !json_output {
        emit(
            &mut out,
            render_for(
                locale,
                "journal.cli.sol.speakers.seed.from.imports.seeding.voiceprints.from.import.segments",
                &[],
            ),
        );
    }
    let stats = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/seed-from-imports",
        vec![],
        Some(json!({"commit": commit})),
    ) {
        Ok(stats) => stats,
        Err(error) if error.reason_code() == Some("speaker_owner_centroid_required") => {
            return CommandOutput {
                stdout: out,
                stderr: format!(
                    "{}\n",
                    display_for(
                        locale,
                        "journal.cli.client.error.detail",
                        [(
                            "detail",
                            &(&error
                                .detail_for(locale)
                                .unwrap_or_else(|| error.message_for(locale)))
                        )]
                    )
                ),
                exit: 1,
            };
        }
        Err(error) => return speaker_error_preserving_stdout(locale, out, error),
    };
    if json_output {
        return crate::agent_output::literal_output(stdout_json(&stats));
    }
    let stats = crate::agent_output::literal_body(stats);
    render_seed_from_imports(locale, &mut out, &stats);
    CommandOutput::success(out)
}

#[must_use]
pub fn suggest(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(suggest_output(ctx))
}

fn suggest_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--limit", Some("-n"))];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[FlagSpec::true_flag("--json")],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let limit = parsed.value("--limit").unwrap_or("5");
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        "/app/speakers/api/suggest",
        vec![QueryParam::single("limit", limit)],
        None,
    ) {
        Ok(body) => body,
        Err(error) => return speaker_error(locale, error),
    };
    if parsed.flag("--json") {
        return stdout_json(&body);
    }
    stdout_line(string_field(&body, "markdown").unwrap_or_default())
}

#[must_use]
pub fn detect(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(locale, ctx.args, &[], &[FlagSpec::true_flag("--force")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    post_json(
        ctx,
        "/app/speakers/api/owner/detect",
        json!({"force": parsed.flag("--force")}),
    )
}

#[must_use]
pub fn build_from_tags(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/owner/build-from-tags",
        vec![],
        None,
    ) {
        Ok(result) => result,
        Err(error) => return speaker_error(locale, error),
    };
    if parsed.flag("--json") {
        return stdout_json(&result);
    }
    let mut out = String::new();
    render_build_from_tags(locale, &mut out, &result);
    CommandOutput::success(out)
}

#[must_use]
pub fn rebuild_owner(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(
        locale,
        ctx.args,
        &[],
        &[
            FlagSpec::true_flag("--override"),
            FlagSpec::true_flag("--json"),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/owner/rebuild",
        vec![],
        Some(json!({"override": parsed.flag("--override")})),
    ) {
        Ok(result) => result,
        Err(error) => return speaker_error(locale, error),
    };
    if parsed.flag("--json") {
        return stdout_json(&result);
    }
    let mut out = String::new();
    render_rebuild_owner(locale, &mut out, &result);
    CommandOutput::success(out)
}

#[must_use]
pub fn tag_owner(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--stream-layout", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[FlagSpec::true_flag("--json")],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("DAY"))],
        ));
    };
    let Some(stream) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("STREAM"))],
        ));
    };
    let Some(segment) = parsed.positionals.get(2) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SEGMENT"))],
        ));
    };
    let Some(source) = parsed.positionals.get(3) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SOURCE"))],
        ));
    };
    let Some(sentence_id) = parsed
        .positionals
        .get(4)
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SENTENCE_ID"))],
        ));
    };
    let stream_layout = match parse_stream_layout_option(locale, &parsed) {
        Ok(layout) => layout,
        Err(err) => return err,
    };
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/owner/tag-cli",
        vec![],
        Some(
            json!({"day": day, "stream_layout": stream_layout, "stream": stream, "segment_key": segment, "source": source, "sentence_id": sentence_id}),
        ),
    ) {
        Ok(result) => result,
        Err(error)
            if matches!(
                error.reason_code(),
                Some(
                    "entity_blocked"
                        | "invalid_day"
                        | "invalid_segment_or_stream"
                        | "speaker_owner_identity_required"
                        | "speaker_sentence_missing"
                )
            ) =>
        {
            return detail_error(locale, error);
        }
        Err(error) => return speaker_error(locale, error),
    };
    if parsed.flag("--json") {
        return stdout_json(&result);
    }
    if string_field(&result, "status").as_deref() == Some("already_assigned") {
        stdout_line(display_for(
            locale,
            "journal.cli.sol.speakers.tag.owner.owner.sentence.already.tagged",
            [
                ("day", &(day)),
                ("stream_layout", &(stream_layout)),
                ("stream", &(stream)),
                ("segment", &(segment)),
                ("sentence_id", &(sentence_id)),
            ],
        ))
    } else {
        stdout_line(display_for(
            locale,
            "journal.cli.sol.speakers.tag.owner.tagged.owner.sentence",
            [
                ("day", &(day)),
                ("stream_layout", &(stream_layout)),
                ("stream", &(stream)),
                ("segment", &(segment)),
                ("sentence_id", &(sentence_id)),
            ],
        ))
    }
}

#[must_use]
pub fn sentences(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("DAY"))],
        ));
    };
    let Some(stream) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("STREAM"))],
        ));
    };
    let Some(segment) = parsed.positionals.get(2) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SEGMENT"))],
        ));
    };
    let Some(source) = parsed.positionals.get(3) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SOURCE"))],
        ));
    };
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        &format!("/app/speakers/api/review-cli/{day}/{stream}/{segment}/{source}"),
        vec![],
        None,
    ) {
        Ok(body) => body,
        Err(error) => return speaker_error(locale, error),
    };
    if parsed.flag("--json") {
        return crate::agent_output::literal_output(stdout_json(&body));
    }
    let body = crate::agent_output::literal_body(body);
    let mut out = String::new();
    emit(
        &mut out,
        display_for(
            locale,
            "journal.cli.sol.speakers.sentences.sentences.for",
            [
                ("day", &(day)),
                ("stream", &(stream)),
                ("segment", &(segment)),
                ("source", &(source)),
            ],
        ),
    );
    for sentence in array_field(&body, "sentences") {
        let marker = if sentence
            .get("has_embedding")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "*"
        } else {
            "-"
        };
        emit(
            &mut out,
            format!(
                "{marker} {}: {}",
                value_to_string(sentence.get("sentence_id")),
                value_to_string(sentence.get("text")),
            ),
        );
    }
    CommandOutput::success(out)
}

#[must_use]
pub fn day_segments(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--limit", Some("-n"))];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[FlagSpec::true_flag("--json")],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("DAY"))],
        ));
    };
    let limit = parsed.value("--limit").unwrap_or("20");
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        &format!("/app/speakers/api/segments-cli/{day}"),
        vec![QueryParam::single("limit", limit)],
        None,
    ) {
        Ok(body) => body,
        Err(error) => return speaker_error(locale, error),
    };
    if parsed.flag("--json") {
        return stdout_json(&body);
    }
    let mut out = String::new();
    emit(&mut out, {
        let display_shown = (value_to_string(body.get("returned"))).to_string();
        let display_total = (value_to_string(body.get("total"))).to_string();
        let display_limit = (value_to_string(body.get("limit"))).to_string();
        render_for(
            locale,
            "journal.cli.sol.speakers.day.segments.showing.of.segments.limit",
            &[
                ("shown", CopyValue::String(&display_shown)),
                ("total", CopyValue::String(&display_total)),
                ("limit", CopyValue::String(&display_limit)),
                (
                    "quantity",
                    crate::command::count_selector(display_total.parse().unwrap_or(0)),
                ),
            ],
        )
    });
    for segment in array_field(&body, "segments") {
        emit(
            &mut out,
            format!(
                "{}/{}: {}",
                value_to_string(segment.get("stream")),
                value_to_string(segment.get("key")),
                string_array(segment.get("sources")).join(", "),
            ),
        );
    }
    CommandOutput::success(out)
}

#[must_use]
pub fn confirm_owner(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(
        locale,
        ctx.args,
        &[],
        &[
            FlagSpec::paired("--backfill", "--no-backfill", true),
            FlagSpec::true_flag("--json"),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let json_output = parsed.flag("--json");
    // The speakers overview holds the owner review, so the link needs no day.
    let review_entry = "/app/speakers/";
    if json_output {
        let detail = "Owner voice candidates must be confirmed in the browser.";
        CommandOutput::failure(
            format!(
                "{}\n",
                json!({
                    "reason_code": "review_required", "error": "review_required",
                    "detail": detail, "message": format!("{detail}\nReview and confirm at {review_entry}"),
                    "review_entry": review_entry,
                })
            ),
            1,
        )
    } else {
        stderr(display_for(
            locale,
            "journal.cli.sol.speakers.confirm.owner.review.error.owner.voice.candidates.must",
            [("review_entry", &(review_entry))],
        ))
    }
}

#[must_use]
pub fn reject_owner(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(
        locale,
        ctx.args,
        &[("--version", None)],
        &[FlagSpec::true_flag("--json")],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(version) = parsed.value("--version") else {
        return stderr(render_for(
            locale,
            "journal.cli.sol.speakers.reject.owner.error.version.is.required",
            &[],
        ));
    };
    post_json(
        ctx,
        "/app/speakers/api/owner/reject-cli",
        json!({"version": version}),
    )
}

#[must_use]
pub fn owner_ready(ctx: CommandContext<'_>) -> CommandOutput {
    post_json(ctx, "/app/speakers/api/owner/ready", Value::Null)
}

fn parse_json_commit(locale: &str, args: &[String]) -> Result<ParsedArgs, String> {
    parse_args(
        locale,
        args,
        &[],
        &[
            FlagSpec::true_flag("--commit"),
            FlagSpec::true_flag("--json"),
        ],
    )
}

#[derive(Debug, Default)]
struct ParsedArgs {
    positionals: Vec<String>,
    values: Vec<(String, String)>,
    bools: Vec<(String, bool)>,
}

impl ParsedArgs {
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| value.as_str())
    }

    fn values(&self, name: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(key, _value)| key == name)
            .map(|(_key, value)| value.clone())
            .collect()
    }

    fn bool_value(&self, name: &str) -> Option<bool> {
        self.bools
            .iter()
            .rev()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| *value)
    }

    fn flag(&self, name: &str) -> bool {
        self.bool_value(name).unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy)]
struct FlagSpec {
    canonical: &'static str,
    primary: &'static str,
    secondary: Option<&'static str>,
    default: bool,
}

impl FlagSpec {
    const fn true_flag(token: &'static str) -> Self {
        Self {
            canonical: token,
            primary: token,
            secondary: None,
            default: false,
        }
    }

    const fn paired(primary: &'static str, secondary: &'static str, default: bool) -> Self {
        Self {
            canonical: primary,
            primary,
            secondary: Some(secondary),
            default,
        }
    }
}

fn locale_for_options(ctx: CommandContext<'_>, options: &[(&str, Option<&str>)]) -> &'static str {
    ctx.human_locale_matching(|token| {
        options
            .iter()
            .any(|&(name, alias)| name == token || alias == Some(token))
    })
}

fn parse_args(
    locale: &str,
    args: &[String],
    value_options: &[(&str, Option<&str>)],
    flags: &[FlagSpec],
) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    for flag in flags {
        if flag.default {
            parsed
                .bools
                .push((flag.canonical.to_string(), flag.default));
        }
    }
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if let Some((name, value)) = token.split_once('=')
            && let Some(canonical) = canonical_value_option(name, value_options)
        {
            parsed
                .values
                .push((canonical.to_string(), value.to_string()));
        } else if let Some(canonical) = canonical_value_option(token, value_options) {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(display_for(
                    locale,
                    "journal.cli.client.option.value",
                    [("option", &(token))],
                ));
            };
            parsed.values.push((canonical.to_string(), value.clone()));
        } else if let Some((canonical, value)) = canonical_flag(token, flags) {
            parsed.bools.push((canonical.to_string(), value));
        } else if token.starts_with('-') {
            return Err(display_for(
                locale,
                "journal.cli.client.option.unknown",
                [("option", &(token))],
            ));
        } else {
            parsed.positionals.push(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn canonical_value_option<'a>(
    token: &str,
    options: &'a [(&'a str, Option<&'a str>)],
) -> Option<&'a str> {
    options.iter().find_map(|(long, short)| {
        if token == *long || short.is_some_and(|short| token == short) {
            Some(*long)
        } else {
            None
        }
    })
}

fn canonical_flag<'a>(token: &str, flags: &'a [FlagSpec]) -> Option<(&'a str, bool)> {
    flags.iter().find_map(|flag| {
        if token == flag.primary {
            Some((flag.canonical, true))
        } else if flag.secondary.is_some_and(|secondary| token == secondary) {
            Some((flag.canonical, false))
        } else {
            None
        }
    })
}

fn request_pretty_get(ctx: CommandContext<'_>, path: &str) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    match request_json(locale, ctx, HttpMethod::Get, path, vec![], None) {
        Ok(value) => crate::agent_output::literal_output(stdout_json(&value)),
        Err(error) => speaker_error(locale, error),
    }
}

fn post_json(ctx: CommandContext<'_>, path: &str, body: Value) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let json = if body.is_null() { None } else { Some(body) };
    match request_json(locale, ctx, HttpMethod::Post, path, vec![], json) {
        Ok(value) => crate::agent_output::literal_output(stdout_json(&value)),
        Err(error) => speaker_error(locale, error),
    }
}

fn request_json(
    locale: &str,
    ctx: CommandContext<'_>,
    method: HttpMethod,
    path: &str,
    params: Vec<QueryParam>,
    json: Option<Value>,
) -> Result<Value, ClientError> {
    let response = ctx.transport.request(ApiRequest {
        method,
        path: path.to_string(),
        params,
        json,
        policy: TimeoutPolicy::Api,
        headers: crate::transport::locale_headers(locale),
    })?;
    decode_response(&response)
}

fn speaker_error(locale: &str, error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(crate::error::service_down_for(locale)),
        other => stderr(other.message_for(locale)),
    }
}

fn discovery_error(locale: &str, error: ClientError, json_output: bool) -> CommandOutput {
    if let ClientError::ReasonRejected { payload, .. } = &error
        && !payload.is_null()
    {
        if json_output {
            return CommandOutput {
                stdout: String::new(),
                stderr: format!("{}\n", json_pretty_utf8(payload)),
                exit: 1,
            };
        }
        let mut err = String::new();
        emit(&mut err, error.message_for(locale));
        emit(
            &mut err,
            render_for(
                locale,
                "journal.cli.sol.speakers.discovery.error.try.again",
                &[],
            ),
        );
        return CommandOutput {
            stdout: String::new(),
            stderr: err,
            exit: 1,
        };
    }
    speaker_error(locale, error)
}

fn speaker_error_preserving_stdout(
    locale: &str,
    stdout: String,
    error: ClientError,
) -> CommandOutput {
    let stderr = match error {
        ClientError::Unreachable { .. } => format!("{}\n", crate::error::service_down_for(locale)),
        other => format!("{}\n", other.message_for(locale)),
    };
    CommandOutput {
        stdout,
        stderr,
        exit: 1,
    }
}

fn detail_error(locale: &str, error: ClientError) -> CommandOutput {
    // A server detail can name a speaker.
    stderr(crate::agent_output::literal_links(
        &error
            .detail_for(locale)
            .unwrap_or_else(|| error.message_for(locale)),
    ))
}

fn identify_error(locale: &str, error: ClientError, request_id: Option<&str>) -> CommandOutput {
    let payload = match &error {
        ClientError::ReasonRejected { payload, .. } => payload.as_ref(),
        _ => &Value::Null,
    };
    let operation_id = string_field(payload, "operation_id");
    let retry_request_id = request_id
        .map(str::to_string)
        .or_else(|| string_field(payload, "request_id"));
    let mut err = String::new();
    emit(
        &mut err,
        crate::agent_output::literal_links(
            &error
                .detail_for(locale)
                .unwrap_or_else(|| error.message_for(locale)),
        ),
    );
    if let Some(request_id) = retry_request_id {
        emit(
            &mut err,
            display_for(
                locale,
                "journal.cli.sol.speakers.identify.error.retry.with.the.same.request",
                [("request_id", &(request_id))],
            ),
        );
    }
    emit(
        &mut err,
        render_for(
            locale,
            "journal.cli.sol.speakers.identify.error.inspect.operations.with.solstone.call",
            &[],
        ),
    );
    if let Some(operation_id) = operation_id {
        emit(
            &mut err,
            display_for(
                locale,
                "journal.cli.sol.speakers.identify.error.inspect.this.operation.with.solstone",
                [("operation_id", &(operation_id))],
            ),
        );
    }
    CommandOutput {
        stdout: String::new(),
        stderr: err,
        exit: 1,
    }
}

fn is_identify_failure(error: &ClientError) -> bool {
    error
        .reason_code()
        .is_some_and(|code| IDENTIFY_FAILURE_CODES.contains(&code))
}

fn stdout_json(value: &Value) -> CommandOutput {
    CommandOutput::success(format!("{}\n", json_pretty_ascii(value)))
}

fn stdout_line(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", value.as_ref()))
}

fn parse_stream_layout_option(
    locale: &str,
    parsed: &ParsedArgs,
) -> Result<&'static str, CommandOutput> {
    match parsed.value("--stream-layout") {
        None => Ok("named"),
        Some("direct") => Ok("direct"),
        Some("named") => Ok("named"),
        Some(_) => Err(stderr(render_for(
            locale,
            "journal.cli.sol.speakers.parse.stream.layout.option.error.invalid.value.for.stream",
            &[],
        ))),
    }
}

fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}

fn emit(out: &mut String, line: impl AsRef<str>) {
    out.push_str(line.as_ref());
    out.push('\n');
}

fn render_discovery_warnings(locale: &str, out: &mut String, result: &Value) {
    for issue in array_field(result, "issues") {
        let reason_code = string_field(issue, "reason_code").unwrap_or_default();
        let message = string_field(issue, "message").unwrap_or_default();
        let count = issue.get("count").and_then(Value::as_i64).unwrap_or(0);
        if count == 0 {
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.discovery.warnings.warning",
                    [("reason_code", &(reason_code)), ("message", &(message))],
                ),
            );
        } else {
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.discovery.warnings.warning.count",
                    [
                        ("reason_code", &(reason_code)),
                        ("message", &(message)),
                        ("count", &(count)),
                    ],
                ),
            );
        }
    }
}

fn render_bootstrap_stats(locale: &str, out: &mut String, stats: &Value) {
    emit(out, "");
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.segments.scanned",
            [("count", &(value_to_string(stats.get("segments_scanned"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.single.speaker.segments",
            [(
                "count",
                &(value_to_string(stats.get("single_speaker_segments"))),
            )],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.unique.speakers",
            [("count", &(object_len(stats.get("speakers_found"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.entities.created",
            [("count", &(value_to_string(stats.get("entities_created"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.embeddings.saved",
            [("count", &(value_to_string(stats.get("embeddings_saved"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.owner",
            [(
                "count",
                &(value_to_string(stats.get("embeddings_skipped_owner"))),
            )],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.duplicate",
            [(
                "count",
                &(value_to_string(stats.get("embeddings_skipped_duplicate"))),
            )],
        ),
    );
    render_speaker_counts(
        out,
        stats.get("speakers_found"),
        render_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.top.speakers.by.embedding.count",
            &[],
        ),
    );
    render_errors(locale, out, stats.get("errors"));
}

fn render_resolve_names(locale: &str, out: &mut String, stats: &Value) {
    emit(out, "");
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.resolve.names.entities.with.voiceprints",
            [(
                "count",
                &(value_to_string(stats.get("entities_with_voiceprints"))),
            )],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.resolve.names.pairs.compared",
            [("count", &(value_to_string(stats.get("pairs_compared"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.resolve.names.high.similarity.pairs",
            [("count", &(array_field(stats, "matches_found").len()))],
        ),
    );
    for item in array_field(stats, "auto_merged") {
        if let Some(preview) = item.get("preview") {
            emit(out, preview.to_string());
        }
    }
    let auto = array_field(stats, "auto_merged");
    if !auto.is_empty() {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.resolve.names.auto.merged",
                [("count", &(auto.len()))],
            ),
        );
        for item in auto {
            emit(
                out,
                format!(
                    "  {} -> {} ({})",
                    value_to_string(item.get("alias")),
                    value_to_string(item.get("canonical")),
                    value_to_string(item.get("similarity")),
                ),
            );
        }
    }
    let ambiguous = array_field(stats, "ambiguous");
    if !ambiguous.is_empty() {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.resolve.names.ambiguous",
                [("count", &(ambiguous.len()))],
            ),
        );
        for item in ambiguous {
            let candidates = array_field(item, "candidates")
                .iter()
                .map(|candidate| {
                    format!(
                        "{} ({})",
                        value_to_string(candidate.get("name")),
                        value_to_string(candidate.get("similarity"))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            emit(
                out,
                format!("  {}: {candidates}", value_to_string(item.get("name"))),
            );
        }
    }
    render_errors(locale, out, stats.get("errors"));
}

fn render_attribute_segment(
    locale: &str,
    out: &mut String,
    wrap: &Value,
    result: &Value,
    commit: bool,
    save: bool,
    accumulate: bool,
) {
    let labels = array_field(result, "labels");
    let unmatched = array_field(result, "unmatched");
    let resolved = labels
        .iter()
        .filter(|label| !label.get("speaker").unwrap_or(&Value::Null).is_null())
        .count();
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.attribute.segment.sentences",
            [("count", &(labels.len()))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.attribute.segment.resolved",
            [("resolved", &(resolved))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.attribute.segment.unmatched",
            [("count", &(unmatched.len()))],
        ),
    );
    let mut methods: Map<String, Value> = Map::new();
    for label in labels {
        let method = string_field(label, "method").unwrap_or_else(|| "unmatched".to_string());
        let count = methods.get(&method).and_then(Value::as_i64).unwrap_or(0) + 1;
        methods.insert(method, Value::Number(count.into()));
    }
    emit(
        out,
        render_for(
            locale,
            "journal.cli.sol.speakers.attribute.segment.by.method",
            &[],
        ),
    );
    let mut names = methods.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for method in names {
        emit(
            out,
            format!("  {method}: {}", value_to_string(methods.get(&method))),
        );
    }
    if commit && save {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.attribute.segment.wrote",
                [("path", &(value_to_string(wrap.get("written_path"))))],
            ),
        );
    }
    if commit
        && accumulate
        && !result.get("source").unwrap_or(&Value::Null).is_null()
        && let Some(saved) = wrap.get("accumulated").and_then(Value::as_object)
        && !saved.is_empty()
    {
        emit(
            out,
            render_for(
                locale,
                "journal.cli.sol.speakers.attribute.segment.accumulated.voiceprints",
                &[],
            ),
        );
        for (entity_id, count) in saved {
            emit(out, {
                let display_entity_id = (entity_id).to_string();
                let display_embeddings = (value_to_string(Some(count))).to_string();
                render_for(
                    locale,
                    "journal.cli.sol.speakers.attribute.segment.embeddings",
                    &[
                        ("entity_id", CopyValue::String(&display_entity_id)),
                        ("embeddings", CopyValue::String(&display_embeddings)),
                        (
                            "quantity",
                            crate::command::count_selector(display_embeddings.parse().unwrap_or(0)),
                        ),
                    ],
                )
            });
        }
    }
}

fn render_propagate(locale: &str, out: &mut String, result: &Value, commit: bool) {
    let statement_count = result
        .get("statement_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let segment_count = result
        .get("segment_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if statement_count == 0 {
        emit(
            out,
            render_for(
                locale,
                "journal.cli.sol.speakers.propagate.nothing.else.would.change",
                &[],
            ),
        );
    } else {
        emit(
            out,
            if commit {
                {
                    let display_statement_count = (statement_count).to_string();
                    let display_segment_count = (segment_count).to_string();
                    render_for(
                        locale,
                        "journal.cli.sol.speakers.propagate.applied.applied.statements.in.segments",
                        &[
                            (
                                "statement_count",
                                CopyValue::String(&display_statement_count),
                            ),
                            ("segment_count", CopyValue::String(&display_segment_count)),
                            (
                                "quantity",
                                crate::command::count_selector(
                                    display_statement_count.parse().unwrap_or(0),
                                ),
                            ),
                            (
                                "segment_count_quantity",
                                crate::command::count_selector(
                                    display_segment_count.parse().unwrap_or(0),
                                ),
                            ),
                        ],
                    )
                }
            } else {
                {
                    let display_statement_count = (statement_count).to_string();
                    let display_segment_count = (segment_count).to_string();
                    render_for(
                        locale,
                        "journal.cli.sol.speakers.propagate.would.change.would.change.statements.in.segments",
                        &[
                            (
                                "statement_count",
                                CopyValue::String(&display_statement_count),
                            ),
                            ("segment_count", CopyValue::String(&display_segment_count)),
                            (
                                "quantity",
                                crate::command::count_selector(
                                    display_statement_count.parse().unwrap_or(0),
                                ),
                            ),
                            (
                                "segment_count_quantity",
                                crate::command::count_selector(
                                    display_segment_count.parse().unwrap_or(0),
                                ),
                            ),
                        ],
                    )
                }
            },
        );
    }
    let errors = array_field(result, "errors");
    if !errors.is_empty() {
        emit(
            out,
            render_for(locale, "journal.cli.sol.speakers.propagate.errors", &[]),
        );
        for error in errors {
            emit(out, format!("  {}", value_to_string(Some(error))));
        }
    }
    if commit
        && let Some(reversal) = result.get("reversal").and_then(Value::as_object)
        && !reversal.is_empty()
    {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.propagate.reverse.with.solstone.call.commit",
                [
                    ("operation", &(value_to_string(reversal.get("verb")))),
                    (
                        "old_speaker",
                        &(value_to_string(reversal.get("old_speaker"))),
                    ),
                    (
                        "new_speaker",
                        &(value_to_string(reversal.get("new_speaker"))),
                    ),
                ],
            ),
        );
    }
}

fn render_backfill(locale: &str, out: &mut String, stats: &Value, elapsed: f64) {
    emit(out, "\n");
    if let Some(op_id) = string_field(stats, "operation_id") {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.backfill.operation.id",
                [("op_id", &(op_id))],
            ),
        );
    }
    if let Some(status) = string_field(stats, "status") {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.backfill.status",
                [("status", &(status))],
            ),
        );
    }
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.total.segments.scanned",
            [("count", &(value_to_string(stats.get("total_scanned"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.selected.for.backfill",
            [("count", &(value_to_string(stats.get("selected_count"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.protected.skipped",
            [("count", &(value_to_string(stats.get("protected_skipped"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.total.planned.count",
            [("count", &(value_to_string(stats.get("total_count"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.completed.count",
            [("count", &(value_to_string(stats.get("completed_count"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.pending.count",
            [("count", &(value_to_string(stats.get("pending_count"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.error.count",
            [("count", &(value_to_string(stats.get("error_count"))))],
        ),
    );
    if elapsed > 0.0 {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.backfill.elapsed.s",
                [("elapsed", &(format!("{:.1}", elapsed)))],
            ),
        );
    }
    if let Some(stage) = string_field(stats, "failure_stage") {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.backfill.failure.stage",
                [("stage", &(stage))],
            ),
        );
    }
    if let Some(detail) = string_field(stats, "failure_detail") {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.backfill.failure.detail",
                [("detail", &(detail))],
            ),
        );
    }
    render_backfill_errors(locale, out, stats.get("error_segments"), 10);
}

fn render_backfill_last_seen(locale: &str, out: &mut String, stats: &Value) {
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.last.seen.speaker.label.files.read",
            [("count", &(value_to_string(stats.get("labels_read"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.last.seen.entities.seen",
            [("count", &(value_to_string(stats.get("entities_seen"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.last.seen.voiceprint.rows.scanned",
            [("count", &(value_to_string(stats.get("rows_scanned"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.last.seen.rows.pending",
            [("count", &(value_to_string(stats.get("rows_pending"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.backfill.last.seen.rows.written",
            [("count", &(value_to_string(stats.get("rows_written"))))],
        ),
    );
    if let Some(pending) = stats.get("pending").and_then(Value::as_object)
        && !pending.is_empty()
    {
        emit(
            out,
            render_for(
                locale,
                "journal.cli.sol.speakers.backfill.last.seen.pending.by.entity",
                &[],
            ),
        );
        for (entity_id, item) in pending {
            emit(
                out,
                format!("  {entity_id}: {}", value_to_string(item.get("rows")),),
            );
        }
    }
    render_errors(locale, out, stats.get("errors"));
}

/// `erase_day` is the day the trash erases what the wipe took, as the owner
/// reads it.
fn render_wipe(locale: &str, out: &mut String, report: &Value, erase_day: Option<&str>) {
    for (key, label) in [
        ("segment_embeddings", "segment_embeddings "),
        ("speaker_labels", "speaker_labels     "),
        ("speaker_corrections", "speaker_corrections"),
        ("entity_voiceprints", "entity_voiceprints "),
        ("owner_centroids", "owner_centroids    "),
        ("owner_candidate", "owner_candidate    "),
        ("speaker_candidates", "speaker_candidates "),
    ] {
        let item = report.get(key).and_then(Value::as_object);
        emit(out, {
            let display_label = (label).to_string();
            let display_files =
                (value_to_string(item.and_then(|value| value.get("count")))).to_string();
            let display_bytes =
                (value_to_string(item.and_then(|value| value.get("bytes")))).to_string();
            render_for(
                locale,
                "journal.cli.sol.speakers.wipe.files.b",
                &[
                    ("label", CopyValue::String(&display_label)),
                    ("files", CopyValue::String(&display_files)),
                    ("bytes", CopyValue::String(&display_bytes)),
                    (
                        "quantity",
                        crate::command::count_selector(display_files.parse().unwrap_or(0)),
                    ),
                ],
            )
        });
    }
    emit(out, {
        let display_files = (value_to_string(report.get("total_files"))).to_string();
        let display_bytes = (value_to_string(report.get("total_bytes"))).to_string();
        render_for(
            locale,
            "journal.cli.sol.speakers.wipe.total.files.b",
            &[
                ("files", CopyValue::String(&display_files)),
                ("bytes", CopyValue::String(&display_bytes)),
                (
                    "quantity",
                    crate::command::count_selector(display_files.parse().unwrap_or(0)),
                ),
            ],
        )
    });
    if report.get("erased").and_then(Value::as_bool) == Some(true) {
        emit(
            out,
            render_for(locale, "journal.cli.sol.speakers.wipe.erased.erased", &[]),
        );
    } else if report.get("trash").is_some_and(Value::is_object) {
        if report.get("erase_failed").and_then(Value::as_bool) == Some(true) {
            emit(
                out,
                render_for(
                    locale,
                    "journal.cli.sol.speakers.wipe.couldn.t.erase.now.it",
                    &[],
                ),
            );
        }
        match erase_day {
            Some(day) => emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.wipe.in.the.trash.erases",
                    [("day", &(day))],
                ),
            ),
            None => emit(
                out,
                render_for(locale, "journal.cli.sol.speakers.wipe.in.the.trash", &[]),
            ),
        }
    }
}

fn render_presence(locale: &str, out: &mut String, result: &Value) {
    let facts = result.get("facts").and_then(Value::as_object);
    emit(out, {
        let display_cluster = (value_to_string(result.get("cluster_id"))).to_string();
        let display_statements =
            (value_or_zero(facts.and_then(|item| item.get("statement_count")))).to_string();
        let display_segments =
            (value_or_zero(facts.and_then(|item| item.get("segment_count")))).to_string();
        let display_conversations =
            (value_or_zero(facts.and_then(|item| item.get("conversation_count")))).to_string();
        render_for(
            locale,
            "journal.cli.sol.speakers.presence.cluster.statements.segments.conversations",
            &[
                ("cluster", CopyValue::String(&display_cluster)),
                ("statements", CopyValue::String(&display_statements)),
                ("segments", CopyValue::String(&display_segments)),
                ("conversations", CopyValue::String(&display_conversations)),
                (
                    "quantity",
                    crate::command::count_selector(display_statements.parse().unwrap_or(0)),
                ),
                (
                    "segments_quantity",
                    crate::command::count_selector(display_segments.parse().unwrap_or(0)),
                ),
                (
                    "conversations_quantity",
                    crate::command::count_selector(display_conversations.parse().unwrap_or(0)),
                ),
            ],
        )
    });
    if result.get("evidence_complete").and_then(Value::as_bool) == Some(true) {
        emit(
            out,
            render_for(
                locale,
                "journal.cli.sol.speakers.presence.evidence.complete",
                &[],
            ),
        );
    } else {
        emit(out, {
            let display_gaps = (array_field(result, "evidence_gaps").len()).to_string();
            render_for(
                locale,
                "journal.cli.sol.speakers.presence.evidence.gap.s",
                &[
                    ("gaps", CopyValue::String(&display_gaps)),
                    (
                        "quantity",
                        crate::command::count_selector(display_gaps.parse().unwrap_or(0)),
                    ),
                ],
            )
        });
    }
    let candidates = result.get("candidates").and_then(Value::as_object);
    let co_presence = candidates
        .and_then(|item| item.get("co_presence"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mention = candidates
        .and_then(|item| item.get("mention"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if co_presence.is_empty() && mention.is_empty() {
        emit(
            out,
            render_for(
                locale,
                "journal.cli.sol.speakers.presence.no.candidate.entities.found",
                &[],
            ),
        );
        return;
    }
    if !co_presence.is_empty() {
        emit(
            out,
            render_for(locale, "journal.cli.sol.speakers.presence.co.presence", &[]),
        );
        for candidate in &co_presence {
            emit(
                out,
                candidate_line(locale, candidate, "screen_conversations", "meeting_days"),
            );
        }
    }
    if !mention.is_empty() {
        emit(
            out,
            render_for(locale, "journal.cli.sol.speakers.presence.mentions", &[]),
        );
        for candidate in &mention {
            emit(
                out,
                candidate_line(
                    locale,
                    candidate,
                    "setting_conversations",
                    "speaker_conversations",
                ),
            );
        }
    }
}

fn candidate_line(locale: &str, candidate: &Value, first: &str, second: &str) -> String {
    let voice = candidate
        .get("has_voice")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let message = match (first == "screen_conversations", voice) {
        (true, true) => {
            "journal.cli.sol.speakers.presence.candidate.meeting.voiced.screen.meeting.days.voice.yes"
        }
        (true, false) => {
            "journal.cli.sol.speakers.presence.candidate.meeting.unvoiced.screen.meeting.days.voice.no"
        }
        (false, true) => {
            "journal.cli.sol.speakers.presence.candidate.mention.voiced.setting.speakers.voice.yes"
        }
        (false, false) => {
            "journal.cli.sol.speakers.presence.candidate.mention.unvoiced.setting.speakers.voice.no"
        }
    };
    display_for(
        locale,
        message,
        [
            ("name", &(value_to_string(candidate.get("name")))),
            ("entity", &(value_to_string(candidate.get("entity_id")))),
            ("first_count", &(value_to_string(candidate.get(first)))),
            ("second_count", &(value_to_string(candidate.get(second)))),
        ],
    )
}

fn render_seed_from_imports(locale: &str, out: &mut String, stats: &Value) {
    emit(out, "");
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.segments.scanned",
            [("count", &(value_to_string(stats.get("segments_scanned"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.seed.from.imports.segments.with.speakers",
            [(
                "count",
                &(value_to_string(stats.get("segments_with_speakers"))),
            )],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.unique.speakers",
            [("count", &(object_len(stats.get("speakers_found"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.embeddings.saved",
            [("count", &(value_to_string(stats.get("embeddings_saved"))))],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.owner",
            [(
                "count",
                &(value_to_string(stats.get("embeddings_skipped_owner"))),
            )],
        ),
    );
    emit(
        out,
        display_for(
            locale,
            "journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.duplicate",
            [(
                "count",
                &(value_to_string(stats.get("embeddings_skipped_duplicate"))),
            )],
        ),
    );
    render_speaker_counts(
        out,
        stats.get("speakers_found"),
        render_for(
            locale,
            "journal.cli.sol.speakers.seed.from.imports.speakers.by.embedding.count",
            &[],
        ),
    );
    let unmatched = array_field(stats, "speakers_unmatched");
    if !unmatched.is_empty() {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.seed.from.imports.unmatched.speakers",
                [("count", &(unmatched.len()))],
            ),
        );
        for name in unmatched {
            emit(out, format!("  {}", value_to_string(Some(name))));
        }
    }
    render_errors(locale, out, stats.get("errors"));
}

fn render_build_from_tags(locale: &str, out: &mut String, result: &Value) {
    match string_field(result, "status").as_deref() {
        Some("confirmed")
            if string_field(result, "next_step").as_deref() == Some("rebuild_owner") =>
        {
            emit(
                out,
                render_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.owner.centroid.already.confirmed",
                    &[],
                ),
            );
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.next.step",
                    [("step", &(value_to_string(result.get("next_step"))))],
                ),
            );
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.guidance",
                    [("guidance", &(value_to_string(result.get("guidance"))))],
                ),
            );
        }
        Some("confirmed") => emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.build.from.tags.owner.centroid.confirmed.from.manual",
                [
                    ("principal", &(value_to_string(result.get("principal_id")))),
                    (
                        "cluster_size",
                        &(value_to_string(result.get("cluster_size"))),
                    ),
                ],
            ),
        ),
        Some("low_quality") => {
            emit(
                out,
                render_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.owner.manual.tags.are.not",
                    &[],
                ),
            );
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.reason",
                    [(
                        "reason",
                        &(value_to_string(result.get("low_quality_reason"))),
                    )],
                ),
            );
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.observed",
                    [("observed", &(value_to_string(result.get("observed_value"))))],
                ),
            );
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.threshold",
                    [(
                        "threshold",
                        &(value_to_string(result.get("threshold_value"))),
                    )],
                ),
            );
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.manual.tags",
                    [("tags", &(value_to_string(result.get("manual_tags_count"))))],
                ),
            );
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.can.build.from.tags",
                    [(
                        "available",
                        &(value_to_string(result.get("can_build_from_tags"))),
                    )],
                ),
            );
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.next.step",
                    [("step", &(value_to_string(result.get("next_step"))))],
                ),
            );
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.guidance",
                    [("guidance", &(value_to_string(result.get("guidance"))))],
                ),
            );
        }
        _ => emit(out, json_pretty_ascii(result)),
    }
}

fn render_rebuild_owner(locale: &str, out: &mut String, result: &Value) {
    match string_field(result, "status").as_deref() {
        Some("rebuilt") => {
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.rebuild.owner.owner.centroid.rebuilt.principal.cluster",
                    [
                        ("principal", &(value_to_string(result.get("principal_id")))),
                        (
                            "cluster_size",
                            &(value_to_string(result.get("cluster_size"))),
                        ),
                    ],
                ),
            );
            if result.get("override_applied").and_then(Value::as_bool) == Some(true) {
                emit(
                    out,
                    render_for(
                        locale,
                        "journal.cli.sol.speakers.rebuild.owner.override.applied.true",
                        &[],
                    ),
                );
            }
            if string_field(result, "next_step").as_deref() != Some("none") {
                emit(
                    out,
                    display_for(
                        locale,
                        "journal.cli.sol.speakers.build.from.tags.next.step",
                        [("step", &(value_to_string(result.get("next_step"))))],
                    ),
                );
                emit(
                    out,
                    display_for(
                        locale,
                        "journal.cli.sol.speakers.build.from.tags.guidance",
                        [("guidance", &(value_to_string(result.get("guidance"))))],
                    ),
                );
            }
        }
        Some("unchanged") => {
            emit(
                out,
                render_for(
                    locale,
                    "journal.cli.sol.speakers.rebuild.owner.owner.centroid.unchanged.evidence.already",
                    &[],
                ),
            );
            if string_field(result, "next_step").as_deref() != Some("none") {
                emit(
                    out,
                    display_for(
                        locale,
                        "journal.cli.sol.speakers.rebuild.owner.next.step",
                        [("next_step", &(value_to_string(result.get("next_step"))))],
                    ),
                );
                emit(
                    out,
                    display_for(
                        locale,
                        "journal.cli.sol.speakers.rebuild.owner.guidance",
                        [("step", &(value_to_string(result.get("guidance"))))],
                    ),
                );
            }
        }
        Some("low_quality" | "refused" | "rejected_regression") => {
            emit(
                out,
                render_for(
                    locale,
                    "journal.cli.sol.speakers.rebuild.owner.owner.centroid.rebuild.did.not",
                    &[],
                ),
            );
            if let Some(reason) = result
                .get("reason")
                .or_else(|| result.get("low_quality_reason"))
            {
                emit(
                    out,
                    display_for(
                        locale,
                        "journal.cli.sol.speakers.build.from.tags.reason",
                        [("reason", &(value_to_string(Some(reason))))],
                    ),
                );
            }
            if result.get("observed_value").is_some() {
                emit(
                    out,
                    display_for(
                        locale,
                        "journal.cli.sol.speakers.build.from.tags.observed",
                        [("observed", &(value_to_string(result.get("observed_value"))))],
                    ),
                );
            }
            if result.get("threshold_value").is_some() {
                emit(
                    out,
                    display_for(
                        locale,
                        "journal.cli.sol.speakers.build.from.tags.threshold",
                        [(
                            "threshold",
                            &(value_to_string(result.get("threshold_value"))),
                        )],
                    ),
                );
            }
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.next.step",
                    [("step", &(value_to_string(result.get("next_step"))))],
                ),
            );
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.build.from.tags.guidance",
                    [("guidance", &(value_to_string(result.get("guidance"))))],
                ),
            );
        }
        _ => emit(out, json_pretty_ascii(result)),
    }
}

fn render_speaker_counts(out: &mut String, value: Option<&Value>, title: String) {
    if let Some(items) = value.and_then(Value::as_object)
        && !items.is_empty()
    {
        emit(out, format!("\n{title}"));
        let mut pairs = items.iter().collect::<Vec<_>>();
        pairs.sort_by_key(|item| Reverse(item.1.as_i64()));
        for (name, count) in pairs.into_iter().take(15) {
            // The keys are speaker names, which the body's escaping leaves alone.
            let name = crate::agent_output::literal_links(name);
            emit(out, format!("  {name}: {}", value_to_string(Some(count))));
        }
    }
}

fn render_errors(locale: &str, out: &mut String, value: Option<&Value>) {
    let errors = value.and_then(Value::as_array).cloned().unwrap_or_default();
    if !errors.is_empty() {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.errors.errors",
                [("count", &(errors.len()))],
            ),
        );
        for error in &errors {
            emit(out, format!("  {}", value_to_string(Some(error))));
        }
    }
}

fn render_backfill_errors(locale: &str, out: &mut String, value: Option<&Value>, limit: usize) {
    let errors = value.and_then(Value::as_array).cloned().unwrap_or_default();
    if !errors.is_empty() {
        emit(
            out,
            display_for(
                locale,
                "journal.cli.sol.speakers.errors.errors",
                [("count", &(errors.len()))],
            ),
        );
        for error in errors.iter().take(limit) {
            let seg = error.get("segment").unwrap_or(error);
            emit(
                out,
                format!(
                    "  {}/{}/{}: {}",
                    value_to_string(seg.get("day")),
                    value_to_string(seg.get("stream")),
                    value_to_string(seg.get("segment_key")),
                    value_to_string(error.get("detail")),
                ),
            );
        }
        if errors.len() > limit {
            emit(
                out,
                display_for(
                    locale,
                    "journal.cli.sol.speakers.backfill.errors.and.more",
                    [("count", &(errors.len() - limit))],
                ),
            );
        }
    }
}

fn array_field<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|item| match item {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    })
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| value_to_string(Some(item)))
                .collect()
        })
        .unwrap_or_default()
}

fn object_len(value: Option<&Value>) -> usize {
    value.and_then(Value::as_object).map_or(0, Map::len)
}

fn value_or_zero(value: Option<&Value>) -> String {
    value.map_or_else(|| "0".to_string(), |item| value_to_string(Some(item)))
}

fn value_to_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Null) | None => "None".to_string(),
        Some(other) => other.to_string(),
    }
}

fn monotonic_seconds(ctx: CommandContext<'_>) -> f64 {
    ctx.clock
        .map(|clock| clock.monotonic())
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use super::*;
    use crate::error::ClientError;
    use crate::seam::{Clock, ExpectedHttpCall, FakeClock, ScriptedHttpTransport};
    use crate::transport::{ApiRequest, HttpMethod, HttpResponse, TimeoutPolicy};
    use serde_json::json;

    #[test]
    fn a_wipe_in_the_trash_says_the_day_it_is_erased_not_the_time() {
        let report = json!({"trash": {"id": "t1", "erase_at": "2026-11-17T06:00:00.000Z", "erase_day": "20261116"}});
        let day = crate::command::format_erase_day("20261116", Some("20261111")).unwrap();
        let mut out = String::new();
        render_wipe("en-US", &mut out, &report, Some(&day));
        assert!(out.contains(&day), "{out}");
        assert!(!out.contains("2026-11-17"), "{out}");
    }

    #[test]
    fn repair_dry_run_reports_the_planned_removals_the_server_sends() {
        let mut out = String::new();
        render_repair_dry_run(
            "en-US",
            &mut out,
            &json!({
                "mode": "dry_run",
                "complete": true,
                "clean": false,
                "planned_removals": [
                    {"entity_id": "tool_a", "entity_dir": "tool_a", "voiceprint_keys": [], "voiceprint_count": 3}
                ],
                "planned_segments": [{"day": "20260101"}, {"day": "20260102"}],
                "gaps": [],
            }),
        );
        assert!(out.contains("tool_a (3 rows)"), "{out}");
        assert!(out.contains(": 2"), "{out}");
    }

    #[test]
    fn parse_stream_layout_option_accepts_omitted_named_and_direct() {
        let args = ParsedArgs::default();
        assert_eq!(parse_stream_layout_option("en-US", &args).unwrap(), "named");

        let mut args = ParsedArgs::default();
        args.values
            .push(("--stream-layout".to_string(), "direct".to_string()));
        assert_eq!(
            parse_stream_layout_option("en-US", &args).unwrap(),
            "direct"
        );

        let mut args = ParsedArgs::default();
        args.values
            .push(("--stream-layout".to_string(), "named".to_string()));
        assert_eq!(parse_stream_layout_option("en-US", &args).unwrap(), "named");

        let mut args = ParsedArgs::default();
        args.values
            .push(("--stream-layout".to_string(), "Direct".to_string()));
        let err = parse_stream_layout_option("en-US", &args).unwrap_err();
        assert_eq!(err.exit, 1);
        assert_eq!(err.stderr, "Error: invalid value for --stream-layout\n");

        let mut args = ParsedArgs::default();
        args.values
            .push(("--stream-layout".to_string(), "other".to_string()));
        let err = parse_stream_layout_option("en-US", &args).unwrap_err();
        assert_eq!(err.exit, 1);
        assert_eq!(err.stderr, "Error: invalid value for --stream-layout\n");
    }

    #[test]
    fn confirm_owner_refusal_json() {
        let args = vec!["--json".to_string()];
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![]);
        let output = confirm_owner(CommandContext {
            args: &args,
            env: &env,
            stdin: "".into(),
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        });

        assert_eq!(output.exit, 1);
        assert_eq!(output.stdout, "");
        let json_err: Value = serde_json::from_str(output.stderr.trim()).expect("stderr JSON");
        assert_eq!(json_err["reason_code"], "review_required");
        assert_eq!(json_err["error"], "review_required");
        assert_eq!(
            json_err["detail"],
            "Owner voice candidates must be confirmed in the browser."
        );
        assert_eq!(json_err["review_entry"], "/app/speakers/");
        assert!(transport.recorded().is_empty());
        transport.assert_done();
    }

    #[test]
    fn confirm_owner_refusal_plain() {
        let args = vec![];
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![]);
        let output = confirm_owner(CommandContext {
            args: &args,
            env: &env,
            stdin: "".into(),
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        });

        assert_eq!(output.exit, 1);
        assert_eq!(output.stdout, "");
        assert!(output.stderr.contains("/app/speakers/"));
        assert!(transport.recorded().is_empty());
        transport.assert_done();
    }

    #[test]
    fn confirm_owner_refusal_with_backfill_flags() {
        for flag in ["--backfill", "--no-backfill"] {
            let args = vec![flag.to_string(), "--json".to_string()];
            let env = BTreeMap::new();
            let transport = ScriptedHttpTransport::new(vec![]);
            let output = confirm_owner(CommandContext {
                args: &args,
                env: &env,
                stdin: "".into(),
                transport: &transport,
                clock: None,
                files: None,
                build_identity: None,
                client_item_ids: None,
                notification_sink: None,
                link_pairing: None,
                link_serve: None,
                link_status_probe: None,
            });

            assert_eq!(output.exit, 1);
            assert_eq!(output.stdout, "");
            let json_err: Value = serde_json::from_str(output.stderr.trim()).expect("stderr JSON");
            assert_eq!(json_err["reason_code"], "review_required");
            assert_eq!(json_err["error"], "review_required");
            assert_eq!(
                json_err["detail"],
                "Owner voice candidates must be confirmed in the browser."
            );
            assert_eq!(json_err["review_entry"], "/app/speakers/");
            assert!(transport.recorded().is_empty());
            transport.assert_done();
        }
    }

    #[test]
    fn reject_owner_requires_version() {
        let args = vec![];
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![]);
        let output = reject_owner(CommandContext {
            args: &args,
            env: &env,
            stdin: "".into(),
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        });

        assert_eq!(output.exit, 1);
        assert!(output.stderr.contains("--version is required"));
        assert!(transport.recorded().is_empty());
        transport.assert_done();
    }

    fn string_args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    fn json_response(value: Value, policy: TimeoutPolicy) -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&value).expect("json response"),
            policy,
        }
    }

    #[test]
    fn backfill_follow_polls_with_30s_interval_until_done() {
        let clock = FakeClock::at_unix(0);
        let transport = ScriptedHttpTransport::new(vec![
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/app/speakers/api/backfill".to_string(),
                    params: vec![],
                    json: Some(
                        json!({"commit": true, "reattribute": false, "accumulation": false}),
                    ),
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({
                        "operation_id": "bfop-follow1",
                        "status": "active_running",
                        "done": false,
                        "total_count": 2,
                        "completed_count": 0,
                        "pending_count": 2,
                    }),
                    TimeoutPolicy::Api,
                )),
            },
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Get,
                    path: "/app/speakers/api/backfill/operations/bfop-follow1".to_string(),
                    params: vec![],
                    json: None,
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({
                        "operation_id": "bfop-follow1",
                        "status": "active_running",
                        "done": false,
                        "total_count": 2,
                        "completed_count": 0,
                        "pending_count": 2,
                    }),
                    TimeoutPolicy::Api,
                )),
            },
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Get,
                    path: "/app/speakers/api/backfill/operations/bfop-follow1".to_string(),
                    params: vec![],
                    json: None,
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({
                        "operation_id": "bfop-follow1",
                        "status": "active_running",
                        "done": false,
                        "total_count": 2,
                        "completed_count": 1,
                        "pending_count": 1,
                    }),
                    TimeoutPolicy::Api,
                )),
            },
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Get,
                    path: "/app/speakers/api/backfill/operations/bfop-follow1".to_string(),
                    params: vec![],
                    json: None,
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({
                        "operation_id": "bfop-follow1",
                        "status": "done",
                        "done": true,
                        "total_count": 2,
                        "completed_count": 2,
                        "pending_count": 0,
                    }),
                    TimeoutPolicy::Api,
                )),
            },
        ]);

        let args = string_args(&["--commit"]);
        let env = BTreeMap::new();
        let ctx = CommandContext {
            args: &args,
            env: &env,
            stdin: "".into(),
            transport: &transport,
            clock: Some(&clock),
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        };

        let output = backfill(ctx);
        assert_eq!(output.exit, 0);
        assert!(clock.monotonic() >= Duration::from_secs(60));
    }

    #[test]
    fn backfill_follow_inactive_status_exits_nonzero_with_diagnostics() {
        let clock = FakeClock::at_unix(0);
        let transport = ScriptedHttpTransport::new(vec![
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/app/speakers/api/backfill".to_string(),
                    params: vec![],
                    json: Some(
                        json!({"commit": true, "reattribute": false, "accumulation": false}),
                    ),
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({
                        "operation_id": "bfop-inactive1",
                        "status": "active_running",
                        "done": false,
                    }),
                    TimeoutPolicy::Api,
                )),
            },
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Get,
                    path: "/app/speakers/api/backfill/operations/bfop-inactive1".to_string(),
                    params: vec![],
                    json: None,
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({
                        "operation_id": "bfop-inactive1",
                        "status": "inactive_interrupted",
                        "done": false,
                    }),
                    TimeoutPolicy::Api,
                )),
            },
        ]);

        let args = string_args(&["--commit", "--json"]);
        let env = BTreeMap::new();
        let ctx = CommandContext {
            args: &args,
            env: &env,
            stdin: "".into(),
            transport: &transport,
            clock: Some(&clock),
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        };

        let output = backfill(ctx);
        assert_ne!(output.exit, 0);
        assert!(output.stderr.contains("backfill-status"));
        assert!(output.stderr.contains("backfill-resume"));
        assert!(output.stderr.contains("bfop-inactive1"));
        let parsed_stdout: Value =
            serde_json::from_str(&output.stdout).expect("valid json in stdout");
        assert_eq!(parsed_stdout["status"], "inactive_interrupted");
    }

    #[test]
    fn backfill_follow_transport_failure_produces_empty_stdout_and_nonzero_exit() {
        let clock = FakeClock::at_unix(0);
        let transport = ScriptedHttpTransport::new(vec![
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/app/speakers/api/backfill".to_string(),
                    params: vec![],
                    json: Some(
                        json!({"commit": true, "reattribute": false, "accumulation": false}),
                    ),
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({
                        "operation_id": "bfop-failtrans",
                        "status": "active_running",
                        "done": false,
                    }),
                    TimeoutPolicy::Api,
                )),
            },
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Get,
                    path: "/app/speakers/api/backfill/operations/bfop-failtrans".to_string(),
                    params: vec![],
                    json: None,
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Err(ClientError::unreachable(Some(
                    "connection refused".to_string(),
                ))),
            },
        ]);

        let args = string_args(&["--commit", "--json"]);
        let env = BTreeMap::new();
        let ctx = CommandContext {
            args: &args,
            env: &env,
            stdin: "".into(),
            transport: &transport,
            clock: Some(&clock),
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        };

        let output = backfill(ctx);
        assert_ne!(output.exit, 0);
        assert_eq!(output.stdout.trim(), "");
    }

    fn repair_call(
        method: HttpMethod,
        path: &str,
        body: Option<Value>,
        result: Value,
    ) -> ExpectedHttpCall {
        ExpectedHttpCall::Request {
            expected: ApiRequest {
                method,
                path: path.to_string(),
                params: vec![],
                json: body,
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(json_response(result, TimeoutPolicy::Api)),
        }
    }

    fn run_speaker_command(
        command: fn(CommandContext<'_>) -> CommandOutput,
        args: &[&str],
        transport: &ScriptedHttpTransport,
        clock: &FakeClock,
    ) -> CommandOutput {
        let args = string_args(args);
        let env = BTreeMap::new();
        command(CommandContext {
            args: &args,
            env: &env,
            stdin: "".into(),
            transport,
            clock: Some(clock),
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        })
    }

    #[test]
    fn repair_commit_follows_the_running_operation_until_it_completes() {
        let clock = FakeClock::at_unix(0);
        let status_path = "/app/speakers/api/repair/operations/repair_0";
        let transport = ScriptedHttpTransport::new(vec![
            repair_call(
                HttpMethod::Post,
                "/app/speakers/api/repair",
                Some(json!({"commit": true, "operation_id": "repair_0"})),
                json!({"operation_id": "repair_0", "status": "running", "planned_segments_count": 2}),
            ),
            repair_call(
                HttpMethod::Get,
                status_path,
                None,
                json!({"operation_id": "repair_0", "status": "running", "checkpointed_count": 1}),
            ),
            repair_call(
                HttpMethod::Get,
                status_path,
                None,
                json!({
                    "operation_id": "repair_0",
                    "status": "completed",
                    "checkpointed_count": 2,
                    "summary": {"complete": true, "clean": true},
                }),
            ),
        ]);

        let output = run_speaker_command(repair, &["--commit"], &transport, &clock);
        assert_eq!(output.exit, 0, "{}", output.stderr);
        assert!(output.stdout.contains("Repair running: repair_0"));
        assert!(output.stdout.contains("Status:    completed"));
        assert!(output.stdout.contains("Clean:     true"));
        assert!(clock.monotonic() >= Duration::from_secs(60));
    }

    #[test]
    fn repair_commit_timeout_names_the_operation_to_follow() {
        let clock = FakeClock::at_unix(0);
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Post,
                path: "/app/speakers/api/repair".to_string(),
                params: vec![],
                json: Some(json!({"commit": true, "operation_id": "repair_0"})),
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Err(ClientError::timeout(None)),
        }]);

        let output = run_speaker_command(repair, &["--commit", "--json"], &transport, &clock);
        assert_ne!(output.exit, 0);
        assert!(output.stderr.contains("may still be running"));
        assert!(output.stderr.contains("repair-status repair_0"));
    }

    #[test]
    fn repair_resume_that_stops_exits_nonzero_with_the_failure_and_advice() {
        let clock = FakeClock::at_unix(0);
        let transport = ScriptedHttpTransport::new(vec![
            repair_call(
                HttpMethod::Post,
                "/app/speakers/api/repair/operations/repair_7/resume",
                None,
                json!({"operation_id": "repair_7", "status": "running"}),
            ),
            repair_call(
                HttpMethod::Get,
                "/app/speakers/api/repair/operations/repair_7",
                None,
                json!({
                    "operation_id": "repair_7",
                    "status": "failed",
                    "checkpointed_count": 3,
                    "latest_failure": {"stage": "prepared_validation", "detail": "drift", "retryable": true},
                }),
            ),
        ]);

        let output = run_speaker_command(repair_resume, &["repair_7"], &transport, &clock);
        assert_ne!(output.exit, 0);
        assert!(
            output
                .stdout
                .contains("Last failure: prepared_validation (drift)")
        );
        assert!(output.stdout.contains("repair-resume repair_7"));
    }

    #[test]
    fn repair_commit_refusal_offers_no_advice() {
        let clock = FakeClock::at_unix(0);
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Post,
                path: "/app/speakers/api/repair".to_string(),
                params: vec![],
                json: Some(json!({"commit": true, "operation_id": "repair_0"})),
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Err(ClientError::unreachable(None)),
        }]);

        let output = run_speaker_command(repair, &["--commit"], &transport, &clock);
        assert_ne!(output.exit, 0);
        assert!(!output.stderr.contains("repair-status"));
        assert!(!output.stderr.contains("repair-resume"));
    }

    #[test]
    fn repair_that_cannot_be_resumed_is_not_offered_a_resume() {
        let clock = FakeClock::at_unix(0);
        let transport = ScriptedHttpTransport::new(vec![
            repair_call(
                HttpMethod::Post,
                "/app/speakers/api/repair/operations/repair_8/resume",
                None,
                json!({"operation_id": "repair_8", "status": "running"}),
            ),
            repair_call(
                HttpMethod::Get,
                "/app/speakers/api/repair/operations/repair_8",
                None,
                json!({
                    "operation_id": "repair_8",
                    "status": "failed",
                    "latest_failure": {"stage": "directory_proof", "detail": "mismatch", "retryable": false},
                }),
            ),
        ]);

        let output = run_speaker_command(repair_resume, &["repair_8"], &transport, &clock);
        assert_ne!(output.exit, 0);
        assert!(
            output
                .stdout
                .contains("Last failure: directory_proof (mismatch)")
        );
        assert!(!output.stdout.contains("repair-resume"));
    }

    #[test]
    fn repair_status_of_an_interrupted_repair_offers_a_resume() {
        let clock = FakeClock::at_unix(0);
        let transport = ScriptedHttpTransport::new(vec![repair_call(
            HttpMethod::Get,
            "/app/speakers/api/repair/operations/repair_9",
            None,
            json!({"operation_id": "repair_9", "status": "interrupted", "checkpointed_count": 5}),
        )]);

        let output = run_speaker_command(repair_status, &["repair_9"], &transport, &clock);
        assert_eq!(output.exit, 0);
        assert!(output.stdout.contains("Status:    interrupted"));
        assert!(output.stdout.contains("repair-resume repair_9"));
    }

    #[test]
    fn a_lost_journal_while_following_does_not_say_the_repair_may_be_running() {
        let clock = FakeClock::at_unix(0);
        let transport = ScriptedHttpTransport::new(vec![
            repair_call(
                HttpMethod::Post,
                "/app/speakers/api/repair/operations/repair_5/resume",
                None,
                json!({"operation_id": "repair_5", "status": "running"}),
            ),
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Get,
                    path: "/app/speakers/api/repair/operations/repair_5".to_string(),
                    params: vec![],
                    json: None,
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Err(ClientError::unreachable(None)),
            },
        ]);

        let output = run_speaker_command(repair_resume, &["repair_5"], &transport, &clock);
        assert_ne!(output.exit, 0);
        assert!(!output.stderr.contains("may still be running"));
        assert!(output.stderr.contains("repair-status repair_5"));
    }
}
