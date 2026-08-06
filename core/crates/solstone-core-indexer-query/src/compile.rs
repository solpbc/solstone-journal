// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;

use crate::atomize::compile_expression;
use crate::temporal::{TemporalExtraction, extract_temporal_references};

/// The text-side result of compiling one journal search query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompileOutcome {
    NoInput,
    FiltersOnly,
    Compiled { expression: String },
    NoTokenizableTerm,
}

/// The complete, deterministic output of the query compiler.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryCompilation {
    pub outcome: CompileOutcome,
    pub temporal: TemporalExtraction,
}

/// Compile a query against an explicit date used to resolve relative language.
pub fn compile_query(query: &str, reference_date: NaiveDate) -> QueryCompilation {
    let temporal = extract_temporal_references(query, reference_date);
    let outcome = if query.trim().is_empty() {
        CompileOutcome::NoInput
    } else if temporal.remaining_text.trim().is_empty()
        && (temporal.day_from.is_some() || temporal.day_to.is_some())
    {
        CompileOutcome::FiltersOnly
    } else if let Some(expression) = compile_expression(&temporal.remaining_text) {
        CompileOutcome::Compiled { expression }
    } else {
        CompileOutcome::NoTokenizableTerm
    };
    QueryCompilation { outcome, temporal }
}
