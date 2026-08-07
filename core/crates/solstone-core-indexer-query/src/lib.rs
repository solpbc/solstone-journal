// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Compilation and read-only execution of journal full-text queries.

mod atomize;
mod collapse;
mod compile;
mod execute;
mod ladder;
mod predicate;
mod temporal;
mod types;

pub use compile::{CompileOutcome, QueryCompilation, compile_query};
pub use execute::{agents, coverage, search, search_counts};
pub use predicate::{EffectiveDateConstraint, PredicateInput, QueryPredicate};
pub use temporal::{TemporalExtraction, extract_temporal_references};
pub use types::{
    CountsResponse, CoverageResponse, CoverageState, IndexAccessError, Order, RequestError,
    SearchHit, SearchMetadata, SearchRequest, SearchResponse,
};

#[cfg(test)]
mod execute_tests;
#[cfg(test)]
mod tests;
