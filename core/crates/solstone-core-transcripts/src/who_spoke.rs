// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Who spoke each transcript line, for the readers that show it.
//!
//! A reader that shows who spoke names a segment's lines from its speaker
//! label rows with the owner's own answers (`talents/owner_line_labels.json`)
//! applied. Voice, evidence, discovery, naming and accumulation code reads the
//! labels file alone and never calls this loader: owner-voice mode removes
//! audio based on voice evidence, and an owner's answer about who spoke is not
//! voice evidence. A repository source census lists the readers allowed to
//! call it.
//!
//! An answer applies only to the source the reader renders, and only while
//! the line at its statement ordinal is still the line it was given for: the
//! same start and the same text, and a stored sentence id, if the line has
//! one, equal to that ordinal (`line_coordinate`). Otherwise it is
//! inert and the line reads from the journal's own row, as if there were no
//! answer.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_speaker_id::owner_line_labels::{
    LineFingerprint, OwnerLineEntry, OwnerLineLabels, decode_owner_line_labels,
    owner_line_labels_path, read_owner_line_labels,
};

use crate::{line_coordinate, next_statement_ordinal};

/// The method an owner's answer reads with in a who-spoke row.
pub const OWNER_ANSWER_METHOD: &str = "user_labeled";

/// What the segment's own `speaker_labels.json` held for this read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalLabels {
    /// No labels file.
    Absent,
    /// A labels file with a `labels` list.
    Loaded,
    /// A labels file that can't be read, isn't a labels record, or is larger
    /// than the reader's bound.
    Damaged,
}

/// One owner answer that applies to a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerAnswer {
    /// The entity the owner named, or `None` for someone they don't know.
    pub speaker: Option<String>,
    /// When the owner gave the answer.
    pub set_at: String,
}

/// The owner's answers file exists but can't be read, isn't a valid record,
/// or is larger than the reader's bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnreadableAnswers {
    pub file: PathBuf,
    pub detail: String,
}

/// The rows a who-spoke reader names a segment's lines from.
#[derive(Clone, Debug, PartialEq)]
pub struct WhoSpoke {
    pub labels: JournalLabels,
    /// Set when the owner's answers can't be read. Every line of the segment
    /// is then an unnamed voice: `rows` and `owner_set` are empty.
    pub owner_unreadable: Option<UnreadableAnswers>,
    /// The journal's rows with every applying answer in place of the row for
    /// its line, or added where the journal had none. An answer's row has the
    /// line's `sentence_id`, its `speaker` (an entity id or `null`),
    /// `"confidence": "high"` and [`OWNER_ANSWER_METHOD`].
    pub rows: Vec<Map<String, Value>>,
    /// The answers that apply, by sentence id.
    pub owner_set: BTreeMap<i64, OwnerAnswer>,
}

impl WhoSpoke {
    /// Whether the segment says anything about who spoke: journal labels, an
    /// applying owner answer, or owner answers that can't be read.
    pub fn present(&self) -> bool {
        self.labels != JournalLabels::Absent
            || !self.owner_set.is_empty()
            || self.owner_unreadable.is_some()
    }
}

/// Load who spoke each line of `source` in one segment. `source` is the audio
/// source stem the reader renders names on; without one no answer applies.
/// `max_bytes` bounds the reads of both the labels file and the owner's
/// answers; either file over it reads as damaged or unreadable.
pub fn load_who_spoke(
    segment_dir: &Path,
    source: Option<&str>,
    max_bytes: Option<usize>,
) -> WhoSpoke {
    let (labels, mut rows) = journal_rows(segment_dir, max_bytes);
    let entries = match owner_entries(segment_dir, max_bytes) {
        Ok(entries) => entries,
        Err(detail) => {
            return WhoSpoke {
                labels,
                owner_unreadable: Some(UnreadableAnswers {
                    file: owner_line_labels_path(segment_dir),
                    detail,
                }),
                rows: Vec::new(),
                owner_set: BTreeMap::new(),
            };
        }
    };
    let owner_set = match source {
        Some(source) => applying(segment_dir, source, entries),
        None => BTreeMap::new(),
    };
    for (sentence_id, answer) in &owner_set {
        let line = |row: &Map<String, Value>| {
            row.get("sentence_id").and_then(Value::as_i64) == Some(*sentence_id)
        };
        match rows.iter().position(line) {
            Some(first) => {
                rows[first] = answer_row(*sentence_id, answer);
                let mut index = 0;
                rows.retain(|row| {
                    let keep = index == first || !line(row);
                    index += 1;
                    keep
                });
            }
            None => rows.push(answer_row(*sentence_id, answer)),
        }
    }
    WhoSpoke {
        labels,
        owner_unreadable: None,
        rows,
        owner_set,
    }
}

fn answer_row(sentence_id: i64, answer: &OwnerAnswer) -> Map<String, Value> {
    Map::from_iter([
        ("sentence_id".to_owned(), Value::from(sentence_id)),
        (
            "speaker".to_owned(),
            answer.speaker.clone().map_or(Value::Null, Value::String),
        ),
        ("confidence".to_owned(), Value::from("high")),
        ("method".to_owned(), Value::from(OWNER_ANSWER_METHOD)),
    ])
}

/// Read a whole file, or up to `max_bytes`; `Ok(None)` when it is absent.
fn read_bounded(path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > max_bytes {
        return Err(format!("larger than {max_bytes} bytes"));
    }
    Ok(Some(bytes))
}

fn journal_rows(
    segment_dir: &Path,
    max_bytes: Option<usize>,
) -> (JournalLabels, Vec<Map<String, Value>>) {
    let path = segment_dir.join("talents").join("speaker_labels.json");
    if !path.is_file() {
        return (JournalLabels::Absent, Vec::new());
    }
    let bytes = match max_bytes {
        Some(max_bytes) => read_bounded(&path, max_bytes).ok().flatten(),
        None => std::fs::read(&path).ok(),
    };
    let rows = bytes
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|payload| match payload {
            Value::Object(mut payload) => match payload.remove("labels") {
                Some(Value::Array(rows)) => Some(rows),
                _ => None,
            },
            _ => None,
        });
    match rows {
        Some(rows) => (
            JournalLabels::Loaded,
            rows.into_iter()
                .filter_map(|row| match row {
                    Value::Object(row) => Some(row),
                    _ => None,
                })
                .collect(),
        ),
        None => (JournalLabels::Damaged, Vec::new()),
    }
}

fn owner_entries(
    segment_dir: &Path,
    max_bytes: Option<usize>,
) -> Result<Vec<OwnerLineEntry>, String> {
    let Some(max_bytes) = max_bytes else {
        return match read_owner_line_labels(segment_dir) {
            OwnerLineLabels::Absent => Ok(Vec::new()),
            OwnerLineLabels::Present(entries) => Ok(entries),
            OwnerLineLabels::Unreadable(detail) => Err(detail),
        };
    };
    match read_bounded(&owner_line_labels_path(segment_dir), max_bytes)? {
        None => Ok(Vec::new()),
        Some(bytes) => decode_owner_line_labels(&bytes),
    }
}

/// The entries for `source` whose line is still the line they were given for.
fn applying(
    segment_dir: &Path,
    source: &str,
    entries: Vec<OwnerLineEntry>,
) -> BTreeMap<i64, OwnerAnswer> {
    let mut wanted = entries
        .into_iter()
        .filter(|entry| entry.source == source)
        .map(|entry| (entry.sentence_id, entry))
        .collect::<BTreeMap<_, _>>();
    let Some(last) = wanted.keys().next_back().copied() else {
        return BTreeMap::new();
    };
    let Ok(file) = File::open(segment_dir.join(format!("{source}.jsonl"))) else {
        return BTreeMap::new();
    };
    let mut applying = BTreeMap::new();
    let mut ordinal = 0;
    for line in BufReader::new(file).split(b'\n') {
        let Ok(line) = line else {
            break;
        };
        let line = String::from_utf8_lossy(&line);
        let Some(id) = next_statement_ordinal(&line, &mut ordinal) else {
            continue;
        };
        if let Some(entry) = wanted.remove(&id)
            && let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line.trim())
            && line_coordinate(&record, Some(id)) == Some(id)
            && LineFingerprint::of_record(&record) == entry.fingerprint
        {
            applying.insert(
                id,
                OwnerAnswer {
                    speaker: entry.speaker,
                    set_at: entry.set_at,
                },
            );
        }
        if id >= last {
            break;
        }
    }
    applying
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use serde_json::json;
    use solstone_core_journal_io::paths::Segment;
    use solstone_core_speaker_id::owner_line_labels::{
        LineFingerprint, OwnerLineEntry, encode_owner_line_labels, owner_line_labels_path,
    };

    use super::*;
    use crate::{
        AgentSpeakers, Sources, TalentSource, VoiceNames, cluster_span, owner_voice_lines,
        read_segment_transcript_page,
    };

    const DAY: &str = "20260731";
    const STREAM: &str = "device";
    const KEY: &str = "090000_60";

    /// Three lines: Quinn by voice, the owner by a confirmed owner match, and
    /// Quinn again.
    const LINES: [&str; 3] = [
        r#"{"start":"00:00:01","speaker":1,"sentence_id":1,"text":"Let us begin"}"#,
        r#"{"start":"00:00:02","speaker":2,"sentence_id":2,"text":"I will send it"}"#,
        r#"{"start":"00:00:03","speaker":1,"sentence_id":3,"text":"Sounds good"}"#,
    ];

    fn attributed_segment(root: &Path) -> PathBuf {
        let dir = root.join("chronicle").join(DAY).join(STREAM).join(KEY);
        fs::create_dir_all(dir.join("talents")).unwrap();
        let mut jsonl = String::from("{\"raw\":\"audio.flac\"}\n");
        for line in LINES {
            jsonl.push_str(line);
            jsonl.push('\n');
        }
        fs::write(dir.join("audio.jsonl"), jsonl).unwrap();
        fs::write(
            dir.join("talents/speaker_labels.json"),
            json!({"labels": [
                {"sentence_id": 1, "speaker": "quinn", "method": "acoustic", "confidence": "high"},
                {"sentence_id": 2, "speaker": "owner", "method": "owner_centroid", "confidence": "high"},
                {"sentence_id": 3, "speaker": "quinn", "method": "acoustic", "confidence": "high"}
            ]})
            .to_string(),
        )
        .unwrap();
        dir
    }

    fn entry(line: usize, speaker: Option<&str>) -> OwnerLineEntry {
        let Value::Object(record) = serde_json::from_str(LINES[line - 1]).unwrap() else {
            unreachable!()
        };
        OwnerLineEntry {
            source: "audio".to_owned(),
            sentence_id: line as i64,
            fingerprint: LineFingerprint::of_record(&record),
            speaker: speaker.map(str::to_owned),
            set_at: "2026-10-31T12:00:00Z".to_owned(),
        }
    }

    /// The owner says Quinn's first line was theirs, their own line was Pat's,
    /// and Quinn's second line was someone they don't know.
    fn answer(dir: &Path) {
        write_answers(
            dir,
            &[
                entry(1, Some("owner")),
                entry(2, Some("pat")),
                entry(3, None),
            ],
        );
    }

    fn write_answers(dir: &Path, entries: &[OwnerLineEntry]) {
        fs::write(
            owner_line_labels_path(dir),
            encode_owner_line_labels(entries),
        )
        .unwrap();
    }

    fn voices() -> VoiceNames {
        VoiceNames {
            principal: "owner".into(),
            owner_voice_confirmed: true,
            people: [
                ("quinn".to_owned(), "Quinn".to_owned()),
                ("pat".to_owned(), "Pat".to_owned()),
            ]
            .into(),
        }
    }

    fn talent_view(root: &Path) -> String {
        let sources = Sources {
            transcripts: true,
            percepts: false,
            talents: TalentSource::Disabled,
            voices: Some(voices()),
            heard_owner: None,
        };
        cluster_span(root, DAY, &[KEY], &sources, Some(STREAM))
            .unwrap()
            .0
            .lines()
            .filter(|line| line.starts_with("[00:"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn agent_page(root: &Path) -> Vec<String> {
        let segment = Segment::resolve_exact(root, DAY, STREAM, KEY)
            .unwrap()
            .unwrap();
        let speakers = AgentSpeakers {
            voices: voices(),
            owner: "The owner".to_owned(),
        };
        read_segment_transcript_page(&segment, None, Some(&speakers), None)
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| entry.text)
            .collect()
    }

    /// Every reader in this crate, as one comparable value.
    fn readers(root: &Path, dir: &Path) -> (String, Vec<String>, Vec<String>) {
        (
            talent_view(root),
            agent_page(root),
            owner_voice_lines(dir, &voices()),
        )
    }

    type Readers = (String, Vec<String>, Vec<String>);

    fn lines(lines: [&str; 3]) -> Vec<String> {
        lines.into_iter().map(str::to_owned).collect()
    }

    /// What every reader showed for the attributed segment before the owner's
    /// answers existed.
    fn attributed_before() -> Readers {
        (
            "[00:00:01] Quinn: Let us begin\n[00:00:02] You: I will send it\n[00:00:03] Quinn: Sounds good"
                .to_owned(),
            lines([
                "[00:00:01] Quinn: Let us begin",
                "[00:00:02] The owner: I will send it",
                "[00:00:03] Quinn: Sounds good",
            ]),
            vec!["I will send it".to_owned()],
        )
    }

    /// What every reader showed for a segment nobody was named on.
    fn unnamed_before() -> Readers {
        (
            "[00:00:01] Speaker 1: Let us begin\n[00:00:02] Speaker 2: I will send it\n[00:00:03] Speaker 1: Sounds good"
                .to_owned(),
            lines([
                "[00:00:01] Speaker 1: Let us begin",
                "[00:00:02] Speaker 2: I will send it",
                "[00:00:03] Speaker 1: Sounds good",
            ]),
            Vec::new(),
        )
    }

    #[test]
    fn who_spoke_readers_follow_the_owners_answers() {
        let root = tempfile::tempdir().unwrap();
        let dir = attributed_segment(root.path());
        assert_eq!(readers(root.path(), &dir), attributed_before());

        answer(&dir);
        let (talent, page, said) = readers(root.path(), &dir);
        assert_eq!(
            talent,
            "[00:00:01] You: Let us begin\n[00:00:02] Pat: I will send it\n[00:00:03] Speaker 1: Sounds good"
        );
        assert_eq!(
            page,
            lines([
                "[00:00:01] The owner: Let us begin",
                "[00:00:02] Pat: I will send it",
                "[00:00:03] Speaker 1: Sounds good",
            ])
        );
        // The line the owner doesn't know names no one, by name or by id.
        for line in [talent.lines().nth(2).unwrap(), page[2].as_str()] {
            assert!(!line.to_lowercase().contains("quinn"), "{line}");
        }
        // What the owner said is the line they claimed, not the one now Pat's.
        assert_eq!(said, vec!["Let us begin".to_owned()]);

        fs::remove_file(owner_line_labels_path(&dir)).unwrap();
        assert_eq!(readers(root.path(), &dir), attributed_before());
        write_answers(&dir, &[]);
        assert_eq!(readers(root.path(), &dir), attributed_before());
    }

    #[test]
    fn lines_that_store_no_sentence_id_are_named_by_their_ordinal_in_every_reader() {
        let root = tempfile::tempdir().unwrap();
        let dir = attributed_segment(root.path());
        let mut jsonl = String::from("{\"raw\":\"audio.flac\"}\n");
        for (index, line) in LINES.iter().enumerate() {
            jsonl.push_str(&line.replace(&format!("\"sentence_id\":{},", index + 1), ""));
            jsonl.push('\n');
        }
        assert!(!jsonl.contains("sentence_id"));
        fs::write(dir.join("audio.jsonl"), jsonl).unwrap();
        assert_eq!(readers(root.path(), &dir), attributed_before());

        answer(&dir);
        let (talent, page, said) = readers(root.path(), &dir);
        assert_eq!(
            talent,
            "[00:00:01] You: Let us begin\n[00:00:02] Pat: I will send it\n[00:00:03] Speaker 1: Sounds good"
        );
        assert_eq!(page[0], "[00:00:01] The owner: Let us begin");
        assert_eq!(page[1], "[00:00:02] Pat: I will send it");
        assert_eq!(said, vec!["Let us begin".to_owned()]);
        assert_eq!(load_who_spoke(&dir, Some("audio"), None).owner_set.len(), 3);
    }

    #[test]
    fn an_answer_names_a_line_on_a_segment_without_labels() {
        let root = tempfile::tempdir().unwrap();
        let dir = attributed_segment(root.path());
        fs::remove_file(dir.join("talents/speaker_labels.json")).unwrap();
        assert_eq!(readers(root.path(), &dir), unnamed_before());

        write_answers(&dir, &[entry(1, Some("pat"))]);
        let (talent, page, said) = readers(root.path(), &dir);
        assert!(
            talent.starts_with("[00:00:01] Pat: Let us begin\n"),
            "{talent}"
        );
        assert_eq!(page[0], "[00:00:01] Pat: Let us begin");
        assert_eq!(page[1..], unnamed_before().1[1..]);
        assert!(said.is_empty());
    }

    #[test]
    fn unreadable_answers_leave_every_line_unnamed() {
        let root = tempfile::tempdir().unwrap();
        let dir = attributed_segment(root.path());
        fs::write(owner_line_labels_path(&dir), "not a record").unwrap();
        assert_eq!(readers(root.path(), &dir), unnamed_before());
        assert!(
            load_who_spoke(&dir, Some("audio"), None)
                .owner_unreadable
                .is_some()
        );

        // Valid answers over the agent page's bound read as unreadable there.
        let mut oversize =
            encode_owner_line_labels(&[entry(1, Some("owner")), entry(2, Some("pat"))]);
        oversize.resize(crate::MAX_TRANSCRIPT_PAGE_BYTES + 1, b' ');
        fs::write(owner_line_labels_path(&dir), oversize).unwrap();
        assert_eq!(agent_page(root.path()), unnamed_before().1);

        fs::remove_file(owner_line_labels_path(&dir)).unwrap();
        assert_eq!(readers(root.path(), &dir), attributed_before());
    }

    #[test]
    fn an_answer_for_a_changed_line_or_another_source_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let dir = attributed_segment(root.path());
        let mut retranscribed = entry(1, Some("pat"));
        retranscribed.fingerprint.text_sha256 = "0".repeat(64);
        let mut moved = entry(2, Some("pat"));
        moved.fingerprint.start = json!("00:00:09");
        let mut renumbered = entry(3, None);
        renumbered.sentence_id = 4;
        write_answers(&dir, &[retranscribed, moved, renumbered]);
        assert_eq!(readers(root.path(), &dir), attributed_before());

        // A line whose stored sentence id no longer matches its ordinal.
        let shifted = LINES[0].replace("\"sentence_id\":1", "\"sentence_id\":7");
        let jsonl = fs::read_to_string(dir.join("audio.jsonl")).unwrap();
        fs::write(dir.join("audio.jsonl"), jsonl.replace(LINES[0], &shifted)).unwrap();
        let unanswered = readers(root.path(), &dir);
        answer(&dir);
        let mut answered = readers(root.path(), &dir);
        answered.0 = answered.0.lines().next().unwrap().to_owned();
        assert_eq!(answered.0, unanswered.0.lines().next().unwrap());
        assert_eq!(answered.1[0], unanswered.1[0]);
        fs::write(dir.join("audio.jsonl"), jsonl).unwrap();

        // Embeddings added for another source after the answers were given.
        answer(&dir);
        fs::write(dir.join("a_audio.npz"), b"").unwrap();
        let answered = readers(root.path(), &dir);
        fs::remove_file(owner_line_labels_path(&dir)).unwrap();
        assert_eq!(answered, readers(root.path(), &dir));
    }

    #[test]
    fn the_loader_puts_answers_in_place_of_rows_and_bounds_both_files() {
        let root = tempfile::tempdir().unwrap();
        let dir = attributed_segment(root.path());
        fs::write(
            dir.join("talents/speaker_labels.json"),
            json!({"labels": [
                {"sentence_id": 2, "speaker": "owner", "method": "owner_centroid"},
                {"sentence_id": 1, "speaker": "quinn", "method": "acoustic"},
                {"sentence_id": 2, "speaker": "quinn", "method": "acoustic"}
            ]})
            .to_string(),
        )
        .unwrap();
        write_answers(&dir, &[entry(2, Some("pat")), entry(3, None)]);
        let row = |id: i64, speaker: Value| json!({"sentence_id": id, "speaker": speaker, "confidence": "high", "method": OWNER_ANSWER_METHOD});
        let who = load_who_spoke(&dir, Some("audio"), None);
        assert_eq!(who.labels, JournalLabels::Loaded);
        assert_eq!(
            who.rows.into_iter().map(Value::Object).collect::<Vec<_>>(),
            vec![
                row(2, json!("pat")),
                json!({"sentence_id": 1, "speaker": "quinn", "method": "acoustic"}),
                row(3, Value::Null),
            ]
        );
        assert_eq!(who.owner_set.keys().copied().collect::<Vec<_>>(), [2, 3]);
        assert_eq!(who.owner_set[&3].speaker, None);

        // No answer applies without a rendered source.
        let who = load_who_spoke(&dir, None, None);
        assert!(who.owner_set.is_empty());
        assert_eq!(who.rows.len(), 3);

        // Damaged or oversize labels leave the owner's rows only.
        let answers = fs::metadata(owner_line_labels_path(&dir)).unwrap().len() as usize;
        let oversize = format!("{{\"labels\":[]}}{}", " ".repeat(answers));
        for (labels, bound) in [("not json", None), (oversize.as_str(), Some(answers))] {
            fs::write(dir.join("talents/speaker_labels.json"), labels).unwrap();
            let who = load_who_spoke(&dir, Some("audio"), bound);
            assert_eq!(who.labels, JournalLabels::Damaged);
            assert_eq!(
                who.rows.into_iter().map(Value::Object).collect::<Vec<_>>(),
                vec![row(2, json!("pat")), row(3, Value::Null)]
            );
        }

        fs::remove_file(dir.join("talents/speaker_labels.json")).unwrap();
        let who = load_who_spoke(&dir, Some("audio"), None);
        assert_eq!(who.labels, JournalLabels::Absent);
        assert!(who.present());
        assert_eq!(who.rows.len(), 2);

        // The owner's answers over the bound read as unreadable.
        let who = load_who_spoke(&dir, Some("audio"), Some(16));
        assert!(who.owner_unreadable.is_some());
        assert!(who.rows.is_empty() && who.owner_set.is_empty());
        assert!(who.present());

        fs::remove_file(owner_line_labels_path(&dir)).unwrap();
        assert!(!load_who_spoke(&dir, Some("audio"), None).present());
    }
}
