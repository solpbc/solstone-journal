// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_system::work_inventory::WorkInventory;
use std::collections::BTreeMap;

/// Non-retryable holds share a day-level action. Send one counted row per
/// day/reason instead of repeating that action for every recording. Explicit
/// retry units stay separate, with their exact identities and evidence.
pub(crate) fn summarize(mut inventory: WorkInventory) -> Value {
    let mut groups: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut holds = Vec::new();
    for hold in std::mem::take(&mut inventory.holds) {
        if hold.retry_unit.is_some() {
            holds.push(serde_json::to_value(hold).expect("serializable work hold"));
        } else {
            *groups.entry((hold.day, hold.reason)).or_default() += 1;
        }
    }
    holds.extend(groups.into_iter().map(|((day, reason), count)| {
        json!({
            "day": day, "reason": reason, "count": count,
        })
    }));
    let mut view = serde_json::to_value(inventory).expect("serializable inventory");
    view["holds"] = Value::Array(holds);
    view
}

/// Additive Band-4 (`WorkBand::Import` = index 3) backfill progress summary for
/// `/app/health/api/state` (`work.backfill`), keeping calm backfill progress
/// distinct from live capture or repair failure counts.
pub(crate) fn backfill_summary(
    inventory: Option<&WorkInventory>,
    paused: bool,
    index_pending: u64,
) -> Value {
    let band = inventory
        .map(|inv| &inv.bands[3])
        .cloned()
        .unwrap_or_default();
    let landed_segments = band.segments_complete + band.segments_owed;
    let indexed_segments = landed_segments.saturating_sub(index_pending);
    let mut backfill_days = BTreeMap::<&str, ()>::new();
    if let Some(inv) = inventory {
        for hold in &inv.holds {
            if hold.band == solstone_core_journal_config::work_priority::WorkBand::Import {
                backfill_days.insert(hold.day.as_str(), ());
            }
        }
    }
    let oldest_held_day = backfill_days.keys().next().map(|s| (*s).to_owned());
    let newest_held_day = backfill_days.keys().next_back().map(|s| (*s).to_owned());
    json!({
        "paused": paused,
        "landed_segments": landed_segments,
        "indexed_segments": indexed_segments,
        "interpreted_segments": band.segments_complete,
        "owed_segments": band.segments_owed,
        "daily_units_complete": band.daily_units_complete,
        "daily_units_owed": band.daily_units_owed,
        "activities_owed": band.activities_owed,
        "eligible": band.eligible,
        "permission_held": band.permission_held,
        "oldest_held_day": oldest_held_day,
        "newest_held_day": newest_held_day,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_system::work_inventory::WorkHold;
    #[test]
    fn counts_repeated_holds_without_collapsing_retry_identities() {
        let hold: WorkHold = serde_json::from_value(json!({
            "day":"20261001", "band":"now", "reason":"spending_confirmation"
        }))
        .unwrap();
        let mut inventory = WorkInventory {
            holds: vec![hold.clone(); 1000],
            ..WorkInventory::default()
        };
        for id in ["one", "two"] {
            let mut retry = hold.clone();
            retry.retry_unit = Some(id.into());
            inventory.holds.push(retry);
        }
        let view = summarize(inventory);
        let holds = view["holds"].as_array().unwrap();
        assert_eq!(holds.len(), 3);
        assert_eq!(holds[0]["retry_unit"], "one");
        assert_eq!(holds[1]["retry_unit"], "two");
        assert_eq!(holds[2]["count"], 1000);
    }
}
