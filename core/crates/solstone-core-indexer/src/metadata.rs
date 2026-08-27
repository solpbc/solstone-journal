// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_format::segment::is_date_key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMetadata {
    pub day: String,
    pub facet: String,
    pub agent: String,
}

pub fn extract_path_metadata(rel_path: &str) -> PathMetadata {
    let normalized = rel_path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    let filename = parts.last().copied().unwrap_or("");
    let basename = basename(filename);
    let is_markdown = filename.ends_with(".md");

    let mut day = String::new();
    let mut facet = String::new();
    let mut agent = String::new();

    if parts.first().is_some_and(|part| is_date_key(part)) {
        day = parts[0].to_string();
    }

    if let Some(talents_idx) = parts.iter().position(|part| *part == "talents")
        && talents_idx + 2 < parts.len()
    {
        facet = parts[talents_idx + 1].to_string();
    }

    if parts.first() == Some(&"facets") && parts.len() >= 3 {
        facet = parts[1].to_string();
        if parts.len() >= 4 && is_date_key(&basename) {
            day = basename.clone();
        } else if parts.len() >= 5 && parts[2] == "activities" && is_date_key(parts[3]) {
            day = parts[3].to_string();
        }
    }

    if parts.first() == Some(&"reflections")
        && parts.len() >= 3
        && parts.get(1) == Some(&"weekly")
        && is_date_key(&basename)
    {
        day = basename.clone();
    }

    if parts.first() == Some(&"imports") && parts.len() >= 2 {
        let import_id = parts[1];
        day = match import_id.split_once('_') {
            Some((prefix, _suffix)) => prefix.to_string(),
            None => import_id.chars().take(8).collect(),
        };
    }

    if parts.first() == Some(&"config")
        && parts.len() >= 3
        && parts.get(1) == Some(&"actions")
        && is_date_key(&basename)
    {
        day = basename.clone();
    }

    if is_markdown {
        if parts.first() == Some(&"facets") && parts.len() >= 4 && parts.get(2) == Some(&"news") {
            agent = "news".to_string();
        } else if parts.first() == Some(&"reflections")
            && parts.len() >= 3
            && parts.get(1) == Some(&"weekly")
        {
            agent = "reflection".to_string();
        } else if parts.first() == Some(&"imports") {
            agent = "import".to_string();
        } else if parts.first() == Some(&"apps") && parts.len() >= 4 {
            agent = format!("{}:{basename}", parts[1]);
        } else {
            agent = basename;
        }
    }

    PathMetadata { day, facet, agent }
}

fn basename(filename: &str) -> String {
    if let Some(value) = filename.strip_suffix(".md") {
        return value.to_string();
    }
    match filename.rsplit_once('.') {
        Some((stem, _extension)) => stem.to_string(),
        None => filename.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(rel: &str) -> (String, String, String) {
        let got = extract_path_metadata(rel);
        (got.day, got.facet, got.agent)
    }

    #[test]
    fn metadata_matrix_matches_python_shapes() {
        let cases = [
            ("20240101/talents/flow.md", "20240101", "", "flow"),
            (
                "20240101/default/123456_300/talents/audio.md",
                "20240101",
                "",
                "audio",
            ),
            (
                "20240101/default/123456_300/talents/work/brief.md",
                "20240101",
                "work",
                "brief",
            ),
            ("facets/work/news/20240101.md", "20240101", "work", "news"),
            (
                "facets/work/activities/20260214/coding_093000_300/session_review.md",
                "20260214",
                "work",
                "session_review",
            ),
            (
                "reflections/weekly/20260308.md",
                "20260308",
                "",
                "reflection",
            ),
            (
                "imports/20260101_120000/summary.md",
                "20260101",
                "",
                "import",
            ),
            (
                "20260101/import.ics/090000_300/imported.md",
                "20260101",
                "",
                "imported",
            ),
            (
                "20260101/import.ics/090000_300/event_transcript.md",
                "20260101",
                "",
                "event_transcript",
            ),
            ("config/actions/20240101.jsonl", "20240101", "", ""),
            ("facets/work/events/20240101.jsonl", "20240101", "work", ""),
            (
                "facets/work/activities/20240101.jsonl",
                "20240101",
                "work",
                "",
            ),
            (
                "facets/work/entities/alice/observations.jsonl",
                "",
                "work",
                "",
            ),
            ("facets/work/logs/20240101.jsonl", "20240101", "work", ""),
        ];
        for (rel, day, facet, agent) in cases {
            assert_eq!(
                meta(rel),
                (day.to_string(), facet.to_string(), agent.to_string()),
                "{rel}"
            );
        }
    }
}
