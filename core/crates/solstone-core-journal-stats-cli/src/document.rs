// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use solstone_core_system_health::BacklogView;

use crate::{ActivityTotals, DayScan, JournalStatsError, SCHEMA_VERSION};

/// Complete schema-v8 journal statistics document.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatsDocument {
    pub schema_version: u32,
    pub generated_at: String,
    pub day_count: usize,
    pub days: BTreeMap<String, DocumentDayStats>,
    pub totals: Totals,
    pub heatmap: Vec<Vec<f64>>,
    pub tokens: TokenDocument,
    pub talents: ActivityDocument,
    pub facets: ActivityDocument,
    pub backlog: BacklogView,
    pub segment_fold_failed_days: Vec<String>,
}

/// The fourteen schema-v8 fields emitted for each day.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DocumentDayStats {
    transcript_sessions: u64,
    transcript_segments: u64,
    transcript_duration: f64,
    transcript_ranges: u64,
    percept_sessions: u64,
    percept_frames: u64,
    percept_duration: f64,
    percept_ranges: u64,
    browser_segments: u64,
    pending_segments: u64,
    segments_pending_think: u64,
    outputs_processed: u64,
    outputs_pending: u64,
    day_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    transcript_sessions: u64,
    transcript_segments: u64,
    transcript_duration: f64,
    transcript_ranges: u64,
    percept_sessions: u64,
    percept_frames: u64,
    percept_duration: f64,
    percept_ranges: u64,
    browser_segments: u64,
    pending_segments: u64,
    segments_pending_think: u64,
    outputs_processed: u64,
    outputs_pending: u64,
    day_bytes: u64,
    total_transcript_duration: f64,
    total_percept_duration: f64,
    backlog_pending_days: usize,
    backlog_stuck_days: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TokenDocument {
    by_day: BTreeMap<String, BTreeMap<String, BTreeMap<String, i64>>>,
    by_model: BTreeMap<String, BTreeMap<String, i64>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ActivityDocument {
    counts: BTreeMap<String, u64>,
    minutes: BTreeMap<String, f64>,
    counts_by_day: BTreeMap<String, BTreeMap<String, u64>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TokenUsage {
    pub(crate) by_day: BTreeMap<String, BTreeMap<String, BTreeMap<String, i64>>>,
    pub(crate) by_model: BTreeMap<String, BTreeMap<String, i64>>,
}

pub(crate) fn assemble_document(
    scans: &BTreeMap<String, DayScan>,
    tokens: TokenUsage,
    backlog: BacklogView,
    now: DateTime<Utc>,
) -> StatsDocument {
    let mut days = BTreeMap::new();
    let mut totals = Totals::default();
    let mut heatmap = vec![vec![0.0; 24]; 7];
    let mut talent_counts = BTreeMap::new();
    let mut talent_minutes = BTreeMap::new();
    let mut talent_counts_by_day = BTreeMap::new();
    let mut facet_counts = BTreeMap::new();
    let mut facet_minutes = BTreeMap::new();
    let mut facet_counts_by_day = BTreeMap::new();
    let mut segment_fold_failed_days = Vec::new();

    for (day, scan) in scans {
        let stats = &scan.stats;
        days.insert(day.clone(), project_day(stats));
        apply_totals(&mut totals, stats);
        if stats.segment_fold_failed {
            segment_fold_failed_days.push(day.clone());
        }
        apply_activity(
            day,
            &scan.agent_data,
            &mut talent_counts,
            &mut talent_minutes,
            &mut talent_counts_by_day,
        );
        apply_activity(
            day,
            &scan.facet_data,
            &mut facet_counts,
            &mut facet_minutes,
            &mut facet_counts_by_day,
        );
        let weekday = usize::from(scan.heatmap_data.weekday);
        if weekday < heatmap.len() {
            for (hour, minutes) in &scan.heatmap_data.hours {
                let hour = usize::from(*hour);
                if hour < heatmap[weekday].len() {
                    heatmap[weekday][hour] += minutes;
                }
            }
        }
    }

    totals.total_transcript_duration = totals.transcript_duration;
    totals.total_percept_duration = totals.percept_duration;
    totals.backlog_pending_days = backlog.pending_days;
    totals.backlog_stuck_days = backlog.stuck_days;

    StatsDocument {
        schema_version: SCHEMA_VERSION,
        generated_at: now.to_rfc3339_opts(SecondsFormat::Micros, false),
        day_count: days.len(),
        days,
        totals,
        heatmap,
        tokens: TokenDocument {
            by_day: tokens.by_day,
            by_model: tokens.by_model,
        },
        talents: ActivityDocument {
            counts: talent_counts,
            minutes: rounded_minutes(talent_minutes),
            counts_by_day: talent_counts_by_day,
        },
        facets: ActivityDocument {
            counts: facet_counts,
            minutes: rounded_minutes(facet_minutes),
            counts_by_day: facet_counts_by_day,
        },
        backlog,
        segment_fold_failed_days,
    }
}

impl StatsDocument {
    pub(crate) fn validate(&self) -> Result<(), JournalStatsError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(JournalStatsError::Validation(format!(
                "schema_version is {}, expected {SCHEMA_VERSION}",
                self.schema_version
            )));
        }
        if self.generated_at.is_empty() {
            return Err(JournalStatsError::Validation(
                "generated_at must be non-empty".to_owned(),
            ));
        }
        Ok(())
    }
}

fn project_day(stats: &crate::DayStats) -> DocumentDayStats {
    DocumentDayStats {
        transcript_sessions: stats.transcript_sessions,
        transcript_segments: stats.transcript_segments,
        transcript_duration: stats.transcript_duration,
        transcript_ranges: stats.transcript_ranges,
        percept_sessions: stats.percept_sessions,
        percept_frames: stats.percept_frames,
        percept_duration: stats.percept_duration,
        percept_ranges: stats.percept_ranges,
        browser_segments: stats.browser_segments,
        pending_segments: stats.pending_segments,
        segments_pending_think: stats.segments_pending_think,
        outputs_processed: stats.outputs_processed,
        outputs_pending: stats.outputs_pending,
        day_bytes: stats.day_bytes,
    }
}

fn apply_totals(totals: &mut Totals, stats: &crate::DayStats) {
    totals.transcript_sessions += stats.transcript_sessions;
    totals.transcript_segments += stats.transcript_segments;
    totals.transcript_duration += stats.transcript_duration;
    totals.transcript_ranges += stats.transcript_ranges;
    totals.percept_sessions += stats.percept_sessions;
    totals.percept_frames += stats.percept_frames;
    totals.percept_duration += stats.percept_duration;
    totals.percept_ranges += stats.percept_ranges;
    totals.browser_segments += stats.browser_segments;
    totals.pending_segments += stats.pending_segments;
    totals.segments_pending_think += stats.segments_pending_think;
    totals.outputs_processed += stats.outputs_processed;
    totals.outputs_pending += stats.outputs_pending;
    totals.day_bytes += stats.day_bytes;
}

fn apply_activity(
    day: &str,
    source: &BTreeMap<String, ActivityTotals>,
    counts: &mut BTreeMap<String, u64>,
    minutes: &mut BTreeMap<String, f64>,
    counts_by_day: &mut BTreeMap<String, BTreeMap<String, u64>>,
) {
    let mut day_counts = BTreeMap::new();
    for (name, values) in source {
        *counts.entry(name.clone()).or_default() += values.count;
        *minutes.entry(name.clone()).or_default() += values.minutes;
        if values.count > 0 {
            day_counts.insert(name.clone(), values.count);
        }
    }
    if !day_counts.is_empty() {
        counts_by_day.insert(day.to_owned(), day_counts);
    }
}

fn rounded_minutes(minutes: BTreeMap<String, f64>) -> BTreeMap<String, f64> {
    minutes
        .into_iter()
        .map(|(name, value)| (name, round_half_even_hundredths(value)))
        .collect()
}

fn round_half_even_hundredths(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }

    let bits = value.to_bits();
    let negative = bits >> 63 != 0;
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1_u64 << 52) - 1);
    let (significand, exponent) = if exponent == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1_u64 << 52), exponent - 1023 - 52)
    };

    if exponent >= 0 {
        return value;
    }

    let scaled = significand * 100;
    let shift = (-exponent) as u32;
    let rounded = if shift >= 64 {
        0
    } else {
        let whole = scaled >> shift;
        let remainder = scaled & ((1_u64 << shift) - 1);
        let halfway = 1_u64 << (shift - 1);
        whole + u64::from(remainder > halfway || (remainder == halfway && whole % 2 == 1))
    };
    let result = rounded as f64 / 100.0;
    if negative { -result } else { result }
}
