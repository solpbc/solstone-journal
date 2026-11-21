// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The map app: a day's location as a list first, and a small map drawn from
//! the journal's own area tier. No tile, basemap or lookup is fetched from
//! anywhere; the browser draws what this crate sends it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as RoutePath, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::{Value, json};
use solstone_core_convey_http::coalesce::ResponseLocale;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_convey_http::window_catalog::{response_locale, text};
use solstone_core_location::accuracy::{StopWords, margin_m};
use solstone_core_location::day::{Device, LocalTime, Point};
use solstone_core_location::places::PlaceStore;
use solstone_core_location::{
    AreaName, AreaTier, Boundary, Bounds, DayLocation, Entry, Reconciler, SourceRef, TownPoint,
};

mod accuracy;
mod places;

const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
/// How often the background pass checks every day's marker.
const MAINTENANCE_PERIOD: Duration = Duration::from_secs(20);
const MAP_TOWNS: usize = 40;

struct AppState {
    journal_root: PathBuf,
    shared_shell: fn() -> Response,
}

/// The map app's routes.
pub fn router(journal_root: PathBuf, shared_shell: fn() -> Response) -> Router {
    Router::new()
        .route("/app/map/", get(root))
        .route("/app/map/workspace", get(workspace))
        .route("/app/map/{day}", get(day_shell))
        .route("/app/map/api/index", get(index))
        .route("/app/map/api/stats/{month}", get(month_stats))
        .route("/app/map/api/day/{day}", get(day_view))
        .route("/app/map/places", get(day_shell_static))
        .route("/app/map/place/{id}", get(place_shell))
        .route("/app/map/week/{day}", get(week_shell))
        .route("/app/map/month/{month}", get(month_shell))
        .route("/app/map/api/places", get(places::list))
        .route(
            "/app/map/api/place/{id}",
            get(places::show).post(places::edit),
        )
        .route("/app/map/api/week/{day}", get(places::week))
        .route("/app/map/api/month/{month}", get(places::month))
        .route("/app/map/api/accuracy", get(accuracy::get))
        .route("/app/map/api/accuracy/download", post(accuracy::download))
        .route("/app/map/api/accuracy/remove", post(accuracy::remove))
        .with_state(Arc::new(AppState {
            journal_root,
            shared_shell,
        }))
}

/// Keep every day's location search document current: once now, then on a
/// short period. A day costs one small read unless its segments changed.
pub fn start_maintenance(journal_root: PathBuf) {
    let spawned = std::thread::Builder::new()
        .name("location-documents".to_owned())
        .spawn(move || {
            let mut reconciler = Reconciler::default();
            loop {
                match solstone_core_location::reconcile_journal(&journal_root, &mut reconciler) {
                    Ok(report) => {
                        for failure in &report.failures {
                            log::warn!("location document: {failure}");
                        }
                    }
                    Err(error) => {
                        log::warn!("location documents paused: {error}");
                    }
                }
                std::thread::sleep(MAINTENANCE_PERIOD);
            }
        });
    if let Err(error) = spawned {
        log::warn!("location documents could not start: {error}");
    }
}

/// Run the maintained reconciliation pass under an embedding owner's Stop.
/// An admitted pass finishes before this future returns; no blocking work is
/// detached when Stop arrives.
pub async fn run_maintenance_until_stopped(
    root: Arc<solstone_core_journal_io::JournalRoot>,
    mut stop: tokio::sync::watch::Receiver<bool>,
    active: Arc<dyn Fn() -> bool + Send + Sync>,
    mut recovery: tokio::sync::watch::Receiver<()>,
) {
    use solstone_core_journal_io::whole_journal_restore::{
        begin_target_use_bound, whole_journal_restore_blocks, whole_journal_restore_generation,
    };
    let mut reconciler = Reconciler::default();
    let mut generation = whole_journal_restore_generation(root.canonical_path());
    let mut rescan = false;
    loop {
        if *stop.borrow() || !active() || root.revalidate_canonical_binding().is_err() {
            return;
        }
        let current_generation = whole_journal_restore_generation(root.canonical_path());
        if rescan || generation != current_generation {
            reconciler = Reconciler::default();
            generation = current_generation;
            rescan = false;
        }
        let lease = match begin_target_use_bound(Arc::clone(&root)) {
            Ok(lease) => Arc::new(lease),
            Err(_) => {
                tokio::select! {
                    _ = stop.changed() => return,
                    result = recovery.changed() => { if result.is_err() { return; } },
                }
                rescan = true;
                continue;
            }
        };
        let frame_root = Arc::clone(&root);
        let frame_lease = Arc::clone(&lease);
        let pass_active = Arc::clone(&active);
        let pass_stop = stop.clone();
        match tokio::task::spawn_blocking(move || {
            let _frame_lease = frame_lease;
            if !*pass_stop.borrow()
                && pass_active()
                && !whole_journal_restore_blocks(frame_root.canonical_path())
            {
                match solstone_core_location::reconcile_journal_owned(
                    Arc::clone(&frame_root),
                    &mut reconciler,
                ) {
                    Ok(report) => {
                        for failure in report.failures {
                            log::warn!("location document: {failure}");
                        }
                    }
                    Err(error) => log::warn!("location documents paused: {error}"),
                }
            }
            reconciler
        })
        .await
        {
            Ok(state) => reconciler = state,
            Err(error) => {
                log::warn!("location documents worker: {error}");
                return;
            }
        }
        drop(lease);
        tokio::select! {
            _ = stop.changed() => return,
            result = recovery.changed() => {
                if result.is_err() { return; }
                rescan = true;
            },
            _ = tokio::time::sleep(MAINTENANCE_PERIOD) => {},
        }
    }
}

pub(crate) fn valid_day(day: &str) -> bool {
    day.len() == 8
        && day.bytes().all(|byte| byte.is_ascii_digit())
        && solstone_core_time::parse_day(day).is_some()
}

pub(crate) fn invalid_day(locale: &str) -> Response {
    error_envelope(
        "invalid_day",
        text(locale, "journal.window.map.response.day.invalid"),
        text(locale, "journal.window.map.response.day.invalid.detail"),
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

pub(crate) fn valid_month(month: &str) -> bool {
    month.len() == 6
        && month.bytes().all(|byte| byte.is_ascii_digit())
        && solstone_core_time::parse_day(&format!("{month}01")).is_some()
}

pub(crate) fn invalid_month(locale: &str, status: StatusCode) -> Response {
    error_envelope(
        "invalid_month",
        text(locale, "journal.window.map.response.month.invalid"),
        text(locale, "journal.window.map.response.month.invalid.detail"),
        status,
    )
    .into_response()
}

fn html(bytes: &'static [u8]) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .expect("workspace response")
}

async fn workspace() -> Response {
    html(WORKSPACE)
}

fn shell(state: &AppState) -> Response {
    let mut response = (state.shared_shell)();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn root(State(state): State<Arc<AppState>>) -> Response {
    let journal_root = state.journal_root.clone();
    let shell_state = state.clone();
    spawn_blocking_response(OwnerReadRole::MapRoot, move || {
        match solstone_core_location::list_location_days(&journal_root).last() {
            Some((day, _)) => Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, format!("/app/map/{day}"))
                .body(Body::empty())
                .expect("redirect builds"),
            None => shell(&shell_state),
        }
    })
    .await
}

async fn day_shell_static(State(state): State<Arc<AppState>>) -> Response {
    shell(&state)
}

async fn place_shell(
    State(state): State<Arc<AppState>>,
    RoutePath(id): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    if !places::valid_id(&id) {
        return places::unknown_place(locale);
    }
    shell(&state)
}

async fn week_shell(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    if !valid_day(&day) {
        return invalid_day(locale);
    }
    shell(&state)
}

async fn month_shell(
    State(state): State<Arc<AppState>>,
    RoutePath(month): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    if !valid_month(&month) {
        return invalid_month(locale, StatusCode::NOT_FOUND);
    }
    shell(&state)
}

async fn day_shell(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    if !valid_day(&day) {
        return invalid_day(locale);
    }
    shell(&state)
}

fn day_counts(journal: &Path) -> BTreeMap<String, u64> {
    solstone_core_location::list_location_days(journal)
        .into_iter()
        .collect()
}

async fn index(State(state): State<Arc<AppState>>) -> Response {
    let journal_root = state.journal_root.clone();
    spawn_blocking_response(OwnerReadRole::MapIndex, move || {
        let counts = day_counts(&journal_root);
        let mut months = BTreeMap::<String, u64>::new();
        for (day, count) in &counts {
            *months.entry(day[..6].to_owned()).or_default() += count;
        }
        let coverage = counts
            .keys()
            .next()
            .zip(counts.keys().next_back())
            .map(|(start, end)| json!({"start": start, "end": end}));
        Json(json!({"coverage": coverage, "months": months})).into_response()
    })
    .await
}

async fn month_stats(
    State(state): State<Arc<AppState>>,
    RoutePath(month): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    if !valid_month(&month) {
        return invalid_month(locale, StatusCode::BAD_REQUEST);
    }
    let journal_root = state.journal_root.clone();
    spawn_blocking_response(OwnerReadRole::MapMonthStats, move || {
        let days: BTreeMap<String, u64> = day_counts(&journal_root)
            .into_iter()
            .filter(|(day, _)| day.starts_with(&month))
            .collect();
        Json(json!(days)).into_response()
    })
    .await
}

async fn day_view(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
    locale: Option<Extension<ResponseLocale>>,
) -> Response {
    let locale = response_locale(locale);
    if !valid_day(&day) {
        return invalid_day(locale);
    }
    let journal_root = state.journal_root.clone();
    spawn_blocking_response(OwnerReadRole::MapDay, move || {
        let view = match solstone_core_location::read_journal_day(&journal_root, &day) {
            Ok(view) => view,
            Err(error) => {
                return places::names_unavailable(locale, error.to_string());
            }
        };
        let tier = match solstone_core_location::area_tier() {
            Ok(tier) => tier,
            Err(error) => {
                return places::names_unavailable(locale, error.to_string());
            }
        };
        let days = solstone_core_location::list_location_days(&journal_root);
        let store = PlaceStore::read(&journal_root);
        let mut payload = day_payload(&view, tier, &days, &store, locale);
        payload["accuracy"] = accuracy::status(&journal_root, locale);
        Json(payload).into_response()
    })
    .await
}

pub(crate) fn time_json(time: &LocalTime) -> Value {
    json!({"utc": solstone_core_time::utc_rfc3339(time.utc), "clock": time.clock(), "day": time.day()})
}

pub(crate) fn round(value: f64) -> f64 {
    (value * 1e5).round() / 1e5
}

pub(crate) fn point_json(point: &Point) -> Value {
    json!([
        round(point.latitude),
        round(point.longitude),
        point.accuracy_m.map(f64::round)
    ])
}

pub(crate) fn sources_json(sources: &[SourceRef]) -> Value {
    Value::Array(
        sources
            .iter()
            .map(|source| {
                json!({
                    "stream": source.stream,
                    "layout": source.layout.as_str(),
                    "segment": source.segment,
                    "lines": source.lines,
                    "url": source.transcripts_url(),
                })
            })
            .collect(),
    )
}

pub(crate) fn area_json(area: &AreaName, locale: &str) -> Value {
    json!({
        "label": area.label_in(locale),
        "words": area.words(),
        "town": area.town.as_ref().map(|town| town.name.clone()),
        "town_km": area.town.as_ref().map(|town| (town.distance_m / 100.0).round() / 10.0),
        "within": area.within,
    })
}

fn outline_json(rings: &[Vec<[f64; 2]>]) -> Value {
    Value::Array(
        rings
            .iter()
            .map(|ring| {
                json!(
                    ring.iter()
                        .map(|[x, y]| json!([round(*y), round(*x)]))
                        .collect::<Vec<_>>()
                )
            })
            .collect(),
    )
}

pub(crate) fn place_json(place: &StopWords) -> Value {
    json!({
        "margin_m": place.margin_m.round(),
        "at": place.at.as_ref().map(|at| json!({
            "name": at.name,
            "layer": at.layer,
            "class": at.class,
            "inside": at.inside,
            "outline": outline_json(&at.outline),
        })),
        "near": place.near.iter().map(|near| json!({
            "name": near.name,
            "category": solstone_core_location::projection::category_words(&near.category),
            "distance_m": near.distance_m.round(),
        })).collect::<Vec<_>>(),
    })
}

fn device_json(view: &DayLocation, index: usize, device: &Device, locale: &str) -> Value {
    let precision_reduced = view.entries.iter().any(|entry| {
        matches!(entry, Entry::Readings { device: owner, precision, .. }
            if *owner == index && *precision == solstone_core_location::record::Precision::Reduced)
    }) && device.platform == solstone_core_location::record::Platform::Ios;
    json!({
        "precision_reduced": precision_reduced,
        "phrase": view.device_phrase_in(index, locale),
        "word": device.word_in(locale),
        "word_key": device.word,
        "visible_name": view.distinguishing_device_name(index),
        "platform": device.platform.as_str(),
        "tier": device.tier,
        "readings": device.readings,
        "visits": device.visits,
        "first": device.first.as_ref().map(time_json),
        "last": device.last.as_ref().map(time_json),
        "interrupted_segments": device.interrupted_segments,
        "unreadable_lines": device.unreadable_lines,
        "unrecognised_lines": device.unrecognised_lines,
        "unreadable_files": device.unreadable_files,
    })
}

/// The place a visit or stay belongs to, as the owner sees it.
pub(crate) fn place_ref(store: &PlaceStore, point: &Point, locale: &str) -> Value {
    let Some(margin) = margin_m(point.accuracy_m) else {
        return Value::Null;
    };
    match store.place_for(point.latitude, point.longitude, margin) {
        Some(place) => json!({
            "id": place.id,
            "owner_name": place.owner_name_in(locale),
            "hidden": place.hidden,
        }),
        None => Value::Null,
    }
}

pub(crate) fn entry_json(entry: &Entry, store: &PlaceStore, locale: &str) -> Value {
    match entry {
        Entry::Readings {
            device,
            area,
            precision,
            first,
            last,
            points,
            sources,
        } => json!({
            "kind": "readings",
            "device": device,
            "area": area_json(area, locale),
            "precision": precision.as_str(),
            "first": time_json(first),
            "last": time_json(last),
            "count": points.len(),
            "points": points.iter().map(point_json).collect::<Vec<_>>(),
            // What the map may draw: nothing near a place the owner hid.
            "map_points": points
                .iter()
                .filter(|point| !store.hides(point.latitude, point.longitude))
                .map(point_json)
                .collect::<Vec<_>>(),
            "segments": sources_json(sources),
        }),
        Entry::Visit {
            device,
            area,
            place,
            arrival,
            departure,
            point,
            sources,
        } => json!({
            "kind": "visit",
            "device": device,
            "area": area_json(area, locale),
            "place": place.as_ref().map(place_json),
            "place_ref": place_ref(store, point, locale),
            "on_map": !store.hides(point.latitude, point.longitude),
            "arrival": arrival.as_ref().map(time_json),
            "departure": departure.as_ref().map(time_json),
            "point": point_json(point),
            "segments": sources_json(sources),
        }),
        Entry::Stay {
            device,
            area,
            place,
            first,
            last,
            count,
            point,
            sources,
        } => json!({
            "kind": "stay",
            "device": device,
            "area": area_json(area, locale),
            "place": place.as_ref().map(place_json),
            "place_ref": place_ref(store, point, locale),
            "on_map": !store.hides(point.latitude, point.longitude),
            "first": time_json(first),
            "last": time_json(last),
            "count": count,
            "point": point_json(point),
            "segments": sources_json(sources),
        }),
        Entry::Unreadable {
            device,
            at,
            lines,
            whole_file,
            sources,
        } => json!({
            "kind": "unreadable",
            "device": device,
            "at": at.as_ref().map(time_json),
            "lines": lines,
            "whole_file": whole_file,
            "segments": sources_json(sources),
        }),
    }
}

/// The smallest box holding every point the day's records draw, or `None`.
fn record_bounds(view: &DayLocation, store: &PlaceStore) -> Option<Bounds> {
    let mut bounds: Option<Bounds> = None;
    let mut include = |point: &Point| {
        if store.hides(point.latitude, point.longitude) {
            return;
        }
        let reach = point.accuracy_m.unwrap_or(0.0).min(50_000.0) / 111_195.0;
        let cos = point.latitude.to_radians().cos().max(0.05);
        let next = Bounds {
            west: point.longitude - reach / cos,
            south: point.latitude - reach,
            east: point.longitude + reach / cos,
            north: point.latitude + reach,
        };
        bounds = Some(match bounds {
            None => next,
            Some(current) => Bounds {
                west: current.west.min(next.west),
                south: current.south.min(next.south),
                east: current.east.max(next.east),
                north: current.north.max(next.north),
            },
        });
    };
    for entry in &view.entries {
        match entry {
            Entry::Readings { points, .. } => points.iter().for_each(&mut include),
            Entry::Visit { point, .. } | Entry::Stay { point, .. } => include(point),
            Entry::Unreadable { .. } => {}
        }
    }
    bounds
}

/// The area drawn around the records: wide enough to show the towns around
/// them, never narrower than about ten kilometres.
fn context_bounds(records: Bounds) -> Bounds {
    let latitude_span = (records.north - records.south).max(0.1);
    let longitude_span = (records.east - records.west).max(0.1);
    Bounds {
        west: (records.west - longitude_span).max(-180.0),
        south: (records.south - latitude_span).max(-90.0),
        east: (records.east + longitude_span).min(180.0),
        north: (records.north + latitude_span).min(90.0),
    }
}

fn bounds_json(bounds: &Bounds) -> Value {
    json!([
        round(bounds.west),
        round(bounds.south),
        round(bounds.east),
        round(bounds.north)
    ])
}

fn boundary_json(boundary: &Boundary) -> Value {
    json!({
        "name": boundary.name,
        "lines": boundary
            .rings
            .iter()
            .map(|line| line.iter().map(|[x, y]| json!([round(*y), round(*x)])).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
    })
}

pub(crate) fn town_json(town: &TownPoint) -> Value {
    json!({"name": town.name, "lat": round(town.latitude), "lon": round(town.longitude), "population": town.population})
}

fn day_payload(
    view: &DayLocation,
    tier: &AreaTier,
    days: &[(String, u64)],
    store: &PlaceStore,
    locale: &str,
) -> Value {
    let records = record_bounds(view, store);
    let map = records.map(|records| {
        let context = context_bounds(records);
        json!({
            "bounds": bounds_json(&records),
            "context": bounds_json(&context),
            "towns": tier.towns_in(context, MAP_TOWNS).iter().map(town_json).collect::<Vec<_>>(),
            "regions": tier.region_boundaries_in(context).iter().map(boundary_json).collect::<Vec<_>>(),
            "countries": tier.country_boundaries_in(context).iter().map(boundary_json).collect::<Vec<_>>(),
        })
    });
    let previous = days
        .iter()
        .rev()
        .find(|(day, _)| day.as_str() < view.day.as_str())
        .map(|(day, _)| day.clone());
    let next = days
        .iter()
        .find(|(day, _)| day.as_str() > view.day.as_str())
        .map(|(day, _)| day.clone());
    json!({
        "day": view.day,
        "devices": view
            .devices
            .iter()
            .enumerate()
            .map(|(index, device)| device_json(view, index, device, locale))
            .collect::<Vec<_>>(),
        "entries": view.entries.iter().map(|entry| entry_json(entry, store, locale)).collect::<Vec<_>>(),
        "map": map,
        "journal_has_location": !days.is_empty() || !view.is_empty(),
        "nearby": {"previous": previous, "next": next},
    })
}

#[cfg(test)]
mod tests;
