// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

use crate::{
    REASON_HOME_MISSING_MOBILE, REASON_LOCAL_PRIVATE_LISTENER_UNREACHABLE,
    REASON_RELAY_ADMISSION_SATURATED, REASON_RELAY_TUNNEL_REJECTED,
    REASON_RELAY_TUNNEL_UNREACHABLE, REASON_SERVICE_TOKEN_REJECTED,
};

/// The owner-visible connection state for the relay listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayHealthState {
    Connecting,
    Connected,
    Reconnecting,
}

impl RelayHealthState {
    /// Returns the stable owner-visible state spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
        }
    }
}

/// A relay-tunnel outcome that has a stable owner-visible reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayTunnelFailure {
    HomeMissingMobile,
    ServiceTokenRejected,
    RelayTunnelRejected { status: u16 },
    RelayTunnelUnreachable,
    LocalPrivateListenerUnreachable,
    RelayAdmissionSaturated,
}

impl RelayTunnelFailure {
    /// Returns the U3 reason for this tunnel failure.
    pub fn reason(self) -> &'static str {
        match self {
            Self::HomeMissingMobile => REASON_HOME_MISSING_MOBILE,
            Self::ServiceTokenRejected => REASON_SERVICE_TOKEN_REJECTED,
            Self::RelayTunnelRejected { .. } => REASON_RELAY_TUNNEL_REJECTED,
            Self::RelayTunnelUnreachable => REASON_RELAY_TUNNEL_UNREACHABLE,
            Self::LocalPrivateListenerUnreachable => REASON_LOCAL_PRIVATE_LISTENER_UNREACHABLE,
            Self::RelayAdmissionSaturated => REASON_RELAY_ADMISSION_SATURATED,
        }
    }

    /// Returns the relay HTTP status when the relay rejected the tunnel.
    pub fn status(self) -> Option<u16> {
        match self {
            Self::RelayTunnelRejected { status } => Some(status),
            Self::HomeMissingMobile
            | Self::ServiceTokenRejected
            | Self::RelayTunnelUnreachable
            | Self::LocalPrivateListenerUnreachable
            | Self::RelayAdmissionSaturated => None,
        }
    }
}

/// Pure in-memory state for a relay listener's owner-visible health payload.
///
/// This type performs no I/O, runtime work, clock reads, or event emission.
/// Callers provide the observation timestamps and admission saturation count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayHealth {
    state: RelayHealthState,
    listen_generation: u64,
    last_successful_relay_tunnel_at: Option<u64>,
    last_relay_tunnel_error: Option<&'static str>,
    last_relay_tunnel_error_at: Option<u64>,
    relay_tunnel_error_status: Option<u16>,
    relay_admission_saturated_count: u64,
    last_relay_listener_ack_at: Option<u64>,
    last_relay_listener_ack_generation: Option<u64>,
}

impl RelayHealth {
    /// Creates an initial health record before any listener attempt has begun.
    pub fn new() -> Self {
        Self {
            state: RelayHealthState::Connecting,
            listen_generation: 0,
            last_successful_relay_tunnel_at: None,
            last_relay_tunnel_error: None,
            last_relay_tunnel_error_at: None,
            relay_tunnel_error_status: None,
            relay_admission_saturated_count: 0,
            last_relay_listener_ack_at: None,
            last_relay_listener_ack_generation: None,
        }
    }

    /// Starts a new relay listener attempt.
    pub fn begin_listen_attempt(&mut self) {
        self.listen_generation = self.listen_generation.saturating_add(1);
    }

    /// Updates the owner-visible listener state.
    pub fn set_state(&mut self, state: RelayHealthState) {
        self.state = state;
    }

    /// Records a successful relay tunnel and clears the prior tunnel error.
    pub fn record_tunnel_success(&mut self, timestamp_ms: u64) {
        self.last_successful_relay_tunnel_at = Some(timestamp_ms);
        self.last_relay_tunnel_error = None;
        self.last_relay_tunnel_error_at = None;
        self.relay_tunnel_error_status = None;
    }

    /// Records a failed relay tunnel without changing the last success.
    pub fn record_tunnel_failure(&mut self, failure: RelayTunnelFailure, timestamp_ms: u64) {
        self.last_relay_tunnel_error = Some(failure.reason());
        self.last_relay_tunnel_error_at = Some(timestamp_ms);
        self.relay_tunnel_error_status = failure.status();
    }

    /// Records an acknowledged heartbeat for the current listener generation.
    pub fn record_listener_ack(&mut self, timestamp_ms: u64) {
        self.last_relay_listener_ack_at = Some(
            self.last_relay_listener_ack_at
                .map_or(timestamp_ms, |previous| previous.max(timestamp_ms)),
        );
        self.last_relay_listener_ack_generation = Some(self.listen_generation);
    }

    /// Replaces the cumulative relay-admission saturation count.
    pub fn set_relay_admission_saturated_count(&mut self, count: u64) {
        self.relay_admission_saturated_count = count;
    }

    /// Returns the complete owner-visible relay health payload.
    pub fn payload(&self) -> Value {
        json!({
            "state": self.state.as_str(),
            "listen_generation": self.listen_generation,
            "last_successful_relay_tunnel_at": self.last_successful_relay_tunnel_at,
            "last_relay_tunnel_error": self.last_relay_tunnel_error,
            "last_relay_tunnel_error_at": self.last_relay_tunnel_error_at,
            "relay_tunnel_error_status": self.relay_tunnel_error_status,
            "relay_admission_saturated_count": self.relay_admission_saturated_count,
            "last_relay_listener_ack_at": self.last_relay_listener_ack_at,
            "last_relay_listener_ack_generation": self.last_relay_listener_ack_generation,
        })
    }
}

impl Default for RelayHealth {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::io;

    use serde_json::{Value, json};

    use super::{RelayHealth, RelayHealthState, RelayTunnelFailure};
    use crate::{
        LINK_HEALTH_EVENT, OFFLINE_TUNNEL_REASONS, REASON_HOME_MISSING_MOBILE,
        REASON_LOCAL_PRIVATE_LISTENER_UNREACHABLE, REASON_RELAY_ADMISSION_SATURATED,
        REASON_RELAY_TUNNEL_REJECTED, REASON_RELAY_TUNNEL_UNREACHABLE,
        REASON_SERVICE_TOKEN_REJECTED,
    };

    #[test]
    fn new_payload_has_exact_owner_visible_keys_and_null_error_fields() {
        let health = RelayHealth::new();

        assert_eq!(
            health.payload(),
            json!({
                "state": "connecting",
                "listen_generation": 0,
                "last_successful_relay_tunnel_at": null,
                "last_relay_tunnel_error": null,
                "last_relay_tunnel_error_at": null,
                "relay_tunnel_error_status": null,
                "relay_admission_saturated_count": 0,
                "last_relay_listener_ack_at": null,
                "last_relay_listener_ack_generation": null,
            })
        );
    }

    #[test]
    fn rejected_tunnel_payload_includes_its_http_status() {
        let mut health = RelayHealth::new();
        health.begin_listen_attempt();
        health.set_state(RelayHealthState::Reconnecting);
        health.record_tunnel_failure(
            RelayTunnelFailure::RelayTunnelRejected { status: 503 },
            1_700_000_000_123,
        );

        assert_eq!(
            health.payload(),
            json!({
                "state": "reconnecting",
                "listen_generation": 1,
                "last_successful_relay_tunnel_at": null,
                "last_relay_tunnel_error": "relay_tunnel_rejected",
                "last_relay_tunnel_error_at": 1_700_000_000_123_u64,
                "relay_tunnel_error_status": 503,
                "relay_admission_saturated_count": 0,
                "last_relay_listener_ack_at": null,
                "last_relay_listener_ack_generation": null,
            })
        );
    }

    #[test]
    fn successful_tunnel_after_failure_clears_only_error_fields() {
        let mut health = RelayHealth::new();
        health.begin_listen_attempt();
        health.set_state(RelayHealthState::Connected);
        health.set_relay_admission_saturated_count(7);
        health.record_tunnel_failure(RelayTunnelFailure::ServiceTokenRejected, 1_000);
        health.record_tunnel_success(1_001);

        assert_eq!(
            health.payload(),
            json!({
                "state": "connected",
                "listen_generation": 1,
                "last_successful_relay_tunnel_at": 1_001,
                "last_relay_tunnel_error": null,
                "last_relay_tunnel_error_at": null,
                "relay_tunnel_error_status": null,
                "relay_admission_saturated_count": 7,
                "last_relay_listener_ack_at": null,
                "last_relay_listener_ack_generation": null,
            })
        );
    }

    #[test]
    fn listener_ack_timestamp_never_moves_backward() {
        let mut health = RelayHealth::new();
        health.begin_listen_attempt();
        health.record_listener_ack(1_000);
        health.begin_listen_attempt();
        health.record_listener_ack(999);

        assert_eq!(health.payload()["last_relay_listener_ack_at"], 1_000);
        assert_eq!(
            health.payload()["last_relay_listener_ack_generation"],
            health.payload()["listen_generation"],
        );
    }

    #[test]
    fn a11_health_payloads_match_the_retained_parity_corpus()
    -> Result<(), Box<dyn std::error::Error>> {
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/vectors/spl-parity-corpus.json"))?;
        let constants = corpus
            .get("constants")
            .ok_or_else(|| io::Error::other("missing corpus constants"))?;
        let all_null = corpus_payload(&corpus, "all_null")?;
        let populated = corpus_payload(&corpus, "populated_rejected")?;

        assert_eq!(
            constants.get("reasons"),
            Some(&json!({
                "home_missing_mobile": REASON_HOME_MISSING_MOBILE,
                "service_token_rejected": REASON_SERVICE_TOKEN_REJECTED,
                "relay_tunnel_rejected": REASON_RELAY_TUNNEL_REJECTED,
                "relay_tunnel_unreachable": REASON_RELAY_TUNNEL_UNREACHABLE,
                "local_private_listener_unreachable": REASON_LOCAL_PRIVATE_LISTENER_UNREACHABLE,
                "relay_admission_saturated": REASON_RELAY_ADMISSION_SATURATED,
            }))
        );
        assert_eq!(
            constants.get("LINK_HEALTH_EVENT"),
            Some(&Value::String(LINK_HEALTH_EVENT.to_owned()))
        );
        let expected_offline: BTreeSet<String> = constants
            .get("OFFLINE_TUNNEL_REASONS")
            .and_then(Value::as_array)
            .ok_or_else(|| io::Error::other("missing offline-reasons corpus value"))?
            .iter()
            .map(|reason| {
                reason
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| io::Error::other("non-string offline-reason corpus value"))
            })
            .collect::<Result<_, _>>()?;
        let actual_offline: BTreeSet<String> = OFFLINE_TUNNEL_REASONS
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(actual_offline, expected_offline);

        let mut initial = RelayHealth::new();
        initial.begin_listen_attempt();
        assert_eq!(initial.payload(), all_null);

        let mut rejected = RelayHealth::new();
        for _ in 0..7 {
            rejected.begin_listen_attempt();
        }
        rejected.set_state(RelayHealthState::Connected);
        rejected.record_listener_ack(1_750_000_000_000);
        rejected.record_tunnel_success(1_750_000_000_000);
        rejected.record_tunnel_failure(
            RelayTunnelFailure::RelayTunnelRejected { status: 502 },
            1_750_000_001_000,
        );
        rejected.set_relay_admission_saturated_count(3);
        assert_eq!(rejected.payload(), populated);
        Ok(())
    }

    fn corpus_payload(corpus: &Value, name: &str) -> Result<Value, Box<dyn std::error::Error>> {
        corpus
            .get("health_payloads")
            .and_then(|payloads| payloads.get(name))
            .cloned()
            .ok_or_else(|| io::Error::other(format!("missing health payload {name}")))
            .map_err(Into::into)
    }
}
