// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! After a naming or a correction, the people already derived from the
//! changed segments follow the new labels.
//!
//! An activity's participation list and its Story actors were resolved from
//! names when they were written. An unnamed voice reached the talents as
//! "Speaker N", N being that segment's diarization speaker, so those entries
//! stayed unresolved. Once the owner names the voice, the sentences carry a
//! label, and the entry can be resolved without asking a talent again.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value};
use solstone_core_entity::{is_admissible_person, load_all_journal_entities};
use solstone_core_facets::DestinationObservation;
use solstone_core_journal_io::SegmentLayout;

/// The actor fields a Story item names, each with its resolved id field.
const ACTOR_FIELDS: [(&str, &str); 4] = [
    ("owner", "owner_entity_id"),
    ("counterparty", "counterparty_entity_id"),
    ("from", "from_entity_id"),
    ("to", "to_entity_id"),
];

/// One segment whose speaker labels a naming or correction changed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChangedSegment {
    pub day: String,
    pub stream: String,
    pub segment_key: String,
}

/// The admissible people a resolution may name, by id, with the names each
/// is known by.
struct People {
    names: BTreeMap<String, String>,
    by_name: BTreeMap<String, BTreeSet<String>>,
    principal: Option<String>,
}

impl People {
    fn load(journal: &Path) -> Result<Self, String> {
        let mut people = Self {
            names: BTreeMap::new(),
            by_name: BTreeMap::new(),
            principal: solstone_core_entity::read_journal_principal(journal)
                .map_err(|e| e.to_string())?
                .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned)),
        };
        for entity in load_all_journal_entities(journal).map_err(|e| e.to_string())? {
            if !is_admissible_person(&entity) {
                continue;
            }
            let name = entity
                .value
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&entity.id)
                .to_owned();
            let aka = entity
                .value
                .get("aka")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            for known in std::iter::once(name.as_str()).chain(aka) {
                people
                    .by_name
                    .entry(fold(known))
                    .or_default()
                    .insert(entity.id.clone());
            }
            people.names.insert(entity.id, name);
        }
        Ok(people)
    }

    /// The one person a name exactly names, if it names exactly one.
    fn named(&self, name: &str) -> Option<&str> {
        let ids = self.by_name.get(&fold(name))?;
        (ids.len() == 1).then(|| ids.iter().next().map(String::as_str))?
    }
}

/// Re-resolve the participation entries and Story actors of every activity
/// on the changed segments. Returns how many entries and actors gained an id.
///
/// Two deterministic rules, and nothing a talent would have to judge:
/// - a "Speaker N" entry or actor with no id gains the person whom N's
///   labeled sentences, across the activity's segments, all name, when they
///   name exactly one person other than the owner;
/// - an entry or actor with no id whose name exactly names one of the people
///   the change named (`named`) gains that person's id.
///
/// Muted or unreadable facets are skipped, as the talent hooks skip them.
pub fn reresolve_changed_segments(
    journal: &Path,
    changed: &[ChangedSegment],
    named: &[String],
) -> Result<usize, String> {
    if changed.is_empty() {
        return Ok(0);
    }
    let people = People::load(journal)?;
    let mut by_day = BTreeMap::<&str, BTreeSet<(&str, &str)>>::new();
    for segment in changed {
        by_day
            .entry(segment.day.as_str())
            .or_default()
            .insert((segment.stream.as_str(), segment.segment_key.as_str()));
    }
    let mut resolved = 0;
    for facet in
        solstone_core_facets::list_declared_facet_names(journal).map_err(|e| e.to_string())?
    {
        let Ok(DestinationObservation::Ready { id, muted: false }) =
            solstone_core_facets::observe_facet_destination(journal, &facet)
        else {
            continue;
        };
        for (day, segments) in &by_day {
            let records = solstone_core_facets::load_activity_records(journal, &facet, day, true)
                .map_err(|e| e.to_string())?;
            for record in records {
                let Some(stream) = record.get("stream").and_then(Value::as_str) else {
                    continue;
                };
                let keys = record_segments(&record);
                if !keys
                    .iter()
                    .any(|key| segments.contains(&(stream, key.as_str())))
                {
                    continue;
                }
                let speakers = SpeakerMap::read(journal, day, stream, &keys, &people);
                // The snapshot only finds candidates. The patch replaces whole
                // arrays, so it is derived from the record as it stands inside
                // the writer's lock, never from the snapshot.
                if record_patch(&record, &people, &speakers, named).is_none() {
                    continue;
                }
                let Some(record_id) = record.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let _guard = solstone_core_facets::hold_activity_enrichment(journal, &facet, &id)
                    .map_err(|e| e.to_string())?;
                let mut count = 0;
                let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
                solstone_core_facets::update_activity_record_with(
                    journal,
                    &facet,
                    day,
                    record_id,
                    |current| {
                        let (_, patch, changed) = record_patch(current, &people, &speakers, named)?;
                        count = changed;
                        Some(patch)
                    },
                    "speaker_identify",
                    "resolved people from named voices",
                    &timestamp,
                )
                .map_err(|e| e.to_string())?;
                resolved += count;
            }
        }
    }
    Ok(resolved)
}

fn record_segments(record: &Map<String, Value>) -> Vec<String> {
    record
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// For each diarization speaker number in an activity's segments, the people
/// its labeled sentences name, other than the owner.
struct SpeakerMap(BTreeMap<i64, BTreeSet<String>>);

impl SpeakerMap {
    fn read(journal: &Path, day: &str, stream: &str, keys: &[String], people: &People) -> Self {
        let layout = if stream == solstone_core_journal_io::DEFAULT_STREAM {
            SegmentLayout::Direct
        } else {
            SegmentLayout::Named
        };
        let mut map = BTreeMap::<i64, BTreeSet<String>>::new();
        for key in keys {
            let Ok(Some(segment)) =
                crate::segment_catalog::resolve_exact(journal, day, stream, key, layout)
            else {
                continue;
            };
            let Some(source) = crate::voice_members::labels_source(&segment) else {
                continue;
            };
            let labels = crate::identify_forward_phases::load_labels(&segment);
            for (sentence_id, speaker) in
                transcript_speakers(&segment.join(format!("{source}.jsonl")))
            {
                let Some(person) = labels
                    .get(&sentence_id)
                    .and_then(|label| label.get("speaker"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                if people.names.contains_key(person) && people.principal.as_deref() != Some(person)
                {
                    map.entry(speaker).or_default().insert(person.to_owned());
                }
            }
        }
        Self(map)
    }

    /// The one person "Speaker N" is, when N's labels agree on exactly one.
    fn person(&self, name: &str) -> Option<&str> {
        let number = name
            .trim()
            .strip_prefix("Speaker ")
            .or_else(|| name.trim().strip_prefix("speaker "))?
            .trim()
            .parse::<i64>()
            .ok()?;
        let people = self.0.get(&number)?;
        (people.len() == 1).then(|| people.iter().next().map(String::as_str))?
    }
}

/// Sentence id and diarization speaker number of each transcript row.
fn transcript_speakers(path: &PathBuf) -> Vec<(i64, i64)> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let Ok(read) = solstone_core_speaker_id::transcript::read_transcript_rows(&bytes) else {
        return Vec::new();
    };
    read.rows
        .iter()
        .filter_map(|row| Some((row.sentence_id, row.value.get("speaker")?.as_i64()?)))
        .collect()
}

/// The arrays of one record that change, its id, and how many entries and
/// actors gain an id; `None` when nothing changes.
fn record_patch(
    record: &Map<String, Value>,
    people: &People,
    speakers: &SpeakerMap,
    named: &[String],
) -> Option<(String, Map<String, Value>, usize)> {
    let record_id = record
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?;
    // The person a name now resolves to, and whether the name is a
    // "Speaker N" placeholder to replace. The owner is never named here.
    let resolve = |name: &str| -> Option<(String, bool)> {
        let resolved = match speakers.person(name) {
            Some(person) => (person, true),
            None => (
                people
                    .named(name)
                    .filter(|id| named.iter().any(|named| named == id))?,
                false,
            ),
        };
        (people.principal.as_deref() != Some(resolved.0))
            .then(|| (resolved.0.to_owned(), resolved.1))
    };
    let mut patch = Map::new();
    let mut count = 0;
    for key in ["participation", "relations"] {
        let Some(Value::Array(items)) = record.get(key) else {
            continue;
        };
        let fields: &[(&str, &str)] = if key == "participation" {
            &[("name", "entity_id")]
        } else {
            &ACTOR_FIELDS
        };
        let mut changed = false;
        let items = items
            .iter()
            .map(|item| {
                let Value::Object(object) = item else {
                    return item.clone();
                };
                let mut object = object.clone();
                for (field, id_field) in fields {
                    if object.get(*id_field) != Some(&Value::Null) {
                        continue;
                    }
                    let Some((person, placeholder)) = object
                        .get(*field)
                        .and_then(Value::as_str)
                        .and_then(&resolve)
                    else {
                        continue;
                    };
                    if placeholder {
                        object.insert(
                            (*field).to_owned(),
                            Value::String(people.names[&person].clone()),
                        );
                    }
                    object.insert((*id_field).to_owned(), Value::String(person));
                    changed = true;
                    count += 1;
                }
                Value::Object(object)
            })
            .collect();
        if changed {
            patch.insert(key.to_string(), Value::Array(items));
        }
    }
    (!patch.is_empty()).then(|| (record_id.to_owned(), patch, count))
}

fn fold(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const DAY: &str = "20260808";

    fn write(root: &Path, path: &str, text: String) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn person(root: &Path, id: &str, name: &str, principal: bool) {
        write(
            root,
            &format!("entities/{id}/entity.json"),
            json!({"id":id,"name":name,"type":"Person","is_principal":principal}).to_string(),
        );
    }

    /// A segment whose transcript gives each sentence a diarization speaker,
    /// and whose labels name some of those sentences.
    fn segment(root: &Path, key: &str, speakers: &[(i64, i64)], labels: &[(i64, &str)]) {
        let base = format!("chronicle/{DAY}/mic/{key}");
        let mut transcript = json!({"raw":"fixture"}).to_string() + "\n";
        for (sentence_id, speaker) in speakers {
            transcript += &(json!({"start":"00:00:01","speaker":speaker,"sentence_id":sentence_id,"text":"words"}).to_string() + "\n");
        }
        write(root, &format!("{base}/audio.jsonl"), transcript);
        write(root, &format!("{base}/audio.npz"), String::new());
        write(
            root,
            &format!("{base}/talents/speaker_labels.json"),
            json!({"labels": labels.iter().map(|(id, who)| json!({"sentence_id":id,"speaker":who,"confidence":"high","method":"user_identified"})).collect::<Vec<_>>()}).to_string(),
        );
    }

    fn journal() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        person(root.path(), "owner", "Avery", true);
        person(root.path(), "bob", "Bob", false);
        person(root.path(), "cara", "Cara", false);
        write(
            root.path(),
            &format!("facets/work/activities/{DAY}.jsonl"),
            json!({"id":"a1","activity":"meeting","stream":"mic","segments":["120000_300","120500_300"],
                "participation":[
                    {"name":"Speaker 2","role":"attendee","source":"voice","entity_id":null},
                    {"name":"Speaker 1","role":"attendee","source":"voice","entity_id":null},
                    {"name":"Speaker 3","role":"attendee","source":"voice","entity_id":null},
                    {"name":"Bob","role":"mentioned","source":"transcript","entity_id":null},
                    {"name":"Cara","role":"mentioned","source":"transcript","entity_id":null}
                ],
                "relations":[{"from":"Speaker 2","from_entity_id":null,"to":"you","to_entity_id":"owner","kind":"works-with","note":""}]
            }).to_string() + "\n",
        );
        // Speaker 2 is Bob in both segments. Speaker 1 is Bob in one and Cara
        // in the other. Speaker 3 is the owner.
        segment(
            root.path(),
            "120000_300",
            &[(1, 2), (2, 1), (3, 3)],
            &[(1, "bob"), (2, "bob"), (3, "owner")],
        );
        segment(
            root.path(),
            "120500_300",
            &[(1, 2), (2, 1)],
            &[(1, "bob"), (2, "cara")],
        );
        root
    }

    fn record(root: &Path) -> Map<String, Value> {
        solstone_core_facets::get_activity_record(root, "work", DAY, "a1")
            .unwrap()
            .unwrap()
    }

    fn changed() -> Vec<ChangedSegment> {
        vec![ChangedSegment {
            day: DAY.to_owned(),
            stream: "mic".to_owned(),
            segment_key: "120500_300".to_owned(),
        }]
    }

    #[test]
    fn a_named_voice_names_its_speaker_entries_and_only_when_unambiguous() {
        let root = journal();
        let resolved =
            reresolve_changed_segments(root.path(), &changed(), &["bob".to_owned()]).unwrap();
        let record = record(root.path());
        let people = record["participation"].as_array().unwrap();
        assert_eq!(people[0]["name"], "Bob");
        assert_eq!(people[0]["entity_id"], "bob");
        // Speaker 1 is two different people across the segments.
        assert_eq!(people[1]["name"], "Speaker 1");
        assert!(people[1]["entity_id"].is_null());
        // Speaker 3 is the owner, whom participation never names.
        assert!(people[2]["entity_id"].is_null());
        // The person this naming named, by an exact name.
        assert_eq!(people[3]["entity_id"], "bob");
        // Someone the naming did not name keeps the hook's resolution.
        assert!(people[4]["entity_id"].is_null());
        assert_eq!(record["relations"][0]["from"], "Bob");
        assert_eq!(record["relations"][0]["from_entity_id"], "bob");
        assert_eq!(record["relations"][0]["to_entity_id"], "owner");
        assert_eq!(
            record["edits"].as_array().unwrap().last().unwrap()["actor"],
            "speaker_identify"
        );
        assert_eq!(resolved, 3);
        // Nothing is left to resolve a second time.
        assert_eq!(
            reresolve_changed_segments(root.path(), &changed(), &["bob".to_owned()]).unwrap(),
            0
        );
    }

    #[test]
    fn an_activity_on_other_segments_is_untouched() {
        let root = journal();
        let before = fs::read(
            root.path()
                .join(format!("facets/work/activities/{DAY}.jsonl")),
        )
        .unwrap();
        let elsewhere = [ChangedSegment {
            day: DAY.to_owned(),
            stream: "mic".to_owned(),
            segment_key: "130000_300".to_owned(),
        }];
        assert_eq!(
            reresolve_changed_segments(root.path(), &elsewhere, &["bob".to_owned()]).unwrap(),
            0
        );
        assert_eq!(
            fs::read(
                root.path()
                    .join(format!("facets/work/activities/{DAY}.jsonl"))
            )
            .unwrap(),
            before
        );
    }
}
