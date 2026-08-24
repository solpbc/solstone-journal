// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cmp::Reverse;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, SERVICE_DOWN_MESSAGE};
use crate::json_format::{json_pretty_ascii, json_pretty_utf8};
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const REPORT_ONLY: &str = "REPORT ONLY — pass --commit to persist.\n";
const IDENTIFY_NAME_REQUIRED_ERROR: &str = "Usage: call speakers identify [OPTIONS] CLUSTER_ID [NAME]\nTry 'call speakers identify --help' for help.\n╭─ Error ──────────────────────────────────────────────────────────────────────╮\n│ Invalid value: name or --entity-id is required                               │\n╰──────────────────────────────────────────────────────────────────────────────╯\n";
const IDENTIFY_FAILURE_CODES: &[&str] = &[
    "speaker_identify_recoverable",
    "speaker_identify_repair_required",
    "speaker_identify_conflict",
    "speaker_identify_operation_not_found",
];

#[must_use]
pub fn status(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let body = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/speakers/api/status",
        vec![],
        None,
    ) {
        Ok(body) => body,
        Err(error) => return speaker_error(error),
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
    stdout_json(&result)
}

#[must_use]
pub fn bootstrap(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = parse_json_commit(ctx.args);
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(&mut out, REPORT_ONLY);
    }
    if !json_output {
        emit(
            &mut out,
            "Bootstrapping voiceprints from single-speaker segments...",
        );
    }
    let stats = match request_json(
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
                stderr: format!("Error: {}\n", error.detail().unwrap_or(error.message())),
                exit: 1,
            };
        }
        Err(error) => return speaker_error_preserving_stdout(out, error),
    };
    if json_output {
        return stdout_json(&stats);
    }
    render_bootstrap_stats(&mut out, &stats);
    CommandOutput::success(out)
}

#[must_use]
pub fn resolve_names(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_json_commit(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(&mut out, REPORT_ONLY);
    }
    if !json_output {
        emit(&mut out, "Resolving speaker name variants...");
    }
    let stats = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/resolve-names",
        vec![],
        Some(json!({"commit": commit})),
    ) {
        Ok(stats) => stats,
        Err(error) => return speaker_error_preserving_stdout(out, error),
    };
    if json_output {
        return stdout_json(&stats);
    }
    render_resolve_names(&mut out, &stats);
    CommandOutput::success(out)
}

#[must_use]
pub fn attribute_segment(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[],
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
        return stderr("Error: missing argument DAY");
    };
    let Some(stream) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument STREAM");
    };
    let Some(segment) = parsed.positionals.get(2) else {
        return stderr("Error: missing argument SEGMENT");
    };
    let commit = parsed.flag("--commit");
    let save = parsed.bool_value("--save").unwrap_or(true);
    let accumulate = parsed.bool_value("--accumulate").unwrap_or(true);
    let json_output = parsed.flag("--json");
    let wrap = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/attribute-segment",
        vec![],
        Some(json!({
            "day": day,
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
                emit(&mut out, REPORT_ONLY);
            }
            return CommandOutput {
                stdout: out,
                stderr: format!("Error: {}\n", error.detail().unwrap_or(error.message())),
                exit: 1,
            };
        }
        Err(error) => return speaker_error(error),
    };
    let mut out = String::new();
    if !commit && !json_output {
        emit(&mut out, REPORT_ONLY);
    }
    let result = wrap.get("result").cloned().unwrap_or(Value::Null);
    if json_output {
        return stdout_json(&result);
    }
    render_attribute_segment(&mut out, &wrap, &result, commit, save, accumulate);
    CommandOutput::success(out)
}

#[must_use]
pub fn correct(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day) = parsed.positionals.first() else {
        return stderr("Error: missing argument DAY");
    };
    let Some(stream) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument STREAM");
    };
    let Some(segment) = parsed.positionals.get(2) else {
        return stderr("Error: missing argument SEGMENT");
    };
    let Some(source) = parsed.positionals.get(3) else {
        return stderr("Error: missing argument SOURCE");
    };
    let Some(sentence_id) = parsed
        .positionals
        .get(4)
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return stderr("Error: missing argument SENTENCE_ID");
    };
    let Some(new_speaker) = parsed.positionals.get(5) else {
        return stderr("Error: missing argument NEW_SPEAKER");
    };
    let result = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/correct-attribution",
        vec![],
        Some(json!({
            "day": day,
            "stream": stream,
            "segment_key": segment,
            "source": source,
            "sentence_id": sentence_id,
            "new_speaker": new_speaker,
        })),
    ) {
        Ok(result) => result,
        Err(error) => return speaker_error(error),
    };
    if parsed.flag("--json") {
        return stdout_json(&result);
    }
    let mut out = String::new();
    if string_field(&result, "status").as_deref() == Some("already_correct") {
        emit(
            &mut out,
            format!("Already correct: {day}/{stream}/{segment} #{sentence_id}"),
        );
        return CommandOutput::success(out);
    }
    let old = string_field(&result, "old_speaker").unwrap_or_else(|| "unassigned".to_string());
    emit(
        &mut out,
        format!(
            "Corrected {day}/{stream}/{segment} #{sentence_id}: {old} -> {}",
            value_to_string(result.get("new_speaker"))
        ),
    );
    let removal = result.get("voiceprint_removal").and_then(Value::as_object);
    emit(
        &mut out,
        format!(
            "Voiceprint removal: {}",
            value_to_string(removal.and_then(|item| item.get("outcome")))
        ),
    );
    let offer = result.get("propagation_offer").and_then(Value::as_object);
    if offer
        .and_then(|item| item.get("available"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        emit(
            &mut out,
            format!(
                "Propagation available: {} statements in {} segments would change",
                value_or_zero(offer.and_then(|item| item.get("statement_count"))),
                value_or_zero(offer.and_then(|item| item.get("segment_count"))),
            ),
        );
        emit(
            &mut out,
            format!(
                "Preview with: solstone call speakers propagate-correction {} {}",
                value_to_string(result.get("old_speaker")),
                value_to_string(result.get("new_speaker")),
            ),
        );
    } else {
        emit(&mut out, "Propagation: nothing else would change");
    }
    CommandOutput::success(out)
}

#[must_use]
pub fn propagate_correction(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_json_commit(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(old_speaker) = parsed.positionals.first() else {
        return stderr("Error: missing argument OLD_SPEAKER");
    };
    let Some(new_speaker) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument NEW_SPEAKER");
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(&mut out, REPORT_ONLY);
    }
    let result = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/propagate-correction",
        vec![],
        Some(json!({"old_speaker": old_speaker, "new_speaker": new_speaker, "commit": commit})),
    ) {
        Ok(result) => result,
        Err(error) => return speaker_error_preserving_stdout(out, error),
    };
    if json_output {
        return stdout_json(&result);
    }
    render_propagate(&mut out, &result, commit);
    CommandOutput::success(out)
}

#[must_use]
pub fn backfill(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[],
        &[
            FlagSpec::true_flag("--commit"),
            FlagSpec::true_flag("--reattribute"),
            FlagSpec::true_flag("--json"),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let reattribute = parsed.flag("--reattribute");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(&mut out, REPORT_ONLY);
    }
    if !json_output {
        emit(&mut out, "Scanning journal for segments with embeddings...");
    }
    let start = monotonic_seconds(ctx);
    let stats = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/backfill",
        vec![],
        Some(json!({"commit": commit, "reattribute": reattribute})),
    ) {
        Ok(stats) => stats,
        Err(error) => return speaker_error_preserving_stdout(out, error),
    };
    let elapsed = monotonic_seconds(ctx) - start;
    if json_output {
        return stdout_json(&stats);
    }
    render_backfill(&mut out, &stats, elapsed);
    CommandOutput::success(out)
}

#[must_use]
pub fn backfill_last_seen(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_json_commit(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(&mut out, REPORT_ONLY);
    }
    let stats = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/backfill-last-seen",
        vec![],
        Some(json!({"commit": commit})),
    ) {
        Ok(stats) => stats,
        Err(error) => return speaker_error_preserving_stdout(out, error),
    };
    if json_output {
        return stdout_json(&stats);
    }
    render_backfill_last_seen(&mut out, &stats);
    CommandOutput::success(out)
}

#[must_use]
pub fn wipe(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_json_commit(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(&mut out, REPORT_ONLY);
    }
    let report = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/wipe",
        vec![],
        Some(json!({"commit": commit})),
    ) {
        Ok(report) => report,
        Err(error) => return speaker_error_preserving_stdout(out, error),
    };
    if json_output {
        return stdout_json(&report);
    }
    render_wipe(&mut out, &report);
    CommandOutput::success(out)
}

#[must_use]
pub fn discover(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let json_output = parsed.flag("--json");
    let result = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/discovery/scan",
        vec![],
        None,
    ) {
        Ok(result) => result,
        Err(error) => return discovery_error(error, json_output),
    };
    if json_output {
        return stdout_json(&result);
    }
    let clusters = array_field(&result, "clusters");
    let mut out = String::new();
    if clusters.is_empty() {
        emit(&mut out, "No recurring unknown speakers found.");
    } else {
        emit(
            &mut out,
            format!("Found {} unknown speaker cluster(s):\n", clusters.len()),
        );
        for cluster in clusters {
            emit(
                &mut out,
                format!(
                    "  Cluster {}: {} samples across {} segments",
                    value_to_string(cluster.get("cluster_id")),
                    value_to_string(cluster.get("size")),
                    value_to_string(cluster.get("segment_count")),
                ),
            );
            for sample in array_field(cluster, "samples") {
                let text = string_field(sample, "text").unwrap_or_default();
                let preview = text.chars().take(60).collect::<String>();
                emit(
                    &mut out,
                    format!(
                        "    - {}/{}/{} sid={}: {}",
                        value_to_string(sample.get("day")),
                        value_to_string(sample.get("stream")),
                        value_to_string(sample.get("segment_key")),
                        value_to_string(sample.get("sentence_id")),
                        preview,
                    ),
                );
            }
            emit(&mut out, "");
        }
    }
    render_discovery_warnings(&mut out, &result);
    CommandOutput::success(out)
}

#[must_use]
pub fn presence(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(cluster_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument CLUSTER_ID");
    };
    let result = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/speakers/api/discovery/cluster/{cluster_id}/presence"),
        vec![],
        None,
    ) {
        Ok(result) => result,
        Err(error) if error.reason_code() == Some("speaker_review_unavailable") => {
            return CommandOutput::failure(
                format!(
                    "Cluster {cluster_id} was not found.\nRun 'solstone call speakers discover' to produce valid cluster ids.\n"
                ),
                1,
            );
        }
        Err(error) => return speaker_error(error),
    };
    if parsed.flag("--json") {
        return stdout_json(&result);
    }
    let mut out = String::new();
    render_presence(&mut out, &result);
    CommandOutput::success(out)
}

#[must_use]
pub fn identify(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
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
        return stderr("Error: missing argument CLUSTER_ID");
    };
    let name = parsed.positionals.get(1).cloned();
    let entity_id = parsed.value("--entity-id");
    if name.is_none() && entity_id.is_none() {
        return CommandOutput::failure(IDENTIFY_NAME_REQUIRED_ERROR, 2);
    }
    let request_id = parsed.value("--request-id").map(str::to_string);
    let reviewed_ids = parsed.values("--reviewed-near-match-entity-id");
    let reviewed_ids_json = if reviewed_ids.is_empty() {
        Value::Null
    } else {
        Value::Array(reviewed_ids.into_iter().map(Value::String).collect())
    };
    let result = match request_json(
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
            return identify_error(error, request_id.as_deref());
        }
        Err(error) if error.reason_code() == Some("speaker_command_failed") => {
            return detail_error(error);
        }
        Err(error) => return speaker_error(error),
    };
    stdout_json(&result)
}

#[must_use]
pub fn identify_undo(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(operation_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument OPERATION_ID");
    };
    match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/discovery/identify/undo",
        vec![],
        Some(json!({"operation_id": operation_id})),
    ) {
        Ok(result) => stdout_json(&result),
        Err(error) if is_identify_failure(&error) => identify_error(error, None),
        Err(error) if error.reason_code() == Some("speaker_command_failed") => detail_error(error),
        Err(error) => speaker_error(error),
    }
}

#[must_use]
pub fn identify_operations(ctx: CommandContext<'_>) -> CommandOutput {
    request_pretty_get(ctx, "/app/speakers/api/discovery/identify/operations")
}

#[must_use]
pub fn identify_operation(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(operation_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument OPERATION_ID");
    };
    match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/speakers/api/discovery/identify/operations/{operation_id}"),
        vec![],
        None,
    ) {
        Ok(result) => stdout_json(&result),
        Err(error) if is_identify_failure(&error) => identify_error(error, None),
        Err(error) => speaker_error(error),
    }
}

#[must_use]
pub fn dismiss_cluster(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--disposition", None)], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(cluster_id) = parsed
        .positionals
        .first()
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return stderr("Error: missing argument CLUSTER_ID");
    };
    let Some(disposition) = parsed.value("--disposition") else {
        return stderr("Error: option --disposition is required.");
    };
    match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/discovery/dismiss",
        vec![],
        Some(json!({"cluster_id": cluster_id, "disposition": disposition})),
    ) {
        Ok(result) => stdout_json(&result),
        Err(error) if error.reason_code() == Some("speaker_command_failed") => detail_error(error),
        Err(error) => speaker_error(error),
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
    let parsed = match parse_args(ctx.args, &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(alias) = parsed.positionals.first() else {
        return stderr("Error: missing argument ALIAS");
    };
    let Some(canonical) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument CANONICAL");
    };
    match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/merge-names",
        vec![],
        Some(json!({"alias": alias, "canonical": canonical})),
    ) {
        Ok(result) => stdout_json(&result),
        Err(error) if error.reason_code() == Some("speaker_command_failed") => detail_error(error),
        Err(error) => speaker_error(error),
    }
}

#[must_use]
pub fn link_import(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--entity-id", None)], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(name) = parsed.positionals.first() else {
        return stderr("Error: missing argument NAME");
    };
    let Some(entity_id) = parsed.value("--entity-id") else {
        return stderr("Error: option --entity-id is required.");
    };
    match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/link-import",
        vec![],
        Some(json!({"name": name, "entity_id": entity_id})),
    ) {
        Ok(result) => stdout_json(&result),
        Err(error) if error.reason_code() == Some("speaker_command_failed") => detail_error(error),
        Err(error) => speaker_error(error),
    }
}

#[must_use]
pub fn seed_from_imports(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_json_commit(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let commit = parsed.flag("--commit");
    let json_output = parsed.flag("--json");
    let mut out = String::new();
    if !commit && !json_output {
        emit(&mut out, REPORT_ONLY);
    }
    if !json_output {
        emit(&mut out, "Seeding voiceprints from import segments...");
    }
    let stats = match request_json(
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
                stderr: format!("Error: {}\n", error.detail().unwrap_or(error.message())),
                exit: 1,
            };
        }
        Err(error) => return speaker_error_preserving_stdout(out, error),
    };
    if json_output {
        return stdout_json(&stats);
    }
    render_seed_from_imports(&mut out, &stats);
    CommandOutput::success(out)
}

#[must_use]
pub fn suggest(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--limit", Some("-n"))],
        &[FlagSpec::true_flag("--json")],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let limit = parsed.value("--limit").unwrap_or("5");
    let body = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/speakers/api/suggest",
        vec![QueryParam::single("limit", limit)],
        None,
    ) {
        Ok(body) => body,
        Err(error) => return speaker_error(error),
    };
    if parsed.flag("--json") {
        return stdout_json(&body);
    }
    stdout_line(string_field(&body, "markdown").unwrap_or_default())
}

#[must_use]
pub fn detect(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[FlagSpec::true_flag("--force")]) {
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
    let parsed = match parse_args(ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let result = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/owner/build-from-tags",
        vec![],
        None,
    ) {
        Ok(result) => result,
        Err(error) => return speaker_error(error),
    };
    if parsed.flag("--json") {
        return stdout_json(&result);
    }
    let mut out = String::new();
    render_build_from_tags(&mut out, &result);
    CommandOutput::success(out)
}

#[must_use]
pub fn rebuild_owner(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
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
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/owner/rebuild",
        vec![],
        Some(json!({"override": parsed.flag("--override")})),
    ) {
        Ok(result) => result,
        Err(error) => return speaker_error(error),
    };
    if parsed.flag("--json") {
        return stdout_json(&result);
    }
    let mut out = String::new();
    render_rebuild_owner(&mut out, &result);
    CommandOutput::success(out)
}

#[must_use]
pub fn tag_owner(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day) = parsed.positionals.first() else {
        return stderr("Error: missing argument DAY");
    };
    let Some(stream) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument STREAM");
    };
    let Some(segment) = parsed.positionals.get(2) else {
        return stderr("Error: missing argument SEGMENT");
    };
    let Some(source) = parsed.positionals.get(3) else {
        return stderr("Error: missing argument SOURCE");
    };
    let Some(sentence_id) = parsed
        .positionals
        .get(4)
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return stderr("Error: missing argument SENTENCE_ID");
    };
    let result = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/owner/tag-cli",
        vec![],
        Some(
            json!({"day": day, "stream": stream, "segment_key": segment, "source": source, "sentence_id": sentence_id}),
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
            return detail_error(error);
        }
        Err(error) => return speaker_error(error),
    };
    if parsed.flag("--json") {
        return stdout_json(&result);
    }
    if string_field(&result, "status").as_deref() == Some("already_assigned") {
        stdout_line(format!(
            "Owner sentence already tagged: {day}/{stream}/{segment} #{sentence_id}"
        ))
    } else {
        stdout_line(format!(
            "Tagged owner sentence: {day}/{stream}/{segment} #{sentence_id}"
        ))
    }
}

#[must_use]
pub fn sentences(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[FlagSpec::true_flag("--json")]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day) = parsed.positionals.first() else {
        return stderr("Error: missing argument DAY");
    };
    let Some(stream) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument STREAM");
    };
    let Some(segment) = parsed.positionals.get(2) else {
        return stderr("Error: missing argument SEGMENT");
    };
    let Some(source) = parsed.positionals.get(3) else {
        return stderr("Error: missing argument SOURCE");
    };
    let body = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/speakers/api/review-cli/{day}/{stream}/{segment}/{source}"),
        vec![],
        None,
    ) {
        Ok(body) => body,
        Err(error) => return speaker_error(error),
    };
    if parsed.flag("--json") {
        return stdout_json(&body);
    }
    let mut out = String::new();
    emit(
        &mut out,
        format!("Sentences for {day}/{stream}/{segment}/{source}:"),
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
    let parsed = match parse_args(
        ctx.args,
        &[("--limit", Some("-n"))],
        &[FlagSpec::true_flag("--json")],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(day) = parsed.positionals.first() else {
        return stderr("Error: missing argument DAY");
    };
    let limit = parsed.value("--limit").unwrap_or("20");
    let body = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/speakers/api/segments-cli/{day}"),
        vec![QueryParam::single("limit", limit)],
        None,
    ) {
        Ok(body) => body,
        Err(error) => return speaker_error(error),
    };
    if parsed.flag("--json") {
        return stdout_json(&body);
    }
    let mut out = String::new();
    emit(
        &mut out,
        format!(
            "Showing {} of {} segments (limit {})",
            value_to_string(body.get("returned")),
            value_to_string(body.get("total")),
            value_to_string(body.get("limit")),
        ),
    );
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
    let parsed = match parse_args(
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
    let backfill_after = parsed.bool_value("--backfill").unwrap_or(true);
    let json_output = parsed.flag("--json");
    let mut result = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/speakers/api/owner/confirm-cli",
        vec![],
        None,
    ) {
        Ok(result) => result,
        Err(error) if error.reason_code() == Some("speaker_command_failed") => {
            return detail_error(error);
        }
        Err(error) => return speaker_error(error),
    };
    let mut out = String::new();
    if !json_output {
        emit(
            &mut out,
            format!(
                "Owner centroid confirmed (principal: {}, cluster_size: {})",
                value_to_string(result.get("principal_id")),
                value_to_string(result.get("cluster_size")),
            ),
        );
    }
    if backfill_after {
        if !json_output {
            emit(&mut out, "Running attribution backfill...");
        }
        let stats = match request_json(
            ctx,
            HttpMethod::Post,
            "/app/speakers/api/backfill",
            vec![],
            Some(json!({"commit": true})),
        ) {
            Ok(stats) => stats,
            Err(error) => return speaker_error_preserving_stdout(out, error),
        };
        if json_output {
            if let Value::Object(object) = &mut result {
                object.insert("backfill".to_string(), stats);
            }
        } else {
            emit(
                &mut out,
                format!(
                    "Backfill complete: {} segments processed, {} already labeled",
                    value_to_string(stats.get("processed")),
                    value_to_string(stats.get("already_labeled")),
                ),
            );
        }
    }
    if json_output {
        stdout_json(&result)
    } else {
        CommandOutput::success(out)
    }
}

#[must_use]
pub fn reject_owner(ctx: CommandContext<'_>) -> CommandOutput {
    post_json(ctx, "/app/speakers/api/owner/reject-cli", Value::Null)
}

#[must_use]
pub fn owner_ready(ctx: CommandContext<'_>) -> CommandOutput {
    post_json(ctx, "/app/speakers/api/owner/ready", Value::Null)
}

fn parse_json_commit(args: &[String]) -> Result<ParsedArgs, String> {
    parse_args(
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

fn parse_args(
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
                return Err(format!("Error: option {token} requires an argument."));
            };
            parsed.values.push((canonical.to_string(), value.clone()));
        } else if let Some((canonical, value)) = canonical_flag(token, flags) {
            parsed.bools.push((canonical.to_string(), value));
        } else if token.starts_with('-') {
            return Err(format!("Error: unknown option {token}."));
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
    match request_json(ctx, HttpMethod::Get, path, vec![], None) {
        Ok(value) => stdout_json(&value),
        Err(error) => speaker_error(error),
    }
}

fn post_json(ctx: CommandContext<'_>, path: &str, body: Value) -> CommandOutput {
    let json = if body.is_null() { None } else { Some(body) };
    match request_json(ctx, HttpMethod::Post, path, vec![], json) {
        Ok(value) => stdout_json(&value),
        Err(error) => speaker_error(error),
    }
}

fn request_json(
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
        headers: Vec::new(),
    })?;
    decode_response(&response)
}

fn speaker_error(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        other => stderr(other.message()),
    }
}

fn discovery_error(error: ClientError, json_output: bool) -> CommandOutput {
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
        emit(&mut err, error.message());
        emit(&mut err, "try again");
        return CommandOutput {
            stdout: String::new(),
            stderr: err,
            exit: 1,
        };
    }
    speaker_error(error)
}

fn speaker_error_preserving_stdout(stdout: String, error: ClientError) -> CommandOutput {
    let stderr = match error {
        ClientError::Unreachable { .. } => format!("{SERVICE_DOWN_MESSAGE}\n"),
        other => format!("{}\n", other.message()),
    };
    CommandOutput {
        stdout,
        stderr,
        exit: 1,
    }
}

fn detail_error(error: ClientError) -> CommandOutput {
    stderr(error.detail().unwrap_or(error.message()))
}

fn identify_error(error: ClientError, request_id: Option<&str>) -> CommandOutput {
    let payload = match &error {
        ClientError::ReasonRejected { payload, .. } => payload.as_ref(),
        _ => &Value::Null,
    };
    let operation_id = string_field(payload, "operation_id");
    let retry_request_id = request_id
        .map(str::to_string)
        .or_else(|| string_field(payload, "request_id"));
    let mut err = String::new();
    emit(&mut err, error.detail().unwrap_or(error.message()));
    if let Some(request_id) = retry_request_id {
        emit(
            &mut err,
            format!("Retry with the same --request-id {request_id}."),
        );
    }
    emit(
        &mut err,
        "Inspect operations with: solstone call speakers identify-operations",
    );
    if let Some(operation_id) = operation_id {
        emit(
            &mut err,
            format!(
                "Inspect this operation with: solstone call speakers identify-operation {operation_id}"
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

fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}

fn emit(out: &mut String, line: impl AsRef<str>) {
    out.push_str(line.as_ref());
    out.push('\n');
}

fn render_discovery_warnings(out: &mut String, result: &Value) {
    for issue in array_field(result, "issues") {
        let reason_code = string_field(issue, "reason_code").unwrap_or_default();
        let message = string_field(issue, "message").unwrap_or_default();
        let count = issue.get("count").and_then(Value::as_i64).unwrap_or(0);
        if count == 0 {
            emit(out, format!("Warning: {reason_code}: {message}"));
        } else {
            emit(
                out,
                format!("Warning: {reason_code}: {message} count={count}"),
            );
        }
    }
}

fn render_bootstrap_stats(out: &mut String, stats: &Value) {
    emit(out, "");
    emit(
        out,
        format!(
            "Segments scanned: {}",
            value_to_string(stats.get("segments_scanned"))
        ),
    );
    emit(
        out,
        format!(
            "Single-speaker segments: {}",
            value_to_string(stats.get("single_speaker_segments"))
        ),
    );
    emit(
        out,
        format!(
            "Unique speakers: {}",
            object_len(stats.get("speakers_found"))
        ),
    );
    emit(
        out,
        format!(
            "Entities created: {}",
            value_to_string(stats.get("entities_created"))
        ),
    );
    emit(
        out,
        format!(
            "Embeddings saved: {}",
            value_to_string(stats.get("embeddings_saved"))
        ),
    );
    emit(
        out,
        format!(
            "Embeddings skipped (owner): {}",
            value_to_string(stats.get("embeddings_skipped_owner"))
        ),
    );
    emit(
        out,
        format!(
            "Embeddings skipped (duplicate): {}",
            value_to_string(stats.get("embeddings_skipped_duplicate"))
        ),
    );
    render_speaker_counts(
        out,
        stats.get("speakers_found"),
        "Top speakers by embedding count:",
    );
    render_errors(out, stats.get("errors"));
}

fn render_resolve_names(out: &mut String, stats: &Value) {
    emit(out, "");
    emit(
        out,
        format!(
            "Entities with voiceprints: {}",
            value_to_string(stats.get("entities_with_voiceprints"))
        ),
    );
    emit(
        out,
        format!(
            "Pairs compared: {}",
            value_to_string(stats.get("pairs_compared"))
        ),
    );
    emit(
        out,
        format!(
            "High-similarity pairs: {}",
            array_field(stats, "matches_found").len()
        ),
    );
    let auto = array_field(stats, "auto_merged");
    if !auto.is_empty() {
        emit(out, format!("\nAuto-merged ({}):", auto.len()));
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
        emit(out, format!("\nAmbiguous ({}):", ambiguous.len()));
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
    render_errors(out, stats.get("errors"));
}

fn render_attribute_segment(
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
    emit(out, format!("Sentences: {}", labels.len()));
    emit(out, format!("Resolved:  {resolved}"));
    emit(out, format!("Unmatched: {}", unmatched.len()));
    let mut methods: Map<String, Value> = Map::new();
    for label in labels {
        let method = string_field(label, "method").unwrap_or_else(|| "unmatched".to_string());
        let count = methods.get(&method).and_then(Value::as_i64).unwrap_or(0) + 1;
        methods.insert(method, Value::Number(count.into()));
    }
    emit(out, "\nBy method:");
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
            format!("\nWrote: {}", value_to_string(wrap.get("written_path"))),
        );
    }
    if commit
        && accumulate
        && !result.get("source").unwrap_or(&Value::Null).is_null()
        && let Some(saved) = wrap.get("accumulated").and_then(Value::as_object)
        && !saved.is_empty()
    {
        emit(out, "\nAccumulated voiceprints:");
        for (entity_id, count) in saved {
            emit(
                out,
                format!("  {entity_id}: {} embeddings", value_to_string(Some(count))),
            );
        }
    }
}

fn render_propagate(out: &mut String, result: &Value, commit: bool) {
    let statement_count = result
        .get("statement_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let segment_count = result
        .get("segment_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let action = if commit { "Applied" } else { "Would change" };
    if statement_count == 0 {
        emit(out, "Nothing else would change.");
    } else {
        emit(
            out,
            format!("{action}: {statement_count} statements in {segment_count} segments"),
        );
    }
    let errors = array_field(result, "errors");
    if !errors.is_empty() {
        emit(out, "\nErrors:");
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
            format!(
                "Reverse with: solstone call {} {} {} --commit",
                value_to_string(reversal.get("verb")),
                value_to_string(reversal.get("old_speaker")),
                value_to_string(reversal.get("new_speaker")),
            ),
        );
    }
}

fn render_backfill(out: &mut String, stats: &Value, elapsed: f64) {
    emit(out, "\n");
    emit(
        out,
        format!(
            "Total segments scanned:    {}",
            value_to_string(stats.get("total_segments"))
        ),
    );
    emit(
        out,
        format!(
            "With embeddings:           {}",
            value_to_string(stats.get("total_eligible"))
        ),
    );
    emit(
        out,
        format!(
            "Without embeddings:        {}",
            value_to_string(stats.get("skipped_no_embed"))
        ),
    );
    emit(
        out,
        format!(
            "Already labeled (skipped): {}",
            value_to_string(stats.get("already_labeled"))
        ),
    );
    emit(
        out,
        format!(
            "Processed this run:        {}",
            value_to_string(stats.get("processed"))
        ),
    );
    emit(out, format!("Elapsed:                   {elapsed:.1}s"));
    if let Some(speakers) = stats.get("speakers_seen").and_then(Value::as_object)
        && !speakers.is_empty()
    {
        emit(out, format!("\nSpeakers identified ({}):", speakers.len()));
        let mut pairs = speakers.iter().collect::<Vec<_>>();
        pairs.sort_by_key(|item| Reverse(item.1.as_i64()));
        for (entity_id, count) in pairs.into_iter().take(20) {
            emit(
                out,
                format!(
                    "  {entity_id}: {} attributions",
                    value_to_string(Some(count))
                ),
            );
        }
    }
    render_backfill_errors(out, stats.get("error_segments"), 10);
}

fn render_backfill_last_seen(out: &mut String, stats: &Value) {
    emit(
        out,
        format!(
            "Speaker label files read: {}",
            value_to_string(stats.get("labels_read"))
        ),
    );
    emit(
        out,
        format!(
            "Entities seen:            {}",
            value_to_string(stats.get("entities_seen"))
        ),
    );
    emit(
        out,
        format!(
            "Voiceprint rows scanned:  {}",
            value_to_string(stats.get("rows_scanned"))
        ),
    );
    emit(
        out,
        format!(
            "Rows pending:             {}",
            value_to_string(stats.get("rows_pending"))
        ),
    );
    emit(
        out,
        format!(
            "Rows written:             {}",
            value_to_string(stats.get("rows_written"))
        ),
    );
    if let Some(pending) = stats.get("pending").and_then(Value::as_object)
        && !pending.is_empty()
    {
        emit(out, "\nPending by entity:");
        for (entity_id, item) in pending {
            emit(
                out,
                format!("  {entity_id}: {}", value_to_string(item.get("rows")),),
            );
        }
    }
    render_errors(out, stats.get("errors"));
}

fn render_wipe(out: &mut String, report: &Value) {
    for (key, label) in [
        ("segment_embeddings", "segment_embeddings "),
        ("speaker_labels", "speaker_labels     "),
        ("speaker_corrections", "speaker_corrections"),
        ("entity_voiceprints", "entity_voiceprints "),
        ("owner_centroids", "owner_centroids    "),
        ("owner_candidate", "owner_candidate    "),
    ] {
        let item = report.get(key).and_then(Value::as_object);
        emit(
            out,
            format!(
                "{label}: {} files ({} B)",
                value_to_string(item.and_then(|value| value.get("count"))),
                value_to_string(item.and_then(|value| value.get("bytes"))),
            ),
        );
    }
    emit(
        out,
        format!(
            "total              : {} files ({} B)",
            value_to_string(report.get("total_files")),
            value_to_string(report.get("total_bytes")),
        ),
    );
}

fn render_presence(out: &mut String, result: &Value) {
    let facts = result.get("facts").and_then(Value::as_object);
    emit(
        out,
        format!(
            "Cluster {}: {} statements, {} segments, {} conversations",
            value_to_string(result.get("cluster_id")),
            value_or_zero(facts.and_then(|item| item.get("statement_count"))),
            value_or_zero(facts.and_then(|item| item.get("segment_count"))),
            value_or_zero(facts.and_then(|item| item.get("conversation_count"))),
        ),
    );
    if result.get("evidence_complete").and_then(Value::as_bool) == Some(true) {
        emit(out, "Evidence: complete");
    } else {
        emit(
            out,
            format!(
                "Evidence: {} gap(s)",
                array_field(result, "evidence_gaps").len()
            ),
        );
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
        emit(out, "No candidate entities found.");
        return;
    }
    if !co_presence.is_empty() {
        emit(out, "\nCo-presence:");
        for candidate in &co_presence {
            emit(
                out,
                candidate_line(candidate, "screen_conversations", "meeting_days"),
            );
        }
    }
    if !mention.is_empty() {
        emit(out, "\nMentions:");
        for candidate in &mention {
            emit(
                out,
                candidate_line(candidate, "setting_conversations", "speaker_conversations"),
            );
        }
    }
}

fn candidate_line(candidate: &Value, first: &str, second: &str) -> String {
    format!(
        "  {} ({}): {}={}, {}={}, voice={}",
        value_to_string(candidate.get("name")),
        value_to_string(candidate.get("entity_id")),
        if first == "screen_conversations" {
            "screen"
        } else {
            "setting"
        },
        value_to_string(candidate.get(first)),
        if second == "meeting_days" {
            "meeting_days"
        } else {
            "speakers"
        },
        value_to_string(candidate.get(second)),
        if candidate
            .get("has_voice")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "yes"
        } else {
            "no"
        },
    )
}

fn render_seed_from_imports(out: &mut String, stats: &Value) {
    emit(out, "");
    emit(
        out,
        format!(
            "Segments scanned: {}",
            value_to_string(stats.get("segments_scanned"))
        ),
    );
    emit(
        out,
        format!(
            "Segments with speakers: {}",
            value_to_string(stats.get("segments_with_speakers"))
        ),
    );
    emit(
        out,
        format!(
            "Unique speakers: {}",
            object_len(stats.get("speakers_found"))
        ),
    );
    emit(
        out,
        format!(
            "Embeddings saved: {}",
            value_to_string(stats.get("embeddings_saved"))
        ),
    );
    emit(
        out,
        format!(
            "Embeddings skipped (owner): {}",
            value_to_string(stats.get("embeddings_skipped_owner"))
        ),
    );
    emit(
        out,
        format!(
            "Embeddings skipped (duplicate): {}",
            value_to_string(stats.get("embeddings_skipped_duplicate"))
        ),
    );
    render_speaker_counts(
        out,
        stats.get("speakers_found"),
        "Speakers by embedding count:",
    );
    let unmatched = array_field(stats, "speakers_unmatched");
    if !unmatched.is_empty() {
        emit(out, format!("\nUnmatched speakers ({}):", unmatched.len()));
        for name in unmatched {
            emit(out, format!("  {}", value_to_string(Some(name))));
        }
    }
    render_errors(out, stats.get("errors"));
}

fn render_build_from_tags(out: &mut String, result: &Value) {
    match string_field(result, "status").as_deref() {
        Some("confirmed")
            if string_field(result, "next_step").as_deref() == Some("rebuild_owner") =>
        {
            emit(out, "Owner centroid already confirmed.");
            emit(
                out,
                format!("Next step: {}", value_to_string(result.get("next_step"))),
            );
            emit(
                out,
                format!("Guidance: {}", value_to_string(result.get("guidance"))),
            );
        }
        Some("confirmed") => emit(
            out,
            format!(
                "Owner centroid confirmed from manual tags (principal: {}, cluster_size: {})",
                value_to_string(result.get("principal_id")),
                value_to_string(result.get("cluster_size")),
            ),
        ),
        Some("low_quality") => {
            emit(out, "Owner manual tags are not ready.");
            emit(
                out,
                format!(
                    "Reason: {}",
                    value_to_string(result.get("low_quality_reason"))
                ),
            );
            emit(
                out,
                format!(
                    "Observed: {}",
                    value_to_string(result.get("observed_value"))
                ),
            );
            emit(
                out,
                format!(
                    "Threshold: {}",
                    value_to_string(result.get("threshold_value"))
                ),
            );
            emit(
                out,
                format!(
                    "Manual tags: {}",
                    value_to_string(result.get("manual_tags_count"))
                ),
            );
            emit(
                out,
                format!(
                    "Can build from tags: {}",
                    value_to_string(result.get("can_build_from_tags"))
                ),
            );
            emit(
                out,
                format!("Next step: {}", value_to_string(result.get("next_step"))),
            );
            emit(
                out,
                format!("Guidance: {}", value_to_string(result.get("guidance"))),
            );
        }
        _ => emit(out, json_pretty_ascii(result)),
    }
}

fn render_rebuild_owner(out: &mut String, result: &Value) {
    match string_field(result, "status").as_deref() {
        Some("rebuilt") => {
            emit(
                out,
                format!(
                    "Owner centroid rebuilt (principal: {}, cluster_size: {})",
                    value_to_string(result.get("principal_id")),
                    value_to_string(result.get("cluster_size")),
                ),
            );
            if result.get("override_applied").and_then(Value::as_bool) == Some(true) {
                emit(out, "Override applied: true");
            }
            if string_field(result, "next_step").as_deref() != Some("none") {
                emit(
                    out,
                    format!("Next step: {}", value_to_string(result.get("next_step"))),
                );
                emit(
                    out,
                    format!("Guidance: {}", value_to_string(result.get("guidance"))),
                );
            }
        }
        Some("unchanged") => {
            emit(out, "Owner centroid unchanged; evidence already matches.");
            if string_field(result, "next_step").as_deref() != Some("none") {
                emit(
                    out,
                    format!("Next step: {}", value_to_string(result.get("next_step"))),
                );
                emit(
                    out,
                    format!("Guidance: {}", value_to_string(result.get("guidance"))),
                );
            }
        }
        Some("low_quality" | "refused" | "rejected_regression") => {
            emit(out, "Owner centroid rebuild did not write.");
            if let Some(reason) = result
                .get("reason")
                .or_else(|| result.get("low_quality_reason"))
            {
                emit(out, format!("Reason: {}", value_to_string(Some(reason))));
            }
            if result.get("observed_value").is_some() {
                emit(
                    out,
                    format!(
                        "Observed: {}",
                        value_to_string(result.get("observed_value"))
                    ),
                );
            }
            if result.get("threshold_value").is_some() {
                emit(
                    out,
                    format!(
                        "Threshold: {}",
                        value_to_string(result.get("threshold_value"))
                    ),
                );
            }
            emit(
                out,
                format!("Next step: {}", value_to_string(result.get("next_step"))),
            );
            emit(
                out,
                format!("Guidance: {}", value_to_string(result.get("guidance"))),
            );
        }
        _ => emit(out, json_pretty_ascii(result)),
    }
}

fn render_speaker_counts(out: &mut String, value: Option<&Value>, title: &str) {
    if let Some(items) = value.and_then(Value::as_object)
        && !items.is_empty()
    {
        emit(out, format!("\n{title}"));
        let mut pairs = items.iter().collect::<Vec<_>>();
        pairs.sort_by_key(|item| Reverse(item.1.as_i64()));
        for (name, count) in pairs.into_iter().take(15) {
            emit(out, format!("  {name}: {}", value_to_string(Some(count))));
        }
    }
}

fn render_errors(out: &mut String, value: Option<&Value>) {
    let errors = value.and_then(Value::as_array).cloned().unwrap_or_default();
    if !errors.is_empty() {
        emit(out, format!("\nErrors ({}):", errors.len()));
        for error in &errors {
            emit(out, format!("  {}", value_to_string(Some(error))));
        }
    }
}

fn render_backfill_errors(out: &mut String, value: Option<&Value>, limit: usize) {
    let errors = value.and_then(Value::as_array).cloned().unwrap_or_default();
    if !errors.is_empty() {
        emit(out, format!("\nErrors ({}):", errors.len()));
        for error in errors.iter().take(limit) {
            emit(
                out,
                format!(
                    "  {}/{}/{}: {}",
                    value_to_string(error.get("day")),
                    value_to_string(error.get("stream")),
                    value_to_string(error.get("segment_key")),
                    value_to_string(error.get("detail")),
                ),
            );
        }
        if errors.len() > limit {
            emit(out, format!("  ... and {} more", errors.len() - limit));
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
