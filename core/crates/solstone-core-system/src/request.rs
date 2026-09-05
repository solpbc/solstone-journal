// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cap::CapResolver;
use crate::error::WireRequestError;
use crate::partition::{Partition, partition_for};

/// A recognized task-service command retaining its exact original wire argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownTaskArgv {
    raw: Vec<String>,
}

impl KnownTaskArgv {
    fn new(raw: Vec<String>) -> Self {
        Self { raw }
    }

    pub fn as_wire(&self) -> &[String] {
        &self.raw
    }
}

/// Typed task command categories accepted by the ordinary bus path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskArgv {
    Think(KnownTaskArgv),
    Indexer(KnownTaskArgv),
    Importer(KnownTaskArgv),
    Brain(KnownTaskArgv),
    Maintenance(KnownTaskArgv),
    Heartbeat(KnownTaskArgv),
    FacetCandidates(KnownTaskArgv),
    /// A forward-compatible ordinary-bus command outside the current census.
    Unknown {
        raw: Vec<String>,
    },
}

impl TaskArgv {
    pub fn from_wire(raw: Vec<String>) -> Result<Self, WireRequestError> {
        if raw.is_empty() {
            return Err(WireRequestError::MissingCommand);
        }

        let known = matches!(
            raw.first().map(String::as_str),
            Some("solstone" | "journal")
        )
        .then(|| raw.get(1).map(String::as_str))
        .flatten();
        let task = KnownTaskArgv::new(raw.clone());
        match known {
            Some("think") => Ok(Self::Think(task)),
            Some("indexer") => Ok(Self::Indexer(task)),
            Some("importer") => Ok(Self::Importer(task)),
            Some("brain") => Ok(Self::Brain(task)),
            Some("maintenance") => Ok(Self::Maintenance(task)),
            Some("heartbeat") => Ok(Self::Heartbeat(task)),
            Some("facet-candidates") => Ok(Self::FacetCandidates(task)),
            _ => Ok(Self::Unknown { raw }),
        }
    }

    pub fn as_wire(&self) -> &[String] {
        match self {
            Self::Think(value)
            | Self::Indexer(value)
            | Self::Importer(value)
            | Self::Brain(value)
            | Self::Maintenance(value)
            | Self::Heartbeat(value)
            | Self::FacetCandidates(value) => value.as_wire(),
            Self::Unknown { raw } => raw,
        }
    }

    pub fn partition(&self) -> Partition {
        partition_for(self.as_wire())
    }
}

/// A non-empty argv admitted solely by the scheduler-facing construction path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledArgv {
    raw: Vec<String>,
}

impl ScheduledArgv {
    pub fn from_wire(raw: Vec<String>) -> Result<Self, WireRequestError> {
        if raw.is_empty() {
            return Err(WireRequestError::MissingCommand);
        }
        Ok(Self { raw })
    }

    pub fn as_wire(&self) -> &[String] {
        &self.raw
    }

    pub fn partition(&self) -> Partition {
        partition_for(&self.raw)
    }
}

/// The wire fields carried by a supervisor task request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireTaskRequest {
    pub cmd: Option<Vec<String>>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub day: Option<String>,
    pub scheduler_name: Option<String>,
    #[serde(default)]
    pub queue_if_active_cmd_differs: bool,
}

/// Internal provenance attached only to automatic whole-day catchup dispatches.
///
/// This deliberately has no wire representation. Bus decoders always leave it
/// absent, so an inbound callosum request cannot claim automatic-catchup state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyCatchupProvenance {
    pub day: String,
}

/// A decoded ordinary bus request. It cannot carry a schedule-only argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusTaskRequest {
    pub cmd: TaskArgv,
    pub reference: String,
    pub day: Option<String>,
    pub scheduler_name: Option<String>,
    pub queue_if_active_cmd_differs: bool,
    pub daily_catchup_provenance: Option<DailyCatchupProvenance>,
}

impl BusTaskRequest {
    /// Decode an ordinary bus message. Its return type intentionally cannot
    /// produce `ExecutionRequest::Scheduled`; only `ScheduledRequest::new`
    /// reaches that enum arm.
    pub fn decode(
        wire: WireTaskRequest,
        fallback_reference: impl Into<String>,
    ) -> Result<Self, WireRequestError> {
        let cmd = TaskArgv::from_wire(wire.cmd.ok_or(WireRequestError::MissingCommand)?)?;
        Ok(Self {
            cmd,
            reference: wire.reference.unwrap_or_else(|| fallback_reference.into()),
            day: wire.day,
            scheduler_name: wire.scheduler_name,
            queue_if_active_cmd_differs: wire.queue_if_active_cmd_differs,
            daily_catchup_provenance: None,
        })
    }
}

/// Scheduler-created work preserving a schedule's open argv grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledRequest {
    pub cmd: ScheduledArgv,
    /// Budget from the configuration that admitted this scheduled run.
    pub max_runtime: Option<Duration>,
    pub reference: String,
    pub day: Option<String>,
    pub scheduler_name: String,
}

impl ScheduledRequest {
    /// Construct scheduler-originated work with no day context.
    pub fn new(
        cmd: ScheduledArgv,
        reference: impl Into<String>,
        scheduler_name: impl Into<String>,
    ) -> Self {
        Self {
            cmd,
            reference: reference.into(),
            day: None,
            scheduler_name: scheduler_name.into(),
            max_runtime: None,
        }
    }
}

/// Work accepted by a future queue from either its bus or scheduler boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionRequest {
    Bus(BusTaskRequest),
    Scheduled(ScheduledRequest),
}

/// Active work observations required for refusal classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTaskSnapshot {
    pub reference: String,
    /// `None` is the active-ref/managed-process race: active command unreadable.
    pub cmd: Option<Vec<String>>,
    /// Unix seconds. `None` means runtime is conservatively treated as zero.
    pub started_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    Wedged,
    StillRunning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRefusal {
    pub reason: RefusalReason,
    pub reference: String,
    pub active_reference: String,
    pub cmd: Vec<String>,
    pub scheduler_name: Option<String>,
}

/// The complete task-admission result; silent Python cases are not refusals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestDisposition {
    Dispatch,
    QueueDespiteActive,
    Refused(TaskRefusal),
    IgnoredMissingCommand,
    IgnoredQueueUnavailable,
}

/// Classify a raw wire request before task queue submission.
pub fn classify_wire_request(
    wire: WireTaskRequest,
    fallback_reference: impl Into<String>,
    queue_available: bool,
    active: Option<ActiveTaskSnapshot>,
    caps: &impl CapResolver,
    now_seconds: u64,
) -> RequestDisposition {
    let request = match BusTaskRequest::decode(wire, fallback_reference) {
        Ok(request) => request,
        Err(WireRequestError::MissingCommand) => return RequestDisposition::IgnoredMissingCommand,
    };
    classify_request(&request, queue_available, active, caps, now_seconds)
}

/// Classify an already decoded ordinary bus request.
pub fn classify_request(
    request: &BusTaskRequest,
    queue_available: bool,
    active: Option<ActiveTaskSnapshot>,
    caps: &impl CapResolver,
    now_seconds: u64,
) -> RequestDisposition {
    if !queue_available {
        return RequestDisposition::IgnoredQueueUnavailable;
    }
    let Some(active) = active else {
        return RequestDisposition::Dispatch;
    };

    let cmd = request.cmd.as_wire();
    if request.queue_if_active_cmd_differs
        && active
            .cmd
            .as_ref()
            .is_some_and(|active_cmd| active_cmd != cmd)
    {
        return RequestDisposition::QueueDespiteActive;
    }

    let runtime = active
        .started_at
        .map(|started| now_seconds.saturating_sub(started))
        .unwrap_or(0);
    let cap = caps.cap_for(&request.cmd.partition());
    let reason = if Duration::from_secs(runtime) > cap.saturating_mul(2) {
        RefusalReason::Wedged
    } else {
        RefusalReason::StillRunning
    };
    RequestDisposition::Refused(TaskRefusal {
        reason,
        reference: request.reference.clone(),
        active_reference: active.reference,
        cmd: cmd.to_vec(),
        scheduler_name: request.scheduler_name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{BusTaskRequest, WireTaskRequest};

    #[test]
    fn wire_decode_cannot_supply_daily_catchup_provenance() {
        let wire: WireTaskRequest = serde_json::from_value(json!({
            "cmd": ["journal", "think", "--day", "20260101"],
            "daily_catchup_provenance": {
                "day": "20260101",
                "reference": "forged",
                "admitted_generation": 99,
                "fingerprint": "forged"
            }
        }))
        .expect("wire request");
        assert!(
            BusTaskRequest::decode(wire, "fallback")
                .expect("decoded request")
                .daily_catchup_provenance
                .is_none()
        );
    }
}
