// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Precision-gated location serialization for MCP read tools.
//!
//! Every coordinate, H3 cell, named-place roster row, and structured day item
//! returned over MCP passes through this module so `Full` / `Approx` /
//! `NamesOnly` and hidden-place rules are enforced in one place.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_format::content::kinds::PlacesPrecision;
use solstone_core_journal_io::{DEFAULT_STREAM, PathOrDay, StreamLocation, iter_segments};
use solstone_core_location::accuracy::StopWords;
use solstone_core_location::day::{DayLocation, Entry, Point};
use solstone_core_location::h3_cell::{ApproxCell, res8_cell};
use solstone_core_location::places::{Place, PlaceStore};
use solstone_core_location::projection::{Reach, hidden_reach};

use crate::dispatch::fold_literal;

pub(crate) const MAX_WEEK_DAY_PLACES: usize = 50;
pub(crate) const MAX_ROSTER_PLACES: usize = 200;
const MAX_WEEK_RUN_CELLS: usize = 12;
const MAX_FETCH_FIXES_PER_DAY: usize = 500;
const READINGS_END_POINT_THRESHOLD_M: f64 = 50.0;

pub(crate) struct PlacesRoster {
    pub(crate) items: Vec<Value>,
    pub(crate) truncated: bool,
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn attach_point_precision(
    obj: &mut Map<String, Value>,
    precision: PlacesPrecision,
    latitude: f64,
    longitude: f64,
) {
    match precision {
        PlacesPrecision::Full => {
            obj.insert("lat".to_owned(), json!(round6(latitude)));
            obj.insert("lon".to_owned(), json!(round6(longitude)));
        }
        PlacesPrecision::Approx => {
            if let Some(cell) = res8_cell(latitude, longitude) {
                obj.insert("h3".to_owned(), json!(cell.h3));
                obj.insert("cell_lat".to_owned(), json!(cell.cell_lat));
                obj.insert("cell_lon".to_owned(), json!(cell.cell_lon));
            }
        }
        PlacesPrecision::NamesOnly => {}
    }
}

/// Format an activity's `place` object according to the connection's places precision.
pub(crate) fn beside_place_json(
    beside: &solstone_core_location::Beside,
    precision: PlacesPrecision,
) -> Value {
    let mut obj = Map::new();
    obj.insert("device".to_owned(), json!(beside.device));
    obj.insert("words".to_owned(), json!(beside.words));
    if let Some(point) = beside.point {
        attach_point_precision(&mut obj, precision, point.latitude, point.longitude);
    }
    Value::Object(obj)
}

fn place_name_for_stop(known: Option<&Place>, stop_words: Option<&StopWords>) -> Option<String> {
    if let Some(name) = known.and_then(Place::owner_name) {
        return Some(name);
    }
    let words = stop_words?;
    if let Some(at) = &words.at {
        return Some(at.name.clone());
    }
    words.near.first().map(|near| format!("near {}", near.name))
}

fn ordered_unique_cells(points: &[Point]) -> Vec<ApproxCell> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for point in points {
        if let Some(cell) = res8_cell(point.latitude, point.longitude)
            && seen.insert(cell.h3.clone())
        {
            out.push(cell);
        }
    }
    out
}

fn approx_cell_json(cell: &ApproxCell) -> Value {
    json!({
        "h3": cell.h3,
        "cell_lat": cell.cell_lat,
        "cell_lon": cell.cell_lon,
    })
}

fn build_entry_item(
    store: &PlaceStore,
    day: &DayLocation,
    entry: &Entry,
    precision: PlacesPrecision,
    for_fetch: bool,
    remaining_fixes: &mut usize,
    fixes_truncated: &mut bool,
) -> Option<Value> {
    let reach = hidden_reach(store, entry);
    if reach == Reach::Whole {
        return None;
    }
    match entry {
        Entry::Unreadable { .. } => None,
        Entry::Visit {
            device,
            area,
            place,
            arrival,
            departure,
            point,
            ..
        } => {
            let known = solstone_core_location::accuracy::margin_m(point.accuracy_m)
                .and_then(|margin| store.place_for(point.latitude, point.longitude, margin))
                .filter(|p| !p.hidden);
            let place_label = place_name_for_stop(known, place.as_ref());
            let area_label = area.label();
            if place_label.is_none() && area_label.is_none() {
                return None;
            }
            let mut obj = Map::new();
            obj.insert("kind".to_owned(), json!("visit"));
            if let Some(name) = place_label {
                obj.insert("place".to_owned(), json!(name));
            }
            if let Some(known) = known.filter(|p| !p.dormant) {
                obj.insert("place_id".to_owned(), json!(known.id));
            }
            if let Some(area_label) = area_label {
                obj.insert("area".to_owned(), json!(area_label));
            }
            if let Some(start) = arrival {
                obj.insert("start".to_owned(), json!(start.clock()));
            }
            if let Some(end) = departure {
                obj.insert("end".to_owned(), json!(end.clock()));
            }
            if for_fetch {
                obj.insert("device".to_owned(), json!(day.device_phrase(*device)));
                if precision == PlacesPrecision::Full
                    && let Some(acc) = point.accuracy_m
                {
                    obj.insert("accuracy_m".to_owned(), json!(round1(acc)));
                }
            }
            attach_point_precision(&mut obj, precision, point.latitude, point.longitude);
            Some(Value::Object(obj))
        }
        Entry::Stay {
            device,
            area,
            place,
            first,
            last,
            count,
            point,
            ..
        } => {
            let known = solstone_core_location::accuracy::margin_m(point.accuracy_m)
                .and_then(|margin| store.place_for(point.latitude, point.longitude, margin))
                .filter(|p| !p.hidden);
            let place_label = place_name_for_stop(known, place.as_ref());
            let area_label = area.label();
            if place_label.is_none() && area_label.is_none() {
                return None;
            }
            let mut obj = Map::new();
            obj.insert("kind".to_owned(), json!("stay"));
            if let Some(name) = place_label {
                obj.insert("place".to_owned(), json!(name));
            }
            if let Some(known) = known.filter(|p| !p.dormant) {
                obj.insert("place_id".to_owned(), json!(known.id));
            }
            if let Some(area_label) = area_label {
                obj.insert("area".to_owned(), json!(area_label));
            }
            obj.insert("start".to_owned(), json!(first.clock()));
            if first.utc != last.utc {
                obj.insert("end".to_owned(), json!(last.clock()));
            }
            obj.insert("count".to_owned(), json!(*count));
            if for_fetch {
                obj.insert("device".to_owned(), json!(day.device_phrase(*device)));
                if precision == PlacesPrecision::Full
                    && let Some(acc) = point.accuracy_m
                {
                    obj.insert("accuracy_m".to_owned(), json!(round1(acc)));
                }
            }
            attach_point_precision(&mut obj, precision, point.latitude, point.longitude);
            Some(Value::Object(obj))
        }
        Entry::Readings {
            device,
            area,
            first,
            last,
            points,
            times,
            ..
        } => {
            // A run partly within reach of a hidden place keeps its times without
            // area, coordinates, fixes or cells.
            if reach == Reach::Part {
                let mut obj = Map::new();
                obj.insert("kind".to_owned(), json!("readings"));
                obj.insert("start".to_owned(), json!(first.clock()));
                if first.utc != last.utc {
                    obj.insert("end".to_owned(), json!(last.clock()));
                }
                obj.insert("count".to_owned(), json!(points.len()));
                if for_fetch {
                    obj.insert("device".to_owned(), json!(day.device_phrase(*device)));
                }
                return Some(Value::Object(obj));
            }
            let area_label = area.label()?;
            let mut obj = Map::new();
            obj.insert("kind".to_owned(), json!("readings"));
            obj.insert("area".to_owned(), json!(area_label));
            obj.insert("start".to_owned(), json!(first.clock()));
            if first.utc != last.utc {
                obj.insert("end".to_owned(), json!(last.clock()));
            }
            obj.insert("count".to_owned(), json!(points.len()));
            if for_fetch {
                obj.insert("device".to_owned(), json!(day.device_phrase(*device)));
            }
            match precision {
                PlacesPrecision::Full => {
                    if let Some(first_pt) = points.first() {
                        attach_point_precision(
                            &mut obj,
                            precision,
                            first_pt.latitude,
                            first_pt.longitude,
                        );
                        if !for_fetch
                            && let Some(last_pt) = points.last()
                            && points.len() > 1
                            && solstone_core_location::pack::distance_m(
                                first_pt.latitude,
                                first_pt.longitude,
                                last_pt.latitude,
                                last_pt.longitude,
                            ) > READINGS_END_POINT_THRESHOLD_M
                        {
                            obj.insert("end_lat".to_owned(), json!(round6(last_pt.latitude)));
                            obj.insert("end_lon".to_owned(), json!(round6(last_pt.longitude)));
                        }
                    }
                    if for_fetch && !points.is_empty() {
                        let take = points.len().min(*remaining_fixes);
                        if take < points.len() {
                            *fixes_truncated = true;
                        }
                        *remaining_fixes = remaining_fixes.saturating_sub(take);
                        let mut fixes = Vec::with_capacity(take);
                        for (index, pt) in points.iter().take(take).enumerate() {
                            let mut fix = Map::new();
                            if let Some(time) = times.get(index) {
                                fix.insert("t".to_owned(), json!(time.clock()));
                            }
                            fix.insert("lat".to_owned(), json!(round6(pt.latitude)));
                            fix.insert("lon".to_owned(), json!(round6(pt.longitude)));
                            if let Some(acc) = pt.accuracy_m {
                                fix.insert("accuracy_m".to_owned(), json!(round1(acc)));
                            }
                            fixes.push(Value::Object(fix));
                        }
                        obj.insert("fixes".to_owned(), Value::Array(fixes));
                    }
                }
                PlacesPrecision::Approx => {
                    let cells = ordered_unique_cells(points);
                    if let Some(first_cell) = cells.first() {
                        obj.insert("h3".to_owned(), json!(first_cell.h3));
                        obj.insert("cell_lat".to_owned(), json!(first_cell.cell_lat));
                        obj.insert("cell_lon".to_owned(), json!(first_cell.cell_lon));
                    }
                    if for_fetch {
                        if !cells.is_empty() {
                            obj.insert(
                                "cells".to_owned(),
                                Value::Array(cells.iter().map(approx_cell_json).collect()),
                            );
                        }
                    } else if cells.len() > 1 {
                        let truncated = cells.len() > MAX_WEEK_RUN_CELLS;
                        obj.insert(
                            "cells".to_owned(),
                            Value::Array(
                                cells
                                    .iter()
                                    .take(MAX_WEEK_RUN_CELLS)
                                    .map(approx_cell_json)
                                    .collect(),
                            ),
                        );
                        if truncated {
                            obj.insert("cells_truncated".to_owned(), json!(true));
                        }
                    }
                }
                PlacesPrecision::NamesOnly => {}
            }
            Some(Value::Object(obj))
        }
    }
}

/// Build the per-day `summary.places` payload for `get_week(include=["places"])`.
/// Returns `None` when every entry on the day was hidden or unreadable.
pub(crate) fn build_week_day_places(
    store: &PlaceStore,
    day: &DayLocation,
    precision: PlacesPrecision,
    reference: Option<String>,
) -> Option<Value> {
    let mut items = Vec::new();
    let mut more = false;
    let mut unused_remaining = 0;
    let mut unused_truncated = false;
    for entry in &day.entries {
        if let Some(item) = build_entry_item(
            store,
            day,
            entry,
            precision,
            false,
            &mut unused_remaining,
            &mut unused_truncated,
        ) {
            if items.len() < MAX_WEEK_DAY_PLACES {
                items.push(item);
            } else {
                more = true;
                break;
            }
        }
    }
    if items.is_empty() {
        return None;
    }
    let mut obj = Map::new();
    obj.insert("items".to_owned(), Value::Array(items));
    obj.insert("more".to_owned(), json!(more));
    if let Some(reference) = reference {
        obj.insert("reference".to_owned(), json!(reference));
    }
    Some(Value::Object(obj))
}

/// Build structured items for `fetch` on a day's location.
/// Returns `(items, fixes_truncated)`, or `None` when no visible items survive.
pub(crate) fn build_fetch_day_places(
    store: &PlaceStore,
    day: &DayLocation,
    precision: PlacesPrecision,
) -> Option<(Vec<Value>, bool)> {
    let mut items = Vec::new();
    let mut remaining_fixes = MAX_FETCH_FIXES_PER_DAY;
    let mut fixes_truncated = false;
    for entry in &day.entries {
        if let Some(item) = build_entry_item(
            store,
            day,
            entry,
            precision,
            true,
            &mut remaining_fixes,
            &mut fixes_truncated,
        ) {
            items.push(item);
        }
    }
    if items.is_empty() {
        return None;
    }
    Some((items, fixes_truncated))
}

/// Compact day summary snippet for empty-query `search(kind=["places"])` browse hits.
pub(crate) fn search_day_places_snippet(full_text: &str, max_chars: usize) -> String {
    let joined = full_text
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let mut out: String = joined.chars().take(max_chars).collect();
    if joined.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn day_has_live_location_segment(journal: &Path, day: &str) -> bool {
    let Ok(segments) = iter_segments(journal, PathOrDay::Day(day)) else {
        return false;
    };
    segments.into_iter().any(|segment| {
        let Ok(identity) = segment.record_identity() else {
            return false;
        };
        let (stream, rel_path) = match segment.stream() {
            StreamLocation::Direct => (
                DEFAULT_STREAM.to_owned(),
                format!("chronicle/{day}/{}/location.jsonl", identity.name),
            ),
            StreamLocation::Named(_) => (
                identity.stream.to_owned(),
                format!(
                    "chronicle/{day}/{}/{}/location.jsonl",
                    identity.stream, identity.name
                ),
            ),
        };
        crate::source_release::check_segment_transcript_release(
            journal,
            day,
            &stream,
            identity.name,
        ) && crate::activity_admit::read_contained_source(journal, &rel_path).is_some()
    })
}

fn place_matches_filters(
    place: &Place,
    area_label: Option<&str>,
    filter_query: Option<&str>,
    filter_place: Option<&str>,
) -> bool {
    let mut hay = String::new();
    if let Some(name) = &place.name {
        hay.push_str(name);
        hay.push(' ');
    }
    if let Some(alias) = place.alias {
        hay.push_str(alias.as_str());
        hay.push(' ');
    }
    if let Some(area) = area_label {
        hay.push_str(area);
    }
    let folded = fold_literal(&hay);
    if let Some(place_filter) = filter_place.map(str::trim).filter(|s| !s.is_empty())
        && !folded.contains(&fold_literal(place_filter))
    {
        return false;
    }
    if let Some(query_filter) = filter_query.map(str::trim).filter(|s| !s.is_empty()) {
        let folded_query = fold_literal(query_filter);
        if folded.contains(&folded_query) {
            return true;
        }
        let tokens: Vec<String> = query_filter
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.chars().count() >= 2)
            .map(fold_literal)
            .collect();
        if tokens.is_empty() {
            return false;
        }
        return tokens.iter().any(|token| folded.contains(token));
    }
    true
}

/// Build the owner's visible named-places roster (`places.json`), counted from
/// surviving non-future stays in `[from, to]`, sorted by `days_count` desc.
///
/// Fails closed (`None`) if `places.json` is unreadable, corrupt, or escapes
/// containment.
pub(crate) fn build_places_roster(
    journal: &Path,
    from: Option<&str>,
    to: Option<&str>,
    today: &str,
    precision: PlacesPrecision,
    filter_query: Option<&str>,
    filter_place: Option<&str>,
) -> Option<PlacesRoster> {
    if solstone_core_location::places::store_path(journal).exists()
        && crate::activity_admit::read_contained_source(journal, "solstone/apps/map/places.json")
            .is_none()
    {
        return None;
    }
    let store = PlaceStore::read_strict(journal).ok()?;
    let tier = solstone_core_location::area_tier().ok()?;
    let eff_to = match to {
        Some(bound) if bound < today => bound,
        _ => today,
    };
    let eff_from = from.unwrap_or("00000101");
    if eff_from > eff_to {
        return Some(PlacesRoster {
            items: Vec::new(),
            truncated: false,
        });
    }

    let stays = solstone_core_location::projection::stays_between(journal, eff_from, eff_to);
    let mut live_days: BTreeMap<String, bool> = BTreeMap::new();
    let mut days_by_place: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for stay in &stays {
        if stay.day.as_str() > eff_to {
            continue;
        }
        let live = *live_days.entry(stay.day.clone()).or_insert_with(|| {
            let record_rel = format!("chronicle/{}/health/location.json", stay.day);
            crate::activity_admit::read_contained_source(journal, &record_rel).is_some()
                && day_has_live_location_segment(journal, &stay.day)
        });
        if !live {
            continue;
        }
        if let Some(place) = store.place_for(stay.latitude, stay.longitude, stay.margin_m)
            && !place.hidden
            && !place.dormant
        {
            days_by_place
                .entry(place.id.as_str())
                .or_default()
                .insert(stay.day.clone());
        }
    }

    let mut rows: Vec<(&Place, Option<String>, BTreeSet<String>)> = store
        .places
        .iter()
        .filter(|place| !place.hidden && !place.dormant)
        .filter_map(|place| {
            let days = days_by_place.remove(place.id.as_str())?;
            if days.is_empty() {
                return None;
            }
            let area = tier.name_point(place.latitude, place.longitude).label();
            if !place_matches_filters(place, area.as_deref(), filter_query, filter_place) {
                return None;
            }
            Some((place, area, days))
        })
        .collect();

    rows.sort_by(|(a_place, a_area, a_days), (b_place, b_area, b_days)| {
        let a_label = a_place
            .name
            .as_deref()
            .or_else(|| a_place.alias.map(|a| a.as_str()))
            .or(a_area.as_deref())
            .unwrap_or("")
            .to_lowercase();
        let b_label = b_place
            .name
            .as_deref()
            .or_else(|| b_place.alias.map(|a| a.as_str()))
            .or(b_area.as_deref())
            .unwrap_or("")
            .to_lowercase();
        Reverse(a_days.len())
            .cmp(&Reverse(b_days.len()))
            .then_with(|| a_label.cmp(&b_label))
            .then_with(|| a_place.id.cmp(&b_place.id))
    });

    let truncated = rows.len() > MAX_ROSTER_PLACES;
    let items = rows
        .into_iter()
        .take(MAX_ROSTER_PLACES)
        .map(|(place, area, days)| {
            let mut obj = Map::new();
            obj.insert("id".to_owned(), json!(place.id));
            if let Some(name) = &place.name {
                obj.insert("name".to_owned(), json!(name));
            }
            if let Some(alias) = place.alias {
                obj.insert("alias".to_owned(), json!(alias.as_str()));
            }
            if let Some(area) = area {
                obj.insert("area".to_owned(), json!(area));
            }
            obj.insert("days_count".to_owned(), json!(days.len()));
            if let Some(first) = days.first() {
                obj.insert("first_day".to_owned(), json!(first));
            }
            if let Some(last) = days.last() {
                obj.insert("last_day".to_owned(), json!(last));
            }
            attach_point_precision(&mut obj, precision, place.latitude, place.longitude);
            Value::Object(obj)
        })
        .collect();

    Some(PlacesRoster { items, truncated })
}
