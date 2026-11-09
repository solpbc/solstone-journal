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
    pub(crate) cursor: Option<String>,
    pub(crate) time_from: Option<String>,
    pub(crate) time_to: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) kind: Option<String>,
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
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    time_from: Option<String>,
    #[serde(default)]
    time_to: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    kind: Option<String>,
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

fn validate_time(time_str: &str) -> Result<(), ToolError> {
    if time_str.len() != 5 {
        return Err(ToolError::InvalidInput);
    }
    let bytes = time_str.as_bytes();
    if !bytes[0].is_ascii_digit()
        || !bytes[1].is_ascii_digit()
        || bytes[2] != b':'
        || !bytes[3].is_ascii_digit()
        || !bytes[4].is_ascii_digit()
    {
        return Err(ToolError::InvalidInput);
    }
    let hour = time_str[0..2]
        .parse::<u32>()
        .map_err(|_| ToolError::InvalidInput)?;
    let minute = time_str[3..5]
        .parse::<u32>()
        .map_err(|_| ToolError::InvalidInput)?;
    if hour > 23 || minute > 59 {
        return Err(ToolError::InvalidInput);
    }
    Ok(())
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
    if !optional_string_within_limit(&params.source, MAX_FACET_BYTES)
        || !optional_string_within_limit(&params.kind, MAX_FACET_BYTES)
        || params
            .kind
            .as_deref()
            .is_some_and(|kind| kind.trim().is_empty())
    {
        return Err(ToolError::InvalidInput);
    }
    let normalized_day = super::normalize_day(params.day)?;
    if (params.source.is_some() && (normalized_day.is_none() || params.facet.is_some()))
        || (params.kind.is_some() && params.source.is_none())
    {
        return Err(ToolError::SourceArguments);
    }
    if (params.time_from.is_some() || params.time_to.is_some()) && normalized_day.is_none() {
        return Err(ToolError::TranscriptTimeArguments);
    }
    if let Some(ref tf) = params.time_from {
        validate_time(tf)?;
    }
    if let Some(ref tt) = params.time_to {
        validate_time(tt)?;
    }
    if matches!((&params.time_from, &params.time_to), (Some(tf), Some(tt)) if tf > tt) {
        return Err(ToolError::TranscriptTimeArguments);
    }
    if params
        .cursor
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > MAX_OPAQUE_REFERENCE_BYTES)
    {
        return Err(ToolError::InvalidInput);
    }
    Ok(ValidatedListTranscripts {
        day: normalized_day,
        facet_id: params.facet,
        limit: params.limit,
        cursor: params.cursor,
        time_from: params.time_from,
        time_to: params.time_to,
        source: params.source,
        kind: params.kind,
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
