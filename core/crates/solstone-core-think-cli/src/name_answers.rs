// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Past meetings follow the owner's answers about names.
//!
//! The journal asks the owner "is this name that person?" for a facet, and
//! the answer is kept in `entities/ambiguities.jsonl`. A new write honours it
//! at once. The activities already written do not: their people were resolved
//! and judged before the answer existed. Each discovery pass compares the
//! answers as they stand with the ones it last applied, and judges again the
//! activities of that facet that hold the name, with no model call
//! ([`solstone_core_talent_runtime::participation::judge_again`]).
//!
//! It reads the answers rather than hooking the route that saves one, so it
//! follows every way an answer changes: the owner's choice, a choice replaced
//! by another, a person removed (which reopens the question), a person
//! recovered, and an answer the review policy gave on its own.
//!
//! ⛔ An activity whose people the owner edited is never touched.

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::durability::{ArtifactId, artifact};
use solstone_core_talent_runtime::participation::{
    LoggedOutputs, Rejudged, holds_asked, judge_again,
};

const VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Applied {
    version: u32,
    /// By question: the facet it was asked in, the name asked about, and the
    /// entity the owner said it is.
    answers: BTreeMap<String, Answer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Answer {
    facet: String,
    name: String,
    entity: String,
}

/// What one pass did, for the log and for tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Answers that differ from what was last applied.
    pub answers: usize,
    pub judged: usize,
    pub changed: usize,
    /// Answers that could not be applied this time, and will be tried again.
    pub waiting: usize,
}

/// What was last applied. Losing it costs one more pass over the facets that
/// hold an answer, and changes nothing a second time.
fn path(journal: &Path) -> PathBuf {
    journal.join(artifact(ArtifactId::NameAnswersApplied).path)
}

fn read_applied(journal: &Path) -> BTreeMap<String, Answer> {
    std::fs::read(path(journal))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Applied>(&bytes).ok())
        .filter(|applied| applied.version == VERSION)
        .map(|applied| applied.answers)
        .unwrap_or_default()
}

/// The facet answers as the journal holds them now.
fn standing(journal: &Path) -> Result<BTreeMap<String, Answer>, String> {
    // A row that can't be read holds no answer to follow; the entity store's
    // own readers are the ones that report it.
    let rows = solstone_core_entity::read_ambiguities(
        journal,
        solstone_core_entity::MalformedPolicy::Skip,
    )
    .map_err(|error| error.to_string())?;
    let mut answers = BTreeMap::new();
    for row in &rows {
        let text = |field: &str| row.get(field).and_then(Value::as_str);
        let scope = |field: &str| {
            row.get("scope")
                .and_then(|scope| scope.get(field))
                .and_then(Value::as_str)
        };
        if let (Some(id), Some("facet"), Some(facet), Some(name), Some("resolved"), Some(entity)) = (
            text("ambiguity_id"),
            scope("kind"),
            scope("facet"),
            text("normalized_query"),
            text("status"),
            text("resolved_entity_id").filter(|entity| !entity.is_empty()),
        ) {
            answers.insert(
                id.to_owned(),
                Answer {
                    facet: facet.to_owned(),
                    name: name.to_owned(),
                    entity: entity.to_owned(),
                },
            );
        }
    }
    Ok(answers)
}

/// One name whose answer changed: who it was said to be, and who it is now.
#[derive(Debug, Default)]
struct Asked {
    questions: BTreeSet<String>,
    entities: BTreeSet<String>,
    /// Set once an activity could not be written: the answer stays unapplied.
    failed: bool,
}

/// Apply every answer that changed since the last pass.
pub fn pass(
    journal: &Path,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Outcome, String> {
    let applied = read_applied(journal);
    let answers = standing(journal)?;
    if applied == answers {
        return Ok(Outcome::default());
    }
    // By facet, then by name. Both the answer as it was and as it is count: a
    // name whose answer was taken back is judged again under what holds now.
    let mut changed = BTreeMap::<String, BTreeMap<String, Asked>>::new();
    for (id, answer) in applied
        .iter()
        .filter(|(id, answer)| answers.get(*id) != Some(answer))
        .chain(
            answers
                .iter()
                .filter(|(id, answer)| applied.get(*id) != Some(answer)),
        )
    {
        let asked = changed
            .entry(answer.facet.clone())
            .or_default()
            .entry(answer.name.clone())
            .or_default();
        asked.questions.insert(id.clone());
        asked.entities.insert(answer.entity.clone());
    }
    let mut outcome = Outcome {
        answers: changed
            .values()
            .flat_map(|names| names.values())
            .flat_map(|asked| &asked.questions)
            .collect::<BTreeSet<_>>()
            .len(),
        ..Outcome::default()
    };
    // Run logs are read only if some record needs one.
    let logs = OnceCell::new();
    for (facet, names) in &mut changed {
        for day in activity_days(journal, facet) {
            let records =
                match solstone_core_facets::load_activity_records(journal, facet, &day, true) {
                    Ok(records) => records,
                    Err(error) => {
                        log::warn!("name answers: {facet} {day} unreadable: {error}");
                        continue;
                    }
                };
            for record in &records {
                let (Some(record_id), Some(listed)) = (
                    record.get("id").and_then(Value::as_str),
                    record.get("participation").and_then(Value::as_array),
                ) else {
                    continue;
                };
                for (name, asked) in names.iter_mut().filter(|(_, asked)| !asked.failed) {
                    let entities = asked
                        .entities
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>();
                    if !listed
                        .iter()
                        .any(|entry| holds_asked(entry, name, &entities))
                    {
                        continue;
                    }
                    checkpoint()?;
                    outcome.judged += 1;
                    match judge_again(
                        journal,
                        facet,
                        &day,
                        record_id,
                        name,
                        &entities,
                        logs.get_or_init(|| LoggedOutputs::read(journal)),
                    ) {
                        Ok(Rejudged::Changed) => outcome.changed += 1,
                        Ok(_) => {}
                        // A facet that is muted or a lock that is busy now
                        // may not be later. The answer is tried again on the
                        // next pass, from the start, and no further here.
                        Err(error) => {
                            log::warn!(
                                "name answers: {facet} {day} {record_id} not judged, will try again: {error}"
                            );
                            asked.failed = true;
                        }
                    }
                }
            }
        }
    }
    // What is recorded as applied: the standing answers, except that a
    // question whose activities could not all be judged stays as it was.
    let waiting = changed
        .values()
        .flat_map(|names| names.values())
        .filter(|asked| asked.failed)
        .flat_map(|asked| asked.questions.iter().cloned())
        .collect::<BTreeSet<_>>();
    outcome.waiting = waiting.len();
    let mut now_applied = answers;
    for id in &waiting {
        match applied.get(id) {
            Some(before) => now_applied.insert(id.clone(), before.clone()),
            None => now_applied.remove(id),
        };
    }
    if now_applied != applied {
        let path = path(journal);
        std::fs::create_dir_all(path.parent().expect("health parent"))
            .map_err(|error| error.to_string())?;
        solstone_core_journal_io::write_json_checked(
            path,
            &Applied {
                version: VERSION,
                answers: now_applied,
            },
            solstone_core_journal_io::JsonWriteOptions::default(),
            checkpoint,
        )
        .map_err(|error| error.to_string())?;
    }
    log::info!(
        "name answers: {} changed, {} activities judged again, {} changed, {} waiting",
        outcome.answers,
        outcome.judged,
        outcome.changed,
        outcome.waiting
    );
    Ok(outcome)
}

fn activity_days(journal: &Path, facet: &str) -> Vec<String> {
    let mut days = std::fs::read_dir(journal.join("facets").join(facet).join("activities"))
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "jsonl").then(|| path.file_stem()?.to_str().map(str::to_owned))?
        })
        .filter(|day| solstone_core_journal_io::is_day_key(day))
        .collect::<Vec<_>>();
    days.sort_unstable();
    days
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const DAY: &str = "20260101";
    const SEGMENT: &str = "090000_60";

    fn write(path: PathBuf, text: String) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A facet with one meeting that heard "Ada" before the owner said who
    /// that is: listed under the bare name, not attending.
    fn facet_that_heard_ada(root: &Path, facet: &str) {
        solstone_core_facets::create_facet(root, facet, facet, "", "", "", None).unwrap();
        write(
            root.join(format!("facets/{facet}/entities/{DAY}.jsonl")),
            format!("{}\n", json!({"id":"ada","name":"Ada","type":"Person"})),
        );
        let said = json!([{"name":"Ada","role":"attendee","source":"voice"}]);
        write(
            root.join(format!("facets/{facet}/activities/{DAY}.jsonl")),
            format!(
                "{}\n",
                json!({"id":"meeting","segments":[SEGMENT],
                    "participation":[{"name":"Ada","role":"mentioned","source":"voice","entity_id":"ada"}],
                    "participation_said": said})
            ),
        );
    }

    fn journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        for (id, name) in [("ada_lovelace", "Ada Lovelace"), ("ada_byron", "Ada Byron")] {
            write(
                root.join(format!("entities/{id}/entity.json")),
                json!({"id":id,"name":name,"type":"Person"}).to_string(),
            );
        }
        let talents = root.join(format!("chronicle/{DAY}/{SEGMENT}/talents"));
        write(
            talents.join("sense.json"),
            json!({"meeting_detected":true}).to_string(),
        );
        write(
            talents.join("speaker_labels.json"),
            json!({"labels":[]}).to_string(),
        );
        write(talents.join("speakers.json"), json!(["Ada"]).to_string());
        facet_that_heard_ada(root, "work");
        facet_that_heard_ada(root, "home");
        journal
    }

    /// The journal asks who "Ada" is in a facet, and the owner answers.
    fn answer(root: &Path, facet: &str, entity: &str) {
        let scope = json!({"kind":"facet","facet":facet});
        let people =
            ["ada_lovelace", "ada_byron"].map(|id| solstone_core_entity::EntityResolutionEntity {
                id: Some(id.to_owned()),
                name: if id == "ada_lovelace" {
                    "Ada Lovelace"
                } else {
                    "Ada Byron"
                }
                .to_owned(),
                aka: Vec::new(),
                emails: Vec::new(),
                blocked: false,
            });
        solstone_core_entity::record_entity_resolution(
            root,
            "Ada",
            &people,
            scope.clone(),
            json!({"lane":"talent.participation","facet":facet,"day":DAY,"record_id":"meeting","field":"speaker.name"}),
            90.0,
            false,
        )
        .unwrap();
        solstone_core_entity::record_ambiguity_choice(
            root,
            &solstone_core_entity::AmbiguityChoiceRequest {
                scope,
                query: "Ada".to_owned(),
                entity_id: entity.to_owned(),
                origin: None,
            },
            &[solstone_core_entity::AmbiguityChoiceEntity {
                id: entity.to_owned(),
                blocked: false,
            }],
        )
        .unwrap();
    }

    fn listed(root: &Path, facet: &str) -> (String, String) {
        let record = solstone_core_facets::get_activity_record(root, facet, DAY, "meeting")
            .unwrap()
            .unwrap();
        let entry = &record["participation"][0];
        (
            entry["entity_id"].as_str().unwrap_or_default().to_owned(),
            entry["role"].as_str().unwrap_or_default().to_owned(),
        )
    }

    fn run(root: &Path) -> Outcome {
        pass(root, &|| Ok(())).unwrap()
    }

    #[test]
    fn a_past_meeting_follows_each_answer_in_the_facet_it_was_given_in() {
        let journal = journal();
        let root = journal.path();
        let unanswered = ("ada".to_owned(), "mentioned".to_owned());
        // No answers: nothing to do, and nothing written.
        assert_eq!(run(root), Outcome::default());
        assert!(!path(root).exists());

        answer(root, "work", "ada_lovelace");
        assert_eq!(
            run(root),
            Outcome {
                answers: 1,
                judged: 1,
                changed: 1,
                waiting: 0
            }
        );
        assert_eq!(
            listed(root, "work"),
            ("ada_lovelace".to_owned(), "attendee".to_owned())
        );
        // The other facet heard the same name and was not answered for.
        assert_eq!(listed(root, "home"), unanswered);
        // Applied once: the next pass does nothing.
        assert_eq!(run(root), Outcome::default());

        // The owner changes the answer, and the meeting follows it.
        answer(root, "work", "ada_byron");
        assert_eq!(run(root).changed, 1);
        assert_eq!(
            listed(root, "work"),
            ("ada_byron".to_owned(), "attendee".to_owned())
        );
        assert_eq!(listed(root, "home"), unanswered);
    }

    fn set_muted(root: &Path, facet: &str, muted: bool) {
        let path = root.join(format!("facets/{facet}/facet.json"));
        let mut declaration: Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        declaration["muted"] = json!(muted);
        std::fs::write(path, declaration.to_string()).unwrap();
    }

    #[test]
    fn an_answer_that_cant_be_applied_now_waits_and_is_applied_later() {
        let journal = journal();
        let root = journal.path();
        answer(root, "work", "ada_lovelace");
        set_muted(root, "work", true);

        let held = run(root);
        assert_eq!((held.changed, held.waiting), (0, 1));
        assert_eq!(listed(root, "work").0, "ada");
        // Still waiting on the next pass, not counted as done.
        assert_eq!(run(root).waiting, 1);

        set_muted(root, "work", false);
        let applied = run(root);
        assert_eq!((applied.changed, applied.waiting), (1, 0));
        assert_eq!(
            listed(root, "work"),
            ("ada_lovelace".to_owned(), "attendee".to_owned())
        );
        assert_eq!(run(root), Outcome::default());
    }

    #[test]
    fn a_person_removed_reopens_the_question_and_the_meeting_follows() {
        let journal = journal();
        let root = journal.path();
        answer(root, "work", "ada_lovelace");
        assert_eq!(run(root).changed, 1);
        // What removing the person does to the question: it is open again.
        let rows = root.join("entities/ambiguities.jsonl");
        let reopened = std::fs::read_to_string(&rows)
            .unwrap()
            .lines()
            .map(|line| {
                let mut row: Value = serde_json::from_str(line).unwrap();
                row["status"] = json!("open");
                row["resolved_entity_id"] = Value::Null;
                row["resolved_at"] = Value::Null;
                row.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(rows, format!("{reopened}\n")).unwrap();

        assert_eq!(run(root).changed, 1);
        // No one answers for the name now: it is the bare name the day
        // detected again, and nothing shows that name attending.
        assert_eq!(
            listed(root, "work"),
            ("ada".to_owned(), "mentioned".to_owned())
        );
        assert_eq!(run(root), Outcome::default());
    }

    #[test]
    fn losing_what_was_applied_costs_one_pass_that_changes_nothing() {
        let journal = journal();
        let root = journal.path();
        answer(root, "work", "ada_lovelace");
        assert_eq!(run(root).changed, 1);
        std::fs::remove_file(path(root)).unwrap();

        assert_eq!(
            run(root),
            Outcome {
                answers: 1,
                judged: 1,
                changed: 0,
                waiting: 0
            }
        );
        assert_eq!(run(root), Outcome::default());
    }
}
