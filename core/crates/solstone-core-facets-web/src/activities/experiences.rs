// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Headless Convey JSON HTTP API routes for Experiences under `/app/activities/api/...`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use axum::{
    Json,
    body::Bytes,
    extract::{Path as RoutePath, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_facets::{
    ActivityRef, ExperienceActivitiesMutation, ExperienceDirectPatch, ExperienceMoment,
    ExperienceStoreError, ShareDetailPolicy, create_owner_experience, get_experience,
    hydrate_experience, list_declared_facet_names, list_experience_day_stems,
    list_experiences_in_range, load_activity_records, load_experiences,
    load_experiences_overlapping_day, mutate_experience_activities, observe_facet_declaration,
    project_experience_for_share, set_experience_hidden, update_experience_direct,
};
use solstone_core_time::{Precision, utc_z};

use super::{ACTOR_HEADER, ACTOR_OWNER, internal, invalid, not_found, query_value};
use crate::{Clock, http};

fn actor_from_headers(headers: &HeaderMap) -> String {
    headers
        .get(ACTOR_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(ACTOR_OWNER)
        .to_owned()
}

fn map_store_error(error: ExperienceStoreError) -> Response {
    match error {
        ExperienceStoreError::MissingExperience { id, .. } => not_found(id),
        ExperienceStoreError::InvalidDay(detail)
        | ExperienceStoreError::InvalidExperienceId(detail) => invalid(http::Detail::Raw(detail)),
        ExperienceStoreError::EmptyActivities => invalid(http::Detail::Raw(
            "an experience requires at least one constituent activity".to_owned(),
        )),
        ExperienceStoreError::DestinationMuted { facet } => {
            invalid(http::Detail::Raw(format!("facet `{facet}` is muted")))
        }
        other => internal(other.to_string()),
    }
}

fn resolve_experience_facet(
    root: &Path,
    query: Option<&str>,
    anchor_day: &str,
    exp_id: &str,
) -> Result<String, Box<Response>> {
    if let Some(facet) = query_value(query, "facet").filter(|s| !s.is_empty()) {
        return Ok(facet);
    }
    let facets = list_declared_facet_names(root).map_err(|e| Box::new(internal(e.to_string())))?;
    let mut matches = Vec::new();
    for facet in facets {
        if get_experience(root, &facet, anchor_day, exp_id)
            .ok()
            .flatten()
            .is_some()
        {
            matches.push(facet);
        }
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(Box::new(not_found(exp_id.to_owned()))),
        _ => Err(Box::new(invalid(http::Detail::Raw(
            "facet query parameter is required when multiple facets match".to_owned(),
        )))),
    }
}

fn parse_share_tier(raw: Option<&str>) -> Result<ShareDetailPolicy, Box<Response>> {
    match raw.unwrap_or("overview").trim().to_lowercase().as_str() {
        "overview" | "overview_only" => Ok(ShareDetailPolicy::OverviewOnly),
        "moments" | "overview_moments_media" => Ok(ShareDetailPolicy::OverviewMomentsMedia),
        "full" | "full_hydrated" | "full_hydrated_detail" => {
            Ok(ShareDetailPolicy::FullHydratedDetail)
        }
        other => Err(Box::new(invalid(http::Detail::Raw(format!(
            "unsupported share tier `{other}` (expected overview, moments, or full)"
        ))))),
    }
}

fn unmuted_facets(root: &Path) -> Vec<String> {
    list_declared_facet_names(root)
        .unwrap_or_default()
        .into_iter()
        .filter(|facet| {
            observe_facet_declaration(root, facet)
                .ok()
                .flatten()
                .is_none_or(|d| d.muted != Some(true))
        })
        .collect()
}

fn facet_summary_json(root: &Path, facet_name: &str, count: usize) -> Value {
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
                "muted": d.muted.unwrap_or(false),
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
            "muted": false,
        }),
    }
}

/// Non-hidden Experiences per anchor day across non-muted facets.
pub(super) fn experience_day_counts(root: &Path) -> BTreeMap<String, usize> {
    experience_day_counts_between(root, "00000000", "99999999")
}

/// Non-hidden Experiences per anchor day across non-muted facets within `[from, to]`.
///
/// Accepts lexical `YYYYMMDD` bounds including year sentinels such as `20260000..=20269999`
/// used by `/app/year/api/view`.
pub(super) fn experience_day_counts_between(
    root: &Path,
    from: &str,
    to: &str,
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for facet in unmuted_facets(root) {
        let Ok(stems) = list_experience_day_stems(root, &facet) else {
            continue;
        };
        for day in stems {
            if day.as_str() < from || day.as_str() > to {
                continue;
            }
            if let Ok(rows) = load_experiences(root, &facet, &day, false) {
                let visible = rows.into_iter().filter(|row| !row.hidden).count();
                if visible > 0 {
                    *counts.entry(day).or_insert(0) += visible;
                }
            }
        }
    }
    counts
}

/// Years on or before `through` that hold at least one non-hidden Experience.
pub(super) fn experience_years(root: &Path, through: &str) -> BTreeSet<String> {
    let facets = unmuted_facets(root);
    let mut by_year: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for facet in &facets {
        if let Ok(stems) = list_experience_day_stems(root, facet) {
            for day in stems {
                if day.as_str() <= through
                    && let Some(year) = day.get(..4)
                {
                    by_year
                        .entry(year.to_owned())
                        .or_default()
                        .push((facet.clone(), day));
                }
            }
        }
    }
    by_year
        .into_iter()
        .filter(|(_, entries)| {
            entries.iter().any(|(facet, day)| {
                load_experiences(root, facet, day, false)
                    .map(|rows| !rows.is_empty())
                    .unwrap_or(false)
            })
        })
        .map(|(year, _)| year)
        .collect()
}

/// `GET /app/experiences/api/index?facet={slug}`
pub(super) async fn index(
    State((root, clock)): State<(PathBuf, Clock)>,
    RawQuery(query): RawQuery,
) -> Response {
    let declared = match list_declared_facet_names(&root) {
        Ok(names) => names,
        Err(error) => return internal(error.to_string()),
    };
    let filter_facet = query_value(query.as_deref(), "facet").filter(|s| !s.is_empty());
    let mut counts = BTreeMap::new();
    let mut per_facet_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut hidden_count = 0_usize;
    for slug in &declared {
        let is_muted = observe_facet_declaration(&root, slug)
            .ok()
            .flatten()
            .is_some_and(|d| d.muted == Some(true));
        let in_scope = match &filter_facet {
            Some(f) => f == slug,
            None => !is_muted,
        };
        let rows = match list_experiences_in_range(&root, slug, "00010101", "99991231", true) {
            Ok(rows) => rows,
            Err(error) => return map_store_error(error),
        };
        let mut visible_in_facet = 0_usize;
        for row in rows {
            if row.hidden {
                if in_scope {
                    hidden_count += 1;
                }
            } else {
                visible_in_facet += 1;
                if in_scope {
                    *counts.entry(row.anchor_day).or_insert(0) += 1;
                }
            }
        }
        per_facet_counts.insert(slug.clone(), visible_in_facet);
    }
    let coverage = match (counts.keys().next(), counts.keys().next_back()) {
        (Some(start), Some(end)) => json!({ "start": start, "end": end }),
        _ => Value::Null,
    };
    let facets: Vec<Value> = declared
        .iter()
        .map(|slug| {
            let c = per_facet_counts.get(slug).copied().unwrap_or(0);
            facet_summary_json(&root, slug, c)
        })
        .collect();
    let today = solstone_core_time::day_key(clock.now().date());
    Json(json!({
        "today": today,
        "coverage": coverage,
        "counts": counts,
        "hidden_count": hidden_count,
        "facets": facets,
    }))
    .into_response()
}

/// `GET /app/activities/api/experiences?facet={slug}&before={day}&limit={n}&from={YYYYMMDD}&to={YYYYMMDD}&include_hidden=0|1&order=desc|asc`
pub(super) async fn list_range(
    State((root, _)): State<(PathBuf, Clock)>,
    RawQuery(query): RawQuery,
) -> Response {
    let from = query_value(query.as_deref(), "from").filter(|s| !s.is_empty());
    let to = query_value(query.as_deref(), "to").filter(|s| !s.is_empty());
    let before = query_value(query.as_deref(), "before").filter(|s| !s.is_empty());
    let limit = match query_value(query.as_deref(), "limit").filter(|s| !s.is_empty()) {
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if n > 0 => Some(n.min(500)),
            _ => {
                return invalid(http::Detail::Raw(format!(
                    "invalid limit `{raw}`: expected positive integer"
                )));
            }
        },
        None => None,
    };
    if let Some(ref d) = from
        && !crate::segments::is_day(d)
    {
        return super::invalid_day_not_found();
    }
    if let Some(ref d) = to
        && !crate::segments::is_day(d)
    {
        return super::invalid_day_not_found();
    }
    if let Some(ref d) = before
        && !crate::segments::is_day(d)
    {
        return super::invalid_day_not_found();
    }
    let from_bound = from.as_deref().unwrap_or("00010101");
    let to_bound = to.as_deref().unwrap_or("99991231");
    let facet = query_value(query.as_deref(), "facet").filter(|s| !s.is_empty());
    let include_hidden = query_value(query.as_deref(), "include_hidden").as_deref() == Some("1");
    let ascending = query_value(query.as_deref(), "order").as_deref() == Some("asc");
    let declared = match list_declared_facet_names(&root) {
        Ok(names) => names,
        Err(error) => return internal(error.to_string()),
    };
    let facets_scope = match facet {
        Some(ref f) => vec![f.clone()],
        None => declared
            .iter()
            .filter(|slug| {
                observe_facet_declaration(&root, slug)
                    .ok()
                    .flatten()
                    .is_none_or(|d| d.muted != Some(true))
            })
            .cloned()
            .collect(),
    };
    let mut all = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_facet_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut hidden_count = 0_usize;
    for slug in &declared {
        let in_scope = facets_scope.iter().any(|s| s == slug);
        match list_experiences_in_range(&root, slug, from_bound, to_bound, true) {
            Ok(rows) => {
                let mut visible_in_facet = 0_usize;
                for row in rows {
                    if row.hidden {
                        if in_scope {
                            hidden_count += 1;
                            if include_hidden {
                                all.push(row);
                            }
                        }
                    } else {
                        visible_in_facet += 1;
                        if in_scope {
                            *counts.entry(row.anchor_day.clone()).or_insert(0) += 1;
                            all.push(row);
                        }
                    }
                }
                per_facet_counts.insert(slug.clone(), visible_in_facet);
            }
            Err(error) => return map_store_error(error),
        }
    }
    let facet_summaries: Vec<Value> = declared
        .iter()
        .map(|slug| {
            let c = per_facet_counts.get(slug).copied().unwrap_or(0);
            facet_summary_json(&root, slug, c)
        })
        .collect();
    if let Some(ref before_day) = before {
        all.retain(|row| row.anchor_day.as_str() < before_day.as_str());
    }
    if ascending {
        all.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.id.cmp(&b.id))
        });
    } else {
        all.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| b.id.cmp(&a.id))
        });
    }
    let mut has_more = false;
    let mut next_before: Option<String> = None;
    if let Some(cap) = limit
        && all.len() > cap
    {
        // Keep complete day boundaries so `before={next_before}` never skips a sibling
        // Experience on the boundary day.
        let boundary_day = all[cap - 1].anchor_day.clone();
        let mut take_n = cap;
        while take_n < all.len() && all[take_n].anchor_day == boundary_day {
            take_n += 1;
        }
        if take_n < all.len() {
            has_more = true;
            next_before = Some(boundary_day);
            all.truncate(take_n);
        }
    }
    let hydrated: Vec<_> = all
        .into_iter()
        .map(|rec| hydrate_experience(&root, rec, None))
        .collect();
    Json(json!({
        "from": from,
        "to": to,
        "before": before,
        "limit": limit,
        "has_more": has_more,
        "next_before": next_before,
        "facet": facet,
        "include_hidden": include_hidden,
        "hidden_count": hidden_count,
        "counts": counts,
        "facets": facet_summaries,
        "experiences": hydrated,
    }))
    .into_response()
}

/// `GET /app/activities/api/experiences/stats/{month}?facet={slug}`
pub(super) async fn month_stats(
    State((root, _)): State<(PathBuf, Clock)>,
    RoutePath(month): RoutePath<String>,
    RawQuery(query): RawQuery,
) -> Response {
    if !crate::segments::is_month(&month) {
        return invalid(http::Detail::Raw(format!(
            "invalid month `{month}`: expected YYYYMM"
        )));
    }
    let facet = query_value(query.as_deref(), "facet").filter(|s| !s.is_empty());
    let facets = match facet {
        Some(f) => vec![f],
        None => match list_declared_facet_names(&root) {
            Ok(names) => names,
            Err(error) => return internal(error.to_string()),
        },
    };
    let year: i16 = month[0..4].parse().unwrap_or(2026);
    let mon: i8 = month[4..6].parse().unwrap_or(1);
    let last_day = jiff::civil::Date::new(year, mon, 1)
        .map(|d| d.days_in_month())
        .unwrap_or(28);
    let from = format!("{month}01");
    let to = format!("{month}{last_day:02}");
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for slug in &facets {
        match list_experiences_in_range(&root, slug, &from, &to, false) {
            Ok(rows) => {
                for row in rows {
                    if !row.hidden && row.anchor_day.starts_with(&month) {
                        *counts.entry(row.anchor_day).or_insert(0) += 1;
                    }
                }
            }
            Err(error) => return map_store_error(error),
        }
    }
    Json(counts).into_response()
}

/// `GET /app/activities/api/day/{day}/experiences?facet={slug}&include_hidden=0|1`
pub(super) async fn list_day(
    State((root, _)): State<(PathBuf, Clock)>,
    RoutePath(day): RoutePath<String>,
    RawQuery(query): RawQuery,
) -> Response {
    if !crate::segments::is_day(&day) {
        return super::invalid_day_not_found();
    }
    let facet = query_value(query.as_deref(), "facet").filter(|s| !s.is_empty());
    let include_hidden = query_value(query.as_deref(), "include_hidden").as_deref() == Some("1");
    let facets = match facet {
        Some(f) => vec![f],
        None => match list_declared_facet_names(&root) {
            Ok(names) => names,
            Err(error) => return internal(error.to_string()),
        },
    };
    let mut experiences = Vec::new();
    for f in facets {
        match load_experiences_overlapping_day(&root, &f, &day, include_hidden) {
            Ok(rows) => {
                for row in rows {
                    experiences.push(hydrate_experience(&root, row, None));
                }
            }
            Err(error) => return map_store_error(error),
        }
    }
    experiences.sort_by(|a, b| {
        a.experience
            .started_at
            .cmp(&b.experience.started_at)
            .then_with(|| a.experience.id.cmp(&b.experience.id))
    });
    Json(json!({
        "day": day,
        "experiences": experiences,
    }))
    .into_response()
}

/// `GET /app/activities/api/day/{anchor_day}/experience/{exp_id}?facet={slug}&include_cross_facet_candidates=0|1`
pub(super) async fn detail(
    State((root, _)): State<(PathBuf, Clock)>,
    RoutePath((anchor_day, exp_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
) -> Response {
    if !crate::segments::is_day(&anchor_day) {
        return super::invalid_day_not_found();
    }
    let facet = match resolve_experience_facet(&root, query.as_deref(), &anchor_day, &exp_id) {
        Ok(f) => f,
        Err(resp) => return *resp,
    };
    let include_cross =
        query_value(query.as_deref(), "include_cross_facet_candidates").as_deref() == Some("1");
    match get_experience(&root, &facet, &anchor_day, &exp_id) {
        Ok(Some(exp)) => {
            let hydrated = hydrate_experience(&root, exp, Some(include_cross));
            Json(json!({ "experience": hydrated })).into_response()
        }
        Ok(None) => not_found(exp_id),
        Err(error) => map_store_error(error),
    }
}

#[derive(Deserialize)]
struct CreateExperienceRequest {
    primary_facet: String,
    activities: Vec<ActivityRef>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    locations: Option<Vec<String>>,
    #[serde(default)]
    note: Option<String>,
}

/// `POST /app/activities/api/experiences/create`
pub(super) async fn create(
    State((root, clock)): State<(PathBuf, Clock)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let payload: CreateExperienceRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(error) => return invalid(http::Detail::Raw(format!("invalid JSON body: {error}"))),
    };
    let primary_facet = payload.primary_facet.trim();
    if primary_facet.is_empty() {
        return invalid(http::Detail::Raw(
            "primary_facet must not be empty".to_owned(),
        ));
    }
    if payload.activities.is_empty() {
        return invalid(http::Detail::Raw(
            "activities must include at least one activity ref".to_owned(),
        ));
    }
    let actor = actor_from_headers(&headers);
    let now_iso = utc_z(clock.now().timestamp(), Precision::Secs);
    let initial_locations = payload.locations;
    let initial_note = payload.note;
    let normalized_activities: Vec<ActivityRef> = payload
        .activities
        .into_iter()
        .map(|mut act_ref| {
            let eff_facet = act_ref.effective_facet(primary_facet).to_owned();
            if (act_ref.stream == "default" || act_ref.stream == "_default")
                && let Ok(rows) = load_activity_records(&root, &eff_facet, &act_ref.day, true)
            {
                let has_exact = rows.iter().any(|row| {
                    row.get("id").and_then(Value::as_str) == Some(act_ref.id.as_str())
                        && row.get("stream").and_then(Value::as_str).unwrap_or("")
                            == act_ref.stream.as_str()
                });
                let has_empty = rows.iter().any(|row| {
                    row.get("id").and_then(Value::as_str) == Some(act_ref.id.as_str())
                        && row
                            .get("stream")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .is_empty()
                });
                if !has_exact && has_empty {
                    act_ref.stream.clear();
                }
            }
            act_ref
        })
        .collect();
    match create_owner_experience(
        &root,
        primary_facet,
        normalized_activities,
        payload.title,
        payload.summary,
        &actor,
        &now_iso,
    ) {
        Ok(mut record) => {
            if initial_locations.as_ref().is_some_and(|l| !l.is_empty()) {
                let patch = ExperienceDirectPatch {
                    title: None,
                    summary: None,
                    locations: initial_locations,
                    moments: None,
                    release_holds: Vec::new(),
                    note: initial_note,
                };
                if let Ok(patched) = update_experience_direct(
                    &root,
                    &record.primary_facet,
                    &record.anchor_day,
                    &record.id,
                    patch,
                    &actor,
                    &now_iso,
                ) {
                    record = patched;
                }
            }
            let asked = solstone_core_system::owner_ask::ask_experience_compose(
                &root,
                &record.primary_facet,
                &record.anchor_day,
                &record.id,
                Some("owner_create"),
            )
            .is_ok();
            let hydrated = hydrate_experience(&root, record, Some(false));
            (
                StatusCode::CREATED,
                Json(json!({
                    "experience": hydrated,
                    "asked": asked,
                })),
            )
                .into_response()
        }
        Err(error) => map_store_error(error),
    }
}

#[derive(Deserialize)]
struct UpdateExperienceRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    locations: Option<Vec<String>>,
    #[serde(default)]
    moments: Option<Vec<ExperienceMoment>>,
    #[serde(default)]
    release_holds: Vec<String>,
    #[serde(default)]
    clear_holds: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

/// `POST /app/activities/api/day/{anchor_day}/experience/{exp_id}/update?facet={slug}`
pub(super) async fn update(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((anchor_day, exp_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !crate::segments::is_day(&anchor_day) {
        return super::invalid_day_not_found();
    }
    let facet = match resolve_experience_facet(&root, query.as_deref(), &anchor_day, &exp_id) {
        Ok(f) => f,
        Err(resp) => return *resp,
    };
    let payload: UpdateExperienceRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(error) => return invalid(http::Detail::Raw(format!("invalid JSON body: {error}"))),
    };
    let mut combined_release_holds = payload.release_holds;
    for hold in payload.clear_holds {
        if !combined_release_holds.contains(&hold) {
            combined_release_holds.push(hold);
        }
    }
    let released_any = !combined_release_holds.is_empty();
    let patch = ExperienceDirectPatch {
        title: payload.title,
        summary: payload.summary,
        locations: payload.locations,
        moments: payload.moments,
        release_holds: combined_release_holds,
        note: payload.note,
    };
    let actor = actor_from_headers(&headers);
    let now_iso = utc_z(clock.now().timestamp(), Precision::Secs);
    match update_experience_direct(&root, &facet, &anchor_day, &exp_id, patch, &actor, &now_iso) {
        Ok(record) => {
            let asked = if released_any {
                solstone_core_system::owner_ask::ask_experience_compose(
                    &root,
                    &record.primary_facet,
                    &record.anchor_day,
                    &record.id,
                    Some("owner_release_hold"),
                )
                .is_ok()
            } else {
                false
            };
            let hydrated = hydrate_experience(&root, record, Some(false));
            Json(json!({
                "experience": hydrated,
                "asked": asked,
            }))
            .into_response()
        }
        Err(error) => map_store_error(error),
    }
}

#[derive(Deserialize)]
struct MutateActivitiesRequest {
    #[serde(default)]
    add: Vec<ActivityRef>,
    #[serde(default)]
    remove: Vec<ActivityRef>,
    #[serde(default)]
    note: Option<String>,
}

/// `POST /app/activities/api/day/{anchor_day}/experience/{exp_id}/activities?facet={slug}`
pub(super) async fn mutate_activities(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((anchor_day, exp_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !crate::segments::is_day(&anchor_day) {
        return super::invalid_day_not_found();
    }
    let facet = match resolve_experience_facet(&root, query.as_deref(), &anchor_day, &exp_id) {
        Ok(f) => f,
        Err(resp) => return *resp,
    };
    let payload: MutateActivitiesRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(error) => return invalid(http::Detail::Raw(format!("invalid JSON body: {error}"))),
    };
    let mutation = ExperienceActivitiesMutation {
        add: payload.add,
        remove: payload.remove,
        note: payload.note,
    };
    let actor = actor_from_headers(&headers);
    let now_iso = utc_z(clock.now().timestamp(), Precision::Secs);
    match mutate_experience_activities(
        &root,
        &facet,
        &anchor_day,
        &exp_id,
        mutation,
        &actor,
        &now_iso,
    ) {
        Ok(record) => {
            let asked = solstone_core_system::owner_ask::ask_experience_compose(
                &root,
                &record.primary_facet,
                &record.anchor_day,
                &record.id,
                Some("owner_activities"),
            )
            .is_ok();
            let hydrated = hydrate_experience(&root, record, Some(false));
            Json(json!({
                "experience": hydrated,
                "asked": asked,
            }))
            .into_response()
        }
        Err(error) => map_store_error(error),
    }
}

#[derive(Default, Deserialize)]
struct MuteBody {
    #[serde(default)]
    note: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn set_hidden(
    root: PathBuf,
    clock: Clock,
    anchor_day: String,
    exp_id: String,
    query: Option<String>,
    headers: HeaderMap,
    body: Bytes,
    hidden: bool,
) -> Response {
    if !crate::segments::is_day(&anchor_day) {
        return super::invalid_day_not_found();
    }
    let facet = match resolve_experience_facet(&root, query.as_deref(), &anchor_day, &exp_id) {
        Ok(f) => f,
        Err(resp) => return *resp,
    };
    let parsed: MuteBody = if body.is_empty() {
        MuteBody::default()
    } else {
        serde_json::from_slice(&body).unwrap_or_default()
    };
    let actor = actor_from_headers(&headers);
    let now_iso = utc_z(clock.now().timestamp(), Precision::Secs);
    match set_experience_hidden(
        &root,
        &facet,
        &anchor_day,
        &exp_id,
        hidden,
        &actor,
        parsed.note.as_deref(),
        &now_iso,
    ) {
        Ok(record) => {
            let hydrated = hydrate_experience(&root, record, None);
            Json(json!({ "experience": hydrated })).into_response()
        }
        Err(error) => map_store_error(error),
    }
}

/// `POST /app/activities/api/day/{anchor_day}/experience/{exp_id}/mute?facet={slug}`
pub(super) async fn mute(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((anchor_day, exp_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    set_hidden(root, clock, anchor_day, exp_id, query, headers, body, true).await
}

/// `POST /app/activities/api/day/{anchor_day}/experience/{exp_id}/unmute?facet={slug}`
pub(super) async fn unmute(
    State((root, clock)): State<(PathBuf, Clock)>,
    RoutePath((anchor_day, exp_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    set_hidden(root, clock, anchor_day, exp_id, query, headers, body, false).await
}

/// `GET /app/activities/api/day/{anchor_day}/experience/{exp_id}/share-preview?facet={slug}&tier=overview|moments|full`
pub(super) async fn share_preview(
    State((root, _)): State<(PathBuf, Clock)>,
    RoutePath((anchor_day, exp_id)): RoutePath<(String, String)>,
    RawQuery(query): RawQuery,
) -> Response {
    if !crate::segments::is_day(&anchor_day) {
        return super::invalid_day_not_found();
    }
    let facet = match resolve_experience_facet(&root, query.as_deref(), &anchor_day, &exp_id) {
        Ok(f) => f,
        Err(resp) => return *resp,
    };
    let tier = match parse_share_tier(query_value(query.as_deref(), "tier").as_deref()) {
        Ok(t) => t,
        Err(resp) => return *resp,
    };
    match get_experience(&root, &facet, &anchor_day, &exp_id) {
        Ok(Some(exp)) => {
            let hydrated = hydrate_experience(&root, exp, None);
            let preview = project_experience_for_share(&hydrated, tier);
            Json(json!({ "preview": preview })).into_response()
        }
        Ok(None) => not_found(exp_id),
        Err(error) => map_store_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use serde_json::Value;
    use solstone_core_facets::{append_activity_record, create_facet};
    use tempfile::TempDir;
    use tower::ServiceExt;

    async fn read_json(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let val = serde_json::from_slice(&bytes).expect("json");
        (status, val)
    }

    #[tokio::test]
    async fn headless_experience_api_full_lifecycle_and_share_preview() {
        let dir = TempDir::new().unwrap();
        create_facet(
            dir.path(),
            "personal",
            "Personal",
            "Personal",
            "#6366f1",
            "✨",
            None,
        )
        .unwrap();
        create_facet(dir.path(), "work", "Work", "Work", "#0ea5e9", "💼", None).unwrap();

        let mut act1 = serde_json::Map::new();
        act1.insert("id".to_owned(), json!("social_180000_600"));
        act1.insert("stream".to_owned(), json!("watch"));
        act1.insert("activity".to_owned(), json!("social"));
        act1.insert("description".to_owned(), json!("Dinner with Maya"));
        act1.insert("segments".to_owned(), json!(["180000_600"]));
        let _ = append_activity_record(dir.path(), "personal", "20260304", act1).unwrap();

        let mut act2 = serde_json::Map::new();
        act2.insert("id".to_owned(), json!("meeting_183000_600"));
        act2.insert("stream".to_owned(), json!("watch"));
        act2.insert("activity".to_owned(), json!("meeting"));
        act2.insert("description".to_owned(), json!("Quick check-in"));
        act2.insert("segments".to_owned(), json!(["183000_600"]));
        let _ = append_activity_record(dir.path(), "work", "20260304", act2).unwrap();

        let app = super::super::routes(dir.path().to_path_buf(), Clock::owner(dir.path()));

        // 1. Create
        let (status, created) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/app/activities/api/experiences/create")
                        .header("content-type", "application/json")
                        .header(ACTOR_HEADER, ACTOR_OWNER)
                        .body(Body::from(
                            json!({
                                "primary_facet": "personal",
                                "activities": [{"day": "20260304", "stream": "watch", "id": "social_180000_600"}],
                                "title": "Evening dinner",
                                "summary": "Alex met Maya for dinner."
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created["asked"], true);
        let exp_id = created["experience"]["id"].as_str().unwrap().to_owned();

        // 2. List day
        let (status, listed) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/app/activities/api/day/20260304/experiences?facet=personal")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["experiences"].as_array().unwrap().len(), 1);
        assert_eq!(
            listed["experiences"][0]["hydrated"]["activities"][0]["segment_evidence"][0]["transcript_url"],
            "/app/transcripts/20260304?stream=watch#180000_600"
        );
        assert!(listed["experiences"][0]["available_media"].is_array());

        // 2b. Multi-day stream & per-day counts
        let (status, stream_resp) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/app/activities/api/experiences?facet=personal&before=20260310&limit=10")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(stream_resp["experiences"].as_array().unwrap().len(), 1);
        assert_eq!(stream_resp["counts"]["20260304"], 1);
        assert_eq!(stream_resp["has_more"], false);

        // 2c. Month stats (including 28-day February safety)
        let (status, march_stats) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/app/activities/api/experiences/stats/202603?facet=personal")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(march_stats["20260304"], 1);
        let (status, feb_stats) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/app/activities/api/experiences/stats/202602?facet=personal")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(feb_stats, json!({}));

        // 3. Detail with cross-facet nearby candidates
        let (status, det) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!(
                            "/app/activities/api/day/20260304/experience/{exp_id}?facet=personal&include_cross_facet_candidates=1"
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            det["experience"]["nearby_candidates"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["ref"]["id"] == "meeting_183000_600")
        );

        // 4. Add cross-facet activity (B1 explicit owner add)
        let (status, added) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/app/activities/api/day/20260304/experience/{exp_id}/activities?facet=personal"
                        ))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({
                                "add": [{"day": "20260304", "stream": "watch", "id": "meeting_183000_600", "facet": "work"}]
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            added["experience"]["activities"].as_array().unwrap().len(),
            2
        );

        // 5. Direct update title + moments
        let (status, updated) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/app/activities/api/day/20260304/experience/{exp_id}/update?facet=personal"
                        ))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({
                                "title": "Dinner and check-in",
                                "moments": [{
                                    "id": "m1",
                                    "started_at": "2026-03-04 18:00",
                                    "ended_at": "2026-03-04 18:40",
                                    "summary": "Alex had dinner and checked in.",
                                    "activity_refs": [{"day": "20260304", "stream": "watch", "id": "social_180000_600"}],
                                    "segment_refs": ["20260304/watch/180000_600"],
                                    "media_refs": []
                                }]
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["experience"]["owner_holds"]["title"], true);

        // 5b. Clear holds (`clear_holds`) -> releases holds & enqueues OwnerAsk experience_compose
        let (status, cleared) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/app/activities/api/day/20260304/experience/{exp_id}/update?facet=personal"
                        ))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({
                                "clear_holds": ["title", "moment:m1"]
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(cleared["asked"], true);
        assert_eq!(cleared["experience"]["owner_holds"]["title"], false);
        assert!(
            cleared["experience"]["owner_holds"]["moment_ids"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        // 5c. Day activities view exposes experience membership back-pointers cleanly
        //     (including cross-facet constituent activity in `work`).
        let (status, day_view) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/app/activities/api/day/20260304/view")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let view_items = day_view["items"].as_array().unwrap();
        for expected_id in ["social_180000_600", "meeting_183000_600"] {
            let act_item = view_items
                .iter()
                .find(|it| it["id"] == expected_id)
                .expect("activity present in day view");
            let backrefs = act_item["experiences"]
                .as_array()
                .expect("experience back-pointer attached");
            assert_eq!(backrefs.len(), 1);
            assert_eq!(backrefs[0]["id"], exp_id);
            assert_eq!(backrefs[0]["primary_facet"], "personal");
            assert_eq!(backrefs[0]["anchor_day"], "20260304");
        }

        // 6. Share preview tiers
        let (status, preview_ov) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!(
                            "/app/activities/api/day/20260304/experience/{exp_id}/share-preview?facet=personal&tier=overview"
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(preview_ov["preview"]["policy"], "overview_only");
        assert!(preview_ov["preview"].get("moments").is_none());

        let (status, preview_full) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(format!(
                            "/app/activities/api/day/20260304/experience/{exp_id}/share-preview?facet=personal&tier=full"
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(preview_full["preview"]["policy"], "full_hydrated_detail");
        assert_eq!(
            preview_full["preview"]["moments"].as_array().unwrap().len(),
            1
        );

        // 7. Mute & Unmute
        let (status, muted) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/app/activities/api/day/20260304/experience/{exp_id}/mute?facet=personal"
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(muted["experience"]["hidden"], true);

        let (status, listed_after_mute) = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/app/activities/api/day/20260304/experiences?facet=personal")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            listed_after_mute["experiences"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let (status, unmuted) = read_json(
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/app/activities/api/day/20260304/experience/{exp_id}/unmute?facet=personal"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(unmuted["experience"]["hidden"], false);

        // Year sentinel bounds ("20260000".."20269999") used by /app/year/api/view must count 20260304.
        let year_counts = experience_day_counts_between(dir.path(), "20260000", "20269999");
        assert_eq!(year_counts.get("20260304"), Some(&1));
    }
}
