// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use solstone_core_callosum::CallosumConnectionPhase;

use crate::{ProcessIdentity, TopMalformed};

/// Native Brain inspection freshness, excluded from the retained fixture
/// projection so a failed inspection cannot masquerade as fresh data.
#[derive(Clone, Debug, PartialEq)]
pub enum BrainHealthState {
    Checking,
    Available {
        observed_at_monotonic: f64,
    },
    Unavailable {
        message: String,
        observed_at_monotonic: f64,
    },
}

impl Default for BrainHealthState {
    fn default() -> Self {
        Self::Unavailable {
            message: String::new(),
            observed_at_monotonic: 0.0,
        }
    }
}

impl BrainHealthState {
    #[must_use]
    pub fn observed_at_monotonic(&self) -> f64 {
        match self {
            Self::Checking => 0.0,
            Self::Available {
                observed_at_monotonic,
            }
            | Self::Unavailable {
                observed_at_monotonic,
                ..
            } => *observed_at_monotonic,
        }
    }
}

/// Continuity metadata is native-only and deliberately excluded from the
/// retained Python fixture projection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum DomainRecovery {
    #[default]
    Complete,
    Incomplete {
        post_gap_evidence: bool,
    },
}

impl DomainRecovery {
    #[must_use]
    pub fn is_incomplete(&self) -> bool {
        matches!(self, Self::Incomplete { .. })
    }

    pub fn incomplete(&mut self) {
        *self = Self::Incomplete {
            post_gap_evidence: false,
        };
    }

    pub fn record_evidence(&mut self) {
        if let Self::Incomplete { post_gap_evidence } = self {
            *post_gap_evidence = true;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainContinuity {
    pub generation: u64,
    pub epoch: u64,
    pub connection: CallosumConnectionPhase,
    pub supervisor: DomainRecovery,
    pub tasks: DomainRecovery,
    pub observe: DomainRecovery,
    pub think: DomainRecovery,
    pub rejected_receive_events: u64,
}

impl Default for DomainContinuity {
    fn default() -> Self {
        Self {
            generation: 0,
            epoch: 0,
            connection: CallosumConnectionPhase::Connecting { attempt: 1 },
            supervisor: DomainRecovery::Incomplete {
                post_gap_evidence: false,
            },
            tasks: DomainRecovery::Incomplete {
                post_gap_evidence: false,
            },
            observe: DomainRecovery::Incomplete {
                post_gap_evidence: false,
            },
            think: DomainRecovery::Incomplete {
                post_gap_evidence: false,
            },
            rejected_receive_events: 0,
        }
    }
}

/// Typed manager state with JSON-valued compatibility leaves where Python
/// deliberately preserves arbitrary payload values.
#[derive(Clone, Debug, PartialEq)]
pub struct TopState {
    pub services: Vec<Value>,
    pub crashed: Vec<Value>,
    pub selected: usize,
    pub service_status: BTreeMap<String, (String, f64)>,
    pub last_log_lines: BTreeMap<String, Value>,
    pub cpu_cache: BTreeMap<u32, f64>,
    /// Native-only RSS cache; intentionally omitted from the Python fixture projection.
    pub memory_cache: BTreeMap<u32, u64>,
    pub cpu_pids: BTreeSet<u32>,
    /// Native birth identities back the retained PID-keyed process projection.
    pub process_identities: BTreeMap<u32, ProcessIdentity>,
    /// Native monotonic timestamps for task runtime rendering.
    pub task_started_at: BTreeMap<String, f64>,
    /// Native monotonic expiry timestamps for finished-task ghosts.
    pub finished_task_finished_at: BTreeMap<String, f64>,
    /// Native wall timestamps for log age rendering.
    pub last_log_at: BTreeMap<String, f64>,
    pub running_tasks: BTreeMap<String, Value>,
    pub finished_tasks: BTreeMap<String, Value>,
    pub command_queues: BTreeMap<String, Value>,
    pub observe_status: BTreeMap<String, Value>,
    pub observe_last_ts: f64,
    pub recent_segments: Vec<Value>,
    pub displayed_mode: String,
    pub last_active_ts: f64,
    pub think_status: BTreeMap<String, Value>,
    pub think_last_completed: BTreeMap<String, Value>,
    pub think_running: bool,
    pub brain_health: Option<Value>,
    pub brain_health_ts: f64,
    pub brain_health_state: BrainHealthState,
    pub continuity: DomainContinuity,
    /// Native-only malformed-event evidence, excluded from the retained fixture projection.
    pub malformed_events: u64,
    /// The latest typed malformed route classification, also native-only.
    pub last_malformed: Option<TopMalformed>,
}

impl Default for TopState {
    fn default() -> Self {
        Self {
            services: Vec::new(),
            crashed: Vec::new(),
            selected: 0,
            service_status: BTreeMap::new(),
            last_log_lines: BTreeMap::new(),
            cpu_cache: BTreeMap::new(),
            memory_cache: BTreeMap::new(),
            cpu_pids: BTreeSet::new(),
            process_identities: BTreeMap::new(),
            task_started_at: BTreeMap::new(),
            finished_task_finished_at: BTreeMap::new(),
            last_log_at: BTreeMap::new(),
            running_tasks: BTreeMap::new(),
            finished_tasks: BTreeMap::new(),
            command_queues: BTreeMap::new(),
            observe_status: BTreeMap::new(),
            observe_last_ts: 0.0,
            recent_segments: Vec::new(),
            displayed_mode: "idle".to_owned(),
            last_active_ts: 0.0,
            think_status: BTreeMap::new(),
            think_last_completed: BTreeMap::new(),
            think_running: false,
            brain_health: None,
            brain_health_ts: 0.0,
            brain_health_state: BrainHealthState::default(),
            continuity: DomainContinuity::default(),
            malformed_events: 0,
            last_malformed: None,
        }
    }
}

impl TopState {
    /// Project exactly the twenty retained `ServiceManager` state keys.
    #[must_use]
    pub fn fixture_value(&self) -> Value {
        let service_status = self
            .service_status
            .iter()
            .map(|(name, (status, at))| (name.clone(), json!([status, at])))
            .collect::<Map<_, _>>();
        let cpu_cache = self
            .cpu_cache
            .iter()
            .map(|(pid, value)| (pid.to_string(), json!(value)))
            .collect::<Map<_, _>>();
        json!({
            "brain_health": self.brain_health,
            "brain_health_ts": self.brain_health_ts,
            "command_queues": self.command_queues,
            "cpu_cache": cpu_cache,
            "cpu_pids": self.cpu_pids,
            "crashed": self.crashed,
            "displayed_mode": self.displayed_mode,
            "finished_tasks": self.finished_tasks,
            "last_active_ts": self.last_active_ts,
            "last_log_lines": self.last_log_lines,
            "observe_last_ts": self.observe_last_ts,
            "observe_status": self.observe_status,
            "recent_segments": self.recent_segments,
            "running_tasks": self.running_tasks,
            "selected": self.selected,
            "service_status": service_status,
            "services": self.services,
            "think_last_completed": self.think_last_completed,
            "think_running": self.think_running,
            "think_status": self.think_status,
        })
    }

    /// Rehydrate a retained fixture state without interpreting native metadata.
    pub fn from_fixture_value(value: &Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "manager state must be an object".to_owned())?;
        Ok(Self {
            services: array(object, "services")?,
            crashed: array(object, "crashed")?,
            selected: object.get("selected").and_then(Value::as_u64).unwrap_or(0) as usize,
            service_status: object_map(object, "service_status")?
                .into_iter()
                .filter_map(|(name, value)| {
                    let values = value.as_array()?;
                    Some((
                        name,
                        (
                            values.first()?.as_str()?.to_owned(),
                            values.get(1)?.as_f64()?,
                        ),
                    ))
                })
                .collect(),
            last_log_lines: object_map(object, "last_log_lines")?,
            cpu_cache: object_map(object, "cpu_cache")?
                .into_iter()
                .filter_map(|(pid, value)| Some((pid.parse().ok()?, value.as_f64()?)))
                .collect(),
            cpu_pids: array(object, "cpu_pids")?
                .into_iter()
                .filter_map(|value| value.as_u64().map(|pid| pid as u32))
                .collect(),
            running_tasks: object_map(object, "running_tasks")?,
            finished_tasks: object_map(object, "finished_tasks")?,
            command_queues: object_map(object, "command_queues")?,
            observe_status: object_map(object, "observe_status")?,
            observe_last_ts: number(object, "observe_last_ts"),
            recent_segments: array(object, "recent_segments")?,
            displayed_mode: object
                .get("displayed_mode")
                .and_then(Value::as_str)
                .unwrap_or("idle")
                .to_owned(),
            last_active_ts: number(object, "last_active_ts"),
            finished_task_finished_at: object_map(object, "finished_tasks")?
                .iter()
                .filter_map(|(reference, task)| {
                    task.get("finished_at")
                        .and_then(Value::as_f64)
                        .map(|finished_at| (reference.clone(), finished_at))
                })
                .collect(),
            think_status: object_map(object, "think_status")?,
            think_last_completed: object_map(object, "think_last_completed")?,
            think_running: object
                .get("think_running")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            brain_health: object
                .get("brain_health")
                .filter(|value| !value.is_null())
                .cloned(),
            brain_health_ts: number(object, "brain_health_ts"),
            ..Self::default()
        })
    }
}

fn array(object: &Map<String, Value>, key: &str) -> Result<Vec<Value>, String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| format!("{key} must be an array"))
}

fn object_map(object: &Map<String, Value>, key: &str) -> Result<BTreeMap<String, Value>, String> {
    object
        .get(key)
        .and_then(Value::as_object)
        .map(|values| {
            values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .ok_or_else(|| format!("{key} must be an object"))
}

fn number(object: &Map<String, Value>, key: &str) -> f64 {
    object.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}
