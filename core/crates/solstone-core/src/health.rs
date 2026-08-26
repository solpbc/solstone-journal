// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use solstone_core_assets::canonical_host_pair;
use solstone_core_callosum::{CallosumEnvelope, CallosumSocketConnection};
use solstone_core_local::install::ced_readiness::{
    CED_READY_DETAIL, CED_UNAVAILABLE_GUIDANCE, CedReadiness, evaluate_ced_readiness,
};
use solstone_core_local::install::rfdetr_readiness::{
    RFDETR_READY_DETAIL, RFDETR_UNAVAILABLE_GUIDANCE, RfdetrReadiness, evaluate_rfdetr_readiness,
};
use solstone_core_system::process::SystemProcessInstanceSource;
use solstone_core_system_health::{
    SyncRescanDiagnosis, describe_sync_rescan, sanitize_for_terminal,
};
use tokio::time::{Instant, timeout};

const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_SYNC_DIAGNOSTIC_FILENAME: &str = "status-diagnostic.check";

fn deadline_after(limit: Duration) -> Instant {
    Instant::now() + limit
}

pub(super) fn run(verbose: bool, debug: bool) -> std::process::ExitCode {
    let _ = (verbose, debug);
    let deadline = deadline_after(STATUS_TIMEOUT);
    let journal = match super::resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => return super::print_journal_error(error),
    };
    let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    let ced = evaluate_ced_readiness(&journal, os, arch);
    let rfdetr = evaluate_rfdetr_readiness(&journal, os, arch);
    let socket_path = journal.join("health").join("callosum.sock");
    let fetch = match inspect_socket(&socket_path) {
        SocketInspection::InvalidUtf8 => Err(PresentedHealthError::InvalidUtf8),
        SocketInspection::NotFound => Err(PresentedHealthError::NotFound {
            path: socket_path
                .to_str()
                .expect("checked before inspection")
                .to_owned(),
        }),
        SocketInspection::NotInspectable(reason) => {
            Err(PresentedHealthError::NotInspectable { reason })
        }
        SocketInspection::Present => {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime
                    .block_on(fetch_status(&socket_path, deadline))
                    .map_err(PresentedHealthError::Fetch),
                Err(error) => Err(PresentedHealthError::RuntimeUnavailable {
                    message: error.to_string(),
                }),
            }
        }
    };
    let sync_diagnosis = should_rescan_sync(&fetch)
        .then(|| no_supervisor_sync_diagnosis(&journal))
        .flatten();
    let (stdout, stderr, code) = present_health(&ced, &rfdetr, fetch);
    let (stdout, stderr, code) = match sync_diagnosis {
        Some(message) => (
            stdout,
            format!("{message}\n"),
            std::process::ExitCode::FAILURE,
        ),
        None => (stdout, stderr, code),
    };
    print!("{stdout}");
    eprint!("{stderr}");
    code
}

fn no_supervisor_sync_diagnosis(journal: &Path) -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64());
    let process_source = SystemProcessInstanceSource;
    match describe_sync_rescan(
        journal,
        STATUS_SYNC_DIAGNOSTIC_FILENAME,
        now,
        &process_source,
    ) {
        SyncRescanDiagnosis::Clean(_) => None,
        SyncRescanDiagnosis::Waiting(message)
        | SyncRescanDiagnosis::HeartbeatNeedsAttention(message)
        | SyncRescanDiagnosis::AdmissionWaitNeedsAttention(message)
        | SyncRescanDiagnosis::Unsafe(message) => Some(message),
    }
}

enum PresentedHealthError {
    InvalidUtf8,
    NotFound { path: String },
    NotInspectable { reason: String },
    RuntimeUnavailable { message: String },
    Fetch(HealthFetchError),
}

fn should_rescan_sync(fetch: &Result<SupervisorStatus, PresentedHealthError>) -> bool {
    matches!(
        fetch,
        Err(PresentedHealthError::InvalidUtf8
            | PresentedHealthError::NotFound { .. }
            | PresentedHealthError::NotInspectable { .. }
            | PresentedHealthError::RuntimeUnavailable { .. })
    )
}

fn ced_line(ced: &CedReadiness) -> String {
    match ced {
        CedReadiness::Ready { .. } => CED_READY_DETAIL.to_owned(),
        CedReadiness::Degraded { .. } => CED_UNAVAILABLE_GUIDANCE.to_owned(),
        CedReadiness::Unsupported { os, arch } => {
            format!("ced install: unsupported platform {os}/{arch}; skipping ced sound-tag assets")
        }
    }
}

fn rfdetr_line(rfdetr: &RfdetrReadiness) -> String {
    match rfdetr {
        RfdetrReadiness::Ready { .. } => RFDETR_READY_DETAIL.to_owned(),
        RfdetrReadiness::Degraded { .. } => RFDETR_UNAVAILABLE_GUIDANCE.to_owned(),
        RfdetrReadiness::Unsupported { os, arch } => {
            format!(
                "rf-detr install: unsupported platform {os}/{arch}; skipping rf-detr object-detection assets"
            )
        }
    }
}

fn present_health(
    ced: &CedReadiness,
    rfdetr: &RfdetrReadiness,
    fetch: Result<SupervisorStatus, PresentedHealthError>,
) -> (String, String, std::process::ExitCode) {
    let stdout = format!("{}\n{}\n", ced_line(ced), rfdetr_line(rfdetr));
    match fetch {
        Ok(status) if matches!(rfdetr, RfdetrReadiness::Ready { .. }) => (
            format!("{stdout}{}", render_status(&status)),
            String::new(),
            std::process::ExitCode::SUCCESS,
        ),
        Ok(status) => (
            format!("{stdout}{}", render_status(&status)),
            format!("{RFDETR_UNAVAILABLE_GUIDANCE}\n"),
            std::process::ExitCode::FAILURE,
        ),
        Err(PresentedHealthError::InvalidUtf8) => (
            stdout,
            "Cannot connect: callosum socket path is not valid UTF-8\n".to_owned(),
            std::process::ExitCode::FAILURE,
        ),
        Err(PresentedHealthError::NotFound { path }) => (
            stdout,
            format!(
                "Cannot connect: callosum socket not found at {}\n",
                sanitize_for_terminal(&path)
            ),
            std::process::ExitCode::FAILURE,
        ),
        Err(PresentedHealthError::NotInspectable { reason }) => (
            stdout,
            format!(
                "Cannot connect: callosum socket is not inspectable: {}\n",
                sanitize_for_terminal(&reason)
            ),
            std::process::ExitCode::FAILURE,
        ),
        Err(PresentedHealthError::RuntimeUnavailable { message }) => (
            stdout,
            format!(
                "Cannot connect: health runtime unavailable: {}\n",
                sanitize_for_terminal(&message)
            ),
            std::process::ExitCode::FAILURE,
        ),
        Err(PresentedHealthError::Fetch(HealthFetchError::TimedOut)) => (
            stdout,
            "Timed out waiting for supervisor status (10s)\n".to_owned(),
            std::process::ExitCode::FAILURE,
        ),
        Err(PresentedHealthError::Fetch(HealthFetchError::MalformedFrames(count))) => (
            stdout,
            format!(
                "Timed out waiting for supervisor status (10s; dropped {count} malformed frame(s))\n"
            ),
            std::process::ExitCode::FAILURE,
        ),
        Err(PresentedHealthError::Fetch(HealthFetchError::StatusError { service, reason })) => (
            stdout,
            format!(
                "Cannot connect: supervisor status error: {}: {}\n",
                sanitize_for_terminal(&service),
                sanitize_for_terminal(&reason)
            ),
            std::process::ExitCode::FAILURE,
        ),
        Err(PresentedHealthError::Fetch(HealthFetchError::InvalidStatus { path })) => (
            stdout,
            format!("{}\n", invalid_status_message(&path)),
            std::process::ExitCode::FAILURE,
        ),
    }
}

fn invalid_status_message(path: &str) -> String {
    format!(
        "Cannot connect: supervisor sent an invalid status payload at {}",
        sanitize_for_terminal(path)
    )
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
            let name = sanitize_for_terminal(&format!("{:16}", service.name));
            match service.reason_code.as_deref() {
                None => {
                    let _ = writeln!(
                        output,
                        "  {name:16} {} restart attempts",
                        service.restart_attempts
                    );
                }
                Some(code) => {
                    let _ = writeln!(
                        output,
                        "  {name:16} {} restart attempts  {}",
                        service.restart_attempts,
                        sanitize_for_terminal(code)
                    );
                }
            }
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
    fn present_health_prefixes_ced_on_success() {
        let status: SupervisorStatus = serde_json::from_value(status_value()).unwrap();
        let rendered = render_status(&status);
        let ced = CedReadiness::Ready {
            library: PathBuf::from("libced.so"),
            model: PathBuf::from("model.gguf"),
        };
        let rfdetr = RfdetrReadiness::Ready {
            binary: PathBuf::from("rfdetr-cli"),
            model: PathBuf::from("rfdetr-nano-f16.gguf"),
        };
        let (stdout, stderr, code) = present_health(&ced, &rfdetr, Ok(status));
        assert!(stdout.starts_with(&format!("{CED_READY_DETAIL}\n")));
        assert!(stdout.contains(RFDETR_READY_DETAIL));
        assert!(stdout.contains("Services:"));
        assert!(stderr.is_empty());
        assert_eq!(code, std::process::ExitCode::SUCCESS);
        assert!(stdout.ends_with(&rendered));
    }

    #[test]
    fn present_health_prefixes_ced_on_connection_failure() {
        let ced = CedReadiness::Degraded {
            cause: solstone_core_local::install::ced_readiness::CedDegradedCause::Absent,
            detail: "sidecar missing".to_owned(),
        };
        let rfdetr = RfdetrReadiness::Ready {
            binary: PathBuf::from("rfdetr-cli"),
            model: PathBuf::from("rfdetr-nano-f16.gguf"),
        };
        let (stdout, stderr, code) = present_health(
            &ced,
            &rfdetr,
            Err(PresentedHealthError::NotFound {
                path: "/journal/health/callosum.sock".to_owned(),
            }),
        );
        assert_eq!(
            stdout,
            format!("{CED_UNAVAILABLE_GUIDANCE}\n{RFDETR_READY_DETAIL}\n")
        );
        assert_eq!(
            stderr,
            "Cannot connect: callosum socket not found at /journal/health/callosum.sock\n"
        );
        assert_eq!(code, std::process::ExitCode::FAILURE);
    }

    #[test]
    fn rescan_covers_every_pre_connection_failure_but_not_socket_fetch_failures() {
        let not_found: Result<SupervisorStatus, PresentedHealthError> =
            Err(PresentedHealthError::NotFound {
                path: "/journal/health/callosum.sock".to_owned(),
            });
        let not_inspectable = Err(PresentedHealthError::NotInspectable {
            reason: "not a socket".to_owned(),
        });
        let runtime_unavailable = Err(PresentedHealthError::RuntimeUnavailable {
            message: "runtime unavailable".to_owned(),
        });
        let invalid_utf8 = Err(PresentedHealthError::InvalidUtf8);
        let fetch_failed = Err(PresentedHealthError::Fetch(HealthFetchError::TimedOut));

        assert!(should_rescan_sync(&not_found));
        assert!(should_rescan_sync(&not_inspectable));
        assert!(should_rescan_sync(&runtime_unavailable));
        assert!(should_rescan_sync(&invalid_utf8));
        assert!(!should_rescan_sync(&fetch_failed));
    }

    #[test]
    fn present_health_fails_for_degraded_rfdetr_with_a_healthy_supervisor() {
        let status: SupervisorStatus = serde_json::from_value(status_value()).unwrap();
        let ced = CedReadiness::Degraded {
            cause: solstone_core_local::install::ced_readiness::CedDegradedCause::Absent,
            detail: "sidecar missing".to_owned(),
        };
        let rfdetr = RfdetrReadiness::Degraded {
            cause: solstone_core_local::install::rfdetr_readiness::RfdetrDegradedCause::Absent,
            detail: "sidecar missing".to_owned(),
        };
        let (_stdout, stderr, code) = present_health(&ced, &rfdetr, Ok(status));
        assert_eq!(stderr, format!("{RFDETR_UNAVAILABLE_GUIDANCE}\n"));
        assert_eq!(code, std::process::ExitCode::FAILURE);
    }

    #[test]
    fn present_health_keeps_ced_degradation_non_gating_when_rfdetr_is_ready() {
        let status: SupervisorStatus = serde_json::from_value(status_value()).unwrap();
        let ced = CedReadiness::Degraded {
            cause: solstone_core_local::install::ced_readiness::CedDegradedCause::Absent,
            detail: "sidecar missing".to_owned(),
        };
        let rfdetr = RfdetrReadiness::Ready {
            binary: PathBuf::from("rfdetr-cli"),
            model: PathBuf::from("rfdetr-nano-f16.gguf"),
        };
        let (_stdout, stderr, code) = present_health(&ced, &rfdetr, Ok(status));
        assert!(stderr.is_empty());
        assert_eq!(code, std::process::ExitCode::SUCCESS);
    }

    #[test]
    fn renderer_appends_sanitized_reason_code_on_the_same_crashed_line() {
        let mut value = status_value();
        value["crashed"] = json!([
            {"name": "local", "restart_attempts": 2, "phase": "failed", "reason_code": null},
            {"name": "convey", "restart_attempts": 5, "phase": "failed", "reason_code": "exit 1"},
            {
                "name": "sense",
                "restart_attempts": 1,
                "phase": "failed",
                "reason_code": "bad\n\u{001b}"
            }
        ]);
        let status: SupervisorStatus = serde_json::from_value(value).unwrap();
        let rendered = render_status(&status);
        assert!(
            rendered.contains("  local            2 restart attempts\n"),
            "null reason_code must keep the existing line: {rendered}"
        );
        assert!(
            rendered.contains("  convey           5 restart attempts  exit 1\n"),
            "plain reason_code must be a same-line suffix: {rendered}"
        );
        assert!(
            rendered.contains("  sense            1 restart attempts  bad\\n\\x1b\n"),
            "hostile reason_code must be sanitized on the same line: {rendered}"
        );
        assert_eq!(
            rendered
                .lines()
                .filter(|line| line.contains("restart attempts"))
                .count(),
            3,
            "each crashed row stays one line: {rendered}"
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
            deadline_after(Duration::from_secs(1)),
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
            deadline_after(Duration::from_millis(1)),
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
    fn invalid_status_sanitizes_dynamic_map_key_paths_exactly_once() {
        let hostile = "bad\n\u{001b}\u{202e}\\literal";
        let mut value = status_value();
        value["queues"] = json!({hostile: "not-a-count"});
        let error = match decode_status::<SupervisorStatus>(value) {
            Ok(_) => panic!("hostile map value must fail"),
            Err(error) => error,
        };
        let HealthFetchError::InvalidStatus { path } = error else {
            panic!("expected invalid status");
        };
        assert!(path.contains(hostile));
        let rendered = invalid_status_message(&path);
        assert_eq!(rendered.lines().count(), 1);
        assert!(!rendered.contains('\u{001b}'));
        assert!(rendered.contains("\\n\\x1b\\u{202e}\\\\literal"));
        assert!(
            !rendered.contains("\\\\n"),
            "escape must occur exactly once"
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
        let result = runtime.block_on(fetch_status_with_listener(
            listener,
            deadline_after(Duration::ZERO),
        ));
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
