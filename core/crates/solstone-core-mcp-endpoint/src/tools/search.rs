// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Deserialize;
use serde_json::Value;
use solstone_core_indexer_query::AdmittedCategory;

use super::{
    MAX_DAY_BYTES, MAX_FACET_BYTES, MAX_OPAQUE_REFERENCE_BYTES, ToolError,
    optional_string_within_limit,
};

pub(crate) const MAX_QUERY_BYTES: usize = 4_096;
pub(crate) const MAX_LIMIT: usize = 100;

/// Closed, validated agent search arguments.
pub(crate) struct ValidatedSearch {
    pub(crate) query: String,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<String>,
    pub(crate) day: Option<String>,
    pub(crate) day_from: Option<String>,
    pub(crate) day_to: Option<String>,
    pub(crate) category: Option<AdmittedCategory>,
    pub(crate) facet_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchParams {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    day: Option<String>,
    #[serde(default)]
    day_from: Option<String>,
    #[serde(default)]
    day_to: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    facet: Option<String>,
}

const fn default_limit() -> usize {
    10
}

pub(crate) fn validate(params: Option<&Value>) -> Result<ValidatedSearch, ToolError> {
    let params = params.cloned().ok_or(ToolError::InvalidInput)?;
    let params =
        serde_json::from_value::<SearchParams>(params).map_err(|_| ToolError::InvalidInput)?;
    if params.query.is_empty()
        || params.query.len() > MAX_QUERY_BYTES
        || !(1..=MAX_LIMIT).contains(&params.limit)
        || params
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_OPAQUE_REFERENCE_BYTES)
        || !optional_string_within_limit(&params.day, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.day_from, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.day_to, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.facet, MAX_FACET_BYTES)
    {
        return Err(ToolError::InvalidInput);
    }
    let category = match params.category.as_deref() {
        None => None,
        Some("transcripts") => Some(AdmittedCategory::Transcripts),
        Some("entities") => Some(AdmittedCategory::Entities),
        Some("facets") => Some(AdmittedCategory::Facets),
        Some(_) => return Err(ToolError::InvalidInput),
    };
    Ok(ValidatedSearch {
        query: params.query,
        limit: params.limit,
        cursor: params.cursor,
        day: params.day,
        day_from: params.day_from,
        day_to: params.day_to,
        category,
        facet_id: params.facet,
    })
}
