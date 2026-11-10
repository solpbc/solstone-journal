// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;

use super::*;

fn command<'a>() -> BodyCommand<'a> {
    BodyCommand {
        import_id: None,
        facet: ImportFacet::Unattended,
        listed_moves_shown: None,
        scheduled: true,
        json: false,
        confirm: true,
        date_from: None,
        date_to: None,
        window_days: None,
        force: false,
        save: false,
    }
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 9).unwrap()
}

#[test]
fn apple_without_the_go_ahead_says_so_before_anything_else() {
    let journal = tempfile::tempdir().unwrap();
    let run = save_apple(
        "nowhere",
        &BodyCommand {
            confirm: false,
            force: true,
            ..command()
        },
        journal.path(),
    );
    assert_eq!(run.exit_code, 1);
    assert_eq!(run.stderr, apple_confirm_words());
    assert!(!journal.path().join("imports").exists());
}

#[test]
fn apple_refuses_dates_and_force_before_any_write() {
    let journal = tempfile::tempdir().unwrap();
    for refused in [
        BodyCommand {
            date_from: Some("2026-01-01"),
            ..command()
        },
        BodyCommand {
            date_to: Some("2026-01-01"),
            ..command()
        },
        BodyCommand {
            force: true,
            ..command()
        },
    ] {
        let run = save_apple("nowhere", &refused, journal.path());
        assert_eq!(run.stderr, APPLE_WHOLE_WORDS);
    }
    assert!(!journal.path().join("imports").exists());
}

const SRC: &str = r#"sourceName="Watch" sourceVersion="11""#;

fn export_with(records: &[String]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("export.xml"),
        format!("<HealthData>{}</HealthData>", records.join("")),
    )
    .unwrap();
    directory
}

fn stage(start: &str, end: &str) -> String {
    format!(
        r#"<Record type="HKCategoryTypeIdentifierSleepAnalysis" {SRC} startDate="{start}" endDate="{end}" value="HKCategoryValueSleepAnalysisAsleepCore"/>"#
    )
}

fn small_export() -> tempfile::TempDir {
    export_with(&[
        format!(
            r#"<Workout workoutActivityType="HKWorkoutActivityTypeRunning" {SRC} startDate="2026-10-10 06:31:00 -0600" endDate="2026-10-10 07:20:00 -0600"/>"#
        ),
        format!(
            r#"<Record type="HKQuantityTypeIdentifierHeartRate" {SRC} unit="count/min" startDate="2026-10-09 06:40:00 -0600" endDate="2026-10-09 06:40:00 -0600" value="70"/>"#
        ),
        format!(
            r#"<Record type="HKQuantityTypeIdentifierStepCount" {SRC} unit="count" startDate="2026-10-11 06:00:00 -0600" endDate="2026-10-11 07:00:00 -0600" value="900"/>"#
        ),
        format!(
            r#"<Record type="HKQuantityTypeIdentifierActiveEnergyBurned" {SRC} unit="kcal" startDate="2026-10-11 06:00:00 -0600" endDate="2026-10-11 07:00:00 -0600" value="9"/>"#
        ),
        // Two stages 20 minutes apart are one night; one more two hours on is another.
        stage("2026-10-09 23:00:00 -0600", "2026-10-10 01:00:00 -0600"),
        stage("2026-10-10 01:20:00 -0600", "2026-10-10 03:00:00 -0600"),
        stage("2026-10-10 05:00:00 -0600", "2026-10-10 06:00:00 -0600"),
    ])
}

#[test]
fn a_preview_counts_what_an_export_holds_and_writes_nothing() {
    let journal = tempfile::tempdir().unwrap();
    let export = small_export();
    let media = export.path().to_str().unwrap();
    let run = preview_apple(media, journal.path(), &ImportFacet::Unattended, true);
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let size: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(size["schema"], "solstone.import.size.v1");
    assert_eq!(size["source"], "apple_health");
    let held = &size["apple_health"];
    assert_eq!(
        held["span"],
        serde_json::json!(["2026-10-09", "2026-10-11"])
    );
    assert_eq!(held["workouts"], 1);
    assert_eq!(held["nights"], 2);
    assert_eq!(held["readings"], 1);
    assert_eq!(held["daily_totals"], 1);
    assert_eq!(
        held["not_read"]["HKQuantityTypeIdentifierActiveEnergyBurned"],
        1
    );
    assert_eq!(
        held["export_bytes"],
        std::fs::metadata(export.path().join("export.xml"))
            .unwrap()
            .len()
    );
    // The page refuses an empty preview, so what's read is its unit.
    assert_eq!(
        size["other_units"],
        serde_json::json!(["Apple Health records", 5])
    );
    assert_eq!(size["go_ahead"], apple_confirm_words().trim_end());
    assert!(size.get("page_refusal").is_none(), "{size}");
    assert_eq!(std::fs::read_dir(journal.path()).unwrap().count(), 0);
    assert!(
        preview_apple(media, journal.path(), &ImportFacet::Unattended, false)
            .stdout
            .contains("1 workout, 2 nights put together from sleep stages")
    );
}

#[test]
fn a_preview_of_a_cut_export_answers_in_words_the_page_shows() {
    let journal = tempfile::tempdir().unwrap();
    let export = tempfile::tempdir().unwrap();
    std::fs::write(
        export.path().join("export.xml"),
        format!(
            r#"<HealthData><Record type="HKQuantityTypeIdentifierHeartRate" {SRC} unit="count/min" startDate="2026-10-09 06:40:00 -0600" endDate="2026-10-09 06:40:00 -0600" value="70"/>"#
        ),
    )
    .unwrap();
    let media = export.path().to_str().unwrap();
    let run = preview_apple(media, journal.path(), &ImportFacet::Unattended, true);
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let size: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(size["refusal"], "export_incomplete");
    assert_eq!(
        size["page_refusal"],
        solstone_core_import_sources::body_status::words("export_incomplete")
    );
    let words = preview_apple(media, journal.path(), &ImportFacet::Unattended, false);
    assert_eq!(words.exit_code, 1);
    assert!(!words.stderr.contains("body-ingest"), "{}", words.stderr);
}

#[test]
fn a_preview_with_a_facet_says_how_many_a_yes_could_move_at_most() {
    let journal = tempfile::tempdir().unwrap();
    let export = small_export();
    let media = export.path().to_str().unwrap();
    let none = preview_apple(
        media,
        journal.path(),
        &ImportFacet::Facet("body".to_owned()),
        true,
    );
    let size: serde_json::Value = serde_json::from_str(&none.stdout).unwrap();
    assert!(size.get("listed_moves").is_none(), "{size}");
}

#[test]
fn oura_flags_are_refused_each_in_its_own_words() {
    let words = |built: BodyCommand<'_>| oura_range(&built, today()).unwrap_err();
    assert!(
        words(BodyCommand {
            date_to: Some("2026-01-01"),
            ..command()
        })
        .contains("--date-from")
    );
    assert!(
        words(BodyCommand {
            window_days: Some(0),
            ..command()
        })
        .contains("at least one")
    );
    assert!(
        words(BodyCommand {
            window_days: Some(3),
            date_from: Some("2026-01-01"),
            ..command()
        })
        .contains("not both")
    );
    assert!(
        words(BodyCommand {
            date_from: Some("2026-12-01"),
            ..command()
        })
        .contains("past today")
    );
    assert!(
        words(BodyCommand {
            date_from: Some("2026-01-01"),
            date_to: Some("2026-12-01"),
            ..command()
        })
        .contains("past today")
    );
    assert!(
        words(BodyCommand {
            date_from: Some("soon"),
            ..command()
        })
        .contains("isn't one")
    );
}

#[test]
fn oura_ranges_mean_what_they_say() {
    let range = |built: BodyCommand<'_>| oura_range(&built, today()).unwrap();
    assert_eq!(range(command()), None);
    assert_eq!(
        range(BodyCommand {
            window_days: Some(2),
            ..command()
        }),
        Some((NaiveDate::from_ymd_opt(2026, 11, 8).unwrap(), today()))
    );
    assert_eq!(
        range(BodyCommand {
            window_days: Some(100_000),
            ..command()
        }),
        Some((NaiveDate::from_ymd_opt(2015, 1, 1).unwrap(), today()))
    );
    assert_eq!(
        range(BodyCommand {
            date_from: Some("2026-01-05"),
            ..command()
        }),
        Some((NaiveDate::from_ymd_opt(2026, 1, 5).unwrap(), today()))
    );
}

#[test]
fn an_oura_plan_refuses_a_range_the_wrong_way_round() {
    let journal = tempfile::tempdir().unwrap();
    let run = sync_oura(
        &BodyCommand {
            date_from: Some("2026-03-01"),
            date_to: Some("2026-02-01"),
            ..command()
        },
        journal.path(),
    );
    assert_eq!(run.exit_code, 1, "{}", run.stdout);
    assert!(
        run.stderr.contains("put the earlier date first"),
        "{}",
        run.stderr
    );
}

#[test]
fn the_go_ahead_refusal_says_where_to_type_it_for_this_platform() {
    let words = apple_confirm_words();
    assert_eq!(
        words.contains("open admin terminal"),
        cfg!(target_os = "macos"),
        "{words}"
    );
}

#[test]
fn an_oura_plan_says_what_it_would_ask_and_writes_nothing() {
    let journal = tempfile::tempdir().unwrap();
    let run = sync_oura(&command(), journal.path());
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert!(
        run.stdout.contains("sleep: 2015-01-01 to "),
        "{}",
        run.stdout
    );
    assert!(!journal.path().join("imports").exists());
    assert!(!journal.path().join("chronicle").exists());
}

#[test]
fn a_facet_that_isnt_one_of_yours_stops_a_body_import() {
    let journal = tempfile::tempdir().unwrap();
    let run = save_apple(
        "nowhere",
        &BodyCommand {
            facet: ImportFacet::Facet("absent".to_owned()),
            ..command()
        },
        journal.path(),
    );
    assert_eq!(run.exit_code, 1);
    assert!(
        run.stderr.contains("isn't one of") || run.stderr.contains("not one of"),
        "{}",
        run.stderr
    );
    assert!(!journal.path().join("imports").exists());
}

// A page start: the import page's record, its upload inside it, and the arguments
// the page's `command()` gives an Apple Health card start of an `export.zip`
// upload (`upload_fallback`, so no positional time).
mod page_start {
    use super::*;
    use serde_json::json;
    use std::fs;

    const P: &str = "20261012_140000";

    fn journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("config")).unwrap();
        fs::write(
            journal.path().join("config/journal.json"),
            json!({"identity": {"timezone": "America/Denver"}}).to_string(),
        )
        .unwrap();
        journal
    }

    /// The page's record for an upload, with the export (a folder, here) where
    /// its preview snapshot would be.
    fn page_record(root: &Path, id: &str, records: &[String]) -> std::path::PathBuf {
        let snapshot = root
            .join("imports")
            .join(id)
            .join("preview-x")
            .join("export");
        fs::create_dir_all(&snapshot).unwrap();
        fs::write(
            snapshot.join("export.xml"),
            format!("<HealthData>{}</HealthData>", records.join("")),
        )
        .unwrap();
        let mut metadata = solstone_core_import::ImportMetadata::new();
        metadata.insert("file_path".into(), json!(snapshot.to_str().unwrap()));
        metadata.insert("source_hint".into(), json!("apple_health"));
        solstone_core_import::write_import_metadata(root, id, &metadata).unwrap();
        snapshot
    }

    #[test]
    fn a_page_run_that_ends_any_way_leaves_no_upload_unless_a_live_run_holds_it() {
        let journal = journal();
        let root = journal.path();
        let stage = |id: &str| {
            let dir = root.join("imports").join(id);
            fs::create_dir_all(&dir).unwrap();
            let upload = dir.join("export.zip");
            fs::write(&upload, b"zip").unwrap();
            let mut metadata = solstone_core_import::ImportMetadata::new();
            metadata.insert("file_path".into(), json!(upload.to_str().unwrap()));
            metadata.insert("source_hint".into(), json!("apple_health"));
            solstone_core_import::write_import_metadata(root, id, &metadata).unwrap();
            upload
        };
        // Superseded: the attempt this run holds ends with no outcome of its own.
        let superseded = stage(P);
        solstone_core_import::record_running_attempt(root, P, 1_000).unwrap();
        crate::body_commands::settle_page_import(root, P, Some("this import was superseded"));
        assert!(!superseded.exists(), "a superseded run's upload goes");
        assert_ne!(
            solstone_core_import::metadata::attempt_holder(root, P),
            solstone_core_import::metadata::AttemptHolder::Held,
            "and so does its hold on the import"
        );
        // Another process holds the import: its run is still going.
        let other = "20261012_150000";
        let held = stage(other);
        let lock = solstone_core_journal_io::hold_lock(
            root.join("imports").join(other).join(".attempt"),
            solstone_core_journal_io::LockOptions::default(),
        )
        .unwrap();
        crate::body_commands::settle_page_import(root, other, Some("import stopped"));
        assert!(held.exists(), "a live run's upload stays");
        drop(lock);
        crate::body_commands::settle_page_import(root, other, Some("import stopped"));
        assert!(!held.exists(), "and goes once nothing holds it");
    }

    fn heart(at: &str) -> String {
        format!(
            r#"<Record type="HKQuantityTypeIdentifierHeartRate" {SRC} unit="count/min" startDate="{at}" endDate="{at}" value="70"/>"#
        )
    }

    fn page_args(media: &Path, id: &str, go_ahead: bool) -> Vec<String> {
        let mut args: Vec<String> = [
            media.to_str().unwrap(),
            "--fallback-timestamp",
            id,
            "--preview-stamp",
            id,
            "--source",
            "apple_health",
            "--no-facet",
            "--import-id",
            id,
        ]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect();
        if go_ahead {
            args.push("--confirm-body-save".to_owned());
        }
        args
    }

    fn run_page(root: &Path, args: &[String]) -> CliRun {
        let lookup_env = |var: &str| (var == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned());
        match crate::cli_argv::run_cli_with(args, root, lookup_env, || false) {
            crate::cli_argv::CliOutcome::Rendered(run)
            | crate::cli_argv::CliOutcome::Imported { run, .. } => run,
            crate::cli_argv::CliOutcome::Registry(_) => {
                panic!("an Apple export isn't a registry import")
            }
        }
    }

    fn imports(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root.join("imports"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort();
        names
    }

    fn attempt(root: &Path, id: &str) -> solstone_core_import::metadata::AttemptRead {
        let metadata = solstone_core_import::metadata::read_provenance(root, id)
            .unwrap()
            .unwrap();
        solstone_core_import::metadata::read_attempt_facts(&metadata)
    }

    #[test]
    fn a_page_start_runs_under_the_page_import_and_makes_no_other() {
        let journal = journal();
        let root = journal.path();
        let media = page_record(root, P, &[heart("2026-10-09 06:40:00 -0600")]);
        let run = run_page(root, &page_args(&media, P, true));
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert_eq!(imports(root), vec![P.to_owned()]);
        let dir = root.join("imports").join(P);
        for name in [
            "manifest.json",
            "summary.md",
            "body-touched.jsonl",
            "body-progress.json",
        ] {
            assert!(dir.join(name).is_file(), "{name}");
        }
        let solstone_core_import::metadata::AttemptRead::Present(facts) = attempt(root, P) else {
            panic!("no attempt");
        };
        assert_eq!(
            facts.state,
            solstone_core_import::metadata::AttemptState::Completed
        );
        assert!(!media.exists(), "the uploaded export isn't kept");
        assert!(solstone_core_import_sources::body_status::read(root, P).is_none());
    }

    #[test]
    fn a_page_start_for_an_import_that_ran_or_doesnt_hold_the_export_changes_nothing() {
        let journal = journal();
        let root = journal.path();
        // Missing: nothing is made.
        let elsewhere = tempfile::tempdir().unwrap();
        fs::write(
            elsewhere.path().join("export.xml"),
            "<HealthData></HealthData>",
        )
        .unwrap();
        let run = run_page(root, &page_args(elsewhere.path(), P, true));
        assert_eq!(run.exit_code, 1);
        assert!(!root.join("imports").join(P).exists());
        // Already ran: byte for byte the same after.
        let media = page_record(root, P, &[heart("2026-10-09 06:40:00 -0600")]);
        assert_eq!(run_page(root, &page_args(&media, P, true)).exit_code, 0);
        let before =
            solstone_core_import::snapshot::fingerprint(&root.join("imports").join(P)).unwrap();
        fs::create_dir_all(&media).unwrap();
        fs::write(media.join("export.xml"), "<HealthData></HealthData>").unwrap();
        let again = run_page(root, &page_args(&media, P, true));
        assert_eq!(again.exit_code, 1);
        fs::remove_dir_all(media.parent().unwrap()).unwrap();
        assert_eq!(
            solstone_core_import::snapshot::fingerprint(&root.join("imports").join(P)).unwrap(),
            before
        );
        // Holding another export, as a run or as a dry run, or anything but an
        // Apple save: refused, and the named import is exactly as it was, its
        // upload included.
        let other = "20261012_150000";
        let held = page_record(root, other, &[heart("2026-10-09 06:40:00 -0600")]);
        let untouched =
            solstone_core_import::snapshot::fingerprint(&root.join("imports").join(other)).unwrap();
        let mut dry = page_args(elsewhere.path(), other, true);
        dry.push("--dry-run".to_owned());
        let mut not_apple = page_args(&held, other, true);
        let at = not_apple
            .iter()
            .position(|arg| arg == "apple_health")
            .unwrap();
        not_apple[at] = "ics".to_owned();
        for args in [page_args(elsewhere.path(), other, true), dry, not_apple] {
            let run = run_page(root, &args);
            assert_ne!(run.exit_code, 0, "{args:?}");
            assert_eq!(
                solstone_core_import::snapshot::fingerprint(&root.join("imports").join(other))
                    .unwrap(),
                untouched,
                "{args:?}"
            );
        }
        assert!(held.exists(), "its upload stays");
        // The desktop command's own preview of the card's save: a dry run of the
        // same arguments, which goes through, writes nothing, and leaves the
        // upload for the save that follows it.
        let mut preview = page_args(&held, other, true);
        preview.push("--dry-run".to_owned());
        let run = run_page(root, &preview);
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert_eq!(
            solstone_core_import::snapshot::fingerprint(&root.join("imports").join(other)).unwrap(),
            untouched,
            "the preview writes nothing"
        );
        let run = run_page(root, &page_args(&held, other, true));
        assert_eq!(run.exit_code, 0, "the save after it: {}", run.stderr);
    }

    #[test]
    fn a_page_start_refused_before_it_runs_says_why_and_lets_the_upload_go() {
        let journal = journal();
        let root = journal.path();
        let media = page_record(root, P, &[heart("2026-10-09 06:40:00 -0600")]);
        let run = run_page(root, &page_args(&media, P, false));
        assert_eq!(run.exit_code, 1);
        assert_eq!(
            solstone_core_import_sources::body_status::read(root, P),
            Some(("go_ahead".to_owned(), 1))
        );
        assert!(matches!(
            attempt(root, P),
            solstone_core_import::metadata::AttemptRead::Absent
        ));
        assert!(!media.exists());
    }

    #[test]
    fn a_page_start_that_stops_on_its_export_records_why_on_its_own_attempt() {
        let journal = journal();
        let root = journal.path();
        let media = page_record(root, P, &[heart("2026-10-09 06:40:00 -0600")]);
        // Cut part way through.
        fs::write(
            media.join("export.xml"),
            format!("<HealthData>{}", heart("2026-10-09 06:40:00 -0600")),
        )
        .unwrap();
        let run = run_page(root, &page_args(&media, P, true));
        assert_eq!(run.exit_code, 1, "{}", run.stdout);
        let solstone_core_import::metadata::AttemptRead::Present(facts) = attempt(root, P) else {
            panic!("the run was admitted");
        };
        assert_eq!(
            solstone_core_import_sources::body_status::read(root, P),
            Some(("export_incomplete".to_owned(), facts.generation))
        );
    }

    #[test]
    fn a_page_start_takes_over_a_stopped_command_line_run_and_its_rows_keep_their_import() {
        let journal = journal();
        let root = journal.path();
        let records = [
            heart("2026-10-09 06:40:00 -0600"),
            heart("2026-10-11 06:40:00 -0600"),
        ];
        let scratch = tempfile::tempdir().unwrap();
        let export = scratch.path().join("export");
        fs::create_dir_all(&export).unwrap();
        fs::write(
            export.join("export.xml"),
            format!("<HealthData>{}</HealthData>", records.join("")),
        )
        .unwrap();
        // The command line's run, stopped after its first day.
        let stop = |point: &str| point == "published";
        let mut stopped =
            crate::body_segments::AppleSegmentsRequest::new(&export, "20261012_120000", None);
        stopped.stop_at = Some(&stop);
        assert!(crate::body_segments::run_apple_health_segments(root, &stopped).is_err());
        // The same export from the page: never a resume, so it takes the stopped run over.
        let media = page_record(root, P, &records);
        let run = run_page(root, &page_args(&media, P, true));
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert_eq!(
            imports(root),
            vec!["20261012_120000".to_owned(), P.to_owned()]
        );
        let first_day = fs::read_dir(root.join("chronicle/20261009/import.apple_health"))
            .unwrap()
            .flatten()
            .find(|entry| entry.path().join("body.jsonl").is_file())
            .unwrap();
        let row: serde_json::Value = serde_json::from_str(
            fs::read_to_string(first_day.path().join("body.jsonl"))
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            row["run"], "20261012_120000",
            "a row keeps the import that first wrote it"
        );
        let manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(root.join("imports").join(P).join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest["days_affected"],
            json!(["20261011"]),
            "the page's import claims only the day it wrote, not the stopped run's: {manifest}"
        );
        // Each run took its place in this journal's order as it was admitted.
        let order = solstone_core_import_sources::body_runs::Order::read(root);
        assert_eq!(order.of(root, "20261012_120000"), Some(1));
        assert_eq!(order.of(root, P), Some(2), "the page's run came after it");
    }
}

#[test]
fn a_large_count_reads_with_its_thousands_marked() {
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1_000), "1,000");
    assert_eq!(grouped(2_000_000), "2,000,000");
}
