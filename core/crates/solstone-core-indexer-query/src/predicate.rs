// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{CompileOutcome, TemporalExtraction};

/// Caller-provided filters that are independent of text compilation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PredicateInput {
    pub day: Option<String>,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
    pub facet: Option<String>,
    pub agent: Option<String>,
    pub stream: Option<String>,
    pub time_bucket: Option<String>,
}

/// The date filter selected by Python-compatible precedence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectiveDateConstraint {
    None,
    Exact(String),
    Range {
        day_from: Option<String>,
        day_to: Option<String>,
    },
}

/// Pure assembly of compiled text and caller filters, without SQL or a database.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryPredicate {
    pub outcome: CompileOutcome,
    pub temporal_day_from: Option<String>,
    pub temporal_day_to: Option<String>,
    pub day: Option<String>,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
    pub facet: Option<String>,
    pub agent: Option<String>,
    pub stream: Option<String>,
    pub time_bucket: Option<String>,
    pub effective_date: EffectiveDateConstraint,
}

impl QueryPredicate {
    /// Build a predicate, preferring exact day, then caller range, then temporal range.
    pub fn new(
        outcome: CompileOutcome,
        temporal: &TemporalExtraction,
        input: PredicateInput,
    ) -> Self {
        let effective_date = if let Some(day) = input.day.clone() {
            EffectiveDateConstraint::Exact(day)
        } else if input.day_from.is_some() || input.day_to.is_some() {
            EffectiveDateConstraint::Range {
                day_from: input.day_from.clone(),
                day_to: input.day_to.clone(),
            }
        } else if temporal.day_from.is_some() || temporal.day_to.is_some() {
            EffectiveDateConstraint::Range {
                day_from: temporal.day_from.clone(),
                day_to: temporal.day_to.clone(),
            }
        } else {
            EffectiveDateConstraint::None
        };
        Self {
            outcome,
            temporal_day_from: temporal.day_from.clone(),
            temporal_day_to: temporal.day_to.clone(),
            day: input.day,
            day_from: input.day_from,
            day_to: input.day_to,
            facet: input.facet.map(|value| value.to_lowercase()),
            agent: input.agent.map(|value| value.to_lowercase()),
            stream: input.stream,
            time_bucket: input.time_bucket,
            effective_date,
        }
    }
}
