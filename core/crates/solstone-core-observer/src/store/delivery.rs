// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::record::ObserverRecord;

pub const OBSERVER_STALE_MS: i64 = 120_000;
pub const OBSERVER_DELIVERY_STALL_MS: i64 = 21_600_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryDivergence {
    pub name: String,
    pub last_seen_age_ms: i64,
    pub last_segment_received_age_ms: i64,
}

/// Return delivery freshness facts for a currently reachable observer.
///
/// The supplied clock keeps this read-only projection deterministic.
pub fn delivery_divergence(
    record: &ObserverRecord,
    now_ms: i64,
    reachable_within_ms: i64,
) -> Option<DeliveryDivergence> {
    let last_seen = usable_observer_stamp(record.last_seen(), now_ms)?;
    let last_segment = usable_observer_stamp(record.last_segment_received_at(), now_ms)?;
    let last_seen_age_ms = now_ms - last_seen;
    if last_seen_age_ms >= reachable_within_ms {
        return None;
    }
    Some(DeliveryDivergence {
        name: record.name().unwrap_or("unknown").to_owned(),
        last_seen_age_ms,
        last_segment_received_age_ms: now_ms - last_segment,
    })
}

fn usable_observer_stamp(value: Option<i64>, now_ms: i64) -> Option<i64> {
    value.filter(|value| *value >= 0 && *value <= now_ms)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn ignores_unreachable_and_reports_reachable_divergence() {
        let record = ObserverRecord::from_value(
            json!({"key":"key","name":"screen","last_seen":900,"last_segment_received_at":100}),
        )
        .unwrap();
        assert_eq!(
            delivery_divergence(&record, 1_000, 120),
            Some(DeliveryDivergence {
                name: "screen".into(),
                last_seen_age_ms: 100,
                last_segment_received_age_ms: 900
            })
        );
        assert_eq!(delivery_divergence(&record, 1_000, 100), None);
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
