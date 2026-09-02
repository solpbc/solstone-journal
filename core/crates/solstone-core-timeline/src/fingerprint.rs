// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, to_value};
use solstone_core_brain::{CanonicalInput, canonical_fingerprint_preserving_array_order};

use crate::{
    CURRENT_SCHEMA_VERSION, CurationRequestV1, SegmentBindingV1, TimelineEntryV1, TimelineError,
};

pub fn curation_input_digest(
    candidates: &[TimelineEntryV1],
    request: &CurationRequestV1,
) -> Result<String, TimelineError> {
    let mut envelope = Map::new();
    envelope.insert(
        "schema_version".to_owned(),
        Value::from(CURRENT_SCHEMA_VERSION),
    );
    envelope.insert("candidates".to_owned(), to_value(candidates)?);
    envelope.insert("request".to_owned(), to_value(request)?);
    Ok(canonical_fingerprint_preserving_array_order(
        &CanonicalInput::Json(Value::Object(envelope)),
    )?)
}

pub fn segment_input_digest(
    binding: &SegmentBindingV1,
    activity_source: &str,
) -> Result<String, TimelineError> {
    let mut envelope = Map::new();
    envelope.insert(
        "schema_version".to_owned(),
        Value::from(CURRENT_SCHEMA_VERSION),
    );
    envelope.insert("binding".to_owned(), to_value(binding)?);
    envelope.insert(
        "activity_source".to_owned(),
        Value::String(activity_source.to_owned()),
    );
    Ok(canonical_fingerprint_preserving_array_order(
        &CanonicalInput::Json(Value::Object(envelope)),
    )?)
}

pub fn continuation_input_digest(
    binding: &SegmentBindingV1,
    predecessor_segment_key: &str,
) -> Result<String, TimelineError> {
    let mut envelope = Map::new();
    envelope.insert(
        "schema_version".to_owned(),
        Value::from(CURRENT_SCHEMA_VERSION),
    );
    envelope.insert("binding".to_owned(), to_value(binding)?);
    envelope.insert(
        "predecessor_segment_key".to_owned(),
        Value::String(predecessor_segment_key.to_owned()),
    );
    Ok(canonical_fingerprint_preserving_array_order(
        &CanonicalInput::Json(Value::Object(envelope)),
    )?)
}

pub fn master_source_digest(
    day_sources: &[(String, String)],
    request: &CurationRequestV1,
) -> Result<String, TimelineError> {
    let mut envelope = Map::new();
    envelope.insert(
        "schema_version".to_owned(),
        Value::from(CURRENT_SCHEMA_VERSION),
    );
    envelope.insert("day_sources".to_owned(), to_value(day_sources)?);
    envelope.insert("request".to_owned(), to_value(request)?);
    Ok(canonical_fingerprint_preserving_array_order(
        &CanonicalInput::Json(Value::Object(envelope)),
    )?)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{CurationContentPartV1, SegmentBindingV1};

    fn request() -> CurationRequestV1 {
        CurationRequestV1 {
            id: None,
            context: "timeline.scratch.rollup".to_owned(),
            contents: vec![CurationContentPartV1::Text {
                text: "Candidates".to_owned(),
            }],
            system_instruction: Some("Curate".to_owned()),
            temperature: 0.3,
            max_output_tokens: 2048,
            thinking_budget: None,
            timeout_s: Some(60.0),
            json_output: true,
            json_schema: Some(json!({"type": "object"})),
            enforce_responsiveness: false,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        }
    }

    fn entry(title: &str) -> TimelineEntryV1 {
        TimelineEntryV1 {
            title: title.to_owned(),
            description: title.to_owned(),
            origin: format!("20260401/{title}"),
            binding: SegmentBindingV1 {
                day: "20260401".to_owned(),
                stream: "_default".to_owned(),
                segment: format!("080000_{}", title.len()),
            },
        }
    }

    #[test]
    fn digest_changes_when_candidate_order_changes() {
        let first = vec![entry("first"), entry("second")];
        let second = vec![entry("second"), entry("first")];

        assert_ne!(
            curation_input_digest(&first, &request()).unwrap(),
            curation_input_digest(&second, &request()).unwrap()
        );
    }

    #[test]
    fn segment_digest_changes_with_activity_source() {
        let binding = SegmentBindingV1 {
            day: "20260401".to_owned(),
            stream: "_default".to_owned(),
            segment: "080000_300".to_owned(),
        };

        assert_ne!(
            segment_input_digest(&binding, "First activity.").unwrap(),
            segment_input_digest(&binding, "Changed activity.").unwrap()
        );
    }

    #[test]
    fn continuation_digest_changes_with_predecessor_reference() {
        let binding = SegmentBindingV1 {
            day: "20260401".to_owned(),
            stream: "_default".to_owned(),
            segment: "080000_300".to_owned(),
        };

        assert_ne!(
            continuation_input_digest(&binding, "070000_300").unwrap(),
            continuation_input_digest(&binding, "071000_300").unwrap()
        );
    }

    #[test]
    fn master_digest_changes_with_day_source_order() {
        let first = vec![
            ("20260401".to_owned(), "one".to_owned()),
            ("20260402".to_owned(), "two".to_owned()),
        ];
        let second = vec![first[1].clone(), first[0].clone()];

        assert_ne!(
            master_source_digest(&first, &request()).unwrap(),
            master_source_digest(&second, &request()).unwrap()
        );
    }
}
