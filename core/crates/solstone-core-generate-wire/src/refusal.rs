// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_generate::{
    ReasonCode, ReasonCodeValue, RefusalReason, RefusedResponse, UnknownReasonCode, contract,
};

use crate::LaneOutcome;

pub fn refusal_for(
    outcome: &LaneOutcome,
    resolved_provider: &str,
    request_id: Option<String>,
) -> RefusedResponse {
    let (vector_id, reason, reason_code) = match outcome {
        LaneOutcome::NoEngine => (
            "refused-no-engine-configured",
            RefusalReason::NoEngineConfigured,
            Some("thinking_engine_not_chosen".to_owned()),
        ),
        LaneOutcome::AttestationNotVerified => (
            "refused-attestation-not-verified",
            RefusalReason::AttestationNotVerified,
            Some("attestation_not_yet_verified".to_owned()),
        ),
        LaneOutcome::BundledFailure(failure) => (
            "refused-provider-response-invalid",
            RefusalReason::ProviderResponseInvalid,
            failure.reason_code.clone(),
        ),
        LaneOutcome::UnimplementedLane => (
            "refused-provider-response-invalid",
            RefusalReason::ProviderResponseInvalid,
            None,
        ),
        LaneOutcome::BundledLocal => {
            panic!("bundled local lane must be invoked before refusal mapping")
        }
    };
    let vector = contract()["conformance_vectors"]
        .as_array()
        .expect("generate contract vectors are an array")
        .iter()
        .find(|vector| vector["id"].as_str() == Some(vector_id))
        .expect("generate contract required refusal vector is present");
    let detail = vector["response"]["detail"]
        .as_str()
        .expect("generate contract refusal detail is a string")
        .to_owned();
    let (reason_code, retryable, blocking) = classify_reason_code(reason_code);
    RefusedResponse {
        id: request_id,
        reason,
        reason_code,
        retryable,
        blocking,
        reset_at_ms: None,
        provider: Some(resolved_provider.to_owned()),
        detail,
    }
}

fn classify_reason_code(reason_code: Option<String>) -> (Option<ReasonCodeValue>, bool, bool) {
    let fallback = &contract()["unknown_member"];
    let Some(received) = reason_code else {
        return (
            None,
            fallback["retryable"]
                .as_bool()
                .expect("generate unknown-member retryable is boolean"),
            fallback["blocking"]
                .as_bool()
                .expect("generate unknown-member blocking is boolean"),
        );
    };
    let value = match ReasonCode::new(received.clone()) {
        Ok(code) => ReasonCodeValue::Known(code),
        Err(_) => ReasonCodeValue::Unknown(UnknownReasonCode {
            received,
            canonical: ReasonCode::new("unknown").expect("unknown code is in fixture"),
        }),
    };
    let entry = contract()["reason_codes"]
        .as_array()
        .expect("generate contract reason codes are an array")
        .iter()
        .find(|entry| entry["code"].as_str() == Some(value.as_wire()));
    let classification = entry.unwrap_or(fallback);
    (
        Some(value),
        classification["retryable"]
            .as_bool()
            .expect("generate classification retryable is boolean"),
        classification["blocking"]
            .as_bool()
            .expect("generate classification blocking is boolean"),
    )
}

#[cfg(test)]
mod tests {
    use solstone_core_local::GenerateFailure;

    use super::*;

    #[test]
    fn lane_outcomes_use_fixture_vectors_and_provider() {
        let cases = [
            (
                LaneOutcome::NoEngine,
                "none",
                RefusalReason::NoEngineConfigured,
                Some("thinking_engine_not_chosen"),
                true,
                true,
            ),
            (
                LaneOutcome::AttestationNotVerified,
                "local",
                RefusalReason::AttestationNotVerified,
                Some("attestation_not_yet_verified"),
                true,
                true,
            ),
            (
                LaneOutcome::UnimplementedLane,
                "local",
                RefusalReason::ProviderResponseInvalid,
                None,
                false,
                true,
            ),
        ];
        for (outcome, provider, reason, reason_code, retryable, blocking) in cases {
            let refusal = refusal_for(&outcome, provider, Some("request".into()));
            assert_eq!(refusal.reason, reason);
            assert_eq!(
                refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                reason_code
            );
            assert_eq!(refusal.retryable, retryable);
            assert_eq!(refusal.blocking, blocking);
            assert_eq!(refusal.provider.as_deref(), Some(provider));
        }
    }

    #[test]
    fn bundled_failure_preserves_known_unknown_and_absent_codes() {
        for (reason_code, expected_wire, retryable, blocking) in [
            (
                Some("provider_response_invalid"),
                Some("provider_response_invalid"),
                true,
                false,
            ),
            (Some("future_code"), Some("future_code"), false, true),
            (None, None, false, true),
        ] {
            let refusal = refusal_for(
                &LaneOutcome::BundledFailure(Box::new(GenerateFailure {
                    schema: "test".into(),
                    outcome: "failure".into(),
                    reason_code: reason_code.map(str::to_owned),
                    detail: "specific failure detail must not escape the fixture fallback".into(),
                    inference: None,
                })),
                "local",
                None,
            );
            assert_eq!(refusal.reason, RefusalReason::ProviderResponseInvalid);
            assert_eq!(
                refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                expected_wire
            );
            assert_eq!(refusal.retryable, retryable);
            assert_eq!(refusal.blocking, blocking);
            assert_eq!(refusal.detail, "fixture provider-response-invalid");
        }
    }
}
