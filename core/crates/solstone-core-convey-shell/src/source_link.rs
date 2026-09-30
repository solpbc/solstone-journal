// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Extension, Query};
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use solstone_core_convey_http::cant_open_response;
use solstone_core_facets::{
    FacetIdResolveError, RetiredFacetState, RetiredFacets, is_well_formed_facet_id, read_news_file,
    read_retired_facets, resolve_facet_id, retired_facets_path,
};
use solstone_core_facets_web::valid_facet;
use solstone_core_format::segment::segment_parse;
use solstone_core_journal_io::cortex_use::talent_directory_name;
use solstone_core_journal_io::path_lexists;
use solstone_core_transcripts_web::{DaySegmentRef, day_segment_list};

use crate::session_gate;

pub trait SourceReads: Send + Sync {
    fn path_exists(&self, path: &Path) -> Result<bool, String>;
    fn read_day_segments(
        &self,
        journal: &Path,
        day: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<DaySegmentRef>, String>;
    fn is_facet_dir(&self, journal: &Path, facet: &str) -> Result<bool, String>;
    fn read_retired_facets(&self, journal: &Path) -> Result<RetiredFacets, String>;
    fn resolve_facet_id(&self, journal: &Path, id: &str) -> Result<String, FacetIdResolveError>;
    fn read_news_file(
        &self,
        journal: &Path,
        facet: &str,
        file: &str,
    ) -> Result<Option<String>, String>;
    fn read_file_text(&self, path: &Path) -> Result<String, String>;
    fn read_run_record(&self, path: &Path) -> Result<Option<String>, String>;
}

pub struct FilesystemReads;

impl SourceReads for FilesystemReads {
    fn path_exists(&self, path: &Path) -> Result<bool, String> {
        path_lexists(path).map_err(|err| err.to_string())
    }

    fn read_day_segments(
        &self,
        journal: &Path,
        day: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<DaySegmentRef>, String> {
        day_segment_list(journal, day, now)
    }

    fn is_facet_dir(&self, journal: &Path, facet: &str) -> Result<bool, String> {
        let path = journal.join("facets").join(facet);
        match fs::metadata(&path) {
            Ok(meta) => Ok(meta.is_dir()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err.to_string()),
        }
    }

    fn read_retired_facets(&self, journal: &Path) -> Result<RetiredFacets, String> {
        retired_facets_path(journal).map_err(|err| err.to_string())?;
        Ok(read_retired_facets(journal))
    }

    fn resolve_facet_id(&self, journal: &Path, id: &str) -> Result<String, FacetIdResolveError> {
        resolve_facet_id(journal, id)
    }

    fn read_news_file(
        &self,
        journal: &Path,
        facet: &str,
        file: &str,
    ) -> Result<Option<String>, String> {
        read_news_file(journal, facet, file).map_err(|err| err.to_string())
    }

    fn read_file_text(&self, path: &Path) -> Result<String, String> {
        fs::read_to_string(path).map_err(|err| err.to_string())
    }

    fn read_run_record(&self, path: &Path) -> Result<Option<String>, String> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }
}

pub fn source_link_router(journal: PathBuf, reads: Arc<dyn SourceReads + Send + Sync>) -> Router {
    Router::new()
        .route("/source", get(handle_source_link))
        .layer(Extension(Arc::new(journal)))
        .layer(Extension(reads))
}

async fn handle_source_link(
    Query(query): Query<BTreeMap<String, String>>,
    Extension(journal): Extension<Arc<PathBuf>>,
    Extension(reads): Extension<Arc<dyn SourceReads + Send + Sync>>,
) -> Response {
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::SourceLink,
        move || {
            let Some(raw_ref) = query.get("ref").filter(|s| !s.trim().is_empty()) else {
                return cant_open_response(
                    StatusCode::BAD_REQUEST,
                    "your journal won't follow this link.",
                );
            };

            let classified = match parse_and_validate_reference(raw_ref) {
                Ok(c) => c,
                Err(RefusalKind::WontFollow) => {
                    return cant_open_response(
                        StatusCode::BAD_REQUEST,
                        "your journal won't follow this link.",
                    );
                }
                Err(RefusalKind::CantShow) => {
                    return cant_open_response(
                        StatusCode::NOT_FOUND,
                        "your journal can't show this kind of source.",
                    );
                }
            };

            let now = Utc::now();
            match resolve_landing(&journal, &classified, reads.as_ref(), now) {
                Ok(LandingOutcome::Redirect(location)) => redirect_response(&location),
                Ok(LandingOutcome::Absence) => {
                    cant_open_response(StatusCode::NOT_FOUND, "it isn't in your journal.")
                }
                Ok(LandingOutcome::CantShow) => cant_open_response(
                    StatusCode::NOT_FOUND,
                    "your journal can't show this kind of source.",
                ),
                Ok(LandingOutcome::WontFollow) => cant_open_response(
                    StatusCode::BAD_REQUEST,
                    "your journal won't follow this link.",
                ),
                Err(err) => {
                    log::error!("source_link: couldn't check: {err}");
                    cant_open_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "your journal couldn't check this source.",
                    )
                }
            }
        },
    )
    .await
}

enum RefusalKind {
    WontFollow,
    CantShow,
}

enum LandingOutcome {
    Redirect(String),
    Absence,
    CantShow,
    WontFollow,
}

enum ClassifiedRef {
    Segment {
        day: String,
        stream: Option<String>,
        segment_key: String,
    },
    Newsletter {
        facet: String,
        day: String,
    },
    Activity {
        facet: String,
        day: String,
        id: String,
    },
    Run {
        day: String,
        relative: String,
    },
    Weekly {
        day: String,
    },
    CantShowArm,
}

fn is_plain_name(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && component
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

pub fn reference_is_moment(uri: &str) -> bool {
    matches!(
        parse_and_validate_reference(uri),
        Ok(ClassifiedRef::Segment { .. } | ClassifiedRef::Activity { .. })
    )
}

fn parse_and_validate_reference(raw: &str) -> Result<ClassifiedRef, RefusalKind> {
    let raw_trimmed = raw.trim();
    let bytes = raw_trimmed.as_bytes();
    if bytes.len() < 6 || !bytes[..6].eq_ignore_ascii_case(b"sol://") {
        return Err(RefusalKind::WontFollow);
    }

    let without_scheme = &raw_trimmed[6..];
    if without_scheme.is_empty() || without_scheme.starts_with('/') {
        return Err(RefusalKind::WontFollow);
    }

    let (path_part, fragment_part) = match without_scheme.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (without_scheme, None),
    };

    let raw_components: Vec<&str> = path_part.split('/').collect();
    if raw_components.is_empty() || raw_components.iter().any(|c| !is_plain_name(c)) {
        return Err(RefusalKind::WontFollow);
    }

    if let Some(fragment) = fragment_part
        && fragment.bytes().any(|b| b < 0x20 || b == 0x7f)
    {
        return Err(RefusalKind::WontFollow);
    }

    let is_day_key = |val: &str| val.len() == 8 && val.bytes().all(|b| b.is_ascii_digit());

    let is_valid_calendar_date =
        |val: &str| is_day_key(val) && NaiveDate::parse_from_str(val, "%Y%m%d").is_ok();

    // 1. Facet schemas
    if raw_components.first() == Some(&"facets") {
        if raw_components.len() == 4 {
            let facet = raw_components[1];
            let kind = raw_components[2];
            let target = raw_components[3];

            if !valid_facet(facet) {
                return Err(RefusalKind::WontFollow);
            }

            match kind {
                "news" => {
                    let day_str = target.strip_suffix(".md").unwrap_or(target);
                    if !is_day_key(day_str) {
                        return Err(RefusalKind::WontFollow);
                    }
                    if !is_valid_calendar_date(day_str) {
                        return Err(RefusalKind::WontFollow);
                    }
                    if fragment_part.is_some() {
                        return Err(RefusalKind::CantShow);
                    }
                    return Ok(ClassifiedRef::Newsletter {
                        facet: facet.to_owned(),
                        day: day_str.to_owned(),
                    });
                }
                "activities" => {
                    if !is_day_key(target) || !is_valid_calendar_date(target) {
                        return Err(RefusalKind::WontFollow);
                    }
                    let Some(id) = fragment_part else {
                        return Err(RefusalKind::CantShow);
                    };
                    if !is_plain_name(id) {
                        return Err(RefusalKind::WontFollow);
                    }
                    return Ok(ClassifiedRef::Activity {
                        facet: facet.to_owned(),
                        day: target.to_owned(),
                        id: id.to_owned(),
                    });
                }
                "events" => {
                    if is_valid_calendar_date(target) {
                        return Ok(ClassifiedRef::CantShowArm);
                    }
                    return Err(RefusalKind::CantShow);
                }
                "reflections" => {
                    if is_valid_calendar_date(target) {
                        return Ok(ClassifiedRef::CantShowArm);
                    }
                    return Err(RefusalKind::CantShow);
                }
                _ => return Err(RefusalKind::CantShow),
            }
        }
        return Err(RefusalKind::CantShow);
    }

    // 2. Reflections arm
    if raw_components.first() == Some(&"reflections") {
        if raw_components.len() == 3 && raw_components[1] == "weekly" {
            let target = raw_components[2];
            if solstone_core_home::weekly::week_day(target).is_none() {
                return Err(RefusalKind::WontFollow);
            }
            if fragment_part.is_some() {
                return Err(RefusalKind::CantShow);
            }
            return Ok(ClassifiedRef::Weekly {
                day: target.to_owned(),
            });
        }
        return Err(RefusalKind::CantShow);
    }

    // 3. Chronicle prefix for runs: sol://chronicle/<day>/<relative>
    if raw_components.first() == Some(&"chronicle") {
        if raw_components.len() >= 3 {
            let day = raw_components[1];
            if !is_day_key(day) || !is_valid_calendar_date(day) {
                return Err(RefusalKind::WontFollow);
            }
            let relative = raw_components[2..].join("/");
            if fragment_part.is_some() {
                return Err(RefusalKind::CantShow);
            }
            return Ok(ClassifiedRef::Run {
                day: day.to_owned(),
                relative,
            });
        }
        return Err(RefusalKind::WontFollow);
    }

    // 4. Day-rooted schemas: sol://<day>/...
    let day = raw_components[0];
    if is_day_key(day) {
        if !is_valid_calendar_date(day) {
            return Err(RefusalKind::WontFollow);
        }

        if fragment_part.is_some() {
            return Err(RefusalKind::CantShow);
        }

        if raw_components.len() == 2 {
            let candidate = raw_components[1];
            if segment_parse(candidate).is_some() {
                return Ok(ClassifiedRef::Segment {
                    day: day.to_owned(),
                    stream: None,
                    segment_key: candidate.to_owned(),
                });
            }
            if candidate.contains('.') {
                return Ok(ClassifiedRef::Run {
                    day: day.to_owned(),
                    relative: candidate.to_owned(),
                });
            }
            return Err(RefusalKind::WontFollow);
        }

        if raw_components.len() == 3 {
            let middle = raw_components[1];
            let last = raw_components[2];
            if middle == "talents" {
                return Ok(ClassifiedRef::Run {
                    day: day.to_owned(),
                    relative: format!("{middle}/{last}"),
                });
            }
            if segment_parse(last).is_some() {
                return Ok(ClassifiedRef::Segment {
                    day: day.to_owned(),
                    stream: Some(middle.to_owned()),
                    segment_key: last.to_owned(),
                });
            }
            if last.contains('.') {
                return Ok(ClassifiedRef::Run {
                    day: day.to_owned(),
                    relative: format!("{middle}/{last}"),
                });
            }
            return Err(RefusalKind::WontFollow);
        }

        if raw_components.len() > 3 {
            let relative = raw_components[1..].join("/");
            return Ok(ClassifiedRef::Run {
                day: day.to_owned(),
                relative,
            });
        }
    }

    Err(RefusalKind::CantShow)
}

fn resolve_landing(
    journal: &Path,
    target: &ClassifiedRef,
    reads: &dyn SourceReads,
    now: DateTime<Utc>,
) -> Result<LandingOutcome, String> {
    match target {
        ClassifiedRef::Segment {
            day,
            stream,
            segment_key,
        } => resolve_segment(journal, day, stream.as_deref(), segment_key, reads, now),
        ClassifiedRef::Newsletter { facet, day } => resolve_newsletter(journal, facet, day, reads),
        ClassifiedRef::Activity { facet, day, id } => {
            resolve_activity(journal, facet, day, id, reads, now)
        }
        ClassifiedRef::Run { day, relative } => resolve_run(journal, day, relative, reads),
        ClassifiedRef::Weekly { day } => match solstone_core_home::weekly::judge(journal, day) {
            solstone_core_home::weekly::WeekJudgment::Page(_) => {
                let location = format!("/app/home/week/{}", encode_path_component(day));
                if !is_safe_location(&location) {
                    return Ok(LandingOutcome::WontFollow);
                }
                Ok(LandingOutcome::Redirect(location))
            }
            solstone_core_home::weekly::WeekJudgment::Absent => Ok(LandingOutcome::Absence),
            solstone_core_home::weekly::WeekJudgment::CantShow => Ok(LandingOutcome::CantShow),
            solstone_core_home::weekly::WeekJudgment::CouldntCheck => {
                Err("weekly reflection couldn't be checked".to_string())
            }
        },
        ClassifiedRef::CantShowArm => Ok(LandingOutcome::CantShow),
    }
}

fn resolve_segment(
    journal: &Path,
    day: &str,
    target_stream: Option<&str>,
    segment_key: &str,
    reads: &dyn SourceReads,
    now: DateTime<Utc>,
) -> Result<LandingOutcome, String> {
    let day_dir = journal.join("chronicle").join(day);
    let day_exists = reads.path_exists(&day_dir)?;
    if !day_exists {
        return Ok(LandingOutcome::Absence);
    }

    let segments = reads.read_day_segments(journal, day, now)?;

    let matching_segments: Vec<&DaySegmentRef> = segments
        .iter()
        .filter(|s| {
            if s.key != segment_key {
                return false;
            }
            match target_stream {
                None => s.direct && s.stream == "_default",
                Some(st) => !s.direct && s.stream == st,
            }
        })
        .collect();

    match matching_segments.len() {
        0 => Ok(LandingOutcome::Absence),
        1 => {
            let stream_param = match target_stream {
                None => "_default",
                Some(st) => st,
            };
            let location = format!(
                "/app/transcripts/{}?stream={}#{}",
                encode_path_component(day),
                encode_path_component(stream_param),
                encode_path_component(segment_key)
            );
            if !is_safe_location(&location) {
                return Ok(LandingOutcome::WontFollow);
            }
            Ok(LandingOutcome::Redirect(location))
        }
        _ => Ok(LandingOutcome::CantShow),
    }
}

fn resolve_facet_directory(
    journal: &Path,
    facet_name: &str,
    reads: &dyn SourceReads,
) -> Result<Result<String, LandingOutcome>, String> {
    let is_dir = reads.is_facet_dir(journal, facet_name)?;
    if is_dir {
        return Ok(Ok(facet_name.to_owned()));
    }

    let retired = reads.read_retired_facets(journal)?;
    let map = match retired {
        RetiredFacets::Loaded(m) => m,
        RetiredFacets::Absent => return Ok(Err(LandingOutcome::Absence)),
        RetiredFacets::Malformed(msg) | RetiredFacets::Unreadable(msg) => {
            return Err(format!("retired.json unusable: {msg}"));
        }
    };

    let Some(entry) = map.get(facet_name) else {
        return Ok(Err(LandingOutcome::Absence));
    };

    match entry.state {
        RetiredFacetState::Deleted => Ok(Err(LandingOutcome::Absence)),
        RetiredFacetState::Merged => Ok(Err(LandingOutcome::CantShow)),
        RetiredFacetState::Renamed => {
            let Some(successor_id) = &entry.successor else {
                return Err("renamed facet missing successor".to_owned());
            };
            if !is_well_formed_facet_id(successor_id) {
                return Err("renamed facet has invalid successor id".to_owned());
            }
            match reads.resolve_facet_id(journal, successor_id) {
                Ok(resolved_dir) => Ok(Ok(resolved_dir)),
                Err(FacetIdResolveError::Missing) => Ok(Err(LandingOutcome::Absence)),
                Err(
                    FacetIdResolveError::Store
                    | FacetIdResolveError::Duplicate
                    | FacetIdResolveError::Malformed,
                ) => Err("failed to resolve renamed facet id".to_owned()),
            }
        }
    }
}

fn resolve_newsletter(
    journal: &Path,
    facet: &str,
    day: &str,
    reads: &dyn SourceReads,
) -> Result<LandingOutcome, String> {
    let resolved_facet = match resolve_facet_directory(journal, facet, reads)? {
        Ok(dir) => dir,
        Err(outcome) => return Ok(outcome),
    };

    let file_name = format!("{day}.md");
    let content = reads.read_news_file(journal, &resolved_facet, &file_name)?;
    match content {
        Some(_) => {
            let location = format!(
                "/app/news/{}/{}",
                encode_path_component(&resolved_facet),
                encode_path_component(day)
            );
            if !is_safe_location(&location) {
                return Ok(LandingOutcome::WontFollow);
            }
            Ok(LandingOutcome::Redirect(location))
        }
        None => Ok(LandingOutcome::Absence),
    }
}

fn resolve_activity(
    journal: &Path,
    facet: &str,
    day: &str,
    id: &str,
    reads: &dyn SourceReads,
    now: DateTime<Utc>,
) -> Result<LandingOutcome, String> {
    let resolved_facet = match resolve_facet_directory(journal, facet, reads)? {
        Ok(dir) => dir,
        Err(outcome) => return Ok(outcome),
    };

    let activity_file = journal
        .join("facets")
        .join(&resolved_facet)
        .join("activities")
        .join(format!("{day}.jsonl"));

    let exists = reads.path_exists(&activity_file)?;
    if !exists {
        return Ok(LandingOutcome::Absence);
    }

    let text = reads.read_file_text(&activity_file)?;
    let mut matching_row = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed)
            && map.get("id").and_then(Value::as_str) == Some(id)
        {
            matching_row = Some(map);
            break;
        }
    }

    let Some(row) = matching_row else {
        return Ok(LandingOutcome::Absence);
    };

    let Some(segments_val) = row.get("segments").and_then(Value::as_array) else {
        return Ok(LandingOutcome::CantShow);
    };

    let candidate_keys: Vec<&str> = segments_val
        .iter()
        .filter_map(Value::as_str)
        .filter(|k| segment_parse(k).is_some())
        .collect();

    if candidate_keys.is_empty() {
        return Ok(LandingOutcome::CantShow);
    }

    let day_dir = journal.join("chronicle").join(day);
    let day_exists = reads.path_exists(&day_dir)?;
    if !day_exists {
        return Ok(LandingOutcome::Absence);
    }

    let day_segments = reads.read_day_segments(journal, day, now)?;
    let mut saw_ambiguity = false;

    for key in candidate_keys {
        let matching_day_segs: Vec<&DaySegmentRef> =
            day_segments.iter().filter(|s| s.key == key).collect();
        if matching_day_segs.is_empty() {
            continue;
        }
        if matching_day_segs.len() > 1 {
            saw_ambiguity = true;
            continue;
        }
        let seg = matching_day_segs[0];
        let stream_param = if seg.direct { "_default" } else { &seg.stream };
        let location = format!(
            "/app/transcripts/{}?stream={}#{}",
            encode_path_component(day),
            encode_path_component(stream_param),
            encode_path_component(&seg.key)
        );
        if !is_safe_location(&location) {
            return Ok(LandingOutcome::WontFollow);
        }
        return Ok(LandingOutcome::Redirect(location));
    }

    if saw_ambiguity {
        Ok(LandingOutcome::CantShow)
    } else {
        Ok(LandingOutcome::Absence)
    }
}

fn resolve_run(
    journal: &Path,
    day: &str,
    relative: &str,
    reads: &dyn SourceReads,
) -> Result<LandingOutcome, String> {
    let output_file_path = journal.join("chronicle").join(day).join(relative);
    let index_file = journal.join("talents").join(format!("{day}.jsonl"));
    let index_exists = reads.path_exists(&index_file)?;

    let rel_parts: Vec<&str> = relative.split('/').collect();

    let mut matching_rows = Vec::new();
    if index_exists {
        let text = reads.read_file_text(&index_file)?;
        for (line_idx, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            if map.get("status").and_then(Value::as_str) == Some("error") {
                continue;
            }
            let Some(out) = map.get("output_file").and_then(Value::as_str) else {
                continue;
            };
            let out_parts: Vec<&str> = out.split(['/', '\\']).collect();
            let matches = out_parts == rel_parts
                || (out_parts.len() == rel_parts.len() + 2
                    && out_parts[0] == "chronicle"
                    && out_parts[1] == day
                    && &out_parts[2..] == rel_parts.as_slice());
            if matches {
                let ts = map.get("ts").and_then(Value::as_i64).unwrap_or(0);
                matching_rows.push((ts, line_idx, map));
            }
        }
    }

    matching_rows.sort_by(|(ts_a, idx_a, _), (ts_b, idx_b, _)| {
        ts_b.cmp(ts_a).then_with(|| idx_b.cmp(idx_a))
    });

    if let Some((_, _, map)) = matching_rows.first() {
        let name = map
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing or empty name in run index".to_owned())?;
        let use_id = map
            .get("use_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing or empty use_id in run index".to_owned())?;

        let record_path = journal
            .join("talents")
            .join(talent_directory_name(name))
            .join(format!("{use_id}.jsonl"));

        match reads.read_run_record(&record_path)? {
            None => Ok(LandingOutcome::Absence),
            Some(text) => {
                let Some(first_line) = text.lines().map(str::trim).find(|l| !l.is_empty()) else {
                    return Err("run record is empty".to_owned());
                };
                let Ok(Value::Object(record_map)) = serde_json::from_str::<Value>(first_line)
                else {
                    return Err("run record first line is not a JSON object".to_owned());
                };
                if record_map.get("use_id").and_then(Value::as_str) != Some(use_id) {
                    return Err("run record use_id mismatch".to_owned());
                }

                let file_exists = reads.path_exists(&output_file_path)?;
                if !file_exists {
                    return Ok(LandingOutcome::Absence);
                }
                let location = format!("/app/thinking/#runs/run/{}", encode_path_component(use_id));
                if !is_safe_location(&location) {
                    return Ok(LandingOutcome::WontFollow);
                }
                Ok(LandingOutcome::Redirect(location))
            }
        }
    } else {
        let file_exists = reads.path_exists(&output_file_path)?;
        if file_exists {
            Ok(LandingOutcome::CantShow)
        } else {
            Ok(LandingOutcome::Absence)
        }
    }
}

fn encode_path_component(val: &str) -> String {
    let mut encoded = String::new();
    for b in val.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{:02X}", b));
        }
    }
    encoded
}

fn is_safe_location(loc: &str) -> bool {
    loc.starts_with('/') && !loc.starts_with("//") && !loc.starts_with("/\\")
}

fn redirect_response(location: &str) -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(session_gate::redirect_body(
            location,
        )))
        .expect("redirect response builds")
}
