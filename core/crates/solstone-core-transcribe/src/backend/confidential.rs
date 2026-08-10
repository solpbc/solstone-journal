// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Confidential-lane routing inputs and the pre-dispatch egress refusal gate.

use serde_json::{Map, Value};
use solstone_core_journal_config::JournalConfigRead;

use crate::TranscribeError;

/// Return the `services.confidential` object only when both levels are objects.
pub(crate) fn confidential_provenance(config: &JournalConfigRead) -> Option<Map<String, Value>> {
    config
        .config
        .as_ref()
        .and_then(|root| root.get("services"))
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .and_then(Value::as_object)
        .cloned()
}

/// A routing approximation that omits the bundled-endpoint exclusion until W5b.
pub(crate) fn confidential_channel_plausible(config: &JournalConfigRead) -> bool {
    confidential_provenance(config).is_some()
        && config
            .config
            .as_ref()
            .and_then(|root| root.get("providers"))
            .and_then(Value::as_object)
            .and_then(|providers| providers.get("local"))
            .and_then(Value::as_object)
            .and_then(|local| local.get("credential"))
            .is_some_and(json_truthy)
}

/// Whether a registered STT backend keeps raw audio on this machine.
pub(crate) fn is_local_backend(name: &str) -> bool {
    matches!(name, "parakeet" | "parakeet-cpp")
}

/// Refuse remote STT before raw audio can leave an active confidential lane.
pub(crate) fn refuse_confidential_egress(
    config: &JournalConfigRead,
    backend: &str,
    confidential_audio_enabled: bool,
) -> Result<(), TranscribeError> {
    if confidential_provenance(config).is_none() {
        return if backend == "confidential" {
            Err(deferred(
                "confidential_lane_inactive",
                "the confidential lane is no longer active",
            ))
        } else {
            Ok(())
        };
    }

    if is_local_backend(backend) {
        return Ok(());
    }
    if backend == "confidential" {
        return if confidential_audio_enabled {
            Ok(())
        } else {
            Err(deferred(
                "confidential_audio_disabled",
                "confidential audio handling is disabled",
            ))
        };
    }

    Err(deferred(
        "confidential_egress_blocked",
        format!("confidential lane blocks STT backend {backend:?}; raw audio must stay local"),
    ))
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => {
            value.as_i64().is_some_and(|value| value != 0)
                || value.as_u64().is_some_and(|value| value != 0)
                || value.as_f64().is_some_and(|value| value != 0.0)
        }
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn deferred(reason: impl Into<String>, detail: impl Into<String>) -> TranscribeError {
    TranscribeError::ConfidentialDeferred {
        reason: reason.into(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use solstone_core_journal_config::JournalConfigRead;

    use super::{
        confidential_channel_plausible, confidential_provenance, refuse_confidential_egress,
    };
    use crate::TranscribeError;

    #[test]
    fn active_lane_permits_parakeet() {
        assert!(refuse_confidential_egress(&active_config(), "parakeet", false).is_ok());
    }

    #[test]
    fn active_lane_permits_parakeet_cpp() {
        assert!(refuse_confidential_egress(&active_config(), "parakeet-cpp", false).is_ok());
    }

    #[test]
    fn active_lane_permits_enabled_confidential_backend() {
        assert!(refuse_confidential_egress(&active_config(), "confidential", true).is_ok());
    }

    #[test]
    fn active_lane_defers_disabled_confidential_backend() {
        assert_deferred_reason(
            refuse_confidential_egress(&active_config(), "confidential", false).unwrap_err(),
            "confidential_audio_disabled",
        );
    }

    #[test]
    fn active_lane_refuses_remote_backend_before_dispatch() {
        assert_deferred_reason(
            refuse_confidential_egress(&active_config(), "remote", true).unwrap_err(),
            "confidential_egress_blocked",
        );
    }

    #[test]
    fn inactive_lane_is_a_no_op_for_local_backend() {
        assert!(refuse_confidential_egress(&config(json!({})), "parakeet", false).is_ok());
    }

    #[test]
    fn inactive_lane_is_a_no_op_for_remote_backend() {
        assert!(refuse_confidential_egress(&config(json!({})), "remote", false).is_ok());
    }

    #[test]
    fn inactive_lane_defers_confidential_backend() {
        assert_deferred_reason(
            refuse_confidential_egress(&config(json!({})), "confidential", true).unwrap_err(),
            "confidential_lane_inactive",
        );
    }

    #[test]
    fn provenance_returns_present_object() {
        let config = config(json!({"services":{"confidential":{"device":"abc"}}}));

        assert_eq!(
            confidential_provenance(&config),
            Some(serde_json::from_value(json!({"device":"abc"})).unwrap())
        );
    }

    #[test]
    fn provenance_is_none_when_services_is_missing() {
        assert_eq!(confidential_provenance(&config(json!({}))), None);
    }

    #[test]
    fn provenance_is_none_when_services_is_not_an_object() {
        assert_eq!(
            confidential_provenance(&config(json!({"services":true}))),
            None
        );
    }

    #[test]
    fn provenance_is_none_when_confidential_is_not_an_object() {
        assert_eq!(
            confidential_provenance(&config(json!({"services":{"confidential":true}}))),
            None
        );
    }

    #[test]
    fn plausible_channel_requires_a_truthy_local_credential() {
        assert!(confidential_channel_plausible(&active_config()));
        assert!(!confidential_channel_plausible(&config(json!({
            "services":{"confidential":{}},
            "providers":{"local":{}}
        }))));
        assert!(!confidential_channel_plausible(&config(json!({
            "services":{"confidential":{}},
            "providers":{"local":{"credential":""}}
        }))));
    }

    fn active_config() -> JournalConfigRead {
        config(json!({
            "services":{"confidential":{"device":"abc"}},
            "providers":{"local":{"credential":"secret"}}
        }))
    }

    fn config(value: Value) -> JournalConfigRead {
        JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(value.as_object().unwrap().clone()),
        }
    }

    fn assert_deferred_reason(error: TranscribeError, expected_reason: &str) {
        assert_eq!(error.exit_code(), 69);
        let TranscribeError::ConfidentialDeferred { reason, .. } = error else {
            panic!("expected confidential deferral");
        };
        assert_eq!(reason, expected_reason);
    }
}
