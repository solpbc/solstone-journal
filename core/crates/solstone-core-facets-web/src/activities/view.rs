// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_facets::{
    ActivityRecord, PlacedRecord, ScheduledEvent, activity_field_held, activity_value_truthy,
    calendar_beside, calendar_day_counts_between, calendar_on_day, edit_removable, edit_removed,
    list_activity_day_stems, list_declared_facet_names, load_activity_records_readable,
    observe_facet_declaration, place_activity_records, read_activity_definitions,
};

type Faces = BTreeMap<String, Vec<solstone_core_location::body_records::Face>>;

/// A facet's body faces as the owner sees them, hidden ones included, by
/// anchor day from `from` to `to`, read and grouped once. The owner reads every
/// window, so nothing is gated.
fn owner_faces(root: &Path, facet: &str, from: &str, to: &str) -> Faces {
    solstone_core_location::body_records::body_faces_range(root, facet, from, to, None, true)
        .unwrap_or_default()
}

/// How many faces a facet shows on a day, hidden ones aside.
fn shown_on(faces: &Faces, day: &str) -> usize {
    faces
        .get(day)
        .map_or(0, |faces| faces.iter().filter(|face| !face.hidden).count())
}

pub fn compute_day_counts(root: &Path, query: Option<&str>) -> BTreeMap<String, usize> {
    let counter = DayCounter::new(root, query);
    counter.counts(&counter.days.iter().collect::<Vec<_>>())
}

/// Like [`compute_day_counts`], including standalone non-declined scheduled
/// calendar events in the `/app/activities/` date navigator when no facet
/// filter is active.
pub fn compute_day_counts_with_calendar(
    root: &Path,
    query: Option<&str>,
) -> BTreeMap<String, usize> {
    let mut counts = compute_day_counts(root, query);
    if query_value(query, "facet")
        .filter(|f| !f.is_empty())
        .is_none()
    {
        for (day, cal_count) in calendar_day_counts_between(root, "00000000", "99999999") {
            if cal_count > 0 {
                counts.entry(day).or_insert(cal_count);
            }
        }
    }
    counts
}

/// [`compute_day_counts`] for the days from `from` to `to`: each the same
/// count, with no other day's activity file opened beyond the few days around
/// the span a body group's members can be filed on.
pub fn compute_day_counts_between(
    root: &Path,
    query: Option<&str>,
    from: &str,
    to: &str,
) -> BTreeMap<String, usize> {
    let counter = DayCounter::new(root, query);
    let days = counter.between(from, to);
    counter.counts(&days)
}

/// The years with a day on or before `through` that [`compute_day_counts`]
/// counts. Each year's days are read in order only until one has a count.
pub fn day_count_years(root: &Path, query: Option<&str>, through: &str) -> BTreeSet<String> {
    let counter = DayCounter::new(root, query);
    let mut by_year: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for day in counter.days.iter().filter(|day| day.as_str() <= through) {
        if let Some(year) = day.get(..4) {
            by_year.entry(year).or_default().push(day);
        }
    }
    by_year
        .into_iter()
        .filter(|(_, days)| {
            days.iter().any(|day| counter.rows(day) > 0) || !counter.counts(days).is_empty()
        })
        .map(|(year, _)| year.to_owned())
        .collect()
}

/// The facets a count covers and every day any of them has a file for.
struct DayCounter<'a> {
    root: &'a Path,
    facets: Vec<String>,
    days: BTreeSet<String>,
}

impl<'a> DayCounter<'a> {
    fn new(root: &'a Path, query: Option<&str>) -> Self {
        let declared = list_declared_facet_names(root).unwrap_or_default();
        let query_facet = query_value(query, "facet").filter(|f| !f.is_empty());

        let facets: Vec<String> = match query_facet {
            Some(f) => {
                if declared.contains(&f) {
                    vec![f]
                } else {
                    Vec::new()
                }
            }
            None => declared
                .into_iter()
                .filter(|f| {
                    observe_facet_declaration(root, f)
                        .ok()
                        .flatten()
                        .is_none_or(|d| d.muted != Some(true))
                })
                .collect(),
        };

        let mut days = BTreeSet::new();
        for f in &facets {
            if let Ok(stems) = list_activity_day_stems(root, f) {
                days.extend(stems);
            }
        }
        Self { root, facets, days }
    }

    fn between(&self, from: &str, to: &str) -> Vec<&String> {
        self.days
            .iter()
            .filter(|day| from <= day.as_str() && day.as_str() <= to)
            .collect()
    }

    /// The records the facets show on `day`, body records aside.
    fn rows(&self, day: &str) -> usize {
        self.facets
            .iter()
            .map(|f| {
                load_activity_records_readable(self.root, f, day, false)
                    .unwrap_or_default()
                    .len()
            })
            .sum()
    }

    /// The count on each of `days` (in order, each held) that has one.
    fn counts(&self, days: &[&String]) -> BTreeMap<String, usize> {
        let body = self.body(days);
        let mut counts = BTreeMap::new();
        for day in days {
            let mut total = 0;
            for f in &self.facets {
                let visible_rows =
                    load_activity_records_readable(self.root, f, day, false).unwrap_or_default();
                total += visible_rows.len() + body.get(f).map_or(0, |groups| groups.on(day).len());
            }
            if total > 0 {
                counts.insert((*day).clone(), total);
            }
        }
        counts
    }

    /// Each facet's body groups to count on `days`, grouped, not composed.
    /// Only the span of `days` is read, with the days around it a group's
    /// members can be filed on: the groups shown under those days are the
    /// ones every day held gives. When an end of either span isn't a date,
    /// the whole span is read, as when every day is counted.
    fn body(
        &self,
        days: &[&String],
    ) -> BTreeMap<&String, solstone_core_location::body_records::Groups> {
        let (Some(first), Some(last)) = (self.days.first(), self.days.last()) else {
            return BTreeMap::new();
        };
        let (Some(&from), Some(&to)) = (days.first(), days.last()) else {
            return BTreeMap::new();
        };
        let dated = |day: &str| solstone_core_time::parse_day(day).is_some();
        let (from, to) = if dated(first) && dated(last) && dated(from) && dated(to) {
            (from, to)
        } else {
            (first, last)
        };
        self.facets
            .iter()
            .map(|f| {
                let groups = solstone_core_location::body_records::body_groups_range(
                    self.root, f, from, to, None, false,
                )
                .unwrap_or_default();
                (f, groups)
            })
            .collect()
    }
}

fn map_facet_declaration(root: &Path, facet_name: &str, count: usize) -> Value {
    match observe_facet_declaration(root, facet_name) {
        Ok(Some(d)) => {
            let title = if d.title.trim().is_empty() {
                facet_name.to_owned()
            } else {
                d.title
            };
            let icon = d.icon.as_deref().unwrap_or("");
            json!({
                "name": facet_name,
                "count": count,
                "title": title,
                "color": d.color,
                "emoji": d.emoji,
                "icon": icon,
                "icon_svg": solstone_core_convey_http::icons::svg(d.icon.as_deref()),
                "muted": d.muted.unwrap_or(false)
            })
        }
        _ => json!({
            "name": facet_name,
            "count": count,
            "title": facet_name,
            "color": "",
            "emoji": "",
            "icon": "",
            "icon_svg": Value::Null,
            "muted": false
        }),
    }
}

pub fn build_view_in(
    root: &Path,
    day: &str,
    query: Option<&str>,
    locale: &str,
) -> Result<Value, String> {
    let declared = list_declared_facet_names(root).map_err(|e| e.to_string())?;
    // Body faces under this day and the day before (a night's), once per facet.
    let previous = solstone_core_time::parse_day(day)
        .and_then(|date| date.yesterday().ok())
        .map(solstone_core_time::day_key);
    let around: BTreeMap<&String, Faces> = declared
        .iter()
        .map(|f| {
            (
                f,
                owner_faces(root, f, previous.as_deref().unwrap_or(day), day),
            )
        })
        .collect();

    let mut facet_entries = Vec::new();
    for f in &declared {
        let count = load_activity_records_readable(root, f, day, false)
            .unwrap_or_default()
            .len()
            + around.get(f).map_or(0, |faces| shown_on(faces, day));
        facet_entries.push(map_facet_declaration(root, f, count));
    }

    let query_facet = query_value(query, "facet").filter(|f| !f.is_empty());
    let unfiltered_facet = query_facet.is_none();
    let scope: Vec<String> = match query_facet {
        Some(f) => {
            if declared.contains(&f) {
                vec![f]
            } else {
                Vec::new()
            }
        }
        None => declared
            .iter()
            .filter(|f| {
                observe_facet_declaration(root, f)
                    .ok()
                    .flatten()
                    .is_none_or(|d| d.muted != Some(true))
            })
            .cloned()
            .collect(),
    };

    let include_hidden = query_value(query, "include_hidden").as_deref() == Some("1");

    let mut scoped_rows: Vec<(String, ActivityRecord)> = Vec::new();
    let mut hidden_count = 0;
    for f in &scope {
        let rows = load_activity_records_readable(root, f, day, true).map_err(|e| e.to_string())?;
        for row in rows {
            if row.get("hidden").is_some_and(activity_value_truthy) {
                hidden_count += 1;
            }
            scoped_rows.push((f.clone(), row));
        }
    }

    let placed_records: Vec<PlacedRecord<'_>> = scoped_rows
        .iter()
        .map(|(f, r)| PlacedRecord {
            facet: f.as_str(),
            record: r,
        })
        .collect();
    let courses = place_activity_records(root, day, &placed_records);

    let mut definitions_by_facet: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for f in &scope {
        match read_activity_definitions(root, f) {
            Ok(defs) => {
                let mut map = BTreeMap::new();
                for def in defs {
                    if let (Some(id), Some(name)) = (
                        def.get("id").and_then(Value::as_str),
                        def.get("name").and_then(Value::as_str),
                    ) && !name.trim().is_empty()
                    {
                        map.insert(id.to_owned(), name.to_owned());
                    }
                }
                definitions_by_facet.insert(f.clone(), map);
            }
            Err(solstone_core_facets::FacetStoreError::MalformedActivityDefinition { .. }) => {
                definitions_by_facet.insert(f.clone(), BTreeMap::new());
            }
            Err(err) => return Err(err.to_string()),
        }
    }

    let calendar = calendar_on_day(root, day);
    let location = solstone_core_location::DayBeside::read(root, day);
    // A damaged ask store reads as nothing updating; the view still shows.
    let asks = solstone_core_system::owner_ask::read(root).unwrap_or_default();
    let mut answers = super::lines::Answers::default();
    let mut items = Vec::new();
    // Body faces this view shows beside a capture activity: under this day and
    // the day before, for a night.
    let mut near_faces = Vec::new();
    for on in std::iter::once(day.to_owned()).chain(previous.clone()) {
        for f in &scope {
            for face in around
                .get(f)
                .and_then(|faces| faces.get(&on))
                .into_iter()
                .flatten()
                .filter(|face| include_hidden || !face.hidden)
                .filter(|face| solstone_core_location::body_records::face_still_there(face))
            {
                if let Some(span) = solstone_core_location::body_records::shown_span(face) {
                    near_faces.push((span, solstone_core_location::body_records::face_json(face)));
                }
            }
        }
    }
    // A capture's span is worked out only when a body face could sit beside
    // it: a journal without body data reads nothing more than it did.
    let faces_today = scope.iter().any(|f| {
        around
            .get(f)
            .and_then(|faces| faces.get(day))
            .is_some_and(|faces| !faces.is_empty())
    });
    let spans_needed = faces_today || !near_faces.is_empty();
    let mut captures = Vec::new();
    let mut receipts_by_facet = BTreeMap::new();
    let hidden_entities = solstone_core_entity::hidden_participation_entity_ids(root);
    for ((facet, row), course) in scoped_rows.iter().zip(courses) {
        let is_hidden = row.get("hidden").is_some_and(activity_value_truthy);
        if is_hidden && !include_hidden {
            continue;
        }

        let mut displayed;
        let row = if locale != "en-US"
            && let Some(receipts) = receipts_by_facet.entry(facet).or_insert_with(|| {
                let path = solstone_core_format::talent_display::activity_path(root, facet, day);
                solstone_core_format::talent_display::read(root, &path)
            }) {
            displayed = row.clone();
            solstone_core_format::talent_display::project_story(&mut displayed, receipts, locale);
            &displayed
        } else {
            row
        };
        let mut item = project_item(
            facet,
            row,
            &course,
            &definitions_by_facet,
            &hidden_entities,
            (day, &calendar, &location),
        );
        if spans_needed
            && let Some(span) =
                solstone_core_location::body_records::capture_instants(root, day, row)
        {
            captures.push((
                span,
                string_field(row, "title"),
                string_field(row, "activity"),
            ));
            let beside = near_faces
                .iter()
                .filter(|(other, _)| solstone_core_location::body_records::half_cover(span, *other))
                .map(|(_, face)| {
                    json!({
                        "kind": face["activity"],
                        "title": face.get("title").cloned().unwrap_or_else(|| face["accounts"][0]["label"].clone()),
                        "start": face["accounts"][0]["start"],
                        "end": face["accounts"][0]["end"],
                    })
                })
                .collect::<Vec<_>>();
            if !beside.is_empty()
                && let Value::Object(item) = &mut item
            {
                item.insert("beside".to_owned(), Value::Array(beside));
            }
        }
        if let Value::Object(item) = &mut item {
            let id = string_field(row, "id");
            item.insert(
                "updating".to_owned(),
                Value::Bool(updating(root, day, facet, &id, asks.values())),
            );
            // An owner answer about who said its lines corrects it while the
            // answer stands.
            if item.get("corrected") == Some(&Value::Bool(false)) && answers.hold(root, day, row) {
                item.insert("corrected".to_owned(), Value::Bool(true));
            }
        }
        items.push(item);
    }

    // A night shown today ends tomorrow: its morning's activities sit beside it.
    if faces_today && let Some(next) = solstone_core_location::body_records::next_day(day) {
        for f in &scope {
            for row in
                load_activity_records_readable(root, f, &next, include_hidden).unwrap_or_default()
            {
                if let Some(span) =
                    solstone_core_location::body_records::capture_instants(root, &next, &row)
                {
                    captures.push((
                        span,
                        string_field(&row, "title"),
                        string_field(&row, "activity"),
                    ));
                }
            }
        }
    }

    // Body faces: composed from their windows, after the day's own items.
    let mut places = BTreeMap::new();
    let zone = solstone_core_journal_config::owner_zone(root);
    for f in &scope {
        let faces = around
            .get(f)
            .and_then(|faces| faces.get(day))
            .cloned()
            .unwrap_or_default();
        for face in faces {
            if face.hidden {
                hidden_count += 1;
                if !include_hidden {
                    continue;
                }
            }
            let mut item = solstone_core_location::body_records::face_json(&face);
            place_body_item(&mut item, &face, &zone, day);
            if let Some(span) = solstone_core_location::body_records::shown_span(&face) {
                // A record with nothing still there sits beside nothing, either way.
                let there = solstone_core_location::body_records::face_still_there(&face);
                let beside = captures
                    .iter()
                    .filter(|_| there)
                    .filter(|(other, _, _)| {
                        solstone_core_location::body_records::half_cover(span, *other)
                    })
                    .map(|(_, title, activity)| json!({"title": title, "activity": activity}))
                    .collect::<Vec<_>>();
                if !beside.is_empty() {
                    item["beside"] = Value::Array(beside);
                }
                // Where a device was at its middle, as that device timed it.
                let middle = span.0 + (span.1 - span.0) / 2;
                if let Some(place) = solstone_core_location::place_at(root, middle, &mut places) {
                    item["place"] = json!({"device": place.device, "words": place.words});
                }
            }
            items.push(item);
        }
    }

    let mut standalone_calendar = Vec::new();
    if unfiltered_facet {
        let mut attached_spans = Vec::new();
        for ((_, row), course) in
            scoped_rows
                .iter()
                .zip(place_activity_records(root, day, &placed_records))
        {
            let is_hidden = row.get("hidden").is_some_and(activity_value_truthy);
            if is_hidden && !include_hidden {
                continue;
            }
            if let Some(span) = course.span {
                attached_spans.push(span);
            }
        }
        let day_date = jiff::civil::Date::strptime("%Y%m%d", day).ok();
        let clamp_min = |at: jiff::civil::DateTime| -> u32 {
            if let Some(d) = day_date {
                if at.date() < d {
                    return 0;
                }
                if at.date() > d {
                    return 1440;
                }
            }
            u32::from(at.hour() as u8) * 60 + u32::from(at.minute() as u8)
        };
        for (idx, event) in calendar.iter().enumerate() {
            if event.declined {
                continue;
            }
            if attached_spans
                .iter()
                .any(|span| event.scheduled_during(day, *span))
            {
                continue;
            }
            let start_min = event.start.map(clamp_min);
            let end_min = event.end.map(clamp_min);
            standalone_calendar.push(json!({
                "id": format!("scheduled_{idx}"),
                "title": event.title,
                "all_day": event.all_day,
                "start": event.start.map(|at| at.strftime("%H:%M").to_string()),
                "end": event.end.map(|at| at.strftime("%H:%M").to_string()),
                "start_min": if event.all_day { None } else { start_min },
                "end_min": if event.all_day { None } else { end_min },
                "people": event.people.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            }));
        }
    }

    let mut payload = json!({
        "items": items,
        "facets": facet_entries,
        "hidden_count": hidden_count,
    });
    if !standalone_calendar.is_empty() {
        payload["calendar"] = Value::Array(standalone_calendar);
    }
    Ok(payload)
}

/// Whether a pending owner ask names the activity, still to hand to its work
/// or handed already, and its talents have not completed since that ask.
fn updating<'a>(
    root: &Path,
    day: &str,
    facet: &str,
    id: &str,
    asks: impl Iterator<Item = &'a solstone_core_system::owner_ask::OwnerAsk>,
) -> bool {
    let named = solstone_core_system::owner_ask::AskedActivity {
        facet: facet.to_owned(),
        activity: id.to_owned(),
    };
    asks.filter(|ask| ask.day == day)
        .filter(|ask| ask.to_seed.contains(&named) || ask.seeded.contains(&named))
        .any(|ask| {
            !solstone_core_system::activity_provenance::completed_since(
                root,
                day,
                facet,
                id,
                ask.created_at_ms,
            )
        })
}

fn project_item(
    facet: &str,
    record: &Map<String, Value>,
    course: &solstone_core_facets::ActivityCourse,
    definitions_by_facet: &BTreeMap<String, BTreeMap<String, String>>,
    hidden_entities: &std::collections::BTreeSet<String>,
    (day, calendar, location): (&str, &[ScheduledEvent], &solstone_core_location::DayBeside),
) -> Value {
    let mut item = Map::new();

    item.insert("id".to_owned(), Value::String(string_field(record, "id")));
    item.insert("facet".to_owned(), Value::String(facet.to_owned()));
    item.insert(
        "title".to_owned(),
        Value::String(string_field(record, "title")),
    );
    item.insert(
        "description".to_owned(),
        Value::String(string_field(record, "description")),
    );
    item.insert(
        "details".to_owned(),
        Value::String(string_field(record, "details")),
    );
    let playback = record.get("playback") == Some(&Value::Bool(true));
    item.insert("playback".to_owned(), Value::Bool(playback));
    item.insert(
        "corrected".to_owned(),
        Value::Bool(
            !string_field(record, "details").is_empty()
                || playback
                || activity_field_held(record, "description"),
        ),
    );

    let source = record
        .get("source")
        .and_then(Value::as_str)
        .map(str::to_owned);
    item.insert(
        "source".to_owned(),
        source
            .as_ref()
            .map(|s| Value::String(s.clone()))
            .unwrap_or(Value::Null),
    );

    let activity = record
        .get("activity")
        .and_then(Value::as_str)
        .map(str::to_owned);
    item.insert(
        "activity".to_owned(),
        activity
            .as_ref()
            .map(|a| Value::String(a.clone()))
            .unwrap_or(Value::Null),
    );

    let activity_label = match activity.as_deref() {
        Some(act) => {
            if let Some(name) = definitions_by_facet.get(facet).and_then(|m| m.get(act)) {
                Value::String(name.clone())
            } else {
                Value::String(act.replace('_', " "))
            }
        }
        None => Value::Null,
    };
    item.insert("activity_label".to_owned(), activity_label);

    let kind = match source.as_deref() {
        Some("user") => "added",
        None => "detected",
        Some(_) => "other",
    };
    item.insert("kind".to_owned(), Value::String(kind.to_owned()));

    let (start_min, end_min, start_str, end_str) = if let Some((span_start, span_end)) = course.span
    {
        let start_min = span_start / 60;
        let end_min = span_end / 60;
        let start_str = Some(format!(
            "{:02}:{:02}",
            (start_min % 1440) / 60,
            (start_min % 1440) % 60
        ));
        let end_str = Some(format!(
            "{:02}:{:02}",
            (end_min % 1440) / 60,
            (end_min % 1440) % 60
        ));
        (Some(start_min), Some(end_min), start_str, end_str)
    } else {
        (None, None, None, None)
    };

    item.insert(
        "start_min".to_owned(),
        start_min.map(Value::from).unwrap_or(Value::Null),
    );
    item.insert(
        "end_min".to_owned(),
        end_min.map(Value::from).unwrap_or(Value::Null),
    );
    item.insert(
        "start".to_owned(),
        start_str.map(Value::from).unwrap_or(Value::Null),
    );
    item.insert(
        "end".to_owned(),
        end_str.map(Value::from).unwrap_or(Value::Null),
    );

    let segments_val: Vec<Value> = course
        .segments
        .iter()
        .map(|seg| {
            let start_min = seg.start_seconds.map(|s| s / 60);
            let start_str =
                start_min.map(|m| format!("{:02}:{:02}", (m % 1440) / 60, (m % 1440) % 60));
            json!({
                "key": seg.key,
                "start": start_str,
                "start_min": start_min,
                "seconds": seg.length_seconds,
                "stream": seg.stream,
            })
        })
        .collect();
    item.insert("segments".to_owned(), Value::Array(segments_val));

    item.insert(
        "stream".to_owned(),
        course
            .stream
            .as_ref()
            .map(|s| Value::String(s.clone()))
            .unwrap_or(Value::Null),
    );
    item.insert("mixed".to_owned(), Value::Bool(course.mixed));
    item.insert("concurrent".to_owned(), Value::Bool(course.concurrent));

    let is_hidden = record.get("hidden").is_some_and(activity_value_truthy);
    item.insert("hidden".to_owned(), Value::Bool(is_hidden));

    let is_cancelled = record.get("cancelled").is_some_and(activity_value_truthy);

    if is_hidden {
        let mut reason = None;
        if let Some(edits) = record.get("edits").and_then(Value::as_array) {
            for edit in edits.iter().rev() {
                let Some(obj) = edit.as_object() else {
                    continue;
                };
                let Some(fields) = obj.get("fields").and_then(Value::as_array) else {
                    continue;
                };
                if fields.iter().any(|f| f.as_str() == Some("hidden")) {
                    let actor = obj.get("actor").and_then(Value::as_str).unwrap_or("");
                    if actor.starts_with("owner:") {
                        reason = Some("owner");
                    } else if actor.starts_with("cli:") {
                        reason = Some("command");
                    }
                    break;
                }
            }
        }
        let reason = reason.unwrap_or(if is_cancelled { "cancelled" } else { "other" });
        item.insert("hidden_reason".to_owned(), Value::String(reason.to_owned()));
    }

    item.insert("cancelled".to_owned(), Value::Bool(is_cancelled));

    let found_at = record
        .get("settled_at")
        .cloned()
        .or_else(|| record.get("created_at").cloned())
        .unwrap_or(Value::Null);
    item.insert("found_at".to_owned(), found_at);

    let edited_by_owner = record
        .get("edits")
        .and_then(Value::as_array)
        .is_some_and(|edits| {
            edits.iter().filter_map(Value::as_object).any(|edit| {
                if edit_removed(edit) {
                    return false;
                }
                let actor = edit.get("actor").and_then(Value::as_str).unwrap_or("");
                let Some(fields) = edit.get("fields").and_then(Value::as_array) else {
                    return false;
                };
                actor.starts_with("owner:")
                    && fields.iter().any(|f| {
                        matches!(
                            f.as_str(),
                            Some("title" | "description" | "details" | "playback")
                        )
                    })
            })
        });
    item.insert("edited_by_owner".to_owned(), Value::Bool(edited_by_owner));

    let summary = record
        .get("story")
        .and_then(Value::as_object)
        .and_then(|story| story.get("body"))
        .and_then(Value::as_str)
        .map(|b| Value::String(b.to_owned()))
        .unwrap_or(Value::Null);
    item.insert("summary".to_owned(), summary);

    let people = record
        .get("participation")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_object)
                .filter(|p| {
                    matches!(
                        p.get("role").and_then(Value::as_str),
                        Some("attendee" | "heard")
                    )
                })
                .filter(|p| {
                    !p.get("entity_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .is_some_and(|id| hidden_entities.contains(id))
                })
                .filter_map(|p| p.get("name").and_then(Value::as_str))
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|s| Value::String(s.to_owned()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    item.insert("people".to_owned(), Value::Array(people));
    let beside = course
        .span
        .map(|span| calendar_beside(calendar, day, span))
        .unwrap_or_default();
    item.insert("calendar".to_owned(), Value::Array(beside));
    // Where a device was while it ran, in the map's words; the link opens the
    // place when the journal has worked one out, else that day on the map.
    let place = course
        .span
        .and_then(|span| location.beside(span, course.stream.as_deref()))
        .map(|beside| {
            let href = match &beside.place_id {
                Some(id) => format!("/app/map/place/{id}"),
                None => format!("/app/map/{day}"),
            };
            json!({"device": beside.device, "words": beside.words, "href": href})
        })
        .unwrap_or(Value::Null);
    item.insert("place".to_owned(), place);

    let edits = record
        .get("edits")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_object)
                .enumerate()
                .map(|(position, obj)| {
                    let timestamp = obj.get("timestamp").cloned().unwrap_or(Value::Null);
                    let actor_val = obj.get("actor").cloned().unwrap_or(Value::Null);
                    let actor_str = obj.get("actor").and_then(Value::as_str).unwrap_or("");
                    let fields_val = obj
                        .get("fields")
                        .filter(|f| f.is_array())
                        .cloned()
                        .unwrap_or_else(|| Value::Array(Vec::new()));
                    let note_val = obj.get("note").cloned().unwrap_or(Value::Null);

                    let kind = match actor_str {
                        "activity" => "grew",
                        "owner:update" => "owner_update",
                        "owner:mute" => "owner_hide",
                        "owner:unmute" => "owner_unhide",
                        "cli:update" => "command_update",
                        "cli:mute" => "command_hide",
                        "cli:unmute" => "command_unhide",
                        "cli:create" => "command_create",
                        solstone_core_facets::OWNER_LINES_ACTOR => "owner_lines",
                        _ => "other",
                    };

                    let mut edit_out = Map::new();
                    edit_out.insert("timestamp".to_owned(), timestamp);
                    edit_out.insert("actor".to_owned(), actor_val);
                    edit_out.insert("fields".to_owned(), fields_val.clone());
                    edit_out.insert("note".to_owned(), note_val);
                    edit_out.insert("kind".to_owned(), Value::String(kind.to_owned()));
                    // The position the remove route addresses this edit by.
                    edit_out.insert("position".to_owned(), Value::from(position));
                    edit_out.insert("removable".to_owned(), Value::Bool(edit_removable(obj)));
                    edit_out.insert("removed".to_owned(), Value::Bool(edit_removed(obj)));
                    if let Some((lines, speaker)) = super::lines::history_fields(obj) {
                        edit_out.insert("lines".to_owned(), Value::from(lines));
                        edit_out.insert("speaker".to_owned(), speaker);
                    }
                    if let Some(suggested_by) = obj.get("suggested_by").filter(|v| v.is_object()) {
                        edit_out.insert("suggested_by".to_owned(), suggested_by.clone());
                    }
                    Value::Object(edit_out)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    item.insert("edits".to_owned(), Value::Array(edits));

    Value::Object(item)
}

fn string_field(record: &Map<String, Value>, key: &str) -> String {
    record
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(value.to_owned());
        }
    }
    None
}

/// What the day's timeline draws a body item by, as it draws any activity: its
/// name and its times on the owner's clock, in minutes from the day's start.
/// A night that runs past midnight is drawn to the day's end.
fn place_body_item(
    item: &mut Value,
    face: &solstone_core_location::body_records::Face,
    zone: &jiff::tz::TimeZone,
    day: &str,
) {
    // What kind it is, where an activity says its type.
    item["activity_label"] = Value::String(face.anchor.activity.clone());
    if let Some(name) = solstone_core_location::body_records::face_line(face, zone, day)
        .and_then(|line| line.get("name").and_then(Value::as_str).map(str::to_owned))
    {
        item["title"] = Value::String(name);
    }
    let Some((start, end)) = solstone_core_location::body_records::shown_span(face) else {
        return;
    };
    let Some(date) = solstone_core_time::parse_day(day) else {
        return;
    };
    let minute = |at: i64| wall_minute(zone, date, at);
    let (start_min, end_min) = (minute(start), minute(end));
    let clock = |minutes: i64| format!("{:02}:{:02}", (minutes % 1440) / 60, minutes % 60);
    item["start_min"] = Value::from(start_min);
    item["end_min"] = Value::from(end_min.max(start_min));
    item["start"] = Value::String(clock(start_min));
    item["end"] = Value::String(clock(end_min.max(start_min)));
}

/// Minutes into `day` on the wall clock in `zone` at the instant `at` (Unix
/// seconds), the grid segment activities are placed on: a day the clocks
/// change on keeps both aligned. Before the day is 0, after it 1440.
fn wall_minute(zone: &jiff::tz::TimeZone, day: jiff::civil::Date, at: i64) -> i64 {
    let Ok(at) = jiff::Timestamp::from_second(at) else {
        return 0;
    };
    let wall = zone.to_datetime(at);
    match wall.date().cmp(&day) {
        std::cmp::Ordering::Less => 0,
        std::cmp::Ordering::Greater => 1440,
        std::cmp::Ordering::Equal => i64::from(wall.hour()) * 60 + i64::from(wall.minute()),
    }
}

#[cfg(test)]
mod wall_minute_tests {
    use super::wall_minute;
    use jiff::civil::date;

    #[test]
    fn a_body_item_sits_at_its_wall_clock_minute_on_a_day_the_clocks_change() {
        let denver = solstone_core_time::zone("America/Denver").unwrap();
        let spring = date(2026, 3, 8);
        // 10:00 MDT, after the clocks went forward: 16:00 UTC.
        assert_eq!(wall_minute(&denver, spring, 1_772_985_600), 600);
        // 01:00 MST, before the change: 08:00 UTC.
        assert_eq!(wall_minute(&denver, spring, 1_772_956_800), 60);
        let autumn = date(2026, 11, 1);
        // 10:00 MST, after the clocks went back: 17:00 UTC.
        assert_eq!(wall_minute(&denver, autumn, 1_793_552_400), 600);
        // The day before and after clamp to the day's ends.
        assert_eq!(wall_minute(&denver, spring, 1_772_870_400), 0);
        assert_eq!(wall_minute(&denver, spring, 1_773_100_800), 1440);
    }

    #[test]
    fn project_item_hides_trashed_entities_and_restores_on_untrash() {
        let root = tempfile::tempdir().unwrap();
        let entities = root.path().join("entities");
        std::fs::create_dir_all(entities.join("ada")).unwrap();
        std::fs::write(
            entities.join("ada/entity.json"),
            r#"{"id":"ada","name":"Ada Lovelace","type":"Person"}"#,
        )
        .unwrap();
        std::fs::write(
            entities.join("retired.json"),
            r#"{"ids":{"trashed_p":{"state":"deleted","dir":"trashed_p","name":"Trashed Person"}}}"#,
        )
        .unwrap();

        let record: serde_json::Map<String, serde_json::Value> = serde_json::from_value(serde_json::json!({
            "id": "a1",
            "title": "Sync",
            "segments": ["090000_300"],
            "participation": [
                {"name": "Ada Lovelace", "entity_id": "ada", "role": "attendee", "source": "voice"},
                {"name": "Trashed Person", "entity_id": "trashed_p", "role": "attendee", "source": "voice"},
                {"name": "Anne", "entity_id": "anne", "role": "heard", "source": "voice"}
            ]
        }))
        .unwrap();
        let courses = solstone_core_facets::place_activity_records(
            root.path(),
            "20261112",
            &[solstone_core_facets::PlacedRecord {
                facet: "work",
                record: &record,
            }],
        );
        let course = &courses[0];
        let defs = std::collections::BTreeMap::new();
        let beside = solstone_core_location::DayBeside::read(root.path(), "20261112");

        let hidden = solstone_core_entity::hidden_participation_entity_ids(root.path());
        let item = super::project_item(
            "work",
            &record,
            course,
            &defs,
            &hidden,
            ("20261112", &[], &beside),
        );
        let names = item["people"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["Ada Lovelace", "Anne"]);

        // Restoring the entity immediately makes it reappear in `people`.
        std::fs::create_dir_all(entities.join("trashed_p")).unwrap();
        std::fs::write(
            entities.join("trashed_p/entity.json"),
            r#"{"id":"trashed_p","name":"Trashed Person","type":"Person"}"#,
        )
        .unwrap();
        let restored = solstone_core_entity::hidden_participation_entity_ids(root.path());
        let item2 = super::project_item(
            "work",
            &record,
            course,
            &defs,
            &restored,
            ("20261112", &[], &beside),
        );
        let names2 = item2["people"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names2, vec!["Ada Lovelace", "Trashed Person", "Anne"]);
    }
}
