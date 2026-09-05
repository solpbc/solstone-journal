// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{Datelike, NaiveDate};
use serde_json::{Value, json};
use solstone_core_timeline::{DayTimelineV1, HourTimelineV1, MasterTimelineV1, MonthTimelineV1};

use crate::{clock::Clock, segments::day_segment_counts};

use super::projection::{self, ArtifactProjection};

pub(super) fn master(root: &Path) -> ArtifactProjection<MasterTimelineV1> {
    projection::master(root)
}

pub fn overview(root: &Path, clock: &Clock) -> Result<Value, std::io::Error> {
    let projection = master(root);
    let counts = day_segment_counts(root, None);
    let today = clock.now().date();
    let months = coverage_months(projection.value.as_ref(), &counts, today)
        .into_iter()
        .map(|ym| {
            let (year, month_num) = month_parts(&ym)?;
            let month = projection
                .value
                .as_ref()
                .and_then(|master| master.months.get(&ym));
            Ok(json!({
                "ym": ym,
                "year": year,
                "month_num": month_num,
                "days_in_month": days_in_month(&ym)?,
                "first_weekday": first_weekday(&ym)?,
                "day_count": month.map_or(0, |month| month.day_count),
                "days_with_data": month.map(days_with_data).unwrap_or_default(),
            }))
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    Ok(json!({
        "now": clock.now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        "today": today.format("%Y%m%d").to_string(),
        "status": projection.status.as_str(),
        "artifact_outcome": projection.outcome.as_str(),
        "generated_at_ms": projection.value.as_ref().map(|master| master.generated_at_ms),
        "provenance": projection.value.as_ref().and_then(|master| master.year_curation.provenance.as_ref()),
        "data_through": rollup_watermark(&projection),
        "months": months,
    }))
}

pub fn month(root: &Path, ym: &str) -> Result<Value, std::io::Error> {
    let projection = master(root);
    let month = projection
        .value
        .as_ref()
        .and_then(|master| master.months.get(ym));
    let days = month
        .map(|month| {
            month
                .days
                .iter()
                .map(|(day, value)| (day.clone(), day_in_master(day, value)))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    Ok(json!({
        "ym": ym,
        "status": projection.status.as_str(),
        "artifact_outcome": projection.outcome.as_str(),
        "generated_at_ms": projection.value.as_ref().map(|master| master.generated_at_ms),
        "provenance": projection.value.as_ref().and_then(|master| master.year_curation.provenance.as_ref()),
        "day_count": month.map_or(0, |month| month.day_count),
        "days_with_data": month.map(days_with_data).unwrap_or_default(),
        "days": days,
    }))
}

pub fn day_rollup(root: &Path, day: &str) -> Value {
    let projection = projection::day(root, day);
    let value = projection.value.as_ref();
    let hours = value
        .map(|day| {
            day.hours
                .iter()
                .map(|(hour, value)| (hour.clone(), hour_payload(value)))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    json!({
        "day": day,
        "status": projection.status.as_str(),
        "artifact_outcome": projection.outcome.as_str(),
        "generated_at_ms": value.map(|day| day.generated_at_ms),
        "provenance": value.and_then(|day| day.day_curation.provenance.as_ref()),
        "day_top": value
            .map(|day| day.day_curation.picks.clone())
            .unwrap_or_default(),
        "day_rationale": value.map(|day| day.day_curation.rationale.as_str()).unwrap_or_default(),
        "hours": hours,
    })
}

pub fn rollup_watermark(projection: &ArtifactProjection<MasterTimelineV1>) -> Option<String> {
    (projection.status == projection::TimelineStatus::Current)
        .then_some(projection.value.as_ref())
        .flatten()
        .into_iter()
        .flat_map(|master| master.months.values())
        .flat_map(|month| month.days.keys())
        .max()
        .cloned()
}

pub fn coverage_months(
    master: Option<&MasterTimelineV1>,
    counts: &BTreeMap<String, usize>,
    today: NaiveDate,
) -> Vec<String> {
    let mut keys = master
        .into_iter()
        .flat_map(|master| master.months.keys())
        .filter(|ym| ym.len() == 6 && ym.bytes().all(|byte| byte.is_ascii_digit()))
        .cloned()
        .collect::<Vec<_>>();
    keys.extend(
        counts
            .keys()
            .filter(|day| day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()))
            .map(|day| day[..6].to_owned()),
    );
    if keys.is_empty() {
        return vec![today.format("%Y%m").to_string()];
    }
    keys.sort();
    keys.dedup();
    month_span(
        &keys[0],
        std::cmp::max(
            keys.last().expect("nonempty"),
            &today.format("%Y%m").to_string(),
        ),
    )
}

fn day_in_master(day: &str, value: &DayTimelineV1) -> Value {
    json!({
        "day": day,
        "generated_at_ms": value.generated_at_ms,
        "provenance": value.day_curation.provenance,
        "day_top": value.day_curation.picks,
        "day_rationale": value.day_curation.rationale,
    })
}

fn hour_payload(hour: &HourTimelineV1) -> Value {
    json!({
        "source_digest": hour.source_digest,
        "segment_count": hour.segment_count,
        "picks": hour.curation.picks,
        "rationale": hour.curation.rationale,
        "provenance": hour.curation.provenance,
    })
}

fn days_with_data(month: &MonthTimelineV1) -> Vec<String> {
    month.days.keys().cloned().collect()
}

fn month_span(start: &str, end: &str) -> Vec<String> {
    let mut year = start[0..4].parse::<i32>().unwrap_or_default();
    let mut month = start[4..6].parse::<u32>().unwrap_or_default();
    let end_year = end[0..4].parse::<i32>().unwrap_or_default();
    let end_month = end[4..6].parse::<u32>().unwrap_or_default();
    let mut months = Vec::new();
    while (year, month) <= (end_year, end_month) {
        months.push(format!("{year:04}{month:02}"));
        month += 1;
        if month > 12 {
            month = 1;
            year += 1;
        }
    }
    months
}

fn month_parts(ym: &str) -> Result<(i32, u32), std::io::Error> {
    let (Some(year), Some(month)) = (ym.get(0..4), ym.get(4..6)) else {
        return Err(std::io::Error::other("invalid timeline month"));
    };
    let year = year
        .parse::<i32>()
        .map_err(|_| std::io::Error::other("invalid timeline month"))?;
    let month = month
        .parse::<u32>()
        .map_err(|_| std::io::Error::other("invalid timeline month"))?;
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
        return Err(std::io::Error::other("invalid timeline month"));
    }
    Ok((year, month))
}

fn days_in_month(ym: &str) -> Result<u32, std::io::Error> {
    let (year, month) = month_parts(ym)?;
    let start = NaiveDate::from_ymd_opt(year, month, 1)
        .ok_or_else(|| std::io::Error::other("invalid timeline month"))?;
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .ok_or_else(|| std::io::Error::other("invalid timeline month"))?;
    Ok((next - start).num_days() as u32)
}

fn first_weekday(ym: &str) -> Result<u32, std::io::Error> {
    let (year, month) = month_parts(ym)?;
    NaiveDate::from_ymd_opt(year, month, 1)
        .map(|date| date.weekday().num_days_from_monday())
        .ok_or_else(|| std::io::Error::other("invalid timeline month"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};

    use super::*;
    use crate::test_support::{fixed_clock, phase_root, write};

    #[test]
    fn master_status_distinguishes_current_stale_and_missing() {
        let root = phase_root("populated");
        assert_eq!(master(root.path()).status.as_str(), "current");

        let mut state: Value = serde_json::from_str(
            &fs::read_to_string(root.path().join("timeline.state.json")).expect("state"),
        )
        .expect("JSON");
        state["published"]["input_digest"] = json!("different-input");
        write(
            &root.path().join("timeline.state.json"),
            &serde_json::to_string(&state).expect("state JSON"),
        );
        let stale = master(root.path());
        assert_eq!(stale.status.as_str(), "stale");
        assert_eq!(stale.outcome.as_str(), "digest_mismatch");
        assert!(stale.value.is_some());
        assert!(rollup_watermark(&stale).is_none());

        fs::remove_file(root.path().join("timeline.json")).expect("master");
        let missing = master(root.path());
        assert_eq!(missing.status.as_str(), "missing");
        assert_eq!(missing.outcome.as_str(), "missing");
    }

    #[test]
    fn day_status_distinguishes_current_stale_and_missing() {
        let root = phase_root("populated");
        assert_eq!(
            projection::day(root.path(), "20260510").status.as_str(),
            "current"
        );

        let state_path = root.path().join("chronicle/20260510/timeline.state.json");
        let mut state: Value =
            serde_json::from_str(&fs::read_to_string(&state_path).expect("state")).expect("JSON");
        state["published"]["input_digest"] = json!("different-input");
        write(
            &state_path,
            &serde_json::to_string(&state).expect("state JSON"),
        );
        let stale = projection::day(root.path(), "20260510");
        assert_eq!(stale.status.as_str(), "stale");
        assert_eq!(stale.outcome.as_str(), "digest_mismatch");

        fs::remove_file(root.path().join("chronicle/20260510/timeline.json")).expect("day");
        let missing = projection::day(root.path(), "20260510");
        assert_eq!(missing.status.as_str(), "missing");
        assert_eq!(missing.outcome.as_str(), "missing");
    }

    #[test]
    fn malformed_and_invalid_master_artifacts_are_named_stale_outcomes() {
        let root = phase_root("populated");
        write(&root.path().join("timeline.json"), "{");
        let malformed = master(root.path());
        assert_eq!(malformed.status.as_str(), "stale");
        assert_eq!(malformed.outcome.as_str(), "malformed");

        write(
            &root.path().join("timeline.json"),
            r#"{"schema_version":1,"kind":"day","source_digest":"input","generated_at_ms":1,"top_n":1,"months":{},"year_top":[],"year_curation":{"input_digest":"input","candidate_count":0,"picks":[],"rationale":"","error":null,"provenance":null}}"#,
        );
        let invalid = master(root.path());
        assert_eq!(invalid.status.as_str(), "stale");
        assert_eq!(invalid.outcome.as_str(), "invalid");
    }

    #[test]
    fn day_rollup_reads_its_own_current_artifact_when_master_is_stale() {
        let root = phase_root("populated");
        let mut state: Value = serde_json::from_str(
            &fs::read_to_string(root.path().join("timeline.state.json")).expect("state"),
        )
        .expect("JSON");
        state["published"]["input_digest"] = json!("stale-master");
        write(
            &root.path().join("timeline.state.json"),
            &serde_json::to_string(&state).expect("state JSON"),
        );

        let master_payload = overview(root.path(), &fixed_clock()).expect("overview");
        let day_payload = day_rollup(root.path(), "20260510");

        assert_eq!(master_payload["status"], "stale");
        assert_eq!(day_payload["status"], "current");
        assert_eq!(day_payload["day_top"][0]["binding"]["stream"], "_default");
        assert_eq!(day_payload["provenance"]["model"], "corpus-day-model");
    }
}
