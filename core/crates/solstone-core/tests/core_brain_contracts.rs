// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[path = "support/await_outcome.rs"]
mod await_outcome;
#[allow(dead_code)]
#[path = "support/race_classification.rs"]
mod race_classification;

#[path = "await_outcome.rs"]
mod await_outcome_contract;
#[path = "brain_fingerprint.rs"]
mod brain_fingerprint;
#[path = "brain_inspect.rs"]
mod brain_inspect;
#[path = "brain_prerequisite_renewal_session.rs"]
mod brain_prerequisite_renewal_session;
#[path = "brain_refresh_session.rs"]
mod brain_refresh_session;
#[path = "brain_runtime_failure.rs"]
mod brain_runtime_failure;
#[path = "cogitate_session.rs"]
mod cogitate_session;
#[path = "generate_session.rs"]
mod generate_session;
#[path = "generate_wire.rs"]
mod generate_wire;
#[path = "local_generate.rs"]
mod local_generate;
#[path = "race_classifier_routing.rs"]
mod race_classifier_routing;
#[path = "talent_contract.rs"]
mod talent_contract;
#[path = "warm.rs"]
mod warm_contract;
