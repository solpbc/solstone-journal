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
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use endpoint::test_support as endpoint_test_support;
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
