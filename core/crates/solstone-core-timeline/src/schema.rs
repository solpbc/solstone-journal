// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::TimelineError;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineKind {
    Segment,
    Day,
    Master,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentSelectorV1 {
    pub day: String,
    pub segment: String,
    pub stream: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentBindingV1 {
    pub day: String,
    pub stream: String,
    pub segment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineEntryV1 {
    pub title: String,
    pub description: String,
    pub origin: String,
    pub binding: SegmentBindingV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationProvenanceV1 {
    pub model: String,
    pub finish_reason: String,
    pub schema_validation: Value,
    pub inference: Value,
    pub usage: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentSummaryV1 {
    pub title: String,
    pub description: String,
    pub origin: String,
    pub continuation_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentTimelineV1 {
    pub schema_version: u32,
    pub kind: TimelineKind,
    pub binding: SegmentBindingV1,
    pub input_digest: String,
    pub generated_at_ms: i64,
    pub summary: SegmentSummaryV1,
    pub provenance: Option<GenerationProvenanceV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurationRecordV1 {
    pub input_digest: String,
    pub candidate_count: usize,
    pub picks: Vec<TimelineEntryV1>,
    pub rationale: String,
    pub error: Option<String>,
    pub provenance: Option<GenerationProvenanceV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HourTimelineV1 {
    pub source_digest: String,
    pub segment_count: usize,
    pub curation: CurationRecordV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DayTimelineV1 {
    pub schema_version: u32,
    pub kind: TimelineKind,
    pub day: String,
    pub source_digest: String,
    pub generated_at_ms: i64,
    pub top_n: usize,
    pub segment_count: usize,
    pub hour_count: usize,
    pub hours: BTreeMap<String, HourTimelineV1>,
    pub day_curation: CurationRecordV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonthTimelineEntryV1 {
    pub month: String,
    pub entry: TimelineEntryV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonthTimelineV1 {
    pub day_count: usize,
    pub days: BTreeMap<String, DayTimelineV1>,
    pub month_curation: CurationRecordV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MasterTimelineV1 {
    pub schema_version: u32,
    pub kind: TimelineKind,
    pub source_digest: String,
    pub generated_at_ms: i64,
    pub top_n: usize,
    pub months: BTreeMap<String, MonthTimelineV1>,
    pub year_top: Vec<MonthTimelineEntryV1>,
    pub year_curation: CurationRecordV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CurationContentPartV1 {
    Text { text: String },
    Image { mime_type: String, data: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurationRequestV1 {
    pub id: Option<String>,
    pub context: String,
    pub contents: Vec<CurationContentPartV1>,
    pub system_instruction: Option<String>,
    pub temperature: f64,
    pub max_output_tokens: u64,
    pub thinking_budget: Option<u64>,
    pub timeout_s: Option<f64>,
    pub json_output: bool,
    pub json_schema: Option<Value>,
    pub enforce_responsiveness: bool,
    pub attempt_index: u64,
    pub exclusive_admission: bool,
    pub transport_retries: Option<u64>,
}

pub fn validate_segment_binding(binding: &SegmentBindingV1) -> Result<(), TimelineError> {
    if binding.day.is_empty() || binding.stream.is_empty() || binding.segment.is_empty() {
        return Err(TimelineError::MalformedBinding {
            day: binding.day.clone(),
            stream: binding.stream.clone(),
            segment: binding.segment.clone(),
        });
    }
    Ok(())
}

pub fn validate_segment_timeline(value: &SegmentTimelineV1) -> Result<(), TimelineError> {
    validate_header(value.schema_version, value.kind, TimelineKind::Segment)?;
    validate_segment_binding(&value.binding)
}

pub fn validate_day_timeline(value: &DayTimelineV1) -> Result<(), TimelineError> {
    validate_header(value.schema_version, value.kind, TimelineKind::Day)?;
    if value.day.is_empty() {
        return Err(TimelineError::MalformedBinding {
            day: value.day.clone(),
            stream: String::new(),
            segment: String::new(),
        });
    }
    validate_curation(&value.day_curation)?;
    for hour in value.hours.values() {
        validate_curation(&hour.curation)?;
    }
    Ok(())
}

pub fn validate_master_timeline(value: &MasterTimelineV1) -> Result<(), TimelineError> {
    validate_header(value.schema_version, value.kind, TimelineKind::Master)?;
    validate_curation(&value.year_curation)?;
    for month in value.months.values() {
        validate_curation(&month.month_curation)?;
        for day in month.days.values() {
            validate_day_timeline(day)?;
        }
    }
    for entry in &value.year_top {
        validate_segment_binding(&entry.entry.binding)?;
    }
    Ok(())
}

fn validate_header(
    schema_version: u32,
    actual: TimelineKind,
    expected: TimelineKind,
) -> Result<(), TimelineError> {
    if schema_version != CURRENT_SCHEMA_VERSION {
        return Err(TimelineError::SchemaVersionMismatch {
            expected: CURRENT_SCHEMA_VERSION,
            actual: schema_version,
        });
    }
    if actual != expected {
        return Err(TimelineError::SchemaKindMismatch { expected, actual });
    }
    Ok(())
}

fn validate_curation(curation: &CurationRecordV1) -> Result<(), TimelineError> {
    for pick in &curation.picks {
        validate_segment_binding(&pick.binding)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> SegmentBindingV1 {
        SegmentBindingV1 {
            day: "20260401".to_owned(),
            stream: "_default".to_owned(),
            segment: "080000_300".to_owned(),
        }
    }

    fn segment() -> SegmentTimelineV1 {
        SegmentTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Segment,
            binding: binding(),
            input_digest: "digest".to_owned(),
            generated_at_ms: 1,
            summary: SegmentSummaryV1 {
                title: "Title".to_owned(),
                description: "Description".to_owned(),
                origin: "20260401/080000_300".to_owned(),
                continuation_of: None,
            },
            provenance: None,
        }
    }

    #[test]
    fn segment_validation_rejects_wrong_version() {
        let mut value = segment();
        value.schema_version += 1;
        assert!(matches!(
            validate_segment_timeline(&value),
            Err(TimelineError::SchemaVersionMismatch { .. })
        ));
    }

    #[test]
    fn segment_validation_rejects_wrong_kind() {
        let mut value = segment();
        value.kind = TimelineKind::Day;
        assert!(matches!(
            validate_segment_timeline(&value),
            Err(TimelineError::SchemaKindMismatch { .. })
        ));
    }

    #[test]
    fn segment_validation_rejects_malformed_binding() {
        let mut value = segment();
        value.binding.stream.clear();
        assert!(matches!(
            validate_segment_timeline(&value),
            Err(TimelineError::MalformedBinding { .. })
        ));
    }
}
