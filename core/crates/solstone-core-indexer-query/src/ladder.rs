// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;

use crate::atomize::relaxation_terms;
use crate::compile::compile_query;
use crate::execute::{QueryConnection, SqlPlan, plan_from_outcome};
use crate::{IndexAccessError, QueryCompilation, SearchRequest};

const RELAX_STOPWORDS: &[&str] = &[
    "what", "who", "whom", "whose", "when", "where", "why", "how", "which", "did", "do", "does",
    "doing", "done", "is", "are", "was", "were", "am", "be", "been", "being", "the", "a", "an",
    "this", "that", "these", "those", "there", "here", "i", "we", "you", "it", "me", "us", "them",
    "they", "he", "she", "his", "her", "its", "my", "our", "your", "their", "about", "with", "for",
    "of", "to", "in", "on", "at", "by", "from", "as", "into", "over", "under", "up", "down", "out",
    "off", "and", "or", "not", "any", "some", "have", "has", "had", "will", "would", "can",
    "could", "should", "shall", "may", "might", "must", "get", "got", "gets",
];

/// Try Python-compatible recall relaxation without bypassing W2 compilation.
pub(super) fn relaxed_plan(
    connection: &mut QueryConnection,
    compilation: &QueryCompilation,
    request: &SearchRequest,
    reference_date: NaiveDate,
) -> Result<Option<SqlPlan>, IndexAccessError> {
    let Some(words) = relaxation_terms(&compilation.temporal.remaining_text) else {
        return Ok(None);
    };
    let content: Vec<String> = words
        .iter()
        .filter(|word| !RELAX_STOPWORDS.contains(&word.to_lowercase().as_str()))
        .cloned()
        .collect();

    if !content.is_empty() && content != words {
        let plan = candidate_plan(&content.join(" "), compilation, request, reference_date);
        if connection.has_rows(&plan)? {
            return Ok(Some(plan));
        }
    }

    if content.len() > 1 {
        let plan = candidate_plan(&content.join(" OR "), compilation, request, reference_date);
        if connection.has_rows(&plan)? {
            return Ok(Some(plan));
        }
    }

    if content.is_empty() && plan_has_date_constraint(compilation, request) {
        let candidate = compile_query("", reference_date);
        let plan = plan_from_outcome(candidate.outcome, &compilation.temporal, request);
        if connection.has_rows(&plan)? {
            return Ok(Some(plan));
        }
    }

    Ok(None)
}

fn candidate_plan(
    candidate_text: &str,
    original: &QueryCompilation,
    request: &SearchRequest,
    reference_date: NaiveDate,
) -> SqlPlan {
    let candidate = compile_query(candidate_text, reference_date);
    plan_from_outcome(candidate.outcome, &original.temporal, request)
}

fn plan_has_date_constraint(compilation: &QueryCompilation, request: &SearchRequest) -> bool {
    request.day.is_some()
        || request.day_from.is_some()
        || request.day_to.is_some()
        || compilation.temporal.day_from.is_some()
        || compilation.temporal.day_to.is_some()
}
