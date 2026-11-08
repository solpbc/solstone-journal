// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::TimeZone;
use serde_json::{Value, json};
use solstone_core_generate::{GeneratedResponse, RefusalReason, RefusedResponse};
use solstone_core_import::strava_names::{STRAVA_LOCK, STRAVA_NAMES_FILE};

use super::*;

const LIVE: &str = r#"{"providers":{"active":{"provider":"local"}}}"#;

/// Noon in the owner's zone, which no test window holds.
fn noon() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 8, 19, 0, 0).unwrap()
}

/// A journal with the facets `body` and `other`, its zone and owner, and
/// thinking set up by `providers` and `processing`.
fn journal_with(processing: Value) -> tempfile::TempDir {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    let mut config = serde_json::from_str::<Value>(LIVE).unwrap();
    config["identity"] = json!({"name": "Jordan Lee", "timezone": "America/Denver"});
    if !processing.is_null() {
        config["processing"] = processing;
    }
    write_config(root, &config);
    for facet in ["body", "other"] {
        solstone_core_facets::create_facet(root, facet, facet, "Description", "blue", "🏃", None)
            .unwrap();
    }
    fs::create_dir_all(root.join("imports")).unwrap();
    journal
}

fn journal() -> tempfile::TempDir {
    journal_with(Value::Null)
}

fn write_config(root: &Path, config: &Value) {
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/journal.json"), config.to_string()).unwrap();
}

/// A workout as Strava's import writes it: its first tile at 07:00 on `day`,
/// filed under `facet`, and a second tile after it.
struct Workout<'a> {
    id: u64,
    day: &'a str,
    name: &'a str,
    fields: Value,
    facet: Option<&'a str>,
}

impl<'a> Workout<'a> {
    fn new(id: u64, day: &'a str, name: &'a str) -> Self {
        Self {
            id,
            day,
            name,
            fields: json!({}),
            facet: Some("body"),
        }
    }

    fn fields(mut self, fields: Value) -> Self {
        self.fields = fields;
        self
    }

    fn facet(mut self, facet: Option<&'a str>) -> Self {
        self.facet = facet;
        self
    }

    /// Write both tiles, returning the first's folder. A workout's tiles are
    /// keyed by its id, so workouts on one day don't collide.
    fn write(&self, root: &Path) -> PathBuf {
        let date = format!("{}-{}-{}", &self.day[..4], &self.day[4..6], &self.day[6..]);
        let start = format!("{date}T07:00:00-06:00");
        let stream = root.join("chronicle").join(self.day).join(STRAVA_STREAM);
        for (index, hour) in ["07", "08"].into_iter().enumerate() {
            let tile = json!({"schema": "solstone.import.strava.tile.v1",
                "import_id": "20261010_120000", "activity_id": self.id, "zone": "America/Denver",
                "tile": {"index": index, "count": 2, "start": start, "end": start, "seconds": 300},
                "workout": {"name": self.name, "type": "Ride", "start": start,
                    "elapsed_seconds": 600, "distance_m": 5000.0, "commute": false,
                    "entered_by_hand": false, "strava_fields": self.fields}});
            let dir = stream.join(format!("{hour}{:04}_300", self.id));
            fs::create_dir_all(dir.join("talents")).unwrap();
            fs::write(dir.join("workout.json"), format!("{tile}\n")).unwrap();
            if let Some(facet) = self.facet {
                fs::write(
                    dir.join("talents/facets.json"),
                    json!([{ "facet": facet }]).to_string(),
                )
                .unwrap();
            }
        }
        stream.join(format!("07{:04}_300", self.id))
    }
}

type Reply = Box<dyn Fn(&[String]) -> Result<GenerateResponse, ClientError>>;

/// A stand-in model: it answers from `reply`, given the words it was sent,
/// and records every call.
struct Model {
    reply: Reply,
    calls: RefCell<Vec<(WorkPriority, GenerateRequest)>>,
    lock: Option<PathBuf>,
}

impl Model {
    fn new(reply: impl Fn(&[String]) -> Result<GenerateResponse, ClientError> + 'static) -> Self {
        Self {
            reply: Box::new(reply),
            calls: RefCell::new(Vec::new()),
            lock: None,
        }
    }

    /// Answers with `entities`, whatever it's sent.
    fn answering(entities: Value) -> Self {
        Self::new(move |_| Ok(generated(&json!({ "entities": entities }).to_string())))
    }

    /// Never called.
    fn unused() -> Self {
        Self::new(|_| panic!("no call may be made"))
    }

    fn count(&self) -> usize {
        self.calls.borrow().len()
    }
}

impl NamesModel for Model {
    fn execute(
        &self,
        priority: WorkPriority,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ClientError> {
        if let Some(lock) = &self.lock {
            assert!(
                !solstone_core_journal_io::lock_is_held(lock).unwrap(),
                "a model call holds no lock"
            );
        }
        self.calls.borrow_mut().push((priority, request.clone()));
        let ContentPart::Text { text } = &request.contents[0] else {
            panic!("the words come first, as text");
        };
        let words = serde_json::from_str::<Value>(text).unwrap()["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|word| word.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        (self.reply)(&words)
    }
}

fn generated(text: &str) -> GenerateResponse {
    GenerateResponse::Generated(Box::new(GeneratedResponse {
        id: None,
        text: text.to_owned(),
        model: "stand-in".to_owned(),
        usage: Value::Null,
        finish_reason: "stop".to_owned(),
        thinking: None,
        schema_validation: None,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied: Vec::new(),
    }))
}

fn refused() -> GenerateResponse {
    GenerateResponse::Refused(RefusedResponse {
        id: None,
        reason: RefusalReason::ProviderResponseInvalid,
        reason_code: None,
        retryable: true,
        blocking: false,
        reset_at_ms: None,
        provider: None,
        detail: "synthetic".to_owned(),
    })
}

fn person(name: &str, span: &str) -> Value {
    json!({"name": name, "type": "Person", "words_span": span})
}

fn store(root: &Path) -> StravaNames {
    read_strava_names(root).unwrap()
}

fn names(line: &StravaNamesLine) -> Vec<(&str, &str)> {
    line.entities
        .iter()
        .map(|name| (name.entity_type.as_str(), name.name.as_str()))
        .collect()
}

fn run(root: &Path, model: &Model) -> StravaNamesOutcome {
    record_strava_names(root, model, noon())
}

#[test]
fn only_workouts_with_the_owners_words_are_read_and_gear_needs_no_call() {
    let journal = journal();
    let root = journal.path();
    Workout::new(1, "20261001", "Hill repeats with Sam").write(root);
    Workout::new(2, "20261002", "Morning Ride").write(root);
    Workout::new(3, "20261003", "").write(root);
    Workout::new(4, "20261004", "Radfahrt am Abend").write(root);
    Workout::new(5, "20261005", "Lunch Ride")
        .fields(json!({"Activity Gear": "Trek Domane SL6 (Blue Thunder)"}))
        .write(root);
    let model = Model::answering(json!([person("Sam", "with Sam")]));

    let outcome = run(root, &model);

    assert_eq!(model.count(), 1);
    assert_eq!(
        (
            outcome.workouts,
            outcome.calls,
            outcome.no_words,
            outcome.failed
        ),
        (5, 1, 4, 0)
    );
    assert_eq!(
        outcome.gear.iter().map(String::as_str).collect::<Vec<_>>(),
        ["Trek Domane SL6 (Blue Thunder)"]
    );
    let lines = store(root);
    assert_eq!(lines.len(), 5);
    assert_eq!(names(&lines["1"]), [("Person", "Sam")]);
    assert_eq!(lines["5"].gear, ["Trek Domane SL6 (Blue Thunder)"]);
    assert!(lines["5"].entities.is_empty());
    assert!(lines.values().all(|line| line.words_hash.is_some()));
}

#[test]
fn the_model_is_sent_only_the_words_as_data_at_import_priority() {
    let journal = journal();
    let root = journal.path();
    let mut model = Model::answering(json!([]));
    model.lock = Some(root.join(STRAVA_LOCK));
    Workout::new(7, "20261010", "Sonntagsrunde mit Anna")
        .fields(json!({"Aktivitätsbeschreibung": "Kalt und windig",
            "Activity Private Note": "Knie ok", "Aktivitätsausrüstung": "Canyon Endurace",
            "Media": "photo.jpg"}))
        .write(root);

    run(root, &model);

    let calls = model.calls.borrow();
    let (priority, request) = &calls[0];
    assert_eq!(priority.band, WorkBand::Import);
    assert_eq!(priority.source_time, 20_261_010_070_007);
    let texts = request
        .contents
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => text.clone(),
            other => panic!("only text is sent, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(texts.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(&texts[0]).unwrap(),
        json!({"words": ["Sonntagsrunde mit Anna", "Kalt und windig", "Knie ok"]})
    );
    assert_eq!(texts[1], NAMES_PROMPT);
    let sent = texts.join("\n");
    for absent in ["Canyon", "Strava", "Ride", "5000", "photo", "Media", "body"] {
        assert!(
            !sent.contains(absent),
            "{absent:?} is not the owner's words"
        );
    }
    assert_eq!(request.temperature, 0.0);
    assert!(request.json_output);
    assert!(request.json_schema.is_some());
}

#[test]
fn the_client_carries_the_import_band() {
    let priority = WorkPriority {
        band: WorkBand::Import,
        source_time: 20_261_010_070_000,
    };
    let (name, value) = priority_env(priority);
    assert_eq!(name, WORK_PRIORITY_ENV);
    assert_eq!(value, "4:20261010070000");
    assert_eq!(value.parse::<WorkPriority>().unwrap(), priority);
}

#[test]
fn the_same_download_makes_no_call_and_an_edit_makes_one() {
    let journal = journal();
    let root = journal.path();
    let model = Model::answering(json!([person("Sam", "with Sam")]));
    Workout::new(1, "20261001", "Hill repeats with Sam").write(root);
    Workout::new(2, "20261002", "Morning Ride").write(root);
    run(root, &model);
    assert_eq!(model.count(), 1);
    let first = fs::read(root.join(STRAVA_NAMES_FILE)).unwrap();

    let again = run(root, &model);
    assert_eq!(model.count(), 1);
    assert_eq!((again.calls, again.cached), (0, 2));
    assert_eq!(fs::read(root.join(STRAVA_NAMES_FILE)).unwrap(), first);

    Workout::new(1, "20261001", "Hill repeats with Sam")
        .fields(json!({"Activity Description": "Sam and Lee"}))
        .write(root);
    let edited = run(root, &model);
    assert_eq!(model.count(), 2);
    assert_eq!((edited.calls, edited.cached), (1, 1));
}

#[test]
fn a_name_not_in_the_words_or_of_another_type_is_dropped() {
    let journal = journal();
    let root = journal.path();
    Workout::new(1, "20261001", "Hill repeats with Sam")
        .fields(json!({"Activity Description": "New Pegasus shoes, Trek fitted"}))
        .write(root);
    let model = Model::answering(json!([
        person("Sam", "with Sam"),
        person("Barack", "with Barack"),
        person("Samuel", "with Sam"),
        {"name": "Pegasus", "type": "Tool", "words_span": "New Pegasus shoes"},
        {"name": "Trek", "type": "Company", "words_span": "Trek fitted"},
        {"name": "Lee", "type": "Person"},
    ]));

    run(root, &model);

    assert_eq!(
        names(&store(root)["1"]),
        [("Tool", "Pegasus"), ("Person", "Sam")]
    );
}

#[test]
fn a_failed_call_is_counted_not_kept_and_tried_again() {
    let journal = journal();
    let root = journal.path();
    Workout::new(1, "20261001", "Hill repeats with Sam").write(root);
    Workout::new(2, "20261002", "Tempo with Lee").write(root);
    for model in [
        Model::new(|_| Ok(refused())),
        Model::new(|_| Ok(generated("not json"))),
        Model::new(|_| Ok(generated(r#"{"people": []}"#))),
        Model::new(|_| Err(ClientError::Decode("synthetic".to_owned()))),
    ] {
        let outcome = run(root, &model);
        assert_eq!((outcome.calls, outcome.failed), (2, 2));
        assert!(outcome.store_error.is_none());
        let lines = store(root);
        assert_eq!(lines.len(), 2);
        assert!(lines.values().all(|line| line.words_hash.is_none()));
    }

    let model = Model::new(|words| {
        let name = if words[0].contains("Sam") {
            "Sam"
        } else {
            "Lee"
        };
        Ok(generated(
            &json!({"entities": [person(name, &format!("with {name}"))]}).to_string(),
        ))
    });
    let outcome = run(root, &model);
    assert_eq!((outcome.calls, outcome.failed), (2, 0));
    let lines = store(root);
    assert_eq!(names(&lines["1"]), [("Person", "Sam")]);
    assert_eq!(names(&lines["2"]), [("Person", "Lee")]);
}

#[test]
fn no_call_is_made_while_import_work_may_not_run() {
    let deferred = |window: Value| json!({"mode": "deferred", "gate": {"time_window": window}});
    for (processing, wait) in [
        (
            deferred(json!({"start": "01:00", "end": "02:00"})),
            NamesWait::WaitingForWindow,
        ),
        (deferred(json!({"enabled": false})), NamesWait::NoCondition),
    ] {
        let journal = journal_with(processing);
        let root = journal.path();
        Workout::new(1, "20261001", "Hill repeats with Sam")
            .fields(json!({"Activity Gear": "Road bike"}))
            .write(root);
        let outcome = run(root, &Model::unused());
        assert_eq!((outcome.calls, outcome.waiting), (0, 1));
        assert_eq!(outcome.wait, Some(wait));
        let line = &store(root)["1"];
        assert!(line.words_hash.is_none());
        assert_eq!(line.gear, ["Road bike"]);
    }

    let journal = journal();
    let root = journal.path();
    write_config(root, &json!({"identity": {"timezone": "America/Denver"}}));
    Workout::new(1, "20261001", "Hill repeats with Sam").write(root);
    let outcome = run(root, &Model::unused());
    assert_eq!(outcome.wait, Some(NamesWait::LaneUnavailable));

    let journal = journal_with(deferred(json!({"start": "11:00", "end": "13:00"})));
    let root = journal.path();
    Workout::new(1, "20261001", "Hill repeats with Sam").write(root);
    let model = Model::answering(json!([]));
    let outcome = run(root, &model);
    assert_eq!((outcome.calls, outcome.wait), (1, None));
}

#[test]
fn a_workout_no_longer_live_loses_its_line_and_counts_for_nothing() {
    let journal = journal();
    let root = journal.path();
    let model = Model::answering(json!([person("Sam", "with Sam")]));
    Workout::new(1, "20261001", "Hill repeats with Sam").write(root);
    let deleted = Workout::new(2, "20261002", "Hill repeats with Sam").write(root);
    // An earlier run kept a line for both, reading the first; the second, with
    // no facet there, wasn't read. It has been deleted since.
    let earlier = journal_with(Value::Null);
    Workout::new(1, "20261001", "Hill repeats with Sam").write(earlier.path());
    Workout::new(2, "20261002", "Hill repeats with Sam")
        .facet(None)
        .write(earlier.path());
    run(earlier.path(), &model);
    fs::copy(
        earlier.path().join(STRAVA_NAMES_FILE),
        root.join(STRAVA_NAMES_FILE),
    )
    .unwrap();
    assert_eq!(store(root).len(), 2);
    fs::remove_file(deleted.join("workout.json")).unwrap();
    fs::remove_dir_all(deleted.join("talents")).unwrap();
    fs::write(deleted.join(TOMBSTONE), "{}").unwrap();

    let outcome = run(root, &model);

    assert_eq!(model.count(), 1);
    assert_eq!(outcome.pruned, 1);
    assert_eq!(store(root).keys().collect::<Vec<_>>(), ["1"]);
    assert_eq!((outcome.created, outcome.below_threshold), (0, 1));
    assert!(!root.join("entities/sam").exists());
}

#[test]
fn a_name_becomes_an_entity_on_enough_distinct_workout_days_per_type() {
    let journal = journal();
    let root = journal.path();
    // Sam on two workouts of one day.
    Workout::new(1, "20261001", "Hill repeats with Sam").write(root);
    Workout::new(11, "20261001", "Hill repeats with Sam").write(root);
    // The bike, as gear, and the Pegasus, in the words, on four days.
    for (index, day) in ["20261002", "20261003", "20261004", "20261005"]
        .into_iter()
        .enumerate()
    {
        Workout::new(20 + index as u64, day, "Morning Ride")
            .fields(json!({"Activity Gear": "Road bike", "Activity Description": "Pegasus shoes"}))
            .write(root);
    }
    // The shoes, as gear, on five.
    for (index, day) in ["20261006", "20261007", "20261008", "20261009", "20261010"]
        .into_iter()
        .enumerate()
    {
        Workout::new(30 + index as u64, day, "Morning Ride")
            .fields(json!({"Activity Gear": "Fast shoes"}))
            .write(root);
    }
    let model = Model::new(|words| {
        let entities = if words[0].contains("Sam") {
            json!([person("Sam", "with Sam")])
        } else {
            json!([{"name": "Pegasus", "type": "Tool", "words_span": "Pegasus shoes"}])
        };
        Ok(generated(&json!({ "entities": entities }).to_string()))
    });

    let outcome = run(root, &model);

    assert_eq!((outcome.created, outcome.below_threshold), (1, 3));
    let identity = |id: &str| {
        serde_json::from_slice::<Value>(
            &fs::read(root.join("entities").join(id).join("entity.json")).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(identity("fast_shoes")["type"], "Tool");
    let link = serde_json::from_slice::<Value>(
        &fs::read(root.join("facets/body/entities/fast_shoes/entity.json")).unwrap(),
    )
    .unwrap();
    assert!(link["description"].as_str().unwrap().contains('5'));

    // A second day for Sam.
    Workout::new(2, "20261002", "Tempo with Sam").write(root);
    let model = Model::answering(json!([person("Sam", "with Sam")]));
    let outcome = run(root, &model);
    assert_eq!(model.count(), 1);
    assert_eq!((outcome.created, outcome.present), (1, 1));
    assert_eq!(identity("sam")["type"], "Person");

    // Both exist now: nothing is created or described again.
    let before = snapshot(root, &["entities", "facets"]);
    let outcome = run(root, &model);
    assert_eq!((outcome.created, outcome.present), (0, 2));
    assert_eq!(snapshot(root, &["entities", "facets"]), before);
}

#[test]
fn a_name_someone_already_has_or_the_owner_is_left_alone() {
    let journal = journal();
    let root = journal.path();
    let identity = |dir: &str, value: Value| {
        let path = root.join("entities").join(dir).join("entity.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value.to_string()).unwrap();
    };
    identity(
        "sam_chen",
        json!({"id": "sam_chen", "name": "Sam Chen", "type": "Person", "aka": ["Sam"]}),
    );
    let link = root.join("facets/other/entities/sam_chen/entity.json");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    fs::write(
        &link,
        json!({"entity_id": "sam_chen", "description": "a friend", "detached": true}).to_string(),
    )
    .unwrap();
    for (id, day) in [(1, "20261001"), (2, "20261002")] {
        Workout::new(id, day, "Hill repeats with Sam and Jordan Lee").write(root);
    }
    let model = Model::answering(json!([
        person("Sam", "with Sam"),
        person("Jordan Lee", "Jordan Lee"),
    ]));
    let before = snapshot(root, &["entities", "facets"]);

    let outcome = run(root, &model);

    assert_eq!((outcome.created, outcome.present), (0, 2));
    assert_eq!(snapshot(root, &["entities", "facets"]), before);
}

#[test]
fn a_workout_with_no_facet_gives_no_entity() {
    let journal = journal();
    let root = journal.path();
    for (id, day) in [(1, "20261001"), (2, "20261002")] {
        Workout::new(id, day, "Hill repeats with Sam")
            .facet(None)
            .write(root);
    }
    let model = Model::answering(json!([person("Sam", "with Sam")]));

    let outcome = run(root, &model);

    assert_eq!((outcome.unfiled, outcome.created), (2, 0));
    assert!(!root.join("entities").exists());
}

#[test]
fn a_workout_with_no_facet_is_read_only_once_it_has_one() {
    let journal = journal();
    let root = journal.path();
    Workout::new(1, "20261001", "Hill repeats with Sam")
        .facet(None)
        .write(root);
    let model = Model::answering(json!([person("Sam", "with Sam")]));

    let outcome = run(root, &model);
    assert_eq!((model.count(), outcome.calls, outcome.unfiled), (0, 0, 1));

    // The same download, imported with body.
    Workout::new(1, "20261001", "Hill repeats with Sam").write(root);
    let outcome = run(root, &model);
    assert_eq!((model.count(), outcome.calls, outcome.unfiled), (1, 1, 0));
}

#[test]
fn a_name_in_a_muted_facet_becomes_no_entity() {
    let journal = journal();
    let root = journal.path();
    for (id, day) in [(1, "20261001"), (2, "20261002")] {
        Workout::new(id, day, "Hill repeats with Sam").write(root);
    }
    solstone_core_facets::set_facet_muted(root, "body", true).unwrap();
    let model = Model::answering(json!([person("Sam", "with Sam")]));

    let outcome = run(root, &model);

    assert_eq!((outcome.muted, outcome.created), (1, 0));
    assert!(!root.join("entities").exists());
}

#[test]
fn no_days_evidence_moves() {
    let journal = journal();
    let root = journal.path();
    for (id, day) in [(1, "20261001"), (2, "20261002")] {
        Workout::new(id, day, "Hill repeats with Sam")
            .fields(json!({"Activity Gear": "Road bike"}))
            .write(root);
    }
    for facet in ["body", "other"] {
        let entities = root.join("facets").join(facet).join("entities");
        fs::create_dir_all(&entities).unwrap();
        fs::write(entities.join("20261001.jsonl"), "{\"name\":\"Lee\"}\n").unwrap();
        let activities = root.join("facets").join(facet).join("activities");
        fs::create_dir_all(&activities).unwrap();
        fs::write(activities.join("20261001.jsonl"), "{\"id\":\"a\"}\n").unwrap();
    }
    let evidence = |root: &Path| {
        let mut files = snapshot(root, &["chronicle"]);
        for (path, bytes) in snapshot(root, &["facets"]) {
            let text = path.to_string_lossy().replace('\\', "/");
            let day_detections = text.contains("/entities/")
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".jsonl"));
            if day_detections || text.contains("/activities/") || text.ends_with("/activities") {
                files.insert(path, bytes);
            }
        }
        files
    };
    let before = evidence(root);
    let model = Model::answering(json!([person("Sam", "with Sam")]));

    let outcome = run(root, &model);

    assert_eq!(outcome.created, 1);
    assert_eq!(evidence(root), before);
}

/// Every directory and file under `roots` with its bytes.
fn snapshot(root: &Path, roots: &[&str]) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                into.insert(relative, None);
                walk(root, &path, into);
            } else {
                into.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }
    let mut files = BTreeMap::new();
    for name in roots {
        walk(root, &root.join(name), &mut files);
    }
    files
}
