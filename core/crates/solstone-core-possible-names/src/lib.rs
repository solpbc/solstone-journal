// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Possible names for the speech in one segment.
//!
//! After transcription the possible-names talent asks the owner's model which
//! journal entities a span of speech may refer to, and records the accepted
//! choices in `<segment>/talents/possible_names.json`. This crate owns that
//! file: its types, its one writer, and the check every reader runs before it
//! shows an entry.
//!
//! The transcript is never changed. A possible name is a tentative label shown
//! beside the words it may refer to, and only while it still holds:
//! [`live_entries`] hides an entry whose transcript bytes, words, entity,
//! canonical name or facet relationship no longer match, and limits an agent
//! connection to entities attached to a facet it may see. Nothing is written
//! to hide an entry; the next run of the talent regenerates the file.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{AtomicWriteError, JsonWriteOptions, write_json};
pub use solstone_core_transcripts::PossibleNameLabels;
use solstone_core_transcripts::{next_statement_ordinal, statement_text};

pub use solstone_core_format::content::possible_names::{
    POSSIBLE_NAMES_RELATIVE_PATH, POSSIBLE_NAMES_SCHEMA, is_possible_names_path,
    possible_name_label,
};

/// Largest sidecar a reader accepts.
pub const MAX_SIDECAR_BYTES: u64 = 1024 * 1024;

/// `<segment>/talents/possible_names.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    /// Always [`POSSIBLE_NAMES_SCHEMA`].
    pub schema: String,
    /// Policy version of the retrieval and prompt that produced the entries.
    pub talent_version: String,
    /// The transcript the entries address, as it was when they were chosen.
    pub source: SidecarSource,
    /// Hex revision of the entity registry the candidates were drawn from, as
    /// the segment saw it (its own facets included). Only the talent compares it.
    pub registry_revision: String,
    pub generated_at_ms: i64,
    /// Accepted choices. Empty when the talent abstained.
    pub entries: Vec<SidecarEntry>,
}

/// The transcript file an entry list addresses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarSource {
    /// Transcript file name inside the segment directory, normally `audio.jsonl`.
    pub file: String,
    /// Lowercase hex SHA-256 of the file's bytes ([`source_sha256`]).
    pub sha256: String,
}

/// One accepted possible name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarEntry {
    /// 1-based statement ordinal, as speaker labels count them
    /// (`solstone_core_transcripts::next_statement_ordinal`).
    pub sentence: u64,
    /// UTF-8 byte span of the statement's words (`corrected` when present,
    /// else `text`; `solstone_core_transcripts::statement_text`).
    pub start_byte: usize,
    pub end_byte: usize,
    /// Exactly the words at that span.
    pub heard: String,
    pub entity_id: String,
    /// The entity's canonical name when the entry was chosen.
    pub name: String,
}

impl Sidecar {
    /// A sidecar under the current schema.
    #[must_use]
    pub fn new(
        talent_version: impl Into<String>,
        source: SidecarSource,
        registry_revision: impl Into<String>,
        generated_at_ms: i64,
        entries: Vec<SidecarEntry>,
    ) -> Self {
        Self {
            schema: POSSIBLE_NAMES_SCHEMA.to_owned(),
            talent_version: talent_version.into(),
            source,
            registry_revision: registry_revision.into(),
            generated_at_ms,
            entries,
        }
    }
}

/// Failure reading a sidecar that exists.
#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    TooLarge,
    Malformed(String),
}

impl fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "possible names could not be read: {error}"),
            Self::TooLarge => write!(
                formatter,
                "possible names are larger than {MAX_SIDECAR_BYTES} bytes"
            ),
            Self::Malformed(detail) => write!(formatter, "possible names are malformed: {detail}"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Failure saving a sidecar.
#[derive(Debug)]
pub enum WriteError {
    /// The sidecar breaks the contract; nothing was written.
    Invalid(String),
    Io(AtomicWriteError),
}

impl fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(detail) => write!(formatter, "possible names refused: {detail}"),
            Self::Io(error) => write!(formatter, "possible names could not be saved: {error}"),
        }
    }
}

impl std::error::Error for WriteError {}

/// Where a segment's sidecar lives.
#[must_use]
pub fn sidecar_path(segment_dir: &Path) -> PathBuf {
    segment_dir.join(POSSIBLE_NAMES_RELATIVE_PATH)
}

/// Read a segment's sidecar. `Ok(None)` when it has none.
pub fn read(segment_dir: &Path) -> Result<Option<Sidecar>, ReadError> {
    let file = match File::open(sidecar_path(segment_dir)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ReadError::Io(error)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_SIDECAR_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(ReadError::Io)?;
    if bytes.len() as u64 > MAX_SIDECAR_BYTES {
        return Err(ReadError::TooLarge);
    }
    let sidecar: Sidecar =
        serde_json::from_slice(&bytes).map_err(|error| ReadError::Malformed(error.to_string()))?;
    if sidecar.schema != POSSIBLE_NAMES_SCHEMA {
        return Err(ReadError::Malformed(format!(
            "schema {:?} is not {POSSIBLE_NAMES_SCHEMA}",
            sidecar.schema
        )));
    }
    Ok(Some(sidecar))
}

/// Atomically replace a segment's sidecar. The only writer of the file.
///
/// The caller enqueues the path for search afterwards
/// (`solstone_core_indexer_store::todo::enqueue_path`).
pub fn write(segment_dir: &Path, sidecar: &Sidecar) -> Result<(), WriteError> {
    check_contract(sidecar).map_err(WriteError::Invalid)?;
    write_json(
        sidecar_path(segment_dir),
        sidecar,
        JsonWriteOptions::default(),
    )
    .map_err(WriteError::Io)
}

/// Lowercase hex SHA-256 of a transcript file's current bytes: the value a
/// sidecar's `source.sha256` must hold for its entries to be shown.
pub fn source_sha256(segment_dir: &Path, file: &str) -> io::Result<String> {
    if !is_transcript_file_name(file) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{file:?} is not a transcript file name"),
        ));
    }
    Ok(sha256_hex(&fs::read(segment_dir.join(file))?))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A transcript a possible name may address: one speech transcript file
/// directly inside the segment, named as the transcript readers name them.
fn is_transcript_file_name(name: &str) -> bool {
    !name.contains(['/', '\\'])
        && !name.starts_with('.')
        && (name == "audio.jsonl"
            || name.ends_with("_audio.jsonl")
            || name.ends_with("_transcript.jsonl")
            || solstone_core_format::audio_archive::is_audio_archive_transcript_name(name))
}

fn check_contract(sidecar: &Sidecar) -> Result<(), String> {
    if sidecar.schema != POSSIBLE_NAMES_SCHEMA {
        return Err(format!("schema must be {POSSIBLE_NAMES_SCHEMA}"));
    }
    if !is_transcript_file_name(&sidecar.source.file) {
        return Err(format!(
            "source file {:?} is not a transcript in the segment",
            sidecar.source.file
        ));
    }
    let digest = &sidecar.source.sha256;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("source sha256 must be 64 lowercase hex digits".to_owned());
    }
    for (index, entry) in sidecar.entries.iter().enumerate() {
        let problem = if entry.sentence == 0 {
            Some("sentence ordinals start at 1")
        } else if entry.start_byte >= entry.end_byte {
            Some("start_byte must be before end_byte")
        } else if entry.heard.len() != entry.end_byte - entry.start_byte {
            Some("heard must be exactly the bytes of its span")
        } else if entry.entity_id.trim().is_empty() {
            Some("entity_id is empty")
        } else if entry.name.trim().is_empty() {
            Some("name is empty")
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(format!("entry {index}: {problem}"));
        }
    }
    Ok(())
}

/// Who is reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Viewer {
    /// The journal's owner.
    Owner,
    /// An agent connection, with the facet directories it may see. Callers
    /// resolve these through the connection's own facet scope.
    Facets(BTreeSet<String>),
}

/// An entry that may be shown to this viewer now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveEntry {
    /// Position in the sidecar's `entries`, which is also its search chunk index.
    pub index: usize,
    /// Transcript file name inside the segment.
    pub file: String,
    pub sentence: u64,
    pub start_byte: usize,
    pub end_byte: usize,
    pub heard: String,
    pub entity_id: String,
    pub name: String,
}

/// The entity facts possible names are checked against, read once and then
/// reused, so one response checks every entry against one view of the
/// registry. Reads go through the entity and facet stores.
pub struct Registry {
    journal_root: PathBuf,
    attached: OnceLock<Option<BTreeMap<String, AttachedEntity>>>,
    retired: Mutex<HashMap<String, bool>>,
}

struct AttachedEntity {
    name: Option<String>,
    facets: BTreeSet<String>,
}

impl Registry {
    #[must_use]
    pub fn new(journal_root: &Path) -> Self {
        Self {
            journal_root: journal_root.to_path_buf(),
            attached: OnceLock::new(),
            retired: Mutex::new(HashMap::new()),
        }
    }

    /// Unblocked entities with a non-detached facet relationship, by id, with
    /// those facets. `None` when the registry can't be read: nothing is shown.
    fn attached(&self) -> Option<&BTreeMap<String, AttachedEntity>> {
        self.attached
            .get_or_init(|| match load_attached(&self.journal_root) {
                Ok(attached) => Some(attached),
                Err(detail) => {
                    log::warn!("possible names are hidden: {detail}");
                    None
                }
            })
            .as_ref()
    }

    /// Merged, deleted, or in the middle of a delete; a damaged record counts.
    fn retired(&self, entity_id: &str) -> bool {
        let mut retired = self
            .retired
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *retired.entry(entity_id.to_owned()).or_insert_with(|| {
            !matches!(
                solstone_core_entity::retired_record_hold(&self.journal_root, entity_id),
                solstone_core_entity::RetiredRecordHold::Free
            )
        })
    }

    /// Whether `entity_id` still exists under `name` and this viewer may see it.
    pub fn admits(&self, entity_id: &str, name: &str, viewer: &Viewer) -> bool {
        let Some(entity) = self.attached().and_then(|attached| attached.get(entity_id)) else {
            return false;
        };
        if entity.name.as_deref() != Some(name) || entity.facets.is_empty() {
            return false;
        }
        let visible = match viewer {
            Viewer::Owner => true,
            Viewer::Facets(facets) => !entity.facets.is_disjoint(facets),
        };
        visible && !self.retired(entity_id)
    }
}

fn load_attached(journal_root: &Path) -> Result<BTreeMap<String, AttachedEntity>, String> {
    let map = solstone_core_entity::read_identity_map(journal_root)
        .map_err(|error| format!("entity identities could not be read: {error}"))?;
    let facet_dirs = solstone_core_facets::list_facet_directories(journal_root)
        .map_err(|error| format!("facets could not be listed: {error}"))?;
    let mut attached = BTreeMap::<String, AttachedEntity>::new();
    for facet_dir in facet_dirs {
        // Blocked entities and detached relationships are left out here.
        let entities = solstone_core_facets::list_scoped_facet_entities_tolerant_with_resolved_ids(
            journal_root,
            &facet_dir,
            false,
            false,
            &map.resolved,
        )
        .map_err(|error| format!("facet {facet_dir} could not be read: {error}"))?;
        for entity in entities {
            attached
                .entry(entity.entity_id)
                .or_insert_with(|| AttachedEntity {
                    name: entity
                        .identity
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    facets: BTreeSet::new(),
                })
                .facets
                .insert(facet_dir.clone());
        }
    }
    Ok(attached)
}

/// One entity the talent may offer as a possible name: the same population
/// [`Registry::admits`] shows, read whole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CandidateEntity {
    pub entity_id: String,
    /// Canonical name.
    pub name: String,
    /// Entity type, empty when none is recorded.
    #[serde(rename = "type")]
    pub entity_type: String,
    /// Recorded alternative names, in stored order, blank ones left out.
    pub aka: Vec<String>,
    /// Each non-detached facet relationship by facet directory, with its
    /// description (empty when it has none).
    pub facets: BTreeMap<String, String>,
}

/// Every entity a possible name may be drawn from, by id: unblocked, with a
/// non-detached facet relationship and a canonical name, and neither merged
/// nor deleted. Reads go through the entity and facet stores.
pub fn load_candidate_entities(journal_root: &Path) -> Result<Vec<CandidateEntity>, String> {
    let map = solstone_core_entity::read_identity_map(journal_root)
        .map_err(|error| format!("entity identities could not be read: {error}"))?;
    let facet_dirs = solstone_core_facets::list_facet_directories(journal_root)
        .map_err(|error| format!("facets could not be listed: {error}"))?;
    let mut found = BTreeMap::<String, CandidateEntity>::new();
    for facet_dir in facet_dirs {
        let entities = solstone_core_facets::list_scoped_facet_entities_tolerant_with_resolved_ids(
            journal_root,
            &facet_dir,
            false,
            false,
            &map.resolved,
        )
        .map_err(|error| format!("facet {facet_dir} could not be read: {error}"))?;
        for entity in entities {
            let Some(name) = entity.identity.get("name").and_then(Value::as_str) else {
                continue;
            };
            let description = entity
                .relationship
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            found
                .entry(entity.entity_id.clone())
                .or_insert_with(|| CandidateEntity {
                    entity_id: entity.entity_id.clone(),
                    name: name.to_owned(),
                    entity_type: entity
                        .identity
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    aka: entity
                        .identity
                        .get("aka")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .filter(|aka| !aka.trim().is_empty())
                        .map(str::to_owned)
                        .collect(),
                    facets: BTreeMap::new(),
                })
                .facets
                .insert(facet_dir.clone(), description);
        }
    }
    Ok(found
        .into_values()
        .filter(|entity| {
            matches!(
                solstone_core_entity::retired_record_hold(journal_root, &entity.entity_id),
                solstone_core_entity::RetiredRecordHold::Free
            )
        })
        .collect())
}

/// The speech transcript a segment's possible names address: `audio.jsonl`
/// when the segment has one, otherwise its only speech transcript. `None` when
/// it has none, or several and no `audio.jsonl`. An imported AI chat is never
/// speech, so it is never addressed.
#[must_use]
pub fn speech_transcript(segment_dir: &Path) -> Option<String> {
    let origin = solstone_core_journal_io::segment_origin(segment_dir);
    let mut names = fs::read_dir(segment_dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| is_transcript_file_name(name))
        .filter(|name| {
            !matches!(
                solstone_core_format::content::classify(&format!("{origin}/{name}")),
                solstone_core_format::content::ContentResolution::Indexed(
                    solstone_core_format::content::Family::AiChat
                )
            )
        })
        .collect::<Vec<_>>();
    if names.iter().any(|name| name == "audio.jsonl") {
        return Some("audio.jsonl".to_owned());
    }
    (names.len() == 1).then(|| names.remove(0))
}

/// The entries of a segment's sidecar this viewer may be shown now, in
/// sidecar order. Every reader calls this (or [`live_entries_with`]).
///
/// An entry is shown only when the transcript file still has the bytes the
/// entries were chosen from (otherwise none is), its statement still holds
/// `heard` at its span, its entity exists, is not blocked, merged or deleted,
/// still has `name` as its canonical name and a non-detached facet
/// relationship, and, for an agent, one of those facets is one it may see.
#[must_use]
pub fn live_entries(journal_root: &Path, segment_dir: &Path, viewer: &Viewer) -> Vec<LiveEntry> {
    live_entries_with(&Registry::new(journal_root), segment_dir, viewer)
}

/// [`live_entries`] against a registry the caller reuses across segments.
#[must_use]
pub fn live_entries_with(
    registry: &Registry,
    segment_dir: &Path,
    viewer: &Viewer,
) -> Vec<LiveEntry> {
    let sidecar = match read(segment_dir) {
        Ok(Some(sidecar)) => sidecar,
        Ok(None) => return Vec::new(),
        Err(error) => {
            log::warn!(
                "possible names for {} are hidden: {error}",
                segment_dir.display()
            );
            return Vec::new();
        }
    };
    if sidecar.entries.is_empty() || !is_transcript_file_name(&sidecar.source.file) {
        return Vec::new();
    }
    let Ok(bytes) = fs::read(segment_dir.join(&sidecar.source.file)) else {
        return Vec::new();
    };
    if !sidecar
        .source
        .sha256
        .eq_ignore_ascii_case(&sha256_hex(&bytes))
    {
        return Vec::new();
    }
    let Ok(transcript) = std::str::from_utf8(&bytes) else {
        return Vec::new();
    };
    let statements = statement_words(
        transcript,
        &sidecar
            .entries
            .iter()
            .map(|entry| entry.sentence)
            .collect::<BTreeSet<_>>(),
    );
    sidecar
        .entries
        .into_iter()
        .enumerate()
        .filter(|(_, entry)| {
            statements
                .get(&entry.sentence)
                .and_then(|words| words.get(entry.start_byte..entry.end_byte))
                .is_some_and(|words| !words.is_empty() && words == entry.heard)
        })
        .filter(|(_, entry)| registry.admits(&entry.entity_id, &entry.name, viewer))
        .map(|(index, entry)| LiveEntry {
            index,
            file: sidecar.source.file.clone(),
            sentence: entry.sentence,
            start_byte: entry.start_byte,
            end_byte: entry.end_byte,
            heard: entry.heard,
            entity_id: entry.entity_id,
            name: entry.name,
        })
        .collect()
}

/// The words of each wanted statement, counted as speaker labels count them.
fn statement_words(transcript: &str, wanted: &BTreeSet<u64>) -> BTreeMap<u64, String> {
    let mut ordinal = 0;
    let mut words = BTreeMap::new();
    for line in transcript.lines() {
        let Some(sentence) = next_statement_ordinal(line, &mut ordinal)
            .and_then(|sentence| u64::try_from(sentence).ok())
        else {
            continue;
        };
        if !wanted.contains(&sentence) {
            continue;
        }
        if let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line.trim())
            && let Some((_, text)) = statement_text(&record)
        {
            words.insert(sentence, text.to_owned());
        }
    }
    words
}

/// The labels a transcript reader shows for these entries, or `None` when
/// there are none.
#[must_use]
pub fn transcript_labels(entries: &[LiveEntry]) -> Option<PossibleNameLabels> {
    let first = entries.first()?;
    let mut labels = PossibleNameLabels {
        file: first.file.clone(),
        statements: BTreeMap::new(),
    };
    for entry in entries.iter().filter(|entry| entry.file == first.file) {
        let Ok(sentence) = i64::try_from(entry.sentence) else {
            continue;
        };
        labels
            .statements
            .entry(sentence)
            .or_default()
            .push((entry.end_byte, entry.name.clone()));
    }
    Some(labels)
}

/// One reader's live entries across the segments a response touches: each
/// segment is checked once, all against one registry view.
pub struct LiveSegments {
    journal_root: PathBuf,
    viewer: Viewer,
    registry: Registry,
    segments: Mutex<HashMap<PathBuf, std::sync::Arc<Vec<LiveEntry>>>>,
}

impl LiveSegments {
    #[must_use]
    pub fn new(journal_root: &Path, viewer: Viewer) -> Self {
        Self {
            journal_root: journal_root.to_path_buf(),
            viewer,
            registry: Registry::new(journal_root),
            segments: Mutex::new(HashMap::new()),
        }
    }

    /// [`live_entries`] for this reader, read once per segment.
    pub fn entries(&self, segment_dir: &Path) -> std::sync::Arc<Vec<LiveEntry>> {
        let mut segments = self
            .segments
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::sync::Arc::clone(
            segments
                .entry(segment_dir.to_path_buf())
                .or_insert_with(|| {
                    std::sync::Arc::new(live_entries_with(
                        &self.registry,
                        segment_dir,
                        &self.viewer,
                    ))
                }),
        )
    }

    /// The live entry a stored search row (path, chunk index, content) stands for.
    pub fn indexed(&self, rel: &str, idx: i64, content: &str) -> Option<LiveEntry> {
        let segment_dir = segment_dir_for_indexed_path(&self.journal_root, rel)?;
        indexed_entry(&self.entries(&segment_dir), idx, content).cloned()
    }
}

/// The segment directory an indexed possible-names path belongs to.
#[must_use]
pub fn segment_dir_for_indexed_path(journal_root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.strip_prefix("chronicle/").unwrap_or(rel);
    if !is_possible_names_path(rel) {
        return None;
    }
    let parts = rel.split('/').collect::<Vec<_>>();
    let [day, stream, segment, _, _] = parts.as_slice() else {
        return None;
    };
    if [day, stream, segment]
        .iter()
        .any(|part| part.is_empty() || **part == "." || **part == "..")
    {
        return None;
    }
    Some(
        journal_root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment),
    )
}

/// The live entry an indexed chunk stands for: the entry at chunk `idx`,
/// provided the chunk still reads as that entry's label.
#[must_use]
pub fn indexed_entry<'a>(
    entries: &'a [LiveEntry],
    idx: i64,
    content: &str,
) -> Option<&'a LiveEntry> {
    let index = usize::try_from(idx).ok()?;
    entries
        .iter()
        .find(|entry| entry.index == index)
        .filter(|entry| possible_name_label(&entry.name) == content)
}

#[cfg(test)]
mod tests;
