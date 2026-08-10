// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

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
                .min_by_key(|(_, record)| record.created_at().unwrap_or(0))
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

    #[test]
    fn plan_keeps_oldest_and_sums_only_numeric_non_boolean_stats() {
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
        assert_eq!(plan[0].survivor_prefix, "aaaaaaaa");
        assert_eq!(plan[0].revoked_prefixes, vec!["bbbbbbbb"]);
        assert_eq!(plan[0].stats["segments_received"], 3);
        assert_eq!(plan[0].stats["bytes_received"], 7);
        assert_eq!(plan[0].stats["extra"], 1.5);
        assert!(plan[0].stats.get("flag").is_none());
    }
}
