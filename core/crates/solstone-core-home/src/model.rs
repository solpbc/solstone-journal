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
}
