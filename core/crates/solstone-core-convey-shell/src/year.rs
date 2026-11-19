// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The year app: one calendar year of what the journal holds, by day and by
//! each place and name by week.
//!
//! Everything here is a count of stored records or a date. Nothing is ranked,
//! scored or picked: rows come back in the order they first appear.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};

use crate::asset_response;

/// The year app's pages and its one read.
pub fn router(journal_root: PathBuf) -> Router {
    Router::new()
        .route("/app/year/", get(shell))
        .route("/app/year/workspace", get(workspace))
        .route("/app/year/static/{*rest}", get(static_file))
        .route("/app/year/api/view", get(view_route))
        .with_state(Arc::new(journal_root))
}

async fn shell() -> Response {
    asset_response("/static/shell.html")
}

async fn workspace() -> Response {
    asset_response("/app/year/workspace")
}

async fn static_file(axum::extract::Path(rest): axum::extract::Path<String>) -> Response {
    asset_response(&format!("/app/year/static/{rest}"))
}

async fn view_route(
    State(root): State<Arc<PathBuf>>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let locale = crate::window_catalog::request_locale(&headers);
    let year = query.get("year").cloned();
    if year.as_deref().is_some_and(|year| !is_year(year)) {
        return error_envelope(
            "invalid_year",
            crate::window_catalog::text(locale, "journal.window.year.year.invalid"),
            crate::window_catalog::text(locale, "journal.window.year.year.invalid.detail"),
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }
    spawn_blocking_response(OwnerReadRole::YearView, move || {
        let zone = solstone_core_journal_config::owner_zone(&root);
        let today =
            solstone_core_time::day_key(jiff::Timestamp::now().to_zoned_datetime(zone).date());
        Json(year_view(&root, &today, year.as_deref())).into_response()
    })
    .await
}

fn is_year(value: &str) -> bool {
    value.len() == 4 && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// One year of the journal. With no `year`, the latest year that holds a day
/// on or before `today`; with nothing held, today's year, empty. Days after
/// `today` are never counted.
pub(crate) fn year_view(root: &Path, today: &str, year: Option<&str>) -> Value {
    let held = |day: &str| day <= today;

    let activities: BTreeMap<String, u64> = solstone_core_facets_web::activity_day_counts(root)
        .into_iter()
        .filter(|(day, count)| held(day) && *count > 0)
        .map(|(day, count)| (day, count as u64))
        .collect();
    let location: BTreeMap<String, u64> = solstone_core_location::list_location_days(root)
        .into_iter()
        .filter(|(day, _)| held(day))
        .collect();

    // Places: what the map shows, less anything the owner hid from it. A
    // hidden place's own name stays out of the names too.
    let store = solstone_core_location::places::PlaceStore::read(root);
    let hidden_place_names: Vec<String> = store
        .places
        .iter()
        .filter(|place| place.hidden)
        .filter_map(|place| place.name.clone())
        .collect();
    let (places, places_unavailable) = match solstone_core_location::read_places(root) {
        Ok(views) => {
            let places = views
                .into_iter()
                .filter(|view| {
                    !view.place.hidden && !store.hides(view.place.latitude, view.place.longitude)
                })
                .map(|view| {
                    let days: BTreeSet<String> = view
                        .stays
                        .iter()
                        .map(|stay| stay.day.clone())
                        .filter(|day| held(day))
                        .collect();
                    (view.place.id.clone(), view.label(), days)
                })
                .collect::<Vec<_>>();
            (places, false)
        }
        Err(error) => {
            log::warn!("year: places not listed: {error}");
            (Vec::new(), true)
        }
    };

    // Names: when any hide can't be read, none are listed.
    let (names, names_unavailable) = match solstone_core_facets::detected_name_days(
        root,
        "00000000",
        today,
        &hidden_place_names,
    ) {
        Ok(names) => (names, false),
        Err(error) => {
            log::warn!("year: names not listed: {error}");
            (Vec::new(), true)
        }
    };

    let mut years: BTreeSet<String> = BTreeSet::new();
    let mut note = |day: &str| {
        if let Some(year) = day.get(..4) {
            years.insert(year.to_owned());
        }
    };
    activities.keys().for_each(|day| note(day));
    location.keys().for_each(|day| note(day));
    for (_, _, days) in &places {
        days.iter().for_each(|day| note(day));
    }
    for name in &names {
        name.days.iter().for_each(|day| note(day));
    }

    let year = year
        .map(str::to_owned)
        .or_else(|| years.last().cloned())
        .unwrap_or_else(|| today.get(..4).unwrap_or_default().to_owned());
    let in_year = |day: &String| day.starts_with(&year);

    // Rows run in the order they first appear in the year, then by label.
    let mut place_rows: Vec<(String, String, Vec<String>)> = places
        .into_iter()
        .map(|(id, label, days)| (id, label, days.into_iter().filter(in_year).collect()))
        .filter(|(_, _, days): &(String, String, Vec<String>)| !days.is_empty())
        .collect();
    place_rows.sort_by(|a, b| (&a.2[0], &a.1, &a.0).cmp(&(&b.2[0], &b.1, &b.0)));
    let mut name_rows: Vec<(String, String, Vec<String>)> = names
        .into_iter()
        .map(|name| {
            let days = name.days.into_iter().filter(in_year).collect();
            (name.entity_type, name.name, days)
        })
        .filter(|(_, _, days): &(String, String, Vec<String>)| !days.is_empty())
        .collect();
    name_rows.sort_by(|a, b| (&a.2[0], &a.1, &a.0).cmp(&(&b.2[0], &b.1, &b.0)));

    let layer = |counts: BTreeMap<String, u64>| -> BTreeMap<String, u64> {
        counts.into_iter().filter(|(day, _)| in_year(day)).collect()
    };
    json!({
        "year": year,
        "years": years,
        "today": today,
        "layers": {
            "activities": layer(activities),
            "location": layer(location),
        },
        "places": place_rows
            .into_iter()
            .map(|(id, label, days)| json!({"id": id, "label": label, "days": days}))
            .collect::<Vec<_>>(),
        "names": name_rows
            .into_iter()
            .map(|(entity_type, name, days)| json!({"type": entity_type, "name": name, "days": days}))
            .collect::<Vec<_>>(),
        "places_unavailable": places_unavailable,
        "names_unavailable": names_unavailable,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use solstone_core_location::places::PlaceStore;
    use tower::ServiceExt;

    use super::{router, year_view};

    const LOCATION_HEADER: &str = r#"{"accuracy":"full","fix_count":2,"gap":false,"kind":"location","platform":"ios","schema":"solstone.location.segment/1","source":"location","tier":"full"}"#;

    fn facet(root: &Path, name: &str) {
        solstone_core_facets::create_facet(root, name, name, "", "", "", None).unwrap();
    }

    fn detect(root: &Path, facet: &str, day: &str, names: &[&str]) {
        let folder = root.join("facets").join(facet).join("entities");
        fs::create_dir_all(&folder).unwrap();
        let lines: String = names
            .iter()
            .map(|name| {
                format!(
                    "{}\n",
                    json!({"type": "Person", "name": name, "description": ""})
                )
            })
            .collect();
        fs::write(folder.join(format!("{day}.jsonl")), lines).unwrap();
    }

    /// A visit on each day at one spot, so the journal holds one place.
    fn visits(root: &Path, days: &[&str], lat: f64) {
        for day in days {
            let iso = format!("{}-{}-{}", &day[..4], &day[4..6], &day[6..8]);
            let dir = root
                .join("chronicle")
                .join(day)
                .join("phone_location")
                .join("150000_300");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("location.jsonl"),
                format!(
                    "{LOCATION_HEADER}\n{{\"arrival\":\"{iso}T21:02:00Z\",\"departure\":\"{iso}T22:40:00Z\",\"h_acc\":20,\"lat\":{lat},\"lon\":-105.0001,\"schema\":\"solstone.location.visit/1\"}}\n"
                ),
            )
            .unwrap();
        }
        let mut reconciler = solstone_core_location::Reconciler::default();
        solstone_core_location::reconcile_journal(root, &mut reconciler).unwrap();
    }

    fn name_rows(view: &Value) -> Vec<(String, Vec<String>)> {
        view["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["name"].as_str().unwrap().to_owned(),
                    row["days"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|day| day.as_str().unwrap().to_owned())
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn an_empty_journal_answers_today_s_year_with_nothing_held() {
        let root = tempfile::tempdir().unwrap();
        let view = year_view(root.path(), "20261117", None);
        assert_eq!(view["year"], "2026");
        assert_eq!(view["years"], json!([]));
        assert_eq!(view["names"], json!([]));
        assert_eq!(view["places"], json!([]));
        assert_eq!(view["names_unavailable"], false);
        assert_eq!(view["places_unavailable"], false);
    }

    #[test]
    fn the_latest_held_year_opens_and_days_after_today_are_not_counted() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        detect(root.path(), "work", "20240305", &["Ada Wong"]);
        detect(root.path(), "work", "20250102", &["Ada Wong"]);
        detect(root.path(), "work", "20270101", &["Ada Wong"]);
        let view = year_view(root.path(), "20261117", None);
        assert_eq!(view["years"], json!(["2024", "2025"]));
        assert_eq!(view["year"], "2025");
        assert_eq!(
            name_rows(&view),
            [("Ada Wong".to_owned(), vec!["20250102".to_owned()])]
        );
        let earlier = year_view(root.path(), "20261117", Some("2024"));
        assert_eq!(
            name_rows(&earlier),
            [("Ada Wong".to_owned(), vec!["20240305".to_owned()])]
        );
    }

    #[test]
    fn names_run_in_the_order_they_first_appear_then_by_name() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        // Zed is found on the most days and still comes last.
        detect(root.path(), "work", "20250301", &["Cy Young", "Ben Okri"]);
        detect(root.path(), "work", "20250302", &["Zed Moss"]);
        detect(root.path(), "work", "20250303", &["Zed Moss"]);
        detect(root.path(), "work", "20250304", &["Zed Moss"]);
        let view = year_view(root.path(), "20261117", None);
        let order: Vec<String> = name_rows(&view).into_iter().map(|row| row.0).collect();
        assert_eq!(order, ["Ben Okri", "Cy Young", "Zed Moss"]);
    }

    #[test]
    fn unreadable_hides_list_no_names_and_say_so() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        detect(root.path(), "work", "20250301", &["Ben Okri"]);
        solstone_core_facets::hide_detected_name(root.path(), "work", "Ada Wong").unwrap();
        let hidden = fs::read_dir(root.path().join("facets/work"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().contains("hidden"))
            })
            .expect("the hide wrote a file in the facet");
        fs::write(hidden, "{").unwrap();
        let view = year_view(root.path(), "20261117", Some("2025"));
        assert_eq!(view["names_unavailable"], true);
        assert_eq!(view["names"], json!([]));
    }

    #[test]
    fn a_place_is_listed_with_its_days_until_the_owner_hides_it() {
        let root = tempfile::tempdir().unwrap();
        visits(root.path(), &["20251001", "20251002"], 39.7527);
        let view = year_view(root.path(), "20261117", None);
        assert_eq!(view["places_unavailable"], false);
        let places = view["places"].as_array().unwrap();
        assert_eq!(places.len(), 1);
        assert_eq!(places[0]["days"], json!(["20251001", "20251002"]));
        assert_eq!(view["layers"]["location"].as_object().unwrap().len(), 2);

        let id = places[0]["id"].as_str().unwrap().to_owned();
        PlaceStore::update(root.path(), |store| {
            for place in &mut store.places {
                if place.id == id {
                    place.hidden = true;
                }
            }
        })
        .unwrap();
        let view = year_view(root.path(), "20261117", None);
        assert_eq!(view["places"], json!([]));
    }

    async fn get(root: &Path, uri: &str) -> (StatusCode, String) {
        let response = router(root.to_path_buf())
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn the_read_answers_a_year_and_refuses_what_is_not_one() {
        let root = tempfile::tempdir().unwrap();
        facet(root.path(), "work");
        detect(root.path(), "work", "20250301", &["Ben Okri"]);
        let (status, body) = get(root.path(), "/app/year/api/view?year=2025").await;
        assert_eq!(status, StatusCode::OK);
        let view: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["year"], "2025");
        assert_eq!(name_rows(&view).len(), 1);

        for bad in ["25", "20255", "abcd", ""] {
            let (status, body) = get(root.path(), &format!("/app/year/api/view?year={bad}")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
            let refusal: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(refusal["reason_code"], "invalid_year", "{bad}");
        }
    }

    #[tokio::test]
    async fn the_page_its_workspace_and_its_script_are_served() {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "/app/year/",
            "/app/year/workspace",
            "/app/year/static/year.js",
        ] {
            let (status, body) = get(root.path(), path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            if path.ends_with("workspace") {
                assert!(body.contains("id=\"year-root\""), "{path}");
            }
        }
    }
}
