// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

use super::record::ObserverRecord;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAssessment {
    pub name: String,
    pub last_seen: Option<i64>,
    pub last_seen_age_ms: Option<i64>,
    pub reach: Reach,
    pub last_segment_received_age_ms: Option<i64>,
    pub rejecting: bool,
    pub state: OwnerState,
    pub device_binding_kind: Option<String>,
    pub ingest_rejection: Option<Map<String, Value>>,
    pub beacon: Option<Map<String, Value>>,
}

/// Inspect delivery for the assessed subset of `records` at `now_ms`.
/// Unassessed devices are omitted. Order: input order among assessed.
pub fn inspect_delivery(records: &[ObserverRecord], now_ms: i64) -> Vec<DeliveryAssessment> {
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
            let reach = match last_seen_age_ms {
                Some(age) if age < OBSERVER_ACTIVE_MS => Reach::Active,
                Some(age) if age < OBSERVER_STALE_MS => Reach::Stale,
                _ => Reach::Offline,
            };
            DeliveryAssessment {
                name: record.name().unwrap_or("unknown").to_owned(),
                last_seen: record.last_seen(),
                last_seen_age_ms,
                reach,
                last_segment_received_age_ms,
                rejecting,
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

fn is_assessed(record: &ObserverRecord, now_ms: i64) -> bool {
    !record.revoked()
        && record.enabled() != Some(false)
        && (usable_observer_stamp(record.last_segment_received_at(), now_ms).is_some()
            || record.ingest_rejection().is_some())
}

fn usable_observer_stamp(value: Option<i64>, now_ms: i64) -> Option<i64> {
    value.filter(|value| *value >= 0 && *value <= now_ms)
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
        let rows = inspect_delivery(&[revoked, disabled, never, rejecting], NOW);
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["rej"]
        );
        assert!(rows[0].rejecting);
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
}
