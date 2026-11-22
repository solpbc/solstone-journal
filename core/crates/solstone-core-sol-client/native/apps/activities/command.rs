// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use jiff::civil::Date;
use serde_json::{Map, Value};
use solstone_core_cli_copy::{display_for, render_for};

use crate::command::{CommandContext, CommandOutput, journal_today};
use crate::decode::decode_response;
use crate::error::ClientError;
use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

const ACTIVITY_NOT_FOUND: &str = "activity_not_found";
const ACTIVITY_ALREADY_EXISTS: &str = "activity_already_exists";
const ACTIVITY_INVALID: &str = "activity_invalid";
const VALID_LIST_SOURCES: &[&str] = &["cogitate", "user"];
const VALID_CREATE_SOURCES: &[&str] = &["user"];
const MUTABLE_FIELDS: &[&str] = &["title", "description", "details"];

#[must_use]
pub fn list(ctx: CommandContext<'_>) -> CommandOutput {
    crate::agent_output::literal_output(list_output(ctx))
}

fn list_output(ctx: CommandContext<'_>) -> CommandOutput {
    let (parsed, locale) = match parse_args(
        ctx,
        &[
            ("--day", Some("-d")),
            ("--from", None),
            ("--to", None),
            ("--facet", Some("-f")),
            ("--activity", Some("-a")),
            ("--entity", None),
            ("--source", None),
        ],
        &["--all", "--json"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let day = parsed.value("--day");
    let from_day = parsed.value("--from");
    let to_day = parsed.value("--to");
    if day.is_some() && (from_day.is_some() || to_day.is_some()) {
        return stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.range.options",
            &[],
        ));
    }
    let source = parsed.value("--source");
    if let Some(source) = source
        && !VALID_LIST_SOURCES.contains(&source)
    {
        return stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.source.list",
            &[],
        ));
    }
    let resolved_days = if let Some(day) = day {
        match resolve_day(locale, Some(day), ctx) {
            Ok(day) => vec![day],
            Err(output) => return output,
        }
    } else if from_day.is_some() || to_day.is_some() {
        let start_day = match from_day {
            Some(day) => day.to_string(),
            None => match resolve_day_or_today(None, ctx) {
                Ok(day) => day,
                Err(error) => return transport_error(locale, error),
            },
        };
        let end_day = to_day.map_or_else(|| start_day.clone(), str::to_string);
        match iter_days(locale, &start_day, &end_day) {
            Ok(days) => days,
            Err(output) => return output,
        }
    } else {
        match resolve_day_or_today(None, ctx) {
            Ok(day) => vec![day],
            Err(error) => return transport_error(locale, error),
        }
    };

    let facet = parsed
        .value("--facet")
        .map(str::to_string)
        .or_else(|| env_value(ctx, "SOL_FACET"));
    let mut items = Vec::new();
    for day in resolved_days {
        let mut params = vec![QueryParam::single(
            "include_hidden",
            if parsed.has_flag("--all") { "1" } else { "0" },
        )];
        if let Some(facet) = facet.as_ref() {
            params.push(QueryParam::single("facet", facet));
        }
        let body = match request_json(
            ctx,
            HttpMethod::Get,
            &format!("/app/activities/api/day/{day}/records"),
            params,
            None,
        ) {
            Ok(body) => body,
            Err(error) => return transport_error(locale, error),
        };
        if let Some(body_items) = body.get("items").and_then(Value::as_array) {
            items.extend(body_items.iter().cloned());
        }
    }

    let activity = parsed.value("--activity");
    let entity = parsed.value("--entity").map(str::to_lowercase);
    items.retain(|item| item_matches(item, activity, entity.as_deref(), source));
    items.sort_by_key(sort_item_key);

    if parsed.has_flag("--json") {
        let records = items
            .iter()
            .filter_map(|item| item.get("record").cloned())
            .collect::<Vec<_>>();
        stdout_json(&Value::Array(records))
    } else if items.is_empty() {
        stdout_line(render_for(
            locale,
            "journal.cli.sol.call.activities.empty",
            &[],
        ))
    } else {
        let markdown = items
            .iter()
            .filter_map(|item| item.get("markdown").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n");
        stdout_line(markdown)
    }
}

#[must_use]
pub fn get(ctx: CommandContext<'_>) -> CommandOutput {
    let (parsed, locale) = match parse_args(
        ctx,
        &[("--facet", Some("-f")), ("--day", Some("-d"))],
        &["--json"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(span_id) = parsed.positionals.first() else {
        return stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.span.missing",
            &[],
        ));
    };
    let facet = match resolve_facet(locale, parsed.value("--facet"), ctx) {
        Ok(value) => value,
        Err(output) => return output,
    };
    let day = match resolve_day(locale, parsed.value("--day"), ctx) {
        Ok(value) => value,
        Err(output) => return output,
    };
    let body = match request_json(
        ctx,
        HttpMethod::Get,
        &format!("/app/activities/api/day/{day}/record/{span_id}"),
        vec![QueryParam::single("facet", facet)],
        None,
    ) {
        Ok(body) => body,
        Err(error) if is_reason(&error, ACTIVITY_NOT_FOUND) => {
            return stderr(display_for(
                locale,
                "journal.cli.sol.call.activities.span.absent",
                [("span_id", &span_id)],
            ));
        }
        Err(error) => return transport_error(locale, error),
    };
    render_record_body(&body, parsed.has_flag("--json"))
}

#[must_use]
pub fn create(ctx: CommandContext<'_>) -> CommandOutput {
    let (parsed, locale) = match parse_args(
        ctx,
        &[
            ("--facet", Some("-f")),
            ("--day", Some("-d")),
            ("--since-segment", None),
            ("--source", None),
            ("--title", None),
            ("--activity", None),
            ("--description", None),
            ("--details", None),
        ],
        &["--json"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let source = parsed.value("--source").unwrap_or("user");
    if !VALID_CREATE_SOURCES.contains(&source) {
        return stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.source.create",
            &[],
        ));
    }
    let facet = match resolve_facet(locale, parsed.value("--facet"), ctx) {
        Ok(value) => value,
        Err(output) => return output,
    };
    let day = match resolve_day(locale, parsed.value("--day"), ctx) {
        Ok(value) => value,
        Err(output) => return output,
    };
    let since_segment = parsed.value("--since-segment");
    if let Some(segment) = since_segment
        && !valid_segment_key(segment)
    {
        return stderr(display_for(
            locale,
            "journal.cli.sol.call.activities.segment.invalid",
            [("segment", &segment)],
        ));
    }

    let payload_flags_supplied = ["--title", "--activity", "--description", "--details"]
        .iter()
        .any(|name| parsed.value(name).is_some());
    let (mut body, activity_type) = if payload_flags_supplied {
        let Some(title) = parsed.value("--title") else {
            return stderr(render_for(
                locale,
                "journal.cli.sol.call.activities.title.option",
                &[],
            ));
        };
        let Some(activity) = parsed.value("--activity") else {
            return stderr(render_for(
                locale,
                "journal.cli.sol.call.activities.kind.option",
                &[],
            ));
        };
        let mut body = Map::new();
        body.insert("title".to_string(), Value::String(title.to_string()));
        body.insert("activity".to_string(), Value::String(activity.to_string()));
        body.insert("source".to_string(), Value::String(source.to_string()));
        insert_optional_string(&mut body, "description", parsed.value("--description"));
        insert_optional_string(&mut body, "details", parsed.value("--details"));
        (body, activity.to_string())
    } else {
        let raw = match ctx.stdin.read_for(locale) {
            Ok(raw) => raw,
            Err(output) => return output,
        };
        match stdin_create_payload(locale, &raw, source) {
            Ok(result) => result,
            Err(output) => return output,
        }
    };
    if let Some(segment) = since_segment {
        body.insert(
            "since_segment".to_string(),
            Value::String(segment.to_string()),
        );
    }

    let response = match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/app/activities/api/day/{day}/records"),
        vec![QueryParam::single("facet", &facet)],
        Some(Value::Object(body)),
    ) {
        Ok(body) => body,
        Err(error) if is_reason(&error, ACTIVITY_NOT_FOUND) => {
            return stderr(display_for(
                locale,
                "journal.cli.sol.call.activities.facet.unknown",
                [("facet", &facet), ("activity_type", &activity_type)],
            ));
        }
        Err(error) if is_reason(&error, ACTIVITY_ALREADY_EXISTS) => {
            return stderr(display_for(
                locale,
                "journal.cli.sol.call.activities.exists",
                [("detail", &error.detail_for(locale).unwrap_or_default())],
            ));
        }
        Err(error) if is_reason(&error, ACTIVITY_INVALID) => {
            return stderr(display_for(
                locale,
                "journal.cli.client.error.detail",
                [("detail", &error.detail_for(locale).unwrap_or_default())],
            ));
        }
        Err(error) => return transport_error(locale, error),
    };
    render_record_body(&response, parsed.has_flag("--json"))
}

#[must_use]
pub fn update(ctx: CommandContext<'_>) -> CommandOutput {
    let (parsed, locale) = match parse_args(
        ctx,
        &[
            ("--facet", Some("-f")),
            ("--day", Some("-d")),
            ("--note", None),
            ("--title", None),
            ("--description", None),
            ("--details", None),
        ],
        &["--json"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(span_id) = parsed.positionals.first() else {
        return stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.span.missing",
            &[],
        ));
    };
    let facet = match resolve_facet(locale, parsed.value("--facet"), ctx) {
        Ok(value) => value,
        Err(output) => return output,
    };
    let day = match resolve_day(locale, parsed.value("--day"), ctx) {
        Ok(value) => value,
        Err(output) => return output,
    };

    let payload_flags_supplied = ["--title", "--description", "--details"]
        .iter()
        .any(|name| parsed.value(name).is_some());
    let patch = if payload_flags_supplied {
        let mut patch = Map::new();
        insert_optional_string(&mut patch, "title", parsed.value("--title"));
        insert_optional_string(&mut patch, "description", parsed.value("--description"));
        insert_optional_string(&mut patch, "details", parsed.value("--details"));
        patch
    } else {
        let raw = match ctx.stdin.read_for(locale) {
            Ok(raw) => raw,
            Err(output) => return output,
        };
        match stdin_update_patch(locale, &raw) {
            Ok(patch) => patch,
            Err(output) => return output,
        }
    };
    if patch.is_empty() {
        return stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.patch.empty",
            &[],
        ));
    }
    let note = parsed
        .value("--note")
        .map(str::to_string)
        .unwrap_or_else(|| {
            let mut fields = patch.keys().cloned().collect::<Vec<_>>();
            fields.sort();
            format!("updated fields: {}", fields.join(", "))
        });
    let mut request_body = Map::new();
    request_body.insert("patch".to_string(), Value::Object(patch));
    request_body.insert("note".to_string(), Value::String(note));

    let response = match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/app/activities/api/day/{day}/record/{span_id}/update"),
        vec![QueryParam::single("facet", facet)],
        Some(Value::Object(request_body)),
    ) {
        Ok(body) => body,
        Err(error) if is_reason(&error, ACTIVITY_NOT_FOUND) => {
            return stderr(display_for(
                locale,
                "journal.cli.sol.call.activities.span.absent",
                [("span_id", &span_id)],
            ));
        }
        Err(error) => return transport_error(locale, error),
    };
    render_record_body(&response, parsed.has_flag("--json"))
}

#[must_use]
pub fn mute(ctx: CommandContext<'_>) -> CommandOutput {
    set_mute_state(ctx, "mute")
}

#[must_use]
pub fn unmute(ctx: CommandContext<'_>) -> CommandOutput {
    set_mute_state(ctx, "unmute")
}

fn set_mute_state(ctx: CommandContext<'_>, verb: &str) -> CommandOutput {
    let (parsed, locale) = match parse_args(
        ctx,
        &[
            ("--facet", Some("-f")),
            ("--day", Some("-d")),
            ("--reason", None),
        ],
        &["--json"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(span_id) = parsed.positionals.first() else {
        return stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.span.missing",
            &[],
        ));
    };
    let facet = match resolve_facet(locale, parsed.value("--facet"), ctx) {
        Ok(value) => value,
        Err(output) => return output,
    };
    let day = match resolve_day(locale, parsed.value("--day"), ctx) {
        Ok(value) => value,
        Err(output) => return output,
    };
    let mut request_body = Map::new();
    request_body.insert(
        "reason".to_string(),
        parsed
            .value("--reason")
            .map(|reason| Value::String(reason.to_string()))
            .unwrap_or(Value::Null),
    );
    let response = match request_json(
        ctx,
        HttpMethod::Post,
        &format!("/app/activities/api/day/{day}/record/{span_id}/{verb}"),
        vec![QueryParam::single("facet", facet)],
        Some(Value::Object(request_body)),
    ) {
        Ok(body) => body,
        Err(error) if is_reason(&error, ACTIVITY_NOT_FOUND) => {
            return stderr(display_for(
                locale,
                "journal.cli.sol.call.activities.span.absent",
                [("span_id", &span_id)],
            ));
        }
        Err(error) => return transport_error(locale, error),
    };
    render_record_body(&response, parsed.has_flag("--json"))
}

#[derive(Debug, Default)]
struct ParsedArgs {
    positionals: Vec<String>,
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl ParsedArgs {
    fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn has_flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

fn parse_args(
    ctx: CommandContext<'_>,
    options: &[(&str, Option<&str>)],
    flags: &[&str],
) -> Result<(ParsedArgs, &'static str), String> {
    let args = ctx.args;
    let mut aliases = BTreeMap::new();
    for (long, short) in options {
        aliases.insert((*long).to_string(), (*long).to_string());
        if let Some(short) = short {
            aliases.insert((*short).to_string(), (*long).to_string());
        }
    }
    let locale = ctx.human_locale_matching(|option| aliases.contains_key(option));
    let flag_set = flags.iter().copied().collect::<BTreeSet<_>>();
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if let Some((name, value)) = token.split_once('=')
            && name.starts_with("--")
            && let Some(canonical) = aliases.get(name)
        {
            parsed.values.insert(canonical.clone(), value.to_string());
        } else if let Some(canonical) = aliases.get(token) {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(crate::args::option_argument_for(locale, token));
            };
            parsed.values.insert(canonical.clone(), value.clone());
        } else if flag_set.contains(token.as_str()) {
            parsed.flags.insert(token.clone());
        } else if token.starts_with('-') {
            return Err(crate::args::unknown_option_for(locale, token));
        } else {
            parsed.positionals.push(token.clone());
        }
        index += 1;
    }
    Ok((parsed, locale))
}

fn env_value(ctx: CommandContext<'_>, name: &str) -> Option<String> {
    ctx.env
        .get(name)
        .filter(|value| !value.is_empty())
        .map(String::to_string)
}

fn resolve_day(
    locale: &str,
    arg: Option<&str>,
    ctx: CommandContext<'_>,
) -> Result<String, CommandOutput> {
    if let Some(arg) = arg {
        return Ok(arg.to_string());
    }
    if let Some(value) = env_value(ctx, "SOL_DAY") {
        return Ok(value);
    }
    Err(stderr(render_for(
        locale,
        "journal.cli.sol.call.activities.day.missing",
        &[],
    )))
}

fn resolve_day_or_today(arg: Option<&str>, ctx: CommandContext<'_>) -> Result<String, ClientError> {
    match arg
        .map(str::to_string)
        .or_else(|| env_value(ctx, "SOL_DAY"))
    {
        Some(day) => Ok(day),
        None => journal_today(ctx),
    }
}

fn resolve_facet(
    locale: &str,
    arg: Option<&str>,
    ctx: CommandContext<'_>,
) -> Result<String, CommandOutput> {
    if let Some(arg) = arg {
        return Ok(arg.to_string());
    }
    if let Some(value) = env_value(ctx, "SOL_FACET") {
        return Ok(value);
    }
    Err(stderr(render_for(
        locale,
        "journal.cli.sol.call.activities.facet.missing",
        &[],
    )))
}

fn parse_day(locale: &str, value: &str) -> Result<Date, CommandOutput> {
    solstone_core_time::parse_day(value).ok_or_else(|| {
        stderr(display_for(
            locale,
            "journal.cli.sol.call.activities.day.invalid",
            [("value", &value)],
        ))
    })
}

fn iter_days(locale: &str, start_day: &str, end_day: &str) -> Result<Vec<String>, CommandOutput> {
    let start = parse_day(locale, start_day)?;
    let end = parse_day(locale, end_day)?;
    if end < start {
        return Err(stderr(display_for(
            locale,
            "journal.cli.sol.call.activities.range.reversed",
            [("end_day", &end_day), ("start_day", &start_day)],
        )));
    }
    let mut days = Vec::new();
    let mut cursor = start;
    while cursor <= end {
        days.push(solstone_core_time::day_key(cursor));
        let Ok(next) = cursor.tomorrow() else {
            break;
        };
        cursor = next;
    }
    Ok(days)
}

fn valid_segment_key(segment: &str) -> bool {
    let Some((time_part, len_part)) = segment.split_once('_') else {
        return false;
    };
    if time_part.len() != 6
        || !time_part.chars().all(|ch| ch.is_ascii_digit())
        || len_part.is_empty()
        || !len_part.chars().all(|ch| ch.is_ascii_digit())
    {
        return false;
    }
    let hour = time_part[0..2].parse::<u8>().unwrap_or(24);
    let minute = time_part[2..4].parse::<u8>().unwrap_or(60);
    let second = time_part[4..6].parse::<u8>().unwrap_or(60);
    hour <= 23 && minute <= 59 && second <= 59
}

fn stdin_create_payload(
    locale: &str,
    raw: &str,
    source: &str,
) -> Result<(Map<String, Value>, String), CommandOutput> {
    let payload = read_stdin_json(locale, raw, false)?;
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if title.is_empty() {
        return Err(stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.title.required",
            &[],
        )));
    }
    let activity = payload
        .get("activity")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if activity.is_empty() {
        return Err(stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.kind.required",
            &[],
        )));
    }
    let mut body = Map::new();
    body.insert("title".to_string(), Value::String(title));
    body.insert("activity".to_string(), Value::String(activity.clone()));
    body.insert("source".to_string(), Value::String(source.to_string()));
    if let Some(value) = payload.get("description") {
        body.insert("description".to_string(), value.clone());
    }
    if let Some(value) = payload.get("details") {
        body.insert("details".to_string(), value.clone());
    }
    if let Some(value) = payload.get("participation") {
        body.insert(
            "participation".to_string(),
            Value::Array(validate_participation(locale, value)?),
        );
    }
    Ok((body, activity))
}

fn stdin_update_patch(locale: &str, raw: &str) -> Result<Map<String, Value>, CommandOutput> {
    let payload = read_stdin_json(locale, raw, true)?;
    let mut patch = Map::new();
    let mut extra = Vec::new();
    for (key, value) in payload {
        if MUTABLE_FIELDS.contains(&key.as_str()) {
            patch.insert(key, value);
        } else {
            extra.push(key);
        }
    }
    if !extra.is_empty() {
        extra.sort();
        return Err(stderr(display_for(
            locale,
            "journal.cli.sol.call.activities.patch.disallowed",
            [("fields", &extra.join(", "))],
        )));
    }
    Ok(patch)
}

fn read_stdin_json(
    locale: &str,
    raw: &str,
    allow_empty: bool,
) -> Result<Map<String, Value>, CommandOutput> {
    let raw = raw.trim();
    if raw.is_empty() {
        if allow_empty {
            return Ok(Map::new());
        }
        return Err(stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.stdin.object",
            &[],
        )));
    }
    let payload = serde_json::from_str::<Value>(raw).map_err(|error| {
        stderr(display_for(
            locale,
            "journal.cli.sol.call.activities.stdin.json",
            [("error", &error)],
        ))
    })?;
    match payload {
        Value::Object(object) => Ok(object),
        _ => Err(stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.stdin.object",
            &[],
        ))),
    }
}

fn validate_participation(locale: &str, value: &Value) -> Result<Vec<Value>, CommandOutput> {
    let Some(entries) = value.as_array() else {
        return Err(stderr(render_for(
            locale,
            "journal.cli.sol.call.activities.participation.array",
            &[],
        )));
    };
    let mut cleaned = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(object) = entry.as_object() else {
            return Err(stderr(display_for(
                locale,
                "journal.cli.sol.call.activities.participation.object",
                [("index", &index)],
            )));
        };
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            return Err(stderr(display_for(
                locale,
                "journal.cli.sol.call.activities.participation.name",
                [("index", &index)],
            )));
        }
        let role = object.get("role").and_then(Value::as_str);
        if !matches!(role, Some("attendee" | "mentioned" | "visible" | "heard")) {
            return Err(stderr(display_for(
                locale,
                "journal.cli.sol.call.activities.participation.role",
                [
                    ("index", &index),
                    (
                        "role",
                        &object
                            .get("role")
                            .map_or("null".to_string(), json_value_to_python_string),
                    ),
                ],
            )));
        }
        let source = object.get("source").and_then(Value::as_str);
        if !matches!(
            source,
            Some("voice" | "speaker_label" | "transcript" | "screen" | "other")
        ) {
            return Err(stderr(display_for(
                locale,
                "journal.cli.sol.call.activities.participation.source",
                [
                    ("index", &index),
                    (
                        "source",
                        &object
                            .get("source")
                            .map_or("null".to_string(), json_value_to_python_string),
                    ),
                ],
            )));
        }
        match object.get("confidence") {
            Some(Value::Number(_)) => {}
            _ => {
                return Err(stderr(display_for(
                    locale,
                    "journal.cli.sol.call.activities.participation.confidence",
                    [("index", &index)],
                )));
            }
        }
        if !matches!(object.get("context"), Some(Value::String(_))) {
            return Err(stderr(display_for(
                locale,
                "journal.cli.sol.call.activities.participation.context",
                [("index", &index)],
            )));
        }
        let mut cleaned_entry = Map::new();
        for (key, item) in object {
            if key != "entity_id" {
                cleaned_entry.insert(key.clone(), item.clone());
            }
        }
        cleaned_entry.insert("name".to_string(), Value::String(name));
        cleaned_entry.insert("role".to_string(), Value::String(role.unwrap().to_string()));
        cleaned_entry.insert(
            "source".to_string(),
            Value::String(source.unwrap().to_string()),
        );
        cleaned.push(Value::Object(cleaned_entry));
    }
    Ok(cleaned)
}

fn json_value_to_python_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "None".to_string(),
        _ => value.to_string(),
    }
}

fn insert_optional_string(body: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        body.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn item_matches(
    item: &Value,
    activity: Option<&str>,
    entity: Option<&str>,
    source: Option<&str>,
) -> bool {
    let Some(record) = item.get("record").and_then(Value::as_object) else {
        return false;
    };
    if let Some(activity) = activity
        && record.get("activity").and_then(Value::as_str) != Some(activity)
    {
        return false;
    }
    if let Some(source) = source
        && record.get("source").and_then(Value::as_str) != Some(source)
    {
        return false;
    }
    if let Some(entity_query) = entity {
        let active_entities = record
            .get("active_entities")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !active_entities
            .iter()
            .any(|value| value_to_string(value).to_lowercase().contains(entity_query))
        {
            return false;
        }
    }
    true
}

fn sort_item_key(item: &Value) -> (String, String, i64, String) {
    let Some(record) = item.get("record").and_then(Value::as_object) else {
        return (String::new(), String::new(), 0, String::new());
    };
    (
        record.get("day").map_or_else(String::new, value_to_string),
        record
            .get("facet")
            .map_or_else(String::new, value_to_string),
        record
            .get("created_at")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        record.get("id").map_or_else(String::new, value_to_string),
    )
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => value.to_string(),
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
        headers: vec![],
        policy: TimeoutPolicy::Api,
    })?;
    decode_response(&response)
}

fn is_reason(error: &ClientError, reason: &str) -> bool {
    error.reason_code() == Some(reason)
}

fn render_record_body(body: &Value, json_output: bool) -> CommandOutput {
    crate::agent_output::literal_output(if json_output {
        stdout_json(body.get("record").unwrap_or(&Value::Null))
    } else {
        stdout_line(body.get("markdown").and_then(Value::as_str).unwrap_or(""))
    })
}

fn stdout_json(value: &Value) -> CommandOutput {
    CommandOutput::success(format!(
        "{}\n",
        serde_json::to_string_pretty(value).expect("JSON output should serialize")
    ))
}

fn stdout_line(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::success(format!("{}\n", value.as_ref()))
}

fn stderr(value: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("{}\n", value.as_ref()), 1)
}

fn transport_error(locale: &str, error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => stderr(crate::error::service_down_for(locale)),
        _ => stderr(error.message_for(locale)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::command::{CommandContext, CommandOutput};
    use crate::seam::{ExpectedHttpCall, ScriptedHttpTransport};
    use crate::transport::{ApiRequest, HttpMethod, QueryParam, TimeoutPolicy};

    #[test]
    fn list_unreachable_renders_service_down_message() {
        let args: Vec<String> = vec![];
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![
            crate::command::shell_today_call("20260723"),
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Get,
                    path: "/app/activities/api/day/20260723/records".to_string(),
                    params: vec![QueryParam::single("include_hidden", "0")],
                    json: None,
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Err(ClientError::unreachable(Some(
                    "io: Connection refused".to_string(),
                ))),
            },
        ]);
        let output = list(CommandContext {
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

        assert_eq!(
            output,
            CommandOutput {
                stdout: String::new(),
                stderr: "journal isn't running. start it with 'solstone journal up' and retry.\n"
                    .to_string(),
                exit: 1,
            }
        );
        transport.assert_done();
    }

    #[test]
    fn transport_error_renderer_preserves_timeout_message() {
        assert_eq!(
            transport_error(
                solstone_core_cli_copy::ENGLISH,
                ClientError::unreachable(Some("io: x".to_string()))
            ),
            CommandOutput {
                stdout: String::new(),
                stderr: "journal isn't running. start it with 'solstone journal up' and retry.\n"
                    .to_string(),
                exit: 1,
            }
        );
        assert_eq!(
            transport_error(
                solstone_core_cli_copy::ENGLISH,
                ClientError::timeout(Some("x".to_string()))
            ),
            CommandOutput {
                stdout: String::new(),
                stderr: "The journal didn't answer in time.\n".to_string(),
                exit: 1,
            }
        );
    }

    #[test]
    fn validate_participation_accepts_heard_and_enumerates_roles_on_error() {
        let valid_entry = serde_json::json!([{
            "name": "Iris",
            "role": "heard",
            "source": "voice",
            "confidence": 0.9,
            "context": "Spoke during conversation",
        }]);
        let res = validate_participation(solstone_core_cli_copy::ENGLISH, &valid_entry);
        assert!(res.is_ok());

        let invalid_entry = serde_json::json!([{
            "name": "Iris",
            "role": "invalid_role",
            "source": "voice",
            "confidence": 0.9,
            "context": "Test",
        }]);
        let err =
            validate_participation(solstone_core_cli_copy::ENGLISH, &invalid_entry).unwrap_err();
        let stderr_str = err.stderr;
        assert!(stderr_str.contains("attendee"));
        assert!(stderr_str.contains("mentioned"));
        assert!(stderr_str.contains("visible"));
        assert!(stderr_str.contains("heard"));
    }
}
