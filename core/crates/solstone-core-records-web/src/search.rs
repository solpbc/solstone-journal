// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{collections::BTreeMap, fs, path::PathBuf};

use axum::{
    Router,
    extract::Query,
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use chrono::{Datelike, Local, NaiveDate};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_indexer_query::{
    IndexedEntry, SearchRequest, read_indexed_entry, search, search_counts,
};
use solstone_core_journal_io::bounded_read::{JournalReadError, MAX_BYTES, read_text};

use crate::talent_outputs;

const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const WORKSPACE: &str = include_str!("../assets/search.html");

pub fn router(journal_root: PathBuf) -> Router {
    let search_root = journal_root.clone();
    let agents_root = journal_root.clone();
    let entry_root = journal_root.clone();
    Router::new()
        .route(
            "/app/search/api/entry",
            get(move |query: Query<EntryQuery>| entry_api(entry_root.clone(), query)),
        )
        .route(
            "/app/search/api/search",
            get(move |query: Query<SearchQuery>| search_api(search_root.clone(), query)),
        )
        .route(
            "/app/search/api/agents",
            get(move |query: Query<AgentsQuery>| agents_api(agents_root.clone(), query)),
        )
        .route(
            "/app/search/api/read",
            get(move |query: Query<ReadQuery>| read_api(journal_root.clone(), query)),
        )
}

pub async fn shell() -> Response {
    asset(SHELL)
}

pub async fn root() -> Redirect {
    Redirect::permanent("/app/search/")
}

pub async fn workspace() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(WORKSPACE))
        .expect("embedded search workspace response")
}

#[derive(Default, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    facet: Option<String>,
    agent: Option<String>,
    stream: Option<String>,
    time_bucket: Option<String>,
    day_from: Option<String>,
    day_to: Option<String>,
}

async fn search_api(journal_root: PathBuf, Query(query): Query<SearchQuery>) -> Response {
    let (day_from, day_to) = match day_range(query.day_from.as_deref(), query.day_to.as_deref()) {
        Ok(range) => range,
        Err(detail) => return invalid_day(&detail),
    };
    let request = SearchRequest {
        query: query.q.unwrap_or_default().trim().to_owned(),
        limit: query.limit.unwrap_or(5).clamp(1, 100),
        offset: 0,
        day: None,
        day_from: None,
        day_to: None,
        facet: none_if_blank(query.facet),
        agent: none_if_blank(query.agent),
        stream: none_if_blank(query.stream),
        time_bucket: none_if_blank(query.time_bucket),
        relax: true,
        counts: false,
        order: Default::default(),
    };
    let reference = today();
    let mut base_request = request.clone();
    base_request.facet = None;
    base_request.agent = None;
    let base = match search_counts(&journal_root, &base_request, reference) {
        Ok(counts) => counts,
        Err(error) => return search_failed(&error.to_string()),
    };
    let filtered = match search_counts(&journal_root, &request, reference) {
        Ok(counts) => counts,
        Err(error) => return search_failed(&error.to_string()),
    };
    let mut days = filtered
        .days
        .iter()
        .filter(|(day, _)| in_range(day, day_from.as_deref(), day_to.as_deref()))
        .map(|(day, count)| (day.clone(), *count))
        .collect::<Vec<_>>();
    days.sort_by(|left, right| right.0.cmp(&left.0));
    let total = if day_from.is_some() || day_to.is_some() {
        days.iter().map(|(_, count)| count).sum::<u64>()
    } else {
        filtered.total
    };
    let total_days = days.len();
    let offset = query.offset.unwrap_or(0);
    let page = days.into_iter().skip(offset).take(20).collect::<Vec<_>>();
    let mut day_results = Vec::new();
    let facets = facets(&journal_root);
    for (day, total) in &page {
        let mut per_day = request.clone();
        per_day.day = Some(day.clone());
        per_day.limit = request.limit;
        let response = match search(&journal_root, &per_day, reference) {
            Ok(response) => response,
            Err(error) => return search_failed(&error.to_string()),
        };
        let results = response
            .results
            .into_iter()
            .map(|hit| {
                json!({
                    "id": hit.id,
                    "entry_id": hit.row_id,
                    "day": hit.metadata.day,
                    "agent": hit.metadata.agent,
                    "agent_label": agent_label(&hit.metadata.agent),
                    "facet": hit.metadata.facet,
                    "facet_title": facets.get(&hit.metadata.facet).map_or(&hit.metadata.facet, |facet| &facet.title),
                    "text": highlight(&hit.text, &request.query),
                    "stream": hit.metadata.stream,
                    "path": hit.metadata.path,
                    "idx": hit.metadata.idx,
                    "score": hit.score,
                })
            })
            .collect::<Vec<_>>();
        day_results.push(json!({
            "day": day,
            "date": format_date(day),
            "total": total,
            "showing": results.len(),
            "results": results,
        }));
    }
    axum::Json(json!({
        "total": total,
        "total_days": total_days,
        "relaxed": filtered.relaxed,
        "days": day_results,
        "facets": facet_counts(&facets, &base.facets),
        "talents": talent_counts(&base.agents),
    }))
    .into_response()
}

#[derive(Default, Deserialize)]
struct AgentsQuery {
    day: Option<String>,
    segment: Option<String>,
}

async fn agents_api(journal_root: PathBuf, Query(query): Query<AgentsQuery>) -> Response {
    let day = query.day.unwrap_or_else(today_day);
    if !valid_day(&day) {
        return invalid_day("day must be YYYYMMDD");
    }
    match talent_outputs::list(&journal_root, &day, none_if_blank(query.segment).as_deref()) {
        Ok(value) => axum::Json(value).into_response(),
        Err(detail) => invalid_value(&detail),
    }
}

#[derive(Deserialize)]
struct EntryQuery {
    path: String,
    idx: i64,
    entry_id: i64,
}

async fn entry_api(journal_root: PathBuf, Query(query): Query<EntryQuery>) -> Response {
    if query.path.is_empty() || query.path.len() > 4096 || query.idx < 0 || query.entry_id <= 0 {
        return invalid_value("invalid entry reference");
    }
    let result = tokio::task::spawn_blocking(move || {
        read_indexed_entry(
            &journal_root,
            &query.path,
            query.idx,
            query.entry_id,
            MAX_BYTES,
        )
    })
    .await;
    match result {
        Ok(Ok(IndexedEntry::Found(content))) => {
            axum::Json(json!({"content": content})).into_response()
        }
        Ok(Ok(IndexedEntry::TooLarge)) => invalid_value("this entry is too long to display here."),
        Ok(Ok(IndexedEntry::NotFound)) => {
            file_not_found("this entry is no longer indexed. search again to refresh the results.")
        }
        Ok(Err(_)) | Err(_) => file_read_failed("the search index couldn't be read. try again."),
    }
}

#[derive(Default, Deserialize)]
struct ReadQuery {
    path: Option<String>,
    agent: Option<String>,
    day: Option<String>,
    segment: Option<String>,
    max_bytes: Option<u64>,
}

async fn read_api(journal_root: PathBuf, Query(query): Query<ReadQuery>) -> Response {
    if query.max_bytes.unwrap_or(MAX_BYTES) != MAX_BYTES {
        return invalid_value("max_bytes must be 16384 for HTTP reads");
    }
    let rel = if let Some(path) = query.path {
        if query.agent.is_some() || query.day.is_some() || query.segment.is_some() {
            return invalid_path("path cannot be combined with agent, day, or segment");
        }
        if path.starts_with("entity_search:") {
            return invalid_path("entity results have no file to read");
        }
        if has_index_suffix(&path) {
            return invalid_path("strip the search-result :idx suffix before reading a path");
        }
        path
    } else {
        let Some(agent) = query.agent.filter(|agent| !agent.is_empty()) else {
            return invalid_value("agent or path is required");
        };
        let day = query.day.unwrap_or_else(today_day);
        if !valid_day(&day) {
            return invalid_day("day must be YYYYMMDD");
        }
        match talent_outputs::find(
            &journal_root,
            &agent,
            &day,
            none_if_blank(query.segment).as_deref(),
        ) {
            Ok(path) => path,
            Err(talent_outputs::FindError::Invalid(detail)) => return invalid_value(&detail),
            Err(talent_outputs::FindError::NotFound) => {
                return file_not_found("talent output not found");
            }
        }
    };
    match read_text(&journal_root, &rel) {
        Ok(content) => axum::Json(json!({"path": rel, "content": content})).into_response(),
        Err(JournalReadError::Path(detail)) => invalid_path(&detail),
        Err(JournalReadError::NotFound) => file_not_found("journal file not found"),
        Err(JournalReadError::TooLarge(detail)) => invalid_value(&detail),
        Err(JournalReadError::Encoding(detail)) => file_read_failed(&detail),
        Err(JournalReadError::Io) => file_read_failed("unable to read journal file"),
    }
}

fn day_range(
    from: Option<&str>,
    to: Option<&str>,
) -> Result<(Option<String>, Option<String>), String> {
    let from = parse_day_bound("day_from", from)?;
    let to = parse_day_bound("day_to", to)?;
    if from
        .as_ref()
        .zip(to.as_ref())
        .is_some_and(|(from, to)| from > to)
    {
        return Err("day_from must be <= day_to".into());
    }
    Ok((from, to))
}

fn parse_day_bound(name: &str, value: Option<&str>) -> Result<Option<String>, String> {
    let value = value.unwrap_or("").trim();
    if value.is_empty() || matches!(value, "00000000" | "99999999") {
        return Ok(None);
    }
    if !valid_day(value) {
        return Err(format!("{name} must be YYYYMMDD"));
    }
    NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| format!("{name} must be a real day"))?;
    Ok(Some(value.to_owned()))
}

fn in_range(day: &str, from: Option<&str>, to: Option<&str>) -> bool {
    from.is_none_or(|from| day >= from) && to.is_none_or(|to| day <= to)
}

fn none_if_blank(value: Option<String>) -> Option<String> {
    value.and_then(|value| (!value.trim().is_empty()).then(|| value.trim().to_owned()))
}

fn valid_day(day: &str) -> bool {
    day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit())
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

fn today_day() -> String {
    Local::now().format("%Y%m%d").to_string()
}

struct Facet {
    title: String,
    color: String,
    emoji: String,
    muted: bool,
}

fn facets(journal_root: &std::path::Path) -> BTreeMap<String, Facet> {
    let mut facets = BTreeMap::new();
    let Ok(entries) = fs::read_dir(journal_root.join("facets")) else {
        return facets;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(source) = fs::read_to_string(entry.path().join("facet.json")) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&source) else {
            continue;
        };
        facets.insert(
            name.clone(),
            Facet {
                title: value["title"].as_str().unwrap_or(&name).to_owned(),
                color: value["color"].as_str().unwrap_or_default().to_owned(),
                emoji: value["emoji"].as_str().unwrap_or_default().to_owned(),
                muted: value["muted"].as_bool().unwrap_or(false),
            },
        );
    }
    facets
}

fn facet_counts(facets: &BTreeMap<String, Facet>, counts: &BTreeMap<String, u64>) -> Vec<Value> {
    let mut values = facets
        .iter()
        .filter(|(_, facet)| !facet.muted)
        .map(|(name, facet)| json!({"name": name, "title": facet.title, "color": facet.color, "emoji": facet.emoji, "count": counts.get(name).copied().unwrap_or(0)}))
        .collect::<Vec<_>>();
    values.sort_by_key(|value| std::cmp::Reverse(value["count"].as_u64().unwrap_or(0)));
    values
}

fn talent_counts(counts: &BTreeMap<String, u64>) -> Vec<Value> {
    counts
        .iter()
        .map(|(name, count)| json!({"name": name, "label": agent_label(name), "icon": agent_icon(name), "count": count}))
        .collect()
}

fn agent_icon(agent: &str) -> &'static str {
    match agent {
        "flow" => "activity",
        "meetings" => "users",
        "screen" => "monitor",
        "audio" => "mic-vocal",
        "entity" => "user",
        "news" => "newspaper",
        "import" => "import",
        _ => "file-text",
    }
}

fn format_date(day: &str) -> String {
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").expect("indexed day is valid");
    let suffix = match date.day() % 100 {
        11..=13 => "th",
        _ => match date.day() % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        },
    };
    format!(
        "{} {} {}{}",
        date.format("%A"),
        date.format("%B"),
        date.day(),
        suffix
    )
}

fn agent_label(agent: &str) -> String {
    agent
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn highlight(text: &str, query: &str) -> String {
    let words = text.split_whitespace().take(50).collect::<Vec<_>>();
    let mut value = html_escape(&words.join(" "));
    if text.split_whitespace().count() > 50 {
        value.push_str("...");
    }
    for term in query
        .split_whitespace()
        .filter(|term| !matches!(term.to_ascii_uppercase().as_str(), "AND" | "OR" | "NOT"))
    {
        let term = term.trim_matches(|character| character == '"' || character == '*');
        if term.len() >= 2 {
            value = replace_case_insensitive(&value, term);
        }
    }
    value
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#x27;")
}

fn replace_case_insensitive(value: &str, term: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let needle = term.to_ascii_lowercase();
    let mut result = String::new();
    let mut start = 0;
    while let Some(offset) = lower[start..].find(&needle) {
        let index = start + offset;
        result.push_str(&value[start..index]);
        result.push_str("<strong>");
        result.push_str(&value[index..index + term.len()]);
        result.push_str("</strong>");
        start = index + term.len();
    }
    result.push_str(&value[start..]);
    result
}

fn has_index_suffix(path: &str) -> bool {
    path.rsplit_once(':').is_some_and(|(_, suffix)| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn asset(bytes: &'static [u8]) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(bytes))
        .expect("embedded search shell response")
}

fn invalid_day(detail: &str) -> Response {
    error_envelope(
        "invalid_day",
        "I couldn't use that day.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
fn invalid_value(detail: &str) -> Response {
    error_envelope(
        "invalid_request_value",
        "I couldn't use one of those values.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
fn invalid_path(detail: &str) -> Response {
    error_envelope(
        "invalid_path",
        "I couldn't use that path.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
fn file_not_found(detail: &str) -> Response {
    error_envelope(
        "file_not_found",
        "I couldn't find that file.",
        detail,
        StatusCode::NOT_FOUND,
    )
    .into_response()
}
fn file_read_failed(detail: &str) -> Response {
    error_envelope(
        "file_read_failed",
        "I couldn't read that file.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}
fn search_failed(detail: &str) -> Response {
    // The reference publishes the underlying error text here, and `sol call journal search`
    // prints this body verbatim into a talent's context. Discarding it left an owner and a
    // talent unable to tell an absent index from a corrupt one. Every other refusal helper in
    // this file already carries a detail; this one was the outlier.
    error_envelope(
        "search_failed",
        "I couldn't search for that.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

#[cfg(test)]
mod search_failure_detail_tests {
    use super::*;

    // The reference publishes the underlying error text with this refusal, and the
    // journal search command prints the response body verbatim into a talent's
    // context. A port answering an empty detail leaves an owner and a talent unable
    // to distinguish an absent index from a corrupt one.
    //
    // Caught in live validation against a running server, not by the frozen corpus:
    // no captured case reaches this branch, because the seeded journal has an index.
    // That is the coverage limit the corpus is meant to state rather than hide.
    #[test]
    fn search_failure_carries_its_detail() {
        let response = search_failed("journal index is absent: /x/indexer/journal.sqlite");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn search_failure_helper_accepts_a_detail_at_all() {
        // Guards the exact regression: the helper previously took no detail, and every
        // call site discarded its error with a wildcard match.
        let response = search_failed("journal index is unreadable");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
