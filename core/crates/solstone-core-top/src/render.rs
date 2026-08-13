// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_system_health::sanitize_for_terminal;

use crate::TopState;

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
    fn cyan(&self) -> &str {
        "\x1b[36m"
    }
    fn normal(&self) -> &str {
        "\x1b[0m"
    }
}

/// ANSI style used by a real terminal adapter.
pub struct PlainTopStyle;
impl TopStyle for PlainTopStyle {}

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
    let title = "solstone activity manager";
    output.push_str(style.bold());
    output.push_str(style.cyan());
    output.push_str(&center(title, width));
    output.push_str(style.normal());
    output.push_str("\n\n");
    output.push_str(style.bold());
    output.push_str("  Service         PID      Uptime            MB      %  Last Log");
    output.push_str(style.normal());
    output.push('\n');
    rule(&mut output, width);
    if state.services.is_empty() {
        output.push_str(style.dim());
        output.push_str("  (waiting for services)");
        output.push_str(style.normal());
        output.push('\n');
    }
    for service in state.services.iter().take(256) {
        service_line(&mut output, service, state, frame, width, style);
    }
    rule(&mut output, width);
    observe_section(&mut output, state, frame, style);
    rule(&mut output, width);
    think_section(&mut output, state, style);
    output.push_str(style.bold());
    output.push_str("  Task            PID      Runtime           MB      %  Last Log");
    output.push_str(style.normal());
    output.push('\n');
    if state.running_tasks.is_empty() && state.finished_tasks.is_empty() {
        output.push_str(style.dim());
        output.push_str("  -");
        output.push_str(style.normal());
        output.push('\n');
    }
    for task in state.running_tasks.values().take(256) {
        task_line(&mut output, task, state, frame, width, style);
    }
    for task in state.finished_tasks.values().take(256) {
        task_line(&mut output, task, state, frame, width, style);
    }
    rule(&mut output, width);
    output.push_str("  ");
    output.push_str(style.bold());
    output.push_str("Brain Health");
    output.push_str(style.normal());
    output.push('\n');
    match &state.brain_health {
        Some(value) => dynamic_line(&mut output, &value_text(value), style),
        None => {
            output.push_str(style.dim());
            output.push_str("  (status unavailable)");
            output.push_str(style.normal());
            output.push('\n');
        }
    }
    crashed_section(&mut output, state, style);
    rule(&mut output, width);
    output.push_str(style.dim());
    output.push_str("q: Quit");
    output.push_str(style.normal());
    output.truncate(MAX_FRAME_BYTES);
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
    state: &TopState,
    frame: FrameSample,
    width: usize,
    style: &dyn TopStyle,
) {
    let name = service.get("name").map(value_text).unwrap_or_default();
    let pid = service.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    let status = state.service_status.get(&name);
    let icon = status_icon(status, frame.wall_seconds);
    out.push_str("  ");
    out.push_str(icon.0);
    out.push_str(&pad(&name, 14));
    out.push_str(&format!(
        " {:>6}  {:>8}  {:>12} {:>6}  ",
        pid,
        format_uptime(
            service
                .get("uptime_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ),
        "-",
        state
            .cpu_cache
            .get(&pid)
            .map_or("-".to_owned(), |cpu| format!("{cpu:.0}"))
    ));
    append_log(
        out,
        service.get("ref").and_then(Value::as_str),
        state,
        frame,
        width,
        style,
    );
}

fn task_line(
    out: &mut String,
    task: &Value,
    state: &TopState,
    frame: FrameSample,
    width: usize,
    style: &dyn TopStyle,
) {
    let name = task.get("name").map(value_text).unwrap_or_default();
    let pid = task.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    out.push_str("  ");
    out.push_str(&pad(&name, 14));
    out.push_str(&format!(
        " {:>6}  {:>8}  {:>12} {:>6}  ",
        pid,
        format_runtime(0),
        "-",
        state
            .cpu_cache
            .get(&pid)
            .map_or("-".to_owned(), |cpu| format!("{cpu:.0}"))
    ));
    append_log(
        out,
        task.get("ref").and_then(Value::as_str),
        state,
        frame,
        width,
        style,
    );
}

fn append_log(
    out: &mut String,
    reference: Option<&str>,
    state: &TopState,
    frame: FrameSample,
    width: usize,
    style: &dyn TopStyle,
) {
    let Some(log) = reference
        .and_then(|reference| state.last_log_lines.get(reference))
        .and_then(Value::as_array)
    else {
        out.push('\n');
        return;
    };
    let age = log
        .first()
        .and_then(Value::as_object)
        .and_then(|value| value.get("seconds"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let stream = log.get(1).map(value_text).unwrap_or_default();
    let line = log.get(2).map(value_text).unwrap_or_default();
    if stream == "stderr" {
        out.push_str(style.red());
    }
    out.push_str(&truncate_scalars(
        &line,
        width.saturating_sub(LOG_FIXED_WIDTH),
    ));
    if stream == "stderr" {
        out.push_str(style.normal());
    }
    let _ = frame;
    let _ = age;
    out.push('\n');
}

fn observe_section(out: &mut String, state: &TopState, frame: FrameSample, style: &dyn TopStyle) {
    out.push_str("  ");
    out.push_str(style.bold());
    out.push_str("Observe");
    out.push_str(style.normal());
    out.push(' ');
    let active = state.displayed_mode != "idle" && frame.wall_seconds - state.last_active_ts < 10.0;
    out.push_str(if active { "●" } else { "○" });
    out.push('\n');
    if state.observe_status.is_empty() {
        out.push_str(style.dim());
        out.push_str("  (waiting for status)");
        out.push_str(style.normal());
        out.push('\n');
    } else {
        dynamic_line(
            out,
            &format!(
                "  {}",
                value_text(&Value::Object(
                    state.observe_status.clone().into_iter().collect()
                ))
            ),
            style,
        );
    }
}

fn think_section(out: &mut String, state: &TopState, style: &dyn TopStyle) {
    out.push_str("  ");
    out.push_str(style.bold());
    out.push_str("Think");
    out.push_str(style.normal());
    out.push('\n');
    if state.think_status.is_empty()
        && !state.think_running
        && state.think_last_completed.is_empty()
    {
        out.push_str(style.dim());
        out.push_str("  (waiting for think)");
        out.push_str(style.normal());
        out.push('\n');
    } else {
        let value = if state.think_status.is_empty() {
            Value::Object(state.think_last_completed.clone().into_iter().collect())
        } else {
            Value::Object(state.think_status.clone().into_iter().collect())
        };
        dynamic_line(out, &format!("  {}", value_text(&value)), style);
    }
}

fn crashed_section(out: &mut String, state: &TopState, style: &dyn TopStyle) {
    if state.crashed.is_empty() {
        return;
    }
    out.push_str("  ");
    out.push_str(style.bold());
    out.push_str(style.red());
    out.push_str("Crashed");
    out.push_str(style.normal());
    out.push('\n');
    for crash in state.crashed.iter().take(256) {
        let name = crash.get("name").map(value_text).unwrap_or_default();
        let attempts = crash
            .get("restart_attempts")
            .map(value_text)
            .unwrap_or_else(|| "0".to_owned());
        dynamic_line(
            out,
            &format!("  {name} (restart attempts: {attempts})"),
            style,
        );
    }
}

fn rule(out: &mut String, width: usize) {
    out.push_str(&"─".repeat(width));
    out.push('\n');
}
fn dynamic_line(out: &mut String, value: &str, _style: &dyn TopStyle) {
    out.push_str(&truncate_scalars(value, 1024));
    out.push('\n');
}
fn center(value: &str, width: usize) -> String {
    let cap = truncate_scalars(value, width);
    format!(
        "{}{}",
        " ".repeat(width.saturating_sub(cap.chars().count()) / 2),
        cap
    )
}
fn pad(value: &str, width: usize) -> String {
    let value = truncate_scalars(value, width);
    format!("{value:<width$}", width = width)
}
fn truncate_scalars(value: &str, width: usize) -> String {
    let sanitized = sanitize_for_terminal(&value.chars().take(1024).collect::<String>());
    let count = sanitized.chars().count();
    if count <= width {
        sanitized
    } else if width <= 3 {
        sanitized.chars().take(width).collect()
    } else {
        format!(
            "{}...",
            sanitized.chars().take(width - 3).collect::<String>()
        )
    }
}
fn value_text(value: &Value) -> String {
    value.as_str().map_or_else(
        || bounded_json(value),
        |text| text.chars().take(1024).collect(),
    )
}
fn bounded_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => format!("\"{}\"", value.chars().take(1024).collect::<String>()),
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
                    key.chars().take(256).collect::<String>(),
                    bounded_json(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
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
}
