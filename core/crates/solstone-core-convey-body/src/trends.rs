// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{Duration, NaiveDate};

pub type TrendsSignature = (u128, u64, u128, u64);
type TrendsCache = BTreeMap<String, (TrendsSignature, Arc<TrendsPayload>)>;

static TRENDS_CACHE: OnceLock<Mutex<TrendsCache>> = OnceLock::new();

pub const TYPICAL_BASELINE_DAYS: i64 = 90;
pub const TYPICAL_MIN_VALUES: usize = 14;
const TYPICAL_SIGNAL_KEYS: [&str; 4] = ["readiness", "sleep_score", "asleep_minutes", "resting_hr"];

#[derive(Debug, Clone, PartialEq)]
pub struct TrendsPayload {
    pub signals: Vec<TrendSignal>,
    pub annotations: Vec<TrendAnnotation>,
    pub generated_at_day: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrendSignal {
    pub key: String,
    pub label: String,
    pub unit_label: String,
    pub daily: Vec<(String, f64)>,
    pub coverage: TrendCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrendCoverage {
    pub first_day: String,
    pub last_day: String,
    pub days: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrendAnnotation {
    pub day: String,
    pub label: String,
}

#[derive(Debug)]
pub enum TrendsCacheError {
    CachePoisoned,
}

impl std::fmt::Display for TrendsCacheError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CachePoisoned => formatter.write_str("trends cache lock was poisoned"),
        }
    }
}
impl std::error::Error for TrendsCacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CachePoisoned => None,
        }
    }
}

pub fn read_trends_cache(
    database_path: impl AsRef<Path>,
    signature: TrendsSignature,
) -> Result<Option<Arc<TrendsPayload>>, TrendsCacheError> {
    let key = database_path.as_ref().to_string_lossy().into_owned();
    let cache = TRENDS_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    let cache = cache.lock().map_err(|_| TrendsCacheError::CachePoisoned)?;
    Ok(cache.get(&key).and_then(|(cached_signature, payload)| {
        (*cached_signature == signature).then(|| Arc::clone(payload))
    }))
}

pub fn replace_trends_cache(
    database_path: impl AsRef<Path>,
    signature: TrendsSignature,
    payload: TrendsPayload,
) -> Result<Arc<TrendsPayload>, TrendsCacheError> {
    let key = database_path.as_ref().to_string_lossy().into_owned();
    let payload = Arc::new(payload);
    let cache = TRENDS_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    cache
        .lock()
        .map_err(|_| TrendsCacheError::CachePoisoned)?
        .insert(key, (signature, Arc::clone(&payload)));
    Ok(payload)
}

pub fn typical_by_signal(payload: Option<&TrendsPayload>, day: &str) -> BTreeMap<String, f64> {
    let Some(payload) = payload else {
        return BTreeMap::new();
    };
    let Ok(day) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return BTreeMap::new();
    };
    let window_start = (day - Duration::days(TYPICAL_BASELINE_DAYS))
        .format("%Y%m%d")
        .to_string();
    let day = day.format("%Y%m%d").to_string();
    let mut typical = BTreeMap::new();
    for signal in &payload.signals {
        if !TYPICAL_SIGNAL_KEYS.contains(&signal.key.as_str()) {
            continue;
        }
        let mut values = signal
            .daily
            .iter()
            .filter_map(|(value_day, value)| {
                (window_start <= *value_day && *value_day < day).then_some(*value)
            })
            .collect::<Vec<_>>();
        if values.len() < TYPICAL_MIN_VALUES {
            continue;
        }
        values.sort_by(f64::total_cmp);
        let middle = values.len() / 2;
        let median = if values.len().is_multiple_of(2) {
            (values[middle - 1] + values[middle]) / 2.0
        } else {
            values[middle]
        };
        typical.insert(signal.key.clone(), median);
    }
    typical
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use chrono::{Duration, NaiveDate};

    use super::*;

    fn signal(key: &str, unit_label: &str, daily: Vec<(String, f64)>) -> TrendSignal {
        let first_day = daily
            .first()
            .map_or_else(String::new, |(day, _)| day.clone());
        let last_day = daily
            .last()
            .map_or_else(String::new, |(day, _)| day.clone());
        TrendSignal {
            key: key.to_owned(),
            label: format!("{key} label"),
            unit_label: unit_label.to_owned(),
            coverage: TrendCoverage {
                first_day,
                last_day,
                days: daily.len(),
            },
            daily,
        }
    }
    fn payload(label: &str) -> TrendsPayload {
        TrendsPayload {
            signals: vec![signal("readiness", "", vec![])],
            annotations: vec![TrendAnnotation {
                day: "20240101".to_owned(),
                label: label.to_owned(),
            }],
            generated_at_day: "20240101".to_owned(),
        }
    }
    fn days_before(day: NaiveDate, count: usize) -> Vec<(String, f64)> {
        (1..=count)
            .map(|offset| {
                (
                    (day - Duration::days(offset as i64))
                        .format("%Y%m%d")
                        .to_string(),
                    offset as f64,
                )
            })
            .collect()
    }

    #[test]
    fn payload_shape_and_typical_filter_match_python_contract() {
        let target = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        let mut readiness = days_before(target, 14);
        readiness.push(("20240601".to_owned(), 999.0));
        let signals = [
            ("resting_hr", "bpm", days_before(target, 5)),
            ("vascular_age", "", days_before(target, 14)),
            ("asleep_minutes", "h", days_before(target, 13)),
            ("sleep_score", "", days_before(target, 14)),
            ("readiness", "", readiness),
            ("temp_deviation", "°C", days_before(target, 14)),
            ("stress_high_minutes", "h", days_before(target, 14)),
            ("steps", "steps", days_before(target, 14)),
            ("body_mass", "lb", days_before(target, 14)),
            ("glucose_avg", "mg/dL", days_before(target, 14)),
        ]
        .into_iter()
        .map(|(key, unit, values)| signal(key, unit, values))
        .collect::<Vec<_>>();
        let payload = TrendsPayload {
            signals,
            annotations: vec![TrendAnnotation {
                day: "20240501".to_owned(),
                label: "source begins".to_owned(),
            }],
            generated_at_day: "20240601".to_owned(),
        };
        assert_eq!(payload.signals.len(), 10);
        assert_eq!(payload.signals[1].unit_label, "");
        assert_eq!(payload.annotations[0].label, "source begins");
        let typical = typical_by_signal(Some(&payload), "20240601");
        assert_eq!(
            typical.keys().cloned().collect::<Vec<_>>(),
            ["readiness", "sleep_score"]
        );
        assert_eq!(typical["readiness"], 7.5);
        assert!(!typical.contains_key("asleep_minutes"));
        assert!(!typical.contains_key("vascular_age"));
    }

    #[test]
    fn cache_replaces_stale_signature_without_accumulating_entries() {
        let path = Path::new("/synthetic/trends.sqlite");
        let cache = TRENDS_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
        cache.lock().unwrap().clear();
        let first = replace_trends_cache(path, (1, 1, 0, 0), payload("first")).unwrap();
        let repeated = read_trends_cache(path, (1, 1, 0, 0)).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &repeated));
        let changed = replace_trends_cache(path, (2, 1, 0, 0), payload("changed")).unwrap();
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(read_trends_cache(path, (1, 1, 0, 0)).unwrap().is_none());
        assert_eq!(cache.lock().unwrap().len(), 1);
    }
}
