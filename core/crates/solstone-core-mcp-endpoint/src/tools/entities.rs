// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Deserialize;
use serde_json::Value;

use super::{
    MAX_FACET_BYTES, MAX_OPAQUE_REFERENCE_BYTES, ToolError, optional_string_within_limit,
    search::MAX_LIMIT,
};

pub(crate) struct ValidatedListEntities {
    pub(crate) facet_id: Option<String>,
    pub(crate) limit: usize,
}

pub(crate) struct ValidatedGetEntity {
    pub(crate) reference: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(default)]
    facet: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetParams {
    reference: String,
}

const fn default_limit() -> usize {
    100
}

pub(crate) fn validate_list(params: Option<&Value>) -> Result<ValidatedListEntities, ToolError> {
    let params = params.cloned().unwrap_or_else(|| serde_json::json!({}));
    let params =
        serde_json::from_value::<ListParams>(params).map_err(|_| ToolError::InvalidInput)?;
    if !(1..=MAX_LIMIT).contains(&params.limit) {
        return Err(ToolError::InvalidInput);
    }
    if !optional_string_within_limit(&params.facet, MAX_FACET_BYTES) {
        return Err(ToolError::InvalidInput);
    }
    Ok(ValidatedListEntities {
        facet_id: params.facet,
        limit: params.limit,
    })
}

pub(crate) fn validate_get(params: Option<&Value>) -> Result<ValidatedGetEntity, ToolError> {
    let params = params.cloned().ok_or(ToolError::InvalidInput)?;
    let params =
        serde_json::from_value::<GetParams>(params).map_err(|_| ToolError::InvalidInput)?;
    if params.reference.is_empty() || params.reference.len() > MAX_OPAQUE_REFERENCE_BYTES {
        return Err(ToolError::InvalidInput);
    }
    Ok(ValidatedGetEntity {
        reference: params.reference,
    })
}
