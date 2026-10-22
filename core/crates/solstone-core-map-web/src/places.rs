// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Places, one place, a place's edit menu, and a week of stays.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, Duration, NaiveDate};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_location::places::{Alias, PlaceStore, clean_name};
use solstone_core_location::{Entry, PlaceView};

use crate::{
    AppState, area_json, entry_json, invalid_day, invalid_month, place_json, round, town_json,
    valid_day, valid_month,
};

pub(crate) fn valid_id(id: &str) -> bool {
    id.len() == 12
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub(crate) fn unknown_place() -> Response {
    error_envelope(
        "unknown_place",
        "that place isn't in your journal.",
        "Place not found",
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

fn names_unavailable(detail: String) -> Response {
    error_envelope(
        "area_names_unavailable",
        "the place names that come with your journal couldn't be read.",
        detail,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

fn view_json(view: &PlaceView, with_stays: bool) -> Value {
    let place = &view.place;
    let mut value = json!({
        "id": place.id,
        "label": view.label(),
        "owner_name": place.owner_name(),
        "name": place.name,
        "alias": place.alias.map(Alias::as_str),
        "hidden": place.hidden,
        "lat": round(place.latitude),
        "lon": round(place.longitude),
        "area": area_json(&view.area),
        "words": view.words.as_ref().map(place_json),
        "dates": view.description.dates,
        "first": view.description.first,
        "last": view.description.last,
        "night_dates": view.description.night_dates,
    });
    if with_stays {
        value["stays"] = Value::Array(
            view.stays
                .iter()
                .zip(&view.clocks)
                .map(|(stay, (start, end))| {
                    json!({
                        "day": stay.day,
                        "start": start,
                        "end": end,
                        "kind": stay.kind,
                    })
                })
                .collect(),
        );
    }
    value
}

pub(crate) async fn list(State(state): State<Arc<AppState>>) -> Response {
    let journal_root = state.journal_root.clone();
    spawn_blocking_response(OwnerReadRole::MapPlaces, move || {
        match solstone_core_location::read_places(&journal_root) {
            Ok(views) => Json(json!({
                "places": views.iter().map(|view| view_json(view, false)).collect::<Vec<_>>(),
            }))
            .into_response(),
            Err(error) => names_unavailable(error.to_string()),
        }
    })
    .await
}

pub(crate) async fn show(
    State(state): State<Arc<AppState>>,
    RoutePath(id): RoutePath<String>,
) -> Response {
    if !valid_id(&id) {
        return unknown_place();
    }
    let journal_root = state.journal_root.clone();
    spawn_blocking_response(OwnerReadRole::MapPlace, move || {
        match solstone_core_location::read_places(&journal_root) {
            Ok(views) => match views.iter().find(|view| view.place.id == id) {
                Some(view) => Json(view_json(view, true)).into_response(),
                None => unknown_place(),
            },
            Err(error) => names_unavailable(error.to_string()),
        }
    })
    .await
}

fn invalid_edit(detail: &str) -> Response {
    error_envelope(
        "invalid_place_edit",
        "that change couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

/// Rename, call it home or work, or hide it from the map. A field left out
/// stays as it is; an empty name or a null clears it.
pub(crate) async fn edit(
    State(state): State<Arc<AppState>>,
    RoutePath(id): RoutePath<String>,
    body: Bytes,
) -> Response {
    if !valid_id(&id) {
        return unknown_place();
    }
    let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(&body) else {
        return invalid_edit("Body must be a JSON object");
    };
    let name = match fields.get("name") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(name)) => Some(clean_name(name)),
        Some(_) => return invalid_edit("name must be a string or null"),
    };
    let alias = match fields.get("alias") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(alias)) if alias == "home" => Some(Some(Alias::Home)),
        Some(Value::String(alias)) if alias == "work" => Some(Some(Alias::Work)),
        Some(_) => return invalid_edit("alias must be home, work or null"),
    };
    let hidden = match fields.get("hidden") {
        None => None,
        Some(Value::Bool(hidden)) => Some(*hidden),
        Some(_) => return invalid_edit("hidden must be true or false"),
    };
    let journal_root = state.journal_root.clone();
    let found = PlaceStore::update(&journal_root, |store| {
        let Some(place) = store.places.iter_mut().find(|place| place.id == id) else {
            return false;
        };
        if let Some(name) = name {
            place.name = name;
        }
        if let Some(alias) = alias {
            place.alias = alias;
        }
        if let Some(hidden) = hidden {
            place.hidden = hidden;
        }
        true
    });
    match found {
        Ok(true) => {}
        Ok(false) => return unknown_place(),
        Err(error) => {
            return error_envelope(
                "place_edit_failed",
                "that change couldn't be saved.",
                error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response();
        }
    }
    match solstone_core_location::read_places(&journal_root) {
        Ok(views) => match views.iter().find(|view| view.place.id == id) {
            Some(view) => Json(view_json(view, true)).into_response(),
            None => unknown_place(),
        },
        Err(error) => names_unavailable(error.to_string()),
    }
}

/// Monday to Sunday around `day`, each with its visits and stays.
pub(crate) async fn week(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
) -> Response {
    if !valid_day(&day) {
        return invalid_day();
    }
    let journal_root = state.journal_root.clone();
    spawn_blocking_response(OwnerReadRole::MapWeek, move || {
        let Ok(date) = NaiveDate::parse_from_str(&day, "%Y%m%d") else {
            return invalid_day();
        };
        let monday = date - Duration::days(i64::from(date.weekday().num_days_from_monday()));
        let store = PlaceStore::read(&journal_root);
        let mut days = Vec::new();
        for offset in 0..7 {
            let key = (monday + Duration::days(offset))
                .format("%Y%m%d")
                .to_string();
            let view = match solstone_core_location::read_journal_day(&journal_root, &key) {
                Ok(view) => view,
                Err(error) => return names_unavailable(error.to_string()),
            };
            let items: Vec<Value> = view
                .entries
                .iter()
                .filter(|entry| matches!(entry, Entry::Visit { .. } | Entry::Stay { .. }))
                .map(|entry| entry_json(entry, &store))
                .collect();
            days.push(json!({
                "day": key,
                "readings": view.readings(),
                "devices": view.devices.iter().map(|device| device.word).collect::<Vec<_>>(),
                "items": items,
            }));
        }
        let step = |weeks: i64| {
            (monday + Duration::days(7 * weeks))
                .format("%Y%m%d")
                .to_string()
        };
        Json(json!({
            "start": monday.format("%Y%m%d").to_string(),
            "days": days,
            "previous": step(-1),
            "next": step(1),
        }))
        .into_response()
    })
    .await
}

fn device_month_payload(
    view: &solstone_core_location::DayLocation,
    index: usize,
    device: &solstone_core_location::day::Device,
) -> Value {
    let hash = Sha256::digest(device.key.as_bytes());
    let hex_key = format!("{hash:064x}")[..8].to_string();
    json!({
        "key": hex_key,
        "word": device.word,
        "phrase": view.device_phrase(index),
        "platform": device.platform.as_str(),
        "readings": device.readings,
        "interrupted_segments": device.interrupted_segments,
        "unreadable_lines": device.unreadable_lines,
        "unreadable_files": device.unreadable_files,
    })
}

fn day_items(view: &solstone_core_location::DayLocation, store: &PlaceStore) -> Vec<Value> {
    view.entries
        .iter()
        .filter(|entry| matches!(entry, Entry::Visit { .. } | Entry::Stay { .. }))
        .map(|entry| entry_json(entry, store))
        .collect()
}

fn day_devices(view: &solstone_core_location::DayLocation) -> Vec<Value> {
    view.devices
        .iter()
        .enumerate()
        .map(|(index, device)| device_month_payload(view, index, device))
        .collect()
}

/// A calendar month of visits and stays, framed by the days before and after.
pub(crate) async fn month(
    State(state): State<Arc<AppState>>,
    RoutePath(month): RoutePath<String>,
) -> Response {
    if !valid_month(&month) {
        return invalid_month(StatusCode::BAD_REQUEST);
    }
    let journal_root = state.journal_root.clone();
    spawn_blocking_response(OwnerReadRole::MapMonth, move || {
        let year: i32 = match month[..4].parse() {
            Ok(y) => y,
            Err(_) => return invalid_month(StatusCode::BAD_REQUEST),
        };
        let month_num: u32 = match month[4..6].parse() {
            Ok(m) => m,
            Err(_) => return invalid_month(StatusCode::BAD_REQUEST),
        };
        let Some(first_date) = NaiveDate::from_ymd_opt(year, month_num, 1) else {
            return invalid_month(StatusCode::BAD_REQUEST);
        };
        let before_date = first_date - Duration::days(1);
        let before_day = before_date.format("%Y%m%d").to_string();
        let previous = before_date.format("%Y%m").to_string();

        let next_date = if month_num == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap()
        } else {
            NaiveDate::from_ymd_opt(year, month_num + 1, 1).unwrap()
        };
        let next = next_date.format("%Y%m").to_string();
        let after_day = next_date.format("%Y%m%d").to_string();

        let days_in_month = (next_date - first_date).num_days();

        let tier = match solstone_core_location::area_tier() {
            Ok(tier) => tier,
            Err(error) => return names_unavailable(error.to_string()),
        };
        let store = PlaceStore::read(&journal_root);

        let before_view = match solstone_core_location::read_journal_day(&journal_root, &before_day)
        {
            Ok(v) => v,
            Err(e) => return names_unavailable(e.to_string()),
        };
        let before_items = day_items(&before_view, &store);
        let before_devices = day_devices(&before_view);

        let after_view = match solstone_core_location::read_journal_day(&journal_root, &after_day) {
            Ok(v) => v,
            Err(e) => return names_unavailable(e.to_string()),
        };
        let after_items = day_items(&after_view, &store);
        let after_devices = day_devices(&after_view);

        let mut days = Vec::with_capacity(days_in_month as usize);
        let mut day_views = Vec::with_capacity(days_in_month as usize);
        let mut day_items_list = Vec::with_capacity(days_in_month as usize);

        for offset in 0..days_in_month {
            let date = first_date + Duration::days(offset);
            let day_key = date.format("%Y%m%d").to_string();
            let view = match solstone_core_location::read_journal_day(&journal_root, &day_key) {
                Ok(v) => v,
                Err(e) => return names_unavailable(e.to_string()),
            };
            let items = day_items(&view, &store);
            let devices = day_devices(&view);
            let has_location = !view.entries.is_empty();

            days.push(json!({
                "day": day_key,
                "has_location": has_location,
                "items": items,
                "devices": devices,
            }));
            day_items_list.push(items);
            day_views.push(view);
        }

        let mut place_ids = BTreeSet::new();
        let collect_places = |items: &[Value], set: &mut BTreeSet<String>| {
            for item in items {
                if let Some(id) = item
                    .get("place_ref")
                    .and_then(|pr| pr.get("id"))
                    .and_then(|id| id.as_str())
                {
                    set.insert(id.to_string());
                }
            }
        };
        collect_places(&before_items, &mut place_ids);
        for day_items in &day_items_list {
            collect_places(day_items, &mut place_ids);
        }
        collect_places(&after_items, &mut place_ids);

        let mut places = BTreeMap::new();
        for id in place_ids {
            if let Some(place) = store.get(&id) {
                places.insert(
                    place.id.clone(),
                    json!({
                        "id": place.id,
                        "lat": round(place.latitude),
                        "lon": round(place.longitude),
                    }),
                );
            }
        }

        let mut seen_towns = BTreeSet::new();
        let mut towns = Vec::new();
        for (view, items) in day_views.iter().zip(&day_items_list) {
            if items.is_empty() {
                continue;
            }
            if let Some(records) = super::record_bounds(view, &store) {
                let context = super::context_bounds(records);
                for town in tier.towns_in(context, 12) {
                    let key = (
                        town.name.clone(),
                        (town.latitude * 100.0).round() as i64,
                        (town.longitude * 100.0).round() as i64,
                    );
                    if seen_towns.insert(key) {
                        towns.push(town_json(&town));
                    }
                }
            }
        }

        let any_day_has_devices = !before_view.is_empty()
            || !after_view.is_empty()
            || day_views.iter().any(|v| !v.is_empty());
        let journal_has_location = !solstone_core_location::list_location_days(&journal_root)
            .is_empty()
            || any_day_has_devices;

        let today = chrono::Utc::now()
            .with_timezone(&solstone_core_journal_config::owner_zone(&journal_root))
            .format("%Y%m%d")
            .to_string();

        Json(json!({
            "month": month,
            "previous": previous,
            "next": next,
            "today": today,
            "journal_has_location": journal_has_location,
            "days": days,
            "before": {
                "items": before_items,
                "devices": before_devices,
            },
            "after": {
                "items": after_items,
                "devices": after_devices,
            },
            "places": places,
            "towns": towns,
        }))
        .into_response()
    })
    .await
}
