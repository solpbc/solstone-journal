// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only daily source projection. This is independent of the search database.

use chrono::{Duration, NaiveDate};
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

/// Part of every daily contract.  ⚠ Bumping it re-derives only today and the
/// seven closed days before it (older days keep their output and read as made
/// with an earlier version); re-deriving further back is `--from-scratch`.
pub const SEMANTIC_EVIDENCE_VERSION: &str = "daily-sources-3";
const OUTPUTS: &[&str] = &[
    // Retired; outputs it already wrote are still not evidence.
    "schedule",
    "daily_schedule",
    "morning_briefing",
    "entities_review",
    "entity_suggest",
    "entity_observer",
    "_entities_entities_review",
    "_entities_entity_suggest",
    "_entities_entity_observer",
    "pulse",
    "topics",
    "objects",
    "screen_text",
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

/// Day documents the journal derives for search, at `chronicle/<day>/<name>`.
///
/// These files stay excluded from daily evidence. Admitting a new source into
/// the evidence value changes `revision_under` on every day that source touches,
/// so `EarlierContract` does not keep those days. The seven-day window applies
/// only when evidence still matches under the accepted contract.
const DERIVED_DAY_DOCUMENTS: &[&str] = &["location.md"];

/// A supplemental source's immutable contract policy. Mutable source state
/// belongs only to unfinished-packet freshness, never semantic evidence.
#[derive(Debug)]
pub struct DailyInput {
    pub id: &'static str,
    pub version: &'static str,
    pub recipients: &'static [&'static str],
    pub lookback_closed_days: i64,
}

pub const DAILY_INPUTS: &[DailyInput] = &[DailyInput {
    id: "location",
    version: "location-words-1",
    lookback_closed_days: 7,
    recipients: &[
        "entities:entity_suggest",
        "entities:entity_observer",
        "morning_briefing",
    ],
}];

impl DailyInput {
    pub fn applies(
        &self,
        hook: &str,
        day: &str,
        config: &Map<String, Value>,
    ) -> Result<bool, String> {
        if !self.recipients.contains(&hook) {
            return Ok(false);
        }
        let cutoff = solstone_core_journal_config::daily_input_since(config, self.id, self.version)
            .map_err(|error| error.to_string())?;
        Ok(self.version_since(hook, day, cutoff).is_some())
    }
    fn version_since(
        &self,
        hook: &str,
        day: &str,
        cutoff: Option<NaiveDate>,
    ) -> Option<&'static str> {
        let cutoff = cutoff?;
        let day = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
        (self.recipients.contains(&hook) && day >= cutoff).then_some(self.version)
    }
}

/// Registered providers admitted to a recipient's contract on this day.
pub fn daily_input_contract(
    hook: &str,
    day: &str,
    config: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let mut admitted = Map::new();
    for input in DAILY_INPUTS {
        if input.applies(hook, day, config)? {
            admitted.insert(input.id.to_owned(), Value::String(input.version.to_owned()));
        }
    }
    Ok(admitted)
}

fn output_path(rel: &str) -> bool {
    let parts = rel.split('/').collect::<Vec<_>>();
    if let [day, name] = parts.as_slice()
        && day.len() == 8
        && day.bytes().all(|b| b.is_ascii_digit())
        && DERIVED_DAY_DOCUMENTS.contains(name)
    {
        return true;
    }
    let stem = Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    parts.contains(&"talents") && OUTPUTS.contains(&stem)
}

/// Canonical chunks for one day, in path/original-chunk order.
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
        // Calendar notes are not something the owner did; no daily unit reads them.
        if resolution == ContentResolution::Indexed(Family::Calendar) {
            continue;
        }
        let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if resolution == ContentResolution::Unrecognized
            && (filename.ends_with("_transcript.md") || filename == "imported.md")
        {
            resolution = ContentResolution::Indexed(Family::Markdown);
        }
        let path_selected_family = matches!(
            resolution,
            ContentResolution::Indexed(Family::ProtectedVideo | Family::DesktopActivity)
        );
        // A written shape is an input contract: malformed/unreadable sidecars cannot mean empty evidence.
        let sidecar = path.with_file_name(content::SHAPE_SIDECAR_BASENAME);
        match fs::read_to_string(&sidecar) {
            Ok(text) => {
                let shape: Value = serde_json::from_str(&text)
                    .map_err(|e| format!("{}: {e}", sidecar.display()))?;
                let shape = shape.as_object().ok_or("shape sidecar is not an object")?;
                if !path_selected_family && let Some(value) = shape.get(filename) {
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
        let mut text = if path_selected_family {
            let raw = fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            raw.lines()
                .filter(|line| {
                    let trimmed = line.trim();
                    !trimmed.is_empty()
                        && serde_json::from_str::<Value>(trimmed)
                            .ok()
                            .is_some_and(|v| v.is_object())
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            strict_text(&path)?
        };
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
    // Path, then chunk order. Occurrence times mix two clocks across source
    // families (true epoch, and local wall time stored as if it were UTC), so
    // they cannot order chunks from different sources.
    chunks.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.idx.cmp(&b.idx)));
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
        "daily_schedule"
            | "morning_briefing"
            | "entities:entities_review"
            | "entities:entity_suggest"
            | "entities:entity_observer"
    ) || pre.zip(post).is_some_and(|(a, b)| a != b)
    {
        return Err(format!("unsupported daily hook: {effective}"));
    }
    let phases_match = match effective {
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
    daily_inputs: Option<&Map<String, Value>>,
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
        for field in ["disabled"] {
            if let Some(v) = values.get(field) {
                effective.insert(field.to_owned(), v.clone());
            }
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
    // The output ceiling is not part of what a day's result means: it is measured
    // above every natural completion, so retuning it must not re-owe finished days.
    for key in [
        "path",
        "file",
        "mtime",
        "description",
        "label",
        "group",
        "priority",
        "max_output_tokens",
        "thinking_budget",
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
    let mut contract = json!({"version":SEMANTIC_EVIDENCE_VERSION,"name":name,"metadata":effective,"body":body.trim(),"templates":templates,"facets":facets,"brain":brain});
    if let Some(inputs) = daily_inputs.filter(|inputs| !inputs.is_empty()) {
        contract
            .as_object_mut()
            .expect("contract digest is an object")
            .insert("daily_inputs".to_owned(), Value::Object(inputs.clone()));
    }
    Ok(digest(&contract))
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
    compute_daily_evidence_cached(journal, day, name, metadata, body, facet, overrides, cache)
        .map(|evidence| (evidence.revision, evidence.contract))
}

/// One unit's revision, its contract, and the evidence the revision was made from.
///
/// The evidence is kept so a caller can ask whether an accepted result was made
/// from this same evidence under an older contract ([`Self::revision_under`]),
/// without storing anything new in the unit record.
#[derive(Clone, Debug)]
pub struct DailyEvidence {
    pub revision: String,
    pub contract: String,
    evidence: Value,
}

impl DailyEvidence {
    /// The revision this evidence would have had under `contract`.
    pub fn revision_under(&self, contract: &str) -> String {
        revision_digest(contract, &self.evidence)
    }

    /// Retry permission follows source changes, not a maintenance window's
    /// calendar anchor or newly named empty days. Ordinary acceptance retains
    /// the full calendar evidence above.
    pub fn retry_revision_under(&self, contract: &str) -> String {
        let Some(windows) = self.evidence.get("windows").and_then(Value::as_object) else {
            return self.revision_under(contract);
        };
        let sources = windows
            .iter()
            .filter(|(_, entries)| entries.as_array().is_none_or(|entries| !entries.is_empty()))
            .collect::<BTreeMap<_, _>>();
        revision_digest(contract, &json!({"windows": sources}))
    }
}

fn revision_digest(contract: &str, evidence: &Value) -> String {
    digest(&json!({"contract":contract,"evidence":evidence}))
}

/// Uncached [`compute_daily_evidence_cached`].
pub fn compute_daily_evidence(
    journal: &Path,
    day: &str,
    name: &str,
    metadata: &Map<String, Value>,
    body: &str,
    facet: Option<&str>,
    overrides: Option<&Map<String, Value>>,
) -> Result<DailyEvidence, String> {
    compute_daily_evidence_cached(
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

/// [`compute_daily_evidence_revision_cached`], keeping the evidence it hashed.
#[allow(clippy::too_many_arguments)] // The unit identity and its cache travel together.
pub fn compute_daily_evidence_cached(
    journal: &Path,
    day: &str,
    name: &str,
    metadata: &Map<String, Value>,
    body: &str,
    facet: Option<&str>,
    overrides: Option<&Map<String, Value>>,
    cache: &mut DayProjectionCache,
) -> Result<DailyEvidence, String> {
    let hook = daily_hook(name, metadata)?;
    let configured = solstone_core_journal_config::read_journal_config(journal)
        .map_err(|error| error.to_string())?
        .config
        .unwrap_or_default();
    let inputs = daily_input_contract(&hook, day, &configured)?;
    let contract =
        compute_contract_digest(journal, name, metadata, body, overrides, Some(&inputs))?;
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|e| e.to_string())?;
    let evidence = if hook == "daily_schedule" {
        let anchor = journal_today(journal);
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
            // The schedule reads segment times as when the owner was active,
            // which a calendar note is not.
            let keys = iter_segments(journal, PathOrDay::Day(&d))
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|s| !solstone_core_segment::is_calendar_segment(journal, s))
                .map(|s| s.key().to_owned())
                .collect::<Vec<_>>();
            windows.insert(d, keys);
        }
        json!({"anchor":anchor.to_string(),"windows":windows})
    } else {
        // ⛔ No forward window for the briefing.  Its packet reads nothing from
        // the days after it, while a forward read of the activity files hashes
        // rows the packet never reads: every closed day then re-owed the eight
        // briefings before it.  A briefing is
        // generated the night its day closes, when every later day is still
        // today or the future, and it is frozen once its morning has passed
        // (`solstone_core_system::daily_coverage::accepted_reuse`).
        let mut sources = Vec::new();
        for offset in evidence_day_offsets(&hook) {
            let d = (date + Duration::days(offset)).format("%Y%m%d").to_string();
            sources.extend(if hook == "entities:entities_review" {
                capture_facet_day_sources(journal, &d, "entities", facet)?.sources
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
        json!({"day":day,"facet":facet,"sources":sources,"suggestions_digest":suggestions_digest})
    };
    Ok(DailyEvidence {
        revision: revision_digest(&contract, &evidence),
        contract,
        evidence,
    })
}

/// The days, relative to its own, whose sources a daily unit's evidence reads.
/// The entity review reads only the seven days before its day, so its evidence
/// changes when those days do, never with its own.
pub fn evidence_day_offsets(hook: &str) -> Vec<i64> {
    match hook {
        "entities:entities_review" => (-7..=-1).collect(),
        _ => vec![0],
    }
}

/// Today in the journal's owner zone.
pub fn journal_today(journal: &Path) -> NaiveDate {
    chrono::Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .date_naive()
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
        evidence_of(root, "20260910", name, facet).revision
    }

    fn metadata_for(name: &str) -> Map<String, Value> {
        let hook = match name {
            "morning_briefing" => json!({"pre":name}),
            _ => json!({"pre":name,"post":name}),
        };
        let mut metadata = Map::from_iter([("hook".to_owned(), hook)]);
        if name == "daily_schedule" {
            metadata.insert("meta".to_owned(), json!({"lookback_days":7}));
        }
        metadata
    }

    fn evidence_of(root: &Path, day: &str, name: &str, facet: Option<&str>) -> DailyEvidence {
        compute_daily_evidence(root, day, name, &metadata_for(name), "prompt", facet, None)
            .unwrap_or_else(|error| panic!("{name} {day}: {error}"))
    }

    fn write_config(root: &Path, value: &Value) {
        write(root, "config/journal.json", &value.to_string());
    }

    /// The briefing's revision used to hash the observed activity of the eight
    /// days after it, which its packet never reads, so every closed day
    /// re-owed the eight briefings before it.  Later days no longer move it,
    /// and a damaged later day no longer fails it.
    #[test]
    fn a_later_days_activity_does_not_move_a_briefings_revision() {
        let root = root("briefing-no-forward-window");
        write(
            &root,
            "chronicle/20260910/talents/flow.md",
            "# Flow\nMeeting.",
        );
        write(&root, "facets/work/facet.json", r#"{"title":"Work"}"#);
        let before = revision(&root, "morning_briefing", None);
        write(
            &root,
            "facets/work/activities/20260911.jsonl",
            "{\"id\":\"a\",\"title\":\"Observed\",\"source\":\"observed\"}\n",
        );
        write(
            &root,
            "facets/work/activities/20260913.jsonl",
            "{\"truncated",
        );
        assert_eq!(revision(&root, "morning_briefing", None), before);
        // Control: the day's own evidence still moves it.
        write(
            &root,
            "chronicle/20260910/talents/flow.md",
            "# Flow\nAnother meeting.",
        );
        assert_ne!(revision(&root, "morning_briefing", None), before);
    }

    #[test]
    fn a_revision_can_be_recomputed_under_an_older_contract() {
        let root = root("revision-under-contract");
        write(
            &root,
            "chronicle/20260910/talents/flow.md",
            "# Flow\nMeeting.",
        );
        let metadata = Map::from_iter([("hook".to_owned(), json!({"pre":"morning_briefing"}))]);
        let old = compute_daily_evidence(
            &root,
            "20260910",
            "morning_briefing",
            &metadata,
            "old prompt",
            None,
            None,
        )
        .unwrap();
        let new = compute_daily_evidence(
            &root,
            "20260910",
            "morning_briefing",
            &metadata,
            "new prompt",
            None,
            None,
        )
        .unwrap();
        assert_ne!(old.contract, new.contract);
        assert_ne!(old.revision, new.revision);
        assert_eq!(new.revision_under(&old.contract), old.revision);
        write(
            &root,
            "chronicle/20260910/talents/flow.md",
            "# Flow\nChanged.",
        );
        let changed = compute_daily_evidence(
            &root,
            "20260910",
            "morning_briefing",
            &metadata,
            "new prompt",
            None,
            None,
        )
        .unwrap();
        assert_ne!(changed.revision_under(&old.contract), old.revision);
    }

    /// 2.0.42's first start wrote `location.md` into every past day holding phone
    /// location, and every such day's outputs read as owed by changed evidence.
    #[test]
    fn the_derived_location_document_is_not_daily_evidence() {
        let root = root("derived-location-document");
        write(
            &root,
            "chronicle/20260910/talents/flow.md",
            "# Flow\nMeeting.",
        );
        let names = ["morning_briefing", "entities:entity_observer"];
        let before = names.map(|name| revision(&root, name, Some("work")));
        for text in [
            "# location on Thursday, September 10, 2026\n\n- readings near Golden, 14:00 to 14:09: 3 readings from your iphone.\n",
            "# location on Thursday, September 10, 2026\n\n- a visit your iphone reported at a place you named.\n",
        ] {
            write(&root, "chronicle/20260910/location.md", text);
            assert_eq!(
                names.map(|name| revision(&root, name, Some("work"))),
                before
            );
        }
        assert!(
            capture_day_sources(&root, "20260910")
                .unwrap()
                .iter()
                .all(|chunk| chunk.path != "20260910/location.md")
        );
        write_config(
            &root,
            &json!({"identity":{"timezone":"UTC"},"daily_inputs":{"location":{"location-words-1":"20260901"}}}),
        );
        let activated = names.map(|name| evidence_of(&root, "20260910", name, Some("work")));
        for text in [
            "# location on Thursday, September 10, 2026\n\n- readings near Golden, 14:00 to 14:09: 3 readings from your iphone.\n",
            "# location on Thursday, September 10, 2026\n\n- a visit your iphone reported at a place you named.\n",
        ] {
            write(&root, "chronicle/20260910/location.md", text);
            let next = names.map(|name| evidence_of(&root, "20260910", name, Some("work")));
            for (index, name) in names.iter().enumerate() {
                assert_eq!(next[index].evidence, activated[index].evidence, "{name}");
                assert_eq!(next[index].contract, activated[index].contract, "{name}");
                assert!(
                    !next[index].evidence.to_string().contains("location.md"),
                    "{name}"
                );
            }
        }
        // Control: the day's own material still moves every revision.
        write(
            &root,
            "chronicle/20260910/talents/flow.md",
            "# Flow\nAnother meeting.",
        );
        for (name, before) in names.iter().zip(before) {
            assert_ne!(revision(&root, name, Some("work")), before, "{name}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn calendar_notes_never_move_a_daily_units_evidence() {
        let root = root("calendar-not-daily-evidence");
        let today = journal_today(&root).format("%Y%m%d").to_string();
        write(
            &root,
            "streams/phone_calendar.json",
            r#"{"name":"phone_calendar","kind":"observer","host":null,"platform":null,"created_at":0,"last_day":null,"last_segment":null,"seq":1,"cid":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","source":"calendar"}"#,
        );
        write(
            &root,
            &format!("chronicle/{today}/talents/flow.md"),
            "# Flow\nMeeting.",
        );
        let names = [
            "morning_briefing",
            "entities:entity_observer",
            "daily_schedule",
        ];
        let before = names.map(|name| evidence_of(&root, &today, name, Some("work")).revision);
        write(
            &root,
            &format!(
                "chronicle/{today}/phone_calendar/094000_300/calendar_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.jsonl"
            ),
            include_str!("../../../fixtures/calendar_notes/golden.jsonl"),
        );
        let with_notes = names.map(|name| evidence_of(&root, &today, name, Some("work")).revision);
        assert_eq!(with_notes, before);
        assert!(
            capture_day_sources(&root, &today)
                .unwrap()
                .iter()
                .all(|chunk| !chunk.path.contains("calendar_"))
        );
        // The twin: an ordinary capture segment moves every revision.
        write(
            &root,
            &format!("chronicle/{today}/mic/100000_300/audio.jsonl"),
            r#"{"start":"00:00:01","text":"hello there"}"#,
        );
        let with_capture =
            names.map(|name| evidence_of(&root, &today, name, Some("work")).revision);
        for (index, name) in names.iter().enumerate() {
            assert_ne!(with_capture[index], before[index], "{name}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_imported_calendar_s_notes_never_move_a_daily_units_evidence() {
        let root = root("calendar-import-not-daily-evidence");
        let today = journal_today(&root).format("%Y%m%d").to_string();
        let window = format!("chronicle/{today}/import.ics/094130_301");
        write(
            &root,
            &format!("{window}/event_transcript.md"),
            "## planning\nThe garden beds.\n",
        );
        let names = [
            "morning_briefing",
            "entities:entity_observer",
            "daily_schedule",
        ];
        let before = names.map(|name| evidence_of(&root, &today, name, Some("work")).revision);
        write(
            &root,
            &format!("{window}/calendar_{}.jsonl", "b".repeat(64)),
            include_str!("../../../fixtures/calendar_notes/golden.jsonl"),
        );
        let with_notes = names.map(|name| evidence_of(&root, &today, name, Some("work")).revision);
        assert_eq!(with_notes, before);
        // The twin: a changed transcript in the same window moves every unit
        // that reads it; the schedule reads only segment times.
        write(
            &root,
            &format!("{window}/event_transcript.md"),
            "## planning\nThe garden beds, moved.\n",
        );
        let with_transcript =
            names.map(|name| evidence_of(&root, &today, name, Some("work")).revision);
        for (index, name) in names[..2].iter().enumerate() {
            assert_ne!(with_transcript[index], before[index], "{name}");
        }
        fs::remove_dir_all(root).unwrap();
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
        let mut previous = revision(&root, "morning_briefing", None);
        for (rel, text) in files {
            write(&root, rel, text);
            let next = revision(&root, "morning_briefing", None);
            assert_ne!(previous, next, "{rel}");
            previous = next;
        }
        fs::remove_file(root.join(files[0].0)).unwrap();
        assert_ne!(previous, revision(&root, "morning_briefing", None));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_strava_workout_is_daily_evidence_for_its_day_once() {
        let root = root("daily-sources-strava");
        for (key, index) in [("070200_300", 0), ("070700_300", 1)] {
            write(
                &root,
                &format!("chronicle/20260910/import.strava/{key}/workout.json"),
                &serde_json::json!({
                    "activity_id": 9,
                    "tile": {"index": index, "count": 2},
                    "workout": {"name": "Morning Run", "type": "Run", "distance_m": 10200.0}
                })
                .to_string(),
            );
        }
        let sources = capture_day_sources(&root, "20260910").unwrap();
        let workouts = sources
            .iter()
            .filter(|chunk| chunk.text.contains("## Strava workout: Morning Run"))
            .collect::<Vec<_>>();
        assert_eq!(workouts.len(), 1);
        assert_eq!(workouts[0].agent, "import.strava");
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
        assert_eq!(chunks[0].idx, 0);
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
        let before = revision(&root, "morning_briefing", None);
        write(&root, path, &format!("{prefix}Meeting is on Friday."));
        assert_eq!(projected, capture_day_sources(&root, "20260910").unwrap());
        assert_ne!(before, revision(&root, "morning_briefing", None));
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
        let before = revision(&root, "entities:entity_suggest", Some("work"));
        row["source"] = json!("import");
        row["start"] = json!("11:00:00");
        write(&root, path, &row.to_string());
        assert_eq!(projected, capture_day_sources(&root, "20260910").unwrap());
        assert_ne!(
            before,
            revision(&root, "entities:entity_suggest", Some("work"))
        );

        let audio = "chronicle/20260910/mic/090000_60/audio.jsonl";
        write(
            &root,
            audio,
            "{\"imported\":{\"facet\":\"work\",\"id\":\"one\"},\"_solstone_processing\":{\"state\":\"first\"}}\n{\"start\":\"00:00:00\",\"text\":\"A factual statement.\"}\n",
        );
        let before = revision(&root, "morning_briefing", None);
        write(
            &root,
            audio,
            "{\"imported\":{\"facet\":\"work\",\"id\":\"one\"},\"_solstone_processing\":{\"state\":\"second\"}}\n{\"start\":\"00:00:00\",\"text\":\"A factual statement.\"}\n",
        );
        assert_eq!(before, revision(&root, "morning_briefing", None));
        write(
            &root,
            audio,
            "{\"imported\":{\"facet\":\"personal\",\"id\":\"one\"}}\n{\"start\":\"00:00:00\",\"text\":\"A factual statement.\"}\n",
        );
        assert_ne!(before, revision(&root, "morning_briefing", None));
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
    fn daily_contract_uses_schema_contents_and_ignores_the_output_ceiling() {
        let root = root("daily-contract");
        write(
            &root,
            "talent/morning_briefing.schema.json",
            "{\"type\":\"object\"}",
        );
        let metadata = Map::from_iter([
            (
                "path".to_owned(),
                json!(root.join("talent/morning_briefing.md")),
            ),
            ("schema".to_owned(), json!("morning_briefing.schema.json")),
            ("hook".to_owned(), json!({"pre":"morning_briefing"})),
            ("max_output_tokens".to_owned(), json!(1024)),
        ]);
        let first =
            compute_contract_digest(&root, "morning_briefing", &metadata, "prompt", None, None)
                .unwrap();
        // Retuning a talent's ceiling, or an old owner override of it, re-owes nothing.
        let mut retuned = metadata.clone();
        retuned.insert("max_output_tokens".to_owned(), json!(4096));
        assert_eq!(
            first,
            compute_contract_digest(&root, "morning_briefing", &retuned, "prompt", None, None)
                .unwrap()
        );
        let overrides = Map::from_iter([(
            "talent.system.morning_briefing".to_owned(),
            json!({"max_output_tokens":9999,"extract":true}),
        )]);
        assert_eq!(
            first,
            compute_contract_digest(
                &root,
                "morning_briefing",
                &metadata,
                "prompt",
                Some(&overrides),
                None
            )
            .unwrap()
        );
        write(
            &root,
            "talent/morning_briefing.schema.json",
            "{\"type\":\"array\"}",
        );
        assert_ne!(
            first,
            compute_contract_digest(&root, "morning_briefing", &metadata, "prompt", None, None)
                .unwrap()
        );
        write(&root, "talent/morning_briefing.schema.json", "malformed");
        assert!(
            compute_contract_digest(&root, "morning_briefing", &metadata, "prompt", None, None)
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn absent_cutoff_keeps_todays_contract_bytes() {
        let root = root("absent-cutoff-contract");
        let evidence = evidence_of(&root, "20260910", "morning_briefing", None);
        let direct = compute_contract_digest(
            &root,
            "morning_briefing",
            &metadata_for("morning_briefing"),
            "prompt",
            None,
            None,
        )
        .unwrap();
        assert_eq!(evidence.contract, direct);
        assert!(evidence.evidence.get("daily_location_input").is_none());
        assert!(!root.join("config/journal.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn eligible_recipient_appends_daily_location_input_and_excluded_hooks_do_not_read_a_malformed_cutoff()
     {
        let root = root("location-input-cutoff");
        write(&root, "facets/work/facet.json", r#"{"title":"Work"}"#);
        let recipients = [
            "entities:entity_suggest",
            "entities:entity_observer",
            "morning_briefing",
        ];
        let excluded = ["daily_schedule", "entities:entities_review"];
        write_config(
            &root,
            &json!({"identity":{"timezone":"UTC"},"daily_inputs":{"location":{"location-words-1":"not-a-day"}}}),
        );
        for name in excluded {
            let facet = (name != "daily_schedule").then_some("work");
            let without =
                compute_contract_digest(&root, name, &metadata_for(name), "prompt", None, None)
                    .unwrap();
            let evidence = evidence_of(&root, "20260910", name, facet);
            assert_eq!(evidence.contract, without, "{name}");
            assert!(evidence.evidence.get("daily_location_input").is_none());
        }
        for name in recipients {
            let facet = (name != "morning_briefing").then_some("work");
            assert!(
                compute_daily_evidence(
                    &root,
                    "20260910",
                    name,
                    &metadata_for(name),
                    "prompt",
                    facet,
                    None,
                )
                .is_err(),
                "{name}"
            );
        }
        write_config(
            &root,
            &json!({"identity":{"timezone":"UTC"},"daily_inputs":{"location":{"location-words-1":"20260910"}}}),
        );
        for name in recipients {
            let facet = (name != "morning_briefing").then_some("work");
            let on_cutoff = evidence_of(&root, "20260910", name, facet);
            let with_bit = compute_contract_digest(
                &root,
                name,
                &metadata_for(name),
                "prompt",
                None,
                Some(
                    &serde_json::from_value(json!({"location": DAILY_INPUTS[0].version})).unwrap(),
                ),
            )
            .unwrap();
            let without =
                compute_contract_digest(&root, name, &metadata_for(name), "prompt", None, None)
                    .unwrap();
            assert_eq!(on_cutoff.contract, with_bit, "{name}");
            assert_ne!(on_cutoff.contract, without, "{name}");
            assert!(on_cutoff.evidence.get("daily_location_input").is_none());
            let before = evidence_of(&root, "20260909", name, facet);
            assert_eq!(before.contract, without, "{name} before cutoff");
        }
        for name in excluded {
            let facet = (name != "daily_schedule").then_some("work");
            let evidence = evidence_of(&root, "20260910", name, facet);
            let without =
                compute_contract_digest(&root, name, &metadata_for(name), "prompt", None, None)
                    .unwrap();
            assert_eq!(evidence.contract, without, "{name}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn daily_location_input_version_is_none_until_the_cutoff() {
        let cutoff = chrono::NaiveDate::parse_from_str("20260910", "%Y%m%d").unwrap();
        for hook in [
            "entities:entity_suggest",
            "entities:entity_observer",
            "morning_briefing",
        ] {
            assert_eq!(DAILY_INPUTS[0].version_since(hook, "20260910", None), None);
            assert_eq!(
                DAILY_INPUTS[0].version_since(hook, "20260909", Some(cutoff)),
                None
            );
            assert_eq!(
                DAILY_INPUTS[0].version_since(hook, "20260910", Some(cutoff)),
                Some(DAILY_INPUTS[0].version)
            );
            assert_eq!(
                DAILY_INPUTS[0].version_since(hook, "not-a-day", Some(cutoff)),
                None
            );
        }
        for hook in ["daily_schedule", "entities:entities_review"] {
            assert_eq!(
                DAILY_INPUTS[0].version_since(hook, "20260910", Some(cutoff)),
                None
            );
        }
    }

    #[test]
    fn compute_daily_evidence_on_a_journal_with_no_config_file_creates_none() {
        let root = root("evidence-no-config");
        let evidence = evidence_of(&root, "20260910", "morning_briefing", None);
        assert!(!evidence.contract.is_empty());
        assert!(!root.join("config/journal.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn daily_maintenance_shares_calendar_window_and_observes_named_segment_topology() {
        let root = root("daily-maintenance-window");
        let anchor = journal_today(&root);
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
        let first = revision(&root, "morning_briefing", None);
        write(
            &root,
            "config/journal.json",
            r#"{"providers":{"active":{"provider":"local","model":"m1","pid":2}}}"#,
        );
        assert_eq!(first, revision(&root, "morning_briefing", None));
        write(
            &root,
            "config/journal.json",
            r#"{"providers":{"active":{"provider":"local","model":"m2","pid":2}}}"#,
        );
        assert_ne!(first, revision(&root, "morning_briefing", None));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn daily_contract_refuses_missing_unknown_or_changed_hook_phases() {
        assert!(daily_hook("morning_briefing", &Map::new()).is_err());
        assert!(
            daily_hook(
                "morning_briefing",
                &Map::from_iter([("hook".to_owned(), json!({"post":"morning_briefing"}))])
            )
            .is_err()
        );
        assert!(
            daily_hook(
                "morning_briefing",
                &Map::from_iter([("hook".to_owned(), json!({"pre":"unknown"}))])
            )
            .is_err()
        );
        // A retired daily talent is an unknown hook.
        assert!(
            daily_hook(
                "schedule",
                &Map::from_iter([("hook".to_owned(), json!({"post":"schedule"}))])
            )
            .is_err()
        );
    }

    // --- one projection per day, shared by the units of a coverage read ---------

    const DAY: &str = "20260910";
    const UNITS: [(&str, Option<&str>); 4] = [
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
            "entities:entity_observer",
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
            "entities:entity_observer",
            Some("work"),
            &mut DayProjectionCache::new(&root, DAY),
        )
        .unwrap();
        assert_ne!(first, second);
        assert_eq!(
            second,
            uncached(&root, DAY, "entities:entity_observer", Some("work")).unwrap()
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
            cached(
                &root,
                DAY,
                "entities:entity_observer",
                Some("work"),
                &mut cache
            )
            .unwrap_err(),
            failure
        );
        assert_eq!(
            cached(&root, DAY, "morning_briefing", None, &mut cache).unwrap_err(),
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
        let expected = uncached(&root, DAY, "entities:entity_observer", Some("work")).unwrap();
        for mut cache in [
            DayProjectionCache::new(&root, "20260909"),
            DayProjectionCache::new(&other, DAY),
        ] {
            assert_eq!(
                cached(
                    &root,
                    DAY,
                    "entities:entity_observer",
                    Some("work"),
                    &mut cache
                )
                .unwrap(),
                expected
            );
            assert_eq!(cache.requests(), 0, "a mismatched cache is never asked");
            assert_eq!(cache.captures(), 1, "the direct capture is counted");
        }
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(other).unwrap();
    }

    #[test]
    fn topics_md_in_talents_excluded_from_daily_evidence_projection_and_revision() {
        let root = root("topics-md-excluded");
        write(
            &root,
            "chronicle/20260910/mic/090000_60/talents/sense.md",
            "# Sense\n\nOriginal sense narrative.",
        );
        let rev_before = revision(&root, "morning_briefing", None);
        let proj_before = capture_day_projection(&root, "20260910").expect("capture before");
        assert!(
            !proj_before.chunks.is_empty(),
            "projection should include sense.md chunk"
        );

        write(
            &root,
            "chronicle/20260910/mic/090000_60/talents/topics.md",
            "## Topics\n- testing\n\n## Quotes\n- none\n",
        );
        let rev_after = revision(&root, "morning_briefing", None);
        let proj_after = capture_day_projection(&root, "20260910").expect("capture after");

        assert_eq!(rev_before, rev_after);
        assert_eq!(proj_before.chunks, proj_after.chunks);
        assert_eq!(proj_before.sources, proj_after.sources);

        write(
            &root,
            "chronicle/20260910/mic/090000_60/talents/sense.md",
            "# Sense\n\nUpdated sense narrative.",
        );
        let rev_changed = revision(&root, "morning_briefing", None);
        assert_ne!(rev_before, rev_changed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn objects_md_in_talents_excluded_from_daily_evidence_projection_and_revision() {
        let root = root("objects-md-excluded");
        write(
            &root,
            "chronicle/20260910/desk/090000_60/talents/sense.md",
            "# Sense\n\nOriginal sense narrative.",
        );
        let rev_before = revision(&root, "morning_briefing", None);
        let proj_before = capture_day_projection(&root, "20260910").expect("capture before");

        write(
            &root,
            "chronicle/20260910/desk/090000_60/talents/objects.md",
            "# Objects\n\n- person: in 2 frames, up to 3 at once\n",
        );
        let rev_after = revision(&root, "morning_briefing", None);
        let proj_after = capture_day_projection(&root, "20260910").expect("capture after");

        assert_eq!(rev_before, rev_after);
        assert_eq!(proj_before.chunks, proj_after.chunks);
        assert_eq!(proj_before.sources, proj_after.sources);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn screen_text_md_in_talents_excluded_from_daily_evidence_projection_and_revision() {
        let root = root("screen-text-md-excluded");
        write(
            &root,
            "chronicle/20260910/desk/090000_60/talents/sense.md",
            "# Sense\n\nOriginal sense narrative.",
        );
        let rev_before = revision(&root, "morning_briefing", None);
        let proj_before = capture_day_projection(&root, "20260910").expect("capture before");

        write(
            &root,
            "chronicle/20260910/desk/090000_60/talents/screen_text.md",
            "## 00:01 · editor\n\n- code on screen\n",
        );
        let rev_after = revision(&root, "morning_briefing", None);
        let proj_after = capture_day_projection(&root, "20260910").expect("capture after");

        assert_eq!(rev_before, rev_after);
        assert_eq!(proj_before.chunks, proj_after.chunks);
        assert_eq!(proj_before.sources, proj_after.sources);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_video_captures_sources_and_tolerates_corrupt_lines() {
        let root = root("protected-video-daily");
        let note_rel = "chronicle/20260910/desk/120000_60/120000_60_protected_video.jsonl";
        let row_a = r#"{"t":"protected_video","ts":1792027800000,"end_ts":1792030500000,"service":"Netflix","app":"Comet","title":"Netflix"}"#;
        let row_b = r#"{"t":"protected_video","ts":1000,"end_ts":91000,"service":"Apple TV"}"#;
        write(&root, note_rel, &format!("{row_a}\n{row_b}\n"));

        let sources = capture_day_sources(&root, "20260910").expect("capture day sources");
        assert_eq!(sources.len(), 2);
        assert_eq!(
            sources[0].path,
            "20260910/desk/120000_60/120000_60_protected_video.jsonl"
        );
        assert_eq!(sources[0].occurrence_time_ms, Some(1792027800000));
        assert_eq!(sources[0].agent, "protected_video");
        assert_eq!(sources[1].occurrence_time_ms, Some(1000));
        assert_eq!(sources[1].agent, "protected_video");

        // Same day, add a valid activity and add corrupt lines to note file
        write(
            &root,
            "facets/work/activities/20260910.jsonl",
            r#"{"source":"user","title":"QuarterlySentinelToken"}"#,
        );
        write(&root, note_rel, &format!("{row_a}\n{{\n42\n{row_b}\n"));
        let sources_mixed =
            capture_day_sources(&root, "20260910").expect("capture with mixed note");
        assert!(
            sources_mixed
                .iter()
                .any(|c| c.text.contains("QuarterlySentinelToken"))
        );
        assert!(sources_mixed.iter().any(|c| c.text.contains("Netflix")));
        assert!(sources_mixed.iter().any(|c| c.text.contains("Apple TV")));

        // Sibling shape.json with Browser
        write(
            &root,
            "chronicle/20260910/desk/120000_60/shape.json",
            r#"{"120000_60_protected_video.jsonl":"Browser"}"#,
        );
        let sources_sidecar = capture_day_sources(&root, "20260910").expect("capture with sidecar");
        let note_chunks = sources_sidecar
            .iter()
            .filter(|c| c.path.ends_with("_protected_video.jsonl"))
            .collect::<Vec<_>>();
        assert_eq!(note_chunks.len(), 2);
        assert_eq!(note_chunks[0].agent, "protected_video");
        assert!(note_chunks[0].text.contains("Netflix"));

        // Sibling shape.json with NotAShape
        write(
            &root,
            "chronicle/20260910/desk/120000_60/shape.json",
            r#"{"120000_60_protected_video.jsonl":"NotAShape"}"#,
        );
        let sources_not_a_shape =
            capture_day_sources(&root, "20260910").expect("capture with unparseable shape");
        assert!(
            sources_not_a_shape
                .iter()
                .any(|c| c.agent == "protected_video")
        );

        // Malformed sidecar JSON on this file must still return Err
        write(
            &root,
            "chronicle/20260910/desk/120000_60/shape.json",
            "{not json",
        );
        assert!(capture_day_sources(&root, "20260910").is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_video_activity_malformed_json_still_errors_without_panic() {
        let root = root("protected-video-activity-malformed");
        write(&root, "facets/work/activities/20260910.jsonl", "{not json");
        let result = capture_day_sources(&root, "20260910");
        assert!(result.is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn desktop_activity_captures_sources_two_files_and_tolerates_corrupt_lines() {
        let root = root("desktop-activity-daily");
        let note_rel_1 = "chronicle/20260910/desk/120000_60/120000_60_desktop_activity.jsonl";
        let note_rel_2 = "chronicle/20260910/desk/120000_60/120000_60_z1_desktop_activity.jsonl";
        let row_1 = r#"{"v":1,"t":"desktop_activity","kind":"foreground","method":"nsworkspace","ts":1792027800000,"end_ts":1792027800000,"app":"Finder"}"#;
        let row_2 = r#"{"v":1,"t":"desktop_activity","kind":"terminal","method":"tmux","ts":1792027850000,"end_ts":1792027860000,"exe":"bash"}"#;
        write(&root, note_rel_1, &format!("{row_1}\n{{\n"));
        write(&root, note_rel_2, &format!("{row_2}\n"));

        // Conflicting shape.json does not change family or agent
        write(
            &root,
            "chronicle/20260910/desk/120000_60/shape.json",
            r#"{"120000_60_desktop_activity.jsonl":"Browser","120000_60_z1_desktop_activity.jsonl":"Activity"}"#,
        );

        let sources = capture_day_sources(&root, "20260910").expect("capture day sources");
        let act_chunks = sources
            .iter()
            .filter(|c| c.path.ends_with("_desktop_activity.jsonl"))
            .collect::<Vec<_>>();
        assert_eq!(act_chunks.len(), 2);
        assert_eq!(act_chunks[0].occurrence_time_ms, Some(1792027800000));
        assert_eq!(act_chunks[0].agent, "desktop_activity");
        assert!(act_chunks[0].text.contains("Finder"));

        assert_eq!(act_chunks[1].occurrence_time_ms, Some(1792027850000));
        assert_eq!(act_chunks[1].agent, "desktop_activity");
        assert!(act_chunks[1].text.contains("bash"));

        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
#[test]
fn retry_maintenance_fingerprint_ignores_midnight_without_new_sources() {
    let evidence = |anchor: &str, windows: Value| DailyEvidence {
        revision: "unused".into(),
        contract: "contract".into(),
        evidence: json!({"anchor":anchor,"windows":windows}),
    };
    let before = evidence(
        "2026-01-01",
        json!({"20260101":[],"20251231":["120000_300"]}),
    );
    let after = evidence(
        "2026-01-02",
        json!({"20260102":[],"20260101":[],"20251231":["120000_300"]}),
    );
    assert_ne!(before.revision_under("same"), after.revision_under("same"));
    assert_eq!(
        before.retry_revision_under("same"),
        after.retry_revision_under("same")
    );
    let changed = evidence(
        "2026-01-02",
        json!({"20260102":["090000_300"],"20260101":[],"20251231":["120000_300"]}),
    );
    assert_ne!(
        before.retry_revision_under("same"),
        changed.retry_revision_under("same")
    );
}
