// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{BodyDigest, BodySourceFamily};

/// One replayed body-dedupe row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyDedupeRow {
    dedupe_key: String,
    source_family: BodySourceFamily,
    source_record_id: Option<String>,
    record_type: String,
    start_time: String,
    end_time: Option<String>,
    value_hash: Option<BodyDigest>,
    first_import_id: Option<String>,
    latest_import_id: Option<String>,
    normalized_ref: Option<String>,
    raw_ref: Option<String>,
}

impl BodyDedupeRow {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        dedupe_key: String,
        source_family: BodySourceFamily,
        source_record_id: Option<String>,
        record_type: String,
        start_time: String,
        end_time: Option<String>,
        value_hash: Option<BodyDigest>,
        first_import_id: Option<String>,
        latest_import_id: Option<String>,
        normalized_ref: Option<String>,
        raw_ref: Option<String>,
    ) -> Self {
        Self {
            dedupe_key,
            source_family,
            source_record_id,
            record_type,
            start_time,
            end_time,
            value_hash,
            first_import_id,
            latest_import_id,
            normalized_ref,
            raw_ref,
        }
    }

    pub(crate) fn update(&mut self, incoming: Self) {
        if incoming.source_record_id.is_some() {
            self.source_record_id = incoming.source_record_id;
        }
        self.start_time = incoming.start_time;
        self.end_time = incoming.end_time;
        if incoming.value_hash.is_some() {
            self.value_hash = incoming.value_hash;
        }
        self.latest_import_id = incoming.latest_import_id;
        if incoming.normalized_ref.is_some() {
            self.normalized_ref = incoming.normalized_ref;
        }
        if incoming.raw_ref.is_some() {
            self.raw_ref = incoming.raw_ref;
        }
    }

    pub fn dedupe_key(&self) -> &str {
        &self.dedupe_key
    }

    pub fn source_family(&self) -> BodySourceFamily {
        self.source_family
    }

    pub fn source_record_id(&self) -> Option<&str> {
        self.source_record_id.as_deref()
    }

    pub fn record_type(&self) -> &str {
        &self.record_type
    }

    pub fn start_time(&self) -> &str {
        &self.start_time
    }

    pub fn end_time(&self) -> Option<&str> {
        self.end_time.as_deref()
    }

    pub fn value_hash(&self) -> Option<&BodyDigest> {
        self.value_hash.as_ref()
    }

    pub fn first_import_id(&self) -> Option<&str> {
        self.first_import_id.as_deref()
    }

    pub fn latest_import_id(&self) -> Option<&str> {
        self.latest_import_id.as_deref()
    }

    pub fn normalized_ref(&self) -> Option<&str> {
        self.normalized_ref.as_deref()
    }

    pub fn raw_ref(&self) -> Option<&str> {
        self.raw_ref.as_deref()
    }
}
