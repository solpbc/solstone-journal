// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cmp::Reverse;

use serde_json::{Map, Value};

use super::record::ObserverRecord;

#[derive(Debug, Clone, PartialEq)]
pub struct ReconcilePlan {
    pub name: String,
    pub survivor_prefix: String,
    pub revoked_prefixes: Vec<String>,
    pub stats: Map<String, Value>,
}

pub fn reconcile_plan(records: &[ObserverRecord]) -> Vec<ReconcilePlan> {
    let mut groups: Vec<(String, Vec<&ObserverRecord>)> = Vec::new();
    for record in records {
        if !record.revoked() {
            let name = record.name().unwrap_or_default();
            if let Some((_, group)) = groups.iter_mut().find(|(existing, _)| existing == name) {
                group.push(record);
            } else {
                groups.push((name.to_owned(), vec![record]));
            }
        }
    }
    groups
        .into_iter()
        .filter_map(|(_, records)| {
            if records.len() < 2 {
                return None;
            }
            let survivor_index = records
                .iter()
                .enumerate()
                .min_by_key(|(index, record)| {
                    let bound_rank = if record.device_binding_device().is_some() {
                        0
                    } else {
                        1
                    };
                    (
                        bound_rank,
                        Reverse(segments_received(record)),
                        Reverse(record.last_segment_received_at()),
                        Reverse(record.last_seen()),
                        record.created_at(),
                        *index,
                    )
                })
                .map(|(index, _)| index)
                .expect("nonempty group");
            let survivor = records[survivor_index];
            let stats = aggregate_stats(&records);
            Some(ReconcilePlan {
                name: survivor.name().unwrap_or_default().to_owned(),
                survivor_prefix: survivor.prefix(),
                revoked_prefixes: records
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != survivor_index)
                    .map(|(_, record)| record.prefix())
                    .collect(),
                stats,
            })
        })
        .collect()
}

fn segments_received(record: &ObserverRecord) -> i64 {
    record
        .stats()
        .and_then(|stats| stats.get("segments_received"))
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
        .map(|number| number as i64)
        .unwrap_or(0)
}

pub fn aggregate_stats(records: &[&ObserverRecord]) -> Map<String, Value> {
    let mut totals: Map<String, Value> = Map::new();
    for record in records {
        let Some(stats) = record.stats() else {
            continue;
        };
        for (key, value) in stats {
            let Some(number) = value.as_f64() else {
                continue;
            };
            if value.is_boolean() {
                continue;
            }
            let total = totals.get(key).and_then(Value::as_f64).unwrap_or(0.0) + number;
            let value = if total.fract() == 0.0 {
                Value::from(total as i64)
            } else {
                Value::from(total)
            };
            totals.insert(key.to_owned(), value);
        }
    }
    totals
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(key: &str, created_at: i64, stats: Value) -> ObserverRecord {
        ObserverRecord::from_value(
            json!({"key":key,"name":"same","created_at":created_at,"stats":stats}),
        )
        .expect("record")
    }

    fn named(value: Value) -> ObserverRecord {
        ObserverRecord::from_value(value).expect("record")
    }

    fn cert(digit: char) -> Value {
        json!({"device": format!("sha256:{}", digit.to_string().repeat(64)), "kind": "cert"})
    }

    #[test]
    fn ac1_empty_unbound_loses_when_created_at_ties() {
        let records = vec![
            named(
                json!({"key":"emptykey1","name":"rokid","created_at":50,"stats":{"segments_received":0}}),
            ),
            named(
                json!({"key":"livekey01","name":"rokid","created_at":50,"stats":{"segments_received":94}}),
            ),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "livekey0");
        assert_eq!(plan[0].revoked_prefixes, vec!["emptykey"]);
    }

    #[test]
    fn ac2_sole_binding_beats_every_other_key() {
        // RED today (keep-oldest picks the unbound older created_at) and would
        // also be red against most-segments-always (unbound has 99 vs bound 1).
        let records = vec![
            named(json!({
                "key":"boundkey1","name":"rokid",
                "device_binding":cert('a'),
                "created_at":200,
                "last_segment_received_at":10,
                "last_seen":10,
                "stats":{"segments_received":1}
            })),
            named(json!({
                "key":"freekey01","name":"rokid",
                "created_at":100,
                "last_segment_received_at":999,
                "last_seen":999,
                "stats":{"segments_received":99}
            })),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "boundkey");
        assert_eq!(plan[0].revoked_prefixes, vec!["freekey0"]);
    }

    #[test]
    fn ac3_fatter_bound_wins_among_many_bindings() {
        let records = vec![
            named(json!({
                "key":"certakey1","name":"rokid",
                "device_binding":cert('a'),
                "created_at":200,
                "stats":{"segments_received":10}
            })),
            named(json!({
                "key":"certbkey1","name":"rokid",
                "device_binding":cert('b'),
                "created_at":300,
                "stats":{"segments_received":20}
            })),
            named(json!({
                "key":"unboundx1","name":"rokid",
                "created_at":100,
                "stats":{"segments_received":3000}
            })),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "certbkey");
        assert_eq!(plan[0].revoked_prefixes, vec!["certakey", "unboundx"]);
    }

    #[test]
    fn ac4_unbound_group_most_segments_wins() {
        let records = vec![
            named(
                json!({"key":"thinkey01","name":"rokid-late","created_at":1,"stats":{"segments_received":10}}),
            ),
            named(
                json!({"key":"fatkey001","name":"rokid-late","created_at":2,"stats":{"segments_received":94}}),
            ),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "fatkey00");
        assert_eq!(plan[0].revoked_prefixes, vec!["thinkey0"]);
    }

    #[test]
    fn plan_prefers_most_segments_and_sums_only_numeric_non_boolean_stats() {
        let records = vec![
            record(
                "bbbbbbbb1",
                2,
                json!({"segments_received":2,"bytes_received":3,"flag":true,"note":"x"}),
            ),
            record(
                "aaaaaaaa1",
                1,
                json!({"segments_received":1,"bytes_received":4,"extra":1.5}),
            ),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "bbbbbbbb");
        assert_eq!(plan[0].revoked_prefixes, vec!["aaaaaaaa"]);
        assert_eq!(plan[0].stats["segments_received"], 3);
        assert_eq!(plan[0].stats["bytes_received"], 7);
        assert_eq!(plan[0].stats["extra"], 1.5);
        assert!(plan[0].stats.get("flag").is_none());
    }

    #[test]
    fn ac5_segment_tie_later_last_segment_received_at_wins() {
        // Keep-oldest would pick earlierkey (created_at 100). A last-seen-first
        // ordering would also pick earlierkey (last_seen 999). Later
        // last_segment_received_at is the only key that selects laterkey.
        let records = vec![
            named(json!({
                "key":"laterkey1","name":"rokid",
                "created_at":300,
                "last_segment_received_at":200,
                "last_seen":10,
                "stats":{"segments_received":5}
            })),
            named(json!({
                "key":"earlierk1","name":"rokid",
                "created_at":100,
                "last_segment_received_at":100,
                "last_seen":999,
                "stats":{"segments_received":5}
            })),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "laterkey");
        assert_eq!(plan[0].revoked_prefixes, vec!["earlierk"]);
    }

    #[test]
    fn ac6_missing_last_segment_received_at_sorts_older() {
        let records = vec![
            named(json!({
                "key":"seenkey01","name":"rokid",
                "created_at":200,
                "last_seen":50,
                "stats":{"segments_received":5}
            })),
            named(json!({
                "key":"blindkey1","name":"rokid",
                "created_at":100,
                "stats":{"segments_received":5}
            })),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "seenkey0");
        assert_eq!(plan[0].revoked_prefixes, vec!["blindkey"]);
    }

    #[test]
    fn ac7_older_created_at_wins_when_recency_is_missing() {
        let records = vec![
            named(
                json!({"key":"newerkey1","name":"rokid","created_at":200,"stats":{"segments_received":5}}),
            ),
            named(
                json!({"key":"olderkey1","name":"rokid","created_at":100,"stats":{"segments_received":5}}),
            ),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "olderkey");
        assert_eq!(plan[0].revoked_prefixes, vec!["newerkey"]);
    }

    #[test]
    fn ac8_different_names_do_not_merge() {
        let records = vec![
            named(
                json!({"key":"iphone01","name":"iphone.mobile","created_at":1,"stats":{"segments_received":1}}),
            ),
            named(
                json!({"key":"idfvkey1","name":"iphone-abc123.mobile","created_at":1,"stats":{"segments_received":1}}),
            ),
        ];
        assert!(reconcile_plan(&records).is_empty());
    }

    #[test]
    fn ac11_revoked_records_stay_out_of_the_group() {
        let records = vec![
            named(
                json!({"key":"revokeda1","name":"rokid","revoked":true,"created_at":1,"stats":{"segments_received":10}}),
            ),
            named(
                json!({"key":"revokedb1","name":"rokid","revoked":true,"created_at":2,"stats":{"segments_received":20}}),
            ),
            named(
                json!({"key":"liveonly1","name":"rokid","created_at":3,"stats":{"segments_received":1}}),
            ),
        ];
        assert!(reconcile_plan(&records).is_empty());
    }

    #[test]
    fn float_segments_received_beats_zero() {
        let records = vec![
            named(
                json!({"key":"zerokey01","name":"rokid","created_at":1,"stats":{"segments_received":0}}),
            ),
            named(
                json!({"key":"floatkey1","name":"rokid","created_at":2,"stats":{"segments_received":94.0}}),
            ),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "floatkey");
        assert_eq!(plan[0].revoked_prefixes, vec!["zerokey0"]);
    }

    #[test]
    fn missing_created_at_sorts_older_than_stored_zero() {
        let records = vec![
            named(
                json!({"key":"zerots001","name":"rokid","created_at":0,"stats":{"segments_received":1}}),
            ),
            named(json!({"key":"missing01","name":"rokid","stats":{"segments_received":1}})),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "missing0");
        assert_eq!(plan[0].revoked_prefixes, vec!["zerots00"]);
    }

    #[test]
    fn full_key_tie_keeps_first_in_group_order() {
        let records = vec![
            named(
                json!({"key":"firstkey1","name":"rokid","created_at":1,"stats":{"segments_received":1}}),
            ),
            named(
                json!({"key":"secondky1","name":"rokid","created_at":1,"stats":{"segments_received":1}}),
            ),
        ];
        let plan = reconcile_plan(&records);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].survivor_prefix, "firstkey");
        assert_eq!(plan[0].revoked_prefixes, vec!["secondky"]);
    }
}
