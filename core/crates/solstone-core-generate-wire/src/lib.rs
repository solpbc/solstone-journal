// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure request, lane, response, and usage-log logic for the generate wire.

mod bundled;
mod endpoint;
mod lane;
mod refusal;
mod request;
mod token_log;

pub use bundled::{BundledError, LOCAL_MODEL_ID, bundled_generate, bundled_input};
pub use endpoint::{
    ENDPOINT_SERVED_WINDOW_CACHE_TTL, EndpointFailure, EndpointGenerated, EndpointResult,
    EndpointRuntime, EndpointTransport, EndpointTransportError, OverflowDecision,
    UreqEndpointTransport, endpoint_generate, endpoint_overflow_decision,
};
pub use lane::{LaneOutcome, resolve_lane};
pub use refusal::refusal_for;
pub use request::parse_one_shot_request;
pub use token_log::record_generate_usage;

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
        for value in contract()["reason_codes"]
            .as_array()
            .expect("fixture reason codes are an array")
        {
            members.push(
                value["code"]
                    .as_str()
                    .expect("fixture reason code is a string"),
            );
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

        let sources = [
            include_str!("bundled.rs"),
            include_str!("lane.rs"),
            include_str!("request.rs"),
            include_str!("token_log.rs"),
            include_str!("lib.rs"),
        ];
        for member in members {
            let quoted_member = format!("\"{member}\"");
            assert!(
                sources
                    .iter()
                    .all(|source| !source.contains(&quoted_member)),
                "closed generate vocabulary member {member:?} must stay in refusal.rs"
            );
        }
    }
}
