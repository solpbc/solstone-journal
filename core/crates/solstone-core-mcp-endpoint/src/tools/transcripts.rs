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
    pub(crate) day_from: Option<String>,
    pub(crate) day_to: Option<String>,
    pub(crate) facet_id: Option<String>,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<String>,
    pub(crate) time_from: Option<String>,
    pub(crate) time_to: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) kind: Option<String>,
}

use solstone_core_observe_audio::{MAX_CLIP_DURATION_MS, MIN_CLIP_DURATION_MS};

pub(crate) const DEFAULT_AUDIO_DURATION_MS: u64 = 30_000;
pub(crate) const MAX_AUDIO_START_MS: u64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioEncoding {
    Content,
    Base64,
}

impl AudioEncoding {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Base64 => "base64",
        }
    }
}

pub(crate) struct AudioClipRequest {
    pub(crate) start_ms: u64,
    pub(crate) duration_ms: u64,
    pub(crate) track: Option<String>,
    pub(crate) encoding: AudioEncoding,
}

pub(crate) struct ValidatedGetTranscript {
    pub(crate) offset_ms: Option<u32>,
    pub(crate) display: Option<String>,
    pub(crate) audio: Option<AudioClipRequest>,
    pub(crate) reference: String,
    pub(crate) cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(default)]
    day: Option<String>,
    #[serde(default, alias = "from")]
    day_from: Option<String>,
    #[serde(default, alias = "to")]
    day_to: Option<String>,
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
    #[serde(default, alias = "kind")]
    entry_kind: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetParams {
    offset_ms: Option<u32>,
    display: Option<String>,
    #[serde(default)]
    audio: Option<bool>,
    #[serde(default)]
    audio_start_ms: Option<u64>,
    #[serde(default)]
    audio_duration_ms: Option<u64>,
    #[serde(default)]
    audio_track: Option<String>,
    #[serde(default)]
    audio_encoding: Option<String>,
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
        || !optional_string_within_limit(&params.entry_kind, MAX_FACET_BYTES)
        || params
            .entry_kind
            .as_deref()
            .is_some_and(|kind| kind.trim().is_empty())
    {
        return Err(ToolError::InvalidInput);
    }
    let normalized_day = super::normalize_day(params.day)?;
    let day_from = super::normalize_day(params.day_from)?;
    let day_to = super::normalize_day(params.day_to)?;
    if (normalized_day.is_some() && (day_from.is_some() || day_to.is_some()))
        || matches!((&day_from, &day_to), (Some(from), Some(to)) if to < from)
    {
        return Err(ToolError::ActivityDayArguments);
    }
    if (params.source.is_some() && (normalized_day.is_none() || params.facet.is_some()))
        || (params.entry_kind.is_some() && params.source.is_none())
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
        day_from,
        day_to,
        facet_id: params.facet,
        limit: params.limit,
        cursor: params.cursor,
        time_from: params.time_from,
        time_to: params.time_to,
        source: params.source,
        kind: params.entry_kind,
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
    if (params.offset_ms.is_some() && params.cursor.is_some())
        || params
            .display
            .as_ref()
            .is_some_and(|name| params.offset_ms.is_none() || name.is_empty() || name.len() > 255)
    {
        return Err(ToolError::InvalidInput);
    }
    let audio_options_present = params.audio_start_ms.is_some()
        || params.audio_duration_ms.is_some()
        || params.audio_track.is_some()
        || params.audio_encoding.is_some();
    let audio = match params.audio {
        Some(true) => {
            if params.cursor.is_some() || params.offset_ms.is_some() || params.display.is_some() {
                return Err(ToolError::InvalidInput);
            }
            let start_ms = params.audio_start_ms.unwrap_or(0);
            if start_ms > MAX_AUDIO_START_MS {
                return Err(ToolError::InvalidInput);
            }
            let duration_ms = params
                .audio_duration_ms
                .unwrap_or(DEFAULT_AUDIO_DURATION_MS);
            if !(MIN_CLIP_DURATION_MS..=MAX_CLIP_DURATION_MS).contains(&duration_ms) {
                return Err(ToolError::InvalidInput);
            }
            if params
                .audio_track
                .as_ref()
                .is_some_and(|name| name.is_empty() || name.len() > 255)
            {
                return Err(ToolError::InvalidInput);
            }
            let encoding = match params.audio_encoding.as_deref() {
                None | Some("content") => AudioEncoding::Content,
                Some("base64") => AudioEncoding::Base64,
                Some(_) => return Err(ToolError::InvalidInput),
            };
            Some(AudioClipRequest {
                start_ms,
                duration_ms,
                track: params.audio_track,
                encoding,
            })
        }
        Some(false) | None => {
            if audio_options_present {
                return Err(ToolError::InvalidInput);
            }
            None
        }
    };
    Ok(ValidatedGetTranscript {
        offset_ms: params.offset_ms,
        display: params.display,
        audio,
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
