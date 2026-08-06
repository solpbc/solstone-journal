// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure compilation of journal full-text queries and their filter predicates.

mod atomize;
mod compile;
mod predicate;
mod temporal;

pub use compile::{CompileOutcome, QueryCompilation, compile_query};
pub use predicate::{EffectiveDateConstraint, PredicateInput, QueryPredicate};
pub use temporal::{TemporalExtraction, extract_temporal_references};

#[cfg(test)]
mod tests;
