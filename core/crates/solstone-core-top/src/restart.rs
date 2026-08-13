// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::TopState;

const NO_ACK_SECONDS: f64 = 5.0;
const RESTART_SECONDS: f64 = 10.0;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RestartIdError {
    #[error("restart entropy unavailable")]
    EntropyUnavailable,
    #[error("restart id sequence exhausted")]
    SequenceExhausted,
}

/// Session-owned opaque restart ID source. It is deliberately separate from
/// transport so tests can exercise correlation without ambient entropy.
pub trait RestartIdSource {
    fn next_restart_id(&mut self) -> Result<String, RestartIdError>;
}

/// Production restart IDs share one nonce for a Top session and use a checked
/// sequence so an ID is never reused after any enqueue outcome.
#[derive(Clone, Debug)]
pub struct SessionRestartIds {
    process_id: u32,
    nonce: Result<[u8; 16], RestartIdError>,
    next_sequence: u64,
}

impl SessionRestartIds {
    #[must_use]
    pub fn new() -> Self {
        let mut nonce = [0_u8; 16];
        let nonce = getrandom::fill(&mut nonce)
            .map(|()| nonce)
            .map_err(|_| RestartIdError::EntropyUnavailable);
        Self {
            process_id: std::process::id(),
            nonce,
            next_sequence: 1,
        }
    }

    #[must_use]
    pub fn with_nonce(process_id: u32, nonce: [u8; 16]) -> Self {
        Self {
            process_id,
            nonce: Ok(nonce),
            next_sequence: 1,
        }
    }

    #[must_use]
    pub fn unavailable(process_id: u32) -> Self {
        Self {
            process_id,
            nonce: Err(RestartIdError::EntropyUnavailable),
            next_sequence: 1,
        }
    }

    #[must_use]
    pub fn with_nonce_and_sequence(process_id: u32, nonce: [u8; 16], next_sequence: u64) -> Self {
        Self {
            process_id,
            nonce: Ok(nonce),
            next_sequence,
        }
    }

    #[must_use]
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
}

impl Default for SessionRestartIds {
    fn default() -> Self {
        Self::new()
    }
}

impl RestartIdSource for SessionRestartIds {
    fn next_restart_id(&mut self) -> Result<String, RestartIdError> {
        let nonce = self.nonce.as_ref().map_err(Clone::clone)?;
        if self.next_sequence == u64::MAX {
            return Err(RestartIdError::SequenceExhausted);
        }
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(RestartIdError::SequenceExhausted)?;
        Ok(format!(
            "top-v1-{}-{}-{sequence}",
            self.process_id,
            nonce
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestartFailure {
    RestartTimedOut,
    Discontinuity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestartPhase {
    Pending,
    Restarting,
    Stopped,
    Started,
    Interrupted,
    Failed(RestartFailure),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RestartAttempt {
    pub restart_id: String,
    pub generation: u64,
    pub epoch: u64,
    pub phase: RestartPhase,
    pub issued_at: f64,
    pub phase_at: f64,
    pub started_deadline: Option<f64>,
    pub terminal_at: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestartEnqueueResult {
    Enqueued,
    Full,
    Closed,
    TransportError,
}

/// Transport is deliberately separate from restart protocol state.
pub trait TopRestartTransport {
    fn emit_restart(&mut self, service: &str, restart_id: &str) -> RestartEnqueueResult;
    fn current_generation(&self) -> u64;
    fn current_epoch(&self) -> u64;
    fn restart_ids(&mut self) -> &mut dyn RestartIdSource;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestartRequestOutcome {
    Emitted {
        restart_id: String,
    },
    Rejected,
    Failed {
        restart_id: Option<String>,
        error: RestartRequestError,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestartRequestError {
    Id(RestartIdError),
    QueueFull,
    QueueClosed,
    Transport,
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
) -> RestartRequestOutcome {
    if !is_supported(service)
        || !state
            .services
            .iter()
            .any(|entry| entry.get("name").and_then(serde_json::Value::as_str) == Some(service))
    {
        return RestartRequestOutcome::Rejected;
    }
    if state.restart_attempts.get(service).is_some_and(|attempt| {
        matches!(
            attempt.phase,
            RestartPhase::Pending | RestartPhase::Restarting | RestartPhase::Stopped
        )
    }) {
        return RestartRequestOutcome::Rejected;
    }
    let restart_id = match transport.restart_ids().next_restart_id() {
        Ok(restart_id) => restart_id,
        Err(error) => {
            return RestartRequestOutcome::Failed {
                restart_id: None,
                error: RestartRequestError::Id(error),
            };
        }
    };
    let enqueue = transport.emit_restart(service, &restart_id);
    if enqueue != RestartEnqueueResult::Enqueued {
        let error = match enqueue {
            RestartEnqueueResult::Full => RestartRequestError::QueueFull,
            RestartEnqueueResult::Closed => RestartRequestError::QueueClosed,
            RestartEnqueueResult::TransportError => RestartRequestError::Transport,
            RestartEnqueueResult::Enqueued => unreachable!(),
        };
        return RestartRequestOutcome::Failed {
            restart_id: Some(restart_id),
            error,
        };
    }
    state.restart_attempts.insert(
        service.to_owned(),
        RestartAttempt {
            restart_id: restart_id.clone(),
            generation: transport.current_generation(),
            epoch: transport.current_epoch(),
            phase: RestartPhase::Pending,
            issued_at: now,
            phase_at: now,
            started_deadline: None,
            terminal_at: None,
        },
    );
    RestartRequestOutcome::Emitted { restart_id }
}

/// Advance timeout bounds and return newly terminal transitions.
pub fn advance_restart_attempts(state: &mut TopState, now: f64) -> Vec<RestartTransition> {
    let mut transitions = Vec::new();
    for (service, attempt) in &mut state.restart_attempts {
        let failure = match attempt.phase {
            RestartPhase::Pending if now - attempt.issued_at >= NO_ACK_SECONDS => None,
            RestartPhase::Restarting | RestartPhase::Stopped
                if attempt
                    .started_deadline
                    .is_some_and(|deadline| now >= deadline) =>
            {
                Some(RestartFailure::RestartTimedOut)
            }
            _ => None,
        };
        if matches!(attempt.phase, RestartPhase::Pending)
            && now - attempt.issued_at >= NO_ACK_SECONDS
        {
            attempt.phase = RestartPhase::Interrupted;
            attempt.phase_at = now;
            attempt.terminal_at = Some(now);
            transitions.push(RestartTransition {
                service: service.clone(),
                phase: RestartPhase::Interrupted,
            });
        } else if let Some(failure) = failure {
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

/// Correlate an authoritative lifecycle acknowledgement only when generation,
/// epoch, service, and restart id all match the live attempt.
pub fn acknowledge_restart(
    state: &mut TopState,
    service: &str,
    restart_id: Option<&str>,
    generation: u64,
    epoch: u64,
    event: &str,
    now: f64,
) -> Option<RestartTransition> {
    let attempt = state.restart_attempts.get_mut(service)?;
    if attempt.generation != generation
        || attempt.epoch != epoch
        || restart_id != Some(attempt.restart_id.as_str())
    {
        return None;
    }
    let phase = match (&attempt.phase, event) {
        (RestartPhase::Pending | RestartPhase::Stopped, "restarting") => RestartPhase::Restarting,
        (RestartPhase::Pending | RestartPhase::Restarting, "stopped") => RestartPhase::Stopped,
        (RestartPhase::Pending | RestartPhase::Restarting | RestartPhase::Stopped, "started") => {
            RestartPhase::Started
        }
        _ => return None,
    };
    if matches!(phase, RestartPhase::Restarting | RestartPhase::Stopped) {
        attempt
            .started_deadline
            .get_or_insert(now + RESTART_SECONDS);
    }
    let terminal = matches!(phase, RestartPhase::Started);
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
                    RestartPhase::Pending | RestartPhase::Restarting | RestartPhase::Stopped
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_nonce_id_is_session_scoped_and_checked() {
        let mut ids = SessionRestartIds::with_nonce(42, [0xab; 16]);
        assert_eq!(
            ids.next_restart_id(),
            Ok("top-v1-42-abababababababababababababababab-1".to_owned())
        );
        assert_eq!(ids.next_sequence(), 2);
        let mut exhausted = SessionRestartIds::with_nonce_and_sequence(42, [0; 16], u64::MAX);
        assert_eq!(
            exhausted.next_restart_id(),
            Err(RestartIdError::SequenceExhausted)
        );
    }
}
