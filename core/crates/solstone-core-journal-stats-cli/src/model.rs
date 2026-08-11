// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 8;

/// The fourteen schema-v8 day fields and the fold degradation marker.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DayStats {
    pub transcript_sessions: u64,
    pub transcript_segments: u64,
    pub transcript_duration: f64,
    pub transcript_ranges: u64,
    pub percept_sessions: u64,
    pub percept_frames: u64,
    pub percept_duration: f64,
    pub percept_ranges: u64,
    pub browser_segments: u64,
    pub pending_segments: u64,
    pub segments_pending_think: u64,
    pub outputs_processed: u64,
    pub outputs_pending: u64,
    pub day_bytes: u64,
    #[serde(default)]
    pub segment_fold_failed: bool,
}

/// Count and estimated duration accumulated for one activity key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActivityTotals {
    pub count: u64,
    pub minutes: f64,
}

/// Per-day heatmap data, with Monday represented by weekday zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HeatmapData {
    pub weekday: u8,
    pub hours: BTreeMap<u8, f64>,
}

/// Durable per-day cache payload and the result of a cache hit or scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayScan {
    pub schema_version: u32,
    pub stats: DayStats,
    pub agent_data: BTreeMap<String, ActivityTotals>,
    pub facet_data: BTreeMap<String, ActivityTotals>,
    pub heatmap_data: HeatmapData,
}

impl Default for DayScan {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            stats: DayStats::default(),
            agent_data: BTreeMap::new(),
            facet_data: BTreeMap::new(),
            heatmap_data: HeatmapData::default(),
        }
    }
}

/// Cache behavior observed while producing a day result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    Hit,
    Saved,
    SaveFailed { message: String },
    NotSavedMissingDay,
}

/// A computed day payload plus the observable cache persistence result.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanDayOutcome {
    pub scan: DayScan,
    pub cache_status: CacheStatus,
}
