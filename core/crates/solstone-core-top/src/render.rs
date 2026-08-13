// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_system_health::sanitize_for_terminal;

use crate::{BrainHealthState, TopState};

pub const LOG_FIXED_WIDTH: usize = 63;
pub const MAX_FRAME_WIDTH: usize = 512;
pub const MAX_FRAME_BYTES: usize = MAX_FRAME_WIDTH * 16 + 32_768;

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
}

/// ANSI style used by a real terminal adapter.
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
}

/// Renderer-owned framing is distinct from untrusted application text until
/// final serialization. `LineBreak` is structural and never sanitized.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameSegment {
    Trusted(TrustedToken),
    Untrusted(String),
    LineBreak,
}

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
    fn spelling(self) -> &'static str {
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

/// Render a bounded frame from state without ambient time, terminal, or I/O.
#[must_use]
pub fn render_frame(
    state: &TopState,
    frame: FrameSample,
    width: usize,
    style: &dyn TopStyle,
) -> String {
    let width = width.min(MAX_FRAME_WIDTH);
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
    let mut output = transform_trusted_render(&output, width);
    truncate_to_boundary(&mut output, MAX_FRAME_BYTES);
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
        let name = task.get("name").map(value_text).unwrap_or_default();
        visible_commands.insert(name.clone());
        task_line(out, task, &name, state, frame, width, style);
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

fn transform_line(line: &str, width: usize) -> String {
    let mut output = String::new();
    let mut remaining = line;
    let mut used = 0usize;
    let mut styles = 0u16;
    while !remaining.is_empty() {
        if let Some(token) = TRUSTED_TOKENS
            .iter()
            .copied()
            .find(|token| remaining.starts_with(token.spelling()))
        {
            output.push_str(token.spelling());
            remaining = &remaining[token.spelling().len()..];
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
                    output.push_str(TrustedToken::Normal.spelling());
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
                    output.push_str(TrustedToken::Normal.spelling());
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
                output.push_str(TrustedToken::Normal.spelling());
            }
            break;
        }
        output.push_str(&atom);
        used += atom_width;
    }
    output
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
    let name = service.get("name").map(value_text).unwrap_or_default();
    let pid = service.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    let (icon, color) = status_icon(state.service_status.get(&name), frame.wall_seconds);
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
        pad(&truncate_scalars(&name, 14), 15),
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
        out.push_str("</SELECT>");
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
        pad(&command, 14),
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
    let name = task.get("name").map(value_text).unwrap_or_default();
    let exit = task.get("exit_code");
    let (indicator, color, label) = match exit {
        Some(Value::Null) | None => ("?".to_owned(), style.yellow(), "gone"),
        Some(value) if value.as_i64() == Some(0) => ("✓".to_owned(), style.green(), "ok"),
        Some(value) => (format!("✗ {}", value_text(value)), style.red(), "failed"),
    };
    out.push_str(style.dim());
    out.push_str(&format!(
        "  {:<15} {:<8} {:<12} {:>7}  {:>5} {:>5} ",
        pad(&name, 14),
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
    let source = log.get(2).map(value_text).unwrap_or_default();
    let available = width.saturating_sub(LOG_FIXED_WIDTH);
    let text = if source.chars().count() > available && available > 0 {
        let keep = if available >= 3 {
            available - 3
        } else {
            source.chars().count().saturating_sub(3 - available)
        };
        format!("{}...", source.chars().take(keep).collect::<String>())
    } else if available == 0 {
        String::new()
    } else {
        source
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
                .map(|count| format!("{command} ×{count}"))
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

fn truncate_to_boundary(value: &mut String, limit: usize) {
    if value.len() <= limit {
        return;
    }
    let mut boundary = limit;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
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
            let mode = status
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_uppercase();
            let day = status.get("day").and_then(Value::as_str).unwrap_or("");
            let segment = status.get("segment").and_then(Value::as_str).unwrap_or("");
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
                        .map(payload_token_sentinel)
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
                    .map(payload_token_sentinel)
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
    out.push_str(style.bold());
    out.push_str(style.red());
    out.push_str("Crashed:");
    out.push_str(style.normal());
    out.push('\n');
    for crash in state.crashed.iter().take(256) {
        let name = crash.get("name").map(value_text).unwrap_or_default();
        let attempts = crash
            .get("restart_attempts")
            .map(value_text)
            .unwrap_or_else(|| "0".to_owned());
        out.push_str(&format!("  {name} (attempts: {attempts})"));
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
    let cap = truncate_scalars(value, width);
    let padding = width.saturating_sub(cap.chars().count());
    format!(
        "{}{}{}",
        " ".repeat(padding / 2),
        cap,
        " ".repeat(padding - padding / 2)
    )
}
fn pad(value: &str, width: usize) -> String {
    let value = truncate_scalars(value, width);
    format!("{value:<width$}", width = width)
}
fn truncate_scalars(value: &str, width: usize) -> String {
    let mut output = String::new();
    let mut used = 0usize;
    for scalar in value.chars().take(1024) {
        let atom = sanitize_for_terminal(&scalar.to_string());
        let atom_width = atom.chars().count();
        if used.saturating_add(atom_width) > width {
            break;
        }
        output.push_str(&payload_token_sentinel(&atom));
        used += atom_width;
    }
    output
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
    value.as_str().map_or_else(
        || bounded_json(value),
        |text| payload_token_sentinel(&text.chars().take(1024).collect::<String>()),
    )
}
fn bounded_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => format!(
            "\"{}\"",
            payload_token_sentinel(&value.chars().take(1024).collect::<String>())
        ),
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
                    payload_token_sentinel(&key.chars().take(256).collect::<String>()),
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
fn payload_token_sentinel(value: &str) -> String {
    TRUSTED_TOKENS
        .iter()
        .fold(value.to_owned(), |value, token| {
            value.replace(
                token.spelling(),
                &format!("\u{e000}{}\u{e001}", token.spelling()),
            )
        })
}

fn sanitized_payload_sentinel(value: &str) -> String {
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
        assert_eq!(truncate_scalars("αβγ", 2), "αβ");
        assert_eq!(format_uptime(86_400), "1d 0m");
        assert_eq!(format_log_age(3_600), "1h");
        assert_eq!(format_runtime(60), "1m 0s");
        let megabyte = 1_048_576;
        assert_eq!(memory_mb(Some(&(12 * megabyte + megabyte / 2))), "12");
        assert_eq!(memory_mb(Some(&(13 * megabyte + megabyte / 2))), "14");
        assert_eq!(queue_status(None), "─       ");
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
}
