// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded source-freshness sentinel for the Body archive status payload.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{Datelike, NaiveDate};
use serde_json::{Value, json};
use solstone_core_journal_config::read_journal_config;

use crate::day::{long_day, source_label, string_field, valid_day};
use crate::{MonthReader, coverage_month_keys};

pub(crate) const FRESHNESS_SCAN_MONTH_CAP: usize = 6;

/// Reads valid `body.freshness.quiet_days` entries from the journal configuration.
pub(crate) fn quiet_day_expectations(root: &Path) -> BTreeMap<String, i64> {
    let Ok(read) = read_journal_config(root) else {
        return BTreeMap::new();
    };
    let Some(config) = read.config else {
        return BTreeMap::new();
    };
    let Some(body) = config.get("body").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    let Some(freshness) = body.get("freshness").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    let Some(quiet_days) = freshness.get("quiet_days").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    quiet_days
        .iter()
        .filter_map(|(name, value)| {
            (!name.is_empty())
                .then(|| value.as_i64())
                .flatten()
                .filter(|value| *value > 0)
                .map(|value| (name.clone(), value))
        })
        .collect()
}

pub(crate) fn normalize_source_text(text: &str) -> String {
    text.replace('’', "'").to_lowercase()
}

pub(crate) fn source_matches_expected(label: &str, expected: &str) -> bool {
    normalize_source_text(label).contains(&normalize_source_text(expected))
}

pub(crate) fn recent_month_keys(today: NaiveDate, cap: usize) -> Vec<String> {
    let mut year = today.year();
    let mut month = today.month();
    let mut keys = Vec::with_capacity(cap);
    for _ in 0..cap {
        keys.push(format!("{year}-{month:02}"));
        if month == 1 {
            year -= 1;
            month = 12;
        } else {
            month -= 1;
        }
    }
    keys
}

pub(crate) fn expected_source_last_days(
    root: &Path,
    quiet_days: &BTreeMap<String, i64>,
    today: NaiveDate,
) -> Result<BTreeMap<String, Option<String>>, String> {
    let mut last_days = quiet_days
        .keys()
        .cloned()
        .map(|name| (name, None))
        .collect::<BTreeMap<_, Option<String>>>();
    let existing = coverage_month_keys(root).map_err(|error| error.to_string())?;
    let mut reader = MonthReader::new(root);
    for month in recent_month_keys(today, FRESHNESS_SCAN_MONTH_CAP) {
        if last_days.values().all(Option::is_some) {
            break;
        }
        if !existing.contains(&month) {
            continue;
        }
        let rows = reader
            .read_month(&month)
            .map_err(|error| error.to_string())?;
        for row in rows.iter() {
            let Some(day) = string_field(&row.day).filter(|day| valid_day(day)) else {
                continue;
            };
            let label = source_label(row);
            for (name, found) in &mut last_days {
                if found.as_ref().is_none_or(|current| day > current.as_str())
                    && source_matches_expected(&label, name)
                {
                    *found = Some(day.to_owned());
                }
            }
        }
    }
    Ok(last_days)
}

pub(crate) fn build_source_freshness(
    root: &Path,
    quiet_days: &BTreeMap<String, i64>,
    today: NaiveDate,
) -> Result<Value, String> {
    if coverage_month_keys(root)
        .map_err(|error| error.to_string())?
        .is_empty()
    {
        return Ok(json!({"sources": [], "quiet_lines": [], "quiet": false}));
    }
    let last_days = expected_source_last_days(root, quiet_days, today)?;
    let mut sources = Vec::new();
    let mut quiet_lines = Vec::new();
    for (name, threshold) in quiet_days {
        let entry = match last_days.get(name).cloned().flatten() {
            None => {
                let detail = format!("no data in the last {FRESHNESS_SCAN_MONTH_CAP} months");
                json!({
                    "name": name, "last_day": Value::Null, "last_label": Value::Null,
                    "days_since": Value::Null, "quiet_after_days": threshold, "quiet": true,
                    "detail": detail, "line": format!("{name} — {detail}"),
                })
            }
            Some(last_day) => {
                let last =
                    NaiveDate::parse_from_str(&last_day, "%Y%m%d").expect("validated source day");
                let days_since = (today - last).num_days().max(0);
                let ago = match days_since {
                    0 => "today".to_owned(),
                    1 => "1 day ago".to_owned(),
                    count => format!("{count} days ago"),
                };
                let quiet = days_since > *threshold;
                json!({
                    "name": name, "last_day": last_day,
                    "last_label": long_day(last), "days_since": days_since,
                    "quiet_after_days": threshold, "quiet": quiet,
                    "detail": format!("{} · {ago}", long_day(last)),
                    "line": if quiet { Value::String(format!("{name} last delivered {ago}")) } else { Value::Null },
                })
            }
        };
        if let Some(line) = entry["line"].as_str() {
            quiet_lines.push(Value::String(line.to_owned()));
        }
        sources.push(entry);
    }
    Ok(json!({"sources": sources, "quiet_lines": quiet_lines, "quiet": !quiet_lines.is_empty()}))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::{Map, json};

    use super::*;
    use crate::{
        BodyAggregateSeed, BodyJournalSeed, BodySeedBundle, BodySeedManifest, seed_body_journal,
    };

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-body-freshness-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn seeded(root: &Path, config: Value, day: &str, label: &str) {
        let row = serde_json::from_value::<Map<String, Value>>(json!({"dedupe_key":"source","record_type":"Signal","start_date":format!("{}T01:00:00Z", &format!("{}-{}-{}", &day[..4], &day[4..6], &day[6..])),"day":day,"source_name":label})).unwrap();
        seed_body_journal(
            root,
            &BodyJournalSeed {
                dates: BTreeSet::new(),
                day_summaries: BTreeMap::new(),
                bundles: vec![BodySeedBundle {
                    import_id: "bundle".into(),
                    source_family: "apple_health".into(),
                    manifest: BodySeedManifest::Present {
                        source_type: Some("apple_health".into()),
                        entry_count: Some(1),
                        extra: Map::new(),
                    },
                    shards: BTreeMap::from([(format!("{}-{}", &day[..4], &day[4..6]), vec![row])]),
                }],
                aggregate: BodyAggregateSeed::Direct,
                journal_config: config.as_object().cloned(),
            },
        )
        .unwrap();
    }

    #[test]
    fn matching_normalizes_apostrophes_and_is_directional() {
        assert!(source_matches_expected("Mara’s Watch", "mara's"));
        assert!(!source_matches_expected("Mara", "Mara's Watch"));
    }

    #[test]
    fn recent_months_cross_a_year_boundary() {
        assert_eq!(
            recent_month_keys(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(), 3),
            ["2026-01", "2025-12", "2025-11"]
        );
    }

    #[test]
    fn config_rejection_ladder_keeps_the_valid_entry() {
        let root = TempDir::new();
        seeded(
            root.path(),
            json!({"body":{"freshness":{"quiet_days":{"Good":2,"":4,"bad":"3","bool":true,"zero":0}}}}),
            "20260801",
            "Good Source",
        );
        assert_eq!(
            quiet_day_expectations(root.path()),
            BTreeMap::from([("Good".into(), 2)])
        );
        for config in [
            json!({"body":[]}),
            json!({"body":{"freshness":[]}}),
            json!({"body":{"freshness":{}}}),
            json!({"body":{"freshness":{"quiet_days":[]}}}),
        ] {
            let root = TempDir::new();
            seeded(root.path(), config, "20260801", "Good Source");
            assert!(quiet_day_expectations(root.path()).is_empty());
        }
    }

    #[test]
    fn injected_today_drives_all_freshness_rendering_branches_without_a_cache() {
        let root = TempDir::new();
        seeded(
            root.path(),
            json!({"body":{"freshness":{"quiet_days":{"Source":2,"Never":1}}}}),
            "20260801",
            "Source Device",
        );
        let expected = quiet_day_expectations(root.path());
        let today = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        let fresh = build_source_freshness(root.path(), &expected, today).unwrap();
        let source = fresh["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["name"] == "Source")
            .unwrap();
        let never = fresh["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["name"] == "Never")
            .unwrap();
        assert_eq!(source["detail"], "August 1, 2026 · today");
        assert_eq!(never["detail"], "no data in the last 6 months");
        let one_day =
            build_source_freshness(root.path(), &expected, today.succ_opt().unwrap()).unwrap();
        let source = one_day["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["name"] == "Source")
            .unwrap();
        assert_eq!(source["line"], Value::Null);
        let quiet =
            build_source_freshness(root.path(), &expected, today + chrono::Duration::days(3))
                .unwrap();
        let source = quiet["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["name"] == "Source")
            .unwrap();
        assert_eq!(source["line"], "Source last delivered 3 days ago");
        let beyond_cap = build_source_freshness(
            root.path(),
            &expected,
            NaiveDate::from_ymd_opt(2027, 3, 1).unwrap(),
        )
        .unwrap();
        let source = beyond_cap["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["name"] == "Source")
            .unwrap();
        assert_eq!(source["last_day"], Value::Null);
    }

    #[test]
    fn source_scan_stops_after_all_expected_sources_are_found() {
        let root = TempDir::new();
        seeded(
            root.path(),
            json!({"body":{"freshness":{"quiet_days":{"Source":1}}}}),
            "20260801",
            "Source Device",
        );
        let unreadable_older = root.path().join("imports/older/normalized/2026-07.jsonl");
        fs::create_dir_all(unreadable_older.parent().unwrap()).unwrap();
        fs::write(unreadable_older, "not json\n").unwrap();
        let expected = quiet_day_expectations(root.path());
        assert_eq!(
            expected_source_last_days(
                root.path(),
                &expected,
                NaiveDate::from_ymd_opt(2026, 8, 2).unwrap(),
            )
            .unwrap(),
            BTreeMap::from([("Source".to_owned(), Some("20260801".to_owned()))])
        );
    }
}
