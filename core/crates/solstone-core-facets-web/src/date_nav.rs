// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde_json::{Value, json};

// Deliberate local copy: sibling ports keep date-nav helpers private and this timeline-only arity must stay three-key.
pub fn date_nav_index(counts: &BTreeMap<String, usize>) -> Value {
    let mut months = BTreeMap::<String, usize>::new();
    let days = counts
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(day, count)| {
            *months.entry(day[..6].to_owned()).or_default() += count;
            day.clone()
        })
        .collect::<Vec<_>>();
    let coverage = days
        .first()
        .zip(days.last())
        .map(|(start, end)| json!({"start": start, "end": end}));
    json!({"coverage": coverage, "months": months})
}

pub fn day_grid_payload(counts: &BTreeMap<String, usize>, watermark: Option<&str>) -> Value {
    let mut days = serde_json::Map::new();
    let mut pending = serde_json::Map::new();
    for (day, count) in counts {
        let target = if watermark.is_some_and(|mark| day.as_str() <= mark) {
            &mut days
        } else {
            &mut pending
        };
        target.insert(day.clone(), json!(count));
    }
    let all_days = counts.keys().collect::<Vec<_>>();
    let coverage = all_days
        .first()
        .zip(all_days.last())
        .map(|(start, end)| json!({"start": start, "end": end}));
    json!({"coverage": coverage, "days": days, "pending": pending})
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{date_nav_index, day_grid_payload};

    #[test]
    fn ac7_established_empty_grid_has_exactly_three_keys() {
        let payload = day_grid_payload(&BTreeMap::new(), None);
        assert_eq!(
            payload,
            json!({"coverage": null, "days": {}, "pending": {}})
        );
        assert_eq!(payload.as_object().expect("object").len(), 3);
    }

    #[test]
    fn ac8_none_watermark_leaves_all_days_pending_and_nav_drops_zeroes() {
        let counts = BTreeMap::from([("20260510".to_owned(), 3), ("20260511".to_owned(), 1)]);
        let payload = day_grid_payload(&counts, None);
        assert_eq!(payload["days"], json!({}));
        assert_eq!(payload["pending"], json!({"20260510": 3, "20260511": 1}));
        let index = date_nav_index(&BTreeMap::from([
            ("20260509".to_owned(), 0),
            ("20260510".to_owned(), 3),
        ]));
        assert_eq!(
            index,
            json!({"coverage": {"start": "20260510", "end": "20260510"}, "months": {"202605": 3}})
        );
    }
}
