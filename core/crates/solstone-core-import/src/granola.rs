// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Granola's owner-key source. Remote IDs own stable placements; only a completed
//! publication advances a note's revision or the listing watermark.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, JsonWriteOptions, LockOptions, create_directory_with_mode,
    hold_lock, write_json, write_jsonl,
};
use solstone_core_segment::{ImportSource, Kind, SegmentDir, StreamHints};

mod http;
#[cfg(all(test, feature = "full-tests"))]
mod tests;
pub use http::LiveApi;

const STATE: &str = "imports/granola-source.json";
const OUTPUT: &str = "audio.jsonl";

/// Closed failures intentionally contain no transport details, response bodies or key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidKey,
    Key,
    Access,
    Limited,
    Unavailable,
    Response,
    Transcript,
    Storage,
    Busy,
    Stopped,
}
impl Error {
    /// Every failure, so a stored message can be read back as its kind.
    pub const ALL: [Self; 10] = [
        Self::InvalidKey,
        Self::Key,
        Self::Access,
        Self::Limited,
        Self::Unavailable,
        Self::Response,
        Self::Transcript,
        Self::Storage,
        Self::Busy,
        Self::Stopped,
    ];
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidKey => {
                "that isn't a complete Granola personal API key. it starts with grn_."
            }
            Self::Key => {
                "Granola no longer accepts this key. it may have been revoked or reached its expiration date. add a new key to carry on."
            }
            Self::Access => {
                "Granola turned this key away. API access needs Granola Business or Enterprise, and on Enterprise an admin can turn it off."
            }
            Self::Limited => {
                "Granola asked your journal to slow down. it will try again on its next check."
            }
            Self::Unavailable => {
                "couldn't reach Granola. your journal will try again on its next check."
            }
            Self::Response => {
                "Granola sent something your journal couldn't read. it will try again on its next check."
            }
            Self::Transcript => {
                "Granola didn't include a meeting's transcript. your journal will try again on its next check."
            }
            Self::Storage => {
                "your journal couldn't save what came from Granola. check that this computer has free space."
            }
            Self::Busy => "your journal is already checking Granola. try again when it finishes.",
            Self::Stopped => {
                "the check stopped before it finished. your journal will pick it up on its next check."
            }
        }
    }
    /// A stable name for the failure's kind, for surfaces that choose an action by it.
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidKey => "invalid_key",
            Self::Key => "key",
            Self::Access => "access",
            Self::Limited => "limited",
            Self::Unavailable => "unavailable",
            Self::Response => "response",
            Self::Transcript => "transcript",
            Self::Storage => "storage",
            Self::Busy => "busy",
            Self::Stopped => "stopped",
        }
    }
    /// The failure a stored message names. A message from an older wording names none.
    pub fn from_message(message: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|error| error.message() == message)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for Error {}

/// The fixed-origin HTTP boundary. Tests supply API-shaped responses in memory.
pub trait Api {
    fn get(&mut self, path: &str, query: &[(&str, String)]) -> Result<(u16, Value), Error>;
}

#[derive(Deserialize)]
struct Page {
    notes: Vec<Summary>,
    #[serde(rename = "hasMore")]
    more: bool,
    cursor: Option<String>,
}
#[derive(Deserialize)]
struct Summary {
    id: String,
    updated_at: String,
    deleted_at: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
struct Turn {
    text: String,
    start_time: String,
    end_time: String,
    speaker: Value,
}
#[derive(Deserialize, Serialize)]
struct Note {
    id: String,
    title: Option<String>,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
    web_url: String,
    summary_text: String,
    summary_markdown: Option<String>,
    private_notes_text: Option<String>,
    private_notes_markdown: Option<String>,
    attendees: Vec<Value>,
    calendar_event: Option<Value>,
    transcript: Option<Vec<Turn>>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    watermark: Option<String>,
    last_sync: Option<String>,
    last_added: Option<String>,
    error: Option<String>,
    #[serde(default)]
    notes: BTreeMap<String, Placement>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Placement {
    day: String,
    segment: String,
    origin: String,
    revision: Option<String>,
    #[serde(default)]
    removed: bool,
    #[serde(default)]
    legacy: bool,
    #[serde(default)]
    placed: bool,
}

fn storage<T>(_: T) -> Error {
    Error::Storage
}
fn instant(value: &str) -> Result<DateTime<Utc>, Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| Error::Response)
}
fn note_id(value: &str) -> Result<String, Error> {
    if value.is_ascii()
        && value.len() == 18
        && value.starts_with("not_")
        && value[4..].bytes().all(|b| b.is_ascii_alphanumeric())
    {
        Ok(value.to_owned())
    } else {
        Err(Error::Response)
    }
}
fn stream(id: &str) -> String {
    // Preserve every API ID byte on case-insensitive filesystems.
    let encoded: String = id.bytes().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}{encoded}",
        solstone_core_format::content::granola::STREAM_PREFIX
    )
}
fn legacy_id(value: &str) -> Option<String> {
    if let Ok(id) = note_id(value) {
        return Some(id);
    }
    // Only UUID document IDs have a case-insensitive canonical spelling.
    (value.len() == 36
        && value.bytes().enumerate().all(|(i, byte)| {
            if [8, 13, 18, 23].contains(&i) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        }))
    .then(|| value.to_ascii_lowercase())
}
fn state(root: &Path) -> Result<State, Error> {
    match fs::read(root.join(STATE)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(storage),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(storage(e)),
    }
}
fn save(root: &Path, state: &State) -> Result<(), Error> {
    write_json(
        root.join(STATE),
        state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    )
    .map_err(storage)
}
/// Serialize connection changes with the complete sync, without holding config locks over HTTP.
pub fn hold(root: &Path) -> Result<FileLock, Error> {
    create_directory_with_mode(&root.join("imports"), 0o700).map_err(storage)?;
    hold_lock(
        root.join(STATE),
        LockOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    )
    .map_err(|_| Error::Busy)
}
fn key(root: &Path) -> Result<Option<String>, Error> {
    let config = solstone_core_journal_config::read_journal_config(root)
        .map_err(storage)?
        .config
        .unwrap_or_default();
    Ok(config
        .get("granola")
        .and_then(|v| v.get("api_key"))
        .and_then(Value::as_str)
        .map(str::to_owned))
}
/// Does not return the credential or remote meeting content.
pub fn status(root: &Path) -> Result<Value, Error> {
    let state = state(root)?;
    // Meetings this source has put in the journal and the owner still keeps.
    let meetings = state
        .notes
        .values()
        .filter(|p| p.revision.is_some() && !p.removed && !p.legacy)
        .count();
    // An older wording names no kind; it still shows, with the generic action.
    let code = state
        .error
        .as_deref()
        .map(|message| Error::from_message(message).map_or("unknown", Error::code));
    Ok(json!({
        "connected": key(root)?.is_some(),
        "last_sync": state.last_sync,
        "last_added": state.last_added,
        "attention": state.error,
        "attention_code": code,
        "meetings": meetings,
    }))
}
/// Ask Granola whether it accepts a key, before it replaces a working one.
pub fn check_key(key: &str) -> Result<(), Error> {
    validate_key(key)?;
    check_with(&mut LiveApi::probe(key.to_owned()))
}
fn check_with(api: &mut dyn Api) -> Result<(), Error> {
    let (status, body) = api.get("/v1/notes", &[("page_size", "1".to_owned())])?;
    checked(status, body).map(|_| ())
}
/// Validate a personal key before changing connection state.
pub fn validate_key(key: &str) -> Result<(), Error> {
    if !key.starts_with("grn_")
        || key.len() <= 4
        || key.len() > 4096
        || !key.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(Error::InvalidKey);
    }
    Ok(())
}
/// A new key requires [`hold`] and revisits history, retaining every stable ID
/// and removal. Removing a key also cancels the active transport before taking that hold.
pub fn set_key(root: &Path, api_key: Option<&str>) -> Result<(), Error> {
    if let Some(key) = api_key {
        validate_key(key)?;
        let mut state = state(root)?;
        state.watermark = None;
        state.error = None;
        save(root, &state)?;
    }
    use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
    mutate_journal_config(root, LockOptions::default(), |config| {
        if let Some(key) = api_key {
            config.insert("granola".into(), json!({"api_key":key}));
        } else {
            config.remove("granola");
        }
        JournalConfigMutation {
            changed: true,
            value: (),
        }
    })
    .map_err(storage)?;
    Ok(())
}

/// Run the enabled source. A disconnected scheduled invocation is a harmless no-op.
pub fn sync(root: &Path) -> Result<(), Error> {
    let _hold = hold(root)?;
    let Some(key) = key(root)? else {
        return Ok(());
    };
    sync_with(
        root,
        &mut LiveApi::new(root.to_path_buf(), key),
        &mut publish,
    )
}

type Publisher<'a> = dyn FnMut(&Path, &str, &Placement, &Path) -> Result<(), Error> + 'a;

fn sync_with(root: &Path, api: &mut dyn Api, publish: &mut Publisher<'_>) -> Result<(), Error> {
    let mut state = state(root)?;
    let result = run(root, api, &mut state, publish);
    state.error = result.as_ref().err().map(|e| e.message().to_owned());
    save(root, &state)?;
    result
}
fn checked(status: u16, body: Value) -> Result<Value, Error> {
    match status {
        200 => Ok(body),
        401 => Err(Error::Key),
        403 => Err(Error::Access),
        429 => Err(Error::Limited),
        500..=599 => Err(Error::Unavailable),
        _ => Err(Error::Response),
    }
}
fn next_cursor(
    more: bool,
    cursor: Option<String>,
    seen: &mut BTreeSet<String>,
) -> Result<Option<String>, Error> {
    if !more {
        return Ok(None);
    }
    let cursor = cursor
        .filter(|s| !s.is_empty() && s.len() <= 8192)
        .ok_or(Error::Response)?;
    if !seen.insert(cursor.clone()) {
        return Err(Error::Response);
    }
    Ok(Some(cursor))
}
fn run(
    root: &Path,
    api: &mut dyn Api,
    state: &mut State,
    publish: &mut Publisher<'_>,
) -> Result<(), Error> {
    let legacy = legacy_ids(root)?;
    let mut cursor: Option<String> = None;
    let mut seen = BTreeSet::new();
    let mut watermark = None;
    loop {
        if !crate::work_continues() {
            return Err(Error::Stopped);
        }
        let mut query = vec![("page_size", "30".to_owned())];
        if let Some(since) = &state.watermark {
            query.push(("updated_after", since.clone()));
        }
        if let Some(value) = &cursor {
            query.push(("cursor", value.clone()));
        }
        let (status, body) = api.get("/v1/notes", &query)?;
        let page: Page =
            serde_json::from_value(checked(status, body)?).map_err(|_| Error::Response)?;
        if cursor.is_none() {
            // The first page bounds this scan in Granola's clock. Later pages
            // may include edits made during a long initial history sync.
            watermark = page
                .notes
                .iter()
                .map(|note| instant(&note.updated_at))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .max();
        }
        for summary in page.notes {
            let id = note_id(&summary.id)?;
            instant(&summary.updated_at)?;
            if summary.deleted_at.is_some()
                || state.notes.get(&id).is_some_and(|p| {
                    p.removed || p.legacy || p.revision.as_deref() == Some(&summary.updated_at)
                })
            {
                continue;
            }
            let note = fetch_note(api, &summary.id)?;
            if note_id(&note.id)? != id {
                return Err(Error::Response);
            }
            if note.deleted_at.is_some() {
                continue;
            }
            instant(&note.updated_at)?;
            let transcript = note.transcript.as_ref().ok_or(Error::Transcript)?;
            let origin = validate_note(&note, transcript)?;
            if !state.notes.contains_key(&id) {
                let old_id = note.web_url.rsplit('/').next().and_then(legacy_id);
                let local = origin.with_timezone(&solstone_core_journal_config::owner_zone(root));
                let end = transcript
                    .iter()
                    .map(|t| instant(&t.end_time))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .max()
                    .unwrap_or(origin);
                let seconds = ((end - origin).num_milliseconds() + 999)
                    .div_euclid(1000)
                    .max(1);
                state.notes.insert(
                    id.clone(),
                    Placement {
                        day: local.format("%Y%m%d").to_string(),
                        segment: format!("{}_{seconds}", local.format("%H%M%S")),
                        origin: origin.to_rfc3339(),
                        revision: None,
                        removed: false,
                        placed: false,
                        legacy: legacy.contains(&id)
                            || old_id.as_ref().is_some_and(|id| legacy.contains(id)),
                    },
                );
                save(root, state)?;
            }
            let placement = state.notes.get_mut(&id).expect("placement inserted");
            if placement.legacy {
                continue;
            }
            let directory = directory(root, &id, placement)?;
            if solstone_core_segment::owner_deleted(&directory).map_err(storage)?
                || (placement.revision.is_some() && !directory.join(OUTPUT).exists())
                || (placement.placed && !directory.exists())
            {
                placement.removed = true;
                save(root, state)?;
                continue;
            }
            let rows = rows(
                &note,
                placement,
                solstone_core_journal_config::owner_zone(root),
            )?;
            create_directory_with_mode(&directory, 0o700).map_err(storage)?;
            if !placement.placed {
                placement.placed = true;
                save(root, state)?;
            }
            let placement = state.notes.get_mut(&id).expect("placement retained");
            solstone_core_segment::output_attempt::prepare_segment(&directory).map_err(storage)?;
            let output = directory.join(OUTPUT);
            let claims = solstone_core_segment::output_attempt::OutputClaims::acquire(
                [output.clone()],
                LockOptions::default(),
            )
            .map_err(storage)?;
            {
                let _commit = match claims.commit(&directory) {
                    Ok(commit) => commit,
                    Err(e) => {
                        if solstone_core_segment::owner_deleted(&directory).map_err(storage)? {
                            placement.removed = true;
                            save(root, state)?;
                            continue;
                        }
                        return Err(storage(e));
                    }
                };
                write_jsonl(&output, rows, AtomicWriteOptions { mode: Some(0o600) })
                    .map_err(storage)?;
            }
            drop(claims);
            publish(root, &id, placement, &output)?;
            placement.revision = Some(note.updated_at.clone());
            state.last_added = Some(Utc::now().to_rfc3339());
            save(root, state)?;
        }
        cursor = next_cursor(page.more, page.cursor, &mut seen)?;
        if cursor.is_none() {
            break;
        }
    }
    // A small overlap includes edits at the strict updated_after boundary.
    if let Some(watermark) = watermark {
        state.watermark = Some(
            (watermark - Duration::minutes(2)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        );
    }
    state.last_sync = Some(Utc::now().to_rfc3339());
    Ok(())
}
fn fetch_note(api: &mut dyn Api, id: &str) -> Result<Note, Error> {
    note_id(id)?;
    let path = format!("/v1/notes/{id}");
    let (status, body) = api.get(&path, &[("include", "transcript".into())])?;
    if status != 413 {
        return serde_json::from_value(checked(status, body)?).map_err(|_| Error::Response);
    }
    let (status, body) = api.get(&path, &[])?;
    let mut note: Note =
        serde_json::from_value(checked(status, body)?).map_err(|_| Error::Response)?;
    let mut turns = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = BTreeSet::new();
    loop {
        let mut query = vec![("page_size", "100".into())];
        if let Some(value) = cursor {
            query.push(("cursor", value));
        }
        let (status, body) = api.get(&format!("{path}/transcript"), &query)?;
        let body = checked(status, body)?;
        #[derive(Deserialize)]
        struct TranscriptPage {
            transcript: Vec<Turn>,
            #[serde(rename = "hasMore")]
            more: bool,
            cursor: Option<String>,
        }
        let page: TranscriptPage = serde_json::from_value(body).map_err(|_| Error::Response)?;
        turns.extend(page.transcript);
        cursor = next_cursor(page.more, page.cursor, &mut seen)?;
        if cursor.is_none() {
            break;
        }
    }
    note.transcript = Some(turns);
    Ok(note)
}
fn validate_note(note: &Note, turns: &[Turn]) -> Result<DateTime<Utc>, Error> {
    let created = instant(&note.created_at)?;
    for person in &note.attendees {
        if !person.get("email").is_some_and(Value::is_string)
            || person
                .get("name")
                .is_some_and(|v| !v.is_null() && !v.is_string())
        {
            return Err(Error::Response);
        }
    }
    if note.calendar_event.as_ref().is_some_and(|v| !v.is_object()) {
        return Err(Error::Response);
    }

    let mut start = None;
    for turn in turns {
        let at = instant(&turn.start_time)?;
        for field in ["name", "diarization_label", "attribution"] {
            if turn
                .speaker
                .get(field)
                .is_some_and(|v| !v.is_null() && !v.is_string())
            {
                return Err(Error::Response);
            }
        }

        if instant(&turn.end_time)? < at
            || !turn.speaker.get("source").is_some_and(Value::is_string)
        {
            return Err(Error::Response);
        }
        start = Some(start.map_or(at, |old: DateTime<Utc>| old.min(at)));
    }
    let calendar_start = note
        .calendar_event
        .as_ref()
        .and_then(|v| v.get("scheduled_start_time"))
        .and_then(Value::as_str)
        .map(instant)
        .transpose()?;
    Ok(start.or(calendar_start).unwrap_or(created))
}
fn directory(root: &Path, id: &str, p: &Placement) -> Result<PathBuf, Error> {
    Ok(SegmentDir::resolve(root, &p.day, &p.segment, &stream(id))
        .map_err(storage)?
        .path()
        .to_path_buf())
}
fn rows(note: &Note, placement: &Placement, zone: chrono_tz::Tz) -> Result<Vec<Value>, Error> {
    let origin = instant(&placement.origin)?;
    let mut metadata = serde_json::to_value(note).map_err(storage)?;
    metadata
        .as_object_mut()
        .expect("note object")
        .remove("transcript");
    let mut rows = vec![
        json!({(solstone_core_format::content::granola::HEADER_KEY):metadata,"granola_start_ms":origin.timestamp_millis()}),
    ];
    for turn in note.transcript.as_ref().ok_or(Error::Transcript)? {
        let start = instant(&turn.start_time)?;
        let speaker = turn
            .speaker
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|name| format!("Granola: {name}"))
            .unwrap_or_else(|| {
                let label = ["diarization_label", "attribution", "source"]
                    .iter()
                    .find_map(|field| {
                        turn.speaker
                            .get(*field)
                            .and_then(Value::as_str)
                            .filter(|value| !value.is_empty())
                    })
                    .unwrap_or("unknown");
                format!("Granola: {label}")
            });
        let clock = start.with_timezone(&zone);
        rows.push(json!({"start":clock.format("%H:%M:%S%.3f").to_string(),"source_ms":start.timestamp_millis(),"end_ms":instant(&turn.end_time)?.timestamp_millis(),"granola_start_time":turn.start_time,"granola_end_time":turn.end_time,"speaker":speaker,"text":turn.text,"source":"granola","granola_speaker":turn.speaker}));
    }
    Ok(rows)
}
fn publish(root: &Path, id: &str, placement: &Placement, output: &Path) -> Result<(), Error> {
    let segments = [crate::CreatedSegment {
        day: placement.day.clone(),
        segment: placement.segment.clone(),
        stream: stream(id),
        hints: StreamHints {
            kind: Some(Kind::Imported(ImportSource::Named("granola".into()))),
            host: None,
            platform: None,
        },
    }];
    let record = crate::publish::publish(crate::publish::PublicationInput {
        journal: root,
        import_dir: None,
        import_id: id,
        importer: "granola",
        revision: None,
        segments: &segments,
        files_created: &[output.to_path_buf()],
        index_only: &[],
        may_write_record: None,
    })
    .map_err(storage)?;
    if record.status != crate::publish::PublicationStatus::Success {
        return Err(Error::Storage);
    }
    Ok(())
}
fn legacy_ids(root: &Path) -> Result<BTreeSet<String>, Error> {
    let mut ids = BTreeSet::new();
    for (day, _) in solstone_core_segment::list_days(root).map_err(storage)? {
        for segment in solstone_core_segment::list_stream_segments(root, &day, "import.granola")
            .map_err(storage)?
        {
            for name in ["conversation_transcript.jsonl", "audio.jsonl"] {
                let path = segment.path().join(name);
                let file = match fs::File::open(path) {
                    Ok(file) => file,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => {
                        log::warn!("Granola could not match a prior imported transcript");
                        continue;
                    }
                };
                let mut line = String::new();
                if std::io::BufReader::new(file).read_line(&mut line).is_err() {
                    log::warn!("Granola could not match a prior imported transcript");
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    log::warn!("Granola could not match a prior imported transcript");
                    continue;
                };
                if let Some(id) = value
                    .get("imported")
                    .and_then(|v| v.get("id"))
                    .and_then(Value::as_str)
                    .and_then(legacy_id)
                {
                    ids.insert(id);
                }
            }
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    #[test]
    fn captured_api_turns_keep_their_words_times_and_attribution() {
        // A public-API response from fictional meeting audio, with the test
        // identity redacted and its transcript bounded to the first two turns.
        let fixture: Value =
            serde_json::from_str(include_str!("granola/fixtures/note.json")).unwrap();
        let note: Note = serde_json::from_value(fixture.clone()).unwrap();
        let origin = validate_note(&note, note.transcript.as_ref().unwrap()).unwrap();
        let placement = Placement {
            day: "20261111".into(),
            segment: "000000_300".into(),
            origin: origin.to_rfc3339(),
            revision: None,
            removed: false,
            legacy: false,
            placed: false,
        };
        let saved = rows(&note, &placement, chrono_tz::UTC).unwrap();
        for (source, row) in fixture["transcript"]
            .as_array()
            .unwrap()
            .iter()
            .zip(saved.iter().skip(1))
        {
            assert_eq!(row["text"], source["text"]);
            assert_eq!(row["granola_start_time"], source["start_time"]);
            assert_eq!(row["granola_end_time"], source["end_time"]);
            assert_eq!(row["granola_speaker"], source["speaker"]);
        }
        assert_eq!(
            saved.len(),
            fixture["transcript"].as_array().unwrap().len() + 1
        );
        let mut nullable = note;
        let speaker = &mut nullable.transcript.as_mut().unwrap()[0].speaker;
        speaker["diarization_label"] = Value::Null;
        speaker["name"] = json!("");
        let saved = rows(&nullable, &placement, chrono_tz::UTC).unwrap();
        assert_eq!(saved[1]["speaker"], "Granola: them");
    }

    #[test]
    fn a_stored_message_reads_back_as_its_kind() {
        let mut codes = BTreeSet::new();
        for error in Error::ALL {
            assert_eq!(Error::from_message(error.message()), Some(error));
            assert!(codes.insert(error.code()));
        }
        assert_eq!(Error::from_message("an older wording"), None);
    }

    #[test]
    fn a_key_check_reads_granolas_answer() {
        struct Answer(u16);
        impl Api for Answer {
            fn get(&mut self, path: &str, query: &[(&str, String)]) -> Result<(u16, Value), Error> {
                assert_eq!(path, "/v1/notes");
                assert_eq!(query, [("page_size", "1".to_owned())]);
                Ok((self.0, json!({"notes":[],"hasMore":false})))
            }
        }
        assert_eq!(check_with(&mut Answer(200)), Ok(()));
        assert_eq!(check_with(&mut Answer(401)), Err(Error::Key));
        assert_eq!(check_with(&mut Answer(403)), Err(Error::Access));
        assert_eq!(check_with(&mut Answer(503)), Err(Error::Unavailable));
        assert_eq!(check_key("not-a-key"), Err(Error::InvalidKey));
    }

    #[test]
    fn bad_ids_and_cursor_cycles_are_errors() {
        for id in ["../other", "not_1d3tmYTlCICgj/", "éaa_abcdefghijklm"] {
            assert!(note_id(id).is_err());
        }
        let mut seen = BTreeSet::new();
        assert_eq!(
            next_cursor(true, Some("next".into()), &mut seen).unwrap(),
            Some("next".into())
        );
        assert_eq!(
            next_cursor(true, Some("next".into()), &mut seen),
            Err(Error::Response)
        );
        assert_eq!(next_cursor(true, None, &mut seen), Err(Error::Response));
    }
}
