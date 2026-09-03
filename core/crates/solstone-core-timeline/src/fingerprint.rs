// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, to_value};
use solstone_core_brain::{CanonicalInput, canonical_fingerprint_preserving_array_order};

use crate::{
    CURRENT_SCHEMA_VERSION, CurationRequestV1, SegmentBindingV1, SegmentSourceV1, TimelineEntryV1,
    TimelineError,
};

/// One curation request performed while producing a rollup artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurationJobV1 {
    pub scope: String,
    pub candidates: Vec<TimelineEntryV1>,
    pub request: CurationRequestV1,
}

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
    fingerprint_envelope(envelope)
}

/// Fingerprint the complete ordered set of curation jobs that make up a rollup.
pub fn curation_jobs_digest(jobs: &[CurationJobV1]) -> Result<String, TimelineError> {
    let mut envelope = base_envelope();
    envelope.insert("jobs".to_owned(), to_value(jobs)?);
    fingerprint_envelope(envelope)
}

pub fn segment_input_digest(
    binding: &SegmentBindingV1,
    source: &SegmentSourceV1,
) -> Result<String, TimelineError> {
    let mut envelope = base_envelope();
    envelope.insert("binding".to_owned(), to_value(binding)?);
    envelope.insert("source".to_owned(), to_value(source)?);
    fingerprint_envelope(envelope)
}

pub fn master_source_digest(
    day_sources: &[(String, String)],
    jobs: &[CurationJobV1],
) -> Result<String, TimelineError> {
    let mut envelope = base_envelope();
    envelope.insert("day_sources".to_owned(), to_value(day_sources)?);
    envelope.insert("jobs".to_owned(), to_value(jobs)?);
    fingerprint_envelope(envelope)
}

fn base_envelope() -> Map<String, Value> {
    let mut envelope = Map::new();
    envelope.insert(
        "schema_version".to_owned(),
        Value::from(CURRENT_SCHEMA_VERSION),
    );
    envelope
}

fn fingerprint_envelope(envelope: Map<String, Value>) -> Result<String, TimelineError> {
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

        let source = |sha256: &str| SegmentSourceV1::GeneratedActivity {
            schema_version: crate::SEGMENT_SOURCE_SCHEMA_VERSION,
            relative_path: "chronicle/20260401/080000_300/talents/activity.md".to_owned(),
            sha256: sha256.repeat(64),
        };

        assert_ne!(
            segment_input_digest(&binding, &source("a")).unwrap(),
            segment_input_digest(&binding, &source("b")).unwrap()
        );
    }

    #[test]
    fn continuation_digest_changes_with_predecessor_reference() {
        let binding = SegmentBindingV1 {
            day: "20260401".to_owned(),
            stream: "_default".to_owned(),
            segment: "080000_300".to_owned(),
        };

        let source = |predecessor: &str| SegmentSourceV1::Continuation {
            schema_version: crate::SEGMENT_SOURCE_SCHEMA_VERSION,
            relative_path: "chronicle/20260401/080000_300/talents/activity.md".to_owned(),
            sha256: "a".repeat(64),
            predecessor_segment_key: predecessor.to_owned(),
            change_evidence_relative_path: "chronicle/20260401/080000_300/talents/change.json"
                .to_owned(),
            change_evidence_sha256: "c".repeat(64),
        };

        assert_ne!(
            segment_input_digest(&binding, &source("070000_300")).unwrap(),
            segment_input_digest(&binding, &source("071000_300")).unwrap()
        );

        let baseline = source("070000_300");
        let mut changed_evidence = baseline.clone();
        let SegmentSourceV1::Continuation {
            change_evidence_sha256,
            ..
        } = &mut changed_evidence
        else {
            unreachable!();
        };
        *change_evidence_sha256 = "e".repeat(64);
        assert_ne!(
            segment_input_digest(&binding, &baseline).unwrap(),
            segment_input_digest(&binding, &changed_evidence).unwrap()
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
            master_source_digest(&first, &jobs()).unwrap(),
            master_source_digest(&second, &jobs()).unwrap()
        );
    }

    #[test]
    fn curation_jobs_digest_covers_each_scoped_request() {
        let baseline = jobs();
        let mut changed = baseline.clone();
        changed[1].request.system_instruction = Some("Changed hour prompt".to_owned());

        assert_ne!(
            curation_jobs_digest(&baseline).unwrap(),
            curation_jobs_digest(&changed).unwrap()
        );
    }

    fn jobs() -> Vec<CurationJobV1> {
        vec![
            CurationJobV1 {
                scope: "hour:09".to_owned(),
                candidates: vec![entry("first")],
                request: request(),
            },
            CurationJobV1 {
                scope: "day".to_owned(),
                candidates: vec![entry("first"), entry("second")],
                request: request(),
            },
        ]
    }
}
