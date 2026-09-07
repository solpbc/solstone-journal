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
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_indexer_query::{
    IndexAccessError, IndexedEntry, SearchRequest, read_indexed_entry, search, search_counts,
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
    match tokio::task::spawn_blocking(move || search_response(journal_root, query)).await {
        Ok(response) => response,
        Err(_) => file_read_failed("the search index couldn't be read. try again."),
    }
}

fn search_response(journal_root: PathBuf, query: SearchQuery) -> Response {
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
        Err(error) => return search_failed(&error),
    };
    let filtered = match search_counts(&journal_root, &request, reference) {
        Ok(counts) => counts,
        Err(error) => return search_failed(&error),
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
            Err(error) => return search_failed(&error),
        };
        let results = response
            .results
            .into_iter()
            .map(|hit| {
                let readable = readable_record(&hit.text);
                json!({
                    "id": hit.id,
                    "entry_id": hit.row_id,
                    "day": hit.metadata.day,
                    "agent": hit.metadata.agent,
                    "agent_label": agent_label(&hit.metadata.agent),
                    "facet": hit.metadata.facet,
                    "facet_title": facets.get(&hit.metadata.facet).map_or(&hit.metadata.facet, |facet| &facet.title),
                    "text": excerpt_html(&hit.text, readable.as_ref(), &request.query),
                    "ts": readable.as_ref().and_then(|record| record.ts),
                    "record": readable.as_ref().map(|_| cap_words(&hit.text)),
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
    idx: Option<i64>,
    entry_id: Option<i64>,
    agent: Option<String>,
    day: Option<String>,
    segment: Option<String>,
    max_bytes: Option<u64>,
}

async fn read_api(journal_root: PathBuf, Query(query): Query<ReadQuery>) -> Response {
    if query.max_bytes.unwrap_or(MAX_BYTES) != MAX_BYTES {
        return invalid_value("max_bytes must be 16384 for HTTP reads");
    }
    if query.idx.is_some() || query.entry_id.is_some() {
        if query.agent.is_some() || query.day.is_some() || query.segment.is_some() {
            return invalid_path("an indexed entry cannot be combined with agent, day, or segment");
        }
        let (Some(path), Some(idx), Some(entry_id)) = (query.path, query.idx, query.entry_id)
        else {
            return invalid_value(
                "an indexed entry requires path, idx, and entry_id from the same search result",
            );
        };
        return entry_api(
            journal_root,
            Query(EntryQuery {
                path,
                idx,
                entry_id,
            }),
        )
        .await;
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

/// The human fields of a stored agent record, in the order a reader wants them.
///
/// G2-B04 read these on the client, out of the excerpt the server had already
/// sent. That excerpt is capped at 50 words with `...` appended, so any record
/// longer than 50 words no longer parsed as JSON and the whole readable path
/// silently stopped firing — the owner got braces, keys and a raw model id back
/// for exactly the records with the most to say. The record is read here, where
/// it is still whole, and the cap then applies to the sentences.
const RECORD_TEXT_FIELDS: [&str; 8] = [
    "headline",
    "summary_sentence",
    "summary",
    "text",
    "body",
    "sentence",
    "message",
    "note",
];

/// The readable sentences and timestamp of a stored agent record.
struct ReadableRecord {
    text: String,
    ts: Option<i64>,
}

/// Read a stored `ts` however the record wrote it.
///
/// The indexer writes an integer, but a record carried in from an import can
/// hold the same value as a JSON float or as a string. The client this moved
/// off read it with `Number(record.ts)`, which took all three; reading only
/// `as_i64` here dropped the time off exactly those rows and the card then
/// showed no time at all. The float bound is the largest integer an f64 holds
/// exactly, which is the range `Number()` could carry.
fn record_ts(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    let number = match value {
        Value::Number(_) => value.as_f64(),
        Value::String(text) => {
            let text = text.trim();
            match text.parse::<i64>() {
                Ok(parsed) => return Some(parsed),
                Err(_) => text.parse::<f64>().ok(),
            }
        }
        _ => None,
    }?;
    if !number.is_finite() || number.abs() > 9_007_199_254_740_992.0 {
        return None;
    }
    Some(number as i64)
}

/// A stored `ts` is milliseconds on every record the indexer writes, but a
/// record carried in from an import can hold seconds. 1e11 is early 1973 read
/// as milliseconds and the year 5138 read as seconds, so no real journal
/// timestamp lands under it in milliseconds and a value below it is seconds.
fn record_millis(value: i64) -> Option<i64> {
    if value <= 0 {
        return None;
    }
    Some(if value < 100_000_000_000 {
        value * 1000
    } else {
        value
    })
}

/// Read a stored agent record's human sentences, or `None` when the text is not
/// one.
fn readable_record(text: &str) -> Option<ReadableRecord> {
    let raw = text.trim();
    if !raw.starts_with('{') || !raw.ends_with('}') {
        return None;
    }
    let record: Map<String, Value> = serde_json::from_str(raw).ok()?;
    let mut parts: Vec<String> = Vec::new();
    for field in RECORD_TEXT_FIELDS {
        let part = match record.get(field).and_then(Value::as_str) {
            Some(value) => value.trim(),
            None => continue,
        };
        if part.is_empty() {
            continue;
        }
        // A summary that already opens with the headline would say it twice.
        // Of two values where one contains the other, the longer one is the one
        // that carries everything the shorter one said (F-22).
        if let Some(index) = parts
            .iter()
            .position(|seen| seen.contains(part) || part.contains(seen.as_str()))
        {
            if part.len() > parts[index].len() {
                parts[index] = part.to_owned();
                // The replacement was only ever compared against the one part
                // it matched, so a fragment pushed earlier survives beside it
                // ("... at noon. noon"). Drop every part the longer value now
                // says (F-22).
                let mut cursor = 0;
                parts.retain(|seen| {
                    let keep = cursor == index || !part.contains(seen.as_str());
                    cursor += 1;
                    keep
                });
            }
            continue;
        }
        parts.push(part.to_owned());
    }
    if parts.is_empty() {
        return None;
    }
    // A headline rarely ends in a full stop, so joining on a bare space runs it
    // into the sentence that follows it.
    let mut sentences = String::new();
    for part in parts {
        if sentences.is_empty() {
            sentences.push_str(&part);
            continue;
        }
        if !sentences.ends_with(['.', '!', '?', '\u{2026}']) {
            sentences.push('.');
        }
        sentences.push(' ');
        sentences.push_str(&part);
    }
    Some(ReadableRecord {
        text: sentences,
        ts: record.get("ts").and_then(record_ts).and_then(record_millis),
    })
}

/// The excerpt cap: 50 words, with an ellipsis when there was more.
fn cap_words(text: &str) -> String {
    let mut value = text
        .split_whitespace()
        .take(50)
        .collect::<Vec<_>>()
        .join(" ");
    if text.split_whitespace().count() > 50 {
        value.push_str("...");
    }
    value
}

/// The excerpt the card shows, with the searched terms bolded.
///
/// A row can match on a field the readable sentences never carry -- a tag, a
/// key, the model id -- and the readable excerpt then renders with nothing
/// bolded and no way to see why the row is there at all. When the readable
/// sentences hold no match and the stored record does, the record is what the
/// owner is shown; otherwise the sentences win, which is every ordinary hit.
fn excerpt_html(raw: &str, readable: Option<&ReadableRecord>, query: &str) -> String {
    let Some(record) = readable else {
        return highlight(raw, query);
    };
    let readable_excerpt = highlight(&record.text, query);
    if readable_excerpt.contains("<strong>") {
        return readable_excerpt;
    }
    let raw_excerpt = highlight(raw, query);
    if raw_excerpt.contains("<strong>") {
        raw_excerpt
    } else {
        readable_excerpt
    }
}

fn highlight(text: &str, query: &str) -> String {
    let mut value = html_escape(&cap_words(text));
    for term in highlight_terms(query) {
        if term.len() >= 2 {
            value = replace_case_insensitive(&value, &term);
        }
    }
    value
}

/// Split a raw query into the literal strings the excerpt should bold.
///
/// G2-23: a quoted phrase (`"release burn"`) already compiles to a genuine adjacency
/// requirement in `solstone-core-indexer-query::atomize` (see its
/// `balanced_quoted_phrase` case) — the FTS layer does honor the phrase. This function
/// used to undo that by splitting the raw query on whitespace and bolding each word of
/// a phrase independently, which highlights an unrelated lone occurrence of one word
/// (e.g. "burn" inside "vpe:burn") and reads as a phrase search that silently fell back
/// to separate words. A quoted span here stays one highlight unit — the excerpt only
/// bolds it where the literal phrase actually appears, matching what was searched.
fn highlight_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut cursor = 0;
    while cursor < query.len() {
        while let Some(character) = query[cursor..].chars().next() {
            if !character.is_whitespace() {
                break;
            }
            cursor += character.len_utf8();
        }
        if cursor == query.len() {
            break;
        }
        if query[cursor..].starts_with('"') {
            let start = cursor + 1;
            let end = query[start..]
                .find('"')
                .map_or(query.len(), |offset| start + offset);
            let phrase = query[start..end].trim();
            if !phrase.is_empty() {
                terms.push(phrase.to_owned());
            }
            cursor = if end == query.len() { end } else { end + 1 };
            continue;
        }
        let end = query[cursor..]
            .find(char::is_whitespace)
            .map_or(query.len(), |offset| cursor + offset);
        let word =
            query[cursor..end].trim_matches(|character| character == '"' || character == '*');
        if !word.is_empty() && !matches!(word.to_ascii_uppercase().as_str(), "AND" | "OR" | "NOT") {
            terms.push(word.to_owned());
        }
        cursor = end;
    }
    terms
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
        "that day couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
fn invalid_value(detail: &str) -> Response {
    error_envelope(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
fn invalid_path(detail: &str) -> Response {
    error_envelope(
        "invalid_path",
        "that path couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
fn file_not_found(detail: &str) -> Response {
    error_envelope(
        "file_not_found",
        "that file isn't available.",
        detail,
        StatusCode::NOT_FOUND,
    )
    .into_response()
}
fn file_read_failed(detail: &str) -> Response {
    error_envelope(
        "file_read_failed",
        "that file couldn't be read.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}
fn search_failed(error: &IndexAccessError) -> Response {
    // The reference publishes the underlying error text here, and `sol call journal search`
    // prints this body verbatim into a talent's context. Discarding it left an owner and a
    // talent unable to tell an absent index from a corrupt one. Every other refusal helper in
    // this file already carries a detail; this one was the outlier.
    //
    // G2-21: an `IndexAccessError` (absent, unreadable, locked, or empty index) is never the
    // caller's mistake, so it must not read as one. `solstone-core-entities`'s own search route
    // reaches the same conclusion for the identical error type
    // (`entity_search_index_access_failure` in `solstone-core-entities/src/router.rs`) and
    // answers every variant with 503, not 400. Match that: use `error.reason()` — already
    // documented as "stable machine-readable error reason for the CLI JSON envelope" — as the
    // reason code, and drop the "I" persona from the message; the detail (unchanged: the raw
    // `Display` text, which is what the CLI/talent path above depends on) still carries the
    // absent-vs-corrupt distinction.
    error_envelope(
        error.reason(),
        "couldn't search your journal right now.",
        error.to_string(),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

#[cfg(test)]
mod search_failure_detail_tests {
    use axum::body::to_bytes;

    use super::*;

    async fn body_json(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    // The reference publishes the underlying error text with this refusal, and the
    // journal search command prints the response body verbatim into a talent's
    // context. A port answering an empty detail leaves an owner and a talent unable
    // to distinguish an absent index from a corrupt one.
    //
    // Caught in live validation against a running server, not by the frozen corpus:
    // no captured case reaches this branch, because the seeded journal has an index.
    // That is the coverage limit the corpus is meant to state rather than hide.
    #[tokio::test]
    async fn search_failure_carries_its_detail() {
        let error = IndexAccessError::Absent {
            path: PathBuf::from("/x/indexer/journal.sqlite"),
        };
        let response = search_failed(&error);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        assert_eq!(
            body["detail"],
            "journal index is absent: /x/indexer/journal.sqlite"
        );
        assert_eq!(body["reason_code"], "index_absent");
    }

    #[tokio::test]
    async fn search_failure_helper_accepts_a_detail_at_all() {
        // Guards the exact regression: the helper previously took no detail, and every
        // call site discarded its error with a wildcard match.
        let error = IndexAccessError::Unreadable {
            path: PathBuf::from("/x/indexer/journal.sqlite"),
            detail: "disk I/O error".into(),
        };
        let response = search_failed(&error);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        assert!(
            body["detail"]
                .as_str()
                .expect("detail string")
                .contains("disk I/O error")
        );
    }

    // G2-21: an intermittent index lock (concurrent write) previously surfaced as a plain
    // HTTP 400 with no detail, reading as the owner's mistake. It is a server-side,
    // typically transient condition, so it must not be a 400 and it must say why.
    #[tokio::test]
    async fn search_failure_reports_a_locked_index_as_retryable_not_a_client_error() {
        let error = IndexAccessError::Locked {
            path: PathBuf::from("/x/indexer/journal.sqlite"),
            detail: "database is locked".into(),
        };
        let response = search_failed(&error);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        assert_eq!(body["reason_code"], "index_locked");
        assert!(
            body["detail"]
                .as_str()
                .expect("detail string")
                .contains("database is locked")
        );
        // The refusal is server-side; it does not speak in the first person about the
        // owner's request.
        assert!(!body["error"].as_str().expect("error string").contains('I'));
    }

    #[tokio::test]
    async fn search_failure_distinguishes_reason_codes_by_error_variant() {
        for (error, reason) in [
            (
                IndexAccessError::Absent {
                    path: PathBuf::from("/x"),
                },
                "index_absent",
            ),
            (
                IndexAccessError::Empty {
                    path: PathBuf::from("/x"),
                },
                "empty_index",
            ),
        ] {
            let body = body_json(search_failed(&error)).await;
            assert_eq!(body["reason_code"], reason, "{reason}");
        }
    }
}

#[cfg(test)]
mod highlight_phrase_tests {
    use super::*;

    #[test]
    fn highlight_terms_keeps_a_quoted_phrase_as_one_unit() {
        assert_eq!(
            highlight_terms("\"release burn\""),
            vec!["release burn".to_string()]
        );
    }

    #[test]
    fn highlight_terms_splits_unquoted_words_independently() {
        assert_eq!(
            highlight_terms("release burn"),
            vec!["release".to_string(), "burn".to_string()]
        );
    }

    #[test]
    fn highlight_terms_mixes_a_phrase_with_a_bare_word() {
        assert_eq!(
            highlight_terms("\"release burn\" hopper"),
            vec!["release burn".to_string(), "hopper".to_string()]
        );
    }

    #[test]
    fn highlight_terms_drops_bare_boolean_operators_and_star_wildcards() {
        assert_eq!(
            highlight_terms("release AND burn*"),
            vec!["release".to_string(), "burn".to_string()]
        );
    }

    // G2-23: this is the exact shape the review captured — a quoted phrase whose two
    // words are not adjacent in the visible excerpt. The old word-at-a-time highlighter
    // bolded the lone "burn" inside "vpe:burn", claiming a phrase match the excerpt does
    // not show. Honoring the phrase as one unit means it is not highlighted at all here,
    // which is the honest answer for this excerpt.
    #[test]
    fn highlight_does_not_bold_one_half_of_an_unmatched_phrase() {
        let text = "navigated to vpe:burn where an ai assistant completed a handoff";
        let result = highlight(text, "\"release burn\"");
        assert!(!result.contains("<strong>"));
        assert!(result.contains("vpe:burn"));
    }

    #[test]
    fn highlight_bolds_a_genuinely_adjacent_phrase_as_one_span() {
        let text = "we discussed the release burn timeline next";
        let result = highlight(text, "\"release burn\"");
        assert!(result.contains("<strong>release burn</strong>"));
        // Not split into two independent spans.
        assert!(!result.contains("<strong>release</strong>"));
        assert!(!result.contains("<strong>burn</strong>"));
    }

    // F-5: the readable path used to run on the client, on an excerpt the
    // server had already capped at 50 words — so a record with more than 50
    // words in it stopped parsing as JSON and the owner got the raw record
    // back for exactly the entries with the most to say.
    #[test]
    fn a_long_record_still_reads_as_sentences() {
        let sentence = "the release burn ran again ".repeat(40);
        let record = json!({
            "headline": "thursday afternoon",
            "summary": sentence.trim(),
            "model": "local/qwen3.5-4b",
            "ts": 1_788_662_697_014_i64,
        })
        .to_string();
        assert!(record.split_whitespace().count() > 50);
        let readable = readable_record(&record).expect("a stored record reads");
        assert!(
            readable
                .text
                .starts_with("thursday afternoon. the release burn ran again"),
            "{}",
            readable.text
        );
        assert!(!readable.text.contains("qwen3.5-4b"));
        assert_eq!(readable.ts, Some(1_788_662_697_014));
        // The readable text is what the excerpt is built from now, so the
        // matched terms are still bolded on it (L12).
        let excerpt = highlight(&readable.text, "\"release burn\"");
        assert!(
            excerpt.contains("<strong>release burn</strong>"),
            "{excerpt}"
        );
        assert!(excerpt.ends_with("..."), "{excerpt}");
    }

    #[test]
    fn a_short_record_reads_as_sentences() {
        let record = json!({"headline": "release burn", "summary": "it finished.", "ts": 1_788_662_697_014_i64})
            .to_string();
        let readable = readable_record(&record).expect("a stored record reads");
        assert_eq!(readable.text, "release burn. it finished.");
    }

    // F-22: of two values where one contains the other, the longer one carries
    // everything the shorter one said. Keeping the shorter one threw away the
    // sentence and left the fragment.
    #[test]
    fn a_containing_value_replaces_the_one_it_contains() {
        let record = json!({
            "headline": "release burn",
            "summary": "release burn finished at noon.",
        })
        .to_string();
        let readable = readable_record(&record).expect("a stored record reads");
        assert_eq!(readable.text, "release burn finished at noon.");
    }

    // Fresh-eyes 2 #4: the longer-wins replacement was only compared against
    // the one part it matched, so "noon" pushed before it survived beside the
    // sentence that already said it.
    #[test]
    fn a_replacement_also_drops_the_parts_it_contains() {
        let record = json!({
            "headline": "release burn",
            "summary": "noon",
            "text": "release burn finished at noon.",
        })
        .to_string();
        let readable = readable_record(&record).expect("a stored record reads");
        assert_eq!(readable.text, "release burn finished at noon.");
    }

    // Fresh-eyes 2 #3: a row can match on a field the readable sentences never
    // carry. Bolding nothing reads as a result with no reason to be there, so
    // the stored record is the excerpt for exactly that row.
    #[test]
    fn a_match_outside_the_readable_fields_falls_back_to_the_record() {
        let record = json!({
            "headline": "thursday afternoon",
            "summary": "it finished.",
            "model": "local/qwen3.5-4b",
        })
        .to_string();
        let readable = readable_record(&record).expect("a stored record reads");
        let excerpt = excerpt_html(&record, Some(&readable), "qwen3.5");
        assert!(excerpt.contains("<strong>qwen3.5</strong>"), "{excerpt}");
        // An ordinary hit still reads as sentences, not as a stored record.
        let ordinary = excerpt_html(&record, Some(&readable), "thursday");
        assert_eq!(
            ordinary,
            "<strong>thursday</strong> afternoon. it finished."
        );
        // No match anywhere leaves the sentences, never the braces.
        let neither = excerpt_html(&record, Some(&readable), "hopper");
        assert_eq!(neither, "thursday afternoon. it finished.");
    }

    // Fresh-eyes 2 #13: the old client read `ts` with `Number(record.ts)`, so
    // an imported record that stored it as a string or a float still showed a
    // time. Reading only `as_i64` dropped it off exactly those rows.
    #[test]
    fn a_string_or_float_timestamp_is_still_read() {
        let string_ts = json!({"headline": "release burn", "ts": "1788662697014"}).to_string();
        assert_eq!(
            readable_record(&string_ts).expect("reads").ts,
            Some(1_788_662_697_014)
        );
        let float_ts =
            json!({"headline": "release burn", "ts": 1_788_662_697_014.0_f64}).to_string();
        assert_eq!(
            readable_record(&float_ts).expect("reads").ts,
            Some(1_788_662_697_014)
        );
        let seconds_string = json!({"headline": "release burn", "ts": " 1788662697 "}).to_string();
        assert_eq!(
            readable_record(&seconds_string).expect("reads").ts,
            Some(1_788_662_697_000)
        );
        let nonsense = json!({"headline": "release burn", "ts": "not a time"}).to_string();
        assert_eq!(readable_record(&nonsense).expect("reads").ts, None);
    }

    // F-32: an imported record can carry seconds. No millisecond journal
    // timestamp is below 1e11, so a value under it is read as seconds.
    #[test]
    fn a_seconds_timestamp_is_read_as_seconds() {
        assert_eq!(record_millis(1_788_662_697), Some(1_788_662_697_000));
        assert_eq!(record_millis(1_788_662_697_014), Some(1_788_662_697_014));
        assert_eq!(record_millis(0), None);
        assert_eq!(record_millis(-1), None);
    }

    #[test]
    fn text_that_is_not_a_record_is_left_alone() {
        assert!(readable_record("we discussed the release burn timeline").is_none());
        assert!(readable_record("{not json}").is_none());
        assert!(readable_record(r#"{"model": "local/qwen3.5-4b"}"#).is_none());
    }

    #[test]
    fn highlight_keeps_bolding_independent_unquoted_words() {
        let text = "release notes mention a burn later on";
        let result = highlight(text, "release burn");
        assert!(result.contains("<strong>release</strong>"));
        assert!(result.contains("<strong>burn</strong>"));
    }
}
