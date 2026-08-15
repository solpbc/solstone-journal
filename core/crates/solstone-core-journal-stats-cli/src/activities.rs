// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Facet activity aggregation and the per-day heatmap.

use std::{collections::BTreeMap, fs, path::Path};

use chrono::{Datelike, NaiveDate};
use serde_json::Value;
use solstone_core_format::segment::segment_parse;

use crate::{
    error::JournalStatsError,
    model::{ActivityTotals, HeatmapData},
};

pub(crate) struct ActivityAccumulation {
    pub(crate) agent_data: BTreeMap<String, ActivityTotals>,
    pub(crate) facet_data: BTreeMap<String, ActivityTotals>,
    pub(crate) heatmap_data: HeatmapData,
}

pub(crate) fn accumulate_activities(
    journal_root: &Path,
    day: &str,
) -> Result<ActivityAccumulation, JournalStatsError> {
    let weekday: u8 = NaiveDate::parse_from_str(day, "%Y%m%d")
        .map_err(|_| JournalStatsError::InvalidDay(day.to_owned()))?
        .weekday()
        .num_days_from_monday()
        .try_into()
        .unwrap_or(0);
    let mut accumulation = ActivityAccumulation {
        agent_data: BTreeMap::new(),
        facet_data: BTreeMap::new(),
        heatmap_data: HeatmapData {
            weekday,
            hours: BTreeMap::new(),
        },
    };

    for facet in facet_names(journal_root)? {
        let Some(contents) = solstone_core_facets::read_activity_file(
            journal_root,
            &facet,
            &format!("{day}.jsonl"),
        )?
        else {
            continue;
        };

        for line in contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(record) = record.as_object() else {
                continue;
            };
            if record.get("hidden").and_then(Value::as_bool) == Some(true) {
                continue;
            }

            let activity = record
                .get("activity")
                .and_then(Value::as_str)
                .filter(|activity| !activity.is_empty())
                .unwrap_or("unknown")
                .to_owned();
            let segments: Vec<String> = record
                .get("segments")
                .and_then(Value::as_array)
                .map(|segments| {
                    segments
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if segments.is_empty() {
                continue;
            }

            let minutes = estimate_duration_minutes(&segments);
            let agent = accumulation.agent_data.entry(activity).or_default();
            agent.count += 1;
            agent.minutes += minutes as f64;

            let facet_total = accumulation.facet_data.entry(facet.clone()).or_default();
            facet_total.count += 1;
            facet_total.minutes += minutes as f64;

            for segment in &segments {
                let Some((start, end)) = segment_bounds(segment) else {
                    continue;
                };
                add_heatmap_range(&mut accumulation.heatmap_data.hours, start, end);
            }
        }
    }

    Ok(accumulation)
}

pub(crate) fn facet_names(journal_root: &Path) -> Result<Vec<String>, JournalStatsError> {
    let facets = journal_root.join("facets");
    if !facets.is_dir() {
        return Ok(Vec::new());
    }

    let mut names = Vec::new();
    for entry in read_dir(&facets)? {
        if entry
            .file_type()
            .map_err(|source| JournalStatsError::io(entry.path(), source))?
            .is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

pub fn estimate_duration_minutes(segments: &[String]) -> u64 {
    let seconds: u64 = segments
        .iter()
        .filter_map(|segment| segment_bounds(segment).map(|(start, end)| end - start))
        .sum();
    (seconds / 60).max(1)
}

fn segment_bounds(segment: &str) -> Option<(u64, u64)> {
    let start = segment_parse(segment)?;
    let (_, duration) = segment.split_once('_')?;
    let duration = duration.parse::<u64>().ok()?;
    let start =
        u64::from(start.hour) * 3_600 + u64::from(start.minute) * 60 + u64::from(start.second);
    Some((start, start.saturating_add(duration).min(86_399)))
}

fn add_heatmap_range(hours: &mut BTreeMap<u8, f64>, start: u64, end: u64) {
    let mut current = start;
    while current < end {
        let hour: u8 = (current / 3_600).try_into().unwrap_or(u8::MAX);
        if hour >= 24 {
            break;
        }
        let next = ((u64::from(hour) + 1) * 3_600).min(end);
        *hours.entry(hour).or_default() += (next - current) as f64 / 60.0;
        current = next;
    }
}

fn read_dir(path: &Path) -> Result<Vec<fs::DirEntry>, JournalStatsError> {
    fs::read_dir(path)
        .map_err(|source| JournalStatsError::io(path, source))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| JournalStatsError::io(path, source))
}
