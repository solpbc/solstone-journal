// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A Strava import end to end: the facet it files workouts under, the records
//! it writes, the names step after it, and what it says it did.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_format::content::{Family, produce_chunks};
use solstone_core_generate::{ClientError, GenerateRequest, GenerateResponse, GeneratedResponse};
use solstone_core_journal_config::BodyFacetChoice;
use solstone_core_journal_config::work_priority::WorkPriority;

use super::tests::{HEADER_ENGLISH_103, build_csv};
use super::*;
use crate::cli_argv::ImportFacet;
use crate::strava_names::NamesModel;

/// Reads every workout as naming Sam, and checks no call holds Strava's lock.
struct Model {
    lock: PathBuf,
    calls: Cell<usize>,
}

impl NamesModel for Model {
    fn execute(
        &self,
        _priority: WorkPriority,
        _request: &GenerateRequest,
    ) -> Result<GenerateResponse, ClientError> {
        assert!(
            !solstone_core_journal_io::lock_is_held(&self.lock).unwrap(),
            "a model call holds no lock"
        );
        self.calls.set(self.calls.get() + 1);
        Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text:
                json!({"entities": [{"name": "Sam", "type": "Person", "words_span": "with Sam"}]})
                    .to_string(),
            model: "stand-in".to_owned(),
            usage: Value::Null,
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        })))
    }
}

/// A journal on UTC whose thinking may run import work now, with `facets`.
fn journal(facets: &[&str]) -> tempfile::TempDir {
    let journal = tempfile::tempdir().unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        json!({"identity": {"timezone": "UTC"}, "providers": {"active": {"provider": "local"}}})
            .to_string(),
    )
    .unwrap();
    for facet in facets {
        solstone_core_facets::create_facet(root, facet, facet, "", "", "", None).unwrap();
    }
    journal
}

/// A download of three workouts: one with the owner's words, and two with
/// Strava's own names, each on its own day.
fn download(root: &Path, description: &str) -> PathBuf {
    let rows = [
        (
            "901",
            "Oct 4, 2026, 07:00:00 AM",
            "Hill repeats",
            description,
            "600",
        ),
        ("902", "Oct 5, 2026, 09:00:00 AM", "Morning Run", "", "300"),
        ("903", "Oct 6, 2026, 06:00:00 AM", "Morning Run", "", "300"),
    ];
    let quoted = rows
        .iter()
        .map(|(id, date, name, description, elapsed)| {
            vec![
                ("Activity ID", 1, (*id).to_owned()),
                ("Activity Date", 1, format!("\"{date}\"")),
                ("Activity Name", 1, (*name).to_owned()),
                ("Activity Type", 1, "Run".to_owned()),
                ("Activity Description", 1, (*description).to_owned()),
                ("Elapsed Time", 1, (*elapsed).to_owned()),
                ("Elapsed Time", 2, (*elapsed).to_owned()),
                ("Moving Time", 1, (*elapsed).to_owned()),
                ("Distance", 1, "5.0".to_owned()),
                ("Distance", 2, "5000".to_owned()),
            ]
        })
        .collect::<Vec<_>>();
    let tuples = quoted
        .iter()
        .map(|row| {
            row.iter()
                .map(|(name, occurrence, value)| (*name, *occurrence, value.as_str()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let slices = tuples.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let path = root.join("activities.csv");
    fs::write(&path, build_csv(HEADER_ENGLISH_103, &slices)).unwrap();
    path
}

/// Import `csv` as `timestamp` under `facet`, returning its run and import id.
fn import(
    root: &Path,
    csv: &Path,
    timestamp: &str,
    facet: ImportFacet,
    model: &Model,
) -> (CliRun, String) {
    let dispatch = RegistryDispatch {
        source: RegistrySource::Strava,
        media: csv.to_path_buf(),
        timestamp: timestamp.to_owned(),
        dry_run: false,
        json: false,
        force: false,
        owner_timestamp: None,
        fallback_timestamp: None,
        facet,
    };
    let hook = StravaRunHook {
        names_model: Some(model),
        ..StravaRunHook::default()
    };
    let mut selected = None;
    let run = run_strava_hooked(dispatch, root, &mut selected, Some(&hook));
    (run, selected.unwrap_or_default())
}

fn tile(root: &Path, day: &str, key: &str) -> PathBuf {
    root.join("chronicle")
        .join(day)
        .join("import.strava")
        .join(key)
}

fn tile_facet(root: &Path, day: &str, key: &str) -> Option<String> {
    crate::body_activity::window_facet(&tile(root, day, key))
}

/// Every activity record line in `facet` on `day`.
fn records(root: &Path, facet: &str, day: &str) -> Vec<Value> {
    fs::read_to_string(root.join(format!("facets/{facet}/activities/{day}.jsonl")))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn published(root: &Path, id: &str) -> Vec<String> {
    solstone_core_import::publish::read_publication_record(&root.join("imports").join(id))
        .unwrap()
        .map(|record| {
            record
                .segments
                .into_iter()
                .map(|segment| format!("{}/{}", segment.day, segment.segment))
                .collect()
        })
        .unwrap_or_default()
}

fn count(run: &CliRun, key: &str) -> u64 {
    run.stdout
        .split_whitespace()
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("{key} in {}", run.stdout))
        .parse()
        .unwrap()
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(dir: &Path, into: &mut Vec<(PathBuf, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, into);
            } else if !path.to_string_lossy().ends_with(".lock") {
                into.push((path.clone(), fs::read(&path).unwrap_or_default()));
            }
        }
    }
    let mut files = Vec::new();
    for top in ["chronicle", "facets"] {
        walk(&root.join(top), &mut files);
    }
    files.sort();
    files
}

#[test]
fn a_strava_import_files_records_and_reads_names_once_its_tiles_are_published() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "easy with Sam");

    // With no facet: no tile gets one, and no workout a record.
    let (run, _) = import(root, &csv, "20261010_100000", ImportFacet::None, &model);
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(
        (
            count(&run, "records_written"),
            count(&run, "records_no_facet")
        ),
        (0, 3)
    );
    assert_eq!(tile_facet(root, "20261004", "070000_300"), None);
    // The one workout with words has no facet, so its words aren't read yet.
    assert_eq!(
        (
            count(&run, "names_calls"),
            count(&run, "names_unfiled"),
            model.calls.get()
        ),
        (0, 1, 0)
    );

    // An earlier reading filed the second workout's tile under work, with its
    // record; the third workout's day can't be read.
    let sensed = tile(root, "20261005", "090000_300");
    fs::create_dir_all(sensed.join("talents")).unwrap();
    fs::write(sensed.join("talents/facets.json"), r#"[{"facet":"work"}]"#).unwrap();
    let sense_record = json!({"id": "run_090000", "facet": "work", "activity": "running",
        "segments": ["090000_300"], "level_avg": 0.5, "description": "a run",
        "active_entities": [], "created_at": 1, "stream": "import.strava"});
    fs::create_dir_all(root.join("facets/work/activities")).unwrap();
    fs::write(
        root.join("facets/work/activities/20261005.jsonl"),
        format!("{sense_record}\n"),
    )
    .unwrap();
    let unreadable = root.join("facets/work/activities/20261006.jsonl");
    fs::create_dir_all(&unreadable).unwrap();

    // The same download with body: the unsensed workout's tiles move to body
    // and are published again, and it gets its record there. The sensed one
    // keeps its tile and record under work, the unreadable one its tile, and
    // both are counted.
    let (run, id) = import(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Facet("body".into()),
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    for key in ["070000_300", "070500_300"] {
        assert_eq!(tile_facet(root, "20261004", key).as_deref(), Some("body"));
    }
    assert_eq!(
        tile_facet(root, "20261005", "090000_300").as_deref(),
        Some("work")
    );
    assert_eq!(tile_facet(root, "20261006", "060000_300"), None);
    assert_eq!(
        published(root, &id),
        ["20261004/070000_300", "20261004/070500_300"]
    );
    assert_eq!(
        (
            count(&run, "filed"),
            count(&run, "kept_sensed"),
            count(&run, "kept_unreadable")
        ),
        (1, 1, 1)
    );
    assert_eq!(
        (
            count(&run, "records_written"),
            count(&run, "records_kept"),
            count(&run, "records_refused")
        ),
        (1, 1, 1)
    );
    assert_eq!(records(root, "body", "20261004").len(), 1);
    assert_eq!(
        records(root, "work", "20261005"),
        std::slice::from_ref(&sense_record)
    );
    // Filed under body, its words are read now, after the lock was released;
    // the two with none aren't read again.
    assert_eq!(
        (
            count(&run, "names_calls"),
            count(&run, "names_cached"),
            model.calls.get()
        ),
        (1, 2, 1)
    );
    assert!(root.join(format!("imports/{id}/summary.md")).is_file());

    // The day reads again: the third workout is filed and recorded too.
    fs::remove_dir(&unreadable).unwrap();
    let (run, _) = import(
        root,
        &csv,
        "20261010_120000",
        ImportFacet::Facet("body".into()),
        &model,
    );
    assert_eq!(
        (count(&run, "filed"), count(&run, "records_written")),
        (1, 1)
    );
    assert_eq!(
        tile_facet(root, "20261006", "060000_300").as_deref(),
        Some("body")
    );

    // The same download once more: no tile written, none published, no record.
    let before = snapshot(root);
    let (run, id) = import(
        root,
        &csv,
        "20261010_130000",
        ImportFacet::Facet("body".into()),
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert!(published(root, &id).is_empty());
    assert_eq!(
        (
            count(&run, "updated"),
            count(&run, "filed"),
            count(&run, "records_written"),
            count(&run, "names_calls")
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(snapshot(root), before);

    // An edited description: the workout's tiles are rewritten and published
    // again, and what search reads of it changes.
    let csv = download(root, "hard with Sam");
    let (run, id) = import(
        root,
        &csv,
        "20261010_140000",
        ImportFacet::Facet("body".into()),
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(
        published(root, &id),
        ["20261004/070000_300", "20261004/070500_300"]
    );
    let rel = "chronicle/20261004/import.strava/070000_300/workout.json";
    let text = fs::read_to_string(root.join(rel)).unwrap();
    let rendered = produce_chunks(Family::Workout, rel, &text)
        .chunks
        .into_iter()
        .map(|chunk| chunk.content)
        .collect::<String>();
    assert!(rendered.contains("hard with Sam"), "{rendered}");
    assert!(!rendered.contains("easy with Sam"), "{rendered}");
    assert_eq!(count(&run, "names_calls"), 1);
    assert_eq!(records(root, "body", "20261004").len(), 1);
}

#[test]
fn an_import_nobody_chose_for_never_makes_a_facet_and_a_muted_one_is_refused() {
    let journal = journal(&["work", "body"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "");
    let facets = || {
        let inventory = solstone_core_facets::observe_declared_facet_inventory(root).unwrap();
        (inventory.enabled, inventory.muted)
    };

    // Nothing remembered: no facet, and none is made.
    let (run, _) = import(
        root,
        &csv,
        "20261010_100000",
        ImportFacet::Unattended,
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert!(run.stdout.contains(" facet=none "), "{}", run.stdout);
    assert_eq!(tile_facet(root, "20261004", "070000_300"), None);
    assert_eq!(facets(), (vec!["body".into(), "work".into()], vec![]));

    // A remembered, enabled facet is used.
    let mut config: Value =
        serde_json::from_slice(&fs::read(root.join("config/journal.json")).unwrap()).unwrap();
    config["imports"] = json!({"body_facet": solstone_core_journal_config::body_facet_value(
        &BodyFacetChoice::Facet("body".into())
    )});
    fs::write(root.join("config/journal.json"), config.to_string()).unwrap();
    let (run, _) = import(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Unattended,
        &model,
    );
    assert!(run.stdout.contains(" facet=body "), "{}", run.stdout);
    assert_eq!(
        tile_facet(root, "20261004", "070000_300").as_deref(),
        Some("body")
    );

    // Once it's muted, nobody's import uses it or makes another.
    solstone_core_facets::set_facet_muted(root, "body", true).unwrap();
    let (run, _) = import(
        root,
        &csv,
        "20261010_120000",
        ImportFacet::Unattended,
        &model,
    );
    assert!(run.stdout.contains(" facet=none "), "{}", run.stdout);
    assert_eq!(facets(), (vec!["work".into()], vec!["body".into()]));

    // Chosen while muted, it refuses the import before anything is written.
    let fresh = self::journal(&["work", "body"]);
    solstone_core_facets::set_facet_muted(fresh.path(), "body", true).unwrap();
    let csv = download(fresh.path(), "");
    let (run, _) = import(
        fresh.path(),
        &csv,
        "20261010_100000",
        ImportFacet::Facet("body".into()),
        &model,
    );
    assert_eq!(run.exit_code, 1, "{}", run.stdout);
    assert!(!fresh.path().join("chronicle").exists());
    assert!(!fresh.path().join("imports/20261010_100000").exists());
}
