// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_generate::{
    ReasonCode, ReasonCodeValue, RefusalReason, RefusedResponse, UnknownReasonCode, contract,
};

use crate::{LaneOutcome, SanitizedFinishReason, ValidationFailure};

pub(crate) const LIVE_PROVIDER_FAILURE_DETAIL: &str =
    "the configured provider could not produce a usable response";
pub(crate) const HONEST_PROVIDER_RESPONSE_INVALID_DETAIL: &str =
    "the provider returned a response with no visible output";
pub(crate) const HONEST_UNIMPLEMENTED_LANE_DETAIL: &str =
    "no generate implementation exists for the resolved provider lane";

pub fn refusal_for(
    outcome: &LaneOutcome,
    resolved_provider: &str,
    request_id: Option<String>,
) -> RefusedResponse {
    let (vector_id, reason, reason_code, live_detail) = match outcome {
        LaneOutcome::NoEngine => (
            "refused-no-engine-configured",
            RefusalReason::NoEngineConfigured,
            Some("thinking_engine_not_chosen".to_owned()),
            None,
        ),
        LaneOutcome::AttestationNotVerified => (
            "refused-attestation-not-verified",
            RefusalReason::AttestationNotVerified,
            Some("attestation_not_yet_verified".to_owned()),
            None,
        ),
        LaneOutcome::AttestationFailed => (
            "refused-attestation-failed",
            RefusalReason::AttestationFailed,
            Some("attestation_failed".to_owned()),
            None,
        ),
        LaneOutcome::AttestationStale => (
            "refused-attestation-stale",
            RefusalReason::AttestationStale,
            Some("attestation_stale".to_owned()),
            None,
        ),
        LaneOutcome::BundledFailure(failure) => (
            "refused-provider-response-invalid",
            RefusalReason::ProviderResponseInvalid,
            failure.reason_code.clone(),
            Some(LIVE_PROVIDER_FAILURE_DETAIL),
        ),
        LaneOutcome::EndpointFailure(failure) => (
            "refused-provider-response-invalid",
            RefusalReason::ProviderResponseInvalid,
            failure.reason_code.clone(),
            Some(
                failure
                    .detail
                    .as_deref()
                    .unwrap_or(LIVE_PROVIDER_FAILURE_DETAIL),
            ),
        ),
        LaneOutcome::AnthropicFailure(failure) => (
            "refused-provider-response-invalid",
            RefusalReason::ProviderResponseInvalid,
            failure.reason_code.clone(),
            Some(
                failure
                    .detail
                    .as_deref()
                    .unwrap_or(LIVE_PROVIDER_FAILURE_DETAIL),
            ),
        ),
        LaneOutcome::OpenAiFailure(failure) => (
            "refused-provider-response-invalid",
            RefusalReason::ProviderResponseInvalid,
            failure.reason_code.clone(),
            Some(
                failure
                    .detail
                    .as_deref()
                    .unwrap_or(LIVE_PROVIDER_FAILURE_DETAIL),
            ),
        ),
        LaneOutcome::GoogleFailure(failure) => (
            "refused-provider-response-invalid",
            RefusalReason::ProviderResponseInvalid,
            failure.reason_code.clone(),
            Some(
                failure
                    .detail
                    .as_deref()
                    .unwrap_or(LIVE_PROVIDER_FAILURE_DETAIL),
            ),
        ),
        LaneOutcome::ValidationFailure(ValidationFailure::ProviderResponseInvalid {
            raw_response_snippet,
        }) => (
            "refused-provider-response-invalid",
            RefusalReason::ProviderResponseInvalid,
            Some("provider_response_invalid".to_owned()),
            Some(
                raw_response_snippet
                    .as_deref()
                    .unwrap_or(HONEST_PROVIDER_RESPONSE_INVALID_DETAIL),
            ),
        ),
        LaneOutcome::ValidationFailure(ValidationFailure::IncompleteJson { finish_reason }) => (
            "refused-incomplete-json",
            RefusalReason::IncompleteJson,
            matches!(finish_reason, SanitizedFinishReason::MaxTokens)
                .then(|| "incomplete_json_length".to_owned()),
            None,
        ),
        LaneOutcome::ValidationFailure(ValidationFailure::NonResponsiveOutput) => (
            "refused-non-responsive-output",
            RefusalReason::NonResponsiveOutput,
            Some("non_responsive".to_owned()),
            None,
        ),
        LaneOutcome::UnimplementedLane => (
            "refused-provider-response-invalid",
            RefusalReason::ProviderResponseInvalid,
            None,
            Some(HONEST_UNIMPLEMENTED_LANE_DETAIL),
        ),
        LaneOutcome::BundledLocal
        | LaneOutcome::ByoEndpoint(_)
        | LaneOutcome::ConfidentialEndpoint(_)
        | LaneOutcome::Anthropic
        | LaneOutcome::OpenAi
        | LaneOutcome::Google => {
            panic!("bundled local lane must be invoked before refusal mapping")
        }
    };
    let vector = contract()["conformance_vectors"]
        .as_array()
        .expect("generate contract vectors are an array")
        .iter()
        .find(|vector| vector["id"].as_str() == Some(vector_id))
        .expect("generate contract required refusal vector is present");
    let fixture_detail = vector["response"]["detail"]
        .as_str()
        .expect("generate contract refusal detail is a string")
        .to_owned();
    let detail = live_detail.map(str::to_owned).unwrap_or(fixture_detail);
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

/// The two protocol-error reasons, read from the contract rather than spelled
/// as literals.
///
/// ⚠ They live here because the vocabulary guard makes `refusal.rs` the single
/// home for closed-vocabulary members. The session framing used to hold them as
/// literals — that was invisible while the framing sat in the binary crate,
/// where the guard does not reach, and the guard caught them the moment the
/// framing moved into the library.
pub fn protocol_reason(kind: &str) -> &'static str {
    let reasons = solstone_core_generate::contract()["protocol_error"]["reasons"]
        .as_array()
        .expect("the contract carries protocol_error.reasons");
    // ⚠ Positional, because naming them here would be the literal this accessor
    // exists to avoid. The fixture declares exactly two, in this order; a third
    // would silently shift the mapping, so the count is asserted.
    assert_eq!(
        reasons.len(),
        2,
        "the contract's protocol_error.reasons changed shape; this positional \
         mapping no longer holds"
    );
    let wanted = match kind {
        "malformed_request" => 0,
        "internal_failure" => 1,
        other => panic!("unknown protocol error kind {other:?}"),
    };
    reasons[wanted]
        .as_str()
        .expect("protocol error reason is a string")
}

#[cfg(test)]
mod tests {
    use solstone_core_local::GenerateFailure;

    use super::*;
    use crate::{AnthropicFailure, EndpointFailure, GoogleFailure, OpenAiFailure};

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
                LaneOutcome::AttestationFailed,
                "local",
                RefusalReason::AttestationFailed,
                Some("attestation_failed"),
                true,
                true,
            ),
            (
                LaneOutcome::AttestationStale,
                "local",
                RefusalReason::AttestationStale,
                Some("attestation_stale"),
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
            assert_eq!(refusal.detail, LIVE_PROVIDER_FAILURE_DETAIL);
            assert!(
                !refusal
                    .detail
                    .contains("specific failure detail must not escape the fixture fallback")
            );
        }
    }

    #[test]
    fn bundled_exact_admission_public_codes_are_classified() {
        for (reason_code, retryable, blocking) in [
            ("context_budget_exceeded", true, false),
            ("local_endpoint_contract_failed", true, false),
        ] {
            let refusal = refusal_for(
                &LaneOutcome::BundledFailure(Box::new(GenerateFailure {
                    schema: "test".into(),
                    outcome: "failure".into(),
                    reason_code: Some(reason_code.into()),
                    detail: "local implementation detail".into(),
                    inference: None,
                })),
                "local",
                None,
            );
            assert_eq!(
                refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                Some(reason_code)
            );
            assert_eq!(refusal.retryable, retryable);
            assert_eq!(refusal.blocking, blocking);
        }
    }

    #[test]
    fn endpoint_failure_preserves_known_unknown_and_absent_codes() {
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
                &LaneOutcome::EndpointFailure(EndpointFailure {
                    reason_code: reason_code.map(str::to_owned),
                    detail: None,
                }),
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
            assert_eq!(refusal.detail, LIVE_PROVIDER_FAILURE_DETAIL);
        }
    }

    #[test]
    fn anthropic_failure_preserves_known_unknown_and_absent_codes() {
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
                &LaneOutcome::AnthropicFailure(AnthropicFailure {
                    reason_code: reason_code.map(str::to_owned),
                    detail: None,
                }),
                "anthropic",
                None,
            );
            assert_eq!(refusal.reason, RefusalReason::ProviderResponseInvalid);
            assert_eq!(
                refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                expected_wire
            );
            assert_eq!(refusal.retryable, retryable);
            assert_eq!(refusal.blocking, blocking);
            assert_eq!(refusal.detail, LIVE_PROVIDER_FAILURE_DETAIL);
        }
    }

    #[test]
    fn openai_failure_preserves_known_unknown_and_absent_codes() {
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
                &LaneOutcome::OpenAiFailure(OpenAiFailure {
                    reason_code: reason_code.map(str::to_owned),
                    detail: None,
                }),
                "openai",
                None,
            );
            assert_eq!(refusal.reason, RefusalReason::ProviderResponseInvalid);
            assert_eq!(
                refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                expected_wire
            );
            assert_eq!(refusal.retryable, retryable);
            assert_eq!(refusal.blocking, blocking);
            assert_eq!(refusal.detail, LIVE_PROVIDER_FAILURE_DETAIL);
        }
    }

    #[test]
    fn google_failure_preserves_known_unknown_and_absent_codes() {
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
                &LaneOutcome::GoogleFailure(GoogleFailure {
                    reason_code: reason_code.map(str::to_owned),
                    detail: None,
                }),
                "google",
                None,
            );
            assert_eq!(refusal.reason, RefusalReason::ProviderResponseInvalid);
            assert_eq!(
                refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                expected_wire
            );
            assert_eq!(refusal.retryable, retryable);
            assert_eq!(refusal.blocking, blocking);
            assert_eq!(refusal.detail, LIVE_PROVIDER_FAILURE_DETAIL);
        }
    }

    #[test]
    fn validation_failures_use_fixture_reason_classifications() {
        let cases = [
            (
                ValidationFailure::ProviderResponseInvalid {
                    raw_response_snippet: None,
                },
                RefusalReason::ProviderResponseInvalid,
                Some("provider_response_invalid"),
                true,
                false,
            ),
            (
                ValidationFailure::IncompleteJson {
                    finish_reason: SanitizedFinishReason::MaxTokens,
                },
                RefusalReason::IncompleteJson,
                Some("incomplete_json_length"),
                true,
                false,
            ),
            (
                ValidationFailure::IncompleteJson {
                    finish_reason: SanitizedFinishReason::ContentFilter,
                },
                RefusalReason::IncompleteJson,
                None,
                false,
                true,
            ),
            (
                ValidationFailure::NonResponsiveOutput,
                RefusalReason::NonResponsiveOutput,
                Some("non_responsive"),
                false,
                false,
            ),
        ];
        for (failure, reason, reason_code, retryable, blocking) in cases {
            let refusal = refusal_for(&LaneOutcome::ValidationFailure(failure), "local", None);
            assert_eq!(refusal.reason, reason);
            assert_eq!(
                refusal.reason_code.as_ref().map(ReasonCodeValue::as_wire),
                reason_code
            );
            assert_eq!(refusal.retryable, retryable);
            assert_eq!(refusal.blocking, blocking);
        }
    }

    #[test]
    fn blank_provider_response_invalid_uses_honest_detail_not_fixture() {
        let refusal = refusal_for(
            &LaneOutcome::ValidationFailure(ValidationFailure::ProviderResponseInvalid {
                raw_response_snippet: None,
            }),
            "local",
            None,
        );
        assert_eq!(refusal.detail, HONEST_PROVIDER_RESPONSE_INVALID_DETAIL);
        assert!(!refusal.detail.contains("fixture"));
    }

    #[test]
    fn unimplemented_lane_uses_honest_detail_not_fixture() {
        let refusal = refusal_for(&LaneOutcome::UnimplementedLane, "local", None);
        assert_eq!(refusal.detail, HONEST_UNIMPLEMENTED_LANE_DETAIL);
        assert!(!refusal.detail.contains("fixture"));
    }
}
