// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::ClientError;
use crate::transport::{
    ApiRequest, FormField, HttpMethod, MultipartFile, QueryParam, TimeoutPolicy, UploadRequest,
};

const SUPPORT_FALLBACK_1: &str =
    "I couldn't reach support because solstone isn't reachable right now.";
const SUPPORT_FALLBACK_2: &str = "To file a support ticket, visit https://support.solstone.app";
const DRAFT_NOTICE: &str =
    "(Draft not captured — solstone wasn't reachable to save it for review.)";
const FEEDBACK_SUBJECT: &str = "feedback";
const CLOSE_PREVIEW: &str = "Closing this removes the ticket from solstone support's open list; only a minimal closed record is kept.";
const RESOLVED_PREVIEW: &str = "Accepting this resolution removes the ticket from solstone support's open list; only a minimal closed record is kept.";
const STILL_NEED_HELP_PREVIEW: &str = "This tells solstone support the proposed resolution did not work, cancels the pending close, and keeps the ticket open.";

fn generate_action_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let entropy = now ^ u128::from(std::process::id());
    format!("cact1_{entropy:x}")
}

fn resolve_action_id(resume: Option<&str>) -> String {
    resume
        .map(str::to_string)
        .unwrap_or_else(generate_action_id)
}

#[must_use]
pub fn register(ctx: CommandContext<'_>) -> CommandOutput {
    let client = SupportClient::new(ctx);
    if let Err(output) = client.check_enabled() {
        return output;
    }
    match client.request(HttpMethod::Post, "/app/support/api/register", vec![], None) {
        Ok(result) => stdout_line(format!(
            "Registered as: {}",
            display_or(&result["handle"], "?")
        )),
        Err(error) => support_error(error),
    }
}

#[must_use]
pub fn search(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &[], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(query) = parsed.positionals.first() else {
        return stderr("Error: missing argument 'QUERY'.");
    };
    let client = SupportClient::new(ctx);
    if let Err(output) = client.check_enabled() {
        return output;
    }
    let articles = match client.request(
        HttpMethod::Get,
        "/app/support/api/articles",
        vec![QueryParam::single("q", query)],
        None,
    ) {
        Ok(value) => value,
        Err(error) => return support_error(error),
    };
    let items = articles.as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return stdout_line("No articles found.");
    }
    let mut lines = Vec::new();
    for article in &items {
        lines.push(format!(
            "  [{}] {}",
            display_or(&article["slug"], "?"),
            display_or(&article["title"], "Untitled")
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "{} article(s) found. Use `solstone call support article <slug>` to read.",
        items.len()
    ));
    stdout(lines)
}

#[must_use]
pub fn article(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--json"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(slug) = parsed.positionals.first() else {
        return stderr("Error: missing argument 'SLUG'.");
    };
    let client = SupportClient::new(ctx);
    if let Err(output) = client.check_enabled() {
        return output;
    }
    let data = match client.request(
        HttpMethod::Get,
        &format!("/app/support/api/articles/{slug}"),
        vec![],
        None,
    ) {
        Ok(value) => value,
        Err(error) => return support_error(error),
    };
    if parsed.has_flag("--json") {
        stdout_json(&data)
    } else {
        stdout(vec![
            format!("# {}\n", display_or(&data["title"], "Untitled")),
            display_or(&data["content"], "(no content)"),
        ])
    }
}

#[must_use]
pub fn create(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            ("--subject", Some("-s")),
            ("--description", Some("-d")),
            ("--product", Some("-p")),
            ("--severity", None),
            ("--category", None),
            ("--resume", None),
        ],
        &["--skip-kb", "--submit", "--yes", "-y", "--anonymous"],
        &[],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(subject) = parsed.value("--subject") else {
        return stderr("Error: option --subject is required.");
    };
    let Some(description) = parsed.value("--description") else {
        return stderr("Error: option --description is required.");
    };
    if parsed.value("--resume").is_some() && !parsed.has_flag("--submit") {
        return stderr("Error: --resume requires --submit.");
    }
    let product = parsed.value("--product").unwrap_or("solstone");
    let severity = parsed.value("--severity").unwrap_or("medium");
    let category = parsed.value("--category");
    let anonymous = parsed.has_flag("--anonymous");
    let client = SupportClient::new(ctx);
    let config = match client.check_enabled() {
        Ok(config) => config,
        Err(output) => return output,
    };
    if !parsed.has_flag("--submit") {
        let diagnostics = match client.request(
            HttpMethod::Get,
            "/app/support/api/diagnostics",
            vec![],
            None,
        ) {
            Ok(value) => value,
            Err(error) => return support_error(error),
        };
        let mut out = Output::default();
        print_dry_run_preview(
            &mut out,
            DryRunPreview {
                subject,
                product,
                severity,
                category,
                body: description,
                diagnostics: &diagnostics,
                portal_url: config["portal_url"].as_str().unwrap_or_default(),
            },
        );
        let payload = ticket_payload(
            subject,
            description,
            product,
            severity,
            category,
            &diagnostics,
            anonymous,
        );
        capture_draft(&client, &mut out, "create", payload, Some(diagnostics));
        return out.finish(0);
    }

    let mut out = Output::default();
    let mut confirm_lines = ctx.stdin.lines();
    if !parsed.has_flag("--skip-kb") {
        out.stdout.push("Searching knowledge base...".to_string());
        let articles = match client.request(
            HttpMethod::Get,
            "/app/support/api/articles",
            vec![QueryParam::single("q", subject)],
            None,
        ) {
            Ok(value) => value.as_array().cloned().unwrap_or_default(),
            Err(error) => return out.finish_support_error(error),
        };
        if !articles.is_empty() {
            out.stdout.push(String::new());
            out.stdout
                .push(format!("Found {} related article(s):", articles.len()));
            for article in &articles {
                out.stdout.push(format!(
                    "  [{}] {}",
                    display_or(&article["slug"], "?"),
                    display_or(&article["title"], "")
                ));
            }
            out.stdout.push(String::new());
            out.stdout.push(
                "These may answer your question. Use `solstone call support article <slug>` to read."
                    .to_string(),
            );
            if !parsed.has_flag("--yes") {
                match confirm(&mut out, &mut confirm_lines, "Still want to file a ticket?") {
                    Ok(true) => {}
                    Ok(false) => {
                        out.stdout.push("Cancelled.".to_string());
                        return out.finish(0);
                    }
                    Err(()) => return out.finish(1),
                }
            }
        }
    }

    let diagnostics = match client.request(
        HttpMethod::Get,
        "/app/support/api/diagnostics",
        vec![],
        None,
    ) {
        Ok(value) => value,
        Err(error) => return out.finish_support_error(error),
    };
    out.stdout.push(String::new());
    out.stdout.push("--- Ticket Draft ---".to_string());
    out.stdout.push(format!("Subject:     {subject}"));
    out.stdout.push(format!("Product:     {product}"));
    out.stdout.push(format!("Severity:    {severity}"));
    if let Some(category) = category {
        out.stdout.push(format!("Category:    {category}"));
    }
    out.stdout.push(format!("Description: {description}"));
    out.stdout.push(String::new());
    out.stdout.push(format!(
        "Diagnostic data ({} bytes):",
        json_compact(&diagnostics).len()
    ));
    out.stdout.push(json_pretty(&diagnostics));
    out.stdout.push("--- End Draft ---".to_string());
    out.stdout.push(String::new());
    if !parsed.has_flag("--yes") {
        match confirm(&mut out, &mut confirm_lines, "Submit this ticket?") {
            Ok(true) => {}
            Ok(false) => {
                out.stdout.push("Cancelled — nothing was sent.".to_string());
                return out.finish(0);
            }
            Err(()) => return out.finish(1),
        }
    }
    let action_id = resolve_action_id(parsed.value("--resume"));
    out.stdout.push(format!("Action: {action_id}"));
    let payload = ticket_payload(
        subject,
        description,
        product,
        severity,
        category,
        &diagnostics,
        anonymous,
    );
    let result = match client.request_mutation(
        HttpMethod::Post,
        "/app/support/api/tickets",
        vec![],
        Some(Value::Object(payload)),
        &action_id,
    ) {
        Ok(value) => value,
        Err(error) => return out.finish_support_error(error),
    };
    out.stdout.push(format!(
        "Ticket created: #{}",
        display_or(&result["id"], "?")
    ));
    out.finish(0)
}

#[must_use]
pub fn list(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--status", None)], &["--json"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let client = SupportClient::new(ctx);
    if let Err(output) = client.check_enabled() {
        return output;
    }
    let params = parsed
        .value("--status")
        .map(|status| vec![QueryParam::single("status", status)])
        .unwrap_or_default();
    let tickets = match client.request(HttpMethod::Get, "/app/support/api/tickets", params, None) {
        Ok(value) => value,
        Err(error) => return support_error(error),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&tickets);
    }
    let items = tickets.as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return stdout_line("No tickets found.");
    }
    let mut lines = Vec::new();
    for ticket in &items {
        lines.push(format!(
            "  #{:>4}  [{:<12}] {}",
            display_or(&ticket["id"], "?"),
            display_or(&ticket["status"], "?"),
            display_or(&ticket["subject"], "untitled")
        ));
    }
    lines.push(String::new());
    lines.push(format!("{} ticket(s).", items.len()));
    stdout(lines)
}

#[must_use]
pub fn show(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--json"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(ticket_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument 'TICKET_ID'.");
    };
    let client = SupportClient::new(ctx);
    if let Err(output) = client.check_enabled() {
        return output;
    }
    let data = match client.request(
        HttpMethod::Get,
        &format!("/app/support/api/tickets/{ticket_id}"),
        vec![],
        None,
    ) {
        Ok(value) => value,
        Err(error) => return support_error(error),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&data);
    }
    let display_ticket_id = display_or(&data["id"], "?");
    let mut lines = render_ticket_summary(&data, &display_ticket_id);
    if let Some(messages) = data["messages"].as_array()
        && !messages.is_empty()
    {
        lines.push(String::new());
        lines.push(format!("--- {} message(s) ---", messages.len()));
        for message in messages {
            lines.push(String::new());
            lines.push(format!(
                "[{}] {}",
                display_or(&message["handle"], "?"),
                display_or(&message["created_at"], "")
            ));
            lines.push(display_or(&message["content"], ""));
            if let Some(attachments) = message["attachments"].as_array() {
                for attachment in attachments {
                    let size = attachment["size_bytes"].as_u64().unwrap_or(0);
                    lines.push(format!(
                        "  📎 {} ({})",
                        display_or(&attachment["filename"], "?"),
                        size_string(size)
                    ));
                }
            }
        }
    }
    stdout(lines)
}

#[must_use]
pub fn reply(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--body", Some("-b")), ("--resume", None)],
        &["--submit", "--yes", "-y"],
        &["--no-submit"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(ticket_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument 'TICKET_ID'.");
    };
    let Some(body) = parsed.value("--body") else {
        return stderr("Error: option --body is required.");
    };
    if parsed.value("--resume").is_some() && !parsed.has_flag("--submit") {
        return stderr("Error: --resume requires --submit.");
    }
    let client = SupportClient::new(ctx);
    if let Err(output) = client.check_enabled() {
        return output;
    }
    let submit = !parsed.has_flag("--no-submit");
    if !submit {
        let mut out = Output::default();
        out.stdout.push(
            "DRY RUN — nothing was sent. Re-run with --submit to actually send this.".to_string(),
        );
        out.stdout
            .push(format!("Reply to ticket #{ticket_id}:\n{body}"));
        let mut payload = Map::new();
        payload.insert("ticket_id".to_string(), int_or_string(ticket_id));
        payload.insert("content".to_string(), Value::String(body.to_string()));
        capture_draft(&client, &mut out, "reply", payload, None);
        return out.finish(0);
    }
    let mut out = Output::default();
    let mut confirm_lines = ctx.stdin.lines();
    if !parsed.has_flag("--yes") {
        out.stdout
            .push(format!("Reply to ticket #{ticket_id}:\n{body}\n"));
        match confirm(&mut out, &mut confirm_lines, "Send this reply?") {
            Ok(true) => {}
            Ok(false) => {
                out.stdout.push("Cancelled.".to_string());
                return out.finish(0);
            }
            Err(()) => return out.finish(1),
        }
    }
    let action_id = resolve_action_id(parsed.value("--resume"));
    out.stdout.push(format!("Action: {action_id}"));
    if let Err(error) = client.request_mutation(
        HttpMethod::Post,
        &format!("/app/support/api/tickets/{ticket_id}/reply"),
        vec![],
        Some(json!({"content": body})),
        &action_id,
    ) {
        return out.finish_support_error(error);
    }
    out.stdout
        .push(format!("Reply sent to ticket #{ticket_id}."));
    out.finish(0)
}

#[must_use]
pub fn attach(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--resume", None)],
        &["--submit", "--yes", "-y"],
        &["--no-submit"],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(ticket_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument 'TICKET_ID'.");
    };
    let files = parsed
        .positionals
        .iter()
        .skip(1)
        .cloned()
        .collect::<Vec<_>>();
    if files.is_empty() {
        return stderr("Error: missing argument 'FILES'.");
    }
    if parsed.value("--resume").is_some() && !parsed.has_flag("--submit") {
        return stderr("Error: --resume requires --submit.");
    }
    if let Some(file) = first_unreadable_file(ctx, &files) {
        return CommandOutput::failure(format!("Error: file is not readable: {file}\n"), 2);
    }
    let client = SupportClient::new(ctx);
    if let Err(output) = client.check_enabled() {
        return output;
    }
    let submit = !parsed.has_flag("--no-submit");
    if !submit {
        if files.len() > 1 {
            return stderr("Attach one file at a time when preparing a draft for review.");
        }
        let file = &files[0];
        let body = match read_file(ctx, file) {
            Ok(body) => body,
            Err(FileReadError::Missing) => return stderr(format!("Error: file not found: {file}")),
            Err(FileReadError::Unreadable) => {
                return CommandOutput::failure(format!("Error: file is not readable: {file}\n"), 2);
            }
        };
        let filename = file_name(file);
        if let Err(error) = client.upload(
            "/app/support/api/draft",
            vec![MultipartFile {
                field_name: "file".to_string(),
                filename: filename.clone(),
                content_type: None,
                body: body.clone(),
            }],
            vec![
                FormField {
                    name: "verb".to_string(),
                    value: "attach".to_string(),
                },
                FormField {
                    name: "ticket_id".to_string(),
                    value: ticket_id.to_string(),
                },
            ],
        ) {
            return match error {
                ClientError::Unreachable { .. } => support_unreachable(),
                other => stderr(other.detail().unwrap_or_else(|| other.message())),
            };
        }
        return stdout(vec![
            "DRY RUN — nothing was sent. Re-run without --no-submit to upload this.".to_string(),
            format!(
                "Attachment draft for ticket #{ticket_id}: {filename} ({})",
                size_string(body.len() as u64)
            ),
        ]);
    }
    let mut file_bodies = Vec::new();
    for file in &files {
        let body = match read_file(ctx, file) {
            Ok(body) => body,
            Err(FileReadError::Missing) => return stderr(format!("Error: file not found: {file}")),
            Err(FileReadError::Unreadable) => {
                return CommandOutput::failure(format!("Error: file is not readable: {file}\n"), 2);
            }
        };
        file_bodies.push((file.clone(), body));
    }
    let mut out = Output::default();
    out.stdout
        .push(format!("\n--- Attachment Review (ticket #{ticket_id}) ---"));
    for (file, body) in &file_bodies {
        out.stdout.push(format!(
            "  {}  ({})",
            file_name(file),
            size_string(body.len() as u64)
        ));
    }
    out.stdout.push("--- End Review ---\n".to_string());
    let mut confirm_lines = ctx.stdin.lines();
    if !parsed.has_flag("--yes") {
        match confirm(&mut out, &mut confirm_lines, "Upload these files?") {
            Ok(true) => {}
            Ok(false) => {
                out.stdout.push("Cancelled — nothing was sent.".to_string());
                return out.finish(0);
            }
            Err(()) => return out.finish(1),
        }
    }
    let action_id = resolve_action_id(parsed.value("--resume"));
    out.stdout.push(format!("Action: {action_id}"));
    let path = format!("/app/support/api/tickets/{ticket_id}/attachments");
    let multiple_files = file_bodies.len() > 1;
    let mut failed = false;
    for (index, (file, body)) in file_bodies.into_iter().enumerate() {
        let filename = file_name(&file);
        let data = if multiple_files {
            vec![FormField {
                name: "index".to_string(),
                value: index.to_string(),
            }]
        } else {
            vec![]
        };
        match client.upload_mutation(
            &path,
            vec![MultipartFile {
                field_name: "file".to_string(),
                filename: filename.clone(),
                content_type: None,
                body,
            }],
            data,
            &action_id,
        ) {
            Ok(result) => out.stdout.push(format!(
                "Attached: {filename} (id: {})",
                display_or(&result["id"], "?")
            )),
            Err(error) => {
                failed = true;
                out.stderr
                    .push(format!("Failed {filename}: {}", error.message()));
            }
        }
    }
    out.finish(if failed { 1 } else { 0 })
}

#[must_use]
pub fn feedback(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[
            ("--body", Some("-b")),
            ("--product", Some("-p")),
            ("--resume", None),
        ],
        &["--anonymous", "--submit", "--yes", "-y"],
        &[],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(body) = parsed.value("--body") else {
        return stderr("Error: option --body is required.");
    };
    if parsed.value("--resume").is_some() && !parsed.has_flag("--submit") {
        return stderr("Error: --resume requires --submit.");
    }
    let product = parsed.value("--product").unwrap_or("solstone");
    let anonymous = parsed.has_flag("--anonymous");
    let client = SupportClient::new(ctx);
    let config = match client.check_enabled() {
        Ok(config) => config,
        Err(output) => return output,
    };
    if !parsed.has_flag("--submit") {
        let diagnostics = match client.request(
            HttpMethod::Get,
            "/app/support/api/diagnostics",
            vec![],
            None,
        ) {
            Ok(value) => value,
            Err(error) => return support_error(error),
        };
        let mut out = Output::default();
        print_dry_run_preview(
            &mut out,
            DryRunPreview {
                subject: FEEDBACK_SUBJECT,
                product,
                severity: "low",
                category: Some("feedback"),
                body,
                diagnostics: &diagnostics,
                portal_url: config["portal_url"].as_str().unwrap_or_default(),
            },
        );
        let mut payload = Map::new();
        payload.insert("body".to_string(), Value::String(body.to_string()));
        payload.insert("product".to_string(), Value::String(product.to_string()));
        payload.insert("anonymous".to_string(), Value::Bool(anonymous));
        capture_draft(&client, &mut out, "feedback", payload, Some(diagnostics));
        return out.finish(0);
    }
    let mut out = Output::default();
    let mut confirm_lines = ctx.stdin.lines();
    if !parsed.has_flag("--yes") {
        out.stdout.push(format!("Feedback:\n{body}\n"));
        let anon_note = if anonymous { " (anonymous)" } else { "" };
        match confirm(
            &mut out,
            &mut confirm_lines,
            &format!("Submit this feedback{anon_note}?"),
        ) {
            Ok(true) => {}
            Ok(false) => {
                out.stdout.push("Cancelled.".to_string());
                return out.finish(0);
            }
            Err(()) => return out.finish(1),
        }
    }
    let action_id = resolve_action_id(parsed.value("--resume"));
    out.stdout.push(format!("Action: {action_id}"));
    let result = match client.request_mutation(
        HttpMethod::Post,
        "/app/support/api/feedback",
        vec![],
        Some(json!({"body": body, "product": product, "anonymous": anonymous})),
        &action_id,
    ) {
        Ok(value) => value,
        Err(error) => return out.finish_support_error(error),
    };
    out.stdout.push(format!(
        "Feedback submitted: #{}",
        display_or(&result["id"], "?")
    ));
    out.finish(0)
}

#[must_use]
pub fn close(ctx: CommandContext<'_>) -> CommandOutput {
    lifecycle_mutation(
        ctx,
        "close",
        "Close ticket",
        CLOSE_PREVIEW,
        "/close",
        "Close ticket #{ticket_id}? This can't be undone from here.",
        true,
    )
}

#[must_use]
pub fn resolved(ctx: CommandContext<'_>) -> CommandOutput {
    lifecycle_mutation(
        ctx,
        "resolved",
        "Accept the proposed resolution and close ticket",
        RESOLVED_PREVIEW,
        "/resolution/confirm",
        "Accept the proposed resolution and close ticket #{ticket_id}? This can't be undone from here.",
        true,
    )
}

#[must_use]
pub fn still_need_help(ctx: CommandContext<'_>) -> CommandOutput {
    lifecycle_mutation(
        ctx,
        "still_need_help",
        "Keep ticket open",
        STILL_NEED_HELP_PREVIEW,
        "/resolution/still-need-help",
        "Let solstone support know you still need help with ticket #{ticket_id}? This cancels the pending close.",
        false,
    )
}

fn lifecycle_mutation(
    ctx: CommandContext<'_>,
    verb: &str,
    preview_title: &str,
    preview: &str,
    route_suffix: &str,
    prompt_template: &str,
    tombstone: bool,
) -> CommandOutput {
    let parsed = match parse_args(
        ctx.args,
        &[("--resume", None)],
        &["--submit", "--yes", "-y"],
        &[],
    ) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let Some(ticket_id) = parsed.positionals.first() else {
        return stderr("Error: missing argument 'TICKET_ID'.");
    };
    if parsed.value("--resume").is_some() && !parsed.has_flag("--submit") {
        return stderr("Error: --resume requires --submit.");
    }
    let client = SupportClient::new(ctx);
    if let Err(output) = client.check_enabled() {
        return output;
    }
    if !parsed.has_flag("--submit") {
        let mut out = Output::default();
        out.stdout.push(
            "DRY RUN — nothing was sent. Re-run with --submit to actually send this.".to_string(),
        );
        out.stdout.push(format!("{preview_title} #{ticket_id}."));
        out.stdout.push(preview.to_string());
        let mut payload = Map::new();
        payload.insert("ticket_id".to_string(), int_or_string(ticket_id));
        capture_draft(&client, &mut out, verb, payload, None);
        return out.finish(0);
    }

    let mut out = Output::default();
    let mut confirm_lines = ctx.stdin.lines();
    if !parsed.has_flag("--yes") {
        let prompt = prompt_template.replace("{ticket_id}", ticket_id);
        match confirm(&mut out, &mut confirm_lines, &prompt) {
            Ok(true) => {}
            Ok(false) => {
                out.stdout.push("Cancelled — nothing was sent.".to_string());
                return out.finish(0);
            }
            Err(()) => return out.finish(1),
        }
    }
    let action_id = resolve_action_id(parsed.value("--resume"));
    out.stdout.push(format!("Action: {action_id}"));
    let result = match client.request_mutation(
        HttpMethod::Post,
        &format!("/app/support/api/tickets/{ticket_id}{route_suffix}"),
        vec![],
        None,
        &action_id,
    ) {
        Ok(value) => value,
        Err(error) => return out.finish_support_error(error),
    };
    if tombstone {
        out.stdout.extend(render_tombstone(&result));
    } else {
        let display_ticket_id = if result["id"].is_null() {
            display_or(&result["ticket_id"], "?")
        } else {
            display_or(&result["id"], "?")
        };
        out.stdout
            .extend(render_ticket_summary(&result, &display_ticket_id));
    }
    out.finish(0)
}

#[must_use]
pub fn history(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[("--cursor", None)], &["--json"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let client = SupportClient::new(ctx);
    if let Err(output) = client.check_enabled() {
        return output;
    }
    let params = parsed
        .value("--cursor")
        .map(|cursor| vec![QueryParam::single("cursor", cursor)])
        .unwrap_or_default();
    let data = match client.request(
        HttpMethod::Get,
        "/app/support/api/tickets/closed",
        params,
        None,
    ) {
        Ok(value) => value,
        Err(error) => return support_error(error),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&data);
    }
    let tickets = data["tickets"].as_array().cloned().unwrap_or_default();
    let mut lines = Vec::new();
    for ticket in &tickets {
        lines.extend(render_tombstone(ticket));
        lines.push(String::new());
    }
    if let Some(cursor) = data["next_cursor"].as_str() {
        lines.push(format!(
            "Run `solstone call support history --cursor {cursor}` for more."
        ));
    } else {
        lines.push("No more closed tickets.".to_string());
    }
    stdout(lines)
}

#[must_use]
pub fn announcements(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--json"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let client = SupportClient::new(ctx);
    if let Err(output) = client.check_enabled() {
        return output;
    }
    let items = match client.request(
        HttpMethod::Get,
        "/app/support/api/announcements",
        vec![],
        None,
    ) {
        Ok(value) => value,
        Err(error) => return support_error(error),
    };
    if parsed.has_flag("--json") {
        return stdout_json(&items);
    }
    let items = items.as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return stdout_line("No active announcements.");
    }
    let mut lines = Vec::new();
    for item in &items {
        let icon = match item["type"].as_str().unwrap_or_default() {
            "known-issue" => "⚠️",
            "maintenance" => "🔧",
            _ => "📢",
        };
        lines.push(format!(
            "  {icon} {}",
            display_or(&item["title"], "Untitled")
        ));
        if let Some(content) = item["content"].as_str()
            && !content.is_empty()
        {
            lines.push(format!(
                "     {}",
                content.chars().take(120).collect::<String>()
            ));
        }
    }
    lines.push(String::new());
    lines.push(format!("{} announcement(s).", items.len()));
    stdout(lines)
}

#[must_use]
pub fn diagnose(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args, &[], &["--json"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return stderr(error),
    };
    let client = SupportClient::new(ctx);
    match client.request(
        HttpMethod::Get,
        "/app/support/api/diagnostics",
        vec![],
        None,
    ) {
        Ok(data) if parsed.has_flag("--json") => stdout_json(&data),
        Ok(data) => stdout(render_diagnostics(&data)),
        Err(ClientError::Unreachable { .. }) => {
            let identity = ctx
                .build_identity
                .and_then(|provider| provider.build_identity(Path::new("")))
                .unwrap_or_else(default_local_identity);
            let mut output = if parsed.has_flag("--json") {
                Output {
                    stdout: vec![json_pretty(&identity)],
                    stderr: vec![],
                }
            } else {
                Output {
                    stdout: render_local_identity(&identity),
                    stderr: vec![],
                }
            };
            output.stderr.push(SUPPORT_FALLBACK_1.to_string());
            output.stderr.push(SUPPORT_FALLBACK_2.to_string());
            output.finish(1)
        }
        Err(error) => stderr(error.message()),
    }
}

struct SupportClient<'a> {
    ctx: CommandContext<'a>,
}

impl<'a> SupportClient<'a> {
    fn new(ctx: CommandContext<'a>) -> Self {
        Self { ctx }
    }

    fn check_enabled(&self) -> Result<Value, CommandOutput> {
        let config = self
            .request(HttpMethod::Get, "/app/support/api/config", vec![], None)
            .map_err(support_error)?;
        if !config["enabled"].as_bool().unwrap_or(false) {
            return Err(stderr("Support agent is disabled in settings."));
        }
        Ok(config)
    }

    fn request(
        &self,
        method: HttpMethod,
        path: &str,
        params: Vec<QueryParam>,
        json: Option<Value>,
    ) -> Result<Value, ClientError> {
        let response = self.ctx.transport.request(ApiRequest {
            method,
            path: path.to_string(),
            params,
            json,
            headers: vec![],
            policy: TimeoutPolicy::Api,
        })?;
        decode_response(&response)
    }

    fn request_mutation(
        &self,
        method: HttpMethod,
        path: &str,
        params: Vec<QueryParam>,
        json: Option<Value>,
        action_id: &str,
    ) -> Result<Value, ClientError> {
        let response = self.ctx.transport.request(ApiRequest {
            method,
            path: path.to_string(),
            params,
            json,
            headers: vec![("Idempotency-Key".to_string(), action_id.to_string())],
            policy: TimeoutPolicy::Api,
        })?;
        decode_response(&response)
    }

    fn upload(
        &self,
        path: &str,
        files: Vec<MultipartFile>,
        data: Vec<FormField>,
    ) -> Result<Value, ClientError> {
        let response = self.ctx.transport.upload(UploadRequest {
            path: path.to_string(),
            files,
            data,
            headers: vec![],
            boundary: None,
            policy: TimeoutPolicy::Upload,
        })?;
        decode_response(&response)
    }

    fn upload_mutation(
        &self,
        path: &str,
        files: Vec<MultipartFile>,
        data: Vec<FormField>,
        action_id: &str,
    ) -> Result<Value, ClientError> {
        let response = self.ctx.transport.upload(UploadRequest {
            path: path.to_string(),
            files,
            data,
            headers: vec![("Idempotency-Key".to_string(), action_id.to_string())],
            boundary: None,
            policy: TimeoutPolicy::Upload,
        })?;
        decode_response(&response)
    }
}

#[derive(Debug, Default)]
struct ParsedArgs {
    positionals: Vec<String>,
    values: Vec<(String, String)>,
    flags: Vec<String>,
}

impl ParsedArgs {
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| value.as_str())
    }

    fn has_flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }
}

fn parse_args(
    args: &[String],
    options: &[(&str, Option<&str>)],
    flags: &[&str],
    secondary_flags: &[&str],
) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if let Some((name, value)) = token.split_once('=')
            && canonical_option(name, options).is_some()
        {
            parsed.values.push((
                canonical_option(name, options)
                    .expect("checked")
                    .to_string(),
                value.to_string(),
            ));
        } else if let Some(canonical) = canonical_option(token, options) {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(format!("Error: option {token} requires an argument."));
            };
            parsed
                .values
                .push((canonical.to_string(), value.to_string()));
        } else if flags.contains(&token.as_str()) {
            let canonical = if token == "-y" { "--yes" } else { token };
            parsed.flags.push(canonical.to_string());
        } else if secondary_flags.contains(&token.as_str()) {
            parsed.flags.push(token.to_string());
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

struct DryRunPreview<'a> {
    subject: &'a str,
    product: &'a str,
    severity: &'a str,
    category: Option<&'a str>,
    body: &'a str,
    diagnostics: &'a Value,
    portal_url: &'a str,
}

fn print_dry_run_preview(out: &mut Output, preview: DryRunPreview<'_>) {
    let version = preview.diagnostics["version"].as_str().unwrap_or("unknown");
    let revision = preview.diagnostics["revision"].as_str().unwrap_or("none");
    out.stdout.push(
        "DRY RUN — nothing was sent. Re-run with --submit to actually file this.".to_string(),
    );
    out.stdout.push(format!(
        "Build identity — version: {version}  revision: {revision}"
    ));
    out.stdout.push(String::new());
    out.stdout.push("--- Would send ---".to_string());
    out.stdout.push(format!("Subject:     {}", preview.subject));
    out.stdout.push(format!("Product:     {}", preview.product));
    out.stdout
        .push(format!("Severity:    {}", preview.severity));
    if let Some(category) = preview.category {
        out.stdout.push(format!("Category:    {category}"));
    }
    out.stdout.push(format!("Body:        {}", preview.body));
    out.stdout.push(String::new());
    out.stdout.push(format!(
        "user_context ({} bytes):",
        json_compact(preview.diagnostics).len()
    ));
    out.stdout.push(json_pretty(preview.diagnostics));
    out.stdout.push(String::new());
    out.stdout
        .push(format!("Would POST to: {}", preview.portal_url));
    out.stdout.push("--- End dry run ---".to_string());
}

fn capture_draft(
    client: &SupportClient<'_>,
    out: &mut Output,
    verb: &str,
    payload: Map<String, Value>,
    diagnostics_snapshot: Option<Value>,
) {
    let body = json!({
        "verb": verb,
        "payload": payload,
        "diagnostics_snapshot": diagnostics_snapshot,
    });
    if client
        .request(
            HttpMethod::Post,
            "/app/support/api/draft",
            vec![],
            Some(body),
        )
        .is_err()
    {
        out.stderr.push(DRAFT_NOTICE.to_string());
    }
}

fn ticket_payload(
    subject: &str,
    description: &str,
    product: &str,
    severity: &str,
    category: Option<&str>,
    diagnostics: &Value,
    anonymous: bool,
) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("subject".to_string(), Value::String(subject.to_string()));
    payload.insert(
        "description".to_string(),
        Value::String(description.to_string()),
    );
    payload.insert("product".to_string(), Value::String(product.to_string()));
    payload.insert("severity".to_string(), Value::String(severity.to_string()));
    payload.insert(
        "category".to_string(),
        category
            .map(|category| Value::String(category.to_string()))
            .unwrap_or(Value::Null),
    );
    payload.insert("user_context".to_string(), diagnostics.clone());
    payload.insert("auto_context".to_string(), Value::Bool(false));
    payload.insert("anonymous".to_string(), Value::Bool(anonymous));
    payload
}

fn confirm(out: &mut Output, lines: &mut std::str::Lines<'_>, prompt: &str) -> Result<bool, ()> {
    loop {
        let Some(answer) = lines.next() else {
            out.stdout.push(format!("{prompt} [y/N]:Aborted!\n "));
            return Err(());
        };
        out.stdout.push(format!("{prompt} [y/N]: {answer}"));
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "n" | "no" => return Ok(false),
            "y" | "yes" => return Ok(true),
            _ => out.stdout.push("Error: invalid input".to_string()),
        }
    }
}

enum FileReadError {
    Missing,
    Unreadable,
}

fn read_file(ctx: CommandContext<'_>, path: &str) -> Result<Vec<u8>, FileReadError> {
    let provider = ctx.files.ok_or(FileReadError::Missing)?;
    let path = Path::new(path);
    if !provider.exists(path) {
        return Err(FileReadError::Missing);
    }
    provider
        .read(path)
        .map_err(|_error| FileReadError::Unreadable)
}

fn first_unreadable_file(ctx: CommandContext<'_>, paths: &[String]) -> Option<String> {
    let provider = ctx.files?;
    paths.iter().find_map(|path| {
        let file_path = Path::new(path);
        if provider.exists(file_path) && provider.read(file_path).is_err() {
            Some(path.clone())
        } else {
            None
        }
    })
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn size_string(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MB", size as f64 / 1024.0 / 1024.0)
    } else if size >= 1024 {
        format!("{:.0} KB", size as f64 / 1024.0)
    } else {
        format!("{size} bytes")
    }
}

fn render_tombstone(data: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    for (label, key) in [
        ("Ticket", "ticket_id"),
        ("Status", "status"),
        ("Closed", "closed_at"),
        ("Close scheduled", "close_scheduled_at"),
        ("Reason", "reason_code"),
    ] {
        if !data[key].is_null() {
            lines.push(format!("{label}: {}", display_value(&data[key])));
        }
    }
    if lines.is_empty() {
        lines.push("Ticket lifecycle update completed.".to_string());
    }
    lines
}

fn render_ticket_summary(data: &Value, ticket_id: &str) -> Vec<String> {
    vec![
        format!(
            "# Ticket #{}: {}",
            ticket_id,
            display_or(&data["subject"], "")
        ),
        format!(
            "Status: {}  |  Severity: {}",
            display_or(&data["status"], "?"),
            display_or(&data["severity"], "?")
        ),
        format!("Created: {}", display_or(&data["created_at"], "?")),
        String::new(),
        display_or(&data["description"], ""),
    ]
}

fn render_diagnostics(data: &Value) -> Vec<String> {
    let mut lines = vec![
        "# Local Diagnostics\n".to_string(),
        format!("Version:  {}", display_or(&data["version"], "unknown")),
    ];
    let platform = &data["platform"];
    lines.push(format!(
        "Platform: {} {} ({})",
        display_or(&platform["system"], "?"),
        display_or(&platform["release"], ""),
        display_or(&platform["machine"], "")
    ));
    lines.push(format!(
        "Python:   {}",
        display_or(&platform["python"], "?")
    ));
    if let Some(services) = data["services"].as_object()
        && !services.is_empty()
    {
        lines.push(String::new());
        lines.push("Services:".to_string());
        let mut keys = services.keys().collect::<Vec<_>>();
        keys.sort();
        for name in keys {
            let status = display_or(&services[name], "");
            let icon = if status == "running" { "✓" } else { "✗" };
            lines.push(format!("  {icon} {name}: {status}"));
        }
    }
    if let Some(brain) = data.get("brain_health").and_then(Value::as_object)
        && let Some(brain_lines) = brain.get("lines").and_then(Value::as_array)
        && !brain_lines.is_empty()
    {
        lines.push(String::new());
        for line in brain_lines {
            lines.push(display_value(line));
        }
    }
    let errors = data["recent_errors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if errors.is_empty() {
        lines.push(String::new());
        lines.push("No recent errors.".to_string());
    } else {
        lines.push(String::new());
        lines.push(format!("Recent errors ({}):", errors.len()));
        for error in errors {
            let mut time = display_or(&error["time"], "");
            if !time.is_empty() && error["time_approximate"].as_bool().unwrap_or(false) {
                time = format!("~{time}");
            }
            let prefix = if time.is_empty() {
                String::new()
            } else {
                format!("{time} ")
            };
            let message = display_or(&error["message"], "");
            lines.push(format!(
                "  {prefix}[{}] {}",
                display_or(&error["service"], "?"),
                message.chars().take(100).collect::<String>()
            ));
        }
    }
    lines
}

fn render_local_identity(identity: &Value) -> Vec<String> {
    let platform = &identity["platform"];
    vec![
        "# Local Diagnostics\n".to_string(),
        format!(
            "Version:  {}",
            identity["version"].as_str().unwrap_or("unknown")
        ),
        format!(
            "Revision: {}",
            identity["revision"].as_str().unwrap_or("none")
        ),
        format!(
            "Platform: {} {} ({})",
            display_or(&platform["system"], "?"),
            display_or(&platform["release"], ""),
            display_or(&platform["machine"], "")
        ),
        format!("Python:   {}", display_or(&platform["python"], "?")),
    ]
}

fn default_local_identity() -> Value {
    json!({
        "version": null,
        "revision": null,
        "platform": {
            "system": "?",
            "release": "",
            "machine": "",
            "python": "?",
        },
    })
}

fn int_or_string(value: &str) -> Value {
    value
        .parse::<i64>()
        .map(Value::from)
        .unwrap_or_else(|_| Value::String(value.to_string()))
}

fn display_or(value: &Value, default: &str) -> String {
    if value.is_null() {
        default.to_string()
    } else {
        display_value(value)
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Null => "None".to_string(),
        _ => value.to_string(),
    }
}

fn stdout_json(value: &Value) -> CommandOutput {
    stdout(vec![json_pretty(value)])
}

fn json_pretty(value: &Value) -> String {
    ensure_ascii(&serde_json::to_string_pretty(value).expect("JSON output should serialize"))
}

fn json_compact(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(value) => serde_json::to_string(&ensure_ascii(value)).expect("string JSON"),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(json_compact)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(object) => format!(
            "{{{}}}",
            object
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    serde_json::to_string(&ensure_ascii(key)).expect("key JSON"),
                    json_compact(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn ensure_ascii(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii() {
            output.push(ch);
        } else {
            let codepoint = ch as u32;
            if codepoint <= 0xFFFF {
                output.push_str(&format!("\\u{codepoint:04x}"));
            } else {
                let adjusted = codepoint - 0x1_0000;
                let high = 0xD800 + (adjusted >> 10);
                let low = 0xDC00 + (adjusted & 0x3FF);
                output.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
            }
        }
    }
    output
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

fn support_error(error: ClientError) -> CommandOutput {
    match error {
        ClientError::Unreachable { .. } => support_unreachable(),
        other => stderr(other.message()),
    }
}

fn support_unreachable() -> CommandOutput {
    CommandOutput::failure(format!("{SUPPORT_FALLBACK_1}\n{SUPPORT_FALLBACK_2}\n"), 1)
}

#[derive(Debug, Default)]
struct Output {
    stdout: Vec<String>,
    stderr: Vec<String>,
}

impl Output {
    fn finish(self, exit: i32) -> CommandOutput {
        CommandOutput {
            stdout: if self.stdout.is_empty() {
                String::new()
            } else {
                format!("{}\n", self.stdout.join("\n"))
            },
            stderr: if self.stderr.is_empty() {
                String::new()
            } else {
                format!("{}\n", self.stderr.join("\n"))
            },
            exit,
        }
    }

    fn finish_support_error(mut self, error: ClientError) -> CommandOutput {
        match error {
            ClientError::Unreachable { .. } => {
                self.stderr.push(SUPPORT_FALLBACK_1.to_string());
                self.stderr.push(SUPPORT_FALLBACK_2.to_string());
            }
            other => self.stderr.push(other.message().to_string()),
        }
        self.finish(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::command::{CommandContext, CommandOutput};
    use crate::seam::{ExpectedHttpCall, ScriptedHttpTransport};
    use crate::transport::{ApiRequest, HttpMethod, TimeoutPolicy};

    #[test]
    fn search_unreachable_config_renders_support_fallback() {
        let args = vec!["kb".to_string()];
        let env = BTreeMap::new();
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Get,
                path: "/app/support/api/config".to_string(),
                params: vec![],
                json: None,
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Err(ClientError::unreachable(Some(
                "io: Connection refused".to_string(),
            ))),
        }]);
        let output = search(CommandContext {
            args: &args,
            env: &env,
            stdin: "",
            today: "20260723",
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
                stderr: "I couldn't reach support because solstone isn't reachable right now.\nTo file a support ticket, visit https://support.solstone.app\n".to_string(),
                exit: 1,
            }
        );
        transport.assert_done();
    }
}
