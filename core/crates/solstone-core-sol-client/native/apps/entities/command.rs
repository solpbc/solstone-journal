// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Number, Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::ClientError;
use crate::json_format::{json_pretty_ascii, json_pretty_utf8};
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};
use solstone_core_cli_copy::{Value as CopyValue, display_for, render_for};

const ENTITY_HISTORY_BASE_ROUTE: &str = "/app/entities/api/journal";

#[must_use]
pub fn list(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(list_output(ctx))
}

fn list_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--facet", Some("-f")), ("--day", Some("-d"))];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let facet_arg = parsed
        .positionals
        .first()
        .map(String::as_str)
        .or_else(|| parsed.value("--facet"));
    let facet = match resolve_facet(locale, ctx, facet_arg) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let day = parsed.value("--day");
    let result = if let Some(day) = day {
        request_json(
            locale,
            ctx,
            HttpMethod::Get,
            &format!("/app/entities/api/{facet}/detected"),
            vec![QueryParam::single("day", day)],
            None,
        )
    } else {
        request_json(
            locale,
            ctx,
            HttpMethod::Get,
            &format!("/app/entities/api/{facet}"),
            vec![],
            None,
        )
    };
    let body = match result {
        Ok(body) => body,
        Err(error) => return entity_error(locale, error, None, None),
    };
    let entities = if day.is_some() {
        array_field(&body, "items")
    } else {
        array_field(&body, "attached")
    };
    if entities.is_empty() {
        return stdout_line(render_for(
            locale,
            "journal.cli.sol.entities.list.no.entities.found",
            &[],
        ));
    }
    let mut lines = Vec::new();
    lines.push(match day {
        Some(day) => render_for(
            locale,
            "journal.cli.sol.entities.list.detected.detected.for.entities",
            &[
                (
                    "quantity",
                    crate::command::count_selector(entities.len() as u64),
                ),
                ("count", CopyValue::String(&entities.len().to_string())),
                ("day", CopyValue::String(day)),
            ],
        ),
        None => render_for(
            locale,
            "journal.cli.sol.entities.list.attached.attached.entities",
            &[
                (
                    "quantity",
                    crate::command::count_selector(entities.len() as u64),
                ),
                ("count", CopyValue::String(&entities.len().to_string())),
            ],
        ),
    });
    for entity in entities {
        // A hidden detection is listed, and marked.
        lines.push(if entity.get("hidden") == Some(&Value::Bool(true)) {
            display_for(
                locale,
                "journal.cli.sol.entities.list.hidden.hidden",
                [
                    ("name", &(value_or_empty(entity.get("name")))),
                    ("entity_type", &(value_or_empty(entity.get("type")))),
                    (
                        "description",
                        &(value_or_default(entity.get("description"), "")),
                    ),
                ],
            )
        } else {
            display_for(
                locale,
                "journal.cli.sol.entities.list.visible.row",
                [
                    ("name", &(value_or_empty(entity.get("name")))),
                    ("entity_type", &(value_or_empty(entity.get("type")))),
                    (
                        "description",
                        &(value_or_default(entity.get("description"), "")),
                    ),
                ],
            )
        });
    }
    stdout(lines)
}

#[must_use]
pub fn move_entity(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(move_entity_output(ctx))
}

fn move_entity_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--from", None), ("--to", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[("--merge", None), ("--consent", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY"))],
        ));
    };
    let Some(from_facet) = parsed.value("--from") else {
        return stderr(display_for(
            locale,
            "journal.cli.client.option.required",
            [("option", &("--from"))],
        ));
    };
    let Some(to_facet) = parsed.value("--to") else {
        return stderr(display_for(
            locale,
            "journal.cli.client.option.required",
            [("option", &("--to"))],
        ));
    };
    let source_body = match resolve_request(locale, ctx, from_facet, entity) {
        Ok(body) => body,
        Err(output) => return output,
    };
    if !truthy(source_body.get("facet_exists")) {
        return stderr(display_for(
            locale,
            "journal.cli.sol.entities.move.entity.error.facet.from.does.not",
            [("from_facet", &(from_facet))],
        ));
    }
    let target_body = match resolve_request(locale, ctx, to_facet, entity) {
        Ok(body) => body,
        Err(output) => return output,
    };
    if !truthy(target_body.get("facet_exists")) {
        return stderr(display_for(
            locale,
            "journal.cli.sol.entities.move.entity.error.facet.to.does.not",
            [("to_facet", &(to_facet))],
        ));
    }
    let resolved = match resolved_from_body_or_exit(locale, from_facet, entity, &source_body) {
        Ok(resolved) => resolved,
        Err(output) => return output,
    };
    let entity_name = string_field(&resolved, "name").unwrap_or_else(|| entity.clone());
    let merge = parsed.bool_value("--merge").unwrap_or(false);
    let consent = parsed.bool_value("--consent").unwrap_or(false);
    match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/entities/api/move",
        vec![],
        Some(json!({
            "entity": entity_name,
            "from_facet": from_facet,
            "to_facet": to_facet,
            "merge": merge,
            "consent": consent,
        })),
    ) {
        Ok(_body) => stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.move.entity.moved.entity.from.to",
            [
                ("entity_name", &(entity_name)),
                ("from_facet", &(from_facet)),
                ("to_facet", &(to_facet)),
            ],
        )),
        Err(error) => entity_error(locale, error, Some(&entity_name), None),
    }
}

#[must_use]
pub fn detect(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(detect_output(ctx))
}

fn detect_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--facet", Some("-f")), ("--day", Some("-d"))];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(type_) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("TYPE"))],
        ));
    };
    let Some(entity) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY"))],
        ));
    };
    let Some(description) = parsed.positionals.get(2) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("DESCRIPTION"))],
        ));
    };
    let facet = match resolve_facet(locale, ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let day = match resolve_day(locale, ctx, parsed.value("--day")) {
        Ok(day) => day,
        Err(output) => return output,
    };
    match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        &format!("/app/entities/api/{facet}/detected"),
        vec![],
        Some(json!({
            "day": day,
            "type": type_,
            "entity": entity,
            "description": description,
        })),
    ) {
        Ok(body) => stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.detect.entity.detected.for",
            [
                (
                    "entity_name",
                    &(string_field(&body, "name").unwrap_or_else(|| entity.clone())),
                ),
                ("day", &(day)),
            ],
        )),
        Err(error) => entity_error(locale, error, Some(entity), Some(type_)),
    }
}

#[must_use]
pub fn attach(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--facet", Some("-f"))];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(type_) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("TYPE"))],
        ));
    };
    let Some(entity) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY"))],
        ));
    };
    let Some(description) = parsed.positionals.get(2) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("DESCRIPTION"))],
        ));
    };
    let facet = match resolve_facet(locale, ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        &format!("/app/entities/api/{facet}/attach"),
        vec![],
        Some(json!({"type": type_, "name": entity, "description": description})),
    ) {
        Ok(_body) => stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.attach.entity.attached",
            [("entity", &(entity))],
        )),
        Err(error) if error.reason_code() == Some("entity_already_exists") => {
            stdout_line(display_for(
                locale,
                "journal.cli.sol.entities.attach.entity.already.attached",
                [("entity", &(entity))],
            ))
        }
        Err(error) => entity_error(locale, error, Some(entity), Some(type_)),
    }
}

#[must_use]
pub fn update(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(update_output(ctx))
}

fn update_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--facet", Some("-f")), ("--day", Some("-d"))];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY"))],
        ));
    };
    let Some(description) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("DESCRIPTION"))],
        ));
    };
    let facet = match resolve_facet(locale, ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    if let Some(day) = parsed.value("--day") {
        return match request_json(
            locale,
            ctx,
            HttpMethod::Post,
            &format!("/app/entities/api/{facet}/update-detected"),
            vec![],
            Some(json!({"day": day, "entity": entity, "description": description})),
        ) {
            Ok(_body) => stdout_line(display_for(
                locale,
                "journal.cli.sol.entities.update.entity.updated.for",
                [("entity", &(entity)), ("day", &(day))],
            )),
            Err(error) => entity_error(locale, error, Some(entity), None),
        };
    }
    let resolved = match resolve_entity_or_exit(locale, ctx, &facet, entity) {
        Ok(resolved) => resolved,
        Err(output) => return output,
    };
    let resolved_name = string_field(&resolved, "name").unwrap_or_else(|| entity.clone());
    let entity_id = string_field(&resolved, "id").unwrap_or_default();
    match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        &format!("/app/entities/api/{facet}/update-description"),
        vec![],
        Some(json!({
            "entity_id": entity_id,
            "description": description,
            "entity": entity,
            "name": resolved_name,
        })),
    ) {
        Ok(_body) => stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.update.entity.updated",
            [("resolved_name", &(resolved_name))],
        )),
        Err(error) => entity_error(locale, error, Some(&resolved_name), None),
    }
}

#[must_use]
pub fn aka(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(aka_output(ctx))
}

fn aka_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--facet", Some("-f"))];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY"))],
        ));
    };
    let Some(aka_value) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("AKA"))],
        ));
    };
    let facet = match resolve_facet(locale, ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let resolved = match resolve_entity_or_exit(locale, ctx, &facet, entity) {
        Ok(resolved) => resolved,
        Err(output) => return output,
    };
    let resolved_name = string_field(&resolved, "name").unwrap_or_default();
    let base_name = strip_parenthetical(&resolved_name);
    if let Some(first_word) = base_name.split_whitespace().next()
        && first_word.eq_ignore_ascii_case(aka_value)
    {
        return stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.aka.alias.is.the.first.word",
            [
                ("aka_value", &(aka_value)),
                ("resolved_name", &(resolved_name)),
            ],
        ));
    }
    if array_field(&resolved, "aka")
        .iter()
        .any(|item| item.as_str() == Some(aka_value))
    {
        return stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.aka.alias.already.exists.for",
            [
                ("aka_value", &(aka_value)),
                ("resolved_name", &(resolved_name)),
            ],
        ));
    }
    let entity_id = string_field(&resolved, "id").unwrap_or_default();
    match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        &format!("/app/entities/api/{facet}/aka"),
        vec![],
        Some(json!({
            "entity_id": entity_id,
            "aka": aka_value,
            "exclude_name": resolved_name,
            "entity": entity,
        })),
    ) {
        Ok(_body) => stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.aka.added.alias.to",
            [
                ("aka_value", &(aka_value)),
                ("resolved_name", &(resolved_name)),
            ],
        )),
        Err(error) => entity_error(locale, error, Some(&resolved_name), None),
    }
}

#[must_use]
pub fn record_merge_candidate(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(record_merge_candidate_output(ctx))
}

fn record_merge_candidate_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[
        ("--facet", Some("-f")),
        ("--day", Some("-d")),
        ("--evidence", None),
        ("--basis", None),
        ("--detections", None),
        ("--needs", None),
    ];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SOURCE"))],
        ));
    };
    let Some(target) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("TARGET"))],
        ));
    };
    let Some(evidence) = parsed.value("--evidence") else {
        return stderr(display_for(
            locale,
            "journal.cli.client.option.required",
            [("option", &("--evidence"))],
        ));
    };
    let facet = match resolve_facet(locale, ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let day = match resolve_day(locale, ctx, parsed.value("--day")) {
        Ok(day) => day,
        Err(output) => return output,
    };
    let body = json!({
        "facet": facet,
        "day": day,
        "source": source,
        "target": target,
        "evidence": evidence,
        "basis": parsed.value("--basis").unwrap_or("name-variant"),
        "detections": parsed.value("--detections").map(number_value).unwrap_or(Value::Null),
        "needs": parsed.value("--needs").map(number_value).unwrap_or(Value::Null),
    });
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/entities/api/record-merge-candidate",
        vec![],
        Some(body),
    ) {
        Ok(result) => result,
        Err(error) => return entity_error(locale, error, None, None),
    };
    let Some(row) = result.get("row") else {
        return stderr(render_for(
            locale,
            "journal.cli.client.error.malformed",
            &[],
        ));
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(row);
    }
    if truthy(result.get("created")) {
        return stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.record.merge.candidate.merge.candidate.recorded",
            [("source", &(source)), ("target", &(target))],
        ));
    }
    stdout_line(display_for(
        locale,
        "journal.cli.sol.entities.record.merge.candidate.merge.candidate.updated.status",
        [
            ("source", &(source)),
            ("target", &(target)),
            ("status", &(display_value(row.get("status")))),
        ],
    ))
}

#[must_use]
pub fn merge_candidates(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--facet", Some("-f")), ("--status", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        "/app/entities/api/merge-candidates",
        params_from_optional(&[
            ("facet", parsed.value("--facet")),
            ("status", parsed.value("--status")),
        ]),
        None,
    ) {
        Ok(body) => body,
        Err(error) => return entity_error(locale, error, None, None),
    };
    let rows = array_field(&body, "items");
    if parsed.bool_value("--json").unwrap_or(false) {
        return crate::agent_output::literal_output(stdout_json(&Value::Array(rows)));
    }
    if rows.is_empty() {
        return stdout_line(render_for(
            locale,
            "journal.cli.sol.entities.merge.candidates.no.merge.candidates.found",
            &[],
        ));
    }
    let mut lines = Vec::new();
    for row in rows {
        let evidence = row.get("evidence").and_then(Value::as_object);
        lines.push(display_for(
            locale,
            "journal.cli.sol.entities.merge.candidates.facet.detections.needs.last",
            [
                (
                    "source",
                    &(crate::agent_output::literal_links(&value_or_default(row.get("source"), ""))),
                ),
                (
                    "target",
                    &(crate::agent_output::literal_links(&value_or_default(row.get("target"), ""))),
                ),
                ("status", &(value_or_default(row.get("status"), ""))),
                ("facet", &(value_or_default(row.get("facet"), ""))),
                (
                    "detections",
                    &(display_value(evidence.and_then(|item| item.get("detection_count")))),
                ),
                (
                    "needs",
                    &(display_value(evidence.and_then(|item| item.get("needs")))),
                ),
                ("last", &(value_or_default(row.get("last_surfaced"), ""))),
            ],
        ));
    }
    stdout(lines)
}

#[must_use]
pub fn accept_merge_candidate(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(accept_merge_candidate_output(ctx))
}

fn accept_merge_candidate_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--facet", Some("-f"))];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[("--commit", Some("--no-commit")), ("--yes", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source_slug) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SOURCE_SLUG"))],
        ));
    };
    let Some(target_slug) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("TARGET_SLUG"))],
        ));
    };
    let facet = match resolve_facet(locale, ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let commit = parsed.bool_value("--commit").unwrap_or(false);
    if commit && !parsed.bool_value("--yes").unwrap_or(false) {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.merge.approval",
            &[],
        ));
    }
    let result = match request_json_with_policy(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/entities/api/accept-merge-candidate",
        vec![],
        Some(json!({
            "facet": facet,
            "source_slug": source_slug,
            "target_slug": target_slug,
            "commit": commit,
        })),
        if commit {
            TimeoutPolicy::EntityMutation
        } else {
            TimeoutPolicy::Api
        },
    ) {
        Ok(result) => result,
        Err(error) => return entity_error(locale, error, None, None),
    };
    render_accept_merge_candidate(locale, &result, source_slug, target_slug)
}

#[must_use]
pub fn dismiss_merge_candidate(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--facet", Some("-f"))];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source_slug) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SOURCE_SLUG"))],
        ));
    };
    let Some(target_slug) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("TARGET_SLUG"))],
        ));
    };
    let facet = match resolve_facet(locale, ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let result = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/entities/api/dismiss-merge-candidate",
        vec![],
        Some(json!({"facet": facet, "source_slug": source_slug, "target_slug": target_slug})),
    ) {
        Ok(result) => result,
        Err(error) => return entity_error(locale, error, None, None),
    };
    match result.get("status").and_then(Value::as_str) {
        Some("error") => merge_candidate_error(locale, &result),
        Some("dismissed") => stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.dismiss.merge.candidate.dismissed.merge.candidate",
            [
                ("source_slug", &(source_slug)),
                ("target_slug", &(target_slug)),
            ],
        )),
        Some("already_dismissed") => stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.dismiss.merge.candidate.merge.candidate.already.dismissed",
            [
                ("source_slug", &(source_slug)),
                ("target_slug", &(target_slug)),
            ],
        )),
        status => stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.dismiss.merge.candidate.dismiss.result.for",
            [
                ("source_slug", &(source_slug)),
                ("target_slug", &(target_slug)),
                ("status", &(status.unwrap_or("None"))),
            ],
        )),
    }
}

#[must_use]
pub fn merge(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = solstone_core_cli_copy::ENGLISH;
    let parsed = match parse_args(
        locale,
        ctx.args,
        &[],
        &[
            ("--commit", Some("--no-commit")),
            ("--keep-source-as-aka", Some("--no-keep-source-as-aka")),
            ("--yes", None),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source_slug) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("SOURCE_SLUG"))],
        ));
    };
    let Some(target_slug) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("TARGET_SLUG"))],
        ));
    };
    if parsed.bool_value("--commit").unwrap_or(false)
        && !parsed.bool_value("--yes").unwrap_or(false)
    {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.merge.approval",
            &[],
        ));
    }
    let result = match request_json_with_policy(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/entities/api/merge",
        vec![],
        Some(json!({
            "source_slug": source_slug,
            "target_slug": target_slug,
            "commit": parsed.bool_value("--commit").unwrap_or(false),
            "keep_source_as_aka": parsed.bool_value("--keep-source-as-aka").unwrap_or(true),
        })),
        if parsed.bool_value("--commit").unwrap_or(false) {
            TimeoutPolicy::EntityMutation
        } else {
            TimeoutPolicy::Api
        },
    ) {
        Ok(result) => result,
        Err(error) => return merge_json_error(error),
    };
    let output = format!("{}\n", json_pretty_ascii(&result));
    crate::agent_output::literal_output(if result.get("error").is_some() {
        CommandOutput {
            stdout: String::new(),
            stderr: output,
            exit: 1,
        }
    } else {
        CommandOutput::success(output)
    })
}

#[must_use]
pub fn ambiguities(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--status", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    if let Some(status) = parsed.value("--status")
        && !matches!(status, "open" | "resolved")
    {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.ambiguities.error.status.must.be.open",
            &[],
        ));
    }
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        "/app/entities/api/ambiguities",
        params_from_optional(&[("status", parsed.value("--status"))]),
        None,
    ) {
        Ok(body) => body,
        Err(error) => {
            return trust_error(locale, error, parsed.bool_value("--json").unwrap_or(false));
        }
    };
    let rows = array_field(&body, "items");
    if parsed.bool_value("--json").unwrap_or(false) {
        return crate::agent_output::literal_output(stdout_json(&body));
    }
    render_ambiguities(locale, &rows)
}

#[must_use]
pub fn resolve_ambiguity(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(resolve_ambiguity_output(ctx))
}

fn resolve_ambiguity_output(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[("--yes", None), ("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(ambiguity_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("AMBIGUITY_ID"))],
        ));
    };
    let Some(entity_id) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY_ID"))],
        ));
    };
    if !parsed.bool_value("--yes").unwrap_or(false) {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.resolve.ambiguity.refusing.to.resolve.this.ambiguity",
            &[],
        ));
    }
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        &format!("/app/entities/api/ambiguities/{ambiguity_id}/resolve"),
        vec![],
        Some(json!({"entity_id": entity_id})),
    ) {
        Ok(body) => body,
        Err(error) => {
            return trust_error(locale, error, parsed.bool_value("--json").unwrap_or(false));
        }
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    let resolved_at = body
        .get("ambiguity")
        .and_then(|ambiguity| ambiguity.get("resolved_at"))
        .map(|value| display_value(Some(value)))
        .unwrap_or_else(|| "None".to_string());
    stdout_line(display_for(
        locale,
        "journal.cli.sol.entities.resolve.ambiguity.resolved.to.at",
        [
            ("ambiguity_id", &(ambiguity_id)),
            ("entity_id", &(entity_id)),
            ("resolved_at", &(resolved_at)),
        ],
    ))
}

#[must_use]
pub fn resolve_ambiguity_group(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(resolve_ambiguity_group_output(ctx))
}

fn resolve_ambiguity_group_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--member-id", None), ("--revision", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[("--yes", None), ("--json", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY_ID"))],
        ));
    };
    let member_ids = parsed.values("--member-id");
    if member_ids.is_empty() {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.resolve.ambiguity.group.error.missing.required.option.member",
            &[],
        ));
    }
    let Some(revision) = parsed.value("--revision") else {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.resolve.ambiguity.group.error.missing.required.option.revision",
            &[],
        ));
    };
    if !parsed.bool_value("--yes").unwrap_or(false) {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.resolve.ambiguity.group.refusing.to.resolve.this.ambiguity",
            &[],
        ));
    }
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/entities/api/ambiguities/group-resolve",
        vec![],
        Some(json!({
            "entity_id": entity_id,
            "member_ids": member_ids,
            "revision": revision,
        })),
    ) {
        Ok(body) => body,
        Err(error) => {
            return trust_error(locale, error, parsed.bool_value("--json").unwrap_or(false));
        }
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    stdout_line({
        let display_count = (member_ids.len()).to_string();
        let display_entity_id = (entity_id).to_string();
        render_for(
            locale,
            "journal.cli.sol.entities.resolve.ambiguity.group.resolved.ambiguities.to",
            &[
                ("count", CopyValue::String(&display_count)),
                ("entity_id", CopyValue::String(&display_entity_id)),
                (
                    "quantity",
                    crate::command::count_selector(display_count.parse().unwrap_or(0)),
                ),
            ],
        )
    })
}

#[must_use]
pub fn restore_review(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(restore_review_output(ctx))
}

fn restore_review_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[
        ("--ambiguity-id", None),
        ("--facet", Some("-f")),
        ("--source-slug", None),
        ("--target-slug", None),
    ];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[("--yes", None), ("--json", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(target) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("TARGET"))],
        ));
    };
    if !parsed.bool_value("--yes").unwrap_or(false) {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.restore.review.refusing.to.restore.review.item",
            &[],
        ));
    }

    let payload = match target.as_str() {
        "ambiguity" => {
            let Some(ambiguity_id) = parsed.value("--ambiguity-id") else {
                return stderr(render_for(
                    locale,
                    "journal.cli.sol.entities.restore.review.error.missing.required.option.ambiguity",
                    &[],
                ));
            };
            json!({
                "target": "ambiguity",
                "ambiguity_id": ambiguity_id,
            })
        }
        "merge_candidate" => {
            let Some(facet) = parsed.value("--facet") else {
                return stderr(render_for(
                    locale,
                    "journal.cli.sol.entities.restore.review.error.missing.required.option.facet",
                    &[],
                ));
            };
            let Some(source_slug) = parsed.value("--source-slug") else {
                return stderr(render_for(
                    locale,
                    "journal.cli.sol.entities.restore.review.error.missing.required.option.source",
                    &[],
                ));
            };
            let Some(target_slug) = parsed.value("--target-slug") else {
                return stderr(render_for(
                    locale,
                    "journal.cli.sol.entities.restore.review.error.missing.required.option.target",
                    &[],
                ));
            };
            json!({
                "target": "merge_candidate",
                "facet": facet,
                "source_slug": source_slug,
                "target_slug": target_slug,
            })
        }
        _ => {
            return stderr(render_for(
                locale,
                "journal.cli.sol.entities.restore.review.error.target.must.be.ambiguity",
                &[],
            ));
        }
    };

    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/entities/api/review/restore",
        vec![],
        Some(payload),
    ) {
        Ok(body) => body,
        Err(error) => {
            return trust_error(locale, error, parsed.bool_value("--json").unwrap_or(false));
        }
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    stdout_line(display_for(
        locale,
        "journal.cli.sol.entities.restore.review.restored.review.item",
        [("target", &(target))],
    ))
}

#[must_use]
pub fn review(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(review_output(ctx))
}

fn review_output(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        "/app/entities/api/review",
        vec![],
        None,
    ) {
        Ok(body) => body,
        Err(error) => {
            return trust_error(locale, error, parsed.bool_value("--json").unwrap_or(false));
        }
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    let cards = array_field(&body, "ambiguity_cards");
    let set_aside_amb = array_field(&body, "set_aside_ambiguities");
    let set_aside_merge = array_field(&body, "set_aside_merge_candidates");
    let mut lines = Vec::new();
    lines.push({
        let display_cards = (cards.len()).to_string();
        let display_ambiguities = (set_aside_amb.len()).to_string();
        let display_candidates = (set_aside_merge.len()).to_string();
        render_for(
            locale,
            "journal.cli.sol.entities.review.review.items.ambiguity.card.s",
            &[
                ("cards", CopyValue::String(&display_cards)),
                ("ambiguities", CopyValue::String(&display_ambiguities)),
                ("candidates", CopyValue::String(&display_candidates)),
                (
                    "quantity",
                    crate::command::count_selector(display_cards.parse().unwrap_or(0)),
                ),
                (
                    "ambiguities_quantity",
                    crate::command::count_selector(display_ambiguities.parse().unwrap_or(0)),
                ),
                (
                    "candidates_quantity",
                    crate::command::count_selector(display_candidates.parse().unwrap_or(0)),
                ),
            ],
        )
    });
    stdout(lines)
}

#[must_use]
pub fn sweep_review(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[("--yes", None), ("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    if !parsed.bool_value("--yes").unwrap_or(false) {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.sweep.review.refusing.to.run.review.sweep",
            &[],
        ));
    }
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        "/app/entities/api/review/sweep",
        vec![],
        Some(json!({})),
    ) {
        Ok(body) => body,
        Err(error) => {
            return trust_error(locale, error, parsed.bool_value("--json").unwrap_or(false));
        }
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    stdout_line(render_for(
        locale,
        "journal.cli.sol.entities.sweep.review.completed.entity.review.sweep",
        &[],
    ))
}

#[must_use]
pub fn delete_entity(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(delete_entity_output(ctx))
}

fn delete_entity_output(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(
        locale,
        ctx.args,
        &[],
        &[
            ("--yes", None),
            ("--erase", None),
            ("--from-backups", None),
            ("--not-from-backups", None),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY_ID"))],
        ));
    };
    if !parsed.bool_value("--yes").unwrap_or(false) {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.delete.approval",
            &[],
        ));
    }
    let erase = parsed.bool_value("--erase").unwrap_or(false);
    let backups = if erase {
        match crate::command::backups_choice_for(
            locale,
            ctx,
            parsed.bool_value("--from-backups").unwrap_or(false),
            parsed.bool_value("--not-from-backups").unwrap_or(false),
        ) {
            Ok(choice) => choice,
            Err(error) => return stderr(error),
        }
    } else {
        crate::command::BackupsChoice::default()
    };
    // The set route takes a body on POST, so one entity goes as a set of one.
    let body = match request_json_with_policy(
        locale,
        ctx,
        HttpMethod::Post,
        &format!("{ENTITY_HISTORY_BASE_ROUTE}/delete"),
        vec![],
        Some(if erase {
            json!({"ids": [entity_id], "erase": true, "backups": backups.chosen})
        } else {
            json!({"ids": [entity_id], "erase": false})
        }),
        TimeoutPolicy::EntityMutation,
    ) {
        Ok(body) => body,
        Err(error) => return entity_error(locale, error, Some(entity_id), None),
    };
    if let Some(refused) = array_field(&body, "failed").first() {
        return stderr(match string_field(refused, "error") {
            Some(reason) => display_for(
                locale,
                "journal.cli.sol.entities.delete.entity.error.wasn.t.deleted",
                [("entity_id", &(entity_id)), ("reason", &(reason))],
            ),
            None => display_for(
                locale,
                "journal.cli.sol.entities.delete.failed.without.reason.error.wasn.t.deleted.it",
                [("entity_id", &(entity_id))],
            ),
        });
    }
    let Some(result) = array_field(&body, "done")
        .first()
        .and_then(|done| done.get("result"))
        .cloned()
    else {
        return stderr(render_for(
            locale,
            "journal.cli.client.error.malformed",
            &[],
        ));
    };
    if truthy(result.get("erased")) {
        return stdout_line(
            match crate::command::backups_next_line_for(locale, backups) {
                Some(next) => format!(
                    "{} {next}",
                    display_for(
                        locale,
                        "journal.cli.sol.entities.delete.entity.erased",
                        [("entity_id", &(entity_id))]
                    )
                ),
                None => display_for(
                    locale,
                    "journal.cli.sol.entities.delete.entity.erased",
                    [("entity_id", &(entity_id))],
                ),
            },
        );
    }
    stdout_line(crate::command::moved_to_trash_line_for(
        locale,
        entity_id,
        truthy(result.get("erase_failed")),
        crate::command::erase_day_of_for(locale, ctx, &result).as_deref(),
    ))
}

#[must_use]
pub fn entity_history(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(entity_history_output(ctx))
}

fn entity_history_output(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY_ID"))],
        ));
    };
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        &format!("{ENTITY_HISTORY_BASE_ROUTE}/entity/{entity_id}/history"),
        vec![],
        None,
    ) {
        Ok(body) => body,
        Err(error) => {
            return trust_error(locale, error, parsed.bool_value("--json").unwrap_or(false));
        }
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    render_entity_versions(locale, &body)
}

#[must_use]
pub fn restore_version(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(restore_version_output(ctx))
}

fn restore_version_output(ctx: CommandContext<'_>) -> CommandOutput {
    let locale = ctx.human_locale();
    let parsed = match parse_args(locale, ctx.args, &[], &[("--yes", None), ("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity_id) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY_ID"))],
        ));
    };
    let Some(version_id) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("VERSION_ID"))],
        ));
    };
    if !parsed.bool_value("--yes").unwrap_or(false) {
        return stderr(render_for(
            locale,
            "journal.cli.sol.entities.restore.version.refusing.to.restore.this.identity",
            &[],
        ));
    }
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        &format!("{ENTITY_HISTORY_BASE_ROUTE}/entity/{entity_id}/restore"),
        vec![],
        Some(json!({"version_id": version_id})),
    ) {
        Ok(body) => body,
        Err(error) => {
            return trust_error(locale, error, parsed.bool_value("--json").unwrap_or(false));
        }
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    let version = body
        .get("event")
        .and_then(|event| event.get("version_id"))
        .map(|value| display_value(Some(value)))
        .unwrap_or_else(|| "None".to_string());
    stdout_line(display_for(
        locale,
        "journal.cli.sol.entities.restore.version.restored.from.new.history.version",
        [
            ("entity_id", &(entity_id)),
            ("version_id", &(version_id)),
            ("version", &(version)),
        ],
    ))
}

#[must_use]
pub fn network(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[
        ("--kinds", None),
        ("--facet", Some("-f")),
        ("--day-from", None),
        ("--day-to", None),
        ("--limit", Some("-n")),
        ("--evidence-limit", None),
    ];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[("--include-principal", None), ("--json", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY"))],
        ));
    };
    let mut params = edge_filter_params(&parsed);
    params.push(QueryParam::single("entity", entity));
    params.push(QueryParam::single(
        "limit",
        parsed.value("--limit").unwrap_or("25"),
    ));
    params.push(QueryParam::single(
        "evidence_limit",
        parsed.value("--evidence-limit").unwrap_or("5"),
    ));
    params.push(QueryParam::single(
        "include_principal",
        if parsed.bool_value("--include-principal").unwrap_or(false) {
            "True"
        } else {
            "False"
        },
    ));
    let body = match edge_body_or_exit(
        locale,
        ctx,
        "/app/entities/api/network",
        params,
        parsed.bool_value("--json").unwrap_or(false),
    ) {
        Ok(body) => body,
        Err(output) => return output,
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return crate::agent_output::literal_output(stdout_json(&body));
    }
    render_network(locale, &crate::agent_output::literal_body(body))
}

#[must_use]
pub fn history(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[
        ("--kinds", None),
        ("--facet", Some("-f")),
        ("--day-from", None),
        ("--day-to", None),
        ("--limit", Some("-n")),
        ("--offset", None),
    ];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY"))],
        ));
    };
    let mut params = edge_filter_params(&parsed);
    params.push(QueryParam::single("entity", entity));
    push_param(
        &mut params,
        "peer",
        parsed.positionals.get(1).map(String::as_str),
    );
    params.push(QueryParam::single(
        "limit",
        parsed.value("--limit").unwrap_or("50"),
    ));
    params.push(QueryParam::single(
        "offset",
        parsed.value("--offset").unwrap_or("0"),
    ));
    let body = match edge_body_or_exit(
        locale,
        ctx,
        "/app/entities/api/history",
        params,
        parsed.bool_value("--json").unwrap_or(false),
    ) {
        Ok(body) => body,
        Err(output) => return output,
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return crate::agent_output::literal_output(stdout_json(&body));
    }
    render_history(locale, &crate::agent_output::literal_body(body))
}

#[must_use]
pub fn overview(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[
        ("--kinds", None),
        ("--facet", Some("-f")),
        ("--day-from", None),
        ("--day-to", None),
        ("--limit", Some("-n")),
    ];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let mut params = edge_filter_params(&parsed);
    params.push(QueryParam::single(
        "limit",
        parsed.value("--limit").unwrap_or("25"),
    ));
    let body = match edge_body_or_exit(
        locale,
        ctx,
        "/app/entities/api/overview",
        params,
        parsed.bool_value("--json").unwrap_or(false),
    ) {
        Ok(body) => body,
        Err(output) => return output,
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return crate::agent_output::literal_output(stdout_json(&body));
    }
    render_overview(locale, &crate::agent_output::literal_body(body))
}

#[must_use]
pub fn observations(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(observations_output(ctx))
}

fn observations_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--facet", Some("-f")), ("--limit", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(
        locale,
        ctx.args,
        value_options,
        &[("--oldest", None), ("--all", None), ("--json", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY"))],
        ));
    };
    let facet = match resolve_facet(locale, ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let resolved = match resolve_entity_or_exit(locale, ctx, &facet, entity) {
        Ok(resolved) => resolved,
        Err(output) => return output,
    };
    let resolved_name = string_field(&resolved, "name").unwrap_or_default();
    let oldest = parsed.bool_value("--oldest").unwrap_or(false);
    let all = parsed.bool_value("--all").unwrap_or(false);
    let json_output = parsed.bool_value("--json").unwrap_or(false);

    let (total, items) = if all {
        // Walk by id cursor: stable under an update (id unchanged), an append
        // (higher id) and a retirement (nothing shifts), so no row that stays
        // live is skipped and none is repeated. Presented in the requested
        // time order afterwards.
        let mut all_items: Vec<Value> = Vec::new();
        let mut seen: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        let mut after_id: u64 = 0;
        let mut total;
        loop {
            let query = vec![
                QueryParam::single("name", &resolved_name),
                QueryParam::single("limit", "200"),
                QueryParam::single("after_id", after_id.to_string()),
            ];
            let body = match request_json(
                locale,
                ctx,
                HttpMethod::Get,
                &format!("/app/entities/api/{facet}/observations"),
                query,
                None,
            ) {
                Ok(body) => body,
                Err(error) => return observation_error(locale, error, &resolved_name),
            };
            total = integer_field(&body, "total").unwrap_or(0);
            let items = array_field(&body, "items");
            let has_more = body
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut last_id = None;
            for item in items {
                let id = item.get("id").and_then(Value::as_u64);
                if let Some(id) = id {
                    last_id = Some(id);
                    if !seen.insert(id) {
                        continue;
                    }
                }
                all_items.push(item);
            }
            match last_id {
                Some(id) if has_more && id > after_id => after_id = id,
                _ => break,
            }
        }
        let key = |item: &Value| {
            (
                item.get("observed_at").and_then(Value::as_i64).unwrap_or(0),
                item.get("id").and_then(Value::as_u64).unwrap_or(0),
            )
        };
        if oldest {
            all_items.sort_by_key(|item| key(item));
        } else {
            all_items.sort_by_key(|item| std::cmp::Reverse(key(item)));
        }
        (total, all_items)
    } else {
        let limit_str = parsed.value("--limit").unwrap_or("50");
        let mut query = vec![
            QueryParam::single("name", &resolved_name),
            QueryParam::single("limit", limit_str),
        ];
        if oldest {
            query.push(QueryParam::single("order", "oldest"));
        }
        let body = match request_json(
            locale,
            ctx,
            HttpMethod::Get,
            &format!("/app/entities/api/{facet}/observations"),
            query,
            None,
        ) {
            Ok(body) => body,
            Err(error) => return observation_error(locale, error, &resolved_name),
        };
        let total = integer_field(&body, "total").unwrap_or(0);
        let items = array_field(&body, "items");
        (total, items)
    };

    if json_output {
        return stdout_line(json_pretty_ascii(&json!({
            "total": total,
            "items": items,
        })));
    }

    if items.is_empty() {
        return stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.observations.nothing.with.yet",
            [("resolved_name", &(resolved_name))],
        ));
    }

    let mut lines = vec![if oldest {
        {
            let display_total = (total).to_string();
            let display_name = (resolved_name).to_string();
            let display_facet = (facet).to_string();
            render_for(
                locale,
                "journal.cli.sol.entities.observations.oldest.moments.for.in.oldest.first",
                &[
                    ("total", CopyValue::String(&display_total)),
                    ("name", CopyValue::String(&display_name)),
                    ("facet", CopyValue::String(&display_facet)),
                    (
                        "quantity",
                        crate::command::count_selector(display_total.parse().unwrap_or(0)),
                    ),
                ],
            )
        }
    } else {
        {
            let display_total = (total).to_string();
            let display_name = (resolved_name).to_string();
            let display_facet = (facet).to_string();
            render_for(
                locale,
                "journal.cli.sol.entities.observations.newest.moments.for.in.newest.first",
                &[
                    ("total", CopyValue::String(&display_total)),
                    ("name", CopyValue::String(&display_name)),
                    ("facet", CopyValue::String(&display_facet)),
                    (
                        "quantity",
                        crate::command::count_selector(display_total.parse().unwrap_or(0)),
                    ),
                ],
            )
        }
    }];
    for observation in &items {
        let id = observation
            .get("id")
            .and_then(Value::as_u64)
            .map(|id| id.to_string())
            .unwrap_or_default();
        lines.push(format!(
            "  #{}  {}",
            id,
            value_or_default(observation.get("content"), "")
        ));
    }
    if items.len() < total {
        lines.push(if oldest { display_for(locale, "journal.cli.sol.entities.observations.partial.oldest.showing.the.oldest.of.moments", [("shown", &(items.len())), ("total", &(total)), ("name", &(resolved_name))]) } else { display_for(locale, "journal.cli.sol.entities.observations.partial.most.recent.showing.the.most.recent.of", [("shown", &(items.len())), ("total", &(total)), ("name", &(resolved_name))]) });
    }
    stdout(lines)
}

#[must_use]
pub fn observe(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(observe_output(ctx))
}

fn observe_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[("--facet", Some("-f")), ("--source-day", None)];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("ENTITY"))],
        ));
    };
    let Some(content) = parsed.positionals.get(1) else {
        return stderr(display_for(
            locale,
            "journal.cli.client.missing.argument",
            [("argument", &("CONTENT"))],
        ));
    };
    let facet = match resolve_facet(locale, ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let resolved = match resolve_entity_or_exit(locale, ctx, &facet, entity) {
        Ok(resolved) => resolved,
        Err(output) => return output,
    };
    let resolved_name = string_field(&resolved, "name").unwrap_or_default();
    match request_json(
        locale,
        ctx,
        HttpMethod::Post,
        &format!("/app/entities/api/{facet}/observe"),
        vec![],
        Some(json!({
            "name": resolved_name,
            "content": content,
            "source_day": parsed.value("--source-day").map(Value::from).unwrap_or(Value::Null),
            "entity": entity,
        })),
    ) {
        Ok(body) => {
            let result = body.get("result");
            let already_present = result
                .and_then(|r| r.get("already_present"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if already_present {
                stdout_line(display_for(
                    locale,
                    "journal.cli.sol.entities.observe.your.journal.already.holds.that",
                    [("resolved_name", &(resolved_name))],
                ))
            } else {
                let count = result
                    .and_then(|r| r.get("count"))
                    .and_then(Value::as_u64)
                    .unwrap_or(1);
                stdout_line({
                    let display_count = (count).to_string();
                    let display_name = (resolved_name).to_string();
                    let display_facet = (facet).to_string();
                    render_for(
                        locale,
                        "journal.cli.sol.entities.observe.added.added.moments.for.in",
                        &[
                            ("count", CopyValue::String(&display_count)),
                            ("name", CopyValue::String(&display_name)),
                            ("facet", CopyValue::String(&display_facet)),
                            (
                                "quantity",
                                crate::command::count_selector(display_count.parse().unwrap_or(0)),
                            ),
                        ],
                    )
                })
            }
        }
        Err(error) => observation_error(locale, error, &resolved_name),
    }
}

fn observation_error(locale: &str, error: ClientError, name: &str) -> CommandOutput {
    if let Some(detail) = error.detail()
        && detail.contains("line ")
        && let Some(line_str) = detail
            .split("line ")
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .filter(|s| !s.is_empty())
    {
        return stderr(display_for(
            locale,
            "journal.cli.sol.entities.observation.error.couldn.t.read.the.moments",
            [("name", &(name)), ("line_str", &(line_str))],
        ));
    }
    entity_error(locale, error, Some(name), None)
}

#[must_use]
pub fn search(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(search_output(ctx))
}

fn search_output(ctx: CommandContext<'_>) -> CommandOutput {
    let value_options = &[
        ("--query", Some("-q")),
        ("--type", Some("-t")),
        ("--facet", Some("-f")),
        ("--since", None),
        ("--limit", Some("-n")),
    ];
    let locale = locale_for_options(ctx, value_options);
    let parsed = match parse_args(locale, ctx.args, value_options, &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let query = parsed
        .positionals
        .first()
        .map(String::as_str)
        .or_else(|| parsed.value("--query"));
    let mut params = params_from_optional(&[
        ("query", query),
        ("type", parsed.value("--type")),
        ("facet", parsed.value("--facet")),
        ("since", parsed.value("--since")),
    ]);
    params.push(QueryParam::single(
        "limit",
        parsed.value("--limit").unwrap_or("20"),
    ));
    let body = match request_json(
        locale,
        ctx,
        HttpMethod::Get,
        "/app/entities/api/search",
        params,
        None,
    ) {
        Ok(body) => body,
        Err(error) => return entity_error(locale, error, None, None),
    };
    let results = array_field(&body, "items");
    if results.is_empty() {
        return stdout_line(render_for(
            locale,
            "journal.cli.sol.entities.list.no.entities.found",
            &[],
        ));
    }
    let mut lines = vec![{
        let display_count = (results.len()).to_string();
        render_for(
            locale,
            "journal.cli.sol.entities.search.entities",
            &[
                ("count", CopyValue::String(&display_count)),
                (
                    "quantity",
                    crate::command::count_selector(display_count.parse().unwrap_or(0)),
                ),
            ],
        )
    }];
    for entity in results {
        lines.push(format!(
            "  - {} ({}): {}",
            display_value(entity.get("name")),
            display_value(entity.get("type")),
            display_value(entity.get("description"))
        ));
        let facets = array_field(&entity, "facets")
            .iter()
            .map(|item| display_value(Some(item)))
            .collect::<Vec<_>>()
            .join(", ");
        if !facets.is_empty() {
            lines.push(display_for(
                locale,
                "journal.cli.sol.entities.search.facets",
                [("facets", &(facets))],
            ));
        }
    }
    stdout(lines)
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

    fn values(&self, name: &str) -> Vec<&str> {
        self.values
            .iter()
            .filter(|(key, _value)| key == name)
            .map(|(_key, value)| value.as_str())
            .collect()
    }

    fn bool_value(&self, name: &str) -> Option<bool> {
        self.bools
            .iter()
            .rev()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| *value)
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
    options: &[(&str, Option<&str>)],
    bool_options: &[(&str, Option<&str>)],
) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if let Some((name, value)) = token.split_once('=')
            && let Some(canonical) = canonical_option(name, options)
        {
            parsed
                .values
                .push((canonical.to_string(), value.to_string()));
        } else if let Some(canonical) = canonical_option(token, options) {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(display_for(
                    locale,
                    "journal.cli.client.option.value",
                    [("option", &(token))],
                ));
            };
            parsed.values.push((canonical.to_string(), value.clone()));
        } else if let Some((canonical, value)) = bool_option_value(token, bool_options) {
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

fn canonical_option<'a>(token: &str, options: &'a [(&'a str, Option<&'a str>)]) -> Option<&'a str> {
    options.iter().find_map(|(long, short)| {
        if token == *long || short.is_some_and(|short| token == short) {
            Some(*long)
        } else {
            None
        }
    })
}

fn bool_option_value<'a>(
    token: &str,
    options: &'a [(&'a str, Option<&'a str>)],
) -> Option<(&'a str, bool)> {
    options.iter().find_map(|(positive, negative)| {
        if token == *positive {
            Some((*positive, true))
        } else if negative.is_some_and(|negative| token == negative) {
            Some((*positive, false))
        } else {
            None
        }
    })
}

fn request_json(
    locale: &str,
    ctx: CommandContext<'_>,
    method: HttpMethod,
    path: &str,
    params: Vec<QueryParam>,
    json: Option<Value>,
) -> Result<Value, ClientError> {
    request_json_with_policy(locale, ctx, method, path, params, json, TimeoutPolicy::Api)
}

fn request_json_with_policy(
    locale: &str,
    ctx: CommandContext<'_>,
    method: HttpMethod,
    path: &str,
    params: Vec<QueryParam>,
    json: Option<Value>,
    policy: TimeoutPolicy,
) -> Result<Value, ClientError> {
    let response = ctx.transport.request(ApiRequest {
        method,
        path: path.to_string(),
        params,
        json,
        headers: crate::transport::locale_headers(locale),
        policy,
    })?;
    decode_response(&response)
}

fn resolve_request(
    locale: &str,
    ctx: CommandContext<'_>,
    facet: &str,
    entity: &str,
) -> Result<Value, CommandOutput> {
    request_json(
        locale,
        ctx,
        HttpMethod::Get,
        &format!("/app/entities/api/{facet}/resolve"),
        vec![QueryParam::single("name", entity)],
        None,
    )
    .map_err(|error| entity_error(locale, error, Some(entity), None))
    .and_then(|body| {
        if body.is_object() {
            Ok(body)
        } else {
            Err(stderr(render_for(
                locale,
                "journal.cli.client.error.malformed",
                &[],
            )))
        }
    })
}

fn resolve_entity_or_exit(
    locale: &str,
    ctx: CommandContext<'_>,
    facet: &str,
    entity: &str,
) -> Result<Value, CommandOutput> {
    let body = resolve_request(locale, ctx, facet, entity)?;
    resolved_from_body_or_exit(locale, facet, entity, &body)
}

fn resolved_from_body_or_exit(
    locale: &str,
    facet: &str,
    entity: &str,
    body: &Value,
) -> Result<Value, CommandOutput> {
    if let Some(resolved) = body.get("resolved").filter(|value| value.is_object()) {
        return Ok(resolved.clone());
    }
    Err(render_resolve_error(locale, facet, entity, body))
}

fn render_resolve_error(locale: &str, facet: &str, entity: &str, body: &Value) -> CommandOutput {
    if truthy(body.get("blocked")) {
        let blocked_name = string_field(body, "blocked_name").unwrap_or_else(|| entity.to_string());
        return stderr(display_for(
            locale,
            "journal.cli.sol.entities.resolve.error.error.entity.is.hidden",
            [("blocked_name", &(blocked_name))],
        ));
    }
    let candidates = array_field(body, "candidates");
    if !candidates.is_empty() {
        let names = candidates
            .iter()
            .take(3)
            .map(|candidate| value_or_default(candidate.get("name"), ""))
            .collect::<Vec<_>>()
            .join(", ");
        return stderr(display_for(
            locale,
            "journal.cli.sol.entities.resolve.error.error.entity.not.found.did",
            [("entity", &(entity)), ("names", &(names))],
        ));
    }
    stderr(display_for(
        locale,
        "journal.cli.sol.entities.resolve.error.error.entity.not.found.in",
        [("entity", &(entity)), ("facet", &(facet))],
    ))
}

fn edge_body_or_exit(
    locale: &str,
    ctx: CommandContext<'_>,
    path: &str,
    params: Vec<QueryParam>,
    json_output: bool,
) -> Result<Value, CommandOutput> {
    let body = request_json(locale, ctx, HttpMethod::Get, path, params, None)
        .map_err(|error| entity_error(locale, error, None, None))?;
    if !body.is_object() {
        return Err(stderr(render_for(
            locale,
            "journal.cli.client.error.malformed",
            &[],
        )));
    }
    if body.get("resolved").is_some_and(Value::is_null)
        && body.get("query").is_some()
        && body.get("candidates").is_some()
    {
        return Err(if json_output {
            crate::agent_output::literal_output(render_edge_resolution_error(locale, &body, true))
        } else {
            let mut shown = body;
            if let Some(candidates) = shown.get_mut("candidates") {
                *candidates = crate::agent_output::literal_body(candidates.take());
            }
            render_edge_resolution_error(locale, &shown, false)
        });
    }
    Ok(body)
}

fn render_edge_resolution_error(locale: &str, body: &Value, json_output: bool) -> CommandOutput {
    if json_output {
        return CommandOutput {
            stdout: format!("{}\n", json_pretty_utf8(body)),
            stderr: String::new(),
            exit: 1,
        };
    }
    let query = string_field(body, "query");
    let candidates = array_field(body, "candidates");
    if !candidates.is_empty() {
        let labels = candidates
            .iter()
            .take(3)
            .map(candidate_display)
            .collect::<Vec<_>>()
            .join(", ");
        return stderr(match query {
            Some(query) => display_for(
                locale,
                "journal.cli.sol.entities.edge.resolution.error.error.entity.not.found.did",
                [("query", &(query)), ("labels", &(labels))],
            ),
            None => display_for(
                locale,
                "journal.cli.sol.entities.edge.resolution.without.query.suggestions.error.entity.entity.not.found",
                [("labels", &(labels))],
            ),
        });
    }
    stderr(match query {
        Some(query) => display_for(
            locale,
            "journal.cli.sol.entities.edge.resolution.error.error.entity.not.found",
            [("query", &(query))],
        ),
        None => render_for(
            locale,
            "journal.cli.sol.entities.edge.resolution.without.query.empty.error.entity.entity.not.found",
            &[],
        ),
    })
}

fn resolve_facet(
    locale: &str,
    ctx: CommandContext<'_>,
    value: Option<&str>,
) -> Result<String, CommandOutput> {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        return Ok(value.to_string());
    }
    if let Some(value) = ctx.env.get("SOL_FACET").filter(|value| !value.is_empty()) {
        return Ok(value.clone());
    }
    Err(stderr(render_for(
        locale,
        "journal.cli.sol.entities.resolve.facet.error.facet.is.required.pass",
        &[],
    )))
}

fn resolve_day(
    locale: &str,
    ctx: CommandContext<'_>,
    value: Option<&str>,
) -> Result<String, CommandOutput> {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        return Ok(value.to_string());
    }
    if let Some(value) = ctx.env.get("SOL_DAY").filter(|value| !value.is_empty()) {
        return Ok(value.clone());
    }
    Err(stderr(render_for(
        locale,
        "journal.cli.sol.entities.resolve.day.error.day.is.required.pass",
        &[],
    )))
}

fn entity_error(
    locale: &str,
    error: ClientError,
    entity: Option<&str>,
    type_: Option<&str>,
) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(crate::error::service_down_for(locale)),
        _ if error.reason_code() == Some("entity_busy") => {
            stderr(render_for(locale, "journal.cli.sol.entities.busy", &[]))
        }
        _ if error.reason_code() == Some("entity_search_index_unavailable") => stderr(render_for(
            locale,
            "journal.cli.sol.entities.search.index.unavailable",
            &[],
        )),
        _ if error.reason_code() == Some("entity_search_index_busy") => stderr(render_for(
            locale,
            "journal.cli.sol.entities.search.index.busy",
            &[],
        )),
        _ if error.reason_code() == Some("entity_search_index_stale") => stderr(render_for(
            locale,
            "journal.cli.sol.entities.search.index.stale",
            &[],
        )),
        _ if error.reason_code() == Some("entity_search_activity_unavailable") => {
            stderr(render_for(
                locale,
                "journal.cli.sol.entities.search.activity.unreadable",
                &[],
            ))
        }
        _ if error.reason_code() == Some("invalid_entity_type") && type_.is_some() => {
            stderr(display_for(
                locale,
                "journal.cli.sol.entities.entity.error.error.invalid.entity.type",
                [("entity_type", &(type_.unwrap_or("entity")))],
            ))
        }
        _ if error.reason_code() == Some("entity_blocked") => {
            stderr(match error.detail().or(entity) {
                Some(name) => display_for(
                    locale,
                    "journal.cli.sol.entities.entity.error.error.entity.is.hidden",
                    [("name", &(name))],
                ),
                None => render_for(
                    locale,
                    "journal.cli.sol.entities.error.without.name.hidden.error.entity.entity.is.hidden",
                    &[],
                ),
            })
        }
        _ if error.reason_code() == Some("entity_not_found") => {
            stderr(match error.detail().or(entity) {
                Some(name) => display_for(
                    locale,
                    "journal.cli.sol.entities.entity.error.error.entity.not.found",
                    [("name", &(name))],
                ),
                None => render_for(
                    locale,
                    "journal.cli.sol.entities.edge.resolution.without.query.empty.error.entity.entity.not.found",
                    &[],
                ),
            })
        }
        _ if error.reason_code() == Some("edge_index_unavailable") => stderr(render_for(
            locale,
            "journal.cli.sol.entities.edges.unavailable",
            &[],
        )),
        _ if error.reason_code() == Some("entity_alias_conflict") && error.detail().is_some() => {
            stderr(display_for(
                locale,
                "journal.cli.client.error.detail",
                [("detail", &(error.detail().unwrap_or_default()))],
            ))
        }
        _ if matches!(
            error.reason_code(),
            Some("invalid_request_value" | "entity_operation_failed" | "entity_already_exists")
        ) && error.detail().is_some() =>
        {
            stderr(display_for(
                locale,
                "journal.cli.client.error.detail",
                [("detail", &(error.detail().unwrap_or_default()))],
            ))
        }
        _ => stderr(error.message_for(locale)),
    }
}

fn trust_error(locale: &str, error: ClientError, json_output: bool) -> CommandOutput {
    if json_output
        && let ClientError::ReasonRejected { payload, .. } = &error
        && !payload.is_null()
    {
        return CommandOutput {
            stdout: String::new(),
            stderr: format!("{}\n", json_pretty_utf8(payload)),
            exit: 1,
        };
    }
    entity_error(locale, error, None, None)
}

fn merge_json_error(error: ClientError) -> CommandOutput {
    let mut payload = Map::new();
    payload.insert(
        "error".to_string(),
        Value::String(error.detail().unwrap_or(error.message()).to_string()),
    );
    crate::agent_output::literal_output(CommandOutput {
        stdout: String::new(),
        stderr: format!("{}\n", json_pretty_utf8(&Value::Object(payload))),
        exit: 1,
    })
}

fn merge_candidate_error(locale: &str, result: &Value) -> CommandOutput {
    stderr(match result.get("error") {
        Some(value) => display_for(
            locale,
            "journal.cli.client.error.detail",
            [("detail", &display_value(Some(value)))],
        ),
        None => render_for(
            locale,
            "journal.cli.sol.entities.merge.failure.default.error.operation.failed",
            &[],
        ),
    })
}

fn render_accept_merge_candidate(
    locale: &str,
    result: &Value,
    source_slug: &str,
    target_slug: &str,
) -> CommandOutput {
    match result.get("status").and_then(Value::as_str) {
        Some("error") => merge_candidate_error(locale, result),
        Some("preview") => {
            render_merge_preview(locale, result.get("fields").unwrap_or(&Value::Null))
        }
        Some("accepted") => {
            let mut lines = vec![display_for(
                locale,
                "journal.cli.sol.entities.accept.merge.candidate.accepted.merge.candidate",
                [
                    ("source_slug", &(source_slug)),
                    ("target_slug", &(target_slug)),
                ],
            )];
            if let Some(merge_id) = result.get("merge_id").and_then(Value::as_str) {
                lines.push(display_for(
                    locale,
                    "journal.cli.sol.entities.accept.merge.candidate.merge.is.permanent",
                    [("merge_id", &(merge_id))],
                ));
            }
            stdout(lines)
        }
        Some("already_accepted") => stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.accept.merge.candidate.merge.candidate.already.accepted",
            [
                ("source_slug", &(source_slug)),
                ("target_slug", &(target_slug)),
            ],
        )),
        status => stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.accept.merge.candidate.accept.result.for",
            [
                ("source_slug", &(source_slug)),
                ("target_slug", &(target_slug)),
                ("status", &(status.unwrap_or("None"))),
            ],
        )),
    }
}

fn render_merge_preview(locale: &str, fields: &Value) -> CommandOutput {
    let mut lines = vec![
        render_for(
            locale,
            "journal.cli.sol.entities.merge.preview.merge.preview",
            &[],
        )
        .to_string(),
    ];
    // The server measures how many aliases the merge would add, not which.
    lines.push(display_for(
        locale,
        "journal.cli.sol.entities.merge.preview.aliases.added",
        [("aliases", &(display_value(fields.get("akas_added"))))],
    ));
    lines.push(display_for(
        locale,
        "journal.cli.sol.entities.merge.preview.emails.added",
        [("emails", &(display_value(fields.get("emails_added_count"))))],
    ));
    if fields.get("other_changes_not_previewed") == Some(&Value::Bool(true)) {
        lines.push(
            render_for(
                locale,
                "journal.cli.sol.entities.merge.preview.this.preview.covers.alias.and",
                &[],
            )
            .to_owned(),
        );
    } else {
        lines.push(display_for(
            locale,
            "journal.cli.sol.entities.merge.preview.facet.links.moved.merged",
            [
                ("moved", &(display_value(fields.get("facet_moved_count")))),
                ("merged", &(display_value(fields.get("facet_merged_count")))),
            ],
        ));
        lines.push(display_for(
            locale,
            "journal.cli.sol.entities.merge.preview.observations.moved",
            [(
                "observations",
                &(display_value(fields.get("observations_appended"))),
            )],
        ));
        lines.push({
            let display_labels = (display_value(fields.get("labels_rewritten"))).to_string();
            let display_corrections =
                (display_value(fields.get("corrections_rewritten"))).to_string();
            render_for(
                locale,
                "journal.cli.sol.entities.merge.preview.speaker.labels.updated.labels.corrections",
                &[
                    ("labels", CopyValue::String(&display_labels)),
                    ("corrections", CopyValue::String(&display_corrections)),
                    (
                        "quantity",
                        crate::command::count_selector(display_labels.parse().unwrap_or(0)),
                    ),
                    (
                        "corrections_quantity",
                        crate::command::count_selector(display_corrections.parse().unwrap_or(0)),
                    ),
                ],
            )
        });
        lines.push(display_for(
            locale,
            "journal.cli.sol.entities.merge.preview.voice.samples.moved.added.total",
            [
                ("added", &(display_value(fields.get("voiceprints_added")))),
                (
                    "total",
                    &(display_value(fields.get("voiceprints_target_total"))),
                ),
            ],
        ));
    }
    let errors = array_field(fields, "segment_errors");
    if !errors.is_empty() {
        lines.push(display_for(
            locale,
            "journal.cli.sol.entities.merge.preview.segment.update.errors",
            [("errors", &(errors.len()))],
        ));
    }
    stdout(lines)
}

fn render_ambiguities(locale: &str, rows: &[Value]) -> CommandOutput {
    if rows.is_empty() {
        return stdout_line(render_for(
            locale,
            "journal.cli.sol.entities.ambiguities.no.entity.ambiguities.found",
            &[],
        ));
    }
    let mut lines = Vec::new();
    for row in rows {
        let scope = row.get("scope").and_then(Value::as_object);
        let mut scope_label = scope
            .and_then(|scope| scope.get("kind"))
            .map(|value| display_value(Some(value)))
            .unwrap_or_default();
        if let Some(facet) = scope
            .and_then(|scope| scope.get("facet"))
            .and_then(Value::as_str)
        {
            scope_label.push(':');
            scope_label.push_str(facet);
        }
        lines.push(display_for(
            locale,
            "journal.cli.sol.entities.ambiguities.scope.tier",
            [
                ("ambiguity_id", &(display_value(row.get("ambiguity_id")))),
                (
                    "name",
                    &(crate::agent_output::literal_links(&display_value(
                        row.get("original_query"),
                    ))),
                ),
                ("status", &(display_value(row.get("status")))),
                ("scope", &(scope_label)),
                ("tier", &(display_value(row.get("observed_tier")))),
            ],
        ));
        let origins = array_field(row, "origins");
        let mut lanes = origins
            .iter()
            .filter_map(|origin| origin.get("lane").and_then(Value::as_str))
            .filter(|lane| !lane.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        lanes.sort();
        lanes.dedup();
        if !lanes.is_empty() {
            lines.push(display_for(
                locale,
                "journal.cli.sol.entities.ambiguities.from",
                [("sources", &(lanes.join(", ")))],
            ));
        }
        for candidate in array_field(row, "ranked_candidates") {
            let name = candidate
                .get("name")
                .and_then(Value::as_str)
                .map(|name| Value::String(crate::agent_output::literal_links(name)));
            lines.push(display_for(
                locale,
                "journal.cli.sol.entities.ambiguities.score",
                [
                    (
                        "candidate",
                        &(display_entity(
                            candidate.get("id"),
                            name.as_ref().or(candidate.get("name")),
                        )),
                    ),
                    ("score", &(display_value(candidate.get("score")))),
                ],
            ));
        }
    }
    stdout(lines)
}

fn render_entity_versions(locale: &str, body: &Value) -> CommandOutput {
    let rows = array_field(body, "items");
    if rows.is_empty() {
        return stdout_line(match body.get("entity_id") {
            Some(value) => display_for(
                locale,
                "journal.cli.sol.entities.entity.versions.no.identity.history.for",
                [("entity", &(display_value(Some(value))))],
            ),
            None => render_for(
                locale,
                "journal.cli.sol.entities.entity.versions.unknown.no.identity.history.for.entity",
                &[],
            ),
        });
    }
    let mut lines = Vec::new();
    for row in rows {
        let mut label = format!(
            "{}  {}  {}  {}",
            display_value(row.get("seq")),
            display_value(row.get("kind")),
            display_value(row.get("ts")),
            display_value(row.get("version_id"))
        );
        if row.get("merge_id").is_some_and(truthy_value) {
            label.push_str(&display_for(
                locale,
                "journal.cli.sol.entities.entity.versions.merge",
                [
                    ("merge_id", &(display_value(row.get("merge_id")))),
                    ("description", &(display_value(row.get("merge_state")))),
                ],
            ));
        }
        lines.push(label);
    }
    stdout(lines)
}

fn render_network(locale: &str, body: &Value) -> CommandOutput {
    let neighbors = array_field(body, "neighbors");
    let total = int_value(body.get("total_neighbors")).unwrap_or(0);
    let entity_id = value_or_default(body.get("entity_id"), "");
    if total == 0 {
        return stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.network.no.recorded.connections.for",
            [("entity_id", &(entity_id))],
        ));
    }
    let mut lines = vec![if neighbors.len() < total as usize {
        {
            let display_total = (total).to_string();
            let display_entity_id = (entity_id).to_string();
            let display_shown = (neighbors.len()).to_string();
            render_for(
                locale,
                "journal.cli.sol.entities.network.limited.recorded.connections.for.showing",
                &[
                    ("total", CopyValue::String(&display_total)),
                    ("entity_id", CopyValue::String(&display_entity_id)),
                    ("shown", CopyValue::String(&display_shown)),
                    (
                        "quantity",
                        crate::command::count_selector(display_total.parse().unwrap_or(0)),
                    ),
                ],
            )
        }
    } else {
        {
            let display_total = (total).to_string();
            let display_entity_id = (entity_id).to_string();
            render_for(
                locale,
                "journal.cli.sol.entities.network.all.recorded.connections.for",
                &[
                    ("total", CopyValue::String(&display_total)),
                    ("entity_id", CopyValue::String(&display_entity_id)),
                    (
                        "quantity",
                        crate::command::count_selector(display_total.parse().unwrap_or(0)),
                    ),
                ],
            )
        }
    }];
    for (index, neighbor) in neighbors.iter().enumerate() {
        render_neighbor(locale, &mut lines, index + 1, neighbor);
    }
    stdout(lines)
}

fn render_neighbor(locale: &str, lines: &mut Vec<String>, index: usize, neighbor: &Value) {
    let mut parts = vec![
        display_entity(neighbor.get("entity_id"), neighbor.get("name")),
        display_for(
            locale,
            "journal.cli.sol.entities.neighbor.score",
            [(
                "score",
                &(format!("{:.2}", float_value(neighbor.get("score")).unwrap_or(0.0))),
            )],
        ),
        display_for(
            locale,
            "journal.cli.sol.entities.neighbor.count",
            [("count", &(int_value(neighbor.get("count")).unwrap_or(0)))],
        ),
    ];
    let kinds = format_kinds(neighbor.get("kinds"));
    if !kinds.is_empty() {
        parts.push(display_for(
            locale,
            "journal.cli.sol.entities.neighbor.kinds",
            [("kinds", &(kinds))],
        ));
    }
    let seen = format_seen(locale, neighbor);
    if !seen.is_empty() {
        parts.push(seen);
    }
    let directed = format_directed(locale, neighbor);
    if !directed.is_empty() {
        parts.push(directed);
    }
    lines.push(format!("  {index}. {}", parts.join(" ")));
    for row in array_field(neighbor, "evidence") {
        lines.push(format!("     - {}", format_evidence(locale, &row, false)));
    }
}

fn render_history(locale: &str, body: &Value) -> CommandOutput {
    let evidence = array_field(body, "evidence");
    let total = int_value(body.get("total")).unwrap_or(0);
    let entity_id = value_or_default(body.get("entity_id"), "");
    let peer = display_entity(body.get("peer_id"), body.get("peer_name"));
    if total == 0 {
        return stdout_line(display_for(
            locale,
            "journal.cli.sol.entities.history.no.recorded.connection.history.between",
            [("entity_id", &(entity_id)), ("peer", &(peer))],
        ));
    }
    let offset = int_value(body.get("offset")).unwrap_or(0);
    let mut lines = vec![if offset != 0 || evidence.len() < total as usize {
        if evidence.is_empty() {
            {
                let display_total = (total).to_string();
                let display_entity_id = (entity_id).to_string();
                let display_peer = (peer).to_string();
                render_for(
                    locale,
                    "journal.cli.sol.entities.history.no.visible.rows",
                    &[
                        ("total", CopyValue::String(&display_total)),
                        ("entity_id", CopyValue::String(&display_entity_id)),
                        ("peer", CopyValue::String(&display_peer)),
                        (
                            "quantity",
                            crate::command::count_selector(display_total.parse().unwrap_or(0)),
                        ),
                    ],
                )
            }
        } else {
            {
                let display_total = (total).to_string();
                let display_entity_id = (entity_id).to_string();
                let display_peer = (peer).to_string();
                let display_first = (offset + 1).to_string();
                let display_last = (offset + evidence.len() as i64).to_string();
                render_for(
                    locale,
                    "journal.cli.sol.entities.history.range.evidence.rows.for.showing",
                    &[
                        ("total", CopyValue::String(&display_total)),
                        ("entity_id", CopyValue::String(&display_entity_id)),
                        ("peer", CopyValue::String(&display_peer)),
                        ("first", CopyValue::String(&display_first)),
                        ("last", CopyValue::String(&display_last)),
                        (
                            "quantity",
                            crate::command::count_selector(display_total.parse().unwrap_or(0)),
                        ),
                    ],
                )
            }
        }
    } else {
        {
            let display_total = (total).to_string();
            let display_entity_id = (entity_id).to_string();
            let display_peer = (peer).to_string();
            render_for(
                locale,
                "journal.cli.sol.entities.history.all.evidence.rows.for",
                &[
                    ("total", CopyValue::String(&display_total)),
                    ("entity_id", CopyValue::String(&display_entity_id)),
                    ("peer", CopyValue::String(&display_peer)),
                    (
                        "quantity",
                        crate::command::count_selector(display_total.parse().unwrap_or(0)),
                    ),
                ],
            )
        }
    }];
    for row in evidence {
        lines.push(format!("  - {}", format_evidence(locale, &row, true)));
    }
    stdout(lines)
}

fn render_overview(locale: &str, body: &Value) -> CommandOutput {
    let entities = array_field(body, "entities");
    let totals = body.get("totals").and_then(Value::as_object);
    let total_edges = totals
        .and_then(|totals| int_value(totals.get("edges")))
        .unwrap_or(0);
    let total_entities = totals
        .and_then(|totals| int_value(totals.get("entities")))
        .unwrap_or(0);
    if total_edges == 0 || total_entities == 0 {
        return stdout_line(render_for(
            locale,
            "journal.cli.sol.entities.overview.no.recorded.connections.in.the",
            &[],
        ));
    }
    let mut lines = vec![if entities.len() < total_entities as usize {
        {
            let display_edges = (total_edges).to_string();
            let display_entities = (total_entities).to_string();
            let display_shown = (entities.len()).to_string();
            render_for(
                locale,
                "journal.cli.sol.entities.overview.limited.network.overview.edges.across.entities",
                &[
                    ("edges", CopyValue::String(&display_edges)),
                    ("entities", CopyValue::String(&display_entities)),
                    ("shown", CopyValue::String(&display_shown)),
                    (
                        "quantity",
                        crate::command::count_selector(display_edges.parse().unwrap_or(0)),
                    ),
                    (
                        "entities_quantity",
                        crate::command::count_selector(display_entities.parse().unwrap_or(0)),
                    ),
                ],
            )
        }
    } else {
        {
            let display_edges = (total_edges).to_string();
            let display_entities = (total_entities).to_string();
            render_for(
                locale,
                "journal.cli.sol.entities.overview.all.network.overview.edges.across.entities",
                &[
                    ("edges", CopyValue::String(&display_edges)),
                    ("entities", CopyValue::String(&display_entities)),
                    (
                        "quantity",
                        crate::command::count_selector(display_edges.parse().unwrap_or(0)),
                    ),
                    (
                        "entities_quantity",
                        crate::command::count_selector(display_entities.parse().unwrap_or(0)),
                    ),
                ],
            )
        }
    }];
    let kinds = format_kinds(body.get("kinds"));
    if !kinds.is_empty() {
        lines.push(display_for(
            locale,
            "journal.cli.sol.entities.overview.kinds",
            [("kinds", &(kinds))],
        ));
    }
    for (index, entity) in entities.iter().enumerate() {
        let mut parts = vec![display_entity(entity.get("entity_id"), entity.get("name"))];
        if let Some(entity_type) = entity
            .get("type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            parts.push(display_for(
                locale,
                "journal.cli.sol.entities.overview.type",
                [("entity_type", &(entity_type))],
            ));
        }
        parts.push(display_for(
            locale,
            "journal.cli.sol.entities.neighbor.score",
            [(
                "score",
                &(format!("{:.2}", float_value(entity.get("score")).unwrap_or(0.0))),
            )],
        ));
        parts.push(display_for(
            locale,
            "journal.cli.sol.entities.neighbor.count",
            [("count", &(int_value(entity.get("count")).unwrap_or(0)))],
        ));
        let kinds = format_kinds(entity.get("kinds"));
        if !kinds.is_empty() {
            parts.push(display_for(
                locale,
                "journal.cli.sol.entities.neighbor.kinds",
                [("kinds", &(kinds))],
            ));
        }
        let seen = format_seen(locale, entity);
        if !seen.is_empty() {
            parts.push(seen);
        }
        lines.push(format!("  {}. {}", index + 1, parts.join(" ")));
    }
    stdout(lines)
}

fn edge_filter_params(parsed: &ParsedArgs) -> Vec<QueryParam> {
    let mut params = Vec::new();
    for raw in parsed.values("--kinds") {
        for item in raw.split(',') {
            let kind = item.trim();
            if !kind.is_empty() {
                params.push(QueryParam::single("kinds", kind));
            }
        }
    }
    push_param(&mut params, "facet", parsed.value("--facet"));
    push_param(&mut params, "day_from", parsed.value("--day-from"));
    push_param(&mut params, "day_to", parsed.value("--day-to"));
    params
}

fn params_from_optional(values: &[(&str, Option<&str>)]) -> Vec<QueryParam> {
    let mut params = Vec::new();
    for (key, value) in values {
        push_param(&mut params, key, *value);
    }
    params
}

fn push_param(params: &mut Vec<QueryParam>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        params.push(QueryParam::single(key, value));
    }
}

fn number_value(value: &str) -> Value {
    value
        .parse::<i64>()
        .map(Number::from)
        .map(Value::Number)
        .unwrap_or_else(|_| Value::String(value.to_string()))
}

fn stdout_json(value: &Value) -> CommandOutput {
    CommandOutput::success(format!("{}\n", json_pretty_utf8(value)))
}

fn stdout_line(value: impl AsRef<str>) -> CommandOutput {
    stdout(vec![value.as_ref().to_string()])
}

fn stdout(lines: Vec<String>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", lines.join("\n")))
}

fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}

fn array_field(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn integer_field(value: &Value, key: &str) -> Option<usize> {
    value.get(key).and_then(Value::as_u64).map(|n| n as usize)
}

fn value_or_empty(value: Option<&Value>) -> String {
    value
        .map(|value| display_value(Some(value)))
        .unwrap_or_default()
}

fn value_or_default(value: Option<&Value>, default: &str) -> String {
    value
        .map(|value| display_value(Some(value)))
        .unwrap_or_else(|| default.to_string())
}

fn display_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Bool(false)) => "False".to_string(),
        Some(Value::Null) | None => "None".to_string(),
        Some(value) => value.to_string(),
    }
}

fn display_entity(entity_id: Option<&Value>, name: Option<&Value>) -> String {
    let entity_id_text = value_or_default(entity_id, "");
    let name_text = value_or_default(name, "");
    if !name_text.is_empty() && name_text != entity_id_text {
        format!("{name_text} ({entity_id_text})")
    } else {
        entity_id_text
    }
}

fn candidate_display(candidate: &Value) -> String {
    display_entity(candidate.get("id"), candidate.get("name"))
}

fn format_kinds(kinds: Option<&Value>) -> String {
    let Some(object) = kinds.and_then(Value::as_object) else {
        return String::new();
    };
    let mut keys = object.keys().collect::<Vec<_>>();
    keys.sort();
    let mut parts = Vec::new();
    for key in keys {
        let count = object
            .get(key)
            .and_then(|item| item.get("count"))
            .and_then(|value| int_value(Some(value)))
            .unwrap_or(0);
        if count != 0 {
            parts.push(format!("{key}:{count}"));
        }
    }
    parts.join(", ")
}

fn format_seen(locale: &str, item: &Value) -> String {
    let first_seen = item.get("first_seen").and_then(Value::as_str);
    let last_seen = item.get("last_seen").and_then(Value::as_str);
    match (first_seen, last_seen) {
        (Some(first), Some(last)) if first != last => display_for(
            locale,
            "journal.cli.sol.entities.network.seen.range.seen",
            [("first", &(first)), ("last", &(last))],
        ),
        (Some(first), _) => display_for(
            locale,
            "journal.cli.sol.entities.network.seen.seen",
            [("day", &(first))],
        ),
        (_, Some(last)) => display_for(
            locale,
            "journal.cli.sol.entities.network.seen.seen",
            [("day", &(last))],
        ),
        _ => String::new(),
    }
}

fn format_directed(locale: &str, item: &Value) -> String {
    let Some(directed) = item.get("directed").and_then(Value::as_object) else {
        return String::new();
    };
    let out_count = int_value(directed.get("out")).unwrap_or(0);
    let in_count = int_value(directed.get("in")).unwrap_or(0);
    match (out_count, in_count) {
        (0, 0) => String::new(),
        (_, 0) => display_for(
            locale,
            "journal.cli.sol.entities.network.directed.out.directed.out",
            [("out_count", &(out_count))],
        ),
        (0, _) => display_for(
            locale,
            "journal.cli.sol.entities.network.directed.in.directed.in",
            [("in_count", &(in_count))],
        ),
        _ => display_for(
            locale,
            "journal.cli.sol.entities.network.directed.both.directed.out.in",
            [("out_count", &(out_count)), ("in_count", &(in_count))],
        ),
    }
}

fn format_evidence(locale: &str, row: &Value, include_source: bool) -> String {
    let mut text = format!(
        "{} {}",
        value_or_default(
            row.get("day"),
            &render_for(
                locale,
                "journal.cli.sol.entities.evidence.default.day.unknown.day",
                &[]
            )
        ),
        value_or_default(
            row.get("kind"),
            &render_for(
                locale,
                "journal.cli.sol.entities.evidence.kind.missing.unknown",
                &[]
            )
        )
    );
    if let Some(label) = row
        .get("label")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        text.push_str(&format!(" - {label}"));
    }
    if let Some(anchor) = row
        .get("anchor")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        text.push_str(&format!(" ({anchor})"));
    }
    if include_source {
        let mut source_parts = Vec::new();
        if let Some(source) = row
            .get("source")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            source_parts.push(format!("[{source}]"));
        }
        if let Some(path) = row
            .get("path")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            source_parts.push(path.to_string());
        }
        if !source_parts.is_empty() {
            text.push(' ');
            text.push_str(&source_parts.join(" "));
        }
    }
    text
}

fn int_value(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
    })
}

fn float_value(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
    })
}

fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(truthy_value)
}

fn truthy_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn strip_parenthetical(value: &str) -> String {
    let mut output = String::new();
    let mut depth = 0_i32;
    for ch in value.chars() {
        match ch {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => output.push(ch),
            _ => {}
        }
    }
    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn run_without_transport(
        path: &[&str],
        args: &[&str],
    ) -> (CommandOutput, crate::seam::ScriptedHttpTransport) {
        let transport = crate::seam::ScriptedHttpTransport::new(vec![]);
        let (_, handler) = crate::aggregate::handler_for(path).expect("registered native command");
        let output = handler(CommandContext {
            args: &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
            env: &BTreeMap::new(),
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
        (output, transport)
    }

    #[test]
    fn committing_a_merge_needs_yes_and_sends_nothing_without_it() {
        for (path, args) in [
            (&["entities", "merge"][..], &["a", "b", "--commit"][..]),
            (&["entities", "delete"][..], &["bob"][..]),
            (&["entities", "delete"][..], &["bob", "--erase"][..]),
            (
                &["entities", "accept-merge-candidate"][..],
                &["a", "b", "--facet", "work", "--commit"][..],
            ),
            (&["speakers", "resolve-names"][..], &["--commit"][..]),
        ] {
            let (output, transport) = run_without_transport(path, args);
            assert_eq!(output.exit, 1, "{path:?}");
            assert!(
                output.stderr.contains("--yes"),
                "{path:?}: {}",
                output.stderr
            );
            assert!(transport.recorded().is_empty(), "{path:?}");
        }
        assert!(crate::aggregate::handler_for(&["entities", "undo-merge"]).is_none());
    }

    fn run_delete(args: &[&str], json: Value, answer: Value) -> CommandOutput {
        use crate::seam::ExpectedHttpCall;
        use crate::transport::HttpResponse;
        let mut calls = Vec::new();
        // An erase now asks first whether it can also go from the backups.
        if args.contains(&"--erase") {
            calls.push(crate::command::backups_state_call(json!({"set_up": false})));
        }
        calls.push(ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Post,
                path: "/app/entities/api/journal/delete".to_owned(),
                params: vec![],
                json: Some(json),
                headers: vec![],
                policy: TimeoutPolicy::EntityMutation,
            },
            result: Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: answer.to_string().into_bytes(),
                policy: TimeoutPolicy::EntityMutation,
            }),
        });
        // A move that says its erase day asks the journal for its today.
        if answer.pointer("/done/0/result/trash/erase_day").is_some() {
            calls.push(crate::command::shell_today_call("20261111"));
        }
        let transport = crate::seam::ScriptedHttpTransport::new(calls);
        let (_, handler) =
            crate::aggregate::handler_for(&["entities", "delete"]).expect("registered");
        handler(CommandContext {
            args: &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
            env: &BTreeMap::new(),
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
        })
    }

    #[test]
    fn delete_sends_one_entity_as_a_set_and_erases_only_when_asked() {
        let moved = run_delete(
            &["bob", "--yes"],
            json!({"ids": ["bob"], "erase": false}),
            json!({"success": true, "done": [{"id": "bob", "result": {"success": true, "trash": {"id": "t1", "erase_at": "2026-11-17T06:00:00.000Z", "erase_day": "20261116"}}}], "failed": []}),
        );
        assert_eq!(moved.exit, 0, "{}", moved.stderr);
        assert!(moved.stdout.contains("trash"), "{}", moved.stdout);
        // The owner's day, not the time's UTC date.
        let day = crate::command::format_erase_day("20261116", Some("20261111")).unwrap();
        assert!(moved.stdout.contains(&day), "{}", moved.stdout);
        assert!(!moved.stdout.contains("2026-11-17"), "{}", moved.stdout);

        let erased = run_delete(
            &["bob", "--yes", "--erase"],
            json!({"ids": ["bob"], "erase": true, "backups": false}),
            json!({"success": true, "done": [{"id": "bob", "result": {"success": true, "erased": true}}], "failed": []}),
        );
        assert_eq!(erased.exit, 0, "{}", erased.stderr);
        assert!(erased.stdout.contains("Erased"), "{}", erased.stdout);

        let refused = run_delete(
            &["me", "--yes"],
            json!({"ids": ["me"], "erase": false}),
            json!({"success": true, "done": [], "failed": [{"id": "me", "reason": "principal", "error": "you can't delete yourself"}]}),
        );
        assert_eq!(refused.exit, 1);
        assert!(refused.stderr.contains("'me'"), "{}", refused.stderr);
    }

    fn rejected(reason_code: &str) -> ClientError {
        ClientError::ReasonRejected {
            status: 503,
            error: "search refused".to_owned(),
            reason_code: Some(reason_code.to_owned()),
            detail: None,
            payload: Box::new(Value::Null),
        }
    }

    #[test]
    fn entity_search_errors_have_specific_recovery_messages() {
        for (reason_code, message) in [
            (
                "entity_search_index_unavailable",
                render_for(
                    "en-US",
                    "journal.cli.sol.entities.search.index.unavailable",
                    &[],
                ),
            ),
            (
                "entity_search_index_busy",
                render_for("en-US", "journal.cli.sol.entities.search.index.busy", &[]),
            ),
            (
                "entity_search_index_stale",
                render_for("en-US", "journal.cli.sol.entities.search.index.stale", &[]),
            ),
            (
                "entity_search_activity_unavailable",
                render_for(
                    "en-US",
                    "journal.cli.sol.entities.search.activity.unreadable",
                    &[],
                ),
            ),
        ] {
            let output = entity_error("en-US", rejected(reason_code), None, None);
            assert_eq!(output.stderr, format!("{message}\n"), "{reason_code}");
            assert_eq!(output.exit, 1, "{reason_code}");
        }
    }
}
