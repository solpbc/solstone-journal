// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_system_health::sanitize_for_terminal;

use crate::{BrainHealthState, TopState};

pub const LOG_FIXED_WIDTH: usize = 63;
pub const MAX_FRAME_WIDTH: usize = 512;
pub const MAX_FRAME_BYTES: usize = MAX_FRAME_WIDTH * 16 + 32_768;
pub const MAX_FRAME_OPS: usize = MAX_FRAME_BYTES;

/// Clock values for pure, deterministic frame rendering.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameSample {
    pub wall_seconds: f64,
    pub monotonic_seconds: f64,
}

/// Terminal structure is owned by the renderer; untrusted data never supplies
/// any of these strings.
pub trait TopStyle {
    fn home(&self) -> &str {
        "\x1b[H"
    }
    fn clear(&self) -> &str {
        "\x1b[2J"
    }
    fn bold(&self) -> &str {
        "\x1b[1m"
    }
    fn dim(&self) -> &str {
        "\x1b[2m"
    }
    fn red(&self) -> &str {
        "\x1b[31m"
    }
    fn green(&self) -> &str {
        "\x1b[32m"
    }
    fn yellow(&self) -> &str {
        "\x1b[33m"
    }
    fn cyan(&self) -> &str {
        "\x1b[36m"
    }
    fn magenta(&self) -> &str {
        "\x1b[35m"
    }
    fn inverse(&self) -> &str {
        "\x1b[7m"
    }
    fn normal(&self) -> &str {
        "\x1b[0m"
    }
    fn end_select(&self) -> &str {
        "\x1b[27m"
    }
}

/// Retained fixture/test style whose control sequences are token spellings.
pub struct PlainTopStyle;
impl TopStyle for PlainTopStyle {
    fn home(&self) -> &str {
        "<HOME>"
    }
    fn clear(&self) -> &str {
        "<CLEAR>"
    }
    fn bold(&self) -> &str {
        "<BOLD>"
    }
    fn dim(&self) -> &str {
        "<DIM>"
    }
    fn red(&self) -> &str {
        "<RED>"
    }
    fn green(&self) -> &str {
        "<GREEN>"
    }
    fn yellow(&self) -> &str {
        "<YELLOW>"
    }
    fn cyan(&self) -> &str {
        "<CYAN>"
    }
    fn magenta(&self) -> &str {
        "<MAGENTA>"
    }
    fn inverse(&self) -> &str {
        "<SELECT>"
    }
    fn normal(&self) -> &str {
        "<NORMAL>"
    }

    fn end_select(&self) -> &str {
        "</SELECT>"
    }
}

/// ANSI control style used by the live terminal loop.
pub struct AnsiTopStyle;
impl TopStyle for AnsiTopStyle {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustedToken {
    Home,
    Clear,
    Bold,
    Dim,
    Cyan,
    Green,
    Magenta,
    Red,
    Select,
    EndSelect,
    Yellow,
    Normal,
}

impl TrustedToken {
    #[must_use]
    pub fn spelling(self) -> &'static str {
        match self {
            Self::Home => "<HOME>",
            Self::Clear => "<CLEAR>",
            Self::Bold => "<BOLD>",
            Self::Dim => "<DIM>",
            Self::Cyan => "<CYAN>",
            Self::Green => "<GREEN>",
            Self::Magenta => "<MAGENTA>",
            Self::Red => "<RED>",
            Self::Select => "<SELECT>",
            Self::EndSelect => "</SELECT>",
            Self::Yellow => "<YELLOW>",
            Self::Normal => "<NORMAL>",
        }
    }

    fn ansi(self) -> &'static str {
        match self {
            Self::Home => "\x1b[H",
            Self::Clear => "\x1b[2J",
            Self::Bold => "\x1b[1m",
            Self::Dim => "\x1b[2m",
            Self::Cyan => "\x1b[36m",
            Self::Green => "\x1b[32m",
            Self::Magenta => "\x1b[35m",
            Self::Red => "\x1b[31m",
            Self::Select => "\x1b[7m",
            Self::EndSelect => "\x1b[27m",
            Self::Yellow => "\x1b[33m",
            Self::Normal => "\x1b[0m",
        }
    }
}

const TRUSTED_TOKENS: [TrustedToken; 12] = [
    TrustedToken::Home,
    TrustedToken::Clear,
    TrustedToken::Bold,
    TrustedToken::Dim,
    TrustedToken::Cyan,
    TrustedToken::Green,
    TrustedToken::Magenta,
    TrustedToken::Red,
    TrustedToken::Select,
    TrustedToken::EndSelect,
    TrustedToken::Yellow,
    TrustedToken::Normal,
];

/// One renderer-owned style command or one sanitized payload run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TopRenderOp {
    Style(TrustedToken),
    Print(String),
}

/// Render a bounded frame from state without ambient time, terminal, or I/O.
#[must_use]
pub fn render_frame(
    state: &TopState,
    frame: FrameSample,
    width: usize,
    style: &dyn TopStyle,
) -> String {
    let width = width.min(MAX_FRAME_WIDTH);
    let output = build_frame_text(state, frame, width, style);
    let rendered = transform_trusted_render(&output, width);
    if rendered.len() <= MAX_FRAME_BYTES {
        rendered
    } else {
        transform_trusted_render_capped(&output, width, MAX_FRAME_BYTES, style.normal())
    }
}

/// Typed live-path frame: style tokens stay structure, payload stays Print.
#[must_use]
pub fn render_ops(state: &TopState, frame: FrameSample, width: usize) -> Vec<TopRenderOp> {
    let width = width.min(MAX_FRAME_WIDTH);
    let output = build_frame_text(state, frame, width, &AnsiTopStyle);
    let ops = transform_trusted_render_to_ops(&output, width);
    if ops.len() <= MAX_FRAME_OPS {
        ops
    } else {
        transform_trusted_render_to_ops_capped(&output, width, MAX_FRAME_OPS)
    }
}

fn build_frame_text(
    state: &TopState,
    frame: FrameSample,
    width: usize,
    style: &dyn TopStyle,
) -> String {
    let mut output = String::with_capacity((width.saturating_mul(12)).min(MAX_FRAME_BYTES));
    output.push_str(style.home());
    output.push_str(style.clear());
    output.push('\n');
    output.push_str(style.bold());
    output.push_str(style.cyan());
    output.push_str(&center("solstone activity manager", width));
    output.push_str(style.normal());
    output.push_str("\n\n");
    if state.malformed_events > 0 {
        output.push_str(style.red());
        output.push_str(&format!(
            "  malformed events: {} ({})",
            state.malformed_events,
            state
                .last_malformed
                .as_ref()
                .map_or("unknown".to_owned(), ToString::to_string)
        ));
        output.push_str(style.normal());
        output.push('\n');
    }
    output.push_str(style.bold());
    output.push_str("  Service         PID      Uptime            MB      %  Last Log");
    output.push_str(style.normal());
    reconnecting(&mut output, state, &state.continuity.supervisor, style);
    output.push('\n');
    rule(&mut output, width);
    if state.services.is_empty() {
        output.push_str(style.dim());
        output.push_str("  (waiting for services)");
        output.push_str(style.normal());
        output.push('\n');
    } else {
        for (index, service) in state.services.iter().take(256).enumerate() {
            service_line(
                &mut output,
                service,
                index == state.selected,
                state,
                frame,
                width,
                style,
            );
        }
    }
    observe_section(&mut output, state, frame, width, style);
    think_section(&mut output, state, width, style);
    tasks_section(&mut output, state, frame, width, style);
    brain_section(&mut output, state, width, style);
    crashed_section(&mut output, state, style);
    rule(&mut output, width);
    footer(&mut output, state, style);
    output
}

fn tasks_section(
    out: &mut String,
    state: &TopState,
    frame: FrameSample,
    width: usize,
    style: &dyn TopStyle,
) {
    rule(out, width);
    out.push_str(style.bold());
    out.push_str("  Task            PID      Runtime           MB      %  Last Log");
    out.push_str(style.normal());
    reconnecting(out, state, &state.continuity.tasks, style);
    out.push('\n');
    let service_pids = state
        .services
        .iter()
        .filter_map(|service| service.get("pid").and_then(Value::as_u64))
        .collect::<std::collections::BTreeSet<_>>();
    let tasks = state
        .running_tasks
        .values()
        .filter(|task| {
            !task
                .get("pid")
                .and_then(Value::as_u64)
                .is_some_and(|pid| service_pids.contains(&pid))
        })
        .take(256)
        .collect::<Vec<_>>();
    let mut visible_commands = std::collections::BTreeSet::new();
    if tasks.is_empty() && state.finished_tasks.is_empty() {
        let queued = queued_commands(state, &visible_commands);
        out.push_str(style.dim());
        if queued.is_empty() {
            out.push_str("  -");
        } else {
            out.push_str("  queued: ");
            out.push_str(&queued);
        }
        out.push_str(style.normal());
        out.push('\n');
    }
    for task in tasks {
        let name = task.get("name").and_then(Value::as_str).unwrap_or_default();
        visible_commands.insert(name.to_owned());
        task_line(out, task, name, state, frame, width, style);
    }
    for task in state.finished_tasks.values().take(256) {
        ghost_line(out, task, style);
    }
    let queued = queued_commands(state, &visible_commands);
    if !queued.is_empty() {
        out.push_str(style.dim());
        out.push_str("  queued: ");
        out.push_str(&queued);
        out.push_str(style.normal());
        out.push('\n');
    }
}

fn brain_section(out: &mut String, state: &TopState, width: usize, style: &dyn TopStyle) {
    rule(out, width);
    match (&state.brain_health_state, state.brain_health.as_ref()) {
        (BrainHealthState::Available { .. }, Some(value))
            if value
                .get("lines")
                .and_then(Value::as_array)
                .is_some_and(|lines| !lines.is_empty()) =>
        {
            let lines = value["lines"].as_array().expect("checked nonempty lines");
            out.push_str("  ");
            out.push_str(style.bold());
            out.push_str(&value_text(&lines[0]));
            out.push_str(style.normal());
            out.push('\n');
            for line in lines.iter().skip(1) {
                out.push_str(&value_text(line));
                out.push('\n');
            }
        }
        (BrainHealthState::Checking, _) => {
            out.push_str("  ");
            out.push_str(style.bold());
            out.push_str("Brain Health");
            out.push_str(style.normal());
            out.push('\n');
            out.push_str(style.dim());
            out.push_str("  (checking)");
            out.push_str(style.normal());
            out.push('\n');
        }
        (BrainHealthState::Unavailable { message, .. }, _) => {
            out.push_str("  ");
            out.push_str(style.bold());
            out.push_str("Brain Health");
            out.push_str(style.normal());
            out.push('\n');
            out.push_str(style.dim());
            out.push_str("  (status unavailable)");
            out.push_str(style.normal());
            out.push('\n');
            if !message.is_empty() {
                out.push_str(style.dim());
                out.push_str("  ");
                out.push_str(&sanitized_payload_sentinel(message));
                out.push_str(style.normal());
                out.push('\n');
            }
        }
        _ => {
            out.push_str("  ");
            out.push_str(style.bold());
            out.push_str("Brain Health");
            out.push_str(style.normal());
            out.push('\n');
            out.push_str(style.dim());
            out.push_str("  (status unavailable)");
            out.push_str(style.normal());
            out.push('\n');
        }
    }
}

fn footer(out: &mut String, state: &TopState, style: &dyn TopStyle) {
    out.push_str(style.dim());
    out.push_str(match state.services.len() {
        0 => "q: Quit",
        1 => "r: Restart  q: Quit",
        _ => "↑/↓: Navigate  r: Restart  q: Quit",
    });
    out.push_str(style.normal());
}

/// Apply the AC3 trusted-token/sanitized-scalar width transform to every
/// physical line. This also accepts immutable retained fixture strings.
#[must_use]
pub fn transform_trusted_render(input: &str, width: usize) -> String {
    let mut output = String::with_capacity(input.len().min(MAX_FRAME_BYTES));
    for line in input.split_inclusive('\n') {
        let (body, newline) = line
            .strip_suffix('\n')
            .map_or((line, false), |body| (body, true));
        output.push_str(&transform_line(body, width));
        if newline {
            output.push('\n');
        }
    }
    output
}

fn transform_trusted_render_to_ops(input: &str, width: usize) -> Vec<TopRenderOp> {
    let mut ops = Vec::new();
    for line in input.split_inclusive('\n') {
        let (body, newline) = line
            .strip_suffix('\n')
            .map_or((line, false), |body| (body, true));
        transform_line_to_ops(body, width, &mut ops);
        if newline {
            push_print(&mut ops, "\n");
        }
    }
    ops
}

fn transform_trusted_render_to_ops_capped(
    input: &str,
    width: usize,
    op_cap: usize,
) -> Vec<TopRenderOp> {
    let content_cap = op_cap.saturating_sub(1);
    let mut ops = Vec::new();
    for line in input.split_inclusive('\n') {
        let (body, newline) = line
            .strip_suffix('\n')
            .map_or((line, false), |body| (body, true));
        let mut line_ops = Vec::new();
        transform_line_to_ops(body, width, &mut line_ops);
        if newline {
            push_print(&mut line_ops, "\n");
        }
        if ops.len().saturating_add(line_ops.len()) > content_cap {
            break;
        }
        ops.extend(line_ops);
    }
    ops.push(TopRenderOp::Style(TrustedToken::Normal));
    ops
}

fn push_print(ops: &mut Vec<TopRenderOp>, text: &str) {
    if let Some(TopRenderOp::Print(existing)) = ops.last_mut() {
        existing.push_str(text);
    } else {
        ops.push(TopRenderOp::Print(text.to_owned()));
    }
}

fn push_style(ops: &mut Vec<TopRenderOp>, token: TrustedToken) {
    ops.push(TopRenderOp::Style(token));
}

fn transform_line_to_ops(line: &str, width: usize, ops: &mut Vec<TopRenderOp>) {
    let mut remaining = line;
    let mut used = 0usize;
    let mut styles = 0u16;
    while !remaining.is_empty() {
        if let Some((token, length, _)) = trusted_prefix(remaining) {
            push_style(ops, token);
            remaining = &remaining[length..];
            match token {
                TrustedToken::Normal => styles = 0,
                TrustedToken::EndSelect => styles &= !1,
                TrustedToken::Select => styles |= 1,
                TrustedToken::Bold => styles |= 2,
                TrustedToken::Dim => styles |= 4,
                TrustedToken::Cyan => styles |= 8,
                TrustedToken::Green => styles |= 16,
                TrustedToken::Magenta => styles |= 32,
                TrustedToken::Red => styles |= 64,
                TrustedToken::Yellow => styles |= 128,
                TrustedToken::Home | TrustedToken::Clear => {}
            }
            continue;
        }
        if let Some(payload) = remaining.strip_prefix('\u{e000}')
            && let Some(end) = payload.find('\u{e001}')
        {
            let encoded = &payload[..end];
            let atom = sanitize_for_terminal(encoded);
            let atom_width = atom.chars().count();
            if used.saturating_add(atom_width) > width {
                if styles != 0 {
                    push_style(ops, TrustedToken::Normal);
                }
                return;
            }
            push_print(ops, &atom);
            used += atom_width;
            remaining = &payload[end + '\u{e001}'.len_utf8()..];
            continue;
        }
        if let Some(payload) = remaining.strip_prefix('\u{e002}')
            && let Some(end) = payload.find('\u{e003}')
        {
            let atom = &payload[..end];
            let atom_width = atom.chars().count();
            if used.saturating_add(atom_width) > width {
                if styles != 0 {
                    push_style(ops, TrustedToken::Normal);
                }
                return;
            }
            push_print(ops, atom);
            used += atom_width;
            remaining = &payload[end + '\u{e003}'.len_utf8()..];
            continue;
        }
        let scalar = remaining.chars().next().expect("nonempty input has scalar");
        remaining = &remaining[scalar.len_utf8()..];
        let atom = sanitize_for_terminal(&scalar.to_string());
        let atom_width = atom.chars().count();
        if used.saturating_add(atom_width) > width {
            if styles != 0 {
                push_style(ops, TrustedToken::Normal);
            }
            return;
        }
        push_print(ops, &atom);
        used += atom_width;
    }
}

fn transform_trusted_render_capped(
    input: &str,
    width: usize,
    byte_cap: usize,
    normal: &str,
) -> String {
    let terminator_len = normal.len().saturating_add(1);
    let content_cap = byte_cap.saturating_sub(terminator_len);
    let mut output = String::with_capacity(byte_cap);
    for line in input.split_inclusive('\n') {
        let (body, newline) = line
            .strip_suffix('\n')
            .map_or((line, false), |body| (body, true));
        let transformed = transform_line(body, width);
        let line_len = transformed.len().saturating_add(usize::from(newline));
        if output.len().saturating_add(line_len) > content_cap {
            break;
        }
        output.push_str(&transformed);
        if newline {
            output.push('\n');
        }
    }
    output.push_str(normal);
    output.push('\n');
    output
}

fn transform_line(line: &str, width: usize) -> String {
    let mut output = String::new();
    let mut remaining = line;
    let mut used = 0usize;
    let mut styles = 0u16;
    let mut normal = TrustedToken::Normal.spelling();
    while !remaining.is_empty() {
        if let Some((token, length, reset)) = trusted_prefix(remaining) {
            output.push_str(&remaining[..length]);
            remaining = &remaining[length..];
            normal = reset;
            match token {
                TrustedToken::Normal => styles = 0,
                TrustedToken::EndSelect => styles &= !1,
                TrustedToken::Select => styles |= 1,
                TrustedToken::Bold => styles |= 2,
                TrustedToken::Dim => styles |= 4,
                TrustedToken::Cyan => styles |= 8,
                TrustedToken::Green => styles |= 16,
                TrustedToken::Magenta => styles |= 32,
                TrustedToken::Red => styles |= 64,
                TrustedToken::Yellow => styles |= 128,
                TrustedToken::Home | TrustedToken::Clear => {}
            }
            continue;
        }
        if let Some(payload) = remaining.strip_prefix('\u{e000}')
            && let Some(end) = payload.find('\u{e001}')
        {
            let encoded = &payload[..end];
            let atom = sanitize_for_terminal(encoded);
            let atom_width = atom.chars().count();
            if used.saturating_add(atom_width) > width {
                if styles != 0 {
                    output.push_str(normal);
                }
                break;
            }
            output.push_str(&atom);
            used += atom_width;
            remaining = &payload[end + '\u{e001}'.len_utf8()..];
            continue;
        }
        if let Some(payload) = remaining.strip_prefix('\u{e002}')
            && let Some(end) = payload.find('\u{e003}')
        {
            let atom = &payload[..end];
            let atom_width = atom.chars().count();
            if used.saturating_add(atom_width) > width {
                if styles != 0 {
                    output.push_str(normal);
                }
                break;
            }
            output.push_str(atom);
            used += atom_width;
            remaining = &payload[end + '\u{e003}'.len_utf8()..];
            continue;
        }
        let scalar = remaining.chars().next().expect("nonempty input has scalar");
        remaining = &remaining[scalar.len_utf8()..];
        let atom = sanitize_for_terminal(&scalar.to_string());
        let atom_width = atom.chars().count();
        if used.saturating_add(atom_width) > width {
            if styles != 0 {
                output.push_str(normal);
            }
            break;
        }
        output.push_str(&atom);
        used += atom_width;
    }
    output
}

fn trusted_prefix(value: &str) -> Option<(TrustedToken, usize, &'static str)> {
    TRUSTED_TOKENS
        .iter()
        .copied()
        .find_map(|token| {
            value.starts_with(token.spelling()).then_some((
                token,
                token.spelling().len(),
                TrustedToken::Normal.spelling(),
            ))
        })
        .or_else(|| {
            TRUSTED_TOKENS.iter().copied().find_map(|token| {
                value.starts_with(token.ansi()).then_some((
                    token,
                    token.ansi().len(),
                    TrustedToken::Normal.ansi(),
                ))
            })
        })
}

pub fn format_uptime(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3600 {
        return format!("{}m", seconds / 60);
    }
    if seconds < 86_400 {
        return format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60);
    }
    format!("{}d {}m", seconds / 86_400, (seconds % 3600) / 60)
}
pub fn format_runtime(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    }
}
pub fn format_log_age(seconds: u64) -> String {
    if seconds < 60 {
        "0m".to_owned()
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn service_line(
    out: &mut String,
    service: &Value,
    selected: bool,
    state: &TopState,
    frame: FrameSample,
    width: usize,
    style: &dyn TopStyle,
) {
    let name = service
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let pid = service.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    let (icon, color) = status_icon(state.service_status.get(name), frame.wall_seconds);
    if selected {
        out.push_str(style.inverse());
    } else {
        out.push_str(status_style(color, style));
    }
    out.push_str(icon);
    if !selected {
        out.push_str(style.normal());
    }
    out.push_str(&format!(
        " {} {:<8} {:<12} {:>7}  {:>5} {:>5} ",
        pad_clipped(name, 14, 15),
        pid,
        format_uptime(
            service
                .get("uptime_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ),
        memory_mb(state.memory_cache.get(&pid)),
        state
            .cpu_cache
            .get(&pid)
            .map_or("-".to_owned(), |cpu| format!("{cpu:.0}")),
        log_age(service.get("ref").and_then(Value::as_str), state, frame)
    ));
    let (line, stderr) = log_text(service.get("ref").and_then(Value::as_str), state, width);
    if stderr {
        out.push_str(style.red());
    }
    out.push_str(&line);
    if selected {
        out.push_str(style.end_select());
    } else {
        out.push_str(style.normal());
    }
    out.push('\n');
}

fn task_line(
    out: &mut String,
    task: &Value,
    name: &str,
    state: &TopState,
    frame: FrameSample,
    width: usize,
    style: &dyn TopStyle,
) {
    let pid = task.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    let command = match state.command_queues.get(name).and_then(Value::as_u64) {
        Some(queued) if queued > 0 => format!("{name} ({queued})"),
        _ => name.to_owned(),
    };
    let started = task
        .get("ref")
        .and_then(Value::as_str)
        .and_then(|reference| state.task_started_at.get(reference))
        .copied()
        .unwrap_or(frame.monotonic_seconds);
    let runtime = (frame.monotonic_seconds - started).max(0.0) as u64;
    let reference = task.get("ref").and_then(Value::as_str);
    out.push_str(&format!(
        "  {:<15} {:<8} {:<12} {:>7}  {:>5} {:>5} ",
        pad_clipped(&command, 14, 15),
        pid,
        format_runtime(runtime),
        memory_mb(state.memory_cache.get(&pid)),
        state
            .cpu_cache
            .get(&pid)
            .map_or("-".to_owned(), |cpu| format!("{cpu:.0}")),
        log_age(reference, state, frame)
    ));
    let (line, stderr) = log_text(reference, state, width);
    if stderr {
        out.push_str(style.red());
    }
    out.push_str(&line);
    out.push_str(style.normal());
    out.push('\n');
}

fn ghost_line(out: &mut String, task: &Value, style: &dyn TopStyle) {
    let name = task.get("name").and_then(Value::as_str).unwrap_or_default();
    let exit = task.get("exit_code");
    let (indicator, color, label) = match exit {
        Some(Value::Null) | None => ("?".to_owned(), style.yellow(), "gone"),
        Some(value) if value.as_i64() == Some(0) => ("✓".to_owned(), style.green(), "ok"),
        Some(value) => (format!("✗ {}", value_text(value)), style.red(), "failed"),
    };
    out.push_str(style.dim());
    out.push_str(&format!(
        "  {:<15} {:<8} {:<12} {:>7}  {:>5} {:>5} ",
        pad_clipped(name, 14, 15),
        "",
        "",
        "",
        "",
        ""
    ));
    out.push_str(color);
    out.push_str(&indicator);
    out.push_str(style.normal());
    out.push_str(style.dim());
    out.push(' ');
    out.push_str(label);
    out.push_str(style.normal());
    out.push('\n');
}

fn log_age(reference: Option<&str>, state: &TopState, frame: FrameSample) -> String {
    reference
        .and_then(|reference| state.last_log_at.get(reference))
        .map(|at| format_log_age((frame.wall_seconds - *at).max(0.0) as u64))
        .unwrap_or_default()
}

fn log_text(reference: Option<&str>, state: &TopState, width: usize) -> (String, bool) {
    let Some(log) = reference
        .and_then(|reference| state.last_log_lines.get(reference))
        .and_then(Value::as_array)
    else {
        return (String::new(), false);
    };
    let source = log.get(2).and_then(Value::as_str).unwrap_or_default();
    let available = width.saturating_sub(LOG_FIXED_WIDTH);
    let (_, _, truncated) = clip_payload(source, available);
    let text = if truncated && available >= 3 {
        format!("{}...", truncate_scalars(source, available - 3))
    } else if truncated && available > 0 {
        payload_text(source)
    } else if available == 0 {
        String::new()
    } else {
        truncate_scalars(source, available)
    };
    (text, log.get(1).and_then(Value::as_str) == Some("stderr"))
}

fn observe_section(
    out: &mut String,
    state: &TopState,
    frame: FrameSample,
    width: usize,
    style: &dyn TopStyle,
) {
    rule(out, width);
    out.push_str("  ");
    out.push_str(style.bold());
    out.push_str("Observe");
    out.push_str(style.normal());
    reconnecting(out, state, &state.continuity.observe, style);
    out.push(' ');
    if state.observe_last_ts > 0.0 {
        let age = (frame.wall_seconds - state.observe_last_ts).max(0.0);
        out.push_str(if age < 30.0 {
            style.green()
        } else if age < 60.0 {
            style.yellow()
        } else {
            style.red()
        });
        out.push('●');
        out.push_str(style.normal());
    } else {
        out.push_str(style.dim());
        out.push('○');
        out.push_str(style.normal());
    }
    if let Some(stream) = state.observe_status.get("stream").and_then(Value::as_str) {
        out.push(' ');
        out.push_str(&truncate_scalars(stream, 1024));
    }
    out.push('\n');
    if state.observe_status.is_empty() {
        out.push_str(style.dim());
        out.push_str("  (waiting for status)");
        out.push_str(style.normal());
        out.push('\n');
    } else {
        out.push_str("  ");
        match displayed_observe_mode(state, frame.monotonic_seconds) {
            "screencast" => {
                let elapsed = state
                    .observe_status
                    .get("screencast")
                    .and_then(|value| value.get("window_elapsed_seconds"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                out.push_str(style.red());
                out.push_str("[LIVE]");
                out.push_str(style.normal());
                out.push_str(&format!(" screencast {}", format_uptime(elapsed)));
            }
            "tmux" => {
                let captures = state
                    .observe_status
                    .get("tmux")
                    .and_then(|value| value.get("captures"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                out.push_str(style.magenta());
                out.push_str("[TMUX]");
                out.push_str(style.normal());
                out.push_str(&format!(" {captures} captures"));
            }
            _ => {
                out.push_str(style.dim());
                out.push_str(
                    if state
                        .observe_status
                        .get("activity")
                        .and_then(|value| value.get("screen_locked"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        "[IDLE] locked"
                    } else {
                        "[IDLE]"
                    },
                );
                out.push_str(style.normal());
            }
        }
        if state
            .observe_status
            .get("audio")
            .and_then(|value| value.get("threshold_hits"))
            .and_then(Value::as_u64)
            .is_some_and(|hits| hits > 0)
        {
            let hits = state.observe_status["audio"]["threshold_hits"]
                .as_u64()
                .unwrap_or(0);
            out.push_str(" │ ");
            if state.observe_status["audio"]["will_save"].as_bool() == Some(true) {
                out.push_str(style.green());
                out.push_str(&format!("voice {hits}"));
                out.push_str(style.normal());
            } else {
                out.push_str(&format!("voice {hits}"));
            }
        }
        out.push_str(" │ describe ");
        out.push_str(&queue_status(state.observe_status.get("describe")));
        out.push_str(" │ transcribe ");
        out.push_str(&queue_status(state.observe_status.get("transcribe")));
        out.push('\n');
    }
    if !state.recent_segments.is_empty() {
        out.push_str(style.dim());
        out.push_str("  Recent: ");
        let recent = state
            .recent_segments
            .iter()
            .take(3)
            .map(|entry| {
                let segment = entry.get(1).map(value_text).unwrap_or_default();
                let seconds = entry.get(2).and_then(Value::as_u64).unwrap_or(0);
                format!("{segment} ({}m)", (seconds / 60).max(1))
            })
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&recent);
        out.push_str(style.normal());
        out.push('\n');
    }
}

fn displayed_observe_mode(state: &TopState, monotonic_seconds: f64) -> &str {
    let raw_mode = state
        .observe_status
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("idle");
    if raw_mode == "idle" && monotonic_seconds - state.last_active_ts >= 10.0 {
        "idle"
    } else {
        state.displayed_mode.as_str()
    }
}

fn queued_commands(state: &TopState, visible: &std::collections::BTreeSet<String>) -> String {
    state
        .command_queues
        .iter()
        .filter_map(|(command, count)| {
            count
                .as_u64()
                .filter(|count| *count > 0 && !visible.contains(command))
                .map(|count| format!("{} ×{count}", payload_text(command)))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn queue_status(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return pad("─", 8);
    };
    let running = value
        .get("running")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let queued = value
        .get("queued")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let status = match (running, queued) {
        (0, 0) => "─".to_owned(),
        (0, queued) => format!("+{queued}"),
        (running, 0) => format!("▸{running}"),
        (running, queued) => format!("▸{running} +{queued}"),
    };
    pad(&status, 8)
}

fn memory_mb(bytes: Option<&u64>) -> String {
    bytes.map_or_else(
        || "-".to_owned(),
        |bytes| {
            let megabyte = 1_048_576;
            let whole = bytes / megabyte;
            let remainder = bytes % megabyte;
            let rounded =
                if remainder > megabyte / 2 || (remainder == megabyte / 2 && whole % 2 == 1) {
                    whole + 1
                } else {
                    whole
                };
            rounded.to_string()
        },
    )
}

fn think_section(out: &mut String, state: &TopState, width: usize, style: &dyn TopStyle) {
    rule(out, width);
    out.push_str("  ");
    out.push_str(style.bold());
    out.push_str("Think");
    out.push_str(style.normal());
    reconnecting(out, state, &state.continuity.think, style);
    out.push('\n');
    if state.think_running {
        if !state.think_status.is_empty() {
            let status = &state.think_status;
            let mode = payload_text(
                &status
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_uppercase(),
            );
            let day = payload_text(status.get("day").and_then(Value::as_str).unwrap_or(""));
            let segment = payload_text(status.get("segment").and_then(Value::as_str).unwrap_or(""));
            let completed = status
                .get("agents_completed")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let total = status
                .get("agents_total")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let mut parts = vec![format!("[{mode}] {day}/{segment}")];
            if let Some(segment_total) = status.get("segments_total").and_then(Value::as_u64) {
                parts.push(format!(
                    "seg {}/{}",
                    status
                        .get("segments_completed")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    segment_total
                ));
            }
            parts.push(format!("{completed}/{total} agents"));
            if let Some(agents) = status.get("current_agents").and_then(Value::as_array)
                && !agents.is_empty()
            {
                parts.push(
                    agents
                        .iter()
                        .filter_map(Value::as_str)
                        .map(payload_text)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            out.push_str("  ");
            out.push_str(&parts.join(" — "));
            out.push('\n');
        } else {
            out.push_str(style.dim());
            out.push_str("  (waiting for status)");
            out.push_str(style.normal());
            out.push('\n');
        }
    } else if !state.think_last_completed.is_empty() {
        let completed = &state.think_last_completed;
        let success = completed
            .get("success")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let failed = completed.get("failed").and_then(Value::as_u64).unwrap_or(0);
        let duration = completed
            .get("duration_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            / 1000;
        out.push_str(&format!("  Last: {success} ok, "));
        if failed > 0 {
            out.push_str(style.red());
            out.push_str(&format!("{failed} failed"));
            out.push_str(style.normal());
        } else {
            out.push_str("0 failed");
        }
        out.push_str(&format!(" ({duration}s)"));
        if failed > 0
            && let Some(names) = completed.get("failed_names").and_then(Value::as_array)
            && !names.is_empty()
        {
            out.push_str(" — ");
            out.push_str(
                &names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(payload_text)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        out.push('\n');
    } else {
        out.push_str(style.dim());
        out.push_str("  (waiting for think)");
        out.push_str(style.normal());
        out.push('\n');
    }
}

fn crashed_section(out: &mut String, state: &TopState, style: &dyn TopStyle) {
    if state.crashed.is_empty() {
        return;
    }
    let has_retrying = state
        .crashed
        .iter()
        .any(|crash| crash.get("phase").and_then(Value::as_str) == Some("backoff"));
    out.push_str(style.bold());
    if has_retrying {
        out.push_str("Services needing attention:");
    } else {
        out.push_str(style.red());
        out.push_str("Crashed:");
    }
    out.push_str(style.normal());
    out.push('\n');
    for crash in state.crashed.iter().take(256) {
        let name = crash.get("name").map(value_text).unwrap_or_default();
        let attempts = crash
            .get("restart_attempts")
            .map(value_text)
            .unwrap_or_else(|| "0".to_owned());
        let retrying = crash.get("phase").and_then(Value::as_str) == Some("backoff");
        if retrying {
            out.push_str(style.yellow());
            out.push_str(&format!("  {name} (Retrying; attempts: {attempts})"));
            out.push_str(style.normal());
        } else if has_retrying {
            out.push_str(style.red());
            out.push_str(&format!("  {name} (Crashed; attempts: {attempts})"));
            out.push_str(style.normal());
        } else {
            out.push_str(&format!("  {name} (attempts: {attempts})"));
        }
        out.push('\n');
    }
    out.push('\n');
}

fn rule(out: &mut String, width: usize) {
    out.push_str(&"─".repeat(width));
    out.push('\n');
}
fn reconnecting(
    out: &mut String,
    state: &TopState,
    recovery: &crate::DomainRecovery,
    style: &dyn TopStyle,
) {
    if recovery.is_incomplete()
        && !matches!(
            state.continuity.connection,
            solstone_core_callosum::CallosumConnectionPhase::Connecting { .. }
        )
    {
        out.push_str(style.dim());
        out.push_str(" (reconnecting)");
        out.push_str(style.normal());
    }
}
fn center(value: &str, width: usize) -> String {
    let (cap, used, _) = clip_payload(value, width);
    let padding = width.saturating_sub(used);
    format!(
        "{}{}{}",
        " ".repeat(padding / 2),
        cap,
        " ".repeat(padding - padding / 2)
    )
}
fn pad(value: &str, width: usize) -> String {
    pad_clipped(value, width, width)
}
fn truncate_scalars(value: &str, width: usize) -> String {
    clip_payload(value, width).0
}

fn pad_clipped(value: &str, clip_width: usize, field_width: usize) -> String {
    let (mut output, used, _) = clip_payload(value, clip_width);
    output.push_str(&" ".repeat(field_width.saturating_sub(used)));
    output
}

fn clip_payload(value: &str, width: usize) -> (String, usize, bool) {
    let mut output = String::new();
    let mut used = 0usize;
    let mut scalars = value.chars();
    let mut truncated = false;
    for (count, scalar) in scalars.by_ref().enumerate() {
        if count == 1024 {
            truncated = true;
            break;
        }
        let atom = sanitize_payload_scalar(scalar);
        let atom_width = atom.chars().count();
        if used.saturating_add(atom_width) > width {
            truncated = true;
            break;
        }
        output.push_str(&payload_atom_sentinel(&atom));
        used += atom_width;
    }
    if !truncated && scalars.next().is_some() {
        truncated = true;
    }
    (output, used, truncated)
}

fn status_style<'a>(color: &str, style: &'a dyn TopStyle) -> &'a str {
    match color {
        "green" => style.green(),
        "red" => style.red(),
        "yellow" => style.yellow(),
        _ => style.normal(),
    }
}
fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| bounded_json(value), payload_text)
}
fn bounded_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => format!("\"{}\"", payload_text(value)),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .take(32)
                .map(bounded_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .take(32)
                .map(|(key, value)| format!(
                    "\"{}\":{}",
                    payload_text(&key.chars().take(256).collect::<String>()),
                    bounded_json(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

// Mark token spellings carried by payload so the trusted framing scanner can
// never mistake them for renderer-owned style. They are restored inside the
// sanitizer atom during serialization.
fn payload_text(value: &str) -> String {
    value
        .chars()
        .take(1024)
        .map(sanitize_payload_scalar)
        .map(|atom| payload_atom_sentinel(&atom))
        .collect()
}

fn sanitize_payload_scalar(scalar: char) -> String {
    match scalar {
        '\u{e000}'..='\u{e003}' => format!("\\u{{{:x}}}", u32::from(scalar)),
        _ => sanitize_for_terminal(&scalar.to_string()),
    }
}

fn sanitized_payload_sentinel(value: &str) -> String {
    payload_text(value)
}

fn payload_atom_sentinel(value: &str) -> String {
    format!("\u{e002}{value}\u{e003}")
}

fn status_icon(status: Option<&(String, f64)>, now: f64) -> (&'static str, &'static str) {
    match status {
        Some((kind, at)) if now - at <= 5.0 => match kind.as_str() {
            "restarting" => ("◐", "yellow"),
            "started" => ("✓", "green"),
            "stopped" => ("✗", "red"),
            _ => ("↻", "yellow"),
        },
        _ => (" ", "normal"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_truncation_and_boundaries_are_stable() {
        assert_eq!(
            transform_trusted_render(&truncate_scalars("αβγ", 2), 2),
            "αβ"
        );
        assert_eq!(format_uptime(86_400), "1d 0m");
        assert_eq!(format_log_age(3_600), "1h");
        assert_eq!(format_runtime(60), "1m 0s");
        let megabyte = 1_048_576;
        assert_eq!(memory_mb(Some(&(12 * megabyte + megabyte / 2))), "12");
        assert_eq!(memory_mb(Some(&(13 * megabyte + megabyte / 2))), "14");
        assert_eq!(transform_trusted_render(&queue_status(None), 8), "─       ");
    }
    #[test]
    fn ansi_style_keeps_renderer_owned_controls_out_of_payload_sanitization() {
        let rendered = render_frame(
            &TopState::default(),
            FrameSample::default(),
            80,
            &AnsiTopStyle,
        );
        assert!(rendered.starts_with("\x1b[H\x1b[2J"));
        assert!(rendered.contains("\x1b[1m"));
        assert!(!rendered.contains("<HOME>"));
        assert!(!rendered.contains("\\x1b[H"));
    }
    #[test]
    fn hostile_input_is_bounded() {
        let mut state = TopState::default();
        state.services.push(
            serde_json::json!({"name": "x".repeat(1_000_000), "pid": 1, "uptime_seconds": 0}),
        );
        assert!(
            render_frame(&state, FrameSample::default(), 512, &PlainTopStyle).len()
                <= MAX_FRAME_BYTES
        );
    }
    #[test]
    fn hostile_input_work_is_prefix_bounded_at_one_sixteen_and_sixty_four_megabytes() {
        let render = |length| {
            let mut state = TopState::default();
            state
                .services
                .push(serde_json::json!({"name":"x".repeat(length),"pid":1,"uptime_seconds":0}));
            render_frame(&state, FrameSample::default(), 120, &PlainTopStyle)
        };
        let one = render(1024 * 1024);
        let sixteen = render(16 * 1024 * 1024);
        let sixty_four = render(64 * 1024 * 1024);
        // A counting GlobalAlloc needs unsafe code, which this workspace forbids.
        assert_eq!(one, sixteen);
        assert_eq!(sixteen, sixty_four);
        assert!(one.len() <= MAX_FRAME_BYTES);
    }

    #[test]
    fn frame_cap_preserves_utf8_boundaries_for_hostile_unicode() {
        let state = TopState {
            crashed: (0..256)
                .map(|_| serde_json::json!({"name":"界".repeat(1024), "restart_attempts":1}))
                .collect(),
            ..TopState::default()
        };
        let rendered = std::panic::catch_unwind(|| {
            render_frame(
                &state,
                FrameSample::default(),
                MAX_FRAME_WIDTH,
                &PlainTopStyle,
            )
        })
        .expect("renderer must not truncate inside a UTF-8 scalar");
        assert!(rendered.len() <= MAX_FRAME_BYTES);
        assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
    }

    fn style_sequence(ops: &[TopRenderOp]) -> Vec<TrustedToken> {
        ops.iter()
            .filter_map(|op| match op {
                TopRenderOp::Style(token) => Some(*token),
                TopRenderOp::Print(_) => None,
            })
            .collect()
    }

    fn print_text(ops: &[TopRenderOp]) -> String {
        ops.iter()
            .filter_map(|op| match op {
                TopRenderOp::Print(text) => Some(text.as_str()),
                TopRenderOp::Style(_) => None,
            })
            .collect()
    }

    fn service_named(name: &str) -> TopState {
        let mut state = TopState::default();
        state.services.push(serde_json::json!({
            "name": name, "pid": 1, "uptime_seconds": 0
        }));
        state
    }

    #[test]
    fn render_ops_is_independent_of_construction_style() {
        let sample = FrameSample::default();
        let width = 80;
        for state in [TopState::default(), service_named("supervisor"), {
            let mut state = service_named("supervisor");
            state.crashed.push(serde_json::json!({
                "name": "crash", "restart_attempts": 1
            }));
            state
        }] {
            let ansi = transform_trusted_render_to_ops(
                &build_frame_text(&state, sample, width, &AnsiTopStyle),
                width,
            );
            let plain = transform_trusted_render_to_ops(
                &build_frame_text(&state, sample, width, &PlainTopStyle),
                width,
            );
            assert_eq!(ansi, plain);
        }
    }

    #[test]
    fn render_ops_never_reclassifies_payload_trusted_spellings_as_style() {
        let sample = FrameSample::default();
        let width = 80;
        let control = render_ops(&service_named("svc"), sample, width);
        let control_styles = style_sequence(&control);
        for (name, expected_print) in [("<RED>", "<RED>"), ("\x1b[31m", "\\x1b[31m")] {
            let ops = render_ops(&service_named(name), sample, width);
            assert_eq!(style_sequence(&ops), control_styles, "{name:?}");
            let text = print_text(&ops);
            assert!(
                text.contains(expected_print),
                "{name:?} missing {expected_print:?} in {text:?}"
            );
        }
    }

    fn crashed_named(name: &str) -> TopState {
        let mut state = TopState::default();
        state
            .crashed
            .push(serde_json::json!({"name": name, "restart_attempts": 1}));
        state
    }

    #[test]
    fn crashed_section_distinguishes_retrying_from_crashed_services() {
        let mut state = TopState::default();
        state
            .crashed
            .push(serde_json::json!({"name":"convey","restart_attempts":5,"phase":"backoff"}));
        state
            .crashed
            .push(serde_json::json!({"name":"local","restart_attempts":2,"phase":"failed"}));
        let rendered = render_frame(&state, FrameSample::default(), 120, &PlainTopStyle);
        assert!(rendered.contains("Services needing attention:"));
        assert!(rendered.contains("<YELLOW>  convey (Retrying; attempts: 5)<NORMAL>"));
        assert!(rendered.contains("<RED>  local (Crashed; attempts: 2)<NORMAL>"));
    }

    #[test]
    fn render_ops_preserves_control_chars_and_private_markers_inside_print() {
        let sample = FrameSample::default();
        let width = 120;
        let hostile = "plain\x1btext\x07more\u{e000}\u{e001}\u{e002}\u{e003}tail";
        let ops = render_ops(&crashed_named(hostile), sample, width);
        assert_eq!(
            style_sequence(&ops),
            style_sequence(&render_ops(&crashed_named("svc"), sample, width))
        );
        let text = print_text(&ops);
        assert!(text.contains("plain"), "{text:?}");
        assert!(text.contains("\\x1b"), "{text:?}");
        assert!(text.contains("\\u{7}"), "{text:?}");
        assert!(text.contains("more"), "{text:?}");
        assert!(text.contains("\\u{e000}"), "{text:?}");
        assert!(text.contains("\\u{e001}"), "{text:?}");
        assert!(text.contains("\\u{e002}"), "{text:?}");
        assert!(text.contains("\\u{e003}"), "{text:?}");
        assert!(text.contains("tail"), "{text:?}");
        assert!(!text.contains(['\u{e000}', '\u{e001}', '\u{e002}', '\u{e003}']));
        assert!(!text.contains('\x1b'));
        assert!(!text.contains('\x07'));
    }

    #[test]
    fn render_ops_keeps_large_unicode_scalars_intact_across_tokens() {
        let payload = format!("{}😀αβγ", "界".repeat(4096));
        let input = format!("<BOLD>{payload}<NORMAL>");
        let ops = std::panic::catch_unwind(|| {
            transform_trusted_render_to_ops(&input, payload.chars().count() + 32)
        })
        .expect("tokenizer must not panic on mixed multi-byte input");
        assert_eq!(
            ops,
            vec![
                TopRenderOp::Style(TrustedToken::Bold),
                TopRenderOp::Print(payload),
                TopRenderOp::Style(TrustedToken::Normal),
            ]
        );
    }

    fn fully_maximal_hostile_combined_state() -> TopState {
        let hostile = format!("{}\u{e000}\u{1b}<RED>", "界".repeat(1024));
        let mut state = TopState::default();
        let statuses = ["started", "stopped", "restarting", "other"];
        for index in 0..256u32 {
            let service_name = format!("svc-{index}-{hostile}");
            let service_ref = format!("svc-ref-{index}");
            state.services.push(serde_json::json!({
                "name": service_name,
                "pid": index + 1,
                "ref": service_ref,
                "uptime_seconds": 0
            }));
            state
                .service_status
                .insert(service_name, (statuses[index as usize % 4].to_owned(), 0.0));
            state.last_log_lines.insert(
                service_ref,
                serde_json::json!([{"seconds": 0}, "stderr", format!("log-{hostile}")]),
            );
            let task_name = format!("task-{index}-{hostile}");
            let task_ref = format!("task-ref-{index}");
            state.running_tasks.insert(
                task_name.clone(),
                serde_json::json!({
                    "name": task_name,
                    "pid": 1000 + index,
                    "ref": task_ref
                }),
            );
            state.last_log_lines.insert(
                task_ref,
                serde_json::json!([{"seconds": 0}, "stderr", format!("task-log-{hostile}")]),
            );
            state.finished_tasks.insert(
                format!("ghost-{index}"),
                serde_json::json!({
                    "name": format!("ghost-{index}-{hostile}"),
                    "exit_code": 1
                }),
            );
            state.crashed.push(serde_json::json!({
                "name": format!("{index}-{hostile}"),
                "restart_attempts": index,
            }));
        }
        state
    }

    fn assert_prints_have_no_raw_markers_or_esc(ops: &[TopRenderOp]) {
        for op in ops {
            if let TopRenderOp::Print(text) = op {
                assert!(
                    !text.contains(['\u{e000}', '\u{e001}', '\u{e002}', '\u{e003}']),
                    "private marker in print {text:?}"
                );
                assert!(!text.contains('\u{1b}'), "raw ESC in print {text:?}");
            }
        }
    }

    #[test]
    fn transform_trusted_render_to_ops_capped_reserves_a_trailing_reset() {
        let style = AnsiTopStyle;
        let input = format!(
            "{}{}{}{}{}\n{}tail{}\n",
            style.home(),
            style.clear(),
            style.bold(),
            payload_atom_sentinel("\\x1b"),
            style.red(),
            style.dim(),
            style.normal(),
        );
        let uncapped = transform_trusted_render_to_ops(&input, 512);
        for op_cap in 1..=uncapped.len().saturating_add(4) {
            let ops = transform_trusted_render_to_ops_capped(&input, 512, op_cap);
            assert!(ops.len() <= op_cap, "cap {op_cap}: {}", ops.len());
            assert_eq!(
                ops.last(),
                Some(&TopRenderOp::Style(TrustedToken::Normal)),
                "cap {op_cap}: {ops:?}"
            );
        }
    }

    #[test]
    fn render_ops_stays_within_max_frame_ops_for_hostile_maximal_state() {
        let ops = render_ops(
            &fully_maximal_hostile_combined_state(),
            FrameSample::default(),
            MAX_FRAME_WIDTH,
        );
        assert!(ops.len() <= MAX_FRAME_OPS, "{}", ops.len());
        assert_eq!(ops.last(), Some(&TopRenderOp::Style(TrustedToken::Normal)));
        assert_prints_have_no_raw_markers_or_esc(&ops);
    }

    #[test]
    fn render_ops_capped_helper_truncates_real_maximal_frame_content_and_resets() {
        let sample = FrameSample::default();
        let state = fully_maximal_hostile_combined_state();
        let output = build_frame_text(&state, sample, MAX_FRAME_WIDTH, &AnsiTopStyle);
        let uncapped = transform_trusted_render_to_ops(&output, MAX_FRAME_WIDTH);
        assert!(
            uncapped.len() > 256,
            "maximal combined state should exceed a single-family row count: {}",
            uncapped.len()
        );
        assert!(uncapped.len() <= MAX_FRAME_OPS, "{}", uncapped.len());
        assert_prints_have_no_raw_markers_or_esc(&uncapped);
        for cap in [uncapped.len() / 2, uncapped.len() / 8, 1] {
            let ops = transform_trusted_render_to_ops_capped(&output, MAX_FRAME_WIDTH, cap);
            assert!(ops.len() <= cap, "cap {cap}: {}", ops.len());
            assert_eq!(
                ops.last(),
                Some(&TopRenderOp::Style(TrustedToken::Normal)),
                "cap {cap}: {ops:?}"
            );
            assert_prints_have_no_raw_markers_or_esc(&ops);
        }
    }

    #[test]
    fn render_ops_empty_state_is_bounded_and_nonempty() {
        let ops = render_ops(&TopState::default(), FrameSample::default(), 80);
        assert!(!ops.is_empty());
        assert!(ops.len() <= MAX_FRAME_OPS, "{}", ops.len());
        assert_eq!(ops.first(), Some(&TopRenderOp::Style(TrustedToken::Home)));
        assert_eq!(ops.get(1), Some(&TopRenderOp::Style(TrustedToken::Clear)));
        assert_eq!(ops.last(), Some(&TopRenderOp::Style(TrustedToken::Normal)));
    }

    #[test]
    fn payload_atoms_are_indivisible_and_private_markers_are_escaped() {
        for width in 0..4 {
            assert_eq!(truncate_scalars("\u{1b}", width), "");
        }
        assert_eq!(
            transform_trusted_render(&truncate_scalars("\u{1b}", 4), 4),
            "\\x1b"
        );
        for marker in ['\u{e000}', '\u{e001}', '\u{e002}', '\u{e003}'] {
            let escaped = format!("\\u{{{:x}}}", u32::from(marker));
            for width in 0..escaped.chars().count() {
                assert_eq!(truncate_scalars(&marker.to_string(), width), "");
            }
            assert_eq!(
                transform_trusted_render(
                    &truncate_scalars(&marker.to_string(), escaped.chars().count()),
                    escaped.chars().count(),
                ),
                escaped
            );
        }
    }

    #[test]
    fn ansi_frame_cap_never_splits_controls_or_payload_atoms() {
        let style = AnsiTopStyle;
        let input = format!(
            "{}{}{}{}{}\n{}tail{}\n",
            style.home(),
            style.clear(),
            style.bold(),
            payload_atom_sentinel("\\x1b"),
            style.red(),
            style.dim(),
            style.normal(),
        );
        let terminator = format!("{}\n", style.normal());
        for cap in terminator.len()..=input.len().saturating_add(terminator.len()) {
            let rendered = transform_trusted_render_capped(&input, 512, cap, style.normal());
            assert!(rendered.len() <= cap, "cap {cap}: {}", rendered.len());
            assert!(rendered.ends_with(&terminator), "cap {cap}: {rendered:?}");
            assert!(!rendered.contains(['\u{e000}', '\u{e001}', '\u{e002}', '\u{e003}']));
            let mut text = rendered;
            for sequence in TRUSTED_TOKENS.map(TrustedToken::ansi) {
                text = text.replace(sequence, "");
            }
            assert!(!text.contains('\u{1b}'), "cap {cap}: {text:?}");
            assert!(!text.ends_with('\\'));
            assert!(!text.ends_with("\\x"));
            assert!(!text.ends_with("\\x1"));
        }
    }

    #[test]
    fn production_ansi_frame_cap_reserves_a_complete_reset_and_newline() {
        let state = TopState {
            crashed: (0..256)
                .map(|index| {
                    serde_json::json!({
                        "name": format!("{index}-{}\u{e000}\u{1b}<RED>", "界".repeat(1024)),
                        "restart_attempts": index,
                    })
                })
                .collect(),
            ..TopState::default()
        };
        let rendered = render_frame(
            &state,
            FrameSample::default(),
            MAX_FRAME_WIDTH,
            &AnsiTopStyle,
        );
        assert!(rendered.len() <= MAX_FRAME_BYTES);
        assert!(rendered.ends_with("\u{1b}[0m\n"));
        assert!(!rendered.contains(['\u{e000}', '\u{e001}', '\u{e002}', '\u{e003}']));
        let mut text = rendered;
        for sequence in TRUSTED_TOKENS.map(TrustedToken::ansi) {
            text = text.replace(sequence, "");
        }
        assert!(!text.contains('\u{1b}'));
        assert!(!text.ends_with('\\'));
        assert!(!text.ends_with("\\x"));
        assert!(!text.ends_with("\\x1"));
        assert!(!text.ends_with("\\u{"));
    }
}
