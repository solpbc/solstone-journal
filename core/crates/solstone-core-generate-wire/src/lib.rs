// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure request, lane, response, and usage-log logic for the generate wire.

mod anthropic;
mod bundled;
pub mod chatgpt;
mod confidential;
mod endpoint;
mod google;
mod lane;
mod openai;
pub mod overrides;
mod pool;
mod refusal;
mod request;
mod responsiveness;
mod schema_prep;
mod schema_validation;
pub mod session;
mod thinking;
mod token_log;
mod validation;

pub use anthropic::{
    AnthropicFailure, AnthropicGenerated, AnthropicResult, AnthropicTransport,
    UreqAnthropicTransport, anthropic_generate,
};
pub use bundled::{
    BundledError, LOCAL_MODEL_ID, bundled_generate, bundled_generate_with_authority, bundled_input,
};
pub use chatgpt::{ChatGptFailure, ChatGptResult, chatgpt_generate};
#[cfg(not(windows))]
pub use confidential::confidential_generate_in_package;
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use confidential::test_support;
pub use confidential::{ConfidentialResult, confidential_generate};
pub use endpoint::{
    ENDPOINT_SERVED_WINDOW_CACHE_TTL, EndpointFailure, EndpointGenerated, EndpointResult,
    EndpointRuntime, EndpointTransport, EndpointTransportError, OverflowDecision,
    UreqEndpointTransport, endpoint_generate, endpoint_overflow_decision,
};
pub use google::{
    GoogleFailure, GoogleGenerated, GoogleResult, GoogleTransport, UreqGoogleTransport,
    google_generate,
};
pub use lane::{LaneOutcome, resolve_lane};
pub use openai::{
    OpenAiFailure, OpenAiGenerated, OpenAiResult, OpenAiTransport, UreqOpenAiTransport,
    openai_generate,
};
#[cfg(feature = "test-hooks")]
pub use pool::{ConfidentialChannelPool, PoolClock, SystemPoolClock};
pub use refusal::refusal_for;
pub use request::parse_one_shot_request;
pub use responsiveness::{
    NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS, ResponsivenessSignal, ResponsivenessVerdict,
    classify_output_responsiveness,
};
pub use schema_prep::{anthropic_schema_violations, prepare_provider_schema};
pub use schema_validation::{SchemaValidationResult, validate_schema_with_annotations};
pub use session::{SessionConfig, SessionHost, SessionOutcome, run_session};
pub use thinking::{BYO_THINKING_BUDGET_KEY, BYO_THINKING_BUDGETS, Thinking, byo_thinking};
pub use token_log::{GenerateUsageMetadata, record_generate_usage, record_usage, usage_for_log};
pub use validation::{
    ProviderResultAssessment, ProviderResultView, SanitizedFinishReason, ValidationFailure,
    assess_provider_result,
};
