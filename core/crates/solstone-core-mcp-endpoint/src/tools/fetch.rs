// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Deserialize;
use serde_json::Value;

use super::{MAX_OPAQUE_REFERENCE_BYTES, ToolError};

/// A closed opaque entry reference.
pub(crate) struct ValidatedFetch {
    pub(crate) reference: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchParams {
    reference: String,
}

pub(crate) fn validate(params: Option<&Value>) -> Result<ValidatedFetch, ToolError> {
    let params = params.cloned().ok_or(ToolError::InvalidInput)?;
    let params =
        serde_json::from_value::<FetchParams>(params).map_err(|_| ToolError::InvalidInput)?;
    if params.reference.is_empty() || params.reference.len() > MAX_OPAQUE_REFERENCE_BYTES {
        return Err(ToolError::InvalidInput);
    }
    Ok(ValidatedFetch {
        reference: params.reference,
    })
}
