// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct BacklogSource {
    pub backlog: Option<serde_json::Map<String, Value>>,
    pub validity: BacklogValidity,
    pub generated_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BacklogValidity {
    Missing,
    Unparseable,
    Malformed,
    NoBacklogKey,
    Valid,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlowDocument {
    pub content: Option<String>,
    pub updated_at: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PulseNarrative {
    pub content: Option<String>,
    pub updated_at: Option<String>,
    pub needs: Vec<String>,
    pub window: Option<PulseWindow>,
}

/// Counts refer to input records, which may describe overlapping activity.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PulseWindow {
    pub segments: u64,
    pub activities: u64,
    pub input_segments: u64,
    pub input_activities: u64,
    pub since_ms: Option<i64>,
    pub gaps: Vec<String>,
}
