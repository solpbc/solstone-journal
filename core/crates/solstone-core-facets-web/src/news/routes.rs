// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{collections::BTreeMap, fs, path::PathBuf};

use axum::{
    Router,
    body::Body,
    extract::{Path, RawQuery},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};

use super::{copy, dates, store};
use crate::{
    assets,
    clock::Clock,
    date_nav, http, pdf,
    segments::{is_day, is_month},
};

const PLAIN_NOT_FOUND: &str = "Newsletter not found";
// Flask's reference emits its stock 500 page. Its bytes could not be verified from this
// tree, so these are deliberately our own stable HTML 500 bytes.
const HTML_INTERNAL_ERROR: &str = "<!doctype html>\n<html lang=en>\n<title>500 Internal Server Error</title>\n<h1>Internal Server Error</h1>\n";
// Deliberate duplicate of convey-shell's 207-byte body: facets-web cannot depend on
// convey-shell in production because convey-shell already depends on this crate (Cargo cycle).
const SHARED_NOT_FOUND: &str = "<!doctype html>\n<html lang=en>\n<title>404 Not Found</title>\n<h1>Not Found</h1>\n<p>The requested URL was not found on the server. If you entered the URL manually please check your spelling and try again.</p>\n";

pub fn routes(root: PathBuf, clock: Clock) -> Router {
    let state_root = root.clone();
    let state_clock = clock.clone();
    let index_root = root.clone();
    let grid_root = root.clone();
    let grid_clock = clock.clone();
    let stats_root = root.clone();
    let day_root = root.clone();
    let facet_root = root.clone();
    let detail_root = root.clone();
    let raw_root = root.clone();
    let pdf_root = root;
    Router::new()
        .route("/app/news/", get(|| async { assets::shell() }))
        .route(
            "/app/news/workspace",
            get(|| async { assets::news_workspace() }),
        )
        .route("/app/news/background", get(|| async { shared_not_found() }))
        .route(
            "/app/news/api/state",
            get(move || state(state_root.clone(), state_clock.clone())),
        )
        .route(
            "/app/news/api/index",
            get(move || index(index_root.clone())),
        )
        .route(
            "/app/news/api/grid",
            get(move || grid(grid_root.clone(), grid_clock.clone())),
        )
        .route(
            "/app/news/api/stats/{month}",
            get(move |Path(month)| stats(stats_root.clone(), month)),
        )
        .route(
            "/app/news/api/day/{day}",
            get(move |Path(day)| api_day(day_root.clone(), day)),
        )
        .route(
            "/app/news/api/facet/{facet}",
            get(move |Path(facet), query: RawQuery| facet_feed(facet_root.clone(), facet, query)),
        )
        .route("/app/news/sample", get(|| async { assets::shell() }))
        .route("/app/news/api/sample", get(sample_api))
        .route(
            "/app/news/sample/raw",
            get(|| async { markdown(copy::SAMPLE_CONTENT.to_owned()) }),
        )
        .route(
            "/app/news/api/{facet}/{day}",
            get(move |Path((facet, day))| detail(detail_root.clone(), facet, day)),
        )
        .route(
            "/app/news/{facet}/{day}/raw",
            get(move |Path((facet, day))| raw(raw_root.clone(), facet, day)),
        )
        .route(
            "/app/news/{facet}/{day}/pdf",
            get(move |Path((facet, day))| pdf_route(pdf_root.clone(), facet, day)),
        )
        .route(
            "/app/news/{day}",
            get(|Path(day): Path<String>| async move {
                if is_day(&day) {
                    assets::shell()
                } else {
                    invalid_day_not_found()
                }
            }),
        )
        .route("/app/news/{facet}/{day}", get(|| async { assets::shell() }))
}

fn json_response(value: Value) -> Response {
    axum::Json(value).into_response()
}
fn markdown(value: String) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/markdown; charset=utf-8")
        .body(Body::from(value))
        .expect("markdown response")
}
fn plain_not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(PLAIN_NOT_FOUND))
        .expect("not found")
}
fn html_error() -> Response {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(HTML_INTERNAL_ERROR))
        .expect("html error")
}
fn shared_not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(SHARED_NOT_FOUND))
        .expect("404")
}
fn invalid_day_not_found() -> Response {
    http::error(
        "invalid_day",
        "that day couldn't be used.",
        "Day not found".to_owned(),
        StatusCode::NOT_FOUND,
    )
}
fn invalid_request(detail: impl Into<String>) -> Response {
    http::error(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail.into(),
        StatusCode::BAD_REQUEST,
    )
}

// News derives today only from the injected Clock. This deliberately diverges from
// Python's owner-timezone `_today()`; no news code consults local time or a timezone helper.
fn today(clock: &Clock) -> String {
    clock.now().date().format("%Y%m%d").to_string()
}
fn rows(root: &std::path::Path) -> Vec<store::NewsRow> {
    store::list_newsletters(root)
}
fn counts(rows: &[store::NewsRow]) -> BTreeMap<String, usize> {
    rows.iter().fold(BTreeMap::new(), |mut map, row| {
        *map.entry(row.day.clone()).or_default() += 1;
        map
    })
}
fn label_item(row: &store::NewsRow) -> Value {
    json!({"facet": row.facet, "day": row.day, "label": dates::format_news_list_date(&row.day), "url": format!("/app/news/{}/{}", row.facet, row.day)})
}

async fn state(root: PathBuf, _clock: Clock) -> Response {
    let rows = rows(&root);
    let total_count = rows.len();
    let observer = journal_has_any_observer_input(&root);
    let empty_next = if observer {
        copy::NEWS_EMPTY_PENDING.to_owned()
    } else {
        copy::NEWS_EMPTY_NO_DATE.to_owned()
    };
    let grid_lede = rows.last().map(|row| {
        let template = if total_count == 1 {
            copy::NEWS_GRID_LEDE_ONE
        } else {
            copy::NEWS_GRID_LEDE_OTHER
        };
        template
            .replace("{count}", &total_count.to_string())
            .replace("{month}", &dates::format_news_month(&row.day))
    });
    json_response(
        json!({"newsletters": rows.iter().take(60).map(label_item).collect::<Vec<_>>(), "total_count": total_count,
      "copy": {"kicker": copy::NEWS_KICKER, "index_h1": copy::NEWS_INDEX_H1, "subtitle": copy::NEWS_SUBTITLE, "empty_body": copy::NEWS_EMPTY_BODY, "empty_next": empty_next, "empty_until_then": copy::NEWS_EMPTY_UNTIL_THEN, "sample_link_label": copy::NEWS_SAMPLE_LINK_LABEL, "sample_url": "/app/news/sample", "populated_framing": copy::NEWS_POPULATED_FRAMING, "populated_sample_link": copy::NEWS_POPULATED_SAMPLE_LINK, "populated_next_footer": "", "grid_title": copy::NEWS_GRID_TITLE, "grid_lede": grid_lede, "grid_unit_one": copy::NEWS_GRID_UNIT_ONE, "grid_unit_other": copy::NEWS_GRID_UNIT_OTHER, "grid_unit_none": copy::NEWS_GRID_UNIT_NONE}}),
    )
}
async fn index(root: PathBuf) -> Response {
    json_response(date_nav::date_nav_index(&counts(&rows(&root))))
}
async fn grid(root: PathBuf, clock: Clock) -> Response {
    let counts = counts(&rows(&root));
    let coverage = counts
        .keys()
        .next()
        .map(|start| (start.as_str(), today(&clock)));
    json_response(date_nav::day_grid_payload(
        &counts,
        counts.keys().next_back().map(String::as_str),
        coverage.as_ref().map(|(start, end)| (*start, end.as_str())),
    ))
}
async fn stats(root: PathBuf, month: String) -> Response {
    if !is_month(&month) {
        return http::error(
            "invalid_month",
            "that month couldn't be used.",
            "Invalid month format, expected YYYYMM".to_owned(),
            StatusCode::BAD_REQUEST,
        );
    }
    json_response(
        serde_json::to_value(
            counts(&rows(&root))
                .into_iter()
                .filter(|(day, _)| day.starts_with(&month))
                .collect::<BTreeMap<_, _>>(),
        )
        .expect("counts json"),
    )
}
async fn api_day(root: PathBuf, day: String) -> Response {
    if !is_day(&day) {
        return invalid_day_not_found();
    }
    let matching = rows(&root)
        .into_iter()
        .filter(|row| row.day == day)
        .map(|row| label_item(&row))
        .collect::<Vec<_>>();
    let date_label = dates::format_news_list_date(&day);
    let mut value = json!({"day": day, "date_label": date_label, "newsletters": matching, "copy": {"title": copy::NEWS_DAY_TITLE.replace("{date_label}", &date_label), "subtitle": copy::NEWS_DAY_SUBTITLE, "empty_title": copy::NEWS_DAY_EMPTY_TITLE.replace("{date_label}", &date_label), "empty_body": copy::NEWS_DAY_EMPTY_BODY}});
    if value["newsletters"].as_array().is_some_and(Vec::is_empty) {
        value["empty"] = json!(true);
    }
    json_response(value)
}
async fn facet_feed(root: PathBuf, facet: String, RawQuery(query): RawQuery) -> Response {
    if !store::valid_facet(&facet) {
        return invalid_request("invalid facet");
    }
    let query = query_map(query.as_deref());
    let day = query
        .get("day")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let cursor = query
        .get("cursor")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    if day.is_some_and(|value| !is_day(value)) {
        return http::error(
            "invalid_day",
            "that day couldn't be used.",
            "day must be YYYYMMDD".to_owned(),
            StatusCode::BAD_REQUEST,
        );
    }
    if cursor.is_some_and(|value| !is_day(value)) {
        return invalid_request("cursor must be YYYYMMDD");
    }
    let limit = match query
        .get("limit")
        .map_or(Ok(5), |value| value.parse::<usize>().map_err(|_| ()))
    {
        Ok(value) => value,
        Err(()) => return invalid_request("limit must be an integer"),
    };
    if !(1..=100).contains(&limit) {
        return invalid_request("limit must be between 1 and 100");
    }
    let mut value = store::get_facet_news(&root, &facet, cursor, limit, day);
    value
        .as_object_mut()
        .expect("feed")
        .insert("facet".to_owned(), json!(facet));
    json_response(value)
}
fn query_map(query: Option<&str>) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for pair in query.unwrap_or_default().split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        values
            .entry(form_decode(key))
            .or_insert_with(|| form_decode(value));
    }
    values
}

/// Mirrors Flask's request.args: form decoding, key-only values as empty, and
/// the first occurrence of a duplicate key.
fn form_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let Some(high) = hex_value(bytes[index + 1]) else {
                    decoded.push(b'%');
                    index += 1;
                    continue;
                };
                let Some(low) = hex_value(bytes[index + 2]) else {
                    decoded.push(b'%');
                    index += 1;
                    continue;
                };
                decoded.push(high << 4 | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
async fn sample_api() -> Response {
    let content = store::split_frontmatter(copy::SAMPLE_CONTENT).expect("sample valid");
    json_response(
        json!({"markdown": content, "raw_url": "/app/news/sample/raw", "kicker": copy::NEWS_KICKER, "sample_h1": copy::NEWS_SAMPLE_H1, "sample_banner": copy::NEWS_SAMPLE_BANNER}),
    )
}
async fn detail(root: PathBuf, facet: String, day: String) -> Response {
    if !store::valid_facet(&facet) || !is_day(&day) {
        return http::error(
            "file_not_found",
            "that file isn't available.",
            "Newsletter not found".to_owned(),
            StatusCode::NOT_FOUND,
        );
    }
    match load(&root, &facet, &day) {
        Ok(Some((_raw, content))) => json_response(
            json!({"markdown": content, "raw_url": format!("/app/news/{facet}/{day}/raw"), "pdf_url": format!("/app/news/{facet}/{day}/pdf"), "kicker": copy::NEWS_KICKER, "facet": facet, "date_label": dates::format_news_list_date(&day), "subtitle": copy::NEWS_DETAIL_SUBTITLE.replace("{facet}", &facet), "debug_link_label": copy::NEWS_DETAIL_DEBUG_LINK, "debug_link_url": format!("/app/thinking/#runs/{day}/facet_newsletter")}),
        ),
        Ok(None) => json_response(empty_detail(&facet, &day)),
        // Flask's internal_error envelope is 89 B because it appends a newline. Native
        // http::internal_error() is 88 B and the existing test already pins that body, so tests assert
        // the JSON content rather than importing Flask's trailing-newline divergence.
        Err(()) => http::internal_error(),
    }
}
fn empty_detail(facet: &str, day: &str) -> Value {
    let label = dates::format_news_list_date(day);
    json!({"empty": true, "facet": facet, "day": day, "date_label": label, "day_url": format!("/app/news/{day}"), "copy": {"empty_title": copy::NEWS_DETAIL_EMPTY_TITLE.replace("{facet}", facet), "empty_body": copy::NEWS_DETAIL_EMPTY_BODY.replace("{facet}", facet).replace("{date_label}", &label), "day_link": copy::NEWS_DETAIL_EMPTY_DAY_LINK}})
}
async fn raw(root: PathBuf, facet: String, day: String) -> Response {
    if !store::valid_facet(&facet) || !is_day(&day) {
        return plain_not_found();
    }
    match load(&root, &facet, &day) {
        Ok(Some((raw, _))) => markdown(raw),
        Ok(None) => plain_not_found(),
        Err(()) => html_error(),
    }
}
async fn pdf_route(root: PathBuf, facet: String, day: String) -> Response {
    if !store::valid_facet(&facet) || !is_day(&day) {
        return plain_not_found();
    }
    match load(&root, &facet, &day) {
        Ok(Some((_raw, content))) => Response::builder()
            .header(header::CONTENT_TYPE, "application/pdf")
            .header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"newsletter-{facet}-{day}.pdf\""),
            )
            .body(Body::from(pdf::render(
                &content,
                &facet,
                &dates::format_news_list_date(&day),
            )))
            .expect("pdf response"),
        Ok(None) => plain_not_found(),
        Err(()) => html_error(),
    }
}
fn load(root: &std::path::Path, facet: &str, day: &str) -> Result<Option<(String, String)>, ()> {
    let path = root
        .join("facets")
        .join(facet)
        .join("news")
        .join(format!("{day}.md"));
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).map_err(|_| ())?;
    let content = store::split_frontmatter(&raw)?.to_owned();
    Ok(Some((raw, content)))
}
pub(crate) fn journal_has_any_observer_input(root: &std::path::Path) -> bool {
    fs::read_dir(root.join("chronicle"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| entry.path().is_dir() && is_day(&entry.file_name().to_string_lossy()))
}
