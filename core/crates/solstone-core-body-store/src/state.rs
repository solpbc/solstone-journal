// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use solstone_core_body_source::{BodyDigest, BodyString, ValidatedBodyRowEvent};

use crate::error::{BodyDedupeError, BodyDedupeErrorField};
use crate::legacy::ValidatedLegacyBodyRow;
use crate::row::BodyDedupeRow;
use crate::text::body_string_to_text;

/// Whether a replayed observation inserted or updated its dedupe row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BodyDedupeDisposition {
    Inserted,
    Updated,
}

/// Pure in-memory replay state for validated body observations.
#[derive(Default)]
pub struct BodyDedupeState {
    rows: BTreeMap<String, BodyDedupeRow>,
}

impl BodyDedupeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replays one row-agreed event; bare ledger events cannot satisfy this input type.
    pub fn apply(
        &mut self,
        event: &ValidatedBodyRowEvent,
    ) -> Result<BodyDedupeDisposition, BodyDedupeError> {
        let event = event.event();
        let source_record_id = optional_text(
            event.source_record_id(),
            event,
            BodyDedupeErrorField::SourceRecordId,
        )?;
        let record_type =
            required_text(event.record_type(), event, BodyDedupeErrorField::RecordType)?;
        let start_time = required_text(event.start_time(), event, BodyDedupeErrorField::StartTime)?;
        let end_time = optional_text(event.end_time(), event, BodyDedupeErrorField::EndTime)?;
        let raw_ref = optional_text(event.raw_ref(), event, BodyDedupeErrorField::RawRef)?;
        let normalized_ref = body_string_to_text(event.normalized_ref())
            .expect("normalized_ref is ASCII by validated ledger-event construction");

        let key = event.dedupe_key().as_str().to_owned();
        let incoming = BodyDedupeRow::new(
            key.clone(),
            event.source_family(),
            source_record_id,
            record_type,
            start_time,
            end_time,
            Some(event.value_hash().clone()),
            Some(event.bundle_id().as_str().to_owned()),
            Some(event.bundle_id().as_str().to_owned()),
            Some(normalized_ref),
            raw_ref,
        );
        Ok(self.apply_row(incoming))
    }

    /// Replays one checked row from a pre-native body bundle.
    pub fn apply_legacy(&mut self, observation: &ValidatedLegacyBodyRow) -> BodyDedupeDisposition {
        self.apply_row(observation.row().clone())
    }

    fn apply_row(&mut self, incoming: BodyDedupeRow) -> BodyDedupeDisposition {
        let key = incoming.dedupe_key().to_owned();
        let Some(existing) = self.rows.get_mut(&key) else {
            self.rows.insert(key, incoming);
            return BodyDedupeDisposition::Inserted;
        };

        existing.update(incoming);
        BodyDedupeDisposition::Updated
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn get(&self, key: &BodyDigest) -> Option<&BodyDedupeRow> {
        self.rows.get(key.as_str())
    }

    /// Looks up either a native digest key or an older importer-defined key.
    pub fn get_by_key(&self, key: &str) -> Option<&BodyDedupeRow> {
        self.rows.get(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = &BodyDedupeRow> + '_ {
        self.rows.values()
    }
}

fn required_text(
    value: &BodyString,
    event: &solstone_core_body_source::BodyLedgerEvent,
    field: BodyDedupeErrorField,
) -> Result<String, BodyDedupeError> {
    body_string_to_text(value)
        .ok_or_else(|| BodyDedupeError::new(event.bundle_id().clone(), event.sequence(), field))
}

fn optional_text(
    value: Option<&BodyString>,
    event: &solstone_core_body_source::BodyLedgerEvent,
    field: BodyDedupeErrorField,
) -> Result<Option<String>, BodyDedupeError> {
    value
        .map(|value| required_text(value, event, field))
        .transpose()
}
