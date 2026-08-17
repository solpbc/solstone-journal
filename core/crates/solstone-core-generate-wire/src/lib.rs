// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure request, lane, response, and usage-log logic for the generate wire.

mod anthropic;
mod bundled;
mod confidential;
mod converse;
mod endpoint;
mod google;
mod lane;
mod openai;
pub mod overrides;
mod refusal;
mod request;
mod responsiveness;
mod schema_prep;
mod schema_validation;
pub mod session;
mod token_budget;
mod token_log;
mod validation;

pub use anthropic::{
    AnthropicConverseFailure, AnthropicConverseResult, AnthropicFailure, AnthropicGenerated,
    AnthropicResult, AnthropicTransport, AnthropicTurn, UreqAnthropicTransport, anthropic_converse,
    anthropic_generate,
};
pub use bundled::{
    BundledError, LOCAL_MODEL_ID, bundled_converse, bundled_generate, bundled_input,
};
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use confidential::test_support;
pub use confidential::{ConfidentialResult, confidential_converse, confidential_generate};
pub use converse::{
    ConverseFailure, ConverseMessage, ConverseToolCall, ConverseToolSpec, ConverseTurn,
};
pub use endpoint::{
    ENDPOINT_SERVED_WINDOW_CACHE_TTL, EndpointConverseResult, EndpointFailure, EndpointGenerated,
    EndpointResult, EndpointRuntime, EndpointTransport, EndpointTransportError, OverflowDecision,
    UreqEndpointTransport, endpoint_converse, endpoint_generate, endpoint_overflow_decision,
};
pub use google::{
    GoogleConverseFailure, GoogleConverseResult, GoogleFailure, GoogleGenerated, GoogleResult,
    GoogleTransport, GoogleTurn, UreqGoogleTransport, google_converse, google_generate,
};
pub use lane::{LaneOutcome, resolve_lane};
pub use openai::{
    OpenAiConverseFailure, OpenAiConverseResult, OpenAiFailure, OpenAiGenerated, OpenAiResult,
    OpenAiTransport, OpenAiTurn, UreqOpenAiTransport, openai_converse, openai_generate,
};
pub use refusal::refusal_for;
pub use request::parse_one_shot_request;
pub use responsiveness::{
    NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS, ResponsivenessSignal, ResponsivenessVerdict,
    classify_output_responsiveness,
};
pub use schema_prep::{prepare_provider_schema, unsupported_keyword_hits};
pub use schema_validation::{SchemaValidationResult, validate_schema_with_annotations};
pub use session::{SessionConfig, SessionHost, SessionOutcome, run_session};
pub use token_budget::generate_token_budget;
pub use token_log::{GenerateUsageMetadata, record_generate_usage, record_usage, usage_for_log};
pub use validation::{
    ProviderResultAssessment, ProviderResultView, SanitizedFinishReason, ValidationFailure,
    assess_provider_result,
};

#[cfg(test)]
mod vocabulary_tests {
    use solstone_core_generate::contract;

    #[test]
    fn closed_vocabulary_literals_stay_in_refusal_mapping() {
        let mut members = Vec::new();
        for value in contract()["refusal_reasons"]
            .as_array()
            .expect("fixture refusal reasons are an array")
        {
            members.push(value.as_str().expect("fixture refusal reason is a string"));
        }
        for value in contract()["conformance_vectors"]
            .as_array()
            .expect("fixture vectors are an array")
        {
            members.push(value["id"].as_str().expect("fixture vector id is a string"));
        }
        members.push(
            contract()
                .as_object()
                .expect("fixture is an object")
                .iter()
                .find(|(_, value)| value.get("refusal_reason").is_some())
                .map(|(key, _)| key.as_str())
                .expect("fixture unknown-member entry is present"),
        );

        // Every module in the crate, checked against lib.rs's own `mod`
        // declarations below so a new module cannot escape the scan by being
        // forgotten. The list was frozen at five files for six waves while the
        // crate grew to thirteen modules, and both provider arms were outside
        // it the whole time.
        let sources: &[(&str, &str)] = &[
            ("anthropic", include_str!("anthropic.rs")),
            ("bundled", include_str!("bundled.rs")),
            ("confidential", include_str!("confidential.rs")),
            ("converse", include_str!("converse.rs")),
            ("endpoint", include_str!("endpoint.rs")),
            ("google", include_str!("google.rs")),
            ("lane", include_str!("lane.rs")),
            ("lib", include_str!("lib.rs")),
            ("openai", include_str!("openai.rs")),
            ("overrides", include_str!("overrides.rs")),
            ("refusal", include_str!("refusal.rs")),
            ("request", include_str!("request.rs")),
            ("responsiveness", include_str!("responsiveness.rs")),
            ("schema_prep", include_str!("schema_prep.rs")),
            ("schema_validation", include_str!("schema_validation.rs")),
            ("session", include_str!("session.rs")),
            ("token_budget", include_str!("token_budget.rs")),
            ("token_log", include_str!("token_log.rs")),
            ("validation", include_str!("validation.rs")),
        ];

        // Self-check: the scan covers every module this crate declares.
        for line in include_str!("lib.rs").lines() {
            let line = line.trim();
            let Some(rest) = line
                .strip_prefix("pub mod ")
                .or_else(|| line.strip_prefix("mod "))
            else {
                continue;
            };
            let Some(name) = rest.strip_suffix(';') else {
                continue;
            };
            assert!(
                sources.iter().any(|(module, _)| *module == name),
                "module {name:?} is declared but not scanned by the vocabulary guard"
            );
        }

        // Reason codes are deliberately NOT in this set. Each provider arm
        // classifies its own failures - the reference's classifier is built
        // around Python exception types and does not port - so an arm naming
        // the code it classifies to is correct. What must not escape refusal.rs
        // is the boundary's own refusal vocabulary: the reason strings, the
        // conformance-vector ids, and the unknown-member key.
        for member in members {
            let quoted_member = format!("\"{member}\"");
            for (module, source) in sources {
                if *module == "refusal" {
                    continue;
                }
                assert!(
                    !source.contains(&quoted_member),
                    "closed generate vocabulary member {member:?} must stay in \
                     refusal.rs, found in {module}.rs"
                );
            }
        }
    }
}
