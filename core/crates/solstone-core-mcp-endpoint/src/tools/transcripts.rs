// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Deserialize;
use serde_json::Value;

use super::{
    MAX_DAY_BYTES, MAX_FACET_BYTES, MAX_OPAQUE_REFERENCE_BYTES, ToolError,
    optional_string_within_limit, search::MAX_LIMIT,
};

pub(crate) struct ValidatedListTranscripts {
    pub(crate) day: Option<String>,
    pub(crate) facet_id: Option<String>,
    pub(crate) limit: usize,
}

pub(crate) struct ValidatedGetTranscript {
    pub(crate) reference: String,
    pub(crate) cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(default)]
    day: Option<String>,
    #[serde(default)]
    facet: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetParams {
    reference: String,
    #[serde(default)]
    cursor: Option<String>,
}

const fn default_limit() -> usize {
    10
}

pub(crate) fn validate_list(params: Option<&Value>) -> Result<ValidatedListTranscripts, ToolError> {
    let params = params.cloned().unwrap_or_else(|| serde_json::json!({}));
    let params =
        serde_json::from_value::<ListParams>(params).map_err(|_| ToolError::InvalidInput)?;
    validate_limit(params.limit)?;
    if !optional_string_within_limit(&params.day, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.facet, MAX_FACET_BYTES)
    {
        return Err(ToolError::InvalidInput);
    }
    Ok(ValidatedListTranscripts {
        day: params.day,
        facet_id: params.facet,
        limit: params.limit,
    })
}

pub(crate) fn validate_get(params: Option<&Value>) -> Result<ValidatedGetTranscript, ToolError> {
    let params = params.cloned().ok_or(ToolError::InvalidInput)?;
    let params =
        serde_json::from_value::<GetParams>(params).map_err(|_| ToolError::InvalidInput)?;
    validate_limit(1)?;
    if params
        .cursor
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > MAX_OPAQUE_REFERENCE_BYTES)
    {
        return Err(ToolError::InvalidInput);
    }
    if params.reference.is_empty() || params.reference.len() > MAX_OPAQUE_REFERENCE_BYTES {
        return Err(ToolError::InvalidInput);
    }
    Ok(ValidatedGetTranscript {
        reference: params.reference,
        cursor: params.cursor,
    })
}

fn validate_limit(limit: usize) -> Result<(), ToolError> {
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ToolError::InvalidInput);
    }
    Ok(())
}
