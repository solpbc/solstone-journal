// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Issuer for transcript segment links attached to search and fetch responses.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use solstone_core_format::content::{ContentResolution, Family, classify};
use solstone_core_indexer_query::AdmittedCategory;
use solstone_core_indexer_store::classification::{FacetDeclarationSet, classify_source};
use solstone_core_journal_io::is_day_key;
use solstone_core_journal_io::paths::{
    PathOrDay, Segment, iter_segments_skipping_unreadable_streams,
};
use solstone_core_transcripts::segment_clock;

use crate::dispatch::{
    DispatchPrincipal, boundary, codec, differing_capture_zone, segment_assignment_probe_path,
    source_label, source_labels,
};
use crate::permissions::ConnectionReadSnapshot;
use crate::possible_names::ConnectionPossibleNames;
use crate::references::{ReferenceTarget, SegmentReference};

pub(crate) trait TranscriptPageCheck {
    fn check_first_page(
        &mut self,
        journal_root: &Path,
        segment: &Segment,
    ) -> Result<Option<u64>, ()>;
}

/// Live first-page checks for one response. Who spoke is resolved once, on the
/// first check, so every link in the response
/// binds the same rendering. Possible names are the ones this connection's
/// `get_transcript` shows, so a link binds the version that read will see.
pub(crate) struct LiveTranscriptPages {
    speakers: Option<Option<solstone_core_transcripts::AgentSpeakers>>,
    possible_names: Arc<ConnectionPossibleNames>,
}

impl LiveTranscriptPages {
    pub(crate) fn new(possible_names: Arc<ConnectionPossibleNames>) -> Self {
        Self {
            speakers: None,
            possible_names,
        }
    }

    /// Checks that render who spoke exactly as the caller's own read did.
    pub(crate) fn with_speakers(
        speakers: Option<solstone_core_transcripts::AgentSpeakers>,
        possible_names: Arc<ConnectionPossibleNames>,
    ) -> Self {
        Self {
            speakers: Some(speakers),
            possible_names,
        }
    }
}

impl TranscriptPageCheck for LiveTranscriptPages {
    fn check_first_page(
        &mut self,
        journal_root: &Path,
        segment: &Segment,
    ) -> Result<Option<u64>, ()> {
        // The read numbers voices across segments whether or not this one has
        // labels, so the link resolves who spoke the same way on every segment.
        let speakers = self
            .speakers
            .get_or_insert_with(|| solstone_core_talent_runtime::agent_speakers(journal_root))
            .as_ref();
        let possible_names = self.possible_names.labels(segment.path());
        match solstone_core_transcripts::read_segment_transcript_page(
            segment,
            None,
            speakers,
            possible_names.as_ref(),
        ) {
            Ok(page) if !page.entries.is_empty() => Ok(Some(page.version.fingerprint())),
            Ok(_) => Ok(None),
            Err(_) => Err(()),
        }
    }
}

pub(crate) struct LinkSubject<'a> {
    pub path: &'a str,
    /// Chunk index the caller admitted. Links use `activity_record`, not this index.
    #[allow(dead_code)]
    pub idx: i64,
    /// Indexed prose the caller served. Links do not compare it to a fresh render.
    #[allow(dead_code)]
    pub text: &'a str,
    pub segments: Vec<Value>,
    pub more_segments: bool,
    /// Record taken from the activity file after its digest matched the index.
    /// Activity paths without it yield no links. Other paths ignore it.
    pub activity_record: Option<&'a Map<String, Value>>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn issue_segment_links(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    subjects: &mut [LinkSubject<'_>],
    per_result: usize,
    response_budget: usize,
    response_checks: &mut usize,
    checker: &mut dyn TranscriptPageCheck,
) {
    if !snapshot.categories.contains(&AdmittedCategory::Transcripts) {
        return;
    }
    let Ok(declarations) = FacetDeclarationSet::from_journal(journal_root) else {
        return;
    };
    let transcript_boundary = boundary(snapshot, Some(AdmittedCategory::Transcripts), None);
    let owner_zone = solstone_core_journal_config::owner_zone(journal_root);
    // Read once a link is issued: most subjects name no segment.
    let mut sources = None;

    let mut day_cache: BTreeMap<String, Vec<Segment>> = BTreeMap::new();

    for subject in subjects.iter_mut() {
        let candidates = resolve_candidates(journal_root, subject, &mut day_cache);
        let mut subject_checks = 0;
        for (day, candidate) in candidates {
            let Ok(identity) = candidate.record_identity() else {
                continue;
            };
            let Some(clock) = segment_clock(identity.name) else {
                continue;
            };
            let probe_path = segment_assignment_probe_path(&day, identity.stream, identity.name);
            let classification = classify_source(
                journal_root,
                &probe_path,
                Some(identity.stream),
                &declarations,
            );
            // Scope comes first: a segment this connection may not read leaves
            // no trace, not even in `more_segments`.
            if !transcript_boundary.allows_classification(&classification) {
                continue;
            }
            if !crate::source_release::check_segment_transcript_release(
                journal_root,
                &day,
                identity.stream,
                identity.name,
            ) {
                continue;
            }
            if subject_checks == per_result || *response_checks == response_budget {
                subject.more_segments = true;
                break;
            }
            subject_checks += 1;
            *response_checks += 1;
            if let Ok(Some(version)) = checker.check_first_page(journal_root, &candidate) {
                if !crate::source_release::check_segment_transcript_release(
                    journal_root,
                    &day,
                    identity.stream,
                    identity.name,
                ) {
                    continue;
                }
                let target = ReferenceTarget::Segment(SegmentReference {
                    day: day.clone(),
                    stream: identity.stream.to_string(),
                    segment: identity.name.to_string(),
                    version,
                    source_index: None,
                    byte_offset: None,
                    statement_ordinal: None,
                });
                if let Ok(token) = codec().mint(principal.connection, snapshot.generation, target) {
                    let mut link_val = json!({
                        "time": clock.label,
                        "source": source_label(
                            sources.get_or_insert_with(|| source_labels(journal_root)),
                            identity.stream,
                        ),
                        "reference": token,
                    });
                    if let Some(zone) = differing_capture_zone(
                        journal_root,
                        &owner_zone,
                        &day,
                        identity.name,
                        &candidate,
                    ) {
                        link_val["capture_zone"] = zone;
                    }
                    subject.segments.push(link_val);
                }
            }
        }
    }
}

/// Segments named by an activity record, under the same stream rules as
/// [`resolve_candidates`]. A missing stream stays missing: it is not rewritten
/// to the named stream `default`, and direct segments keep the `_default` identity.
pub(crate) fn activity_segment_candidates(
    journal_root: &Path,
    activity_path: &str,
    record: &Map<String, Value>,
) -> Vec<Segment> {
    let mut day_cache = BTreeMap::new();
    let subject = LinkSubject {
        path: activity_path,
        idx: 0,
        text: "",
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(record),
    };
    resolve_candidates(journal_root, &subject, &mut day_cache)
        .into_iter()
        .map(|(_, segment)| segment)
        .collect()
}

fn resolve_candidates(
    journal_root: &Path,
    subject: &LinkSubject<'_>,
    day_cache: &mut BTreeMap<String, Vec<Segment>>,
) -> Vec<(String, Segment)> {
    let rel_path = subject
        .path
        .strip_prefix("chronicle/")
        .unwrap_or(subject.path);
    let parts: Vec<&str> = rel_path.split('/').collect();

    // 1. Segment file: day-rooted path with at least 3 components whose third contains segment_key
    if parts.len() >= 3 && is_day_key(parts[0]) {
        if let Some(filename) = parts.last() {
            if *filename == "imported.md"
                || filename.ends_with("_transcript.md")
                || *filename == "workout.json"
            {
                return Vec::new();
            }
            if classify(subject.path) == ContentResolution::Indexed(Family::AgentMemory) {
                return Vec::new();
            }
            if subject.path.contains("agent-memory-") && *filename == "note.txt" {
                return Vec::new();
            }
        }
        if solstone_core_format::segment::segment_key(parts[2]).is_some() {
            let day = parts[0];
            let stream = parts[1];
            let seg_name = parts[2];
            let day_segments = get_day_segments(journal_root, day, day_cache);
            let found = day_segments.iter().find(|s| {
                s.record_identity()
                    .map(|id| id.stream == stream && id.name == seg_name)
                    .unwrap_or(false)
            });
            if let Some(segment) = found {
                return vec![(day.to_string(), segment.clone())];
            }
            return Vec::new();
        }
    }

    // 2. Activity file: exactly facets/<facet>/activities/<YYYYMMDD>.jsonl (4 components)
    if parts.len() == 4 && parts[0] == "facets" && parts[2] == "activities" {
        let Some(day) = parts[3].strip_suffix(".jsonl") else {
            return Vec::new();
        };
        if !is_day_key(day) {
            return Vec::new();
        }
        let Some(record) = subject.activity_record else {
            return Vec::new();
        };

        let Some(Value::Array(seg_values)) = record.get("segments") else {
            return Vec::new();
        };
        let keys: Vec<&str> = seg_values.iter().filter_map(Value::as_str).collect();
        if keys.is_empty() {
            return Vec::new();
        }

        let day_segments = get_day_segments(journal_root, day, day_cache);
        let stream_opt = record
            .get("stream")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());

        let mut candidates: Vec<(String, Segment)> = Vec::new();
        for key in keys {
            if let Some(stream_name) = stream_opt {
                for seg in day_segments {
                    if seg.stream().matches(stream_name) && seg.key() == key {
                        candidates.push((day.to_string(), seg.clone()));
                    }
                }
            } else {
                let named_matches: Vec<&Segment> = day_segments
                    .iter()
                    .filter(|s| s.stream().directory().is_some() && s.name().to_str() == Some(key))
                    .collect();
                if !named_matches.is_empty() {
                    for seg in named_matches {
                        candidates.push((day.to_string(), seg.clone()));
                    }
                } else if let Some(flat_seg) = day_segments
                    .iter()
                    .find(|s| s.stream().is_direct() && s.name().to_str() == Some(key))
                {
                    candidates.push((day.to_string(), flat_seg.clone()));
                }
            }
        }

        // Sort candidates by start seconds, record_identity stream (_default for flat), then directory name
        candidates.sort_by(|(_, a), (_, b)| {
            let clock_a = segment_clock(a.name().to_str().unwrap_or(""));
            let clock_b = segment_clock(b.name().to_str().unwrap_or(""));
            let start_a = clock_a.as_ref().map(|c| c.start_seconds).unwrap_or(0);
            let start_b = clock_b.as_ref().map(|c| c.start_seconds).unwrap_or(0);

            let id_a = a.record_identity().ok();
            let id_b = b.record_identity().ok();
            let stream_a = id_a.as_ref().map(|id| id.stream).unwrap_or("");
            let stream_b = id_b.as_ref().map(|id| id.stream).unwrap_or("");
            let name_a = a.name().to_str().unwrap_or("");
            let name_b = b.name().to_str().unwrap_or("");

            start_a
                .cmp(&start_b)
                .then_with(|| stream_a.cmp(stream_b))
                .then_with(|| name_a.cmp(name_b))
        });

        // Drop a later duplicate of the same stream and directory name
        let mut seen = BTreeSet::new();
        candidates.retain(|(_, seg)| {
            if let Ok(id) = seg.record_identity() {
                seen.insert((id.stream.to_string(), id.name.to_string()))
            } else {
                false
            }
        });

        return candidates;
    }

    Vec::new()
}

fn get_day_segments<'a>(
    journal_root: &Path,
    day: &str,
    cache: &'a mut BTreeMap<String, Vec<Segment>>,
) -> &'a [Segment] {
    cache
        .entry(day.to_string())
        .or_insert_with(|| {
            // A calendar segment holds no transcript, so it is never linked.
            iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(day))
                .unwrap_or_default()
                .into_iter()
                .filter(|segment| {
                    !solstone_core_segment::is_calendar_segment(journal_root, segment)
                })
                .collect()
        })
        .as_slice()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use solstone_core_indexer_query::ConnectionScope;
    use solstone_core_npy::write_npy;
    use solstone_core_transcripts::{AgentSpeakers, VoiceNames};
    use zip::{CompressionMethod, ZipWriter};

    use super::*;
    use crate::permissions::ConnectionReadSnapshot;

    const DAY: &str = "20261030";
    const STREAM: &str = "device";

    /// One segment's transcript and embeddings, one line per `(speaker, axis)`.
    fn segment(root: &Path, key: &str, lines: &[(i64, usize)]) {
        let dir = root.join("chronicle").join(DAY).join(STREAM).join(key);
        fs::create_dir_all(&dir).unwrap();
        let mut jsonl = String::from("{\"raw\":\"audio.m4a\"}\n");
        let mut payload = Vec::new();
        let mut ids = Vec::new();
        for (index, (speaker, axis)) in lines.iter().enumerate() {
            jsonl.push_str(&format!(
                "{{\"start\":\"00:00:{index:02}\",\"speaker\":{speaker},\"text\":\"line\"}}\n"
            ));
            let mut row = vec![0.0_f32; 256];
            row[*axis] = 1.0;
            for value in row {
                payload.extend_from_slice(&value.to_le_bytes());
            }
            ids.extend_from_slice(&(index as i32 + 1).to_le_bytes());
        }
        fs::write(dir.join("audio.jsonl"), jsonl).unwrap();
        let mut zip = ZipWriter::new(fs::File::create(dir.join("audio.npz")).unwrap());
        let options =
            zip::write::SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("embeddings.npy", options).unwrap();
        zip.write_all(&write_npy(
            "<f4",
            &format!("({}, 256)", lines.len()),
            &payload,
        ))
        .unwrap();
        zip.start_file("statement_ids.npy", options).unwrap();
        zip.write_all(&write_npy("<i4", &format!("({},)", lines.len()), &ids))
            .unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn a_link_binds_the_version_a_read_sees_on_a_segment_without_labels() {
        let journal = tempfile::tempdir().unwrap();
        // Diarization swaps the two voices' numbers between back-to-back
        // segments, so the second segment's read renumbers them.
        segment(journal.path(), "100000_300", &[(1, 0), (2, 1)]);
        segment(journal.path(), "100500_300", &[(2, 0), (1, 1)]);
        let segment = Segment::resolve_exact(journal.path(), DAY, STREAM, "100500_300")
            .unwrap()
            .unwrap();
        assert!(!segment.path().join("talents/speaker_labels.json").exists());
        let speakers = AgentSpeakers {
            voices: VoiceNames {
                principal: "owner".to_owned(),
                owner_voice_confirmed: false,
                people: BTreeMap::new(),
            },
            owner: "The owner".to_owned(),
        };
        let snapshot = ConnectionReadSnapshot {
            categories: BTreeSet::new(),
            scope: ConnectionScope::WholeJournal,
            generation: 1,
        };
        let possible_names = ConnectionPossibleNames::new(journal.path(), &snapshot);

        let read = solstone_core_transcripts::read_segment_transcript_page(
            &segment,
            None,
            Some(&speakers),
            possible_names.labels(segment.path()).as_ref(),
        )
        .unwrap();
        let mut checker =
            LiveTranscriptPages::with_speakers(Some(speakers), Arc::clone(&possible_names));

        assert_eq!(
            checker.check_first_page(journal.path(), &segment),
            Ok(Some(read.version.fingerprint()))
        );
    }

    #[test]
    fn a_link_binds_the_version_a_read_sees_with_the_owners_answers() {
        use sha2::{Digest, Sha256};

        let journal = tempfile::tempdir().unwrap();
        let dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join("100000_300");
        fs::create_dir_all(dir.join("talents")).unwrap();
        let lines = [
            r#"{"start":"00:00:01","speaker":1,"sentence_id":1,"text":"Let us begin"}"#,
            r#"{"start":"00:00:02","speaker":2,"sentence_id":2,"text":"I will send it"}"#,
        ];
        fs::write(
            dir.join("audio.jsonl"),
            format!("{{\"raw\":\"audio.m4a\"}}\n{}\n", lines.join("\n")),
        )
        .unwrap();
        let answers = |entries: &[(usize, &str)]| {
            let entries = entries
                .iter()
                .map(|(line, speaker)| {
                    let record: Value = serde_json::from_str(lines[line - 1]).unwrap();
                    json!({
                        "source": "audio",
                        "sentence_id": line,
                        "start": record["start"],
                        "text_sha256": format!("{:x}", Sha256::digest(record["text"].as_str().unwrap())),
                        "speaker": speaker,
                        "set_at": "2026-10-31T12:00:00Z",
                    })
                })
                .collect::<Vec<_>>();
            fs::write(
                dir.join("talents/owner_line_labels.json"),
                json!({"version": 1, "entries": entries}).to_string(),
            )
            .unwrap();
        };
        let segment = Segment::resolve_exact(journal.path(), DAY, STREAM, "100000_300")
            .unwrap()
            .unwrap();
        let speakers = AgentSpeakers {
            voices: VoiceNames {
                principal: "owner".to_owned(),
                owner_voice_confirmed: true,
                people: BTreeMap::from([("pat".to_owned(), "Pat".to_owned())]),
            },
            owner: "The owner".to_owned(),
        };
        let snapshot = ConnectionReadSnapshot {
            categories: BTreeSet::new(),
            scope: ConnectionScope::WholeJournal,
            generation: 1,
        };
        let possible_names = ConnectionPossibleNames::new(journal.path(), &snapshot);
        let read = || {
            solstone_core_transcripts::read_segment_transcript_page(
                &segment,
                None,
                Some(&speakers),
                possible_names.labels(segment.path()).as_ref(),
            )
            .unwrap()
        };
        let check = || {
            LiveTranscriptPages::with_speakers(Some(speakers.clone()), Arc::clone(&possible_names))
                .check_first_page(journal.path(), &segment)
        };

        // A segment the journal never attributed.
        let unanswered = read();
        assert_eq!(check(), Ok(Some(unanswered.version.fingerprint())));
        answers(&[(1, "pat")]);
        let answered = read();
        assert_eq!(answered.entries[0].text, "[00:00:01] Pat: Let us begin");
        assert_ne!(answered.version, unanswered.version);
        assert_eq!(check(), Ok(Some(answered.version.fingerprint())));

        // An attributed one.
        fs::write(
            dir.join("talents/speaker_labels.json"),
            json!({"labels":[{"sentence_id":2,"speaker":"owner","method":"owner_centroid"}]})
                .to_string(),
        )
        .unwrap();
        answers(&[(1, "owner"), (2, "pat")]);
        let answered = read();
        assert_eq!(
            answered.entries[0].text,
            "[00:00:01] The owner: Let us begin"
        );
        assert_eq!(answered.entries[1].text, "[00:00:02] Pat: I will send it");
        assert_eq!(check(), Ok(Some(answered.version.fingerprint())));

        // Without answers the read and the link are what they were.
        fs::remove_file(dir.join("talents/speaker_labels.json")).unwrap();
        fs::remove_file(dir.join("talents/owner_line_labels.json")).unwrap();
        assert_eq!(read(), unanswered);
        assert_eq!(check(), Ok(Some(unanswered.version.fingerprint())));
    }
}
