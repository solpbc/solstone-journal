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
    import_stopping(root, csv, timestamp, facet, model, false)
}

/// [`import`], saying how many listed workouts the owner was shown would move.
fn import_shown(
    root: &Path,
    csv: &Path,
    timestamp: &str,
    facet: ImportFacet,
    model: &Model,
    shown: Option<usize>,
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
        listed_moves_shown: shown,
    };
    let hook = StravaRunHook {
        names_model: Some(model),
        ..StravaRunHook::default()
    };
    let mut selected = None;
    let run = run_strava_hooked(dispatch, root, &mut selected, Some(&hook));
    (run, selected.unwrap_or_default())
}

/// [`import`], stopped just before the import is marked complete when `stop`.
fn import_stopping(
    root: &Path,
    csv: &Path,
    timestamp: &str,
    facet: ImportFacet,
    model: &Model,
    stop: bool,
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
        listed_moves_shown: None,
    };
    let hook = StravaRunHook {
        names_model: Some(model),
        stop_before_finish: stop,
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

/// A download listed under `work`, then imported again under `body`.
fn moved_with(shown: Option<usize>) -> (tempfile::TempDir, String) {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "easy with Sam");
    let (run, _) = import(
        root,
        &csv,
        "20261010_100000",
        ImportFacet::Facet("work".into()),
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let (run, id) = import_shown(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Facet("body".into()),
        &model,
        shown,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let summary = fs::read_to_string(root.join("imports").join(id).join("summary.md")).unwrap();
    (journal, summary)
}

#[test]
fn a_facet_named_on_the_command_line_says_how_many_moved_and_from_where() {
    let (journal, summary) = moved_with(None);
    assert!(
        summary.contains("3 workouts moved to this import's facet, from your work facet."),
        "{summary}"
    );
    assert_eq!(records(journal.path(), "body", "20261004").len(), 1);
}

#[test]
fn the_summary_counts_what_moved_and_not_what_was_going_to() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "easy with Sam");
    let (run, _) = import(
        root,
        &csv,
        "20261010_100000",
        ImportFacet::Facet("work".into()),
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    // One workout goes back to being a line an earlier reading filed, and its
    // group is stopped before anything of it is written.
    const DAY: &str = "20261004";
    let segments = fs::read_dir(root.join("chronicle").join(DAY).join("import.strava"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let line = json!({"id": "exercise_070000_300", "activity": "exercise", "segments": segments,
        "created_at": 1, "stream": "import.strava"});
    fs::write(
        root.join(format!("facets/work/activities/{DAY}.jsonl")),
        format!("{line}\n"),
    )
    .unwrap();
    crate::strava_convert::WRITES_SEEN.set(0);
    crate::strava_convert::STOP_AFTER.set(Some(1));
    let (run, id) = import(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Facet("body".into()),
        &model,
    );
    crate::strava_convert::STOP_AFTER.set(None);
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let summary = fs::read_to_string(root.join("imports").join(id).join("summary.md")).unwrap();
    assert!(
        summary.contains("2 workouts moved to this import's facet, from your work facet."),
        "{summary}"
    );
}

#[test]
fn a_run_moves_what_the_owner_was_shown() {
    let (journal, summary) = moved_with(Some(3));
    assert!(
        summary.contains("moved to this import's facet, from your work facet."),
        "{summary}"
    );
    assert_eq!(records(journal.path(), "body", "20261004").len(), 1);
}

#[test]
fn more_to_move_than_the_owner_saw_moves_none_and_says_so() {
    let (journal, summary) = moved_with(Some(2));
    assert!(
        summary
            .contains("nothing already filed moved: 3 workouts would have, and you were told 2."),
        "{summary}"
    );
    assert!(!summary.contains("moved to this import"), "{summary}");
    let root = journal.path();
    assert_eq!(records(root, "work", "20261004").len(), 1);
    assert!(records(root, "body", "20261004").is_empty());
    assert_eq!(
        tile_facet(root, "20261004", "070000_300").as_deref(),
        Some("work")
    );
}

#[test]
fn an_import_nobody_answered_for_moves_nothing_already_listed() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "easy with Sam");
    let (run, _) = import(
        root,
        &csv,
        "20261010_100000",
        ImportFacet::Facet("work".into()),
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(records(root, "work", "20261004").len(), 1);
    solstone_core_journal_config_write::record_body_facet(
        root,
        &BodyFacetChoice::Facet("body".into()),
    )
    .unwrap();

    let (run, _) = import(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Unattended,
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(count(&run, "filed"), 0);
    assert_eq!(count(&run, "kept_sensed"), 3);
    assert_eq!(
        tile_facet(root, "20261004", "070000_300").as_deref(),
        Some("work")
    );
    assert_eq!(records(root, "work", "20261004").len(), 1);
    assert!(records(root, "body", "20261004").is_empty());
}

#[cfg(feature = "full-tests")]
#[test]
fn a_remembered_facet_files_a_new_strava_workout_separately_from_a_revised_one() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "first with Sam");
    let (first, _) = import(
        root,
        &csv,
        "20261010_100000",
        ImportFacet::Facet("work".into()),
        &model,
    );
    assert_eq!(first.exit_code, 0, "{}", first.stderr);
    solstone_core_journal_config_write::record_body_facet(
        root,
        &BodyFacetChoice::Facet("body".into()),
    )
    .unwrap();
    let csv = download(root, "revised with Sam");
    let mut bytes = fs::read_to_string(&csv).unwrap();
    let another = bytes.lines().nth(1).unwrap().replacen("901", "904", 1);
    if !bytes.ends_with('\n') {
        bytes.push('\n');
    }
    bytes.push_str(&another);
    bytes.push('\n');
    fs::write(&csv, bytes).unwrap();
    let (result, _) = import(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Unattended,
        &model,
    );
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(count(&result, "filed"), 1);
    assert_eq!(count(&result, "kept_sensed"), 3);
    assert_eq!(
        tile_facet(root, "20261004", "070000_300").as_deref(),
        Some("work")
    );
    assert_eq!(
        tile_facet(root, "20261004", "070000_301").as_deref(),
        Some("body")
    );
    assert!(
        fs::read_to_string(tile(root, "20261004", "070000_300").join("workout.json"))
            .unwrap()
            .contains("revised with Sam")
    );
    assert_eq!(records(root, "work", "20261004").len(), 1);
    assert_eq!(records(root, "body", "20261004").len(), 1);
}

#[test]
#[cfg(feature = "full-tests")]
fn strava_keeps_an_earlier_filing_after_its_activity_record_is_removed() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "first with Sam");
    let (first, _) = import(
        root,
        &csv,
        "20261010_100000",
        ImportFacet::Facet("work".into()),
        &model,
    );
    assert_eq!(first.exit_code, 0, "{}", first.stderr);
    fs::remove_file(root.join("facets/work/activities/20261004.jsonl")).unwrap();
    solstone_core_journal_config_write::record_body_facet(
        root,
        &BodyFacetChoice::Facet("body".into()),
    )
    .unwrap();
    let csv = download(root, "revised with Sam");
    let (result, _) = import(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Unattended,
        &model,
    );
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(
        tile_facet(root, "20261004", "070000_300").as_deref(),
        Some("work")
    );
    assert_eq!(records(root, "work", "20261004").len(), 1);
    assert!(records(root, "body", "20261004").is_empty());
}

#[test]
#[cfg(feature = "full-tests")]
fn strava_changed_duration_keeps_the_original_tiling_and_creates_no_new_tile() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "first with Sam");
    let (first, _) = import(
        root,
        &csv,
        "20261010_100000",
        ImportFacet::Facet("work".into()),
        &model,
    );
    assert_eq!(first.exit_code, 0, "{}", first.stderr);
    solstone_core_journal_config_write::record_body_facet(
        root,
        &BodyFacetChoice::Facet("body".into()),
    )
    .unwrap();
    let bytes = fs::read_to_string(&csv).unwrap().replace("600", "900");
    fs::write(&csv, bytes).unwrap();
    let (result, _) = import(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Unattended,
        &model,
    );
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(count(&result, "present_timing_changed"), 1);
    assert!(!tile(root, "20261004", "071000_300").exists());
    assert_eq!(
        tile_facet(root, "20261004", "070500_300").as_deref(),
        Some("work")
    );
    assert!(records(root, "body", "20261004").is_empty());
}

#[test]
#[cfg(feature = "full-tests")]
fn strava_a_recreated_tile_keeps_its_existing_workouts_facet() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "first with Sam");
    let (first, _) = import(
        root,
        &csv,
        "20261010_100000",
        ImportFacet::Facet("work".into()),
        &model,
    );
    assert_eq!(first.exit_code, 0, "{}", first.stderr);
    solstone_core_journal_config_write::record_body_facet(
        root,
        &BodyFacetChoice::Facet("body".into()),
    )
    .unwrap();
    fs::remove_dir_all(tile(root, "20261004", "070500_300")).unwrap();
    let (result, _) = import(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Unattended,
        &model,
    );
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(
        tile_facet(root, "20261004", "070500_300").as_deref(),
        Some("work"),
        "tiles: {:?}",
        fs::read_dir(root.join("chronicle/20261004/import.strava"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>()
    );
    assert_eq!(records(root, "work", "20261004").len(), 1);
    assert!(records(root, "body", "20261004").is_empty());
}

#[cfg(feature = "full-tests")]
#[test]
fn strava_no_facet_keeps_revised_tiles_filed_and_leaves_a_new_workout_unfiled() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "first with Sam");
    assert_eq!(
        import(
            root,
            &csv,
            "20261010_100000",
            ImportFacet::Facet("work".into()),
            &model
        )
        .0
        .exit_code,
        0
    );
    fs::remove_dir_all(tile(root, "20261004", "070500_300")).unwrap();
    let mut bytes = fs::read_to_string(&csv).unwrap();
    let another = bytes.lines().nth(1).unwrap().replacen("901", "904", 1);
    bytes.push('\n');
    bytes.push_str(&another);
    bytes.push('\n');
    fs::write(&csv, bytes).unwrap();
    let (result, _) = import(root, &csv, "20261010_110000", ImportFacet::None, &model);
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(
        tile_facet(root, "20261004", "070500_300").as_deref(),
        Some("work")
    );
    assert_eq!(tile_facet(root, "20261004", "070000_301"), None);
    assert_eq!(count(&result, "filed"), 0);
    assert!(records(root, "body", "20261004").is_empty());
}

#[cfg(feature = "full-tests")]
#[test]
fn strava_tile_only_filings_obey_the_move_bound_and_new_workouts_use_the_run_facet() {
    for shown in [0, 1] {
        let journal = journal(&["body", "work"]);
        let root = journal.path();
        let model = Model {
            lock: root.join("imports/.strava"),
            calls: Cell::new(0),
        };
        let csv = download(root, "first with Sam");
        let original = fs::read_to_string(&csv)
            .unwrap()
            .lines()
            .take(2)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&csv, &original).unwrap();
        assert_eq!(
            import(
                root,
                &csv,
                "20261010_100000",
                ImportFacet::Facet("work".into()),
                &model
            )
            .0
            .exit_code,
            0
        );
        fs::remove_file(root.join("facets/work/activities/20261004.jsonl")).unwrap();
        let another = original.lines().nth(1).unwrap().replacen("901", "904", 1);
        fs::write(&csv, format!("{original}{another}\n")).unwrap();
        let (result, id) = import_shown(
            root,
            &csv,
            "20261010_110000",
            ImportFacet::Facet("body".into()),
            &model,
            Some(shown),
        );
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(
            tile_facet(root, "20261004", "070000_300").as_deref(),
            Some(if shown == 0 { "work" } else { "body" })
        );
        assert_eq!(
            tile_facet(root, "20261004", "070000_301").as_deref(),
            Some("body")
        );
        let text = fs::read_to_string(root.join("imports").join(id).join("summary.md")).unwrap();
        assert!(
            text.contains(if shown == 0 {
                "nothing already filed moved: 1 workout would have"
            } else {
                "1 workout moved"
            }),
            "{text}"
        );
    }
}

#[cfg(feature = "full-tests")]
#[test]
fn strava_unreadable_prior_assignments_do_not_become_absent_filings() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "first with Sam");
    assert_eq!(
        import(
            root,
            &csv,
            "20261010_100000",
            ImportFacet::Facet("work".into()),
            &model
        )
        .0
        .exit_code,
        0
    );
    fs::remove_file(root.join("facets/work/activities/20261004.jsonl")).unwrap();
    let path = tile(root, "20261004", "070000_300").join("talents/facets.json");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    solstone_core_journal_config_write::record_body_facet(
        root,
        &BodyFacetChoice::Facet("body".into()),
    )
    .unwrap();
    let (result, _) = import(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Unattended,
        &model,
    );
    assert_ne!(result.exit_code, 0);
    assert!(
        result.stderr.contains("couldn't be filed"),
        "{}",
        result.stderr
    );
    assert!(path.is_dir());
    assert_eq!(
        tile_facet(root, "20261004", "070500_300").as_deref(),
        Some("work")
    );
    assert!(records(root, "body", "20261004").is_empty());
}

#[cfg(feature = "full-tests")]
#[test]
fn strava_a_retained_assignment_repair_uses_its_record_and_survives_until_publication() {
    let journal = journal(&["body", "work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "first with Sam");
    assert_eq!(
        import(
            root,
            &csv,
            "20261010_100000",
            ImportFacet::Facet("work".into()),
            &model
        )
        .0
        .exit_code,
        0
    );
    for key in ["070000_300", "070500_300"] {
        fs::remove_file(tile(root, "20261004", key).join("talents/facets.json")).unwrap();
    }
    solstone_core_journal_config_write::record_body_facet(
        root,
        &BodyFacetChoice::Facet("body".into()),
    )
    .unwrap();
    let (stopped, _) = import_stopping(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Unattended,
        &model,
        true,
    );
    assert_eq!(stopped.exit_code, 0, "{}", stopped.stderr);
    let pending: std::collections::BTreeSet<(String, String)> = serde_json::from_slice(
        &fs::read(root.join(solstone_core_import::strava_refile::STRAVA_REFILE_FILE)).unwrap(),
    )
    .unwrap();
    assert!(pending.contains(&("20261004".to_owned(), "070500_300".to_owned())));
    assert_eq!(
        tile_facet(root, "20261004", "070500_300").as_deref(),
        Some("work")
    );
    let (result, id) = import(
        root,
        &csv,
        "20261010_120000",
        ImportFacet::Unattended,
        &model,
    );
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert!(
        published(root, &id)
            .iter()
            .any(|key| key.ends_with("/070500_300"))
    );
    assert!(
        !root
            .join(solstone_core_import::strava_refile::STRAVA_REFILE_FILE)
            .exists()
    );
    assert_eq!(count(&result, "filed"), 0);
    assert!(records(root, "body", "20261004").is_empty());
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

    // The same download with body, the owner's answer: the unsensed workout's
    // tiles move to body and are published again, and it gets its record there.
    // The sensed one's record becomes a code record and goes with its tile to
    // body too. The unreadable one keeps its tile, and is counted.
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
        Some("body")
    );
    assert_eq!(tile_facet(root, "20261006", "060000_300"), None);
    assert_eq!(
        published(root, &id),
        [
            "20261004/070000_300",
            "20261004/070500_300",
            "20261005/090000_300"
        ]
    );
    assert_eq!(
        (
            count(&run, "filed"),
            count(&run, "kept_sensed"),
            count(&run, "kept_unreadable")
        ),
        (1, 0, 1)
    );
    assert_eq!(
        (
            count(&run, "records_written"),
            count(&run, "records_kept"),
            count(&run, "records_refused")
        ),
        (1, 0, 1)
    );
    assert_eq!(records(root, "body", "20261004").len(), 1);
    assert!(records(root, "work", "20261005").is_empty());
    let converted = records(root, "body", "20261005");
    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0]["source"], "body");
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

    // Choosing no facet on a later import removes nothing and moves nothing.
    let (run, _) = import(root, &csv, "20261010_145000", ImportFacet::None, &model);
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(
        tile_facet(root, "20261004", "070000_300").as_deref(),
        Some("body")
    );
    assert_eq!(records(root, "body", "20261004").len(), 1);

    // An import nobody answered for moves nothing already listed.
    let (run, _) = import(
        root,
        &csv,
        "20261010_145500",
        ImportFacet::Unattended,
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(
        tile_facet(root, "20261004", "070000_300").as_deref(),
        Some("body")
    );
    assert_eq!(records(root, "body", "20261004").len(), 1);

    // The owner answers with another facet: every workout in the download
    // moves there, tiles and record together, so a listing and its workout
    // never split.
    let (run, _) = import(
        root,
        &csv,
        "20261010_150000",
        ImportFacet::Facet("work".into()),
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(count(&run, "filed"), 3);
    assert_eq!(
        tile_facet(root, "20261004", "070000_300").as_deref(),
        Some("work")
    );
    assert_eq!(records(root, "work", "20261004").len(), 1);
    assert!(records(root, "body", "20261004").is_empty());
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

#[test]
fn a_run_stopped_before_it_completes_leaves_no_workout_unlisted_and_no_tile_unpublished() {
    let journal = journal(&["body"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "");
    let (run, _) = import(root, &csv, "20261010_100000", ImportFacet::None, &model);
    assert_eq!(run.exit_code, 0, "{}", run.stderr);

    // With body, stopped after its tiles took body and before it completed:
    // its workouts are already listed.
    let (run, _) = import_stopping(
        root,
        &csv,
        "20261010_110000",
        ImportFacet::Facet("body".into()),
        &model,
        true,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(
        tile_facet(root, "20261004", "070000_300").as_deref(),
        Some("body")
    );
    for day in ["20261004", "20261005", "20261006"] {
        assert_eq!(records(root, "body", day).len(), 1, "{day}");
    }

    // The next import publishes the tiles that took body, though nothing
    // about them changes now; the one after publishes nothing.
    let (run, id) = import(
        root,
        &csv,
        "20261010_120000",
        ImportFacet::Facet("body".into()),
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(count(&run, "filed"), 0);
    assert_eq!(
        published(root, &id),
        [
            "20261004/070000_300",
            "20261004/070500_300",
            "20261005/090000_300",
            "20261006/060000_300"
        ]
    );
    let (run, id) = import(
        root,
        &csv,
        "20261010_130000",
        ImportFacet::Facet("body".into()),
        &model,
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert!(published(root, &id).is_empty());
}

#[test]
fn a_workout_an_earlier_reading_filed_but_never_listed_is_listed_in_that_facet_with_no_facet_chosen()
 {
    let journal = journal(&["work"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "easy with Sam");
    let (run, _) = import(root, &csv, "20261010_100000", ImportFacet::None, &model);
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert!(records(root, "work", "20261004").is_empty());

    // An earlier reading filed the first workout's tiles under work, and
    // listed nothing: its activity was left open.
    for key in ["070000_300", "070500_300"] {
        let dir = tile(root, "20261004", key);
        fs::create_dir_all(dir.join("talents")).unwrap();
        fs::write(dir.join("talents/facets.json"), r#"[{"facet":"work"}]"#).unwrap();
    }

    let (run, _) = import(root, &csv, "20261010_110000", ImportFacet::None, &model);

    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(records(root, "work", "20261004").len(), 1);
    assert_eq!(
        tile_facet(root, "20261004", "070000_300").as_deref(),
        Some("work")
    );
    // Its words are read under that facet.
    assert_eq!((count(&run, "names_calls"), model.calls.get()), (1, 1));
}

#[test]
fn a_listed_tile_deleted_since_is_not_published_again() {
    let journal = journal(&["body"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "");
    let (run, _) = import(root, &csv, "20261010_100000", ImportFacet::None, &model);
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    // An earlier run listed two tiles and stopped; one was deleted since.
    let list = root.join(solstone_core_import::strava_refile::STRAVA_REFILE_FILE);
    fs::write(
        &list,
        json!([["20261004", "070500_300"], ["20261005", "090000_300"]]).to_string(),
    )
    .unwrap();
    let deleted = tile(root, "20261005", "090000_300");
    fs::remove_file(deleted.join("workout.json")).unwrap();
    fs::write(deleted.join(crate::body_activity::TOMBSTONE), "{}").unwrap();

    let (run, id) = import(root, &csv, "20261010_110000", ImportFacet::None, &model);

    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(published(root, &id), ["20261004/070500_300"]);
    assert!(!list.exists());
}

#[test]
fn a_refile_list_that_cant_be_parsed_stops_no_import_and_reaches_no_owner_words() {
    let journal = journal(&["body"]);
    let root = journal.path();
    let model = Model {
        lock: root.join("imports/.strava"),
        calls: Cell::new(0),
    };
    let csv = download(root, "");
    fs::create_dir_all(root.join("imports")).unwrap();
    let garbage = b"{not a list";
    fs::write(
        root.join(solstone_core_import::strava_refile::STRAVA_REFILE_FILE),
        garbage,
    )
    .unwrap();
    let parse_error = serde_json::from_slice::<Value>(garbage)
        .unwrap_err()
        .to_string();

    let (run, id) = import(
        root,
        &csv,
        "20261010_100000",
        ImportFacet::Facet("body".into()),
        &model,
    );

    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(count(&run, "filed"), 3);
    let summary = fs::read_to_string(root.join("imports").join(&id).join("summary.md")).unwrap();
    for shown in [&run.stdout, &run.stderr, &summary] {
        assert!(!shown.contains(&parse_error), "{shown}");
        assert!(!shown.contains("line 1 column"), "{shown}");
    }
    let aside = fs::read_dir(root.join("imports"))
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".strava-refile.wedged-")
        })
        .map(|entry| fs::read(entry.path()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(aside, [garbage.to_vec()]);
}
