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
    fn green(&self) -> &str {
        "\x1b[32m"
    }
    fn yellow(&self) -> &str {
        "\x1b[33m"
    }
    fn cyan(&self) -> &str {
        "\x1b[36m"
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
    reconnecting(
        &mut output,
        state.continuity.supervisor.is_incomplete(),
        style,
    );
    output.push('\n');
    rule(&mut output, width);
    if state.services.is_empty() {
        output.push_str(style.dim());
        output.push_str("  (waiting for services)");
        output.push_str(style.normal());
        output.push('\n');
    }
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
    rule(&mut output, width);
    observe_section(&mut output, state, frame, style);
    rule(&mut output, width);
    think_section(&mut output, state, style);
    output.push_str(style.bold());
    output.push_str("  Task            PID      Runtime           MB      %  Last Log");
    output.push_str(style.normal());
    reconnecting(&mut output, state.continuity.tasks.is_incomplete(), style);
    output.push('\n');
    if state.running_tasks.is_empty() && state.finished_tasks.is_empty() {
        output.push_str(style.dim());
        output.push_str("  -");
        output.push_str(style.normal());
        output.push('\n');
    }
    let service_pids = state
        .services
        .iter()
        .filter_map(|service| service.get("pid").and_then(Value::as_u64))
        .collect::<std::collections::BTreeSet<_>>();
    for task in state
        .running_tasks
        .values()
        .filter(|task| {
            !task
                .get("pid")
                .and_then(Value::as_u64)
                .is_some_and(|pid| service_pids.contains(&pid))
        })
        .take(256)
    {
        task_line(&mut output, task, state, frame, width, style);
    }
    for task in state.finished_tasks.values().take(256) {
        task_line(&mut output, task, state, frame, width, style);
    }
    queued_commands(&mut output, state, style);
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
    truncate_to_boundary(&mut output, MAX_FRAME_BYTES);
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
    let status = state.service_status.get(&name);
    let icon = status_icon(status, frame.wall_seconds);
    if selected {
        out.push_str(style.inverse());
    }
    out.push_str("  ");
    out.push_str(icon.0);
    out.push_str(&pad(&name, 14));
    out.push_str(&format!(
        " {:>6}  {:>8}  {:>7}  {:>5}",
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
    if selected {
        let _ = out.pop();
        out.push_str(style.normal());
        out.push('\n');
    }
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
    let started = task
        .get("ref")
        .and_then(Value::as_str)
        .and_then(|reference| state.task_started_at.get(reference))
        .copied()
        .unwrap_or(frame.monotonic_seconds);
    let runtime = (frame.monotonic_seconds - started).max(0.0) as u64;
    out.push_str("  ");
    out.push_str(&pad(&name, 14));
    out.push_str(&format!(
        " {:>6}  {:>8}  {:>7}  {:>5}",
        pid,
        format_runtime(runtime),
        memory_mb(state.memory_cache.get(&pid)),
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
    let age = reference
        .and_then(|reference| state.last_log_at.get(reference))
        .map(|at| format_log_age((frame.wall_seconds - *at).max(0.0) as u64))
        .unwrap_or_default();
    let stream = log.get(1).map(value_text).unwrap_or_default();
    let line = log.get(2).map(value_text).unwrap_or_default();
    if stream == "stderr" {
        out.push_str(style.red());
    }
    out.push_str(&format!(" {:>5} ", age));
    out.push_str(&truncate_scalars(
        &line,
        width.saturating_sub(LOG_FIXED_WIDTH),
    ));
    if stream == "stderr" {
        out.push_str(style.normal());
    }
    out.push('\n');
}

fn observe_section(out: &mut String, state: &TopState, frame: FrameSample, style: &dyn TopStyle) {
    out.push_str("  ");
    out.push_str(style.bold());
    out.push_str("Observe");
    out.push_str(style.normal());
    reconnecting(out, state.continuity.observe.is_incomplete(), style);
    out.push(' ');
    if state.observe_last_ts > 0.0 {
        let age = (frame.wall_seconds - state.observe_last_ts).max(0.0);
        let color = if age < 30.0 {
            style.green()
        } else if age < 60.0 {
            style.yellow()
        } else {
            style.red()
        };
        out.push_str(color);
        out.push('●');
        out.push_str(style.normal());
    } else {
        out.push_str(style.dim());
        out.push('○');
        out.push_str(style.normal());
    }
    out.push('\n');
    if state.observe_status.is_empty() {
        out.push_str(style.dim());
        out.push_str("  (waiting for status)");
        out.push_str(style.normal());
        out.push('\n');
    } else {
        let mode = state.displayed_mode.as_str();
        let captures = state
            .observe_status
            .get("tmux")
            .and_then(|value| value.get("captures"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let describe = queue_status(state.observe_status.get("describe"));
        let transcribe = queue_status(state.observe_status.get("transcribe"));
        let locked = state
            .observe_status
            .get("activity")
            .and_then(|value| value.get("screen_locked"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mode_status = match mode {
            "screencast" => {
                let elapsed = state
                    .observe_status
                    .get("screencast")
                    .and_then(|value| value.get("window_elapsed_seconds"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                format!("[LIVE] screencast {}", format_uptime(elapsed))
            }
            "tmux" => format!("[TMUX] {captures} captures"),
            _ if locked => "[IDLE] locked".to_owned(),
            _ => "[IDLE]".to_owned(),
        };
        dynamic_line(
            out,
            &format!("  {mode_status} │ describe {describe} │ transcribe {transcribe}"),
            style,
        );
        if !state.recent_segments.is_empty() {
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
            dynamic_line(out, &format!("  Recent: {recent}"), style);
        }
    }
}

fn queued_commands(out: &mut String, state: &TopState, style: &dyn TopStyle) {
    let queued = state
        .command_queues
        .iter()
        .filter_map(|(command, count)| {
            count
                .as_f64()
                .filter(|count| *count > 0.0)
                .map(|count| format!("{command} ×{count}"))
        })
        .collect::<Vec<_>>();
    if !queued.is_empty() {
        dynamic_line(out, &format!("  queued: {}", queued.join(", ")), style);
    }
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

fn think_section(out: &mut String, state: &TopState, style: &dyn TopStyle) {
    out.push_str("  ");
    out.push_str(style.bold());
    out.push_str("Think");
    out.push_str(style.normal());
    reconnecting(out, state.continuity.think.is_incomplete(), style);
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
fn reconnecting(out: &mut String, is_reconnecting: bool, style: &dyn TopStyle) {
    if is_reconnecting {
        out.push_str(style.dim());
        out.push_str(" (reconnecting)");
        out.push_str(style.normal());
    }
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
