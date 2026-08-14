// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use solstone_core_brain::BrainInspection;
use uuid::Uuid;

use crate::state::Outbound;

#[cfg(test)]
use std::path::Path;

pub(crate) const SPP_RENEWAL_ATTEMPT_BOUND_S: u64 = 120;
pub(crate) const SPP_REFRESH_OBSERVATION_BOUND_S: u64 = 300;
pub(crate) const SPP_RENEWAL_RETRY_DELAYS_S: [u64; 5] = [5, 10, 20, 40, 60];
pub(crate) const SPP_RENEWAL_ACK_TIMEOUT_S: u64 = 15;
pub(crate) const SPP_RENEWAL_MAX_WAIT_S: u64 = 60;
pub(crate) const SPP_RENEWAL_DISABLED_IDLE_S: u64 = 30;
pub(crate) const SPP_RENEWAL_PROACTIVE_MARGIN_S: u64 =
    SPP_RENEWAL_ATTEMPT_BOUND_S + SPP_RENEWAL_RETRY_DELAYS_S[0];

const _: () = assert!(
    SPP_RENEWAL_PROACTIVE_MARGIN_S < solstone_core_spp_ratls::TPM_HEARTBEAT_INTERVAL.as_secs() / 2
);

pub(crate) type Now = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync + 'static>;
pub(crate) type Wait = Arc<dyn Fn(Duration) -> bool + Send + Sync + 'static>;

#[derive(Clone, Debug)]
pub(crate) enum RenewalBrainError {
    Read(String),
}

impl std::fmt::Display for RenewalBrainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RenewalBrainError {}

pub(crate) trait RenewalBrain: Send + Sync {
    fn inspect(&self, now: DateTime<Utc>) -> Result<BrainInspection, RenewalBrainError>;
    fn active_fingerprint(&self) -> Result<Option<String>, RenewalBrainError>;
}

pub(crate) struct BrainAdapter {
    journal: PathBuf,
}

impl BrainAdapter {
    pub(crate) fn new(journal: PathBuf) -> Self {
        Self { journal }
    }

    fn config(&self) -> Result<Map<String, Value>, RenewalBrainError> {
        solstone_core_brain::read_journal_config(&self.journal)
            .map_err(|error| RenewalBrainError::Read(error.to_string()))
            .map(|read| read.config.unwrap_or_default())
    }
}

impl RenewalBrain for BrainAdapter {
    fn inspect(&self, now: DateTime<Utc>) -> Result<BrainInspection, RenewalBrainError> {
        let config = self.config()?;
        Ok(solstone_core_brain::inspect_brain_state(
            &self.journal,
            &config,
            now,
        ))
    }

    fn active_fingerprint(&self) -> Result<Option<String>, RenewalBrainError> {
        let config = self.config()?;
        let Some(key) = solstone_core_brain::load_existing_fingerprint_key(&self.journal) else {
            return Ok(None);
        };
        Ok(
            solstone_core_brain::build_active_brain_fingerprint(&config, &key, None)
                .ok()
                .flatten(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AttemptKind {
    Renew { fingerprint: String },
    RefreshExpectedFingerprint { fingerprint: String },
    RefreshExpectAbsent,
}

impl AttemptKind {
    fn action(&self) -> &'static str {
        match self {
            Self::Renew { .. } => "renew",
            Self::RefreshExpectedFingerprint { .. } | Self::RefreshExpectAbsent => "refresh",
        }
    }

    fn observation_bound(&self) -> Duration {
        match self {
            Self::Renew { .. } => Duration::from_secs(SPP_RENEWAL_ATTEMPT_BOUND_S),
            Self::RefreshExpectedFingerprint { .. } | Self::RefreshExpectAbsent => {
                Duration::from_secs(SPP_REFRESH_OBSERVATION_BOUND_S)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attempt {
    kind: AttemptKind,
    reference: String,
    observed_before: Option<DateTime<Utc>>,
    expires_before: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PlanOutcome {
    Disabled,
    Checking,
    Wait(Duration),
    Start {
        kind: AttemptKind,
        observed_before: Option<DateTime<Utc>>,
        expires_before: Option<DateTime<Utc>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerificationResult {
    Verified,
    Rejected,
}

#[derive(Clone, Debug)]
struct RenewalEvent {
    state: &'static str,
    fields: Vec<RenewalField>,
}

#[derive(Clone, Debug)]
pub(crate) struct RenewalField {
    key: &'static str,
    value: Option<RenewalFieldValue>,
}

#[derive(Clone, Debug)]
pub(crate) enum RenewalFieldValue {
    Text(String),
    Integer(i32),
    Seconds(f64),
}

impl RenewalField {
    fn text(key: &'static str, value: impl Into<String>) -> Self {
        Self {
            key,
            value: Some(RenewalFieldValue::Text(value.into())),
        }
    }

    fn integer(key: &'static str, value: i32) -> Self {
        Self {
            key,
            value: Some(RenewalFieldValue::Integer(value)),
        }
    }

    fn seconds(key: &'static str, value: f64) -> Self {
        Self {
            key,
            value: Some(RenewalFieldValue::Seconds(value)),
        }
    }
}

pub(crate) trait RenewalDiagnostics: Send {
    fn emit(&mut self, rendered: &str);
}

pub(crate) struct StderrRenewalDiagnostics<W: Write + Send> {
    sink: W,
}

impl<W: Write + Send> StderrRenewalDiagnostics<W> {
    pub(crate) fn new(sink: W) -> Self {
        Self { sink }
    }
}

impl<W: Write + Send> RenewalDiagnostics for StderrRenewalDiagnostics<W> {
    fn emit(&mut self, rendered: &str) {
        let _ = writeln!(self.sink, "{rendered}");
    }
}

pub(crate) fn format_renewal_event(state: &str, fields: &[RenewalField]) -> String {
    let mut sorted = BTreeMap::new();
    for field in fields {
        if let Some(value) = &field.value {
            let rendered = match value {
                RenewalFieldValue::Text(value) => value.clone(),
                RenewalFieldValue::Integer(value) => value.to_string(),
                RenewalFieldValue::Seconds(value) => python_float(*value),
            };
            sorted.insert(field.key, rendered);
        }
    }
    let suffix = sorted
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(" ");
    if suffix.is_empty() {
        format!("event=spp_renewal_{state}")
    } else {
        format!("event=spp_renewal_{state} {suffix}")
    }
}

fn python_float(value: f64) -> String {
    let mut rendered = value.to_string();
    if !rendered.contains(['.', 'e', 'E']) {
        rendered.push_str(".0");
    }
    rendered
}

#[derive(Default)]
pub(crate) struct RenewalMachine {
    pending: Option<Attempt>,
    ack_deadline: Option<DateTime<Utc>>,
    running: Option<Attempt>,
    running_deadline: Option<DateTime<Utc>>,
    successor_after_ref: Option<String>,
    successor_deadline: Option<DateTime<Utc>>,
    retry_index: usize,
    retry_after: Option<DateTime<Utc>>,
    last_mode: Option<&'static str>,
    events: Vec<RenewalEvent>,
}

impl RenewalMachine {
    fn event(&mut self, state: &'static str, fields: Vec<RenewalField>) {
        self.events.push(RenewalEvent { state, fields });
    }

    fn take_events(&mut self) -> Vec<RenewalEvent> {
        std::mem::take(&mut self.events)
    }

    fn schedule_retry(&mut self, now: DateTime<Utc>) {
        let delay = SPP_RENEWAL_RETRY_DELAYS_S[self
            .retry_index
            .min(SPP_RENEWAL_RETRY_DELAYS_S.len().saturating_sub(1))];
        self.retry_index += 1;
        self.retry_after = Some(now + chrono::Duration::seconds(delay as i64));
        self.event(
            "retrying",
            vec![RenewalField::seconds("delay_s", delay as f64)],
        );
    }

    fn clear_pending(&mut self, clear_retry: bool) {
        self.pending = None;
        self.ack_deadline = None;
        if clear_retry {
            self.retry_after = None;
        }
    }

    fn clear_running(&mut self) {
        self.running = None;
        self.running_deadline = None;
    }

    fn clear_successor(&mut self) {
        self.successor_after_ref = None;
        self.successor_deadline = None;
    }

    fn clear_demand(&mut self) {
        self.clear_pending(true);
        self.clear_running();
        self.clear_successor();
        self.retry_index = 0;
        self.retry_after = None;
    }

    fn step_error(&mut self, now: DateTime<Utc>, reason: &str) {
        self.event("failed", vec![RenewalField::text("reason", reason)]);
        self.clear_pending(false);
        self.clear_running();
        self.clear_successor();
        self.schedule_retry(now);
    }

    fn disabled(&mut self, disabled: bool) -> Option<Duration> {
        if !disabled {
            return None;
        }
        self.clear_demand();
        if self.last_mode != Some("disabled") {
            self.event(
                "disabled",
                vec![RenewalField::text("reason", "non_spp_lane")],
            );
        }
        self.last_mode = Some("disabled");
        Some(Duration::from_secs(SPP_RENEWAL_DISABLED_IDLE_S))
    }

    fn state_delay(&mut self, now: DateTime<Utc>) -> Option<Duration> {
        if let Some(pending) = &self.pending {
            if self.ack_deadline.is_some_and(|deadline| now >= deadline) {
                self.event(
                    "failed",
                    vec![
                        RenewalField::text("reason", "start_ack_timeout"),
                        RenewalField::text("ref", pending.reference.clone()),
                    ],
                );
                self.clear_pending(true);
                self.schedule_retry(now);
            }
            return Some(seconds_until(self.ack_deadline, now, 5));
        }
        if let Some(running) = &self.running {
            if self
                .running_deadline
                .is_some_and(|deadline| now >= deadline)
            {
                self.event(
                    "stale",
                    vec![
                        RenewalField::text("reason", "running_observation_timeout"),
                        RenewalField::text("ref", running.reference.clone()),
                    ],
                );
                self.clear_running();
                self.schedule_retry(now);
                return Some(seconds_until(self.retry_after, now, 5));
            }
            return Some(seconds_until(self.running_deadline, now, 5));
        }
        if let Some(reference) = &self.successor_after_ref {
            if self
                .successor_deadline
                .is_some_and(|deadline| now < deadline)
            {
                return Some(seconds_until(self.successor_deadline, now, 5));
            }
            self.event(
                "stale",
                vec![
                    RenewalField::text("reason", "successor_observation_timeout"),
                    RenewalField::text("active_ref", reference.clone()),
                ],
            );
            self.clear_successor();
        }
        if let Some(retry_after) = self.retry_after {
            if now < retry_after {
                return Some(seconds_until(Some(retry_after), now, 5));
            }
            self.retry_after = None;
        }
        None
    }

    pub(crate) fn plan(
        &mut self,
        now: DateTime<Utc>,
        inspection: &BrainInspection,
        active_fingerprint: Option<String>,
    ) -> PlanOutcome {
        let projection = &inspection.projection;
        if projection.active_lane.as_deref() != Some("spp") {
            self.last_mode = Some("disabled");
            return PlanOutcome::Disabled;
        }
        if projection.aggregate_state == "checking" {
            self.last_mode = Some("checking");
            return PlanOutcome::Checking;
        }
        let Some(fingerprint) = active_fingerprint else {
            self.last_mode = Some("refresh");
            return PlanOutcome::Start {
                kind: AttemptKind::RefreshExpectAbsent,
                observed_before: None,
                expires_before: None,
            };
        };
        let record = inspection.record.as_ref().and_then(Value::as_object);
        let component = record
            .and_then(|record| record.get("evidence"))
            .and_then(Value::as_object)
            .and_then(|evidence| evidence.get("lane_prerequisites"))
            .and_then(Value::as_object);
        let observed = component
            .and_then(|component| component.get("observed_at"))
            .and_then(Value::as_str)
            .and_then(parse_time);
        let expires = component
            .and_then(|component| component.get("expires_at"))
            .and_then(Value::as_str)
            .and_then(parse_time);
        let refresh = projection.aggregate_state != "ready"
            || record.is_none()
            || record
                .and_then(|record| record.get("fingerprint_sha256"))
                .and_then(Value::as_str)
                != Some(fingerprint.as_str())
            || component.is_none()
            || component
                .and_then(|component| component.get("status"))
                .and_then(Value::as_str)
                != Some("ok")
            || observed.is_none()
            || expires.is_none()
            || expires.is_some_and(|expires| now >= expires);
        if refresh {
            self.last_mode = Some("refresh");
            return PlanOutcome::Start {
                kind: AttemptKind::RefreshExpectedFingerprint { fingerprint },
                observed_before: observed,
                expires_before: expires,
            };
        }
        let expires = expires.expect("checked above");
        let renew_at = expires - chrono::Duration::seconds(SPP_RENEWAL_PROACTIVE_MARGIN_S as i64);
        if now < renew_at {
            let delay = (renew_at - now).to_std().unwrap_or_default();
            self.event(
                "scheduled",
                vec![RenewalField::seconds(
                    "delay_s",
                    (delay.as_millis() as f64 / 1000.0 * 1000.0).round() / 1000.0,
                )],
            );
            self.last_mode = Some("wait");
            return PlanOutcome::Wait(delay);
        }
        self.last_mode = Some("renew");
        PlanOutcome::Start {
            kind: AttemptKind::Renew { fingerprint },
            observed_before: observed,
            expires_before: Some(expires),
        }
    }

    fn mark_pending(&mut self, now: DateTime<Utc>, attempt: Attempt) {
        self.event(
            "in_flight",
            vec![
                RenewalField::text("action", attempt.kind.action()),
                RenewalField::text("ref", attempt.reference.clone()),
            ],
        );
        self.ack_deadline = Some(now + chrono::Duration::seconds(SPP_RENEWAL_ACK_TIMEOUT_S as i64));
        self.pending = Some(attempt);
    }

    fn verification_result(
        attempt: &Attempt,
        exit_code: i32,
        inspection: &BrainInspection,
        active_fingerprint: Option<String>,
    ) -> VerificationResult {
        let persisted = |expected: &str, require_ready: bool| {
            let projection = &inspection.projection;
            if active_fingerprint.as_deref() != Some(expected)
                || projection.active_lane.as_deref() != Some("spp")
                || (require_ready && projection.aggregate_state != "ready")
            {
                return false;
            }
            let Some(record) = inspection.record.as_ref().and_then(Value::as_object) else {
                return false;
            };
            if record.get("active_lane").and_then(Value::as_str) != Some("spp")
                || record.get("fingerprint_sha256").and_then(Value::as_str) != Some(expected)
            {
                return false;
            }
            let component = record
                .get("evidence")
                .and_then(Value::as_object)
                .and_then(|evidence| evidence.get("lane_prerequisites"))
                .and_then(Value::as_object);
            let Some(component) = component else {
                return false;
            };
            if component.get("status").and_then(Value::as_str) != Some("ok") {
                return false;
            }
            let observed = component
                .get("observed_at")
                .and_then(Value::as_str)
                .and_then(parse_time);
            let expires = component
                .get("expires_at")
                .and_then(Value::as_str)
                .and_then(parse_time);
            observed.is_some_and(|observed| {
                attempt
                    .observed_before
                    .is_none_or(|before| observed > before)
            }) && expires
                .is_some_and(|expires| attempt.expires_before.is_none_or(|before| expires > before))
        };
        let verified = match &attempt.kind {
            AttemptKind::Renew { fingerprint } => persisted(fingerprint, false),
            AttemptKind::RefreshExpectedFingerprint { fingerprint } => {
                exit_code == 0 && persisted(fingerprint, true)
            }
            AttemptKind::RefreshExpectAbsent => {
                exit_code == 0
                    && active_fingerprint
                        .as_deref()
                        .is_some_and(|fingerprint| persisted(fingerprint, true))
            }
        };
        if verified {
            VerificationResult::Verified
        } else {
            VerificationResult::Rejected
        }
    }

    fn apply_verification(
        &mut self,
        now: DateTime<Utc>,
        attempt: Attempt,
        result: VerificationResult,
        exit: i32,
    ) {
        match result {
            VerificationResult::Verified => {
                self.retry_index = 0;
                self.retry_after = None;
                let mut fields = vec![RenewalField::text("ref", attempt.reference)];
                if attempt.kind.action() == "refresh" {
                    fields.push(RenewalField::text("action", "refresh"));
                }
                self.event("verified", fields);
            }
            VerificationResult::Rejected => {
                self.event(
                    "failed",
                    vec![
                        RenewalField::text("action", attempt.kind.action()),
                        RenewalField::integer("exit_code", exit),
                        RenewalField::text("ref", attempt.reference),
                    ],
                );
                self.schedule_retry(now);
            }
        }
    }

    #[cfg(test)]
    fn snapshot(&self) -> MachineSnapshot {
        MachineSnapshot {
            pending_ref: self
                .pending
                .as_ref()
                .map(|attempt| attempt.reference.clone()),
            running_ref: self
                .running
                .as_ref()
                .map(|attempt| attempt.reference.clone()),
            successor_ref: self.successor_after_ref.clone(),
            ack_deadline: self.ack_deadline,
            running_deadline: self.running_deadline,
            successor_deadline: self.successor_deadline,
            retry_index: self.retry_index,
        }
    }
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MachineSnapshot {
    pub(crate) pending_ref: Option<String>,
    pub(crate) running_ref: Option<String>,
    pub(crate) successor_ref: Option<String>,
    pub(crate) ack_deadline: Option<DateTime<Utc>>,
    pub(crate) running_deadline: Option<DateTime<Utc>>,
    pub(crate) successor_deadline: Option<DateTime<Utc>>,
    pub(crate) retry_index: usize,
}

fn seconds_until(deadline: Option<DateTime<Utc>>, now: DateTime<Utc>, default: u64) -> Duration {
    deadline
        .and_then(|deadline| (deadline - now).to_std().ok())
        .unwrap_or_else(|| Duration::from_secs(default))
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

pub(crate) fn exit_code(fields: &Map<String, Value>) -> i32 {
    fields
        .get("exit_code")
        .and_then(|value| match value {
            Value::Number(value) => value.as_i64().and_then(|value| i32::try_from(value).ok()),
            Value::String(value) => value.parse::<i32>().ok(),
            _ => None,
        })
        .unwrap_or(-1)
}

#[cfg(test)]
pub(crate) fn write_valid_test_journal(directory: &Path) {
    std::fs::create_dir_all(directory.join("config")).unwrap();
    std::fs::create_dir_all(directory.join("health")).unwrap();
    std::fs::write(
        directory.join("config/journal.json"),
        serde_json::to_vec(&json!({
            "providers":{"active":{"model":"served-model","provider":"local"},"local":{"credential":"endpoint-credential","endpoint_url":"http://127.0.0.1:9099","served_model_id":"served-model"}},
            "services":{"confidential":{"credential_fingerprint_sha256":"cca56da30e3c8a13a11277193fd3263961e2e3d6d9f98038a91dac05e8fde16a","endpoint_url":"http://127.0.0.1:9099","prior_active":{"provider":"local"},"served_model_id":"served-model"}}
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        directory.join("health/brain-fingerprint.key"),
        (0u8..32).collect::<Vec<_>>(),
    )
    .unwrap();
    std::fs::write(
        directory.join("health/brain.json"),
        serde_json::to_vec(&json!({
            "active_lane":"spp","active_model":"served-model","active_provider":"local","aggregate_state":"ready","checking":null,"diagnostic":{},
            "evidence":{
                "cogitate":{"expires_at":"2026-08-07T13:00:00Z","observed_at":"2026-08-06T11:59:00Z","status":"ok"},
                "configuration":{"expires_at":"2026-08-07T13:00:00Z","observed_at":"2026-08-06T11:59:00Z","status":"ok"},
                "generate":{"expires_at":"2026-08-07T13:00:00Z","observed_at":"2026-08-06T11:59:00Z","status":"ok"},
                "lane_prerequisites":{"expires_at":"2026-08-07T13:00:00Z","observed_at":"2026-08-06T11:59:00Z","status":"ok"}
            },
            "fingerprint_sha256":"a27e8e81200b4171bb71a571e0b20b319c8943651cab704ac93530017a06d2ec","reason_code":null,"revision":3,"runtime_failure_marker":null,"schema_version":1,"updated_at":"2026-08-06T12:00:00+00:00"
        }))
        .unwrap(),
    )
    .unwrap();
}

#[derive(Clone)]
pub(crate) struct RenewalHandle {
    machine: Arc<Mutex<RenewalMachine>>,
    brain: Arc<dyn RenewalBrain>,
    diagnostics: Arc<Mutex<Box<dyn RenewalDiagnostics>>>,
    outbound: mpsc::Sender<Outbound>,
}

impl RenewalHandle {
    pub(crate) fn production(journal: PathBuf, outbound: mpsc::Sender<Outbound>) -> Self {
        Self::new(
            Arc::new(BrainAdapter::new(journal)),
            Box::new(StderrRenewalDiagnostics::new(io::stderr())),
            outbound,
        )
    }

    pub(crate) fn new(
        brain: Arc<dyn RenewalBrain>,
        diagnostics: Box<dyn RenewalDiagnostics>,
        outbound: mpsc::Sender<Outbound>,
    ) -> Self {
        Self {
            machine: Arc::new(Mutex::new(RenewalMachine::default())),
            brain,
            diagnostics: Arc::new(Mutex::new(diagnostics)),
            outbound,
        }
    }

    pub(crate) fn startup_refresh_needed(&self, now: DateTime<Utc>) -> bool {
        match self.brain.inspect(now) {
            Err(_) => true,
            Ok(inspection) => {
                let projection = inspection.projection;
                projection.active_lane.as_deref() != Some("spp")
                    && !matches!(projection.aggregate_state.as_str(), "checking" | "ready")
                    && !projection.runtime_transition_in_progress
            }
        }
    }

    pub(crate) fn startup_refresh(&self) {
        let _ = self.outbound.send(Outbound {
            tract: "supervisor",
            event: "request".into(),
            fields: Map::from_iter([("cmd".into(), json!(["journal", "brain", "refresh"]))]),
        });
    }

    fn flush(&self) {
        let events = self
            .machine
            .lock()
            .expect("renewal state lock poisoned")
            .take_events();
        let mut diagnostics = self
            .diagnostics
            .lock()
            .expect("renewal diagnostics lock poisoned");
        for event in events {
            diagnostics.emit(&format_renewal_event(event.state, &event.fields));
        }
    }

    pub(crate) fn step(&self, now: DateTime<Utc>) -> Duration {
        let inspection = match self.brain.inspect(now) {
            Ok(inspection) => inspection,
            Err(error) => return self.fail_step(now, &error.to_string()),
        };
        let disabled = inspection.projection.active_lane.as_deref() != Some("spp");
        {
            let mut machine = self.machine.lock().expect("renewal state lock poisoned");
            if let Some(delay) = machine.disabled(disabled) {
                drop(machine);
                self.flush();
                return delay;
            }
            if let Some(delay) = machine.state_delay(now) {
                drop(machine);
                self.flush();
                return delay;
            }
        }
        let inspection = match self.brain.inspect(now) {
            Ok(inspection) => inspection,
            Err(error) => return self.fail_step(now, &error.to_string()),
        };
        let fingerprint = match self.brain.active_fingerprint() {
            Ok(fingerprint) => fingerprint,
            Err(error) => return self.fail_step(now, &error.to_string()),
        };
        let outcome = self
            .machine
            .lock()
            .expect("renewal state lock poisoned")
            .plan(now, &inspection, fingerprint);
        let delay = match outcome {
            PlanOutcome::Disabled => Duration::from_secs(SPP_RENEWAL_DISABLED_IDLE_S),
            PlanOutcome::Checking => Duration::from_secs(5),
            PlanOutcome::Wait(delay) => delay,
            PlanOutcome::Start {
                kind,
                observed_before,
                expires_before,
            } => self.enqueue_attempt(now, kind, observed_before, expires_before),
        };
        self.flush();
        delay
    }

    fn enqueue_attempt(
        &self,
        now: DateTime<Utc>,
        kind: AttemptKind,
        observed_before: Option<DateTime<Utc>>,
        expires_before: Option<DateTime<Utc>>,
    ) -> Duration {
        let reference = format!("spp-renewal-{}", Uuid::new_v4().simple());
        let command = match &kind {
            AttemptKind::Renew { fingerprint } => json!([
                "journal",
                "brain",
                "renew-prerequisites",
                "--json",
                "--expected-fingerprint",
                fingerprint
            ]),
            AttemptKind::RefreshExpectedFingerprint { fingerprint } => json!([
                "journal",
                "brain",
                "refresh",
                "--json",
                "--expected-fingerprint",
                fingerprint,
                "--expected-active-fingerprint"
            ]),
            AttemptKind::RefreshExpectAbsent => json!([
                "journal",
                "brain",
                "refresh",
                "--json",
                "--expect-active-fingerprint-absent"
            ]),
        };
        let sent = self.outbound.send(Outbound {
            tract: "supervisor",
            event: "request".into(),
            fields: Map::from_iter([
                ("cmd".into(), command),
                ("ref".into(), Value::String(reference.clone())),
                ("scheduler_name".into(), Value::String("spp-renewal".into())),
            ]),
        });
        if sent.is_err() {
            return self.fail_step(now, "SendError");
        }
        self.machine
            .lock()
            .expect("renewal state lock poisoned")
            .mark_pending(
                now,
                Attempt {
                    kind,
                    reference,
                    observed_before,
                    expires_before,
                },
            );
        Duration::from_secs(SPP_RENEWAL_ACK_TIMEOUT_S)
    }

    fn fail_step(&self, now: DateTime<Utc>, reason: &str) -> Duration {
        self.machine
            .lock()
            .expect("renewal state lock poisoned")
            .step_error(now, reason);
        self.flush();
        Duration::from_secs(SPP_RENEWAL_RETRY_DELAYS_S[0])
    }

    pub(crate) fn handle_supervisor(
        &self,
        now: DateTime<Utc>,
        event: &str,
        fields: &Map<String, Value>,
    ) {
        let reference = fields.get("ref").and_then(Value::as_str);
        let verification = {
            let mut machine = self.machine.lock().expect("renewal state lock poisoned");
            match event {
                "started"
                    if reference
                        == machine
                            .pending
                            .as_ref()
                            .map(|attempt| attempt.reference.as_str()) =>
                {
                    let attempt = machine.pending.take().expect("checked pending");
                    machine.ack_deadline = None;
                    machine.running_deadline = Some(
                        now + chrono::Duration::from_std(attempt.kind.observation_bound())
                            .expect("duration"),
                    );
                    machine.event(
                        "in_flight",
                        vec![
                            RenewalField::text("action", attempt.kind.action()),
                            RenewalField::text("ref", attempt.reference.clone()),
                        ],
                    );
                    machine.running = Some(attempt);
                    None
                }
                "skipped"
                    if reference
                        == machine
                            .pending
                            .as_ref()
                            .map(|attempt| attempt.reference.as_str()) =>
                {
                    let pending = machine.pending.take().expect("checked pending");
                    machine.ack_deadline = None;
                    let active = fields
                        .get("active_ref")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let reason = fields
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("skipped");
                    machine.event(
                        "in_flight",
                        vec![
                            RenewalField::text("active_ref", active.clone().unwrap_or_default()),
                            RenewalField::text("reason", reason),
                            RenewalField::text("ref", pending.reference),
                        ],
                    );
                    machine.successor_after_ref = active;
                    machine.successor_deadline = machine.successor_after_ref.as_ref().map(|_| {
                        now + chrono::Duration::seconds(SPP_REFRESH_OBSERVATION_BOUND_S as i64)
                    });
                    if machine.successor_after_ref.is_none() {
                        machine.schedule_retry(now);
                    }
                    None
                }
                "stopped"
                    if reference
                        == machine
                            .running
                            .as_ref()
                            .map(|attempt| attempt.reference.as_str()) =>
                {
                    let attempt = machine.running.take().expect("checked running");
                    machine.running_deadline = None;
                    Some((attempt, exit_code(fields)))
                }
                "stopped" if reference == machine.successor_after_ref.as_deref() => {
                    machine.clear_successor();
                    None
                }
                _ => None,
            }
        };
        if let Some((attempt, exit)) = verification {
            let result = match (self.brain.active_fingerprint(), self.brain.inspect(now)) {
                (Ok(fingerprint), Ok(inspection)) => {
                    RenewalMachine::verification_result(&attempt, exit, &inspection, fingerprint)
                }
                _ => VerificationResult::Rejected,
            };
            self.machine
                .lock()
                .expect("renewal state lock poisoned")
                .apply_verification(now, attempt, result, exit);
        }
        self.flush();
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> MachineSnapshot {
        self.machine
            .lock()
            .expect("renewal state lock poisoned")
            .snapshot()
    }
}

pub(crate) struct RenewalWorkerStart {
    handle: RenewalHandle,
    now: Now,
    wait: Wait,
    stop: mpsc::Sender<()>,
}

pub(crate) struct RenewalWorker {
    stop: mpsc::Sender<()>,
    join: Option<thread::JoinHandle<()>>,
}

impl RenewalWorkerStart {
    pub(crate) fn production(handle: RenewalHandle) -> Self {
        let (stop, receiver) = mpsc::channel();
        let receiver = Arc::new(Mutex::new(receiver));
        Self {
            handle,
            now: Arc::new(Utc::now),
            wait: Arc::new(move |duration| {
                receiver
                    .lock()
                    .expect("renewal stop lock poisoned")
                    .recv_timeout(duration)
                    .is_ok()
            }),
            stop,
        }
    }

    #[cfg(test)]
    fn test(handle: RenewalHandle, now: Now, wait: Wait) -> Self {
        let (stop, _) = mpsc::channel();
        Self {
            handle,
            now,
            wait,
            stop,
        }
    }

    pub(crate) fn spawn(self) -> RenewalWorker {
        let handle = self.handle;
        let now = self.now;
        let wait = self.wait;
        let join = thread::spawn(move || {
            loop {
                let delay = handle.step(now());
                if wait(delay.min(Duration::from_secs(SPP_RENEWAL_MAX_WAIT_S))) {
                    break;
                }
            }
        });
        RenewalWorker {
            stop: self.stop,
            join: Some(join),
        }
    }
}

impl RenewalWorker {
    fn stop(&mut self) {
        let _ = self.stop.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub(crate) struct RenewalService {
    start: Option<RenewalWorkerStart>,
    worker: Option<RenewalWorker>,
}

impl RenewalService {
    pub(crate) fn new(start: RenewalWorkerStart) -> Self {
        Self {
            start: Some(start),
            worker: None,
        }
    }

    pub(crate) fn start_worker_once(&mut self) -> bool {
        let Some(start) = self.start.take() else {
            return false;
        };
        self.worker = Some(start.spawn());
        true
    }

    pub(crate) fn stop(&mut self) {
        if let Some(worker) = &mut self.worker {
            worker.stop();
        }
    }
}

impl Drop for RenewalService {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use solstone_core_brain::BrainProjection;
    use std::fs;

    use super::*;

    #[derive(Clone)]
    struct FakeBrain {
        inspection: Arc<Mutex<Result<BrainInspection, RenewalBrainError>>>,
        fingerprint: Arc<Mutex<Result<Option<String>, RenewalBrainError>>>,
    }

    impl RenewalBrain for FakeBrain {
        fn inspect(&self, _: DateTime<Utc>) -> Result<BrainInspection, RenewalBrainError> {
            self.inspection.lock().unwrap().clone()
        }
        fn active_fingerprint(&self) -> Result<Option<String>, RenewalBrainError> {
            self.fingerprint.lock().unwrap().clone()
        }
    }

    struct SharedWriter(Arc<Mutex<Vec<u8>>>);
    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap()
    }
    fn fingerprint() -> String {
        "a".repeat(64)
    }
    fn inspection(
        lane: &str,
        aggregate: &str,
        observed: DateTime<Utc>,
        expires: DateTime<Utc>,
    ) -> BrainInspection {
        BrainInspection {
            status: solstone_core_brain::InspectionStatus::Ok,
            projection: BrainProjection {
                aggregate_state: aggregate.into(),
                reason_code: None,
                active_lane: Some(lane.into()),
                active_provider: Some("local".into()),
                active_model: Some("served-model".into()),
                fingerprint_sha256: Some(fingerprint()),
                runtime_transition_in_progress: false,
            },
            error: None,
            record: Some(
                json!({"active_lane":lane,"fingerprint_sha256":fingerprint(),"evidence":{"lane_prerequisites":{"status":"ok","observed_at":observed.to_rfc3339(),"expires_at":expires.to_rfc3339()}}}),
            ),
        }
    }
    fn fake(inspection: BrainInspection, value: Option<String>) -> FakeBrain {
        FakeBrain {
            inspection: Arc::new(Mutex::new(Ok(inspection))),
            fingerprint: Arc::new(Mutex::new(Ok(value))),
        }
    }

    fn valid_journal() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        write_valid_test_journal(directory.path());
        directory
    }
    fn handle(brain: FakeBrain) -> (RenewalHandle, mpsc::Receiver<Outbound>, Arc<Mutex<Vec<u8>>>) {
        let (tx, rx) = mpsc::channel();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        (
            RenewalHandle::new(
                Arc::new(brain),
                Box::new(StderrRenewalDiagnostics::new(SharedWriter(bytes.clone()))),
                tx,
            ),
            rx,
            bytes,
        )
    }
    fn due() -> BrainInspection {
        inspection(
            "spp",
            "ready",
            now() - chrono::Duration::minutes(9),
            now() + chrono::Duration::seconds(30),
        )
    }
    fn advance(brain: &FakeBrain) {
        *brain.inspection.lock().unwrap() = Ok(inspection(
            "spp",
            "ready",
            now() + chrono::Duration::seconds(1),
            now() + chrono::Duration::minutes(10),
        ));
    }

    #[test]
    fn planner_returns_disabled_for_non_spp_lane() {
        assert_eq!(
            RenewalMachine::default().plan(
                now(),
                &inspection("none", "unknown", now(), now()),
                Some(fingerprint())
            ),
            PlanOutcome::Disabled
        );
    }
    #[test]
    fn planner_returns_checking_for_checking_projection() {
        assert_eq!(
            RenewalMachine::default().plan(
                now(),
                &inspection("spp", "checking", now(), now()),
                Some(fingerprint())
            ),
            PlanOutcome::Checking
        );
    }
    #[test]
    fn planner_waits_until_proactive_renewal_margin() {
        assert!(matches!(
            RenewalMachine::default().plan(
                now(),
                &inspection("spp", "ready", now(), now() + chrono::Duration::minutes(10)),
                Some(fingerprint())
            ),
            PlanOutcome::Wait(_)
        ));
    }
    #[test]
    fn planner_returns_renew_for_due_valid_prerequisites() {
        assert!(matches!(
            RenewalMachine::default().plan(now(), &due(), Some(fingerprint())),
            PlanOutcome::Start {
                kind: AttemptKind::Renew { .. },
                ..
            }
        ));
    }
    #[test]
    fn planner_returns_expect_absent_refresh_without_active_fingerprint() {
        assert!(matches!(
            RenewalMachine::default().plan(now(), &due(), None),
            PlanOutcome::Start {
                kind: AttemptKind::RefreshExpectAbsent,
                ..
            }
        ));
    }
    #[test]
    fn planner_returns_expected_fingerprint_refresh_for_expired_prerequisites() {
        assert!(matches!(
            RenewalMachine::default().plan(
                now(),
                &inspection(
                    "spp",
                    "ready",
                    now() - chrono::Duration::minutes(2),
                    now() - chrono::Duration::seconds(1)
                ),
                Some(fingerprint())
            ),
            PlanOutcome::Start {
                kind: AttemptKind::RefreshExpectedFingerprint { .. },
                ..
            }
        ));
    }
    #[test]
    fn proactive_margin_stays_below_half_tpm_heartbeat_interval() {
        assert!(
            SPP_RENEWAL_PROACTIVE_MARGIN_S
                < solstone_core_spp_ratls::TPM_HEARTBEAT_INTERVAL.as_secs() / 2
        );
    }

    fn completed(kind: AttemptKind, exit: i32, changed: bool) -> VerificationResult {
        let brain = fake(due(), Some(fingerprint()));
        let (handle, rx, _) = handle(brain.clone());
        let _ = handle.step(now());
        let request = rx.recv().unwrap();
        let reference = request.fields["ref"].as_str().unwrap().to_owned();
        {
            let mut machine = handle.machine.lock().unwrap();
            machine.pending = Some(Attempt {
                kind,
                reference: reference.clone(),
                observed_before: Some(now() - chrono::Duration::minutes(9)),
                expires_before: Some(now() + chrono::Duration::seconds(30)),
            });
            machine.ack_deadline = Some(now() + chrono::Duration::seconds(15));
        }
        handle.handle_supervisor(
            now(),
            "started",
            &Map::from_iter([("ref".into(), Value::String(reference.clone()))]),
        );
        if changed {
            advance(&brain);
        }
        handle.handle_supervisor(
            now(),
            "stopped",
            &Map::from_iter([
                ("ref".into(), Value::String(reference)),
                ("exit_code".into(), Value::from(exit)),
            ]),
        );
        if handle.snapshot().retry_index == 0 {
            VerificationResult::Verified
        } else {
            VerificationResult::Rejected
        }
    }
    #[test]
    fn renew_verifies_advanced_evidence_despite_nonzero_exit() {
        assert_eq!(
            completed(
                AttemptKind::Renew {
                    fingerprint: fingerprint()
                },
                9,
                true
            ),
            VerificationResult::Verified
        );
    }
    #[test]
    fn renew_rejects_unadvanced_evidence_despite_zero_exit() {
        assert_eq!(
            completed(
                AttemptKind::Renew {
                    fingerprint: fingerprint()
                },
                0,
                false
            ),
            VerificationResult::Rejected
        );
    }
    #[test]
    fn refresh_rejects_advanced_evidence_after_nonzero_exit() {
        assert_eq!(
            completed(
                AttemptKind::RefreshExpectedFingerprint {
                    fingerprint: fingerprint()
                },
                1,
                true
            ),
            VerificationResult::Rejected
        );
    }
    #[test]
    fn refresh_verifies_advanced_evidence_after_zero_exit() {
        assert_eq!(
            completed(
                AttemptKind::RefreshExpectedFingerprint {
                    fingerprint: fingerprint()
                },
                0,
                true
            ),
            VerificationResult::Verified
        );
    }
    #[test]
    fn verification_rejects_evidence_with_unchanged_observed_and_expiry_times() {
        assert_eq!(
            completed(
                AttemptKind::Renew {
                    fingerprint: fingerprint()
                },
                0,
                false
            ),
            VerificationResult::Rejected
        );
    }
    #[test]
    fn retry_backoff_saturates_at_sixty_and_verified_attempt_resets_it() {
        let mut machine = RenewalMachine::default();
        for expected in [5, 10, 20, 40, 60, 60] {
            machine.schedule_retry(now());
            assert_eq!(
                seconds_until(machine.retry_after, now(), 0),
                Duration::from_secs(expected)
            );
        }
        machine.retry_index = 4;
        machine.apply_verification(
            now(),
            Attempt {
                kind: AttemptKind::Renew {
                    fingerprint: fingerprint(),
                },
                reference: "r".into(),
                observed_before: None,
                expires_before: None,
            },
            VerificationResult::Verified,
            0,
        );
        assert_eq!(machine.retry_index, 0);
    }
    #[test]
    fn pending_attempt_fails_at_fifteen_second_ack_deadline() {
        let mut machine = RenewalMachine::default();
        machine.mark_pending(
            now(),
            Attempt {
                kind: AttemptKind::Renew {
                    fingerprint: fingerprint(),
                },
                reference: "r".into(),
                observed_before: None,
                expires_before: None,
            },
        );
        let _ = machine.state_delay(now() + chrono::Duration::seconds(15));
        assert_eq!(machine.retry_index, 1);
    }
    #[test]
    fn running_refresh_stales_at_three_hundred_seconds() {
        let mut machine = RenewalMachine {
            running: Some(Attempt {
                kind: AttemptKind::RefreshExpectAbsent,
                reference: "r".into(),
                observed_before: None,
                expires_before: None,
            }),
            running_deadline: Some(now() + chrono::Duration::seconds(300)),
            ..RenewalMachine::default()
        };
        let _ = machine.state_delay(now() + chrono::Duration::seconds(300));
        assert_eq!(machine.retry_index, 1);
    }
    #[test]
    fn running_renew_stales_at_one_hundred_twenty_seconds() {
        let mut machine = RenewalMachine {
            running: Some(Attempt {
                kind: AttemptKind::Renew {
                    fingerprint: fingerprint(),
                },
                reference: "r".into(),
                observed_before: None,
                expires_before: None,
            }),
            running_deadline: Some(now() + chrono::Duration::seconds(120)),
            ..RenewalMachine::default()
        };
        let _ = machine.state_delay(now() + chrono::Duration::seconds(120));
        assert_eq!(machine.retry_index, 1);
    }
    #[test]
    fn running_attempt_without_stop_schedules_retry_after_deadline() {
        let mut machine = RenewalMachine {
            running: Some(Attempt {
                kind: AttemptKind::Renew {
                    fingerprint: fingerprint(),
                },
                reference: "r".into(),
                observed_before: None,
                expires_before: None,
            }),
            running_deadline: Some(now()),
            ..RenewalMachine::default()
        };
        let _ = machine.state_delay(now());
        assert!(machine.retry_after.is_some());
    }
    #[test]
    fn skipped_attempt_with_active_ref_waits_for_successor_stop() {
        let brain = fake(due(), Some(fingerprint()));
        let (handle, rx, _) = handle(brain);
        let _ = handle.step(now());
        let reference = rx.recv().unwrap().fields["ref"]
            .as_str()
            .unwrap()
            .to_owned();
        handle.handle_supervisor(
            now(),
            "skipped",
            &Map::from_iter([
                ("ref".into(), Value::String(reference)),
                ("active_ref".into(), Value::String("a".into())),
            ]),
        );
        assert_eq!(handle.snapshot().successor_ref.as_deref(), Some("a"));
        assert_eq!(handle.snapshot().retry_index, 0);
    }
    #[test]
    fn skipped_attempt_without_active_ref_schedules_retry() {
        let brain = fake(due(), Some(fingerprint()));
        let (handle, rx, _) = handle(brain);
        let _ = handle.step(now());
        let reference = rx.recv().unwrap().fields["ref"]
            .as_str()
            .unwrap()
            .to_owned();
        handle.handle_supervisor(
            now(),
            "skipped",
            &Map::from_iter([("ref".into(), Value::String(reference))]),
        );
        assert_eq!(handle.snapshot().retry_index, 1);
    }
    #[test]
    fn consecutive_disabled_steps_emit_one_disabled_line_per_transition() {
        let brain = fake(
            inspection("none", "unknown", now(), now()),
            Some(fingerprint()),
        );
        let (handle, _, bytes) = handle(brain);
        let _ = handle.step(now());
        let _ = handle.step(now());
        assert_eq!(
            String::from_utf8(bytes.lock().unwrap().clone())
                .unwrap()
                .lines()
                .count(),
            1
        );
    }
    #[test]
    fn startup_refresh_predicate_fires_four_cases_and_suppresses_three_cases() {
        let directory = valid_journal();
        let adapter = BrainAdapter::new(directory.path().to_path_buf());
        assert!(adapter.inspect(now()).unwrap().record.is_some());
        assert!(adapter.active_fingerprint().unwrap().is_some());
        for aggregate in ["unknown", "blocked", "unhealthy"] {
            let (controller, _, _) = handle(fake(
                inspection("none", aggregate, now(), now()),
                Some(fingerprint()),
            ));
            assert!(controller.startup_refresh_needed(now()));
        }
        let (controller, _, _) = handle(FakeBrain {
            inspection: Arc::new(Mutex::new(Err(error_brain()))),
            fingerprint: Arc::new(Mutex::new(Ok(None))),
        });
        assert!(controller.startup_refresh_needed(now()));
        let (controller, _, _) = handle(fake(
            inspection("spp", "unknown", now(), now()),
            Some(fingerprint()),
        ));
        assert!(!controller.startup_refresh_needed(now()));
        let (controller, _, _) = handle(fake(
            inspection("none", "checking", now(), now()),
            Some(fingerprint()),
        ));
        assert!(!controller.startup_refresh_needed(now()));
        let (controller, _, _) = handle(fake(
            inspection("none", "ready", now(), now()),
            Some(fingerprint()),
        ));
        assert!(!controller.startup_refresh_needed(now()));
    }
    #[test]
    fn shipped_stderr_sink_renders_and_writes_the_exact_renewal_line() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = StderrRenewalDiagnostics::new(SharedWriter(bytes.clone()));
        let mut boxed: Box<dyn RenewalDiagnostics> = Box::new(sink);
        boxed.emit(&format_renewal_event(
            "retrying",
            &[
                RenewalField::seconds("delay_s", 5.0),
                RenewalField {
                    key: "skip",
                    value: None,
                },
            ],
        ));
        assert_eq!(
            &*bytes.lock().unwrap(),
            b"event=spp_renewal_retrying delay_s=5.0\n"
        );
    }
    fn error_brain() -> RenewalBrainError {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("config")).unwrap();
        fs::write(directory.path().join("config/journal.json"), b"{").unwrap();
        match BrainAdapter::new(directory.path().to_path_buf()).inspect(now()) {
            Err(error) => error,
            Ok(_) => panic!("corrupt config must fail"),
        }
    }
    #[test]
    fn brain_state_read_failure_does_not_stop_renewal_worker() {
        let brain = FakeBrain {
            inspection: Arc::new(Mutex::new(Err(error_brain()))),
            fingerprint: Arc::new(Mutex::new(Ok(None))),
        };
        let (handle, _, bytes) = handle(brain);
        assert_eq!(handle.step(now()), Duration::from_secs(5));
        assert_eq!(handle.snapshot().retry_index, 1);
        assert!(
            String::from_utf8(bytes.lock().unwrap().clone())
                .unwrap()
                .contains("failed")
        );
    }
    #[test]
    fn fingerprint_read_failure_does_not_stop_renewal_worker() {
        let brain = FakeBrain {
            inspection: Arc::new(Mutex::new(Ok(due()))),
            fingerprint: Arc::new(Mutex::new(Err(error_brain()))),
        };
        let (handle, _, _) = handle(brain);
        assert_eq!(handle.step(now()), Duration::from_secs(5));
        assert_eq!(handle.snapshot().retry_index, 1);
    }
    #[test]
    fn outbound_send_failure_does_not_stop_renewal_worker() {
        let brain = fake(due(), Some(fingerprint()));
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let handle = RenewalHandle::new(
            Arc::new(brain),
            Box::new(StderrRenewalDiagnostics::new(SharedWriter(bytes))),
            tx,
        );
        assert_eq!(handle.step(now()), Duration::from_secs(5));
        assert_eq!(handle.snapshot().retry_index, 1);
    }
    #[test]
    fn worker_caps_a_multi_hour_planned_wait_at_sixty_seconds() {
        let waits = Arc::new(Mutex::new(Vec::new()));
        let wait: Wait = {
            let waits = waits.clone();
            Arc::new(move |delay| {
                waits.lock().unwrap().push(delay);
                true
            })
        };
        let brain = fake(
            inspection("spp", "ready", now(), now() + chrono::Duration::hours(5)),
            Some(fingerprint()),
        );
        let (handle, _, _) = handle(brain);
        let start = RenewalWorkerStart::test(handle, Arc::new(now), wait);
        let mut worker = start.spawn();
        worker.stop();
        assert_eq!(
            waits.lock().unwrap().first(),
            Some(&Duration::from_secs(60))
        );
    }
    fn mismatch(event: &str, tract: &str) {
        let brain = fake(due(), Some(fingerprint()));
        let (handle, rx, _) = handle(brain);
        let _ = handle.step(now());
        let reference = rx.recv().unwrap().fields["ref"]
            .as_str()
            .unwrap()
            .to_owned();
        let before = handle.snapshot();
        if tract == "supervisor" {
            handle.handle_supervisor(
                now(),
                event,
                &Map::from_iter([("ref".into(), Value::String(reference + "x"))]),
            );
        }
        assert_eq!(handle.snapshot(), before);
    }
    #[test]
    fn wrong_tract_envelope_leaves_renewal_state_unchanged() {
        mismatch("started", "cortex");
    }
    #[test]
    fn wrong_event_envelope_leaves_renewal_state_unchanged() {
        mismatch("other", "supervisor");
    }
    #[test]
    fn wrong_ref_envelope_leaves_renewal_state_unchanged() {
        mismatch("started", "supervisor");
    }
    #[test]
    fn malformed_exit_code_normalizes_to_negative_one_and_renew_can_verify() {
        assert_eq!(
            exit_code(&Map::from_iter([(
                "exit_code".into(),
                Value::String("bad".into())
            )])),
            -1
        );
        assert_eq!(
            completed(
                AttemptKind::Renew {
                    fingerprint: fingerprint()
                },
                -1,
                true
            ),
            VerificationResult::Verified
        );
    }
    #[test]
    fn malformed_exit_code_normalizes_to_negative_one_and_refresh_fails() {
        assert_eq!(
            completed(
                AttemptKind::RefreshExpectedFingerprint {
                    fingerprint: fingerprint()
                },
                -1,
                true
            ),
            VerificationResult::Rejected
        );
    }
}
