// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Deserialize;
use serde_json::Value;

use super::{MAX_OPAQUE_REFERENCE_BYTES, ToolError};
use crate::photo_picture::PictureSize;

/// A closed opaque entry reference and optional requested image size.
pub(crate) struct ValidatedFetch {
    pub(crate) reference: String,
    pub(crate) size: Option<PictureSize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchParams {
    reference: String,
    #[serde(default)]
    size: Option<PictureSize>,
}

pub(crate) fn parse_size(value: Option<&Value>) -> Result<Option<PictureSize>, ToolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value::<PictureSize>(value.clone())
        .map(Some)
        .map_err(|_| ToolError::InvalidInput)
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
        size: params.size,
    })
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn fetch_accepts_optional_picture_size_and_rejects_invalid_values() {
        let plain = validate(Some(&json!({"reference": "ref-1"}))).unwrap();
        assert_eq!(plain.size, None);

        let null_size = validate(Some(&json!({"reference": "ref-1", "size": null}))).unwrap();
        assert_eq!(null_size.size, None);
        assert_eq!(parse_size(Some(&Value::Null)).unwrap(), None);

        let full = validate(Some(&json!({"reference": "ref-1", "size": "full"}))).unwrap();
        assert_eq!(full.size, Some(PictureSize::Full));

        let thumb = validate(Some(&json!({"reference": "ref-1", "size": "thumb"}))).unwrap();
        assert_eq!(thumb.size, Some(PictureSize::Thumb));

        for bad in [
            json!({"reference": "ref-1", "size": "huge"}),
            json!({"reference": "ref-1", "size": 320}),
            json!({"reference": "ref-1", "size": true}),
            json!({"reference": "ref-1", "extra": true}),
        ] {
            assert!(validate(Some(&bad)).is_err(), "{bad}");
        }
    }
}
