// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::Deserialize;
use std::fs;
use std::process::Command;

#[derive(Deserialize)]
struct Corpus {
    cases: std::collections::BTreeMap<String, Case>,
}

#[derive(Deserialize)]
struct Case {
    exit: i32,
    stdout: String,
    stderr: String,
}

fn corpus() -> Corpus {
    serde_json::from_str(include_str!(
        "../../../fixtures/schedule_reference_output.json"
    ))
    .expect("frozen corpus parses")
}

fn journal() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp journal");
    fs::create_dir_all(directory.path().join("config")).expect("config");
    fs::create_dir_all(directory.path().join("health")).expect("health");
    fs::write(
        directory.path().join("config/journal.json"),
        br#"{"setup":{"completed_at":1}}"#,
    )
    .expect("journal config");
    directory
}

fn run(journal: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(args)
        .env("SOLSTONE_JOURNAL", journal)
        .output()
        .expect("native schedule runs")
}

fn row<'a>(stdout: &'a str, name: &str) -> &'a str {
    stdout
        .lines()
        .find(|line| line.trim_start().starts_with(name))
        .expect("schedule row")
}

fn cell<'a>(header: &str, row: &'a str, heading: &str, width: usize) -> &'a str {
    let start = header.find(heading).expect("heading");
    row.get(start..start + width)
        .expect("fixed-width cell")
        .trim()
}

fn is_hour_minute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[2] == b':'
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
}

fn is_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 16
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b' '
        && bytes[13] == b':'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13) || byte.is_ascii_digit())
}

fn write_detailed_schedule(journal: &std::path::Path) {
    let now = chrono::Local::now().timestamp();
    fs::write(
        journal.join("config/schedules.json"),
        r#"{"daily_time":"03:00","weekly_day":"monday","weekly_time":"08:15","daily-with-configured-time":{"cmd":["journal","heartbeat"],"every":"daily"},"sub-floor-minute":{"cmd":"journal think --cadence","every":"2m"},"unsupported-interval":{"cmd":"journal check","every":"fortnightly"},"very-long-schedule-name:hourly":{"cmd":["journal","think"],"every":"hourly"},"weekly-status":{"cmd":["journal","maintenance"],"every":"weekly"},"z:disabled":{"cmd":["journal","noop"],"every":"hourly","enabled":false}}"#,
    )
    .expect("schedules");
    fs::write(
        journal.join("health/scheduler.json"),
        format!(
            r#"{{"daily-with-configured-time":{{"last_run":{now}}},"sub-floor-minute":{{"last_run":{now}}},"very-long-schedule-name:hourly":{{"last_run":{now}}},"weekly-status":{{"last_run":{now}}}}}"#
        ),
    )
    .expect("state");
}

#[test]
fn ac5_schedule_bad_flag_is_verb_owned() {
    let output = run(journal().path(), &["schedule", "--nonsense"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "usage: journal schedule [-h] [-v] [-d]\njournal schedule: error: unrecognized arguments: --nonsense\n"
    );
}

#[test]
fn valid_fixture_matches_frozen_distinctive_row_and_footer() {
    let journal = journal();
    fs::write(
        journal.path().join("config/schedules.json"),
        r#"{"daily_time":"03:00","a:daily":{"cmd":["journal","heartbeat"],"every":"daily"},"minute":{"cmd":"journal think --cadence","every":"1m"},"z:disabled":{"cmd":["journal","noop"],"every":"hourly","enabled":false}}"#,
    )
    .expect("schedules");
    let output = run(journal.path(), &["schedule"]);
    let expected = &corpus().cases["simple_table"];
    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace(&journal.path().display().to_string(), "{journal}");
    assert_eq!(output.status.code(), Some(expected.exit));
    assert_eq!(stdout, expected.stdout);
    assert_eq!(String::from_utf8_lossy(&output.stderr), expected.stderr);
}

#[test]
fn captured_stateful_cases_match_native_columns_and_runtime_forms() {
    let corpus = corpus();
    let expected_table = &corpus.cases["table"];
    let expected_midnight = &corpus.cases["midnight"];
    for expected in [expected_table, expected_midnight] {
        assert_eq!(expected.exit, 0);
        assert!(expected.stderr.is_empty());
        assert!(expected.stdout.contains("2026-03-"));
    }
    assert!(
        expected_table
            .stdout
            .contains("very-long-schedule-name:hourly")
    );
    assert!(
        expected_table
            .stdout
            .contains("sub-floor-minute                5m")
    );
    assert!(
        expected_table
            .stdout
            .contains("unsupported-interval            fortnightly")
    );
    assert!(expected_table.stdout.contains("Monday 08:15"));
    assert!(expected_midnight.stdout.contains("midnight"));

    let journal = journal();
    write_detailed_schedule(journal.path());
    let output = run(journal.path(), &["schedule"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(expected_table.exit));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        expected_table.stderr
    );
    let header = stdout.lines().next().expect("header");
    assert_eq!(
        header,
        expected_table
            .stdout
            .lines()
            .next()
            .expect("captured header")
    );
    assert_eq!(
        cell(
            header,
            row(&stdout, "daily-with-configured-time"),
            "EVERY",
            8
        ),
        "daily"
    );
    assert_eq!(
        cell(
            header,
            row(&stdout, "daily-with-configured-time"),
            "NEXT DUE",
            10
        ),
        "03:00"
    );
    assert_eq!(
        cell(header, row(&stdout, "sub-floor-minute"), "EVERY", 8),
        "5m"
    );
    assert!(is_hour_minute(cell(
        header,
        row(&stdout, "sub-floor-minute"),
        "NEXT DUE",
        10
    )));
    assert_eq!(
        cell(header, row(&stdout, "unsupported-interval"), "NEXT DUE", 10),
        "?"
    );
    assert!(is_timestamp(cell(
        header,
        row(&stdout, "very-long-schedule-name:hourly"),
        "LAST RUN",
        18
    )));
    assert!(is_hour_minute(cell(
        header,
        row(&stdout, "very-long-schedule-name:hourly"),
        "NEXT DUE",
        10
    )));
    assert!(
        row(&stdout, "weekly-status").contains("Monday 08:15  journal maintenance"),
        "weekly NEXT DUE exceeds its nominal width without truncation"
    );
    assert_eq!(
        cell(header, row(&stdout, "z:disabled"), "NEXT DUE", 10),
        "disabled"
    );

    fs::write(
        journal.path().join("config/schedules.json"),
        r#"{"daily-at-midnight":{"cmd":["journal","heartbeat"],"every":"daily"}}"#,
    )
    .expect("midnight config");
    fs::write(
        journal.path().join("health/scheduler.json"),
        format!(
            r#"{{"daily-at-midnight":{{"last_run":{}}}}}"#,
            chrono::Local::now().timestamp()
        ),
    )
    .expect("midnight state");
    let output = run(journal.path(), &["schedule"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let header = stdout.lines().next().expect("midnight header");
    assert_eq!(output.status.code(), Some(expected_midnight.exit));
    assert_eq!(
        header,
        expected_midnight
            .stdout
            .lines()
            .next()
            .expect("captured midnight header")
    );
    assert_eq!(
        cell(header, row(&stdout, "daily-at-midnight"), "NEXT DUE", 10),
        "midnight"
    );
}

#[test]
fn schedule_does_not_change_config_or_state_files() {
    let journal = journal();
    let config = journal.path().join("config/schedules.json");
    let state = journal.path().join("health/scheduler.json");
    fs::write(
        &config,
        r#"{"x":{"cmd":["journal","heartbeat"],"every":"daily"}}"#,
    )
    .expect("config");
    fs::write(&state, r#"{"x":{"last_run":0}}"#).expect("state");
    let before = (
        fs::read(&config).expect("config before"),
        fs::read(&state).expect("state before"),
    );
    assert!(run(journal.path(), &["schedule"]).status.success());
    assert_eq!(fs::read(&config).expect("config after"), before.0);
    assert_eq!(fs::read(&state).expect("state after"), before.1);
}
