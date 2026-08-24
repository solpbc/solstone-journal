// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical grant-request selector and closed writer-family / target-scope types.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::digest::{RecordDigest, digest_value};
use crate::error::{ConvergenceError, Refusal, random_hex};
use crate::layout::DayKey;
use crate::schema::ROLE_GRANT_SELECTOR;

/// Closed writer family a grant request may name.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriterFamily {
    Observe,
    Think,
    Import,
}

/// Closed journal-tree target a grant request may name.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetScope {
    Chronicle,
    Entities,
    Facets,
}

/// Intended transaction class bound into a prepared owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionClass {
    AdvanceDirty,
}

/// External operation identity. 32-byte CSPRNG value, lowercase hex.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationId(String);

impl OperationId {
    pub fn generate() -> Result<Self, ConvergenceError> {
        Ok(Self(random_hex()?))
    }

    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

/// Canonical grant-request selector. Empty requests are valid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrantRequestSelector {
    days: Vec<DayKey>,
    requests: Vec<SelectorRequest>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SelectorRequest {
    day: String,
    writer_family: WriterFamily,
    target_scope: TargetScope,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct GrantSelectorCanon {
    role: String,
    days: Vec<String>,
    requests: Vec<SelectorRequest>,
}

impl GrantRequestSelector {
    /// Empty grant set over `admitted` days. Days must be unique.
    pub fn empty(admitted: &[DayKey]) -> Result<Self, ConvergenceError> {
        Self::try_new(
            admitted,
            std::iter::empty::<(&str, WriterFamily, TargetScope)>(),
        )
    }

    /// Bind each request to `admitted`. Rejects aliases, duplicates, and extra days.
    pub fn try_new<S, I>(admitted: &[DayKey], requests: I) -> Result<Self, ConvergenceError>
    where
        S: AsRef<str>,
        I: IntoIterator<Item = (S, WriterFamily, TargetScope)>,
    {
        let mut admitted_set = BTreeSet::new();
        for day in admitted {
            if !admitted_set.insert(day.clone()) {
                return Err(ConvergenceError::Refused(Refusal::DuplicateDays));
            }
        }
        let mut seen = BTreeSet::new();
        let mut ordered = Vec::new();
        for (day, writer_family, target_scope) in requests {
            let key = DayKey::parse(day.as_ref())?;
            if !admitted_set.contains(&key) {
                return Err(ConvergenceError::Refused(Refusal::WrongDay {
                    expected: admitted_set
                        .iter()
                        .map(|day| day.as_str().to_owned())
                        .collect::<Vec<_>>()
                        .join(","),
                    observed: key.as_str().to_owned(),
                }));
            }
            let request = SelectorRequest {
                day: key.as_str().to_owned(),
                writer_family,
                target_scope,
            };
            if !seen.insert((key, writer_family, target_scope)) {
                return Err(ConvergenceError::Refused(Refusal::DuplicateGrantRequest));
            }
            ordered.push(request);
        }
        ordered.sort();
        let mut days: Vec<DayKey> = admitted_set.into_iter().collect();
        days.sort();
        Ok(Self {
            days,
            requests: ordered,
        })
    }

    pub fn days(&self) -> &[DayKey] {
        &self.days
    }

    pub fn digest(&self) -> Result<RecordDigest, ConvergenceError> {
        digest_value(&GrantSelectorCanon {
            role: ROLE_GRANT_SELECTOR.to_owned(),
            days: self
                .days
                .iter()
                .map(|day| day.as_str().to_owned())
                .collect(),
            requests: self.requests.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(value: &str) -> DayKey {
        DayKey::parse(value).unwrap()
    }

    fn admitted() -> Vec<DayKey> {
        vec![day("20260823"), day("20260824")]
    }

    #[test]
    fn permutation_stable_digest() {
        let left = GrantRequestSelector::try_new(
            &admitted(),
            [
                ("20260824", WriterFamily::Think, TargetScope::Facets),
                ("20260823", WriterFamily::Observe, TargetScope::Chronicle),
            ],
        )
        .unwrap();
        let right = GrantRequestSelector::try_new(
            &admitted(),
            [
                ("20260823", WriterFamily::Observe, TargetScope::Chronicle),
                ("20260824", WriterFamily::Think, TargetScope::Facets),
            ],
        )
        .unwrap();
        assert_eq!(left.digest().unwrap(), right.digest().unwrap());
        assert_eq!(left.days(), right.days());
    }

    fn base_one_request() -> GrantRequestSelector {
        GrantRequestSelector::try_new(
            &admitted(),
            [("20260823", WriterFamily::Observe, TargetScope::Chronicle)],
        )
        .unwrap()
    }

    #[test]
    fn digest_changes_under_add() {
        let added = GrantRequestSelector::try_new(
            &admitted(),
            [
                ("20260823", WriterFamily::Observe, TargetScope::Chronicle),
                ("20260823", WriterFamily::Think, TargetScope::Entities),
            ],
        )
        .unwrap();
        assert_ne!(
            base_one_request().digest().unwrap(),
            added.digest().unwrap()
        );
    }

    #[test]
    fn digest_changes_under_remove() {
        let removed = GrantRequestSelector::empty(&admitted()).unwrap();
        assert_ne!(
            base_one_request().digest().unwrap(),
            removed.digest().unwrap()
        );
    }

    #[test]
    fn digest_changes_under_substitution() {
        let substituted = GrantRequestSelector::try_new(
            &admitted(),
            [("20260823", WriterFamily::Import, TargetScope::Chronicle)],
        )
        .unwrap();
        assert_ne!(
            base_one_request().digest().unwrap(),
            substituted.digest().unwrap()
        );
    }

    #[test]
    fn duplicate_tuple_refused() {
        let error = GrantRequestSelector::try_new(
            &admitted(),
            [
                ("20260823", WriterFamily::Observe, TargetScope::Chronicle),
                ("20260823", WriterFamily::Observe, TargetScope::Chronicle),
            ],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::DuplicateGrantRequest)
        ));
    }

    #[test]
    fn alias_day_refused() {
        let error = GrantRequestSelector::try_new(
            &admitted(),
            [("2026-08-23", WriterFamily::Observe, TargetScope::Chronicle)],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::NonCanonicalDays)
        ));
    }

    #[test]
    fn day_outside_admitted_refused() {
        let error = GrantRequestSelector::try_new(
            &admitted(),
            [("20260825", WriterFamily::Observe, TargetScope::Chronicle)],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::WrongDay { observed, .. }) if observed == "20260825"
        ));
    }

    #[test]
    fn empty_selector_valid() {
        let empty = GrantRequestSelector::empty(&admitted()).unwrap();
        assert_eq!(empty.days(), admitted().as_slice());
        let again = GrantRequestSelector::try_new(
            &admitted(),
            std::iter::empty::<(&str, WriterFamily, TargetScope)>(),
        )
        .unwrap();
        assert_eq!(empty.digest().unwrap(), again.digest().unwrap());
        assert_eq!(
            TransactionClass::AdvanceDirty,
            TransactionClass::AdvanceDirty
        );
        let operation = OperationId::generate().unwrap();
        assert_eq!(operation.as_hex().len(), 64);
    }
}
