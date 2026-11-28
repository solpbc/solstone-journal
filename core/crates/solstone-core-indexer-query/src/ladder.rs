// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use jiff::civil::Date;

use crate::atomize::relaxation_terms;
use crate::compile::compile_query;
use crate::execute::{QueryConnection, SqlPlan, plan_from_outcome_with_boundary};
use crate::{ConnectionBoundary, IndexAccessError, QueryCompilation, SearchRequest};

const MIN_PREFIX_CHARS: usize = 4;

const RELAX_STOPWORDS: &[&str] = &[
    "what", "who", "whom", "whose", "when", "where", "why", "how", "which", "did", "do", "does",
    "doing", "done", "is", "are", "was", "were", "am", "be", "been", "being", "the", "a", "an",
    "this", "that", "these", "those", "there", "here", "i", "we", "you", "it", "me", "us", "them",
    "they", "he", "she", "his", "her", "its", "my", "our", "your", "their", "about", "with", "for",
    "of", "to", "in", "on", "at", "by", "from", "as", "into", "over", "under", "up", "down", "out",
    "off", "and", "or", "not", "any", "some", "have", "has", "had", "will", "would", "can",
    "could", "should", "shall", "may", "might", "must", "get", "got", "gets",
];

/// Build the ordered recall-relaxation candidate plans for `compilation`
/// without executing any of them.
pub(super) fn relaxed_candidate_plans(
    compilation: &QueryCompilation,
    request: &SearchRequest,
    reference_date: Date,
    boundary: Option<&ConnectionBoundary>,
    speech_lines_only: bool,
) -> Vec<SqlPlan> {
    let Some(words) = relaxation_terms(&compilation.temporal.remaining_text) else {
        return Vec::new();
    };
    let content: Vec<String> = words
        .iter()
        .filter(|word| !RELAX_STOPWORDS.contains(&word.to_lowercase().as_str()))
        .cloned()
        .collect();
    let mut plans = Vec::new();

    if !content.is_empty() && content != words {
        plans.push(candidate_plan(
            &content.join(" "),
            compilation,
            request,
            reference_date,
            boundary,
            speech_lines_only,
        ));
    }

    if content.len() > 1 {
        plans.push(candidate_plan(
            &content.join(" OR "),
            compilation,
            request,
            reference_date,
            boundary,
            speech_lines_only,
        ));
    }

    // Last word-level rung: let each content word match as a prefix, so
    // `commit` finds `commitment`. Short words are left out because a two- or
    // three-letter prefix matches too much of the index to mean anything.
    let prefixes: Vec<String> = content
        .iter()
        .filter(|word| word.chars().count() >= MIN_PREFIX_CHARS && !word.ends_with('*'))
        .map(|word| format!("{word}*"))
        .collect();
    if !prefixes.is_empty() {
        plans.push(candidate_plan(
            &prefixes.join(" OR "),
            compilation,
            request,
            reference_date,
            boundary,
            speech_lines_only,
        ));
    }

    if content.is_empty() && plan_has_date_constraint(compilation, request) {
        let candidate = compile_query("", reference_date);
        plans.push(plan_from_outcome_with_boundary(
            candidate.outcome,
            &compilation.temporal,
            request,
            boundary,
            speech_lines_only,
        ));
    }

    plans
}

/// Try Python-compatible recall relaxation without bypassing query compilation.
pub(super) fn relaxed_plan(
    connection: &mut QueryConnection,
    compilation: &QueryCompilation,
    request: &SearchRequest,
    reference_date: Date,
    boundary: Option<&ConnectionBoundary>,
    speech_lines_only: bool,
) -> Result<Option<SqlPlan>, IndexAccessError> {
    for plan in relaxed_candidate_plans(
        compilation,
        request,
        reference_date,
        boundary,
        speech_lines_only,
    ) {
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
    reference_date: Date,
    boundary: Option<&ConnectionBoundary>,
    speech_lines_only: bool,
) -> SqlPlan {
    let candidate = compile_query(candidate_text, reference_date);
    plan_from_outcome_with_boundary(
        candidate.outcome,
        &original.temporal,
        request,
        boundary,
        speech_lines_only,
    )
}

fn plan_has_date_constraint(compilation: &QueryCompilation, request: &SearchRequest) -> bool {
    request.day.is_some()
        || request.day_from.is_some()
        || request.day_to.is_some()
        || compilation.temporal.day_from.is_some()
        || compilation.temporal.day_to.is_some()
}
