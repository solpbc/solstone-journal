// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use serde_json::{Map, Value};

/// Ingest notice assembled by the wire layer after a successful apply.
pub struct IngestNotice<'a> {
    pub cid: &'a str,
    pub source: &'a str,
    pub day: &'a str,
    pub stream: &'a str,
    pub segment: &'a str,
    pub files: &'a [String],
    pub meta: &'a Map<String, Value>,
}

/// Post-durability bus seam. A notify failure must never roll back or
/// invalidate already-durable journal writes (event and stream advance). The
/// caller surfaces a non-ok HTTP response when notify fails; on-disk state
/// is unaffected.
pub trait IngestNotifier: Send + Sync {
    fn notify(&self, notice: &IngestNotice<'_>) -> Result<(), Box<dyn Error + Send + Sync>>;
}
