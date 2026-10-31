// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use solstone_core_entity::{is_admissible_person, load_all_journal_entities};
use solstone_core_journal_io::zip2_name::{Zip2NameFailure, prove_zip2_directory};
use solstone_core_npy::parse_npy;
use solstone_core_transcripts::{JournalLabels, OwnerAnswer, load_who_spoke};
use zip::ZipArchive;

#[derive(Clone, Serialize)]
pub(crate) struct SpeakerLabelsState {
    /// The segment has its own speaker labels file.
    pub(crate) present: bool,
    /// That file was read as a labels record.
    pub(crate) loaded: bool,
    pub(crate) source: Option<String>,
    pub(crate) ambiguous: bool,
    /// The segment says who spoke: its own labels, an owner answer that
    /// applies, or owner answers that can't be read.
    pub(crate) who_spoke_present: bool,
}
#[derive(Clone, Serialize)]
pub(crate) struct SpeakerLabel {
    pub(crate) name: String,
    pub(crate) entity_id: String,
    pub(crate) confidence: Value,
    pub(crate) confidence_state: String,
    pub(crate) method: Value,
    pub(crate) owner_margin_declined: bool,
    pub(crate) acoustic_margin_declined: bool,
    pub(crate) is_owner: bool,
}
pub(crate) struct SpeakerJoin {
    pub(crate) state: SpeakerLabelsState,
    pub(crate) labels: BTreeMap<i64, SpeakerLabel>,
    pub(crate) warnings: Vec<crate::segment::WarningDetail>,
    /// Names of admissible people, for voices already confirmed as someone.
    pub(crate) person_names: BTreeMap<String, String>,
    pub(crate) principal: Option<String>,
    /// The owner's answers that apply to lines of `state.source`, by sentence id.
    pub(crate) owner_set: BTreeMap<i64, OwnerAnswer>,
    /// The owner's answers can't be read: every line is an unnamed voice.
    pub(crate) owner_unreadable: bool,
}

/// The anonymous voice a sentence belongs to when nobody has named that sentence.
#[derive(Clone, Serialize)]
pub(crate) struct SpeakerVoice {
    pub(crate) voice_id: i64,
    /// Set when the voice itself was confirmed as an admissible person.
    pub(crate) entity_id: Option<String>,
    pub(crate) name: Option<String>,
}

impl SpeakerJoin {
    /// Voice tags for one audio source, leaving out the owner's own voice.
    pub(crate) fn voices(
        &self,
        journal_root: &Path,
        day: &str,
        stream: &str,
        segment_key: &str,
        source: &str,
    ) -> BTreeMap<i64, SpeakerVoice> {
        solstone_core_speaker_resolve::voice_tags::segment_voice_tags(
            journal_root,
            day,
            stream,
            segment_key,
            source,
        )
        .into_iter()
        .filter(|(_, tag)| tag.confirmed_entity.is_none() || tag.confirmed_entity != self.principal)
        .map(|(sentence_id, tag)| {
            let name = tag
                .confirmed_entity
                .as_ref()
                .and_then(|id| self.person_names.get(id))
                .cloned();
            let voice = SpeakerVoice {
                voice_id: tag.voice_id,
                entity_id: name.as_ref().and(tag.confirmed_entity.clone()),
                name,
            };
            (sentence_id, voice)
        })
        .collect()
    }
}

pub(crate) fn load(
    dir: &Path,
    journal_root: &Path,
    now: DateTime<Utc>,
    has_audio_transcript: bool,
) -> SpeakerJoin {
    let path = dir.join("talents/speaker_labels.json");
    let (page_source, page_ambiguous) = solstone_core_transcripts::speaker_label_source(dir);
    let who = load_who_spoke(dir, page_source.as_deref(), None);
    let present = who.labels != JournalLabels::Absent;
    let who_spoke_present = who.present();
    let (source, ambiguous) = if present || who_spoke_present {
        (page_source, page_ambiguous)
    } else {
        (None, false)
    };
    let mut warnings = Vec::new();
    if present && ambiguous {
        warnings.push(warning(
            &path,
            "speaker label source is ambiguous for this segment",
            now,
        ));
    }
    if let Some(unreadable) = &who.owner_unreadable {
        warnings.push(crate::segment::WarningDetail {
            kind: "owner_line_labels".into(),
            file: unreadable.file.display().to_string(),
            message:
                "your answers about who spoke can't be read, so no one is named in this segment"
                    .into(),
            ts: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        });
    }
    let mut labels = BTreeMap::new();
    let loaded = who.labels == JournalLabels::Loaded;
    let entities = match if who_spoke_present || has_audio_transcript {
        load_all_journal_entities(journal_root)
    } else {
        Ok(Vec::new())
    } {
        Ok(entities) => entities,
        Err(_) => {
            // Without the entity store no label can be named, so say so.
            if present {
                warnings.push(warning(
                    &journal_root.join("entities"),
                    "speaker labels are unavailable for this segment",
                    now,
                ));
            }
            Vec::new()
        }
    };
    let admitted_entity_ids = entities
        .iter()
        .filter(|entity| is_admissible_person(entity))
        .map(|entity| entity.id.as_str())
        .collect::<BTreeSet<_>>();
    let person_names = entities
        .iter()
        .filter(|entity| is_admissible_person(entity))
        .map(|entity| {
            let name = entity
                .value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(entity.id.as_str());
            (entity.id.clone(), name.to_owned())
        })
        .collect::<BTreeMap<_, _>>();
    let mut principals = entities
        .iter()
        .filter(|entity| entity.is_principal() && !entity.is_blocked());
    let principal = principals
        .next()
        .map(|entity| entity.id.clone())
        .filter(|_| principals.next().is_none());
    if who.labels == JournalLabels::Damaged {
        warnings.push(warning(
            &path,
            "speaker labels are unavailable for this segment",
            now,
        ));
    }
    for row in &who.rows {
        let Some(id) = row.get("sentence_id").and_then(Value::as_i64) else {
            continue;
        };
        let Some(entity_id) = row.get("speaker").and_then(Value::as_str) else {
            continue;
        };
        if !admitted_entity_ids.contains(entity_id) {
            continue;
        }
        let name = entities
            .iter()
            .find(|entity| entity.id == entity_id)
            .and_then(|entity| entity.value.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(entity_id)
            .to_owned();
        let confidence = row.get("confidence").cloned().unwrap_or(Value::Null);
        let confidence_state = match confidence.as_str() {
            Some("high" | "medium") => confidence.as_str().unwrap().to_owned(),
            _ => "unknown".into(),
        };
        labels.insert(
            id,
            SpeakerLabel {
                name,
                entity_id: entity_id.into(),
                confidence,
                confidence_state,
                method: row.get("method").cloned().unwrap_or(Value::Null),
                owner_margin_declined: row
                    .get("owner_margin_declined")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                acoustic_margin_declined: row
                    .get("acoustic_margin_declined")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                is_owner: principal.as_deref() == Some(entity_id),
            },
        );
    }
    SpeakerJoin {
        state: SpeakerLabelsState {
            present,
            loaded,
            source,
            ambiguous,
            who_spoke_present,
        },
        labels,
        warnings,
        person_names,
        principal,
        owner_set: who.owner_set,
        owner_unreadable: who.owner_unreadable.is_some(),
    }
}

pub(crate) fn embedding_ids(dir: &Path, source: &str) -> Result<BTreeSet<i64>, Zip2NameFailure> {
    match read_statement_ids(&dir.join(format!("{source}.npz")))? {
        Some(ids) => Ok(ids.into_iter().collect()),
        None => Ok(BTreeSet::new()),
    }
}
pub(crate) fn statement_ordinals(text: &str) -> Vec<Option<i64>> {
    let mut ordinal = 0;
    text.lines()
        .map(|line| solstone_core_transcripts::next_statement_ordinal(line, &mut ordinal))
        .collect()
}

fn warning(path: &Path, message: &str, now: DateTime<Utc>) -> crate::segment::WarningDetail {
    crate::segment::WarningDetail {
        kind: "speaker_labels".into(),
        file: path.display().to_string(),
        message: message.into(),
        ts: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }
}
fn read_statement_ids(path: &Path) -> Result<Option<Vec<i64>>, Zip2NameFailure> {
    let Ok(file) = File::open(path) else {
        return Ok(None);
    };
    let archive = ZipArchive::new(file).map_err(|error| Zip2NameFailure::Unreadable {
        detail: error.to_string(),
    })?;
    let (mut archive, names) = prove_open_zip(archive)?;
    let Some(index) = names.iter().position(|name| name == "statement_ids.npy") else {
        return Ok(None);
    };
    let mut member = archive
        .by_index(index)
        .map_err(|error| Zip2NameFailure::Unreadable {
            detail: error.to_string(),
        })?;
    let mut bytes = Vec::new();
    member
        .read_to_end(&mut bytes)
        .map_err(|error| Zip2NameFailure::Unreadable {
            detail: error.to_string(),
        })?;
    let Some(blob) = parse_npy(&bytes).ok() else {
        return Ok(None);
    };
    if blob.fortran_order || blob.shape.len() != 1 {
        return Ok(None);
    }
    Ok(match blob.descr.as_str() {
        "<i4" => blob
            .payload
            .chunks_exact(4)
            .map(|b| Some(i32::from_le_bytes(b.try_into().ok()?) as i64))
            .collect(),
        "<i8" => blob
            .payload
            .chunks_exact(8)
            .map(|b| Some(i64::from_le_bytes(b.try_into().ok()?)))
            .collect(),
        _ => None,
    })
}

fn prove_open_zip<R: Read + Seek>(
    mut archive: ZipArchive<R>,
) -> Result<(ZipArchive<R>, Vec<String>), Zip2NameFailure> {
    let mut coords = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| Zip2NameFailure::Unreadable {
                detail: error.to_string(),
            })?;
        coords.push((entry.header_start(), entry.central_header_start()));
    }
    let central_start = archive.central_directory_start();
    let mut reader = archive.into_inner();
    let names = prove_zip2_directory(&mut reader, central_start, &coords)?;
    let archive = ZipArchive::new(reader).map_err(|error| Zip2NameFailure::Unreadable {
        detail: error.to_string(),
    })?;
    Ok((archive, names))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use chrono::Utc;
    use serde_json::json;

    use super::load;

    #[test]
    fn screen_only_segment_does_not_join_unrelated_people() {
        let root = tempfile::tempdir().unwrap();
        let entity = root.path().join("entities/person");
        fs::create_dir_all(&entity).unwrap();
        fs::write(
            entity.join("entity.json"),
            r#"{"id":"person","type":"Person","name":"Person","is_principal":true}"#,
        )
        .unwrap();
        let segment = root.path().join("segment");
        fs::create_dir_all(&segment).unwrap();
        let screen = load(&segment, root.path(), Utc::now(), false);
        assert!(screen.person_names.is_empty());
        assert!(screen.principal.is_none());
        let audio = load(&segment, root.path(), Utc::now(), true);
        assert_eq!(
            audio.person_names.get("person").map(String::as_str),
            Some("Person")
        );
        assert_eq!(audio.principal.as_deref(), Some("person"));
    }

    #[test]
    fn load_omits_labels_for_ineligible_speakers() {
        let root = std::env::temp_dir().join(format!(
            "solstone-transcript-speaker-admission-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        for (id, entity) in [
            (
                "person",
                json!({"id":"person","name":"Person","type":"Person"}),
            ),
            ("tool", json!({"id":"tool","name":"Tool","type":"Tool"})),
        ] {
            let entity_dir = root.join("entities").join(id);
            fs::create_dir_all(&entity_dir).expect("entity directory");
            fs::write(
                entity_dir.join("entity.json"),
                serde_json::to_vec(&entity).expect("entity json"),
            )
            .expect("entity writes");
        }
        let segment = root.join("segment");
        fs::create_dir_all(segment.join("talents")).expect("talents directory");
        fs::write(
            segment.join("talents/speaker_labels.json"),
            json!({"labels":[
                {"sentence_id":1,"speaker":"person","confidence":"high"},
                {"sentence_id":2,"speaker":"tool","confidence":"high"}
            ]})
            .to_string(),
        )
        .expect("labels");

        let join = load(&segment, &root, Utc::now(), true);
        assert!(join.labels.contains_key(&1));
        assert!(!join.labels.contains_key(&2));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn load_warns_when_the_entity_store_cannot_be_read() {
        let root = std::env::temp_dir().join(format!(
            "solstone-transcript-speaker-entities-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        // An entity store that resolves outside the journal is refused.
        let outside = root.with_extension("outside");
        fs::create_dir_all(&outside).expect("outside directory");
        fs::create_dir_all(&root).expect("journal");
        std::os::unix::fs::symlink(&outside, root.join("entities")).expect("entities link");
        let segment = root.join("segment");
        fs::create_dir_all(segment.join("talents")).expect("talents directory");
        fs::write(
            segment.join("talents/speaker_labels.json"),
            json!({"labels":[{"sentence_id":1,"speaker":"person","confidence":"high"}]})
                .to_string(),
        )
        .expect("labels");

        let join = load(&segment, &root, Utc::now(), true);
        assert!(join.labels.is_empty());
        assert_eq!(join.warnings.len(), 1);
        assert_eq!(join.warnings[0].kind, "speaker_labels");
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn embedding_ids_refuses_collision() {
        use solstone_core_journal_io::zip2_name::crc32_ieee;

        let dir = tempfile::tempdir().unwrap();
        const UTF8_FLAG: u16 = 1 << 11;
        let flagged = "é".as_bytes();
        let mut local = Vec::new();
        let mut offsets = Vec::new();
        type ZipMember<'a> = (&'a [u8], u16, &'a [u8], &'a [u8]);
        let members: [ZipMember<'_>; 3] = [
            (b"statement_ids.npy", UTF8_FLAG, &[], b"x"),
            (&[0x82], 0, &[], b"a"),
            (flagged, UTF8_FLAG, &[], b"b"),
        ];
        for (name, flags, _extra, data) in members {
            offsets.push(local.len() as u32);
            let crc = crc32_ieee(data);
            let size = data.len() as u32;
            local.extend_from_slice(b"PK\x03\x04");
            local.extend_from_slice(&20_u16.to_le_bytes());
            local.extend_from_slice(&flags.to_le_bytes());
            local.extend_from_slice(&[0; 6]);
            local.extend_from_slice(&crc.to_le_bytes());
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&size.to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&0_u16.to_le_bytes());
            local.extend_from_slice(name);
            local.extend_from_slice(data);
        }
        let cd_start = local.len() as u32;
        let mut central = Vec::new();
        for (index, (name, flags, extra, data)) in members.iter().enumerate() {
            let crc = crc32_ieee(data);
            let size = data.len() as u32;
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&flags.to_le_bytes());
            central.extend_from_slice(&[0; 6]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&(extra.len() as u16).to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u32.to_le_bytes());
            central.extend_from_slice(&offsets[index].to_le_bytes());
            central.extend_from_slice(name);
            central.extend_from_slice(extra);
        }
        let mut bytes = local;
        bytes.extend_from_slice(&central);
        let count = members.len() as u16;
        bytes.extend_from_slice(b"PK\x05\x06");
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&(central.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&cd_start.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        fs::write(dir.path().join("audio.npz"), bytes).unwrap();
        assert!(super::embedding_ids(dir.path(), "audio").is_err());
    }
}
