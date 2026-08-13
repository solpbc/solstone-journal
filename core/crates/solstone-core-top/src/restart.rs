// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::atomic::{AtomicU64, Ordering};

use crate::TopState;

const NO_ACK_SECONDS: f64 = 5.0;
const RESTART_SECONDS: f64 = 10.0;
static NEXT_RESTART_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestartFailure {
    NoAck,
    RestartTimedOut,
    Discontinuity,
    StoppedBeforeStarted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestartPhase {
    Pending,
    Restarting,
    Started,
    Interrupted,
    Failed(RestartFailure),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RestartAttempt {
    pub restart_id: String,
    pub generation: u64,
    pub phase: RestartPhase,
    pub issued_at: f64,
    pub phase_at: f64,
    pub terminal_at: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TopRestartError {
    #[error("restart transport failed")]
    Transport,
}

/// Transport is deliberately separate from restart protocol state.
pub trait TopRestartTransport {
    fn emit_restart(&mut self, service: &str, restart_id: &str) -> Result<(), TopRestartError>;
    fn current_generation(&self) -> u64;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestartRequestOutcome {
    Emitted { restart_id: String },
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestartTransition {
    pub service: String,
    pub phase: RestartPhase,
}

/// Request exactly one restart for a supported currently listed service.
pub fn request_restart(
    state: &mut TopState,
    service: &str,
    now: f64,
    transport: &mut dyn TopRestartTransport,
) -> Result<RestartRequestOutcome, TopRestartError> {
    if !is_supported(service)
        || !state
            .services
            .iter()
            .any(|entry| entry.get("name").and_then(serde_json::Value::as_str) == Some(service))
    {
        return Ok(RestartRequestOutcome::Rejected);
    }
    if state.restart_attempts.get(service).is_some_and(|attempt| {
        matches!(
            attempt.phase,
            RestartPhase::Pending | RestartPhase::Restarting
        )
    }) {
        return Ok(RestartRequestOutcome::Rejected);
    }
    let restart_id = restart_id(
        std::process::id(),
        NEXT_RESTART_ID.fetch_add(1, Ordering::Relaxed),
    );
    transport.emit_restart(service, &restart_id)?;
    state.restart_attempts.insert(
        service.to_owned(),
        RestartAttempt {
            restart_id: restart_id.clone(),
            generation: transport.current_generation(),
            phase: RestartPhase::Pending,
            issued_at: now,
            phase_at: now,
            terminal_at: None,
        },
    );
    Ok(RestartRequestOutcome::Emitted { restart_id })
}

/// Advance timeout bounds and return newly terminal transitions.
pub fn advance_restart_attempts(state: &mut TopState, now: f64) -> Vec<RestartTransition> {
    let mut transitions = Vec::new();
    for (service, attempt) in &mut state.restart_attempts {
        let failure = match attempt.phase {
            RestartPhase::Pending if now - attempt.issued_at >= NO_ACK_SECONDS => {
                Some(RestartFailure::NoAck)
            }
            RestartPhase::Restarting if now - attempt.phase_at >= RESTART_SECONDS => {
                Some(RestartFailure::RestartTimedOut)
            }
            _ => None,
        };
        if let Some(failure) = failure {
            attempt.phase = RestartPhase::Failed(failure);
            attempt.phase_at = now;
            attempt.terminal_at = Some(now);
            transitions.push(RestartTransition {
                service: service.clone(),
                phase: attempt.phase.clone(),
            });
        }
    }
    transitions
}

/// Correlate an authoritative lifecycle acknowledgement only when both epoch
/// and restart id match the live attempt.
pub fn acknowledge_restart(
    state: &mut TopState,
    service: &str,
    restart_id: Option<&str>,
    generation: u64,
    event: &str,
    now: f64,
) -> Option<RestartTransition> {
    let attempt = state.restart_attempts.get_mut(service)?;
    if attempt.generation != generation || restart_id != Some(attempt.restart_id.as_str()) {
        return None;
    }
    let phase = match (&attempt.phase, event) {
        (RestartPhase::Pending, "restarting") => RestartPhase::Restarting,
        (RestartPhase::Pending | RestartPhase::Restarting, "started") => RestartPhase::Started,
        (RestartPhase::Pending, "stopped") => {
            RestartPhase::Failed(RestartFailure::StoppedBeforeStarted)
        }
        (RestartPhase::Restarting, "stopped") => RestartPhase::Interrupted,
        _ => return None,
    };
    let terminal = !matches!(phase, RestartPhase::Pending | RestartPhase::Restarting);
    attempt.phase = phase.clone();
    attempt.phase_at = now;
    attempt.terminal_at = terminal.then_some(now);
    Some(RestartTransition {
        service: service.to_owned(),
        phase,
    })
}

/// Fail all live attempts when their connection generation is no longer valid.
pub fn fail_discontinuous_restarts(
    state: &mut TopState,
    generation: u64,
    now: f64,
) -> Vec<RestartTransition> {
    state
        .restart_attempts
        .iter_mut()
        .filter_map(|(service, attempt)| {
            (attempt.generation != generation
                && matches!(
                    attempt.phase,
                    RestartPhase::Pending | RestartPhase::Restarting
                ))
            .then(|| {
                attempt.phase = RestartPhase::Failed(RestartFailure::Discontinuity);
                attempt.phase_at = now;
                attempt.terminal_at = Some(now);
                RestartTransition {
                    service: service.clone(),
                    phase: attempt.phase.clone(),
                }
            })
        })
        .collect()
}

fn is_supported(service: &str) -> bool {
    matches!(service, "convey" | "sense" | "cortex" | "spl")
}

fn restart_id(process_id: u32, sequence: u64) -> String {
    format!("top-{process_id}-{sequence}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    struct Transport {
        generation: u64,
        emitted: Vec<(String, String)>,
    }
    impl TopRestartTransport for Transport {
        fn emit_restart(&mut self, service: &str, id: &str) -> Result<(), TopRestartError> {
            self.emitted.push((service.to_owned(), id.to_owned()));
            Ok(())
        }
        fn current_generation(&self) -> u64 {
            self.generation
        }
    }
    fn state() -> TopState {
        TopState {
            services: vec![json!({"name":"convey"}), json!({"name":"local"})],
            ..TopState::default()
        }
    }

    #[test]
    fn supported_restart_is_exactly_once_and_retries_after_failure() {
        let mut state = state();
        let mut transport = Transport {
            generation: 4,
            ..Transport::default()
        };
        assert!(matches!(
            request_restart(&mut state, "convey", 0.0, &mut transport).unwrap(),
            RestartRequestOutcome::Emitted { .. }
        ));
        assert_eq!(
            request_restart(&mut state, "convey", 1.0, &mut transport).unwrap(),
            RestartRequestOutcome::Rejected
        );
        assert_eq!(transport.emitted.len(), 1);
        assert_eq!(advance_restart_attempts(&mut state, 5.0).len(), 1);
        assert!(matches!(
            request_restart(&mut state, "convey", 5.0, &mut transport).unwrap(),
            RestartRequestOutcome::Emitted { .. }
        ));
    }

    #[test]
    fn rejects_unsupported_and_stale_acknowledgements() {
        let mut state = state();
        let mut transport = Transport::default();
        for service in ["local", "parakeet", "supervisor", "unknown"] {
            assert_eq!(
                request_restart(&mut state, service, 0.0, &mut transport).unwrap(),
                RestartRequestOutcome::Rejected
            );
        }
        let RestartRequestOutcome::Emitted { restart_id } =
            request_restart(&mut state, "convey", 0.0, &mut transport).unwrap()
        else {
            panic!("emitted")
        };
        assert!(
            acknowledge_restart(
                &mut state,
                "convey",
                Some(&restart_id),
                1,
                "restarting",
                1.0
            )
            .is_none()
        );
        assert!(
            acknowledge_restart(
                &mut state,
                "convey",
                Some(&restart_id),
                0,
                "restarting",
                1.0
            )
            .is_some()
        );
        assert_eq!(advance_restart_attempts(&mut state, 11.0).len(), 1);
    }

    #[test]
    fn restart_ids_do_not_collide_between_processes() {
        assert_ne!(restart_id(101, 1), restart_id(202, 1));
        assert_ne!(restart_id(101, 1), restart_id(101, 2));
    }
}
