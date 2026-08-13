// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, CallosumSocketConnection};
use solstone_core_system_health::sanitize_for_terminal;
use tokio::time::{Instant, timeout};

const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn run(verbose: bool, debug: bool) -> std::process::ExitCode {
    let _ = (verbose, debug);
    let deadline = Instant::now() + STATUS_TIMEOUT;
    let journal = match super::resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => return super::print_journal_error(error),
    };
    let socket_path = journal.join("health").join("callosum.sock");
    match inspect_socket(&socket_path) {
        SocketInspection::InvalidUtf8 => {
            eprintln!("Cannot connect: callosum socket path is not valid UTF-8");
            return std::process::ExitCode::FAILURE;
        }
        SocketInspection::NotFound => {
            eprintln!(
                "Cannot connect: callosum socket not found at {}",
                sanitize_for_terminal(socket_path.to_str().expect("checked before inspection"))
            );
            return std::process::ExitCode::FAILURE;
        }
        SocketInspection::NotInspectable(reason) => {
            eprintln!(
                "Cannot connect: callosum socket is not inspectable: {}",
                sanitize_for_terminal(&reason)
            );
            return std::process::ExitCode::FAILURE;
        }
        SocketInspection::Present => {}
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!(
                "Cannot connect: health runtime unavailable: {}",
                sanitize_for_terminal(&error.to_string())
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    match runtime.block_on(fetch_status(&socket_path, deadline)) {
        Ok(status) => {
            print!("{}", render_status(&status));
            std::process::ExitCode::SUCCESS
        }
        Err(HealthFetchError::TimedOut) => {
            eprintln!("Timed out waiting for supervisor status (10s)");
            std::process::ExitCode::FAILURE
        }
        Err(HealthFetchError::MalformedFrames(count)) => {
            eprintln!(
                "Timed out waiting for supervisor status (10s; dropped {count} malformed frame(s))"
            );
            std::process::ExitCode::FAILURE
        }
        Err(HealthFetchError::StatusError { service, reason }) => {
            eprintln!(
                "Cannot connect: supervisor status error: {}: {}",
                sanitize_for_terminal(&service),
                sanitize_for_terminal(&reason)
            );
            std::process::ExitCode::FAILURE
        }
        Err(HealthFetchError::InvalidStatus { path }) => {
            eprintln!("{}", invalid_status_message(&path));
            std::process::ExitCode::FAILURE
        }
    }
}

fn invalid_status_message(path: &str) -> String {
    format!("Cannot connect: supervisor sent an invalid status payload at {path}")
}

enum SocketInspection {
    InvalidUtf8,
    NotFound,
    NotInspectable(String),
    Present,
}

fn inspect_socket(path: &Path) -> SocketInspection {
    if path.to_str().is_none() {
        return SocketInspection::InvalidUtf8;
    }
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SocketInspection::NotFound,
        Err(error) => SocketInspection::NotInspectable(error.to_string()),
        Ok(metadata) if !metadata.file_type().is_socket() => {
            SocketInspection::NotInspectable("path is not a Unix socket".to_owned())
        }
        Ok(_) => SocketInspection::Present,
    }
}

#[allow(async_fn_in_trait)]
trait StatusListener {
    async fn next_message(&mut self) -> Option<CallosumEnvelope>;
    fn malformed_frame_drops(&self) -> u64;
    async fn stop(&mut self);
}

impl StatusListener for CallosumSocketConnection {
    async fn next_message(&mut self) -> Option<CallosumEnvelope> {
        self.next_message().await
    }

    fn malformed_frame_drops(&self) -> u64 {
        self.malformed_frame_drops()
    }

    async fn stop(&mut self) {
        self.stop().await;
    }
}

async fn fetch_status(
    socket_path: &Path,
    deadline: Instant,
) -> Result<SupervisorStatus, HealthFetchError> {
    let mut listener = CallosumSocketConnection::new(socket_path, Map::new());
    listener.start();
    fetch_status_with_listener(listener, deadline).await
}

async fn fetch_status_with_listener<L: StatusListener>(
    mut listener: L,
    deadline: Instant,
) -> Result<SupervisorStatus, HealthFetchError> {
    let result = receive_status(&mut listener, deadline).await;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if !remaining.is_zero() {
        let _ = timeout(remaining, listener.stop()).await;
    }
    result
}

async fn receive_status<L: StatusListener>(
    listener: &mut L,
    deadline: Instant,
) -> Result<SupervisorStatus, HealthFetchError> {
    let malformed_before = listener.malformed_frame_drops();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Some(message) = timeout(remaining, listener.next_message())
            .await
            .ok()
            .flatten()
        else {
            break;
        };
        if message.tract != "supervisor" {
            continue;
        }
        if message.event == "status-error" {
            let error: StatusError = decode_status(Value::Object(message.extra))?;
            return Err(HealthFetchError::StatusError {
                service: error.service,
                reason: error.reason,
            });
        }
        if message.event == "status" {
            return decode_status(Value::Object(message.extra));
        }
    }
    let dropped = listener
        .malformed_frame_drops()
        .saturating_sub(malformed_before);
    if dropped == 0 {
        Err(HealthFetchError::TimedOut)
    } else {
        Err(HealthFetchError::MalformedFrames(dropped))
    }
}

enum HealthFetchError {
    TimedOut,
    MalformedFrames(u64),
    StatusError { service: String, reason: String },
    InvalidStatus { path: String },
}

fn decode_status<T: DeserializeOwned>(value: Value) -> Result<T, HealthFetchError> {
    let bytes = serde_json::to_vec(&value).expect("received Callosum values serialize");
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let path = stable_error_path(error.path().to_string(), &error.inner().to_string());
        HealthFetchError::InvalidStatus {
            path: if path.is_empty() {
                "$".to_owned()
            } else {
                path
            },
        }
    })
}

fn stable_error_path(path: String, detail: &str) -> String {
    let Some(field) = detail
        .strip_prefix("missing field \u{0060}")
        .and_then(|value| value.split('\u{0060}').next())
    else {
        return path;
    };
    if path.is_empty() {
        field.to_owned()
    } else {
        format!("{path}.{field}")
    }
}

#[derive(Deserialize)]
struct StatusError {
    service: String,
    reason: String,
}

// The wire format is additive: serde deliberately ignores future keys.
#[allow(dead_code)]
#[derive(Deserialize)]
struct SupervisorStatus {
    services: Vec<ServiceWireRow>,
    crashed: Vec<CrashedWireRow>,
    tasks: Vec<TaskWireRow>,
    recent_tasks: Vec<RecentTaskWireRow>,
    queues: BTreeMap<String, u64>,
    stale_heartbeats: Vec<String>,
    stale_heartbeat_details: Vec<StaleHeartbeatDetailWireRow>,
    schedules: Vec<ScheduleWireRow>,
    callosum_clients: usize,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct ServiceWireRow {
    name: String,
    pid: u32,
    uptime_seconds: u64,
    #[serde(rename = "ref")]
    reference: String,
    phase: String,
    reason_code: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct CrashedWireRow {
    name: String,
    restart_attempts: u32,
    phase: String,
    reason_code: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct TaskWireRow {
    #[serde(rename = "ref")]
    reference: String,
    name: String,
    max_runtime_seconds: u64,
    duration_seconds: u64,
    slow: bool,
    stuck: bool,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct RecentTaskWireRow {
    #[serde(rename = "ref")]
    reference: String,
    exit_status: String,
    scheduler_name: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct ScheduleWireRow {
    name: String,
    every: String,
    last_run: Option<f64>,
    due: bool,
    next_run: i64,
    daily_time: Option<String>,
    weekly_day: Option<String>,
    weekly_time: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct StaleHeartbeatDetailWireRow {
    hostname: String,
    machine_id_prefix: String,
    journal_path: String,
    pid: Option<u32>,
    wall_time: Option<String>,
    malformed: bool,
    reason_code: String,
}

fn format_uptime(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let days = seconds / 86_400;
    let remainder = seconds % 86_400;
    let hours = remainder / 3_600;
    let minutes = (remainder % 3_600) / 60;
    let mut parts = Vec::new();
    if days != 0 {
        parts.push(format!("{days}d"));
    }
    if hours != 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes != 0 {
        parts.push(format!("{minutes}m"));
    }
    parts.join(" ")
}

fn render_status(status: &SupervisorStatus) -> String {
    let mut output = String::from("Services:\n");
    for service in &status.services {
        let _ = writeln!(
            output,
            "  {:16} pid {}  uptime {}",
            sanitize_for_terminal(&format!("{:16}", service.name)),
            service.pid,
            format_uptime(service.uptime_seconds)
        );
    }
    if !status.crashed.is_empty() {
        output.push_str("\nCrashed:\n");
        for service in &status.crashed {
            let _ = writeln!(
                output,
                "  {:16} {} restart attempts",
                sanitize_for_terminal(&format!("{:16}", service.name)),
                service.restart_attempts
            );
        }
    }

    output.push('\n');
    let non_zero_queues: Vec<_> = status
        .queues
        .iter()
        .filter(|(_, count)| **count != 0)
        .collect();
    if status.tasks.is_empty() && non_zero_queues.is_empty() {
        output.push_str("Tasks: none\n");
    } else {
        output.push_str("Tasks:\n");
        for task in &status.tasks {
            let _ = write!(
                output,
                "  {:16} {}s",
                sanitize_for_terminal(&format!("{:16}", task.name)),
                task.duration_seconds
            );
            if task.stuck {
                let _ = write!(output, "  STUCK (cap {}s)", task.max_runtime_seconds);
            } else if task.slow {
                let _ = write!(output, "  SLOW (cap {}s)", task.max_runtime_seconds);
            }
            output.push('\n');
        }
        for (name, count) in non_zero_queues {
            let _ = writeln!(
                output,
                "  queued {} {count}",
                sanitize_for_terminal(&format!("{name:9}"))
            );
        }
    }
    if status.stale_heartbeats.is_empty() {
        output.push_str("Heartbeat: ok\n");
    } else {
        let stale = status
            .stale_heartbeats
            .iter()
            .map(|entry| sanitize_for_terminal(entry))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(output, "\nHeartbeat: STALE ({stale})");
    }
    let _ = writeln!(output, "Callosum: {} clients", status.callosum_clients);
    output
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;

    struct FakeListener {
        messages: VecDeque<Option<CallosumEnvelope>>,
        malformed: Cell<u64>,
        add_malformed_on_receive: bool,
        stop_never_resolves: bool,
    }

    #[allow(async_fn_in_trait)]
    impl StatusListener for FakeListener {
        async fn next_message(&mut self) -> Option<CallosumEnvelope> {
            if self.add_malformed_on_receive {
                self.malformed.set(self.malformed.get() + 1);
            }
            self.messages.pop_front().flatten()
        }

        fn malformed_frame_drops(&self) -> u64 {
            self.malformed.get()
        }

        async fn stop(&mut self) {
            if self.stop_never_resolves {
                std::future::pending::<()>().await;
            }
        }
    }

    fn envelope(tract: &str, event: &str, extra: Value) -> CallosumEnvelope {
        CallosumEnvelope {
            tract: tract.to_owned(),
            event: event.to_owned(),
            ts: None,
            extra: extra.as_object().unwrap().clone(),
        }
    }

    fn status_value() -> Value {
        json!({
            "services": [{"name": "convey\n", "pid": 3, "uptime_seconds": 3661,
                "ref": "ref", "phase": "running", "reason_code": null}],
            "crashed": [{"name": "local", "restart_attempts": 2,
                "phase": "failed", "reason_code": null}],
            "tasks": [{"ref": "task", "name": "daily\t", "max_runtime_seconds": 20,
                "duration_seconds": 21, "slow": true, "stuck": false}],
            "recent_tasks": [], "queues": {"z\u{001b}": 1, "zero": 0},
            "stale_heartbeats": ["host (path\r)"], "stale_heartbeat_details": [],
            "schedules": [], "callosum_clients": 2, "new_field": "accepted"
        })
    }

    #[test]
    fn renderer_matches_health_order_and_sanitizes_dynamic_text() {
        let status: SupervisorStatus = serde_json::from_value(status_value()).unwrap();
        assert_eq!(
            render_status(&status),
            "Services:\n  convey\\n          pid 3  uptime 1h 1m\n\nCrashed:\n  local            2 restart attempts\n\nTasks:\n  daily\\t           21s  SLOW (cap 20s)\n  queued z\\x1b        1\n\nHeartbeat: STALE (host (path\\r))\nCallosum: 2 clients\n"
        );
    }

    #[test]
    fn renderer_reports_empty_collections_without_optional_sections() {
        let status: SupervisorStatus = serde_json::from_value(json!({
            "services": [], "crashed": [], "tasks": [], "recent_tasks": [],
            "queues": {}, "stale_heartbeats": [], "stale_heartbeat_details": [],
            "schedules": [], "callosum_clients": 0
        }))
        .unwrap();
        assert_eq!(
            render_status(&status),
            "Services:\n\nTasks: none\nHeartbeat: ok\nCallosum: 0 clients\n"
        );
    }

    #[test]
    fn filters_envelopes_without_a_socket() {
        let mut listener = FakeListener {
            messages: VecDeque::from([
                Some(envelope("other", "status", status_value())),
                Some(envelope("supervisor", "other", status_value())),
                Some(envelope("supervisor", "status", status_value())),
            ]),
            malformed: Cell::new(0),
            add_malformed_on_receive: false,
            stop_never_resolves: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let result = runtime.block_on(receive_status(
            &mut listener,
            Instant::now() + Duration::from_secs(1),
        ));
        assert!(result.is_ok());
    }

    #[test]
    fn reports_malformed_frames_observed_before_timeout_without_a_socket() {
        let mut listener = FakeListener {
            messages: VecDeque::from([None]),
            malformed: Cell::new(0),
            add_malformed_on_receive: true,
            stop_never_resolves: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let result = runtime.block_on(receive_status(
            &mut listener,
            Instant::now() + Duration::from_millis(1),
        ));
        assert!(matches!(result, Err(HealthFetchError::MalformedFrames(1))));
    }

    #[test]
    fn invalid_status_reports_the_first_required_field_path() {
        let mut value = status_value();
        value["services"][0].as_object_mut().unwrap().remove("pid");
        let error = match decode_status::<SupervisorStatus>(value) {
            Ok(_) => panic!("missing field must fail"),
            Err(error) => error,
        };
        let HealthFetchError::InvalidStatus { path } = error else {
            panic!("expected invalid status");
        };
        assert_eq!(path, "services[0].pid");
        assert_eq!(
            invalid_status_message(&path),
            "Cannot connect: supervisor sent an invalid status payload at services[0].pid"
        );
    }

    #[test]
    fn expired_deadline_does_not_wait_for_listener_stop() {
        let listener = FakeListener {
            messages: VecDeque::new(),
            malformed: Cell::new(0),
            add_malformed_on_receive: false,
            stop_never_resolves: true,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let result = runtime.block_on(fetch_status_with_listener(listener, Instant::now()));
        assert!(matches!(result, Err(HealthFetchError::TimedOut)));
    }

    #[test]
    fn invalid_utf8_path_is_rejected_before_filesystem_inspection() {
        for bytes in [
            b"socket-\xff".as_slice(),
            b"socket-\n\xff".as_slice(),
            b"socket-\x1b\xff".as_slice(),
        ] {
            let path = PathBuf::from(OsString::from_vec(bytes.to_vec()));
            assert!(matches!(
                inspect_socket(&path),
                SocketInspection::InvalidUtf8
            ));
        }
    }
}
