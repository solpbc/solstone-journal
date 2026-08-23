// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Serialize;
use serde_json::{Map, Value};

use super::record::ObserverRecord;
use super::reload::{ObserverLoad, ReloadError};

pub const OBSERVER_ACTIVE_MS: i64 = 30_000;
pub const OBSERVER_STALE_MS: i64 = 120_000;
pub const OBSERVER_DELIVERY_STALL_MS: i64 = 21_600_000;
pub const OBSERVER_DELIVERY_LONG_STOP_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Active,
    Stale,
    Offline,
}

impl Reach {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Stale => "stale",
            Self::Offline => "offline",
        }
    }
}

impl Serialize for Reach {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerState {
    Degraded,
    Active,
    Stale,
    Offline,
}

impl OwnerState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Degraded => "degraded",
            Self::Active => "active",
            Self::Stale => "stale",
            Self::Offline => "offline",
        }
    }
}

impl Serialize for OwnerState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnassessedReason {
    AwaitingFirstDelivery,
    InvalidDeliveryEvidence,
    RegistrationResidue,
}

impl UnassessedReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingFirstDelivery => "awaiting_first_delivery",
            Self::InvalidDeliveryEvidence => "invalid_delivery_evidence",
            Self::RegistrationResidue => "registration_residue",
        }
    }
}

impl Serialize for UnassessedReason {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryState {
    RegistryUnknown,
    PartialRegistry,
    RegistryEmpty,
    NoEligibleRecords,
    RegistryComplete,
}

impl RegistryState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegistryUnknown => "registry_unknown",
            Self::PartialRegistry => "partial_registry",
            Self::RegistryEmpty => "registry_empty",
            Self::NoEligibleRecords => "no_eligible_records",
            Self::RegistryComplete => "registry_complete",
        }
    }
}

impl Serialize for RegistryState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAssessment {
    pub name: String,
    pub last_seen: Option<i64>,
    pub last_seen_age_ms: Option<i64>,
    pub reach: Reach,
    pub last_segment_received_age_ms: Option<i64>,
    pub state: OwnerState,
    pub device_binding_kind: Option<String>,
    pub ingest_rejection: Option<Map<String, Value>>,
    pub beacon: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnassessedObserver {
    pub name: String,
    pub reason: UnassessedReason,
    pub reach: Reach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssessedObserverFact {
    pub name: String,
    pub state: OwnerState,
    pub reach: Reach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObserverDeliveryFacts {
    pub registry: RegistryState,
    pub assessed: Vec<AssessedObserverFact>,
    pub unassessed: Vec<UnassessedObserver>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryInspection {
    pub assessed: Vec<DeliveryAssessment>,
    pub unassessed: Vec<UnassessedObserver>,
    pub registry: RegistryState,
}

struct DeliveryPartition {
    assessed: Vec<DeliveryAssessment>,
    unassessed: Vec<UnassessedObserver>,
}

/// Inspect the loaded observer registry at `now_ms`.
///
/// A load error becomes `registry_unknown` with empty collections. Missing
/// observers directories are empty inventory, not unknown.
pub fn inspect_loaded(
    loaded: Result<ObserverLoad, ReloadError>,
    now_ms: i64,
) -> DeliveryInspection {
    match loaded {
        Err(_) => DeliveryInspection {
            assessed: Vec::new(),
            unassessed: Vec::new(),
            registry: RegistryState::RegistryUnknown,
        },
        Ok(load) => {
            let partition = inspect_records(&load.records, now_ms);
            DeliveryInspection {
                assessed: partition.assessed,
                unassessed: partition.unassessed,
                registry: registry_state(&load),
            }
        }
    }
}

fn inspect_records(records: &[ObserverRecord], now_ms: i64) -> DeliveryPartition {
    DeliveryPartition {
        assessed: inspect_delivery(records, now_ms),
        unassessed: unassessed_rows(records, now_ms),
    }
}

/// Inspect delivery for the assessed subset of `records` at `now_ms`.
/// Unassessed devices are omitted. Order: input order among assessed.
fn inspect_delivery(records: &[ObserverRecord], now_ms: i64) -> Vec<DeliveryAssessment> {
    let assessed: Vec<&ObserverRecord> = records
        .iter()
        .filter(|record| is_assessed(record, now_ms))
        .collect();
    let ages: Vec<Option<i64>> = assessed
        .iter()
        .map(|record| {
            usable_observer_stamp(record.last_segment_received_at(), now_ms)
                .map(|stamp| now_ms - stamp)
        })
        .collect();
    assessed
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let last_segment_received_age_ms = ages[index];
            let other_delivered = ages.iter().enumerate().any(|(peer, age)| {
                peer != index && age.is_some_and(|gap| gap <= OBSERVER_DELIVERY_STALL_MS)
            });
            let stalled = last_segment_received_age_ms.is_some_and(|gap| {
                (gap > OBSERVER_DELIVERY_STALL_MS && other_delivered)
                    || gap > OBSERVER_DELIVERY_LONG_STOP_MS
            });
            let rejecting = record.ingest_rejection().is_some();
            let state = if rejecting {
                OwnerState::Degraded
            } else if !stalled {
                OwnerState::Active
            } else if other_delivered {
                OwnerState::Stale
            } else {
                OwnerState::Offline
            };
            let last_seen_age_ms =
                usable_observer_stamp(record.last_seen(), now_ms).map(|stamp| now_ms - stamp);
            DeliveryAssessment {
                name: record.name().unwrap_or("unknown").to_owned(),
                last_seen: record.last_seen(),
                last_seen_age_ms,
                reach: observer_reach(record.last_seen(), now_ms),
                last_segment_received_age_ms,
                state,
                device_binding_kind: record.device_binding_kind().map(str::to_owned),
                ingest_rejection: record.ingest_rejection().cloned(),
                beacon: record.health_beacon().cloned(),
            }
        })
        .collect()
}

/// Worst-of owner rollup. `None` means the assessed set is empty.
pub fn rollup_owner_states(assessed: &[DeliveryAssessment]) -> Option<OwnerState> {
    if assessed.is_empty() {
        return None;
    }
    // Offline only when the whole assessed set is quiet ≥ a day. A mix of
    // overnight-Active and 24h-Offline is stale: saying sol hasn't added
    // anything recently while a device added 8h ago would be a false
    // statement to the owner.
    if assessed.iter().any(|row| row.state == OwnerState::Degraded) {
        Some(OwnerState::Degraded)
    } else if assessed.iter().any(|row| row.state == OwnerState::Stale)
        || (assessed.iter().any(|row| row.state == OwnerState::Offline)
            && assessed.iter().any(|row| row.state == OwnerState::Active))
    {
        Some(OwnerState::Stale)
    } else if assessed.iter().any(|row| row.state == OwnerState::Offline) {
        Some(OwnerState::Offline)
    } else {
        Some(OwnerState::Active)
    }
}

fn is_eligible(record: &ObserverRecord) -> bool {
    !record.revoked() && record.enabled() != Some(false)
}

fn is_assessed(record: &ObserverRecord, now_ms: i64) -> bool {
    is_eligible(record)
        && (usable_observer_stamp(record.last_segment_received_at(), now_ms).is_some()
            || record.ingest_rejection().is_some())
}

fn usable_observer_stamp(value: Option<i64>, now_ms: i64) -> Option<i64> {
    value.filter(|value| *value >= 0 && *value <= now_ms)
}

fn observer_reach(last_seen: Option<i64>, now_ms: i64) -> Reach {
    match usable_observer_stamp(last_seen, now_ms).map(|stamp| now_ms - stamp) {
        Some(age) if age < OBSERVER_ACTIVE_MS => Reach::Active,
        Some(age) if age < OBSERVER_STALE_MS => Reach::Stale,
        _ => Reach::Offline,
    }
}

fn unassessed_rows(records: &[ObserverRecord], now_ms: i64) -> Vec<UnassessedObserver> {
    records
        .iter()
        .filter(|record| is_eligible(record) && !is_assessed(record, now_ms))
        .map(|record| UnassessedObserver {
            name: record.name().unwrap_or("unknown").to_owned(),
            reason: unassessed_reason(record, now_ms),
            reach: observer_reach(record.last_seen(), now_ms),
        })
        .collect()
}

fn unassessed_reason(record: &ObserverRecord, now_ms: i64) -> UnassessedReason {
    // ObserverRecord::integer / last_segment_received_at is
    // map.get(key).and_then(Value::as_i64). Workspace serde_json is 1.0.150
    // without arbitrary_precision; Number::as_i64 returns None for N::Float.
    // String/float type errors therefore collapse to None, which would be
    // misread as missing if we used the typed accessor here.
    if record
        .value()
        .get("last_segment_received_at")
        .is_some_and(|value| !value.is_null())
    {
        UnassessedReason::InvalidDeliveryEvidence
    } else if observer_reach(record.last_seen(), now_ms) == Reach::Active {
        UnassessedReason::AwaitingFirstDelivery
    } else {
        UnassessedReason::RegistrationResidue
    }
}

fn registry_state(load: &ObserverLoad) -> RegistryState {
    if load.regular_json_entries != load.records.len() {
        RegistryState::PartialRegistry
    } else if load.regular_json_entries == 0 {
        RegistryState::RegistryEmpty
    } else if load.records.iter().all(|record| !is_eligible(record)) {
        RegistryState::NoEligibleRecords
    } else {
        RegistryState::RegistryComplete
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NOW: i64 = 1_000_000_000;

    fn rec(
        name: &str,
        last_seen: Option<i64>,
        last_sent: Option<i64>,
        rejecting: bool,
    ) -> ObserverRecord {
        rec_extra(name, last_seen, last_sent, rejecting, json!({}))
    }

    fn rec_extra(
        name: &str,
        last_seen: Option<i64>,
        last_sent: Option<i64>,
        rejecting: bool,
        extra: Value,
    ) -> ObserverRecord {
        let mut value = json!({
            "key": format!("{name}-keyxx"),
            "name": name,
            "enabled": true,
        });
        if let Some(stamp) = last_seen {
            value["last_seen"] = json!(stamp);
        }
        if let Some(stamp) = last_sent {
            value["last_segment_received_at"] = json!(stamp);
        }
        if rejecting {
            value["health"] = json!({"ingest_rejection": {"active_count": 1}});
        }
        if let Some(object) = extra.as_object() {
            for (key, field) in object {
                value[key] = field.clone();
            }
        }
        ObserverRecord::from_value(value).unwrap()
    }

    fn loaded(records: Vec<ObserverRecord>) -> ObserverLoad {
        let regular_json_entries = records.len();
        ObserverLoad {
            records,
            regular_json_entries,
        }
    }

    fn states(records: &[ObserverRecord]) -> Vec<OwnerState> {
        inspect_delivery(records, NOW)
            .into_iter()
            .map(|row| row.state)
            .collect()
    }

    #[test]
    fn inspect_delivery_matches_owner_states() {
        let hour = 3_600_000;
        let seen_fresh = Some(NOW - 1_000);
        assert_eq!(
            states(&[rec("a", seen_fresh, Some(NOW - 89 * hour), false)]),
            vec![OwnerState::Offline]
        );
        assert_eq!(
            states(&[
                rec("a", seen_fresh, Some(NOW - 120_000), false),
                rec("b", seen_fresh, Some(NOW - 7 * hour), false),
            ]),
            vec![OwnerState::Active, OwnerState::Stale]
        );
        assert_eq!(
            states(&[
                rec("a", seen_fresh, Some(NOW - 120_000), false),
                rec("b", seen_fresh, Some(NOW - 25 * hour), false),
            ]),
            vec![OwnerState::Active, OwnerState::Stale]
        );
        assert_eq!(
            states(&[
                rec("a", Some(NOW - 8 * hour), Some(NOW - 8 * hour), false),
                rec("b", Some(NOW - 8 * hour), Some(NOW - 8 * hour), false),
            ]),
            vec![OwnerState::Active, OwnerState::Active]
        );
        assert_eq!(
            states(&[rec("a", seen_fresh, Some(NOW - 25 * hour), false)]),
            vec![OwnerState::Offline]
        );
        assert_eq!(
            states(&[rec(
                "a",
                Some(NOW - 25 * hour),
                Some(NOW - 25 * hour),
                false
            )]),
            vec![OwnerState::Offline]
        );
        let fleet = (0..4)
            .map(|index| {
                rec(
                    &format!("d{index}"),
                    Some(NOW - 200_000),
                    Some(NOW - 41 * hour),
                    false,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(states(&fleet), vec![OwnerState::Offline; 4]);
        assert!(inspect_delivery(&[rec("a", seen_fresh, None, false)], NOW).is_empty());
        assert_eq!(
            states(&[
                rec("residue", seen_fresh, None, false),
                rec("peer", seen_fresh, Some(NOW - 120_000), false),
            ]),
            vec![OwnerState::Active]
        );
        assert_eq!(
            states(&[rec(
                "a",
                seen_fresh,
                Some(NOW - OBSERVER_DELIVERY_STALL_MS - 1),
                false
            )]),
            vec![OwnerState::Active]
        );
        assert_eq!(
            states(&[rec("rej", seen_fresh, None, true)]),
            vec![OwnerState::Degraded]
        );
        assert_eq!(
            states(&[
                rec("rej", seen_fresh, None, true),
                rec("peer", seen_fresh, Some(NOW - 120_000), false),
            ]),
            vec![OwnerState::Degraded, OwnerState::Active]
        );
        assert_eq!(
            states(&[rec("rej", seen_fresh, Some(NOW - 120_000), true)]),
            vec![OwnerState::Degraded]
        );
        let mix = inspect_delivery(
            &[
                rec("night", Some(NOW - 8 * hour), Some(NOW - 8 * hour), false),
                rec("stop", Some(NOW - 25 * hour), Some(NOW - 25 * hour), false),
            ],
            NOW,
        );
        assert_eq!(
            mix.iter().map(|row| row.state).collect::<Vec<_>>(),
            vec![OwnerState::Active, OwnerState::Offline]
        );
        assert_eq!(rollup_owner_states(&mix), Some(OwnerState::Stale));
    }

    #[test]
    fn inspect_delivery_omits_unassessed_and_revoked() {
        let seen = Some(NOW - 1_000);
        let revoked = rec_extra(
            "gone",
            seen,
            Some(NOW - 120_000),
            false,
            json!({"revoked": true}),
        );
        let disabled = rec_extra(
            "off",
            seen,
            Some(NOW - 120_000),
            false,
            json!({"enabled": false}),
        );
        let never = rec("never", seen, None, false);
        let rejecting = rec("rej", seen, None, true);
        let rows = inspect_delivery(
            &[
                revoked.clone(),
                disabled.clone(),
                never.clone(),
                rejecting.clone(),
            ],
            NOW,
        );
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["rej"]
        );
        assert_eq!(rows[0].state, OwnerState::Degraded);
        assert!(rows[0].ingest_rejection.is_some());

        let inspection = inspect_loaded(Ok(loaded(vec![revoked, disabled, never, rejecting])), NOW);
        assert_eq!(
            inspection
                .assessed
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["rej"]
        );
        assert_eq!(
            inspection.unassessed,
            vec![UnassessedObserver {
                name: "never".to_owned(),
                reason: UnassessedReason::AwaitingFirstDelivery,
                reach: Reach::Active,
            }]
        );
        assert_eq!(inspection.registry, RegistryState::RegistryComplete);
    }

    #[test]
    fn usable_observer_stamp_rejects_negative_and_future() {
        assert_eq!(usable_observer_stamp(Some(-1), NOW), None);
        assert_eq!(usable_observer_stamp(Some(NOW + 1), NOW), None);
        assert_eq!(usable_observer_stamp(Some(NOW), NOW), Some(NOW));
        assert_eq!(usable_observer_stamp(None, NOW), None);
    }

    #[test]
    fn typed_health_accessors_reject_non_objects() {
        let record = ObserverRecord::from_value(
            json!({"key":"key","health":{"beacon":{"at":1},"ingest_rejection":{"reason":"bad"}}}),
        )
        .unwrap();
        assert_eq!(record.health_beacon().unwrap()["at"], 1);
        assert_eq!(record.ingest_rejection().unwrap()["reason"], "bad");
    }

    #[test]
    fn token_serialization_matches_as_str() {
        for value in [Reach::Active, Reach::Stale, Reach::Offline] {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                Value::String(value.as_str().to_owned())
            );
        }
        for value in [
            OwnerState::Degraded,
            OwnerState::Active,
            OwnerState::Stale,
            OwnerState::Offline,
        ] {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                Value::String(value.as_str().to_owned())
            );
        }
        for value in [
            UnassessedReason::AwaitingFirstDelivery,
            UnassessedReason::InvalidDeliveryEvidence,
            UnassessedReason::RegistrationResidue,
        ] {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                Value::String(value.as_str().to_owned())
            );
        }
        for value in [
            RegistryState::RegistryUnknown,
            RegistryState::PartialRegistry,
            RegistryState::RegistryEmpty,
            RegistryState::NoEligibleRecords,
            RegistryState::RegistryComplete,
        ] {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                Value::String(value.as_str().to_owned())
            );
        }
    }

    #[test]
    fn classification_precedence_and_invalid_receipts() {
        let seen_fresh = Some(NOW - 1_000);
        let inspection = |record: ObserverRecord| inspect_loaded(Ok(loaded(vec![record])), NOW);

        let rejecting = inspection(rec("rej", seen_fresh, None, true));
        assert_eq!(rejecting.assessed[0].state, OwnerState::Degraded);
        assert!(rejecting.unassessed.is_empty());

        let rejecting_invalid = inspection(rec_extra(
            "rej",
            seen_fresh,
            None,
            true,
            json!({"last_segment_received_at": "x"}),
        ));
        assert_eq!(rejecting_invalid.assessed[0].state, OwnerState::Degraded);
        assert!(rejecting_invalid.unassessed.is_empty());

        let usable = inspection(rec("ok", seen_fresh, Some(NOW - 120_000), false));
        assert_eq!(usable.assessed[0].state, OwnerState::Active);
        assert!(usable.unassessed.is_empty());

        for extra in [
            json!({"last_segment_received_at": "not-a-stamp"}),
            json!({"last_segment_received_at": 1.5}),
            json!({"last_segment_received_at": -1}),
            json!({"last_segment_received_at": NOW + 1}),
        ] {
            let row = inspection(rec_extra("bad", seen_fresh, None, false, extra));
            assert!(row.assessed.is_empty(), "{row:?}");
            assert_eq!(
                row.unassessed[0].reason,
                UnassessedReason::InvalidDeliveryEvidence
            );
            assert_eq!(row.unassessed[0].reach, Reach::Active);
        }

        let awaiting = inspection(rec("never", seen_fresh, None, false));
        assert!(awaiting.assessed.is_empty());
        assert_eq!(
            awaiting.unassessed[0].reason,
            UnassessedReason::AwaitingFirstDelivery
        );
        assert_eq!(awaiting.unassessed[0].reach, Reach::Active);

        let residue = inspection(rec("old", Some(NOW - 200_000), None, false));
        assert_eq!(
            residue.unassessed[0].reason,
            UnassessedReason::RegistrationResidue
        );
        assert_eq!(residue.unassessed[0].reach, Reach::Offline);

        let null_stamp = inspection(rec_extra(
            "null",
            seen_fresh,
            None,
            false,
            json!({"last_segment_received_at": null}),
        ));
        assert_eq!(
            null_stamp.unassessed[0].reason,
            UnassessedReason::AwaitingFirstDelivery
        );
    }

    #[test]
    fn invalid_receipt_stays_invalid_across_the_active_reach_boundary() {
        let left = inspect_loaded(
            Ok(loaded(vec![rec_extra(
                "bad",
                Some(NOW - 29_999),
                None,
                false,
                json!({"last_segment_received_at": "x"}),
            )])),
            NOW,
        );
        assert_eq!(left.unassessed[0].reach, Reach::Active);
        assert_eq!(
            left.unassessed[0].reason,
            UnassessedReason::InvalidDeliveryEvidence
        );

        let right = inspect_loaded(
            Ok(loaded(vec![rec_extra(
                "bad",
                Some(NOW - 30_000),
                None,
                false,
                json!({"last_segment_received_at": "x"}),
            )])),
            NOW,
        );
        assert_eq!(right.unassessed[0].reach, Reach::Stale);
        assert_eq!(
            right.unassessed[0].reason,
            UnassessedReason::InvalidDeliveryEvidence
        );
    }

    #[test]
    fn inspect_loaded_maps_error_to_registry_unknown() {
        let inspection = inspect_loaded(Err(ReloadError::Directory("injected".into())), NOW);
        assert_eq!(inspection.registry, RegistryState::RegistryUnknown);
        assert!(inspection.assessed.is_empty());
        assert!(inspection.unassessed.is_empty());
    }

    #[test]
    fn inspect_loaded_reports_partial_registry_without_fabricating_rows() {
        let inspection = inspect_loaded(
            Ok(ObserverLoad {
                records: vec![rec("peer", Some(NOW - 1_000), Some(NOW - 1_000), false)],
                regular_json_entries: 2,
            }),
            NOW,
        );
        assert_eq!(inspection.registry, RegistryState::PartialRegistry);
        assert_eq!(inspection.assessed.len(), 1);
        assert!(inspection.unassessed.is_empty());
    }

    #[test]
    fn missing_observers_directory_is_registry_empty() {
        let root = tempfile::TempDir::new().unwrap();
        let inspection = inspect_loaded(
            crate::store::load_observers_with_inventory(root.path()),
            NOW,
        );
        assert_eq!(inspection.registry, RegistryState::RegistryEmpty);
        assert!(inspection.assessed.is_empty());
        assert!(inspection.unassessed.is_empty());
    }

    #[test]
    fn all_disabled_or_revoked_is_no_eligible_records() {
        let inspection = inspect_loaded(
            Ok(loaded(vec![
                rec_extra(
                    "off",
                    Some(NOW - 1_000),
                    Some(NOW - 1_000),
                    false,
                    json!({"enabled": false}),
                ),
                rec_extra(
                    "gone",
                    Some(NOW - 1_000),
                    Some(NOW - 1_000),
                    false,
                    json!({"revoked": true}),
                ),
            ])),
            NOW,
        );
        assert_eq!(inspection.registry, RegistryState::NoEligibleRecords);
        assert!(inspection.assessed.is_empty());
        assert!(inspection.unassessed.is_empty());
    }
}
