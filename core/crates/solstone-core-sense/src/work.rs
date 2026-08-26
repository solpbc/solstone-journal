// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Map, Value};

/// Identity is admitted once from an observing event and never reconstructed.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SegmentKey {
    pub day: String,
    pub stream: Option<String>,
    pub segment: String,
}

#[derive(Clone, Debug)]
pub struct SegmentContext {
    pub key: SegmentKey,
    pub cid: Option<String>,
    pub source: Option<String>,
    pub batch: bool,
    pub meta: Option<Map<String, Value>>,
}

#[derive(Clone, Debug)]
pub struct WorkItem {
    pub context: SegmentContext,
    pub file_path: PathBuf,
    pub handler: String,
    pub queued_at: std::time::SystemTime,
}

#[derive(Debug)]
pub struct SegmentState {
    pub context: SegmentContext,
    pub pending: HashSet<PathBuf>,
    pub started_at: Instant,
    pub errors: Vec<String>,
}

impl SegmentState {
    pub fn new(context: SegmentContext) -> Self {
        Self {
            context,
            pending: HashSet::new(),
            started_at: Instant::now(),
            errors: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_key_keeps_same_segment_in_two_streams_distinct() {
        let a = SegmentKey {
            day: "20260812".into(),
            stream: Some("a".into()),
            segment: "120000_30".into(),
        };
        let b = SegmentKey {
            day: "20260812".into(),
            stream: Some("b".into()),
            segment: "120000_30".into(),
        };
        assert_ne!(a, b);
    }
}
