// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{BodyDay, BundleId, EnvelopeError, EnvelopeErrorCode, EnvelopeErrorField};

pub(crate) const APPLE_SUMMARY_SCHEMA: &str = "solstone.body.apple_day_summaries.v1";

/// Checked native body-envelope plan for Apple daily summaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppleSummaryPlan {
    schema: &'static str,
    days: Vec<BodyDay>,
}

impl AppleSummaryPlan {
    /// Binds checked native body-envelope values for Apple daily summaries.
    pub fn new(bundle: &BundleId, days: Vec<BodyDay>) -> Result<Self, EnvelopeError> {
        if !days.windows(2).all(|window| window[0] < window[1]) {
            return Err(EnvelopeError::new(
                Some(bundle.clone()),
                EnvelopeErrorCode::InvalidField,
                EnvelopeErrorField::SummaryDays,
                None,
            ));
        }

        Ok(Self {
            schema: APPLE_SUMMARY_SCHEMA,
            days,
        })
    }

    /// Returns this plan's fixed schema spelling.
    pub fn schema(&self) -> &str {
        self.schema
    }

    /// Returns this plan's checked summary days in supplied order.
    pub fn days(&self) -> &[BodyDay] {
        &self.days
    }
}
