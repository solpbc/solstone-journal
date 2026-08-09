// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use crate::manifest_binding::BODY_SOURCE_SCHEMA_VALUE;
use crate::{
    AppleSummaryPlan, BodyDay, BodyMonth, BodyRawRetention, BodySourceFamily, BodySourceHash,
    BundleId, EnvelopeError, EnvelopeErrorCode, EnvelopeErrorField, EnvelopeLedger, EnvelopeShard,
};

/// Checked native body-envelope values for one bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyEnvelope {
    schema: &'static str,
    bundle_id: BundleId,
    source_family: BodySourceFamily,
    source_hash: BodySourceHash,
    raw_retention: BodyRawRetention,
    row_count: u64,
    days: Vec<BodyDay>,
    shards: Vec<EnvelopeShard>,
    ledger: EnvelopeLedger,
    summary_plan: Option<AppleSummaryPlan>,
}

impl BodyEnvelope {
    /// Binds checked native body-envelope values for one bundle.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bundle_id: BundleId,
        source_family: BodySourceFamily,
        source_hash: BodySourceHash,
        raw_retention: BodyRawRetention,
        row_count: u64,
        days: Vec<BodyDay>,
        shards: Vec<EnvelopeShard>,
        ledger: EnvelopeLedger,
        summary_plan: Option<AppleSummaryPlan>,
    ) -> Result<Self, EnvelopeError> {
        if source_hash.family() != source_family {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::SourceHash,
                None,
            ));
        }
        raw_retention
            .check_compatible(&source_family)
            .map_err(|_| {
                error(
                    &bundle_id,
                    EnvelopeErrorCode::IncompatibleField,
                    EnvelopeErrorField::RawRetention,
                    None,
                )
            })?;
        if let Some(index) = (1_u64..)
            .zip(days.windows(2))
            .find_map(|(index, window)| (window[0] >= window[1]).then_some(index))
        {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::InvalidField,
                EnvelopeErrorField::Days,
                Some(index),
            ));
        }
        if let Some(index) = (0_u64..)
            .zip(&days)
            .find_map(|(index, day)| (!source_hash.includes_day(day)).then_some(index))
        {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::Days,
                Some(index),
            ));
        }
        if (row_count == 0) != days.is_empty() || (days.len() as u64) > row_count {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::Days,
                None,
            ));
        }
        if let Some(index) = (1_u64..)
            .zip(shards.windows(2))
            .find_map(|(index, window)| {
                (window[0].path().as_bytes() >= window[1].path().as_bytes()).then_some(index)
            })
        {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::InvalidField,
                EnvelopeErrorField::Shards,
                Some(index),
            ));
        }
        if (row_count == 0) != shards.is_empty() {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::Shards,
                None,
            ));
        }
        let mut shard_rows = 0_u64;
        for (index, shard) in (0_u64..).zip(&shards) {
            let Some(sum) = shard_rows.checked_add(shard.rows()) else {
                return Err(error(
                    &bundle_id,
                    EnvelopeErrorCode::InvalidField,
                    EnvelopeErrorField::ShardRows,
                    Some(index),
                ));
            };
            shard_rows = sum;
        }
        if shard_rows != row_count {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::CountMismatch,
                EnvelopeErrorField::ShardRows,
                None,
            ));
        }
        let day_months: BTreeSet<BodyMonth> = days.iter().map(BodyDay::month).collect();
        let shard_months: BTreeSet<BodyMonth> =
            shards.iter().map(EnvelopeShard::month).cloned().collect();
        if day_months != shard_months {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::Shards,
                None,
            ));
        }
        if ledger.events() != row_count {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::CountMismatch,
                EnvelopeErrorField::LedgerEvents,
                None,
            ));
        }
        if source_family == BodySourceFamily::AppleHealth && summary_plan.is_none() {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::MissingField,
                EnvelopeErrorField::SummaryPlan,
                None,
            ));
        }
        if source_family == BodySourceFamily::OuraApi && summary_plan.is_some() {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::SummaryPlan,
                None,
            ));
        }
        if source_family == BodySourceFamily::AppleHealth
            && summary_plan
                .as_ref()
                .is_some_and(|plan| plan.days() != days.as_slice())
        {
            return Err(error(
                &bundle_id,
                EnvelopeErrorCode::CountMismatch,
                EnvelopeErrorField::SummaryDays,
                None,
            ));
        }

        Ok(Self {
            schema: BODY_SOURCE_SCHEMA_VALUE,
            bundle_id,
            source_family,
            source_hash,
            raw_retention,
            row_count,
            days,
            shards,
            ledger,
            summary_plan,
        })
    }

    /// Returns the fixed body-source schema spelling.
    pub fn schema(&self) -> &str {
        self.schema
    }

    /// Returns the checked bundle identifier.
    pub fn bundle_id(&self) -> &BundleId {
        &self.bundle_id
    }

    /// Returns the checked source family.
    pub fn source_family(&self) -> BodySourceFamily {
        self.source_family
    }

    /// Returns the checked source hash.
    pub fn source_hash(&self) -> &BodySourceHash {
        &self.source_hash
    }

    /// Returns the checked raw-retention policy.
    pub fn raw_retention(&self) -> BodyRawRetention {
        self.raw_retention
    }

    /// Returns the declared normalized row count.
    pub fn row_count(&self) -> u64 {
        self.row_count
    }

    /// Returns the strictly ordered covered days.
    pub fn days(&self) -> &[BodyDay] {
        &self.days
    }

    /// Returns the strictly path-ordered normalized shards.
    pub fn shards(&self) -> &[EnvelopeShard] {
        &self.shards
    }

    /// Returns the checked ledger sidecar.
    pub fn ledger(&self) -> &EnvelopeLedger {
        &self.ledger
    }

    /// Returns the checked Apple daily-summary plan, when required by the source family.
    pub fn summary_plan(&self) -> Option<&AppleSummaryPlan> {
        self.summary_plan.as_ref()
    }
}

fn error(
    bundle_id: &BundleId,
    code: EnvelopeErrorCode,
    field: EnvelopeErrorField,
    index: Option<u64>,
) -> EnvelopeError {
    EnvelopeError::new(Some(bundle_id.clone()), code, field, index)
}
