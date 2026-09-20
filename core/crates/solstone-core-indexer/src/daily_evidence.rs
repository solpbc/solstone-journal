// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only daily source projection. This is independent of the search database.

use chrono::{Duration, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::content::{self, ContentResolution, Family};
use solstone_core_journal_io::paths::{PathOrDay, iter_segments};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

pub const SEMANTIC_EVIDENCE_VERSION: &str = "daily-sources-3";
const OUTPUTS: &[&str] = &[
    "schedule",
    "daily_schedule",
    "facet_newsletter",
    "morning_briefing",
    "entities_review",
    "entity_suggest",
    "entity_observer",
    "_entities_entities_review",
    "_entities_entity_suggest",
    "_entities_entity_observer",
    "pulse",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceChunk {
    pub path: String,
    pub idx: usize,
    pub text: String,
    pub day: String,
    pub facet: String,
    pub agent: String,
    pub occurrence_time_ms: Option<i64>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceEvidence {
    pub path: String,
    pub shape: String,
    pub content_digest: String,
    pub agent: String,
    pub facet: String,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SourceProjection {
    pub chunks: Vec<SourceChunk>,
    pub sources: Vec<SourceEvidence>,
}

fn entries(path: &Path) -> Result<Vec<fs::DirEntry>, String> {
    let values = match fs::read_dir(path) {
        Ok(values) => values,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let mut values = values
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    values.sort_by_key(|entry| entry.file_name());
    Ok(values)
}

fn collect(
    root: &Path,
    directory: &Path,
    out: &mut BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    for entry in entries(directory)? {
        let name = entry.file_name();
        let name = name.to_str().ok_or("non-UTF8 daily source path")?;
        if name.starts_with('.') || matches!(name, "health" | "logs" | "news") {
            continue;
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "daily source is a symbolic link: {}",
                entry.path().display()
            ));
        }
        if kind.is_dir() {
            collect(root, &entry.path(), out)?;
        } else if kind.is_file() {
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or("non-UTF8 daily source path")?
                .replace('\\', "/");
            out.insert(rel, path);
        }
    }
    Ok(())
}

fn normalized_rel(rel: &str) -> String {
    // The canonical family registry's segment coordinate includes a stream.
    // Direct segments retain their real identity; only classification gets this coordinate.
    let parts = rel.split('/').collect::<Vec<_>>();
    if parts.len() >= 3
        && parts[0].len() == 8
        && parts[1]
            .split_once('_')
            .is_some_and(|(clock, _)| clock.len() == 6 && clock.bytes().all(|b| b.is_ascii_digit()))
    {
        format!("{}/_direct/{}", parts[0], parts[1..].join("/"))
    } else {
        rel.to_owned()
    }
}

fn strict_text(path: &Path) -> Result<String, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    match path.extension().and_then(|v| v.to_str()) {
        Some("jsonl") => {
            for (i, line) in text
                .lines()
                .enumerate()
                .filter(|(_, s)| !s.trim().is_empty())
            {
                let row: Value = serde_json::from_str(line)
                    .map_err(|e| format!("{} line {}: {e}", path.display(), i + 1))?;
                if !row.is_object() {
                    return Err(format!(
                        "{} line {} is not an object",
                        path.display(),
                        i + 1
                    ));
                }
            }
        }
        Some("json") => {
            let value: Value =
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            if !value.is_object() {
                return Err(format!("{} is not a JSON object", path.display()));
            }
        }
        _ => {}
    }
    Ok(text)
}

fn output_path(rel: &str) -> bool {
    let parts = rel.split('/').collect::<Vec<_>>();
    let stem = Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    parts.contains(&"talents") && OUTPUTS.contains(&stem)
}

/// Canonical chunks for one day, in source-time/path/original-chunk order.
/// No all-history discovery, index reads, or source mutations occur here.
pub fn capture_day_sources(journal: &Path, day: &str) -> Result<Vec<SourceChunk>, String> {
    Ok(capture_day_projection(journal, day)?.chunks)
}

pub fn capture_day_projection(journal: &Path, day: &str) -> Result<SourceProjection, String> {
    NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|e| format!("invalid day: {e}"))?;
    let chronicle = journal.join("chronicle");
    let mut files = BTreeMap::new();
    collect(&chronicle, &chronicle.join(day), &mut files)?;
    for facet in entries(&journal.join("facets"))? {
        if !facet.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        for kind in ["activities", "events", "entities"] {
            let path = facet.path().join(kind).join(format!("{day}.jsonl"));
            match fs::metadata(&path) {
                Ok(_) => {
                    files.insert(
                        path.strip_prefix(journal)
                            .map_err(|e| e.to_string())?
                            .to_string_lossy()
                            .replace('\\', "/"),
                        path,
                    );
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        collect(
            journal,
            &facet.path().join("activities").join(day),
            &mut files,
        )?;
    }
    project_sources(files)
}

fn project_sources(files: BTreeMap<String, PathBuf>) -> Result<SourceProjection, String> {
    let mut chunks = Vec::new();
    let mut sources = Vec::new();
    for (rel, path) in files {
        if output_path(&rel) {
            continue;
        }
        let normalized = normalized_rel(&rel);
        let mut resolution = content::classify(&normalized);
        let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if resolution == ContentResolution::Unrecognized
            && (filename.ends_with("_transcript.md") || filename == "imported.md")
        {
            resolution = ContentResolution::Indexed(Family::Markdown);
        }
        // A written shape is an input contract: malformed/unreadable sidecars cannot mean empty evidence.
        let sidecar = path.with_file_name(content::SHAPE_SIDECAR_BASENAME);
        match fs::read_to_string(&sidecar) {
            Ok(text) => {
                let shape: Value = serde_json::from_str(&text)
                    .map_err(|e| format!("{}: {e}", sidecar.display()))?;
                let shape = shape.as_object().ok_or("shape sidecar is not an object")?;
                if let Some(value) = shape.get(filename) {
                    resolution = value
                        .as_str()
                        .and_then(content::parse_shape_name)
                        .ok_or_else(|| format!("invalid shape for {rel}"))?;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        if matches!(
            resolution,
            ContentResolution::Unrecognized | ContentResolution::IndexedElsewhere
        ) {
            continue;
        }
        let mut text = strict_text(&path)?;
        if resolution == ContentResolution::Indexed(Family::Activity) {
            text = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| serde_json::from_str::<Value>(line).expect("validated JSONL"))
                .filter(|row| row.get("source").and_then(Value::as_str) != Some("anticipated"))
                .map(|row| row.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            // A file containing only generated anticipations is not factual
            // evidence, including when the generated file first appears.
            if text.is_empty() {
                continue;
            }
        }
        let content_digest = source_content_digest(&path, &text)?;
        let produced = match resolution {
            ContentResolution::Indexed(family) => {
                content::produce_chunks(family, &normalized, &text)
            }
            ContentResolution::Unindexed(family) => {
                content::produce_raw_percept_chunks(family, &normalized, &text)
            }
            _ => return Err(format!("invalid written source shape: {rel}")),
        };
        if let Some(error) = produced.error {
            return Err(format!("{rel}: {error}"));
        }
        let metadata = crate::metadata::extract_path_metadata(&rel);
        let agent = produced
            .agent_override
            .unwrap_or(metadata.agent)
            .to_lowercase();
        // Search formatting may drop long lines or omit structured fields.
        // Revision identity must retain every source field the hooks can read,
        // even when this source produces no searchable chunks at all.
        sources.push(SourceEvidence {
            path: rel.clone(),
            shape: format!("{resolution:?}"),
            content_digest,
            agent: agent.clone(),
            facet: metadata.facet.to_lowercase(),
            warnings: produced.warnings.clone(),
        });
        for (idx, chunk) in produced.chunks.into_iter().enumerate() {
            if chunk.content.trim().is_empty() {
                continue;
            }
            chunks.push(SourceChunk {
                path: rel.clone(),
                idx,
                text: chunk.content.trim().to_owned(),
                day: metadata.day.clone(),
                facet: metadata.facet.to_lowercase(),
                agent: agent.clone(),
                occurrence_time_ms: chunk.occurrence_time_ms.map(|t| t.0),
                warnings: produced.warnings.clone(),
            });
        }
    }
    chunks.sort_by(|a, b| {
        b.occurrence_time_ms
            .unwrap_or(i64::MIN)
            .cmp(&a.occurrence_time_ms.unwrap_or(i64::MIN))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.idx.cmp(&b.idx))
    });
    Ok(SourceProjection { chunks, sources })
}

fn source_content_digest(path: &Path, text: &str) -> Result<String, String> {
    let value = match path.extension().and_then(|v| v.to_str()) {
        Some("jsonl") => Value::Array(
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    let mut row: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
                    // Processing status is an operational header annotation. All
                    // factual fields and occurrence times retain their full values.
                    if let Some(object) = row.as_object_mut() {
                        object.remove("_solstone_processing");
                    }
                    Ok(row)
                })
                .collect::<Result<Vec<_>, String>>()?,
        ),
        Some("json") => serde_json::from_str(text).map_err(|e| e.to_string())?,
        _ => Value::String(text.to_owned()),
    };
    Ok(digest(&value))
}

fn capture_facet_day_sources(
    journal: &Path,
    day: &str,
    kind: &str,
    selected: Option<&str>,
) -> Result<SourceProjection, String> {
    let mut files = BTreeMap::new();
    for facet in entries(&journal.join("facets"))? {
        if !facet.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        if selected.is_some_and(|name| facet.file_name() != std::ffi::OsStr::new(name)) {
            continue;
        }
        let path = facet.path().join(kind).join(format!("{day}.jsonl"));
        match fs::metadata(&path) {
            Ok(_) => {
                let rel = path
                    .strip_prefix(journal)
                    .map_err(|e| e.to_string())?
                    .to_str()
                    .ok_or("non-UTF8 source path")?
                    .replace('\\', "/");
                files.insert(rel, path);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    project_sources(files)
}

pub fn daily_hook(name: &str, metadata: &Map<String, Value>) -> Result<String, String> {
    if metadata
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "generate")
    {
        return Err("unsupported daily execution type".to_owned());
    }
    let hook = Some(
        metadata
            .get("hook")
            .ok_or_else(|| format!("unsupported daily talent without declared hook: {name}"))?,
    );
    let pre = hook.and_then(|h| h.get("pre")).and_then(Value::as_str);
    let post = hook.and_then(|h| h.get("post")).and_then(Value::as_str);
    let effective = pre
        .or(post)
        .or_else(|| hook.and_then(Value::as_str))
        .ok_or_else(|| format!("unsupported empty daily hook: {name}"))?;
    if name != effective {
        return Err(format!(
            "unsupported daily talent/hook pairing: {name}/{effective}"
        ));
    }
    if !matches!(
        effective,
        "schedule"
            | "daily_schedule"
            | "facet_newsletter"
            | "morning_briefing"
            | "entities:entities_review"
            | "entities:entity_suggest"
            | "entities:entity_observer"
    ) || pre.zip(post).is_some_and(|(a, b)| a != b)
    {
        return Err(format!("unsupported daily hook: {effective}"));
    }
    let phases_match = match effective {
        "schedule" => pre.is_none() && post == Some(effective),
        "morning_briefing" => pre == Some(effective) && post.is_none(),
        _ => pre == Some(effective) && post == Some(effective),
    };
    if !phases_match {
        return Err(format!(
            "unsupported daily hook phase contract: {effective}"
        ));
    }
    Ok(effective.to_owned())
}

pub fn compute_contract_digest(
    journal: &Path,
    name: &str,
    metadata: &Map<String, Value>,
    body: &str,
    overrides: Option<&Map<String, Value>>,
) -> Result<String, String> {
    let mut effective = metadata.clone();
    let key = name
        .split_once(':')
        .map(|(app, n)| format!("talent.{app}.{n}"))
        .unwrap_or_else(|| format!("talent.system.{name}"));
    if let Some(values) = overrides
        .and_then(|v| v.get(&key))
        .and_then(Value::as_object)
    {
        for field in ["disabled", "extract"] {
            if let Some(v) = values.get(field) {
                effective.insert(field.to_owned(), v.clone());
            }
        }
        if let Some(v) = values
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)
        {
            effective.insert("max_output_tokens".to_owned(), json!(v));
        }
    }
    daily_hook(name, &effective)?;
    if let Some(schema) = effective.get("schema") {
        let schema = schema.as_str().ok_or("schema must be a filename")?;
        let base = effective
            .get("path")
            .and_then(Value::as_str)
            .and_then(|p| Path::new(p).parent())
            .ok_or("schema prompt path is missing")?;
        let schema: Value = serde_json::from_str(
            &fs::read_to_string(base.join(schema)).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        effective.insert("schema".to_owned(), schema);
    }
    let templates = template_contract(&effective, body)?;
    for key in [
        "path",
        "file",
        "mtime",
        "description",
        "label",
        "group",
        "priority",
    ] {
        effective.remove(key);
    }
    // Facet declarations determine both prompt scope and runtime schema substitution.
    let facets = facet_declarations(journal)?;
    let configured = solstone_core_journal_config::read_journal_config(journal)
        .map_err(|e| e.to_string())?
        .config
        .unwrap_or_default();
    let active = configured.get("providers").and_then(|v| v.get("active"));
    let brain = json!({"provider":active.and_then(|v|v.get("provider")),"model":active.and_then(|v|v.get("model"))});
    Ok(digest(
        &json!({"version":SEMANTIC_EVIDENCE_VERSION,"name":name,"metadata":effective,"body":body.trim(),"templates":templates,"facets":facets,"brain":brain}),
    ))
}

fn facet_declarations(journal: &Path) -> Result<BTreeMap<String, Value>, String> {
    let mut out = BTreeMap::new();
    for facet in entries(&journal.join("facets"))? {
        if !facet.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let path = facet.path().join("facet.json");
        match fs::read_to_string(&path) {
            Ok(text) => {
                let mut value: Value =
                    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                if let Some(map) = value.as_object_mut() {
                    for key in ["created_at", "updated_at", "last_seen"] {
                        map.remove(key);
                    }
                }
                out.insert(
                    facet
                        .file_name()
                        .to_str()
                        .ok_or("non-UTF8 facet")?
                        .to_owned(),
                    value,
                );
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(out)
}

pub fn digest(value: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("JSON serializes"))
    )
}

/// One day's source projection, captured at most once and shared by the units of a
/// single coverage read.
///
/// A day has dozens of daily units and all but a few derive their revision from the
/// same whole-day projection; capturing it per unit walks and re-hashes the day dozens
/// of times. The cache belongs to the caller of one read and is never global or shared
/// across threads, so every unit sees the day exactly as that one read captured it.
pub struct DayProjectionCache {
    journal: PathBuf,
    day: String,
    /// Only the source list is kept: the chunk text of a projection is large and no unit needs it.
    sources: Option<Result<Vec<SourceEvidence>, String>>,
    captures: u32,
    requests: u32,
}

impl DayProjectionCache {
    pub fn new(journal: &Path, day: &str) -> Self {
        Self {
            journal: journal.to_path_buf(),
            day: day.to_owned(),
            sources: None,
            captures: 0,
            requests: 0,
        }
    }

    /// Whole-day captures made on behalf of this cache's computations, including any
    /// made directly because a call did not match the cache's journal and day.
    #[doc(hidden)]
    pub fn captures(&self) -> u32 {
        self.captures
    }

    /// Times a computation asked this cache for the day's sources.
    #[doc(hidden)]
    pub fn requests(&self) -> u32 {
        self.requests
    }

    fn sources(&mut self) -> Result<Vec<SourceEvidence>, String> {
        self.requests += 1;
        if self.sources.is_none() {
            self.captures += 1;
            self.sources = Some(
                capture_day_projection(&self.journal, &self.day)
                    .map(|projection| projection.sources),
            );
        }
        match self.sources.as_ref() {
            Some(Ok(sources)) => Ok(sources.clone()),
            Some(Err(error)) => Err(error.clone()),
            None => unreachable!("captured above"),
        }
    }
}

pub fn compute_daily_evidence_revision(
    journal: &Path,
    day: &str,
    name: &str,
    metadata: &Map<String, Value>,
    body: &str,
    facet: Option<&str>,
    overrides: Option<&Map<String, Value>>,
) -> Result<(String, String), String> {
    compute_daily_evidence_revision_cached(
        journal,
        day,
        name,
        metadata,
        body,
        facet,
        overrides,
        &mut DayProjectionCache::new(journal, day),
    )
}

/// `compute_daily_evidence_revision` for a caller that computes several units of the
/// same day: the day's own sources are captured once through `cache` instead of once
/// per unit. `cache` must be for this `journal` and `day`; a cache for any other day
/// is ignored and the day is captured directly, so the result never depends on it.
#[allow(clippy::too_many_arguments)] // The unit identity and its cache travel together.
pub fn compute_daily_evidence_revision_cached(
    journal: &Path,
    day: &str,
    name: &str,
    metadata: &Map<String, Value>,
    body: &str,
    facet: Option<&str>,
    overrides: Option<&Map<String, Value>>,
    cache: &mut DayProjectionCache,
) -> Result<(String, String), String> {
    let contract = compute_contract_digest(journal, name, metadata, body, overrides)?;
    let hook = daily_hook(name, metadata)?;
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|e| e.to_string())?;
    let evidence = if hook == "daily_schedule" {
        let anchor = journal_today(journal)?;
        let lookback = metadata
            .get("meta")
            .and_then(|v| v.get("lookback_days"))
            .or_else(|| metadata.get("lookback_days"))
            .and_then(Value::as_i64)
            .unwrap_or(7);
        if !(1..=366).contains(&lookback) {
            return Err("daily schedule lookback outside 1..=366".to_owned());
        }
        let mut windows = BTreeMap::new();
        for offset in 0..lookback {
            let d = (anchor - Duration::days(offset))
                .format("%Y%m%d")
                .to_string();
            let keys = iter_segments(journal, PathOrDay::Day(&d))
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|s| s.key().to_owned())
                .collect::<Vec<_>>();
            windows.insert(d, keys);
        }
        json!({"anchor":anchor.to_string(),"windows":windows})
    } else {
        let offsets: Vec<i64> = match hook.as_str() {
            "entities:entities_review" => (-7..=-1).collect(),
            "morning_briefing" => (0..=8).collect(),
            _ => vec![0],
        };
        // A closed day's evidence stops at the last closed day.  The briefing
        // hook reads forward, and the adoption boundary is seven closed days,
        // so without this every adopted day's window contains the current day
        // — which is still being written, so the revision moves between the
        // reading taken at admission and the one taken after preparation and
        // the day can never freeze.  The day's own sources are never dropped:
        // only offsets that move forward past the last closed day are.
        let today = journal_today(journal)?.format("%Y%m%d").to_string();
        let mut sources = Vec::new();
        for offset in offsets {
            let d = (date + Duration::days(offset)).format("%Y%m%d").to_string();
            if offset > 0 && d >= today {
                continue;
            }
            sources.extend(if hook == "entities:entities_review" {
                capture_facet_day_sources(journal, &d, "entities", facet)?.sources
            } else if offset > 0 {
                capture_facet_day_sources(journal, &d, "activities", None)?.sources
            } else if cache.journal == journal && cache.day == d {
                cache.sources()?
            } else {
                cache.captures += 1;
                capture_day_projection(journal, &d)?.sources
            });
        }
        let suggestions_digest = if hook == "entities:entity_observer" {
            let sugg_path = journal
                .join("facets")
                .join(facet.unwrap_or_default())
                .join("entities")
                .join(format!("{day}_observer_suggestions.json"));
            if let Ok(bytes) = fs::read(&sugg_path) {
                format!("{:x}", Sha256::digest(&bytes))
            } else {
                "missing".to_string()
            }
        } else {
            String::new()
        };
        json!({"day":day,"facet":facet,"sources":sources,"suggestions_digest":suggestions_digest,"upstream":if hook=="morning_briefing" {upstream_evidence(journal,day)?}else{Value::Null}})
    };
    Ok((
        digest(&json!({"contract":contract,"evidence":evidence})),
        contract,
    ))
}

pub fn journal_today(journal: &Path) -> Result<NaiveDate, String> {
    let config = solstone_core_journal_config::read_journal_config(journal)
        .map_err(|e| e.to_string())?
        .config
        .unwrap_or_default();
    let zone = config
        .get("identity")
        .and_then(|v| v.get("timezone"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if zone.is_empty() {
        return Ok(Local::now().date_naive());
    }
    let zone: chrono_tz::Tz = zone
        .parse()
        .map_err(|_| format!("invalid journal timezone {zone}"))?;
    Ok(chrono::Utc::now().with_timezone(&zone).date_naive())
}

fn upstream_evidence(journal: &Path, day: &str) -> Result<Value, String> {
    use solstone_core_journal_io::{
        DailyUnitIdentity, accepted_daily_artifacts_valid, load_daily_unit_record,
    };
    let mut identities = vec![DailyUnitIdentity::new(day, "schedule", None)];
    identities.extend(
        facet_declarations(journal)?
            .keys()
            .map(|facet| DailyUnitIdentity::new(day, "facet_newsletter", Some(facet.clone()))),
    );
    let mut values = Vec::new();
    for identity in identities {
        let value = match load_daily_unit_record(journal, &identity).map_err(|e| e.to_string())? {
            None => Value::Null,
            Some(record) => {
                json!({"evidence":record.evidence_revision,"contract":record.contract_digest,"status":record.status,"accepted":record.accepted.as_ref().map(|a|json!({"evidence":a.evidence_revision,"status":a.status,"artifacts":a.receipts.iter().filter(|r|r.get("kind").and_then(Value::as_str)==Some("required_artifact")).collect::<Vec<_>>()})),"artifacts_valid":accepted_daily_artifacts_valid(journal,&record).map_err(|e|e.to_string())?})
            }
        };
        values.push(json!({"name":identity.name,"facet":identity.facet,"outcome":value}));
    }
    Ok(json!(values))
}

fn template_contract(
    metadata: &Map<String, Value>,
    body: &str,
) -> Result<BTreeMap<String, String>, String> {
    let mut templates = BTreeMap::new();
    let Some(path) = metadata.get("path").and_then(Value::as_str) else {
        return Ok(templates);
    };
    let Some(root) = Path::new(path)
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "solstone"))
    else {
        return Ok(templates);
    };
    for entry in entries(&root.join("think/templates"))? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("non-UTF8 template name")?;
        if body.contains(&format!("${stem}")) || body.contains(&format!("${{{stem}}}")) {
            templates.insert(
                stem.to_owned(),
                fs::read_to_string(&path).map_err(|e| e.to_string())?,
            );
        }
    }
    Ok(templates)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root(name: &str) -> PathBuf {
        let root = crate::test_support::reserve_temp_path(name);
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    fn revision(root: &Path, name: &str, facet: Option<&str>) -> String {
        let hook = match name {
            "schedule" => json!({"post":name}),
            "morning_briefing" => json!({"pre":name}),
            _ => json!({"pre":name,"post":name}),
        };
        compute_daily_evidence_revision(
            root,
            "20260910",
            name,
            &Map::from_iter([("hook".to_owned(), hook)]),
            "prompt",
            facet,
            None,
        )
        .unwrap()
        .0
    }

    #[test]
    fn daily_evidence_tracks_named_direct_imported_sources_and_removal() {
        let root = root("daily-sources-shapes");
        let files = [
            (
                "chronicle/20260910/mic/090000_60/talents/audio.md",
                "# Named\n\nA useful fact.",
            ),
            (
                "chronicle/20260910/100000_60/_transcript.md",
                "# Direct\n\nAnother fact.",
            ),
            (
                "chronicle/20260910/import.ics/110000_60/event_transcript.md",
                "# Imported\n\nAn event.",
            ),
        ];
        let mut previous = revision(&root, "schedule", None);
        for (rel, text) in files {
            write(&root, rel, text);
            let next = revision(&root, "schedule", None);
            assert_ne!(previous, next, "{rel}");
            previous = next;
        }
        fs::remove_file(root.join(files[0].0)).unwrap();
        assert_ne!(previous, revision(&root, "schedule", None));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn daily_evidence_projection_preserves_agent_and_original_chunk_identity() {
        let root = root("daily-source-chunks");
        write(
            &root,
            "chronicle/20260910/talents/Followups.jsonl",
            "{\"ts\":100,\"summary\":\"first\"}\n{\"ts\":200,\"summary\":\"second\"}\n",
        );
        let chunks = capture_day_sources(&root, "20260910").unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].agent, "followups");
        assert_eq!(chunks[0].idx, 1);
        assert_eq!(chunks[0].path, "20260910/talents/Followups.jsonl");
        write(
            &root,
            "chronicle/20260910/import.ics/imported.jsonl",
            "{\"import\":{\"source\":\"ICS\"}}\n{\"type\":\"calendar_event\",\"title\":\"planning\"}\n",
        );
        assert!(
            capture_day_sources(&root, "20260910")
                .unwrap()
                .iter()
                .any(|c| c.agent == "import.ics")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn evidence_retains_long_transcript_lines_eliminated_by_search_formatting() {
        let root = root("daily-lossless-transcript");
        let path = "chronicle/20260910/mic/090000_60/audio_transcript.md";
        let prefix = "context ".repeat(400);
        write(&root, path, &format!("{prefix}Meeting is on Monday."));
        let projected = capture_day_sources(&root, "20260910").unwrap();
        assert!(
            projected.is_empty(),
            "positive control: search formatter must omit the long line"
        );
        let before = revision(&root, "schedule", None);
        write(&root, path, &format!("{prefix}Meeting is on Friday."));
        assert_eq!(projected, capture_day_sources(&root, "20260910").unwrap());
        assert_ne!(before, revision(&root, "schedule", None));
        let report = capture_day_projection(&root, "20260910").unwrap();
        assert_eq!(report.sources.len(), 1);
        assert!(
            !report.sources[0].warnings.is_empty(),
            "omitted chunks must retain their diagnostic"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn evidence_retains_unrendered_activity_fields_and_semantic_percept_headers() {
        let root = root("daily-lossless-structured");
        let path = "facets/work/activities/20260910.jsonl";
        let mut row = json!({"id":"meeting", "source":"user", "title":"Planning", "description":"Weekly planning", "decisions":["ship Monday"], "start":"09:00:00"});
        write(&root, path, &row.to_string());
        let projected = capture_day_sources(&root, "20260910").unwrap();
        let before = revision(&root, "facet_newsletter", Some("work"));
        row["decisions"] = json!(["ship Friday"]);
        row["start"] = json!("11:00:00");
        write(&root, path, &row.to_string());
        assert_eq!(projected, capture_day_sources(&root, "20260910").unwrap());
        assert_ne!(before, revision(&root, "facet_newsletter", Some("work")));

        let audio = "chronicle/20260910/mic/090000_60/audio.jsonl";
        write(
            &root,
            audio,
            "{\"imported\":{\"facet\":\"work\",\"id\":\"one\"},\"_solstone_processing\":{\"state\":\"first\"}}\n{\"start\":\"00:00:00\",\"text\":\"A factual statement.\"}\n",
        );
        let before = revision(&root, "schedule", None);
        write(
            &root,
            audio,
            "{\"imported\":{\"facet\":\"work\",\"id\":\"one\"},\"_solstone_processing\":{\"state\":\"second\"}}\n{\"start\":\"00:00:00\",\"text\":\"A factual statement.\"}\n",
        );
        assert_eq!(before, revision(&root, "schedule", None));
        write(
            &root,
            audio,
            "{\"imported\":{\"facet\":\"personal\",\"id\":\"one\"}}\n{\"start\":\"00:00:00\",\"text\":\"A factual statement.\"}\n",
        );
        assert_ne!(before, revision(&root, "schedule", None));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn daily_evidence_is_copy_stable_and_ignores_generated_activity_and_memory() {
        let a = root("daily-copy-a");
        let b = root("daily-copy-b");
        for root in [&a, &b] {
            write(root, "facets/work/facet.json", "{\"title\":\"Work\"}");
            write(
                root,
                "facets/work/entities/20260910.jsonl",
                "{\"name\":\"Alice\",\"description\":\"Engineer\"}\n",
            );
        }
        let before = revision(&a, "entities:entity_observer", Some("work"));
        assert_eq!(
            before,
            revision(&b, "entities:entity_observer", Some("work"))
        );
        write(
            &a,
            "facets/work/activities/20260910.jsonl",
            "{\"title\":\"Generated\",\"source\":\"anticipated\"}\n",
        );
        write(
            &a,
            "facets/work/entities/alice/observations.jsonl",
            "{\"content\":\"new memory\"}\n",
        );
        write(
            &a,
            "chronicle/20260910/talents/morning_briefing.json",
            "{\"body\":\"output\"}",
        );
        assert_eq!(
            before,
            revision(&a, "entities:entity_observer", Some("work"))
        );
        write(
            &a,
            "facets/work/activities/20260910.jsonl",
            "{\"title\":\"Actual\",\"source\":\"user\"}\n",
        );
        assert_ne!(
            before,
            revision(&a, "entities:entity_observer", Some("work"))
        );
        fs::remove_dir_all(a).unwrap();
        fs::remove_dir_all(b).unwrap();
    }
    #[test]
    fn daily_evidence_review_reads_actual_previous_seven_days_and_rejects_corruption() {
        let root = root("daily-review-window");
        let before = revision(&root, "entities:entities_review", Some("work"));
        write(
            &root,
            "facets/work/entities/20260903.jsonl",
            "{\"name\":\"Alice\",\"description\":\"Engineer\"}\n",
        );
        let after = revision(&root, "entities:entities_review", Some("work"));
        assert_ne!(before, after);
        write(
            &root,
            "facets/work/entities/20260902.jsonl",
            "{\"name\":\"Outside\"}\n",
        );
        assert_eq!(
            after,
            revision(&root, "entities:entities_review", Some("work"))
        );
        write(&root, "facets/work/entities/20260909.jsonl", "not json");
        assert!(
            compute_daily_evidence_revision(
                &root,
                "20260910",
                "entities:entities_review",
                &Map::from_iter([(
                    "hook".to_owned(),
                    json!({"pre":"entities:entities_review","post":"entities:entities_review"})
                )]),
                "prompt",
                Some("work"),
                None
            )
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn daily_contract_uses_schema_contents_and_effective_override() {
        let root = root("daily-contract");
        write(
            &root,
            "talent/schedule.schema.json",
            "{\"type\":\"object\"}",
        );
        let metadata = Map::from_iter([
            ("path".to_owned(), json!(root.join("talent/schedule.md"))),
            ("schema".to_owned(), json!("schedule.schema.json")),
            ("hook".to_owned(), json!({"post":"schedule"})),
            ("max_output_tokens".to_owned(), json!(1024)),
        ]);
        let first = compute_contract_digest(&root, "schedule", &metadata, "prompt", None).unwrap();
        let overrides = Map::from_iter([(
            "talent.system.schedule".to_owned(),
            json!({"max_output_tokens":1024}),
        )]);
        assert_eq!(
            first,
            compute_contract_digest(&root, "schedule", &metadata, "prompt", Some(&overrides))
                .unwrap()
        );
        write(&root, "talent/schedule.schema.json", "{\"type\":\"array\"}");
        assert_ne!(
            first,
            compute_contract_digest(&root, "schedule", &metadata, "prompt", None).unwrap()
        );
        write(&root, "talent/schedule.schema.json", "malformed");
        assert!(compute_contract_digest(&root, "schedule", &metadata, "prompt", None).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn daily_maintenance_shares_calendar_window_and_observes_named_segment_topology() {
        let root = root("daily-maintenance-window");
        let anchor = journal_today(&root).unwrap();
        let today = anchor.format("%Y%m%d").to_string();
        let old = (anchor - Duration::days(7)).format("%Y%m%d").to_string();
        let future = (anchor + Duration::days(1)).format("%Y%m%d").to_string();
        let meta = Map::from_iter([
            ("meta".to_owned(), json!({"lookback_days":7})),
            (
                "hook".to_owned(),
                json!({"pre":"daily_schedule","post":"daily_schedule"}),
            ),
        ]);
        let get = |day| {
            compute_daily_evidence_revision(
                &root,
                day,
                "daily_schedule",
                &meta,
                "prompt",
                None,
                None,
            )
            .unwrap()
            .0
        };
        let first = get("20260101");
        assert_eq!(first, get("20250101"));
        write(
            &root,
            &format!("chronicle/{old}/mic/090000_60/audio.jsonl"),
            "{}",
        );
        write(
            &root,
            &format!("chronicle/{future}/mic/090000_60/audio.jsonl"),
            "{}",
        );
        write(
            &root,
            &format!("chronicle/{today}/health/status.json"),
            "{}",
        );
        assert_eq!(first, get("20250101"));
        write(
            &root,
            &format!("chronicle/{today}/mic/090000_60/audio.jsonl"),
            "{}",
        );
        assert_ne!(first, get("20250101"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn daily_contract_changes_for_brain_choice_but_not_runtime_incarnation() {
        let root = root("daily-brain-contract");
        write(
            &root,
            "config/journal.json",
            r#"{"providers":{"active":{"provider":"local","model":"m1","pid":1}}}"#,
        );
        let first = revision(&root, "schedule", None);
        write(
            &root,
            "config/journal.json",
            r#"{"providers":{"active":{"provider":"local","model":"m1","pid":2}}}"#,
        );
        assert_eq!(first, revision(&root, "schedule", None));
        write(
            &root,
            "config/journal.json",
            r#"{"providers":{"active":{"provider":"local","model":"m2","pid":2}}}"#,
        );
        assert_ne!(first, revision(&root, "schedule", None));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn daily_contract_refuses_missing_unknown_or_changed_hook_phases() {
        assert!(daily_hook("schedule", &Map::new()).is_err());
        assert!(
            daily_hook(
                "schedule",
                &Map::from_iter([("hook".to_owned(), json!({"pre":"schedule"}))])
            )
            .is_err()
        );
        assert!(
            daily_hook(
                "schedule",
                &Map::from_iter([("hook".to_owned(), json!({"post":"unknown"}))])
            )
            .is_err()
        );
    }

    // --- one projection per day, shared by the units of a coverage read ---------

    const DAY: &str = "20260910";
    const UNITS: [(&str, Option<&str>); 7] = [
        ("schedule", None),
        ("facet_newsletter", Some("work")),
        ("facet_newsletter", Some("home")),
        ("entities:entity_observer", Some("work")),
        ("entities:entity_suggest", Some("work")),
        ("entities:entities_review", Some("work")),
        ("morning_briefing", None),
    ];

    /// A day with sources on the day itself, activities on later days (before today) and
    /// entities inside the review window, so every offset route has data.
    fn shared_day(name: &str) -> PathBuf {
        let root = root(name);
        write(
            &root,
            "chronicle/20260910/mic/090000_60/talents/audio.md",
            "# Named\n\nA useful fact.",
        );
        write(
            &root,
            "chronicle/20260910/100000_60/_transcript.md",
            "# Direct\n\nAnother fact.",
        );
        let row = |t: &str| {
            json!({"id":"meeting","source":"user","title":t,"description":"d","start":"09:00:00"})
                .to_string()
        };
        write(
            &root,
            "facets/work/activities/20260910.jsonl",
            &row("Planning"),
        );
        write(
            &root,
            "facets/home/activities/20260910.jsonl",
            &row("Chores"),
        );
        write(
            &root,
            "facets/work/activities/20260911.jsonl",
            &row("Next day"),
        );
        write(
            &root,
            "facets/work/activities/20260913.jsonl",
            &row("Three days later"),
        );
        write(
            &root,
            "facets/work/entities/20260909.jsonl",
            "{\"name\":\"Alice\",\"description\":\"Engineer\"}\n",
        );
        write(
            &root,
            "facets/work/entities/20260904.jsonl",
            "{\"name\":\"Bob\",\"description\":\"Designer\"}\n",
        );
        root
    }

    fn unit_metadata(name: &str) -> Map<String, Value> {
        let hook = match name {
            "schedule" => json!({"post":name}),
            "morning_briefing" => json!({"pre":name}),
            _ => json!({"pre":name,"post":name}),
        };
        Map::from_iter([("hook".to_owned(), hook)])
    }

    fn cached(
        root: &Path,
        day: &str,
        name: &str,
        facet: Option<&str>,
        cache: &mut DayProjectionCache,
    ) -> Result<(String, String), String> {
        compute_daily_evidence_revision_cached(
            root,
            day,
            name,
            &unit_metadata(name),
            "prompt",
            facet,
            None,
            cache,
        )
    }

    fn uncached(
        root: &Path,
        day: &str,
        name: &str,
        facet: Option<&str>,
    ) -> Result<(String, String), String> {
        compute_daily_evidence_revision(
            root,
            day,
            name,
            &unit_metadata(name),
            "prompt",
            facet,
            None,
        )
    }

    #[test]
    fn shared_projection_gives_every_unit_the_revision_it_computed_alone_from_one_capture() {
        let root = shared_day("daily-shared-projection");
        let mut cache = DayProjectionCache::new(&root, DAY);
        let mut revisions = std::collections::BTreeSet::new();
        for (name, facet) in UNITS {
            let shared = cached(&root, DAY, name, facet, &mut cache).unwrap();
            assert_eq!(
                shared,
                uncached(&root, DAY, name, facet).unwrap(),
                "{name} {facet:?}"
            );
            revisions.insert(shared.0);
        }
        assert_eq!(
            revisions.len(),
            UNITS.len(),
            "the fixture must tell the units apart"
        );
        // Every unit but the review reads the day's own projection; the review reads facet entities.
        let projecting = UNITS
            .iter()
            .filter(|(name, _)| *name != "entities:entities_review")
            .count();
        assert_eq!(cache.requests() as usize, projecting);
        assert_eq!(
            cache.captures(),
            1,
            "the day is captured once, not once per unit"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_new_cache_sees_a_changed_source_exactly_as_an_uncached_computation_does() {
        let root = shared_day("daily-shared-projection-fresh");
        let first = cached(
            &root,
            DAY,
            "facet_newsletter",
            Some("work"),
            &mut DayProjectionCache::new(&root, DAY),
        )
        .unwrap();
        write(
            &root,
            "chronicle/20260910/100000_60/_transcript.md",
            "# Direct\n\nA changed fact.",
        );
        let second = cached(
            &root,
            DAY,
            "facet_newsletter",
            Some("work"),
            &mut DayProjectionCache::new(&root, DAY),
        )
        .unwrap();
        assert_ne!(first, second);
        assert_eq!(
            second,
            uncached(&root, DAY, "facet_newsletter", Some("work")).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn units_that_never_read_the_projection_never_capture_it_or_see_its_failure() {
        let root = shared_day("daily-shared-projection-lazy");
        let mut cache = DayProjectionCache::new(&root, DAY);
        cached(
            &root,
            DAY,
            "entities:entities_review",
            Some("work"),
            &mut cache,
        )
        .unwrap();
        assert_eq!((cache.captures(), cache.requests()), (0, 0));
        // A source that makes the whole-day projection fail.
        write(&root, "facets/work/activities/20260910.jsonl", "not json");
        let failure = capture_day_projection(&root, DAY).unwrap_err();
        let mut cache = DayProjectionCache::new(&root, DAY);
        assert_eq!(
            cached(
                &root,
                DAY,
                "entities:entities_review",
                Some("work"),
                &mut cache
            ),
            uncached(&root, DAY, "entities:entities_review", Some("work")),
        );
        assert!(
            cached(
                &root,
                DAY,
                "entities:entities_review",
                Some("work"),
                &mut cache
            )
            .is_ok()
        );
        assert_eq!((cache.captures(), cache.requests()), (0, 0));
        // A unit that does read the projection gets the same error as its own capture.
        assert_eq!(
            cached(&root, DAY, "facet_newsletter", Some("work"), &mut cache).unwrap_err(),
            failure
        );
        assert_eq!(
            cached(&root, DAY, "schedule", None, &mut cache).unwrap_err(),
            failure
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_cache_for_another_day_or_journal_never_supplies_the_result() {
        let root = shared_day("daily-shared-projection-wrong-day");
        let other = shared_day("daily-shared-projection-other-journal");
        write(
            &other,
            "chronicle/20260910/100000_60/_transcript.md",
            "# Direct\n\nDifferent.",
        );
        let expected = uncached(&root, DAY, "facet_newsletter", Some("work")).unwrap();
        for mut cache in [
            DayProjectionCache::new(&root, "20260909"),
            DayProjectionCache::new(&other, DAY),
        ] {
            assert_eq!(
                cached(&root, DAY, "facet_newsletter", Some("work"), &mut cache).unwrap(),
                expected
            );
            assert_eq!(cache.requests(), 0, "a mismatched cache is never asked");
            assert_eq!(cache.captures(), 1, "the direct capture is counted");
        }
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(other).unwrap();
    }
}
