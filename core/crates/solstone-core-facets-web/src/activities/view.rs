// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_facets::{
    ActivityRecord, PlacedRecord, ScheduledEvent, activity_field_held, activity_value_truthy,
    calendar_beside, calendar_on_day, edit_removable, edit_removed, list_activity_day_stems,
    list_declared_facet_names, load_activity_records, observe_facet_declaration,
    place_activity_records, read_activity_definitions,
};

/// The owner's own view reads every body window.
fn owner_reads(_: &str, _: &str, _: Option<&str>) -> bool {
    true
}

/// How many body faces a facet shows on a day, for the owner.
fn body_face_count(root: &Path, facet: &str, day: &str) -> usize {
    solstone_core_location::body_records::body_faces(root, facet, day, &owner_reads, false)
        .map(|faces| faces.len())
        .unwrap_or_default()
}

pub fn compute_day_counts(root: &Path, query: Option<&str>) -> BTreeMap<String, usize> {
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

    let mut days_set = std::collections::BTreeSet::new();
    for f in &facets {
        if let Ok(stems) = list_activity_day_stems(root, f) {
            for stem in stems {
                days_set.insert(stem);
            }
        }
    }

    let mut counts = BTreeMap::new();
    for day in days_set {
        let mut total = 0;
        for f in &facets {
            let visible_rows = load_activity_records(root, f, &day, false).unwrap_or_default();
            total += visible_rows.len() + body_face_count(root, f, &day);
        }
        if total > 0 {
            counts.insert(day, total);
        }
    }
    counts
}

fn map_facet_declaration(root: &Path, facet_name: &str, count: usize) -> Value {
    match observe_facet_declaration(root, facet_name) {
        Ok(Some(d)) => {
            let title = if d.title.trim().is_empty() {
                facet_name.to_owned()
            } else {
                d.title
            };
            json!({
                "name": facet_name,
                "count": count,
                "title": title,
                "color": d.color,
                "muted": d.muted.unwrap_or(false)
            })
        }
        _ => json!({
            "name": facet_name,
            "count": count,
            "title": facet_name,
            "color": "",
            "muted": false
        }),
    }
}

pub fn build_view(root: &Path, day: &str, query: Option<&str>) -> Result<Value, String> {
    let declared = list_declared_facet_names(root).map_err(|e| e.to_string())?;

    let mut facet_entries = Vec::new();
    for f in &declared {
        let count = load_activity_records(root, f, day, false)
            .unwrap_or_default()
            .len()
            + body_face_count(root, f, day);
        facet_entries.push(map_facet_declaration(root, f, count));
    }

    let query_facet = query_value(query, "facet").filter(|f| !f.is_empty());
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
        let rows = load_activity_records(root, f, day, true).map_err(|e| e.to_string())?;
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
    let previous = chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
        .ok()
        .and_then(|date| date.pred_opt())
        .map(|date| date.format("%Y%m%d").to_string());
    let mut near_faces = Vec::new();
    for on in std::iter::once(day.to_owned()).chain(previous) {
        for f in &scope {
            for face in solstone_core_location::body_records::body_faces(
                root,
                f,
                &on,
                &owner_reads,
                include_hidden,
            )
            .unwrap_or_default()
            {
                if let Some(span) = solstone_core_location::body_records::face_instants(&face) {
                    near_faces.push((span, solstone_core_location::body_records::face_json(&face)));
                }
            }
        }
    }
    let mut captures = Vec::new();
    for ((facet, row), course) in scoped_rows.iter().zip(courses) {
        let is_hidden = row.get("hidden").is_some_and(activity_value_truthy);
        if is_hidden && !include_hidden {
            continue;
        }

        let mut item = project_item(
            facet,
            row,
            &course,
            &definitions_by_facet,
            (day, &calendar, &location),
        );
        if let Some(span) = solstone_core_location::body_records::capture_instants(root, day, row) {
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
    if let Some(next) = solstone_core_location::body_records::next_day(day) {
        for f in &scope {
            for row in load_activity_records(root, f, &next, include_hidden).unwrap_or_default() {
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
    for f in &scope {
        let faces =
            solstone_core_location::body_records::body_faces(root, f, day, &owner_reads, true)
                .map_err(|e| e.to_string())?;
        for face in faces {
            if face.hidden {
                hidden_count += 1;
                if !include_hidden {
                    continue;
                }
            }
            let mut item = solstone_core_location::body_records::face_json(&face);
            if let Some(span) = solstone_core_location::body_records::face_instants(&face) {
                let beside = captures
                    .iter()
                    .filter(|(other, _, _)| {
                        solstone_core_location::body_records::half_cover(span, *other)
                    })
                    .map(|(_, title, activity)| json!({"title": title, "activity": activity}))
                    .collect::<Vec<_>>();
                if !beside.is_empty() {
                    item["beside"] = Value::Array(beside);
                }
                let middle = span.0 + (span.1 - span.0) / 2;
                if let Some((on, seconds)) =
                    solstone_core_location::body_records::owner_moment(root, middle)
                {
                    let place = if on == day {
                        location.beside((seconds, seconds + 1), None)
                    } else {
                        solstone_core_location::DayBeside::read(root, &on)
                            .beside((seconds, seconds + 1), None)
                    };
                    if let Some(place) = place {
                        item["place"] = json!({"device": place.device, "words": place.words});
                    }
                }
            }
            items.push(item);
        }
    }

    Ok(json!({
        "items": items,
        "facets": facet_entries,
        "hidden_count": hidden_count,
    }))
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
