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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FailureRecord {
    reason: &'static str,
    at: u64,
    status: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TunnelFailureRecord {
    failure: FailureRecord,
    recorded_before_upgrade: bool,
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
    listen_failure: Option<FailureRecord>,
    tunnel_failure: Option<TunnelFailureRecord>,
    upgrade_marked: bool,
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
            listen_failure: None,
            tunnel_failure: None,
            upgrade_marked: false,
            relay_admission_saturated_count: 0,
            last_relay_listener_ack_at: None,
            last_relay_listener_ack_generation: None,
        }
    }

    /// Starts a new relay listener attempt.
    pub fn begin_listen_attempt(&mut self) {
        self.listen_generation = self.listen_generation.saturating_add(1);
        self.upgrade_marked = false;
        if let Some(tunnel_failure) = self.tunnel_failure.as_mut() {
            tunnel_failure.recorded_before_upgrade = true;
        }
    }

    /// Marks that the current listen connection has upgraded to WebSocket.
    pub fn mark_listen_upgraded(&mut self) {
        self.upgrade_marked = true;
    }

    /// Updates the owner-visible listener state.
    pub fn set_state(&mut self, state: RelayHealthState) {
        self.state = state;
    }

    /// Records a failed listen connection attempt.
    pub fn record_listen_failure(&mut self, failure: RelayTunnelFailure, timestamp_ms: u64) {
        self.listen_failure = Some(FailureRecord {
            reason: failure.reason(),
            at: timestamp_ms,
            status: failure.status(),
        });
    }

    /// Records a successful relay tunnel and clears prior failures.
    pub fn record_tunnel_success(&mut self, timestamp_ms: u64) {
        self.last_successful_relay_tunnel_at = Some(timestamp_ms);
        self.listen_failure = None;
        self.tunnel_failure = None;
    }

    /// Records a failed relay tunnel without changing the last success.
    pub fn record_tunnel_failure(&mut self, failure: RelayTunnelFailure, timestamp_ms: u64) {
        self.tunnel_failure = Some(TunnelFailureRecord {
            failure: FailureRecord {
                reason: failure.reason(),
                at: timestamp_ms,
                status: failure.status(),
            },
            recorded_before_upgrade: !self.upgrade_marked,
        });
    }

    /// Clears failures cleared on the first acknowledgement of a generation.
    pub fn clear_on_first_acknowledgement(&mut self) {
        self.listen_failure = None;
        if let Some(tunnel_failure) = self.tunnel_failure
            && tunnel_failure.failure.reason == crate::REASON_SERVICE_TOKEN_REJECTED
            && tunnel_failure.recorded_before_upgrade
        {
            self.tunnel_failure = None;
        }
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
        let failure = self
            .listen_failure
            .or_else(|| self.tunnel_failure.map(|t| t.failure));
        let (reason, error_at, status) = match failure {
            Some(f) => (Some(f.reason), Some(f.at), f.status),
            None => (None, None, None),
        };
        json!({
            "state": self.state.as_str(),
            "listen_generation": self.listen_generation,
            "last_successful_relay_tunnel_at": self.last_successful_relay_tunnel_at,
            "last_relay_tunnel_error": reason,
            "last_relay_tunnel_error_at": error_at,
            "relay_tunnel_error_status": status,
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
    use serde_json::json;

    use super::{RelayHealth, RelayHealthState, RelayTunnelFailure};

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
    fn hidden_tunnel_failure_returns_with_its_original_timestamp() {
        let mut health = RelayHealth::new();
        health.begin_listen_attempt();
        let t = 1_700_000_000_500;
        health.record_tunnel_failure(RelayTunnelFailure::RelayTunnelRejected { status: 503 }, t);

        // Next generation records a listen transport failure.
        health.begin_listen_attempt();
        health.record_listen_failure(
            RelayTunnelFailure::RelayTunnelUnreachable,
            1_700_000_001_000,
        );
        assert_eq!(
            health.payload()["last_relay_tunnel_error"],
            "relay_tunnel_unreachable"
        );
        assert_eq!(
            health.payload()["last_relay_tunnel_error_at"],
            1_700_000_001_000_u64
        );
        assert_eq!(
            health.payload()["relay_tunnel_error_status"],
            serde_json::Value::Null
        );

        // Next generation marks upgrade and first ack.
        health.begin_listen_attempt();
        health.mark_listen_upgraded();
        health.record_listener_ack(1_700_000_002_000);
        health.clear_on_first_acknowledgement();
        assert_eq!(
            health.payload()["last_relay_tunnel_error"],
            "relay_tunnel_rejected"
        );
        assert_eq!(health.payload()["relay_tunnel_error_status"], 503);
        assert_eq!(health.payload()["last_relay_tunnel_error_at"], t);

        // Repeat sequence with LocalPrivateListenerUnreachable.
        let t2 = 1_700_000_003_000;
        health.record_tunnel_failure(RelayTunnelFailure::LocalPrivateListenerUnreachable, t2);
        health.begin_listen_attempt();
        health.record_listen_failure(
            RelayTunnelFailure::RelayTunnelUnreachable,
            1_700_000_004_000,
        );
        assert_eq!(
            health.payload()["last_relay_tunnel_error"],
            "relay_tunnel_unreachable"
        );

        health.begin_listen_attempt();
        health.mark_listen_upgraded();
        health.record_listener_ack(1_700_000_005_000);
        health.clear_on_first_acknowledgement();
        assert_eq!(
            health.payload()["last_relay_tunnel_error"],
            "local_private_listener_unreachable"
        );
        assert_eq!(
            health.payload()["relay_tunnel_error_status"],
            serde_json::Value::Null
        );
        assert_eq!(health.payload()["last_relay_tunnel_error_at"], t2);
    }

    #[test]
    fn first_ack_clears_only_a_token_rejection_recorded_before_upgrade() {
        let mut health = RelayHealth::new();
        health.begin_listen_attempt();
        health.record_tunnel_failure(RelayTunnelFailure::ServiceTokenRejected, 1_000);
        health.mark_listen_upgraded();
        health.record_listener_ack(1_500);
        health.clear_on_first_acknowledgement();
        assert_eq!(
            health.payload()["last_relay_tunnel_error"],
            serde_json::Value::Null
        );
        assert_eq!(
            health.payload()["last_relay_tunnel_error_at"],
            serde_json::Value::Null
        );

        // Second case: begin, mark upgrade, then record tunnel failure, then first ack.
        health.begin_listen_attempt();
        health.mark_listen_upgraded();
        health.record_tunnel_failure(RelayTunnelFailure::ServiceTokenRejected, 2_000);
        health.record_listener_ack(2_500);
        health.clear_on_first_acknowledgement();
        assert_eq!(
            health.payload()["last_relay_tunnel_error"],
            "service_token_rejected"
        );
        assert_eq!(health.payload()["last_relay_tunnel_error_at"], 2_000);
    }

    #[test]
    fn first_ack_projects_the_tunnel_rejection_timestamp() {
        let mut health = RelayHealth::new();
        health.begin_listen_attempt();
        health.record_listen_failure(RelayTunnelFailure::ServiceTokenRejected, 1_000);

        health.begin_listen_attempt();
        health.mark_listen_upgraded();
        health.record_tunnel_failure(RelayTunnelFailure::ServiceTokenRejected, 2_000);
        health.record_listener_ack(2_500);
        health.clear_on_first_acknowledgement();
        assert_eq!(
            health.payload()["last_relay_tunnel_error"],
            "service_token_rejected"
        );
        assert_eq!(health.payload()["last_relay_tunnel_error_at"], 2_000);
    }
}
