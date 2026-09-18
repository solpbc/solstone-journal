// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use chrono::{Datelike, Local, NaiveDate};
    use rusqlite::params;
    use serde_json::{Map, Value, json};
    use solstone_core_convey_http::envelope::error_envelope;
    use solstone_core_indexer_query::{
        IndexAccessError, OwnerBoundary, SearchRequest, open_owner_index, search, search_counts,
    };
    use solstone_core_indexer_store::db::open_index;

    use crate::search::{SearchQuery, search_response, search_response_with_index};

    static ORACLE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    const DROPPED_SEARCH_FIELDS: &[&str] = &[
        "agent_icon_svg",
        "icon_svg",
        "agent_icon",
        "facet_color",
        "facet_emoji",
        "day_grid",
        "showing_days",
        "has_more",
    ];

    fn temp_journal(name: &str) -> PathBuf {
        let id = ORACLE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from("/var/tmp").join(format!("solstone-oracle-{name}-{id}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("health")).expect("create health dir");
        fs::create_dir_all(path.join("facets/work")).expect("create work facet");
        fs::create_dir_all(path.join("facets/personal")).expect("create personal facet");
        fs::create_dir_all(path.join("facets/finance")).expect("create finance facet");
        fs::write(
            path.join("facets/work/facet.json"),
            r##"{"title": "Work Project", "color": "#00f", "emoji": "💼", "muted": false}"##,
        )
        .expect("write work facet");
        fs::write(
            path.join("facets/personal/facet.json"),
            r##"{"title": "Personal Life", "color": "#0f0", "emoji": "🏠", "muted": false}"##,
        )
        .expect("write personal facet");
        fs::write(
            path.join("facets/finance/facet.json"),
            r##"{"title": "Finance & Taxes", "color": "#f00", "emoji": "💰", "muted": false}"##,
        )
        .expect("write finance facet");
        path
    }

    /// Exact copy of today's baseline search_response assembly BEFORE any changes.
    /// Uses the public search and search_counts primitives across separate readers.
    fn reference_search_response(journal_root: &Path, query: SearchQuery) -> Response {
        let (day_from, day_to) = match day_range(query.day_from.as_deref(), query.day_to.as_deref())
        {
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
        let base = match search_counts(journal_root, OwnerBoundary, &base_request, reference) {
            Ok(counts) => counts,
            Err(error) => return search_failed(&error),
        };
        let filtered = match search_counts(journal_root, OwnerBoundary, &request, reference) {
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
        let facets = facets(journal_root);
        for (day, total) in &page {
            let mut per_day = request.clone();
            per_day.day = Some(day.clone());
            per_day.limit = request.limit;
            let response = match search(journal_root, OwnerBoundary, &per_day, reference) {
                Ok(response) => response,
                Err(error) => return search_failed(&error),
            };
            let results = response
                .results
                .into_iter()
                .map(|hit| {
                    let readable = readable_record(&hit.text);
                    let (excerpt, excerpt_is_record) =
                        excerpt_html(&hit.text, readable.as_ref(), &request.query);
                    json!({
                        "id": hit.id,
                        "entry_id": hit.row_id,
                        "day": hit.metadata.day,
                        "agent": hit.metadata.agent,
                        "agent_label": agent_label(&hit.metadata.agent),
                        "facet": hit.metadata.facet,
                        "facet_title": facets.get(&hit.metadata.facet).map_or(&hit.metadata.facet, |facet| &facet.title),
                        "text": excerpt,
                        "ts": readable.as_ref().and_then(|record| record.ts),
                        "record": readable
                            .as_ref()
                            .filter(|_| !excerpt_is_record)
                            .map(|_| cap_words(&hit.text)),
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
        NaiveDate::parse_from_str(value, "%Y%m%d")
            .map_err(|_| format!("{name} must be a real day"))?;
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

    struct Facet {
        title: String,
        color: String,
        emoji: String,
        muted: bool,
    }

    fn facets(journal_root: &Path) -> BTreeMap<String, Facet> {
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

    fn facet_counts(
        facets: &BTreeMap<String, Facet>,
        counts: &BTreeMap<String, u64>,
    ) -> Vec<Value> {
        let mut values = facets
            .iter()
            .filter(|(_, facet)| !facet.muted)
            .map(|(name, facet)| {
                json!({
                    "name": name,
                    "title": facet.title,
                    "color": facet.color,
                    "emoji": facet.emoji,
                    "count": counts.get(name).copied().unwrap_or(0)
                })
            })
            .collect::<Vec<_>>();
        values.sort_by_key(|value| std::cmp::Reverse(value["count"].as_u64().unwrap_or(0)));
        values
    }

    fn talent_counts(counts: &BTreeMap<String, u64>) -> Vec<Value> {
        counts
            .iter()
            .map(|(name, count)| {
                json!({
                    "name": name,
                    "label": agent_label(name),
                    "icon": agent_icon(name),
                    "count": count
                })
            })
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

    const RECORD_TEXT_FIELDS: [&str; 9] = [
        "headline",
        "summary_sentence",
        "full_details",
        "summary",
        "text",
        "body",
        "sentence",
        "message",
        "note",
    ];

    struct ReadableRecord {
        text: String,
        ts: Option<i64>,
    }

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
            if let Some(index) = parts
                .iter()
                .position(|seen| seen.contains(part) || part.contains(seen.as_str()))
            {
                if part.len() > parts[index].len() {
                    parts[index] = part.to_owned();
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

    fn excerpt_html(raw: &str, readable: Option<&ReadableRecord>, query: &str) -> (String, bool) {
        let Some(record) = readable else {
            return (highlight(raw, query), true);
        };
        let readable_excerpt = highlight(&record.text, query);
        if readable_excerpt.contains("<strong>") {
            return (readable_excerpt, false);
        }
        let raw_excerpt = highlight(raw, query);
        if raw_excerpt.contains("<strong>") {
            (raw_excerpt, true)
        } else {
            (readable_excerpt, false)
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
            if !word.is_empty()
                && !matches!(word.to_ascii_uppercase().as_str(), "AND" | "OR" | "NOT")
            {
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

    fn invalid_day(detail: &str) -> Response {
        error_envelope(
            "invalid_day",
            "that day couldn't be used.",
            detail,
            StatusCode::BAD_REQUEST,
        )
        .into_response()
    }

    fn search_failed(error: &IndexAccessError) -> Response {
        error_envelope(
            error.reason(),
            "couldn't search your journal right now.",
            error.to_string(),
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .into_response()
    }

    /// Canonical JSON serializer matching convey-body's trends pattern.
    pub(crate) fn canonical_json(value: &Value) -> Vec<u8> {
        fn encode(value: &Value, output: &mut String) {
            match value {
                Value::Null => output.push_str("null"),
                Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
                Value::Number(value) => {
                    if let Some(float) = value.as_f64() {
                        output.push_str(&format!("{float:.6}"));
                    } else {
                        output.push_str(&value.to_string());
                    }
                }
                Value::String(value) => string(value, output),
                Value::Array(values) => {
                    output.push('[');
                    for (index, value) in values.iter().enumerate() {
                        if index > 0 {
                            output.push(',');
                        }
                        encode(value, output);
                    }
                    output.push(']');
                }
                Value::Object(values) => {
                    let mut entries = values.iter().collect::<Vec<_>>();
                    entries.sort_by_key(|(key, _)| *key);
                    output.push('{');
                    for (index, (key, value)) in entries.into_iter().enumerate() {
                        if index > 0 {
                            output.push(',');
                        }
                        string(key, output);
                        output.push(':');
                        encode(value, output);
                    }
                    output.push('}');
                }
            }
        }
        fn string(value: &str, output: &mut String) {
            output.push('"');
            for character in value.chars() {
                match character {
                    '"' => output.push_str("\\\""),
                    '\\' => output.push_str("\\\\"),
                    '\n' => output.push_str("\\n"),
                    '\r' => output.push_str("\\r"),
                    '\t' => output.push_str("\\t"),
                    c if c.is_control() => output.push_str(&format!("\\u{:04x}", c as u32)),
                    c => output.push(c),
                }
            }
            output.push('"');
        }
        let mut output = String::new();
        encode(value, &mut output);
        output.into_bytes()
    }

    async fn response_json(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        serde_json::from_slice(&bytes).expect("json body")
    }

    fn assert_no_keys(value: &Value, dropped: &[&str]) {
        match value {
            Value::Object(map) => {
                for key in map.keys() {
                    assert!(
                        !dropped.contains(&key.as_str()),
                        "unexpected dropped field in response: {key}"
                    );
                }
                for nested in map.values() {
                    assert_no_keys(nested, dropped);
                }
            }
            Value::Array(items) => {
                for item in items {
                    assert_no_keys(item, dropped);
                }
            }
            _ => {}
        }
    }

    struct OracleFixture {
        root: PathBuf,
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_chunk_opt_day(
        connection: &rusqlite::Connection,
        content: &str,
        path: &str,
        day: Option<&str>,
        facet: &str,
        agent: &str,
        stream: &str,
        time_bucket: &str,
        idx: i64,
    ) {
        connection
            .execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, time_bucket, idx) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![content, path, day, facet, agent, stream, time_bucket, idx],
            )
            .expect("insert test chunk");
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_chunk(
        connection: &rusqlite::Connection,
        content: &str,
        path: &str,
        day: &str,
        facet: &str,
        agent: &str,
        stream: &str,
        time_bucket: &str,
        idx: i64,
    ) {
        insert_chunk_opt_day(
            connection,
            content,
            path,
            Some(day),
            facet,
            agent,
            stream,
            time_bucket,
            idx,
        );
    }

    fn build_oracle_test_journal() -> OracleFixture {
        let root = temp_journal("rich");
        let connection = open_index(&root).expect("open test index");
        connection
            .execute(
                "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'complete', 0, 0)",
                [],
            )
            .expect("seed complete state");

        // 1. Multi-day span across 25 distinct days (20260101 to 20260125)
        for d in 1..=25 {
            let day = format!("202601{:02}", d);
            let facet = match d % 3 {
                0 => "work",
                1 => "personal",
                _ => "finance",
            };
            let agent = match d % 4 {
                0 => "flow",
                1 => "meetings",
                2 => "audio",
                _ => "news",
            };
            let stream = match d % 2 {
                0 => "chat",
                _ => "notes",
            };
            let time_bucket = match d % 2 {
                0 => "morning",
                _ => "evening",
            };
            let json_text = json!({
                "headline": format!("Daily standup update for day {day}"),
                "summary": format!("Summary of work items on {day} in project Alpha"),
                "ts": 1735689600000_i64 + (d as i64 * 86400000),
            })
            .to_string();
            insert_chunk(
                &connection,
                &json_text,
                &format!("chronicle/{day}/entry.json"),
                &day,
                facet,
                agent,
                stream,
                time_bucket,
                0,
            );
        }

        // 2. Partition pressure on day 20260115: 15 matching items for "partitionpressure"
        for i in 1..=15 {
            let json_text = json!({
                "headline": format!("High priority task {i} partitionpressure"),
                "summary": format!("Important task item {i} regarding partitionpressure"),
                "ts": 1736938800000_i64 + (i * 1000),
            })
            .to_string();
            insert_chunk(
                &connection,
                &json_text,
                &format!("chronicle/20260115/task_{i}.json"),
                "20260115",
                "work",
                "flow",
                "chat",
                "morning",
                i,
            );
        }
        // Also add partitionpressure hits on 20260114 and 20260116 to test day boundaries and ties
        insert_chunk(
            &connection,
            &json!({"headline": "Day 14 item partitionpressure", "ts": 1736852400000_i64})
                .to_string(),
            "chronicle/20260114/item.json",
            "20260114",
            "work",
            "flow",
            "chat",
            "morning",
            0,
        );
        insert_chunk(
            &connection,
            &json!({"headline": "Day 16 item partitionpressure", "ts": 1737025200000_i64})
                .to_string(),
            "chronicle/20260116/item.json",
            "20260116",
            "work",
            "flow",
            "chat",
            "morning",
            0,
        );
        // Equal-score ties across day boundaries: identical content on days 20260117 and 20260118
        insert_chunk(
            &connection,
            &json!({"headline": "Identical tie item partitionpressure", "ts": 1737111600000_i64})
                .to_string(),
            "chronicle/20260117/tie.json",
            "20260117",
            "work",
            "flow",
            "chat",
            "morning",
            0,
        );
        insert_chunk(
            &connection,
            &json!({"headline": "Identical tie item partitionpressure", "ts": 1737198000000_i64})
                .to_string(),
            "chronicle/20260118/tie.json",
            "20260118",
            "work",
            "flow",
            "chat",
            "morning",
            0,
        );

        // 3. Combined-filter adversary: decoys matching partial subsets of (work, flow, chat, morning, 20260110..=20260120)
        insert_chunk(
            &connection,
            "adversary intersection decoy A targetterm",
            "chronicle/20260105/decoy_a.txt",
            "20260105",
            "work",
            "flow",
            "chat",
            "morning",
            0,
        );
        insert_chunk(
            &connection,
            "adversary intersection decoy B targetterm",
            "chronicle/20260112/decoy_b.txt",
            "20260112",
            "work",
            "flow",
            "chat",
            "evening",
            0,
        );
        insert_chunk(
            &connection,
            "adversary intersection decoy C targetterm",
            "chronicle/20260112/decoy_c.txt",
            "20260112",
            "work",
            "flow",
            "notes",
            "morning",
            0,
        );
        insert_chunk(
            &connection,
            &json!({"headline": "Target full intersection winner targetterm", "ts": 1736679600000_i64}).to_string(),
            "chronicle/20260112/winner.json",
            "20260112",
            "work",
            "flow",
            "chat",
            "morning",
            1,
        );

        // 4. NULL and empty day rows: 2 with day="", 1 with SQL NULL day
        insert_chunk(
            &connection,
            "Undated loose note with needleterm",
            "entities/undated_note.txt",
            "",
            "personal",
            "screen",
            "notes",
            "",
            0,
        );
        insert_chunk(
            &connection,
            "Another undated note with needleterm",
            "entities/undated_note_2.txt",
            "",
            "personal",
            "screen",
            "notes",
            "",
            1,
        );
        insert_chunk_opt_day(
            &connection,
            "Third undated note with needleterm and null day",
            "entities/undated_note_3.txt",
            None,
            "personal",
            "screen",
            "notes",
            "",
            2,
        );

        // 5. Relaxation ladder fixtures
        // Existing quantum relaxation query
        insert_chunk(
            &connection,
            &json!({"headline": "Research notes on quantum physics", "ts": 1737370800000_i64})
                .to_string(),
            "chronicle/20260120/quantum.json",
            "20260120",
            "work",
            "meetings",
            "notes",
            "evening",
            0,
        );
        // 5a. Exact-on-some-days vs looser-on-others:
        // Day 20260105 matches both planexactalpha and planexactbeta; Day 20260106 matches only planexactalpha
        insert_chunk(
            &connection,
            &json!({"headline": "Both terms planexactalpha planexactbeta", "ts": 1736074800000_i64}).to_string(),
            "chronicle/20260105/both.json",
            "20260105",
            "work",
            "flow",
            "chat",
            "morning",
            10,
        );
        insert_chunk(
            &connection,
            &json!({"headline": "Single term planexactalpha only", "ts": 1736161200000_i64})
                .to_string(),
            "chronicle/20260106/single.json",
            "20260106",
            "work",
            "flow",
            "chat",
            "morning",
            10,
        );
        // 5b. Bounded page must not re-relax inside the HTTP day range:
        // Day 20260101 (outside) matches relaxalpha and relaxbeta; Day 20260115 (inside 20260110..=20260120) matches only relaxalpha
        insert_chunk(
            &connection,
            &json!({"headline": "Outside range matches both relaxalpha relaxbeta", "ts": 1735689600000_i64}).to_string(),
            "chronicle/20260101/both_relax.json",
            "20260101",
            "work",
            "flow",
            "chat",
            "morning",
            10,
        );
        insert_chunk(
            &connection,
            &json!({"headline": "Inside range matches only relaxalpha", "ts": 1736938800000_i64})
                .to_string(),
            "chronicle/20260115/single_relax.json",
            "20260115",
            "work",
            "flow",
            "chat",
            "morning",
            20,
        );

        OracleFixture { root }
    }

    #[tokio::test]
    async fn search_page_matches_reference_oracle_unfiltered() {
        let fixture = build_oracle_test_journal();
        let query = SearchQuery {
            q: Some("Alpha".into()),
            limit: Some(5),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query.clone());
        let oracle_json = response_json(oracle_res).await;
        let prod_res = search_response(fixture.root.clone(), query);
        let prod_json = response_json(prod_res).await;

        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "unfiltered search production must match reference oracle"
        );
        assert_no_keys(&prod_json, DROPPED_SEARCH_FIELDS);
    }

    #[tokio::test]
    async fn search_page_matches_reference_oracle_filters_and_bounds() {
        let fixture = build_oracle_test_journal();

        // 1. Facet + day range
        let query = SearchQuery {
            q: Some("Alpha".into()),
            limit: Some(3),
            facet: Some("work".into()),
            day_from: Some("20260105".into()),
            day_to: Some("20260120".into()),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query.clone());
        let oracle_json = response_json(oracle_res).await;
        let prod_res = search_response(fixture.root.clone(), query);
        let prod_json = response_json(prod_res).await;

        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "filtered and bounded search production must match reference oracle"
        );
        assert_no_keys(&prod_json, DROPPED_SEARCH_FIELDS);

        // 2. Agent-only
        let query_agent = SearchQuery {
            q: Some("Alpha".into()),
            agent: Some("flow".into()),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query_agent.clone());
        let oracle_json = response_json(oracle_res).await;
        let prod_res = search_response(fixture.root.clone(), query_agent);
        let prod_json = response_json(prod_res).await;
        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "agent-only search production must match reference oracle"
        );
        assert_no_keys(&prod_json, DROPPED_SEARCH_FIELDS);

        // 3. Stream-only
        let query_stream = SearchQuery {
            q: Some("Alpha".into()),
            stream: Some("chat".into()),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query_stream.clone());
        let oracle_json = response_json(oracle_res).await;
        let prod_res = search_response(fixture.root.clone(), query_stream);
        let prod_json = response_json(prod_res).await;
        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "stream-only search production must match reference oracle"
        );
        assert_no_keys(&prod_json, DROPPED_SEARCH_FIELDS);

        // 4. Time-bucket-only
        let query_tb = SearchQuery {
            q: Some("Alpha".into()),
            time_bucket: Some("morning".into()),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query_tb.clone());
        let oracle_json = response_json(oracle_res).await;
        let prod_res = search_response(fixture.root.clone(), query_tb);
        let prod_json = response_json(prod_res).await;
        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "time-bucket-only search production must match reference oracle"
        );
        assert_no_keys(&prod_json, DROPPED_SEARCH_FIELDS);
    }

    #[tokio::test]
    async fn search_page_matches_reference_oracle_combined_filter_adversary() {
        let fixture = build_oracle_test_journal();
        let query = SearchQuery {
            q: Some("targetterm".into()),
            facet: Some("work".into()),
            agent: Some("flow".into()),
            stream: Some("chat".into()),
            time_bucket: Some("morning".into()),
            day_from: Some("20260110".into()),
            day_to: Some("20260120".into()),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query.clone());
        let oracle_json = response_json(oracle_res).await;
        let prod_res = search_response(fixture.root.clone(), query);
        let prod_json = response_json(prod_res).await;

        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "combined filter adversary must match oracle"
        );
        assert_eq!(prod_json["total"], 1);
        assert_eq!(prod_json["days"].as_array().unwrap().len(), 1);
        assert_eq!(prod_json["days"][0]["day"], "20260112");
        assert_no_keys(&prod_json, DROPPED_SEARCH_FIELDS);
    }

    #[tokio::test]
    async fn search_page_matches_reference_oracle_empty_and_offset() {
        let fixture = build_oracle_test_journal();

        // 1. Unmatched query: total 0, days empty, HTTP 200, matches oracle
        let query_unmatched = SearchQuery {
            q: Some("unmatchednonexistentqueryxyz".into()),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query_unmatched.clone());
        let oracle_json = response_json(oracle_res).await;
        let prod_res = search_response(fixture.root.clone(), query_unmatched);
        assert_eq!(prod_res.status(), StatusCode::OK);
        let prod_json = response_json(prod_res).await;
        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "unmatched query must match reference oracle"
        );
        assert_eq!(prod_json["total"], 0);
        assert_eq!(prod_json["total_days"], 0);
        assert_eq!(prod_json["days"].as_array().unwrap().len(), 0);

        // 2. q=Alpha, offset=20 (remaining 5 days of 25 days)
        let query_offset20 = SearchQuery {
            q: Some("Alpha".into()),
            offset: Some(20),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query_offset20.clone());
        let oracle_json = response_json(oracle_res).await;
        let prod_res = search_response(fixture.root.clone(), query_offset20);
        let prod_json = response_json(prod_res).await;
        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "offset 20 page must match reference oracle"
        );
        assert_eq!(prod_json["total"], 25);
        assert_eq!(prod_json["total_days"], 25);
        assert_eq!(prod_json["days"].as_array().unwrap().len(), 5);

        // 3. q=Alpha, offset=100 (past last day): days empty, total 25, total_days 25, fetch_hits_calls == 0
        let query_offset100 = SearchQuery {
            q: Some("Alpha".into()),
            offset: Some(100),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query_offset100.clone());
        let oracle_json = response_json(oracle_res).await;

        let mut index = open_owner_index(&fixture.root, OwnerBoundary).expect("open index");
        let prod_res = search_response_with_index(&fixture.root, &mut index, query_offset100);
        let prod_json = response_json(prod_res).await;
        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "offset 100 page must match reference oracle"
        );
        assert_eq!(prod_json["total"], 25);
        assert_eq!(prod_json["total_days"], 25);
        assert_eq!(prod_json["days"].as_array().unwrap().len(), 0);
        assert_eq!(
            index.query_counters().fetch_hits_calls,
            0,
            "when page days are empty past the end, fetch_day_hits must not be called"
        );
    }

    #[tokio::test]
    async fn search_page_matches_reference_oracle_null_and_empty_days() {
        let fixture = build_oracle_test_journal();
        let query = SearchQuery {
            q: Some("needleterm".into()),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query.clone());
        let oracle_json = response_json(oracle_res).await;
        let prod_res = search_response(fixture.root.clone(), query);
        let prod_json = response_json(prod_res).await;

        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "null and empty days search must match oracle"
        );
        // Unbounded total includes the 3 undated rows (2 empty string, 1 null), but days array has 0 cards
        assert_eq!(prod_json["total"], 3);
        assert_eq!(prod_json["total_days"], 0);
        assert_eq!(prod_json["days"].as_array().unwrap().len(), 0);
        assert_no_keys(&prod_json, DROPPED_SEARCH_FIELDS);
    }

    #[tokio::test]
    async fn search_page_matches_reference_oracle_partition_pressure() {
        let fixture = build_oracle_test_journal();
        let query = SearchQuery {
            q: Some("partitionpressure".into()),
            limit: Some(5),
            ..SearchQuery::default()
        };
        let oracle_res = reference_search_response(&fixture.root, query.clone());
        let oracle_json = response_json(oracle_res).await;
        let prod_res = search_response(fixture.root.clone(), query);
        let prod_json = response_json(prod_res).await;

        assert_eq!(
            canonical_json(&prod_json),
            canonical_json(&oracle_json),
            "partition pressure search must match oracle"
        );
        let days = prod_json["days"].as_array().unwrap();
        // Check days appear in newest-first order: 20260118, 20260117, 20260116, 20260115, 20260114
        let day_names: Vec<&str> = days.iter().map(|d| d["day"].as_str().unwrap()).collect();
        assert!(day_names.contains(&"20260118"));
        assert!(day_names.contains(&"20260117"));
        assert!(day_names.contains(&"20260116"));
        assert!(day_names.contains(&"20260115"));
        assert!(day_names.contains(&"20260114"));
        for i in 0..day_names.len() - 1 {
            assert!(
                day_names[i] > day_names[i + 1],
                "days must be strictly descending"
            );
        }

        // Day 20260115 had 15 items, showing should be capped at 5
        let day_15 = days
            .iter()
            .find(|d| d["day"] == "20260115")
            .expect("day 15 present");
        assert_eq!(day_15["total"], 15);
        assert_eq!(day_15["showing"], 5);
        assert_eq!(day_15["results"].as_array().unwrap().len(), 5);

        assert_no_keys(&prod_json, DROPPED_SEARCH_FIELDS);
    }

    #[tokio::test]
    async fn search_page_matches_reference_oracle_relaxation() {
        let fixture = build_oracle_test_journal();

        // 1. Existing quantum relaxation
        let query1 = SearchQuery {
            q: Some("what is the quantum computing breakthrough".into()),
            ..SearchQuery::default()
        };
        let oracle_res1 = reference_search_response(&fixture.root, query1.clone());
        let oracle_json1 = response_json(oracle_res1).await;
        let prod_res1 = search_response(fixture.root.clone(), query1);
        let prod_json1 = response_json(prod_res1).await;

        assert_eq!(
            canonical_json(&prod_json1),
            canonical_json(&oracle_json1),
            "relaxation search must match oracle"
        );
        assert_eq!(prod_json1["relaxed"], true);
        assert_no_keys(&prod_json1, DROPPED_SEARCH_FIELDS);

        // 4a. Exact-on-some-days vs looser-on-others:
        // Day 20260105 matches both planexactalpha and planexactbeta; Day 20260106 matches only planexactalpha
        // Query "planexactalpha planexactbeta" -> relaxed == false, only 20260105 is a card, 20260106 absent
        let query2 = SearchQuery {
            q: Some("planexactalpha planexactbeta".into()),
            ..SearchQuery::default()
        };
        let oracle_res2 = reference_search_response(&fixture.root, query2.clone());
        let oracle_json2 = response_json(oracle_res2).await;
        let prod_res2 = search_response(fixture.root.clone(), query2);
        let prod_json2 = response_json(prod_res2).await;

        assert_eq!(
            canonical_json(&prod_json2),
            canonical_json(&oracle_json2),
            "exact vs looser query must match oracle"
        );
        assert_eq!(prod_json2["relaxed"], false);
        assert_eq!(prod_json2["total"], 1);
        assert_eq!(prod_json2["total_days"], 1);
        let days2 = prod_json2["days"].as_array().unwrap();
        assert_eq!(days2.len(), 1);
        assert_eq!(days2[0]["day"], "20260105");

        // 4b. Bounded page must not re-relax inside the HTTP day range:
        // Query "what relaxalpha relaxbeta" -> first globally successful rung is stopword-strip AND (matches 20260101)
        // Range 20260110..=20260120 has 20260115 (which only matches relaxalpha)
        // Bounded page must have relaxed: true, total: 0, total_days: 0, days: []
        let query3 = SearchQuery {
            q: Some("what relaxalpha relaxbeta".into()),
            day_from: Some("20260110".into()),
            day_to: Some("20260120".into()),
            ..SearchQuery::default()
        };
        let oracle_res3 = reference_search_response(&fixture.root, query3.clone());
        let oracle_json3 = response_json(oracle_res3).await;
        let prod_res3 = search_response(fixture.root.clone(), query3);
        let prod_json3 = response_json(prod_res3).await;

        assert_eq!(
            canonical_json(&prod_json3),
            canonical_json(&oracle_json3),
            "bounded page must not re-relax inside HTTP range and must match oracle"
        );
        assert_eq!(prod_json3["relaxed"], true);
        assert_eq!(prod_json3["total"], 0);
        assert_eq!(prod_json3["total_days"], 0);
        assert_eq!(prod_json3["days"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn search_page_identical_counts_aggregate_once() {
        let fixture = build_oracle_test_journal();
        let mut index = open_owner_index(&fixture.root, OwnerBoundary).expect("open index");
        let query = SearchQuery {
            q: Some("Alpha".into()),
            limit: Some(5),
            ..SearchQuery::default()
        };
        let response = search_response_with_index(&fixture.root, &mut index, query);
        let _ = response_json(response).await;
        let counters = index.query_counters();
        assert_eq!(
            counters.aggregate_calls, 1,
            "identical counts must execute aggregate_counts exactly once"
        );
        assert_eq!(
            counters.fetch_hits_calls, 1,
            "result cards must execute fetch_hits_calls exactly once"
        );
    }

    #[tokio::test]
    async fn search_page_facet_or_agent_aggregates_twice() {
        let fixture = build_oracle_test_journal();
        let mut index = open_owner_index(&fixture.root, OwnerBoundary).expect("open index");
        let query = SearchQuery {
            q: Some("Alpha".into()),
            facet: Some("work".into()),
            limit: Some(5),
            ..SearchQuery::default()
        };
        let response = search_response_with_index(&fixture.root, &mut index, query);
        let _ = response_json(response).await;
        let counters = index.query_counters();
        assert_eq!(
            counters.aggregate_calls, 2,
            "divergent facet/agent query must execute aggregate_counts twice"
        );
        assert_eq!(
            counters.fetch_hits_calls, 1,
            "result cards must execute fetch_hits_calls exactly once"
        );
    }

    #[tokio::test]
    async fn search_page_one_result_card_plan_one_day_and_twenty_days() {
        let fixture = build_oracle_test_journal();
        // Case 1: 1 day matched
        let mut index1 = open_owner_index(&fixture.root, OwnerBoundary).expect("open index");
        let query1 = SearchQuery {
            q: Some("Alpha".into()),
            day_from: Some("20260101".into()),
            day_to: Some("20260101".into()),
            ..SearchQuery::default()
        };
        let _ = response_json(search_response_with_index(
            &fixture.root,
            &mut index1,
            query1,
        ))
        .await;
        assert_eq!(index1.query_counters().fetch_hits_calls, 1);

        // Case 2: 20+ days matched
        let mut index2 = open_owner_index(&fixture.root, OwnerBoundary).expect("open index");
        let query2 = SearchQuery {
            q: Some("Alpha".into()),
            ..SearchQuery::default()
        };
        let _ = response_json(search_response_with_index(
            &fixture.root,
            &mut index2,
            query2,
        ))
        .await;
        assert_eq!(index2.query_counters().fetch_hits_calls, 1);
    }

    #[tokio::test]
    async fn search_page_aggregate_failure_is_search_failed_envelope() {
        let fixture = build_oracle_test_journal();
        let mut index = open_owner_index(&fixture.root, OwnerBoundary).expect("open index");
        index.inject_aggregate_failure();
        let query = SearchQuery {
            q: Some("Alpha".into()),
            ..SearchQuery::default()
        };
        let response = search_response_with_index(&fixture.root, &mut index, query);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let json = response_json(response).await;
        assert_eq!(json["reason_code"], "index_unreadable");
    }

    #[tokio::test]
    async fn search_page_fetch_failure_is_search_failed_envelope() {
        let fixture = build_oracle_test_journal();
        let mut index = open_owner_index(&fixture.root, OwnerBoundary).expect("open index");
        index.inject_fetch_failure();
        let query = SearchQuery {
            q: Some("Alpha".into()),
            ..SearchQuery::default()
        };
        let response = search_response_with_index(&fixture.root, &mut index, query);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let json = response_json(response).await;
        assert_eq!(json["reason_code"], "index_unreadable");
    }
}
