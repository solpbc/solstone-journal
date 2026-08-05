// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use serde_json::{Map, Value};

use crate::AppliedFile;

/// Post-advance ingest notice assembled by the wire layer.
pub struct IngestNotice<'a> {
    pub did: &'a str,
    pub source: &'a str,
    pub day: &'a str,
    pub stream: &'a str,
    pub segment: &'a str,
    pub files: &'a [AppliedFile],
    pub meta: &'a Map<String, Value>,
}

/// Best-effort bus seam. Its failures must never invalidate journal durability.
pub trait IngestNotifier: Send + Sync {
    fn notify(&self, notice: &IngestNotice<'_>) -> Result<(), Box<dyn Error + Send + Sync>>;
}

/// Production placeholder until a Callosum producer is introduced.
pub struct LoggingIngestNotifier;

impl IngestNotifier for LoggingIngestNotifier {
    fn notify(&self, notice: &IngestNotice<'_>) -> Result<(), Box<dyn Error + Send + Sync>> {
        log::debug!(
            "observer ingest accepted: did={}, stream={}, day={}, segment={}, files={}",
            notice.did,
            notice.stream,
            notice.day,
            notice.segment,
            notice.files.len(),
        );
        Ok(())
    }
}
