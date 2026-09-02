// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

pub const SLEEP_SESSION_GAP_MINUTES: i64 = 60;
pub type SleepInterval = (NaiveDateTime, NaiveDateTime);
pub type SleepStagedInterval = (NaiveDateTime, NaiveDateTime, Option<String>);

#[derive(Debug, Clone, PartialEq)]
pub struct DaySleep {
    pub source: String,
    pub other_sources: Vec<String>,
    pub main: Option<SleepInterval>,
    pub naps: Vec<SleepInterval>,
    pub in_bed_minutes: Option<f64>,
    pub asleep_minutes: Option<f64>,
    pub has_stage_detail: bool,
}

pub fn sleep_stage_kind(value: Option<&str>) -> &'static str {
    let Some(value) = value else { return "unknown" };
    let value = value.to_lowercase();
    if value.contains("asleep") {
        "asleep"
    } else if value.contains("awake") {
        "awake"
    } else if value.contains("inbed") || value.contains("in_bed") || value.contains("in bed") {
        "in_bed"
    } else {
        "unknown"
    }
}

fn asleep_minutes_in_session(
    session: SleepInterval,
    staged: &[SleepStagedInterval],
) -> Option<f64> {
    let clipped = staged
        .iter()
        .filter_map(|(start, end, stage)| {
            if sleep_stage_kind(stage.as_deref()) != "asleep" {
                return None;
            }
            let start = (*start).max(session.0);
            let end = (*end).min(session.1);
            (end > start).then_some((start, end))
        })
        .collect::<Vec<_>>();
    if clipped.is_empty() {
        return None;
    }
    Some(
        merge_sleep_sessions(clipped, 0)
            .into_iter()
            .map(|(start, end)| (end - start).num_seconds() as f64 / 60.0)
            .sum(),
    )
}

pub fn merge_sleep_sessions(
    intervals: impl IntoIterator<Item = SleepInterval>,
    gap_minutes: i64,
) -> Vec<SleepInterval> {
    let mut ordered = intervals.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|interval| interval.0);
    let gap = chrono::Duration::minutes(gap_minutes);
    let mut merged = Vec::<SleepInterval>::new();
    for (start, mut end) in ordered {
        if end < start {
            end = start;
        }
        if let Some((_, last_end)) = merged.last_mut()
            && start <= *last_end + gap
        {
            if end > *last_end {
                *last_end = end;
            }
        } else {
            merged.push((start, end));
        }
    }
    merged
}

pub fn pick_main_session(
    sessions: impl IntoIterator<Item = SleepInterval>,
    day: NaiveDate,
) -> (Option<SleepInterval>, Vec<SleepInterval>) {
    let mut sessions = sessions.into_iter().collect::<Vec<_>>();
    sessions.sort_by_key(|session| session.1);
    let noon = NaiveTime::from_hms_opt(12, 0, 0).expect("valid noon");
    let mut main = None;
    let mut naps = Vec::new();
    for session in sessions {
        if session.1.date() != day {
            continue;
        }
        let crosses_midnight = session.0.date() < day;
        let ends_morning = session.1.time() <= noon;
        if main.is_none() && (crosses_midnight || ends_morning) {
            main = Some(session);
        } else if session.0.date() == day {
            naps.push(session);
        }
    }
    (main, naps)
}

pub fn pick_day_sleep(
    intervals_by_source: &BTreeMap<String, Vec<SleepStagedInterval>>,
    day: NaiveDate,
    gap_minutes: i64,
) -> Option<DaySleep> {
    let mut per_source = BTreeMap::<String, (Option<SleepInterval>, Vec<SleepInterval>)>::new();
    for (source, staged) in intervals_by_source {
        let sessions = merge_sleep_sessions(
            staged.iter().map(|(start, end, _)| (*start, *end)),
            gap_minutes,
        );
        let (main, naps) = pick_main_session(sessions, day);
        if main.is_some() || !naps.is_empty() {
            per_source.insert(source.clone(), (main, naps));
        }
    }
    let mut primary: Option<(&str, i64)> = None;
    for (source, (main, naps)) in &per_source {
        let coverage = main.map_or_else(
            || {
                naps.iter()
                    .map(|(start, end)| (*end - *start).num_seconds())
                    .sum()
            },
            |(start, end)| (end - start).num_seconds(),
        );
        if primary.is_none_or(|(_, best)| coverage > best) {
            primary = Some((source, coverage));
        }
    }
    let (primary, _) = primary?;
    let (main, naps) = per_source.get(primary).expect("selected primary exists");
    let in_bed_minutes = main.map(|(start, end)| (end - start).num_seconds() as f64 / 60.0);
    let staged = intervals_by_source
        .get(primary)
        .expect("selected source exists");
    let staged_asleep = main.and_then(|session| asleep_minutes_in_session(session, staged));
    let (asleep_minutes, has_stage_detail) = match (in_bed_minutes, staged_asleep) {
        (Some(_), Some(asleep)) => (Some(asleep), true),
        (Some(in_bed), None) => (Some(in_bed), false),
        (None, _) => (None, false),
    };
    Some(DaySleep {
        source: primary.to_owned(),
        other_sources: per_source
            .keys()
            .filter(|source| source.as_str() != primary)
            .cloned()
            .collect(),
        main: *main,
        naps: naps.clone(),
        in_bed_minutes,
        asleep_minutes,
        has_stage_detail,
    })
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::*;

    fn moment(value: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M").unwrap()
    }
    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y%m%d").unwrap()
    }
    fn staged(start: &str, end: &str, stage: Option<&str>) -> SleepStagedInterval {
        (moment(start), moment(end), stage.map(str::to_owned))
    }

    #[test]
    fn sessions_obey_gap_boundaries_and_union_overlap() {
        assert_eq!(
            merge_sleep_sessions(
                [
                    (moment("2024-01-01 20:00"), moment("2024-01-01 21:00")),
                    (moment("2024-01-01 22:00"), moment("2024-01-01 23:00"))
                ],
                SLEEP_SESSION_GAP_MINUTES
            )
            .len(),
            1
        );
        assert_eq!(
            merge_sleep_sessions(
                [
                    (moment("2024-01-01 20:00"), moment("2024-01-01 21:00")),
                    (moment("2024-01-01 22:01"), moment("2024-01-01 23:00"))
                ],
                SLEEP_SESSION_GAP_MINUTES
            )
            .len(),
            2
        );
        let merged = merge_sleep_sessions(
            [
                (moment("2024-01-01 20:00"), moment("2024-01-01 22:00")),
                (moment("2024-01-01 21:00"), moment("2024-01-01 23:00")),
            ],
            0,
        );
        assert_eq!((merged[0].1 - merged[0].0).num_minutes(), 180);
    }

    #[test]
    fn main_naps_and_primary_source_follow_canonical_rules() {
        let target = day("20240102");
        let morning = (moment("2024-01-02 09:00"), moment("2024-01-02 10:00"));
        let afternoon = (moment("2024-01-02 14:00"), moment("2024-01-02 15:00"));
        assert_eq!(
            pick_main_session([morning, afternoon], target),
            (Some(morning), vec![afternoon])
        );
        let intervals = BTreeMap::from([
            (
                "Alpha".to_owned(),
                vec![staged("2024-01-01 22:00", "2024-01-02 06:00", None)],
            ),
            (
                "Beta".to_owned(),
                vec![staged("2024-01-01 21:00", "2024-01-02 06:00", None)],
            ),
        ]);
        assert_eq!(
            pick_day_sleep(&intervals, target, SLEEP_SESSION_GAP_MINUTES)
                .unwrap()
                .source,
            "Beta"
        );
        let tied = BTreeMap::from([
            (
                "Alpha".to_owned(),
                vec![staged("2024-01-01 22:00", "2024-01-02 06:00", None)],
            ),
            (
                "Beta".to_owned(),
                vec![staged("2024-01-01 22:00", "2024-01-02 06:00", None)],
            ),
        ]);
        let tied_sleep = pick_day_sleep(&tied, target, SLEEP_SESSION_GAP_MINUTES).unwrap();
        assert_eq!(tied_sleep.source, "Alpha");
        assert_eq!(tied_sleep.other_sources, vec!["Beta".to_owned()]);
    }

    #[test]
    fn staged_asleep_minutes_do_not_fill_the_whole_session() {
        let target = day("20240102");
        let detailed = BTreeMap::from([(
            "Source".to_owned(),
            vec![
                staged("2024-01-01 22:00", "2024-01-02 06:00", Some("in bed")),
                staged("2024-01-01 23:00", "2024-01-02 05:00", Some("asleep core")),
            ],
        )]);
        let sleep = pick_day_sleep(&detailed, target, SLEEP_SESSION_GAP_MINUTES).unwrap();
        assert!(sleep.asleep_minutes < sleep.in_bed_minutes);
        assert!(sleep.has_stage_detail);
        let bare = BTreeMap::from([(
            "Source".to_owned(),
            vec![staged("2024-01-01 22:00", "2024-01-02 06:00", None)],
        )]);
        let sleep = pick_day_sleep(&bare, target, SLEEP_SESSION_GAP_MINUTES).unwrap();
        assert_eq!(sleep.asleep_minutes, sleep.in_bed_minutes);
        assert!(!sleep.has_stage_detail);
    }
}
