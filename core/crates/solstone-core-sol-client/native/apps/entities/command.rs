// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Number, Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::{ClientError, MALFORMED_RESPONSE_MESSAGE, SERVICE_DOWN_MESSAGE};
use crate::json_format::{json_pretty_ascii, json_pretty_utf8};
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const EDGE_INDEX_UNAVAILABLE_MESSAGE: &str = "your connections couldn't be read because the index hasn't been built yet. run `journal indexer --rebuild-edges` to build it.";
const ENTITY_BUSY_MESSAGE: &str =
    "that entity couldn't be updated right now because it was busy. try again in a moment.";
const ENTITY_SEARCH_ACTIVITY_UNAVAILABLE_MESSAGE: &str = "Detected entity activity is unreadable. Run `journal doctor`, repair the reported record, and try again.";
const ENTITY_SEARCH_INDEX_BUSY_MESSAGE: &str =
    "Entity search is unavailable while indexing is in progress. Try again when it finishes.";
const ENTITY_SEARCH_INDEX_STALE_MESSAGE: &str =
    "The entity search index is stale. Run `journal indexer --rescan-full` and try again.";
const ENTITY_SEARCH_INDEX_UNAVAILABLE_MESSAGE: &str = "The entity search index is unavailable. Run `journal indexer --reset --rescan-full` and try again.";
const ENTITY_HISTORY_BASE_ROUTE: &str = "/app/entities/api/journal";

#[must_use]
pub fn list(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--facet", Some("-f")), ("--day", Some("-d"))],
        &[],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let facet_arg = parsed
        .positionals
        .first()
        .map(String::as_str)
        .or_else(|| parsed.value("--facet"));
    let facet = match resolve_facet(ctx, facet_arg) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let day = parsed.value("--day");
    let result = if let Some(day) = day {
        request_json(
            ctx,
            HttpMethod::Get,
            &format!("/app/entities/api/{facet}/detected"),
            vec![QueryParam::single("day", day)],
            None,
        )
    } else {
        request_json(
            ctx,
            HttpMethod::Get,
            &format!("/app/entities/api/{facet}"),
            vec![],
            None,
        )
    };
    let body = match result {
        Ok(body) => body,
        Err(error) => return entity_error(error, None, None),
    };
    let entities = if day.is_some() {
        array_field(&body, "items")
    } else {
        array_field(&body, "attached")
    };
    if entities.is_empty() {
        return stdout_line("No entities found.");
    }
    let mut lines = Vec::new();
    let label = day.map_or_else(
        || "attached".to_string(),
        |day| format!("detected for {day}"),
    );
    lines.push(format!("{} {label} entities:", entities.len()));
    for entity in entities {
        lines.push(format!(
            "  - {} ({}): {}",
            value_or_empty(entity.get("name")),
            value_or_empty(entity.get("type")),
            value_or_default(entity.get("description"), "")
        ));
    }
    stdout(lines)
}

#[must_use]
pub fn move_entity(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--from", None), ("--to", None)],
        &[("--merge", None), ("--consent", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENTITY");
    };
    let Some(from_facet) = parsed.value("--from") else {
        return stderr("Error: option --from is required.");
    };
    let Some(to_facet) = parsed.value("--to") else {
        return stderr("Error: option --to is required.");
    };
    let source_body = match resolve_request(ctx, from_facet, entity) {
        Ok(body) => body,
        Err(output) => return output,
    };
    if !truthy(source_body.get("facet_exists")) {
        return stderr(format!(
            "Error: Facet '{from_facet}' (--from) does not exist."
        ));
    }
    let target_body = match resolve_request(ctx, to_facet, entity) {
        Ok(body) => body,
        Err(output) => return output,
    };
    if !truthy(target_body.get("facet_exists")) {
        return stderr(format!("Error: Facet '{to_facet}' (--to) does not exist."));
    }
    let resolved = match resolved_from_body_or_exit(from_facet, entity, &source_body) {
        Ok(resolved) => resolved,
        Err(output) => return output,
    };
    let entity_name = string_field(&resolved, "name").unwrap_or_else(|| entity.clone());
    let merge = parsed.bool_value("--merge").unwrap_or(false);
    let consent = parsed.bool_value("--consent").unwrap_or(false);
    match request_json(
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
        Ok(_body) => stdout_line(format!(
            "Moved entity '{entity_name}' from '{from_facet}' to '{to_facet}'."
        )),
        Err(error) => entity_error(error, Some(&entity_name), None),
    }
}

#[must_use]
pub fn detect(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--facet", Some("-f")), ("--day", Some("-d"))],
        &[],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(type_) = parsed.positionals.first() else {
        return stderr("Error: missing argument TYPE");
    };
    let Some(entity) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument ENTITY");
    };
    let Some(description) = parsed.positionals.get(2) else {
        return stderr("Error: missing argument DESCRIPTION");
    };
    let facet = match resolve_facet(ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let day = match resolve_day(ctx, parsed.value("--day")) {
        Ok(day) => day,
        Err(output) => return output,
    };
    match request_json(
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
        Ok(body) => stdout_line(format!(
            "Entity '{}' detected for {day}.",
            string_field(&body, "name").unwrap_or_else(|| entity.clone())
        )),
        Err(error) => entity_error(error, Some(entity), Some(type_)),
    }
}

#[must_use]
pub fn attach(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--facet", Some("-f"))], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(type_) = parsed.positionals.first() else {
        return stderr("Error: missing argument TYPE");
    };
    let Some(entity) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument ENTITY");
    };
    let Some(description) = parsed.positionals.get(2) else {
        return stderr("Error: missing argument DESCRIPTION");
    };
    let facet = match resolve_facet(ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/app/entities/api/{facet}/attach"),
        vec![],
        Some(json!({"type": type_, "name": entity, "description": description})),
    ) {
        Ok(_body) => stdout_line(format!("Entity '{entity}' attached.")),
        Err(error) if error.reason_code() == Some("entity_already_exists") => {
            stdout_line(format!("Entity '{entity}' already attached."))
        }
        Err(error) => entity_error(error, Some(entity), Some(type_)),
    }
}

#[must_use]
pub fn update(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--facet", Some("-f")), ("--day", Some("-d"))],
        &[],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENTITY");
    };
    let Some(description) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument DESCRIPTION");
    };
    let facet = match resolve_facet(ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    if let Some(day) = parsed.value("--day") {
        return match request_json(
            ctx,
            HttpMethod::Post,
            &format!("/app/entities/api/{facet}/update-detected"),
            vec![],
            Some(json!({"day": day, "entity": entity, "description": description})),
        ) {
            Ok(_body) => stdout_line(format!("Entity '{entity}' updated for {day}.")),
            Err(error) => entity_error(error, Some(entity), None),
        };
    }
    let resolved = match resolve_entity_or_exit(ctx, &facet, entity) {
        Ok(resolved) => resolved,
        Err(output) => return output,
    };
    let resolved_name = string_field(&resolved, "name").unwrap_or_else(|| entity.clone());
    let entity_id = string_field(&resolved, "id").unwrap_or_default();
    match request_json(
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
        Ok(_body) => stdout_line(format!("Entity '{resolved_name}' updated.")),
        Err(error) => entity_error(error, Some(&resolved_name), None),
    }
}

#[must_use]
pub fn aka(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--facet", Some("-f"))], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENTITY");
    };
    let Some(aka_value) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument AKA");
    };
    let facet = match resolve_facet(ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let resolved = match resolve_entity_or_exit(ctx, &facet, entity) {
        Ok(resolved) => resolved,
        Err(output) => return output,
    };
    let resolved_name = string_field(&resolved, "name").unwrap_or_default();
    let base_name = strip_parenthetical(&resolved_name);
    if let Some(first_word) = base_name.split_whitespace().next()
        && first_word.eq_ignore_ascii_case(aka_value)
    {
        return stdout_line(format!(
            "Alias '{aka_value}' is the first word of '{resolved_name}' (skipped)."
        ));
    }
    if array_field(&resolved, "aka")
        .iter()
        .any(|item| item.as_str() == Some(aka_value))
    {
        return stdout_line(format!(
            "Alias '{aka_value}' already exists for '{resolved_name}'."
        ));
    }
    let entity_id = string_field(&resolved, "id").unwrap_or_default();
    match request_json(
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
        Ok(_body) => stdout_line(format!("Added alias '{aka_value}' to '{resolved_name}'.")),
        Err(error) => entity_error(error, Some(&resolved_name), None),
    }
}

#[must_use]
pub fn record_merge_candidate(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            ("--facet", Some("-f")),
            ("--day", Some("-d")),
            ("--evidence", None),
            ("--basis", None),
            ("--detections", None),
            ("--needs", None),
        ],
        &[("--json", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source) = parsed.positionals.first() else {
        return stderr("Error: missing argument SOURCE");
    };
    let Some(target) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument TARGET");
    };
    let Some(evidence) = parsed.value("--evidence") else {
        return stderr("Error: option --evidence is required.");
    };
    let facet = match resolve_facet(ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let day = match resolve_day(ctx, parsed.value("--day")) {
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
        ctx,
        HttpMethod::Post,
        "/app/entities/api/record-merge-candidate",
        vec![],
        Some(body),
    ) {
        Ok(result) => result,
        Err(error) => return entity_error(error, None, None),
    };
    let Some(row) = result.get("row") else {
        return stderr(MALFORMED_RESPONSE_MESSAGE);
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(row);
    }
    if truthy(result.get("created")) {
        return stdout_line(format!("merge candidate recorded: {source} -> {target}"));
    }
    stdout_line(format!(
        "merge candidate updated: {source} -> {target} (status: {})",
        display_value(row.get("status"))
    ))
}

#[must_use]
pub fn merge_candidates(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--facet", Some("-f")), ("--status", None)],
        &[("--json", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let body = match request_json(
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
        Err(error) => return entity_error(error, None, None),
    };
    let rows = array_field(&body, "items");
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&Value::Array(rows));
    }
    if rows.is_empty() {
        return stdout_line("No merge candidates found.");
    }
    let mut lines = Vec::new();
    for row in rows {
        let evidence = row.get("evidence").and_then(Value::as_object);
        lines.push(format!(
            "{} -> {}  [{}]  facet={}  detections={}  needs={}  last={}",
            value_or_default(row.get("source"), ""),
            value_or_default(row.get("target"), ""),
            value_or_default(row.get("status"), ""),
            value_or_default(row.get("facet"), ""),
            display_value(evidence.and_then(|item| item.get("detection_count"))),
            display_value(evidence.and_then(|item| item.get("needs"))),
            value_or_default(row.get("last_surfaced"), ""),
        ));
    }
    stdout(lines)
}

#[must_use]
pub fn accept_merge_candidate(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--facet", Some("-f"))],
        &[("--commit", Some("--no-commit"))],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source_slug) = parsed.positionals.first() else {
        return stderr("Error: missing argument SOURCE_SLUG");
    };
    let Some(target_slug) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument TARGET_SLUG");
    };
    let facet = match resolve_facet(ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let commit = parsed.bool_value("--commit").unwrap_or(false);
    let result = match request_json(
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
    ) {
        Ok(result) => result,
        Err(error) => return entity_error(error, None, None),
    };
    render_accept_merge_candidate(&result, source_slug, target_slug)
}

#[must_use]
pub fn dismiss_merge_candidate(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--facet", Some("-f"))], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source_slug) = parsed.positionals.first() else {
        return stderr("Error: missing argument SOURCE_SLUG");
    };
    let Some(target_slug) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument TARGET_SLUG");
    };
    let facet = match resolve_facet(ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let result = match request_json(
        ctx,
        HttpMethod::Post,
        "/app/entities/api/dismiss-merge-candidate",
        vec![],
        Some(json!({"facet": facet, "source_slug": source_slug, "target_slug": target_slug})),
    ) {
        Ok(result) => result,
        Err(error) => return entity_error(error, None, None),
    };
    match result.get("status").and_then(Value::as_str) {
        Some("error") => merge_candidate_error(&result),
        Some("dismissed") => stdout_line(format!(
            "Dismissed merge candidate: {source_slug} -> {target_slug}"
        )),
        Some("already_dismissed") => stdout_line(format!(
            "Merge candidate already dismissed: {source_slug} -> {target_slug}"
        )),
        status => stdout_line(format!(
            "dismiss result for {source_slug} -> {target_slug}: {}",
            status.unwrap_or("None")
        )),
    }
}

#[must_use]
pub fn merge(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[],
        &[
            ("--commit", Some("--no-commit")),
            ("--keep-source-as-aka", Some("--no-keep-source-as-aka")),
        ],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(source_slug) = parsed.positionals.first() else {
        return stderr("Error: missing argument SOURCE_SLUG");
    };
    let Some(target_slug) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument TARGET_SLUG");
    };
    let result = match request_json(
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
    ) {
        Ok(result) => result,
        Err(error) => return merge_json_error(error),
    };
    let output = format!("{}\n", json_pretty_ascii(&result));
    if result.get("error").is_some() {
        return CommandOutput {
            stdout: String::new(),
            stderr: output,
            exit: 1,
        };
    }
    CommandOutput::success(output)
}

#[must_use]
pub fn undo_merge(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[("--yes", None), ("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(merge_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument MERGE_ID");
    };
    if !parsed.bool_value("--yes").unwrap_or(false) {
        return stderr("Refusing to undo this merge without --yes.");
    }
    let body = match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/app/entities/api/merge/{merge_id}/undo"),
        vec![],
        Some(json!({})),
    ) {
        Ok(body) => body,
        Err(error) => return trust_error(error, parsed.bool_value("--json").unwrap_or(false)),
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    stdout_line(format!(
        "Undid {merge_id}: restored {} from {} (history {}).",
        display_value(body.get("source_id")),
        display_value(body.get("target_id")),
        display_value(body.get("history_version_id"))
    ))
}

#[must_use]
pub fn ambiguities(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--status", None)], &[("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    if let Some(status) = parsed.value("--status")
        && !matches!(status, "open" | "resolved")
    {
        return stderr("Error: --status must be open or resolved.");
    }
    let body = match request_json(
        ctx,
        HttpMethod::Get,
        "/app/entities/api/ambiguities",
        params_from_optional(&[("status", parsed.value("--status"))]),
        None,
    ) {
        Ok(body) => body,
        Err(error) => return trust_error(error, parsed.bool_value("--json").unwrap_or(false)),
    };
    let rows = array_field(&body, "items");
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    render_ambiguities(&rows)
}

#[must_use]
pub fn resolve_ambiguity(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[("--yes", None), ("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(ambiguity_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument AMBIGUITY_ID");
    };
    let Some(entity_id) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument ENTITY_ID");
    };
    if !parsed.bool_value("--yes").unwrap_or(false) {
        return stderr("Refusing to resolve this ambiguity without --yes.");
    }
    let body = match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/app/entities/api/ambiguities/{ambiguity_id}/resolve"),
        vec![],
        Some(json!({"entity_id": entity_id})),
    ) {
        Ok(body) => body,
        Err(error) => return trust_error(error, parsed.bool_value("--json").unwrap_or(false)),
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    let resolved_at = body
        .get("ambiguity")
        .and_then(|ambiguity| ambiguity.get("resolved_at"))
        .map(|value| display_value(Some(value)))
        .unwrap_or_else(|| "None".to_string());
    stdout_line(format!(
        "Resolved {ambiguity_id} to {entity_id} at {resolved_at}."
    ))
}

#[must_use]
pub fn entity_history(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENTITY_ID");
    };
    let body = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("{ENTITY_HISTORY_BASE_ROUTE}/entity/{entity_id}/history"),
        vec![],
        None,
    ) {
        Ok(body) => body,
        Err(error) => return trust_error(error, parsed.bool_value("--json").unwrap_or(false)),
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    render_entity_versions(&body)
}

#[must_use]
pub fn restore_version(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[("--yes", None), ("--json", None)]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENTITY_ID");
    };
    let Some(version_id) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument VERSION_ID");
    };
    if !parsed.bool_value("--yes").unwrap_or(false) {
        return stderr("Refusing to restore this identity version without --yes.");
    }
    let body = match request_json(
        ctx,
        HttpMethod::Post,
        &format!("{ENTITY_HISTORY_BASE_ROUTE}/entity/{entity_id}/restore"),
        vec![],
        Some(json!({"version_id": version_id})),
    ) {
        Ok(body) => body,
        Err(error) => return trust_error(error, parsed.bool_value("--json").unwrap_or(false)),
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    let version = body
        .get("event")
        .and_then(|event| event.get("version_id"))
        .map(|value| display_value(Some(value)))
        .unwrap_or_else(|| "None".to_string());
    stdout_line(format!(
        "Restored {entity_id} from {version_id}; new history version {version}."
    ))
}

#[must_use]
pub fn network(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            ("--kinds", None),
            ("--facet", Some("-f")),
            ("--day-from", None),
            ("--day-to", None),
            ("--limit", Some("-n")),
            ("--evidence-limit", None),
        ],
        &[("--include-principal", None), ("--json", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENTITY");
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
        ctx,
        "/app/entities/api/network",
        params,
        parsed.bool_value("--json").unwrap_or(false),
    ) {
        Ok(body) => body,
        Err(output) => return output,
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    render_network(&body)
}

#[must_use]
pub fn history(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            ("--kinds", None),
            ("--facet", Some("-f")),
            ("--day-from", None),
            ("--day-to", None),
            ("--limit", Some("-n")),
            ("--offset", None),
        ],
        &[("--json", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENTITY");
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
        ctx,
        "/app/entities/api/history",
        params,
        parsed.bool_value("--json").unwrap_or(false),
    ) {
        Ok(body) => body,
        Err(output) => return output,
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    render_history(&body)
}

#[must_use]
pub fn overview(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            ("--kinds", None),
            ("--facet", Some("-f")),
            ("--day-from", None),
            ("--day-to", None),
            ("--limit", Some("-n")),
        ],
        &[("--json", None)],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let mut params = edge_filter_params(&parsed);
    params.push(QueryParam::single(
        "limit",
        parsed.value("--limit").unwrap_or("25"),
    ));
    let body = match edge_body_or_exit(
        ctx,
        "/app/entities/api/overview",
        params,
        parsed.bool_value("--json").unwrap_or(false),
    ) {
        Ok(body) => body,
        Err(output) => return output,
    };
    if parsed.bool_value("--json").unwrap_or(false) {
        return stdout_json(&body);
    }
    render_overview(&body)
}

#[must_use]
pub fn observations(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--facet", Some("-f"))], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENTITY");
    };
    let facet = match resolve_facet(ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let resolved = match resolve_entity_or_exit(ctx, &facet, entity) {
        Ok(resolved) => resolved,
        Err(output) => return output,
    };
    let resolved_name = string_field(&resolved, "name").unwrap_or_default();
    let body = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/entities/api/{facet}/observations"),
        vec![QueryParam::single("name", &resolved_name)],
        None,
    ) {
        Ok(body) => body,
        Err(error) => return entity_error(error, Some(&resolved_name), None),
    };
    let obs = array_field(&body, "items");
    if obs.is_empty() {
        return stdout_line(format!("No observations for '{resolved_name}'."));
    }
    let mut lines = vec![format!("{} observations for '{resolved_name}':", obs.len())];
    for (index, observation) in obs.iter().enumerate() {
        lines.push(format!(
            "  {}. {}",
            index + 1,
            value_or_default(observation.get("content"), "")
        ));
    }
    stdout(lines)
}

#[must_use]
pub fn observe(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--facet", Some("-f")), ("--source-day", None)],
        &[],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(entity) = parsed.positionals.first() else {
        return stderr("Error: missing argument ENTITY");
    };
    let Some(content) = parsed.positionals.get(1) else {
        return stderr("Error: missing argument CONTENT");
    };
    let facet = match resolve_facet(ctx, parsed.value("--facet")) {
        Ok(facet) => facet,
        Err(output) => return output,
    };
    let resolved = match resolve_entity_or_exit(ctx, &facet, entity) {
        Ok(resolved) => resolved,
        Err(output) => return output,
    };
    let resolved_name = string_field(&resolved, "name").unwrap_or_default();
    match request_json(
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
        Ok(_body) => stdout_line(format!("Observation added to '{resolved_name}'.")),
        Err(error) => entity_error(error, Some(&resolved_name), None),
    }
}

#[must_use]
pub fn search(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            ("--query", Some("-q")),
            ("--type", Some("-t")),
            ("--facet", Some("-f")),
            ("--since", None),
            ("--limit", Some("-n")),
        ],
        &[],
    ) {
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
        ctx,
        HttpMethod::Get,
        "/app/entities/api/search",
        params,
        None,
    ) {
        Ok(body) => body,
        Err(error) => return entity_error(error, None, None),
    };
    let results = array_field(&body, "items");
    if results.is_empty() {
        return stdout_line("No entities found.");
    }
    let mut lines = vec![format!("{} entities:", results.len())];
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
            lines.push(format!("    facets: {facets}"));
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

fn parse_args(
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
                return Err(format!("Error: option {token} requires an argument."));
            };
            parsed.values.push((canonical.to_string(), value.clone()));
        } else if let Some((canonical, value)) = bool_option_value(token, bool_options) {
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
        headers: vec![],
        policy: TimeoutPolicy::Api,
    })?;
    decode_response(&response)
}

fn resolve_request(
    ctx: CommandContext<'_>,
    facet: &str,
    entity: &str,
) -> Result<Value, CommandOutput> {
    request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/entities/api/{facet}/resolve"),
        vec![QueryParam::single("name", entity)],
        None,
    )
    .map_err(|error| entity_error(error, Some(entity), None))
    .and_then(|body| {
        if body.is_object() {
            Ok(body)
        } else {
            Err(stderr(MALFORMED_RESPONSE_MESSAGE))
        }
    })
}

fn resolve_entity_or_exit(
    ctx: CommandContext<'_>,
    facet: &str,
    entity: &str,
) -> Result<Value, CommandOutput> {
    let body = resolve_request(ctx, facet, entity)?;
    resolved_from_body_or_exit(facet, entity, &body)
}

fn resolved_from_body_or_exit(
    facet: &str,
    entity: &str,
    body: &Value,
) -> Result<Value, CommandOutput> {
    if let Some(resolved) = body.get("resolved").filter(|value| value.is_object()) {
        return Ok(resolved.clone());
    }
    Err(render_resolve_error(facet, entity, body))
}

fn render_resolve_error(facet: &str, entity: &str, body: &Value) -> CommandOutput {
    if truthy(body.get("blocked")) {
        let blocked_name = string_field(body, "blocked_name").unwrap_or_else(|| entity.to_string());
        return stderr(format!("Error: Entity '{blocked_name}' is blocked."));
    }
    let candidates = array_field(body, "candidates");
    if !candidates.is_empty() {
        let names = candidates
            .iter()
            .take(3)
            .map(|candidate| value_or_default(candidate.get("name"), ""))
            .collect::<Vec<_>>()
            .join(", ");
        return stderr(format!(
            "Error: Entity '{entity}' not found. Did you mean: {names}"
        ));
    }
    stderr(format!(
        "Error: Entity '{entity}' not found in facet '{facet}'."
    ))
}

fn edge_body_or_exit(
    ctx: CommandContext<'_>,
    path: &str,
    params: Vec<QueryParam>,
    json_output: bool,
) -> Result<Value, CommandOutput> {
    let body = request_json(ctx, HttpMethod::Get, path, params, None)
        .map_err(|error| entity_error(error, None, None))?;
    if !body.is_object() {
        return Err(stderr(MALFORMED_RESPONSE_MESSAGE));
    }
    if body.get("resolved").is_some_and(Value::is_null)
        && body.get("query").is_some()
        && body.get("candidates").is_some()
    {
        return Err(render_edge_resolution_error(&body, json_output));
    }
    Ok(body)
}

fn render_edge_resolution_error(body: &Value, json_output: bool) -> CommandOutput {
    if json_output {
        return CommandOutput {
            stdout: format!("{}\n", json_pretty_utf8(body)),
            stderr: String::new(),
            exit: 1,
        };
    }
    let query = string_field(body, "query").unwrap_or_else(|| "entity".to_string());
    let candidates = array_field(body, "candidates");
    if !candidates.is_empty() {
        let labels = candidates
            .iter()
            .take(3)
            .map(candidate_display)
            .collect::<Vec<_>>()
            .join(", ");
        return stderr(format!(
            "Error: Entity '{query}' not found. Did you mean: {labels}"
        ));
    }
    stderr(format!("Error: Entity '{query}' not found."))
}

fn resolve_facet(ctx: CommandContext<'_>, value: Option<&str>) -> Result<String, CommandOutput> {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        return Ok(value.to_string());
    }
    if let Some(value) = ctx.env.get("SOL_FACET").filter(|value| !value.is_empty()) {
        return Ok(value.clone());
    }
    Err(stderr(
        "Error: facet is required (pass as argument or set SOL_FACET).",
    ))
}

fn resolve_day(ctx: CommandContext<'_>, value: Option<&str>) -> Result<String, CommandOutput> {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        return Ok(value.to_string());
    }
    if let Some(value) = ctx.env.get("SOL_DAY").filter(|value| !value.is_empty()) {
        return Ok(value.clone());
    }
    Err(stderr(
        "Error: day is required (pass as argument or set SOL_DAY).",
    ))
}

fn entity_error(error: ClientError, entity: Option<&str>, type_: Option<&str>) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(SERVICE_DOWN_MESSAGE),
        _ if error.reason_code() == Some("entity_busy") => stderr(ENTITY_BUSY_MESSAGE),
        _ if error.reason_code() == Some("entity_search_index_unavailable") => {
            stderr(ENTITY_SEARCH_INDEX_UNAVAILABLE_MESSAGE)
        }
        _ if error.reason_code() == Some("entity_search_index_busy") => {
            stderr(ENTITY_SEARCH_INDEX_BUSY_MESSAGE)
        }
        _ if error.reason_code() == Some("entity_search_index_stale") => {
            stderr(ENTITY_SEARCH_INDEX_STALE_MESSAGE)
        }
        _ if error.reason_code() == Some("entity_search_activity_unavailable") => {
            stderr(ENTITY_SEARCH_ACTIVITY_UNAVAILABLE_MESSAGE)
        }
        _ if error.reason_code() == Some("invalid_entity_type") && type_.is_some() => {
            stderr(format!(
                "Error: Invalid entity type '{}'.",
                type_.unwrap_or("entity")
            ))
        }
        _ if error.reason_code() == Some("entity_blocked") => {
            let name = error.detail().or(entity).unwrap_or("entity");
            stderr(format!("Error: Entity '{name}' is blocked."))
        }
        _ if error.reason_code() == Some("entity_not_found") => {
            let name = error.detail().or(entity).unwrap_or("entity");
            stderr(format!("Error: Entity '{name}' not found."))
        }
        _ if error.reason_code() == Some("edge_index_unavailable") => {
            stderr(EDGE_INDEX_UNAVAILABLE_MESSAGE)
        }
        _ if error.reason_code() == Some("entity_alias_conflict") && error.detail().is_some() => {
            stderr(format!("Error: {}", error.detail().unwrap_or_default()))
        }
        _ if matches!(
            error.reason_code(),
            Some("invalid_request_value" | "entity_operation_failed" | "entity_already_exists")
        ) && error.detail().is_some() =>
        {
            stderr(format!("Error: {}", error.detail().unwrap_or_default()))
        }
        _ => stderr(error.message()),
    }
}

fn trust_error(error: ClientError, json_output: bool) -> CommandOutput {
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
    entity_error(error, None, None)
}

fn merge_json_error(error: ClientError) -> CommandOutput {
    let mut payload = Map::new();
    payload.insert(
        "error".to_string(),
        Value::String(error.detail().unwrap_or(error.message()).to_string()),
    );
    CommandOutput {
        stdout: String::new(),
        stderr: format!("{}\n", json_pretty_utf8(&Value::Object(payload))),
        exit: 1,
    }
}

fn merge_candidate_error(result: &Value) -> CommandOutput {
    stderr(format!(
        "Error: {}",
        value_or_default(result.get("error"), "operation failed")
    ))
}

fn render_accept_merge_candidate(
    result: &Value,
    source_slug: &str,
    target_slug: &str,
) -> CommandOutput {
    match result.get("status").and_then(Value::as_str) {
        Some("error") => merge_candidate_error(result),
        Some("preview") => render_merge_preview(result.get("fields").unwrap_or(&Value::Null)),
        Some("accepted") => {
            let mut lines = vec![format!(
                "Accepted merge candidate: {source_slug} -> {target_slug}"
            )];
            if let Some(merge_id) = result.get("merge_id").and_then(Value::as_str) {
                lines.push(format!(
                    "Undo with: solstone call entities undo-merge {merge_id} --yes"
                ));
            }
            stdout(lines)
        }
        Some("already_accepted") => {
            let mut lines = vec![format!(
                "Merge candidate already accepted: {source_slug} -> {target_slug}"
            )];
            let undo = result.get("undo").and_then(Value::as_object);
            if undo
                .and_then(|undo| undo.get("available"))
                .is_some_and(truthy_value)
                && let Some(merge_id) = undo
                    .and_then(|undo| undo.get("merge_id"))
                    .and_then(Value::as_str)
            {
                lines.push(format!(
                    "Undo with: solstone call entities undo-merge {merge_id} --yes"
                ));
            } else if let Some(reason) = undo
                .and_then(|undo| undo.get("reason"))
                .and_then(Value::as_str)
            {
                lines.push(format!("Undo unavailable: {reason}"));
            }
            stdout(lines)
        }
        status => stdout_line(format!(
            "accept result for {source_slug} -> {target_slug}: {}",
            status.unwrap_or("None")
        )),
    }
}

fn render_merge_preview(fields: &Value) -> CommandOutput {
    let mut lines = vec!["Merge preview:".to_string()];
    let akas = array_field(fields, "akas_added")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if akas.is_empty() {
        lines.push("  aliases added: none".to_string());
    } else {
        lines.push(format!("  aliases added: {}", akas.join(", ")));
    }
    lines.push(format!(
        "  emails added: {}",
        display_value(fields.get("emails_added_count"))
    ));
    lines.push(format!(
        "  facet links: {} moved, {} merged",
        display_value(fields.get("facet_moved_count")),
        display_value(fields.get("facet_merged_count"))
    ));
    lines.push(format!(
        "  observations moved: {}",
        display_value(fields.get("observations_appended"))
    ));
    lines.push(format!(
        "  speaker labels updated: {} labels, {} corrections",
        display_value(fields.get("labels_rewritten")),
        display_value(fields.get("corrections_rewritten"))
    ));
    lines.push(format!(
        "  voice samples moved: {} added, {} total",
        display_value(fields.get("voiceprints_added")),
        display_value(fields.get("voiceprints_target_total"))
    ));
    let errors = array_field(fields, "segment_errors");
    if !errors.is_empty() {
        lines.push(format!("  segment update errors: {}", errors.len()));
    }
    stdout(lines)
}

fn render_ambiguities(rows: &[Value]) -> CommandOutput {
    if rows.is_empty() {
        return stdout_line("No entity ambiguities found.");
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
        lines.push(format!(
            "{}  {}  [{}]  scope={}  tier={}",
            display_value(row.get("ambiguity_id")),
            display_value(row.get("original_query")),
            display_value(row.get("status")),
            scope_label,
            display_value(row.get("observed_tier"))
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
            lines.push(format!("  from: {}", lanes.join(", ")));
        }
        for candidate in array_field(row, "ranked_candidates") {
            lines.push(format!(
                "  - {} score={}",
                display_entity(candidate.get("id"), candidate.get("name")),
                display_value(candidate.get("score"))
            ));
        }
    }
    stdout(lines)
}

fn render_entity_versions(body: &Value) -> CommandOutput {
    let rows = array_field(body, "items");
    if rows.is_empty() {
        return stdout_line(format!(
            "No identity history for {}.",
            value_or_default(body.get("entity_id"), "entity")
        ));
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
            label.push_str(&format!(
                "  merge={} ({})",
                display_value(row.get("merge_id")),
                display_value(row.get("merge_state"))
            ));
        }
        lines.push(label);
    }
    stdout(lines)
}

fn render_network(body: &Value) -> CommandOutput {
    let neighbors = array_field(body, "neighbors");
    let total = int_value(body.get("total_neighbors")).unwrap_or(0);
    let entity_id = value_or_default(body.get("entity_id"), "");
    if total == 0 {
        return stdout_line(format!("No recorded connections for {entity_id}."));
    }
    let label = plural(total, "recorded connection", None);
    let mut lines = Vec::new();
    if neighbors.len() < total as usize {
        lines.push(format!(
            "{label} for {entity_id} (showing {}):",
            neighbors.len()
        ));
    } else {
        lines.push(format!("{label} for {entity_id}:"));
    }
    for (index, neighbor) in neighbors.iter().enumerate() {
        render_neighbor(&mut lines, index + 1, neighbor);
    }
    stdout(lines)
}

fn render_neighbor(lines: &mut Vec<String>, index: usize, neighbor: &Value) {
    let mut parts = vec![
        display_entity(neighbor.get("entity_id"), neighbor.get("name")),
        format!(
            "score={:.2}",
            float_value(neighbor.get("score")).unwrap_or(0.0)
        ),
        format!("count={}", int_value(neighbor.get("count")).unwrap_or(0)),
    ];
    let kinds = format_kinds(neighbor.get("kinds"));
    if !kinds.is_empty() {
        parts.push(format!("kinds={kinds}"));
    }
    let seen = format_seen(neighbor);
    if !seen.is_empty() {
        parts.push(seen);
    }
    let directed = format_directed(neighbor);
    if !directed.is_empty() {
        parts.push(directed);
    }
    lines.push(format!("  {index}. {}", parts.join(" ")));
    for row in array_field(neighbor, "evidence") {
        lines.push(format!("     - {}", format_evidence(&row, false)));
    }
}

fn render_history(body: &Value) -> CommandOutput {
    let evidence = array_field(body, "evidence");
    let total = int_value(body.get("total")).unwrap_or(0);
    let entity_id = value_or_default(body.get("entity_id"), "");
    let peer = display_entity(body.get("peer_id"), body.get("peer_name"));
    if total == 0 {
        return stdout_line(format!(
            "No recorded connection history between {entity_id} and {peer}."
        ));
    }
    let mut suffix = String::new();
    let offset = int_value(body.get("offset")).unwrap_or(0);
    if offset != 0 || evidence.len() < total as usize {
        suffix = if evidence.is_empty() {
            " (showing 0)".to_string()
        } else {
            format!(
                " (showing {}-{})",
                offset + 1,
                offset + evidence.len() as i64
            )
        };
    }
    let mut lines = vec![format!(
        "{} for {entity_id} <-> {peer}{suffix}:",
        plural(total, "evidence row", None)
    )];
    for row in evidence {
        lines.push(format!("  - {}", format_evidence(&row, true)));
    }
    stdout(lines)
}

fn render_overview(body: &Value) -> CommandOutput {
    let entities = array_field(body, "entities");
    let totals = body.get("totals").and_then(Value::as_object);
    let total_edges = totals
        .and_then(|totals| int_value(totals.get("edges")))
        .unwrap_or(0);
    let total_entities = totals
        .and_then(|totals| int_value(totals.get("entities")))
        .unwrap_or(0);
    if total_edges == 0 || total_entities == 0 {
        return stdout_line("No recorded connections in the edge index.");
    }
    let suffix = if entities.len() < total_entities as usize {
        format!(" (showing {})", entities.len())
    } else {
        String::new()
    };
    let mut lines = vec![format!(
        "Network overview: {} across {}{suffix}:",
        plural(total_edges, "edge", None),
        plural(total_entities, "entity", Some("entities"))
    )];
    let kinds = format_kinds(body.get("kinds"));
    if !kinds.is_empty() {
        lines.push(format!("Kinds: {kinds}"));
    }
    for (index, entity) in entities.iter().enumerate() {
        let mut parts = vec![display_entity(entity.get("entity_id"), entity.get("name"))];
        if let Some(entity_type) = entity
            .get("type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            parts.push(format!("type={entity_type}"));
        }
        parts.push(format!(
            "score={:.2}",
            float_value(entity.get("score")).unwrap_or(0.0)
        ));
        parts.push(format!(
            "count={}",
            int_value(entity.get("count")).unwrap_or(0)
        ));
        let kinds = format_kinds(entity.get("kinds"));
        if !kinds.is_empty() {
            parts.push(format!("kinds={kinds}"));
        }
        let seen = format_seen(entity);
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

fn plural(value: i64, singular: &str, plural: Option<&str>) -> String {
    let noun = if value == 1 {
        singular.to_string()
    } else {
        plural
            .map(str::to_string)
            .unwrap_or_else(|| format!("{singular}s"))
    };
    format!("{value} {noun}")
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

fn format_seen(item: &Value) -> String {
    let first_seen = item.get("first_seen").and_then(Value::as_str);
    let last_seen = item.get("last_seen").and_then(Value::as_str);
    match (first_seen, last_seen) {
        (Some(first), Some(last)) if first != last => format!("seen={first}..{last}"),
        (Some(first), _) => format!("seen={first}"),
        (_, Some(last)) => format!("seen={last}"),
        _ => String::new(),
    }
}

fn format_directed(item: &Value) -> String {
    let Some(directed) = item.get("directed").and_then(Value::as_object) else {
        return String::new();
    };
    let mut parts = Vec::new();
    let out_count = int_value(directed.get("out")).unwrap_or(0);
    let in_count = int_value(directed.get("in")).unwrap_or(0);
    if out_count != 0 {
        parts.push(format!("out:{out_count}"));
    }
    if in_count != 0 {
        parts.push(format!("in:{in_count}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("directed={}", parts.join(", "))
    }
}

fn format_evidence(row: &Value, include_source: bool) -> String {
    let mut text = format!(
        "{} {}",
        value_or_default(row.get("day"), "unknown-day"),
        value_or_default(row.get("kind"), "unknown")
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
                ENTITY_SEARCH_INDEX_UNAVAILABLE_MESSAGE,
            ),
            ("entity_search_index_busy", ENTITY_SEARCH_INDEX_BUSY_MESSAGE),
            (
                "entity_search_index_stale",
                ENTITY_SEARCH_INDEX_STALE_MESSAGE,
            ),
            (
                "entity_search_activity_unavailable",
                ENTITY_SEARCH_ACTIVITY_UNAVAILABLE_MESSAGE,
            ),
        ] {
            let output = entity_error(rejected(reason_code), None, None);
            assert_eq!(output.stderr, format!("{message}\n"), "{reason_code}");
            assert_eq!(output.exit, 1, "{reason_code}");
        }
    }
}
