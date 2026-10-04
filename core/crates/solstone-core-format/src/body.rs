// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{BODY_BUNDLE_DIR_PREFIX, health_card_streams};

use crate::segment::is_date_key;

/// Check if a stream directory name matches any registered card stream.
pub fn is_body_stream(name: &str) -> bool {
    health_card_streams().any(|stream| stream.eq_ignore_ascii_case(name))
}

/// Check if a journal-relative path matches a body source card or bundle path.
pub fn is_body_source_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let mut parts = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .peekable();

    if parts.peek().copied() == Some("chronicle") {
        parts.next();
    }

    let components: Vec<&str> = parts.collect();
    if components.len() < 3 {
        return false;
    }

    let first = components[0];
    let second = components[1];

    if is_date_key(first) && is_body_stream(second) {
        return true;
    }

    if first == "imports"
        && second
            .to_ascii_lowercase()
            .starts_with(BODY_BUNDLE_DIR_PREFIX)
    {
        return true;
    }

    false
}

fn escape_like_pattern(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len() + 4);
    for ch in raw.chars() {
        if ch == '_' || ch == '%' || ch == '\\' {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Generate SQL boolean predicate filtering out body source paths using all registered card streams.
pub fn body_source_predicate() -> String {
    body_source_predicate_in(health_card_streams())
}

/// Generate SQL boolean predicate filtering out body source paths using the provided stream names.
pub fn body_source_predicate_in<'a>(streams: impl IntoIterator<Item = &'a str>) -> String {
    let norm = "CASE WHEN substr(replace(path, char(92), '/'), 1, 10) = 'chronicle/' THEN substr(replace(path, char(92), '/'), 11) ELSE replace(path, char(92), '/') END";
    let day_check = format!(
        "substr({norm}, 1, 8) GLOB '[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]' AND substr({norm}, 9, 1) = '/'"
    );

    let stream_clauses: Vec<String> = streams
        .into_iter()
        .map(|stream| {
            let escaped = escape_like_pattern(stream);
            format!("substr({norm}, 10) LIKE '{escaped}/_%' ESCAPE '\\'")
        })
        .collect();

    let stream_or = if stream_clauses.is_empty() {
        "0".to_string()
    } else {
        stream_clauses.join(" OR ")
    };

    let escaped_prefix = escape_like_pattern(BODY_BUNDLE_DIR_PREFIX);
    let bundle_clause = format!("{norm} LIKE 'imports/{escaped_prefix}%/_%' ESCAPE '\\'");

    format!("(({day_check} AND ({stream_or})) OR ({bundle_clause}))")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{ContentResolution, Family, classify, resolve_spec};

    #[test]
    fn path_table_classification() {
        let positive_paths = [
            "20260101/import.apple_health/000000_86400/day_summary_transcript.md",
            "chronicle/20260101/import.apple_health/000000_86400/day_summary_transcript.md",
            "20260101\\import.apple_health\\000000_86400\\day_summary_transcript.md",
            "20260101/import.oura/090000_300/talents/brief.md",
            "20260101/import.oura/imported.jsonl",
            "imports/body-01JZ8Y3Q4M5N6P7R8S9T0V1W2X/summary.md",
            "20260102/IMPORT.APPLE_HEALTH/090000_300/x.md",
        ];

        for path in positive_paths {
            assert!(
                is_body_source_path(path),
                "expected true for positive path: {path}"
            );
            assert_eq!(
                classify(path),
                ContentResolution::Unrecognized,
                "classify must be Unrecognized for positive path: {path}"
            );
            assert!(
                resolve_spec(path).is_none(),
                "resolve_spec must be None for positive path: {path}"
            );
        }

        let negative_paths = [
            "20260101/import.ics/110000_60/event_transcript.md",
            "20260101/090000_300/day_summary_transcript.md",
            "facets/import.apple_health/news/x.md",
            "imports/20260101_120000/summary.md",
            "20260101/import.appleXhealth/090000_300/imported.md",
            "20260101/import.apple_health",
            "entities/import.oura/090000_300/x.md",
        ];

        for path in negative_paths {
            assert!(
                !is_body_source_path(path),
                "expected false for negative path: {path}"
            );
        }

        // The ics transcript and imports/20260101_120000/summary.md are Indexed(Markdown) and Some(_)
        assert_eq!(
            classify("20260101/import.ics/110000_60/event_transcript.md"),
            ContentResolution::Indexed(Family::Markdown)
        );
        assert!(resolve_spec("20260101/import.ics/110000_60/event_transcript.md").is_some());

        assert_eq!(
            classify("imports/20260101_120000/summary.md"),
            ContentResolution::Indexed(Family::Markdown)
        );
        assert!(resolve_spec("imports/20260101_120000/summary.md").is_some());
    }

    #[test]
    fn every_registered_stream_is_refused() {
        for stream in health_card_streams() {
            assert!(
                is_body_stream(stream),
                "stream {stream} must pass is_body_stream"
            );
            assert!(
                is_body_stream(&stream.to_ascii_uppercase()),
                "uppercase stream {stream} must pass is_body_stream"
            );
            let sample_path = format!("20260731/{stream}/120000_60/imported.md");
            assert!(
                is_body_source_path(&sample_path),
                "sample path {sample_path} must be recognized as body source path"
            );
        }
    }
}
