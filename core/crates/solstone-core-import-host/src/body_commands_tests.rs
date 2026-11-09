// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;

use super::*;

fn command<'a>() -> BodyCommand<'a> {
    BodyCommand {
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
    assert_eq!(run.stderr, APPLE_CONFIRM_WORDS);
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

#[test]
fn a_preview_names_what_would_be_read_and_keeps_the_source() {
    let run = preview_apple(true);
    assert_eq!(run.exit_code, 0);
    assert!(
        run.stdout.contains("\"source\":\"apple_health\""),
        "{}",
        run.stdout
    );
    assert!(preview_apple(false).stdout.contains("workouts"));
}

#[test]
fn a_preview_carries_the_words_the_page_shows() {
    let run = preview_apple(true);
    let preview: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(preview["page_refusal"], APPLE_CONFIRM_WORDS.trim_end());
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
