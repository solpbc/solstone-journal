// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Tell the owner when backup has kept failing, and when it works again.
//!
//! The notice rests only on attempts that were made and failed at the setup
//! the owner has selected now. Age, a missing attempt or a missed schedule
//! never raises it. A pause, backup turned off, or a setup that was replaced
//! ends it without a word.

use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_backup::{
    FAILED_ATTEMPTS, FAILING_SINCE, backup_paused_at, backup_setup_fingerprint, get_backup_config,
    load_hosted_binding,
};
use solstone_core_callosum::CallosumOneShotSender;

/// Failed attempts in a row, at one setup, before backup counts as failing.
/// Backup runs hourly while the journal is up, so a running computer reaches
/// this within hours; it keeps two attempts at either end of a long stretch
/// with the computer off from counting on their own.
const FAILING_ATTEMPTS: u64 = 3;

/// How long that run of failed attempts must span, from the first to the
/// latest. Two days rides out a network outage, a trip or a weekend offline.
const FAILING_SECONDS: i64 = 2 * 24 * 60 * 60;

const TITLE_FAILING: &str = "backup has been failing";
const TITLE_WORKING: &str = "backup is working again";
const ACTION: &str = "/app/backup/";
const SEND_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
enum Notice {
    Raise {
        setup: String,
        message: &'static str,
    },
    Clear {
        setup: String,
        worded: Option<&'static str>,
    },
}

/// Re-announce what the recorded backup state shows now. Best effort: a
/// journal without a running bus, or one that can't be read, sends nothing.
pub fn observe(journal: &Path) {
    let Ok(config) = get_backup_config(journal) else {
        return;
    };
    let current = current_setup(journal, &config);
    for notice in assess(&config, current.as_deref()) {
        send(journal, &notice);
    }
}

/// After an attempt is written: the current truth, and the end of any notice
/// for a setup the attempt shows was replaced. `prior` and `attempted` are the
/// setups of the previous and this attempt.
pub(crate) fn observe_attempt(journal: &Path, prior: Option<&str>, attempted: Option<&str>) {
    let Ok(config) = get_backup_config(journal) else {
        return;
    };
    let current = current_setup(journal, &config);
    let mut notices = assess(&config, current.as_deref());
    notices.extend(replaced(&config, current.as_deref(), prior, attempted));
    for notice in notices {
        send(journal, &notice);
    }
}

fn current_setup(journal: &Path, config: &Map<String, Value>) -> Option<String> {
    let binding = (config.get("mode").and_then(Value::as_str) == Some("operated"))
        .then(|| load_hosted_binding(journal))
        .flatten();
    backup_setup_fingerprint(config, binding.as_ref())
}

fn assess(config: &Map<String, Value>, current: Option<&str>) -> Vec<Notice> {
    // No setup that can be named means nothing to say.
    let Some(current) = current else {
        return Vec::new();
    };
    // A pause, or backup turned off, is the owner's choice: it ends a notice
    // quietly and never raises one.
    if config.get("enabled") != Some(&Value::Bool(true)) || backup_paused_at(config).is_some() {
        return vec![Notice::Clear {
            setup: current.to_owned(),
            worded: None,
        }];
    }
    let Some(latest) = config.get("last_backup").and_then(Value::as_object) else {
        return Vec::new();
    };
    // Only an attempt at the selected setup counts, for or against.
    if latest.get("setup_fingerprint").and_then(Value::as_str) != Some(current) {
        return Vec::new();
    }
    match latest.get("status").and_then(Value::as_str) {
        Some("ok") => vec![Notice::Clear {
            setup: current.to_owned(),
            worded: Some(working_message(latest)),
        }],
        Some("error") if persistently_failing(latest) => vec![Notice::Raise {
            setup: current.to_owned(),
            message: failing_message(latest.get("error_reason").and_then(Value::as_str)),
        }],
        _ => Vec::new(),
    }
}

/// A setup the latest attempt shows was replaced has its notice ended: with
/// words when the attempt at the new setup worked, since backup does work
/// again, and quietly otherwise.
fn replaced(
    config: &Map<String, Value>,
    current: Option<&str>,
    prior: Option<&str>,
    attempted: Option<&str>,
) -> Vec<Notice> {
    let latest = config.get("last_backup").and_then(Value::as_object);
    let worked = latest
        .filter(|latest| {
            current.is_some()
                && attempted == current
                && latest.get("status").and_then(Value::as_str) == Some("ok")
        })
        .map(working_message);
    let mut notices = Vec::new();
    for setup in [prior, attempted].into_iter().flatten() {
        if Some(setup) == current || notices.iter().any(|notice| notice_setup(notice) == setup) {
            continue;
        }
        notices.push(Notice::Clear {
            setup: setup.to_owned(),
            worded: worked,
        });
    }
    notices
}

fn notice_setup(notice: &Notice) -> &str {
    match notice {
        Notice::Raise { setup, .. } | Notice::Clear { setup, .. } => setup,
    }
}

fn persistently_failing(latest: &Map<String, Value>) -> bool {
    let count = latest.get(FAILED_ATTEMPTS).and_then(Value::as_u64);
    let since = latest.get(FAILING_SINCE).and_then(Value::as_i64);
    let time = latest.get("time").and_then(Value::as_i64);
    match (count, since, time) {
        (Some(count), Some(since), Some(time)) => {
            count >= FAILING_ATTEMPTS && time.saturating_sub(since) >= FAILING_SECONDS
        }
        _ => false,
    }
}

/// The cause from the latest failed attempt, then one action.
fn failing_message(reason: Option<&str>) -> &'static str {
    match reason {
        Some("auth_failed") => {
            "your journal's key couldn't open the backup at your backup destination. open backup to check it."
        }
        Some("repo_missing") => {
            "no backup was found at your backup destination. open backup to check it."
        }
        Some("broker_unreachable") => {
            "encrypted backup couldn't be reached. open backup for details."
        }
        Some("rclone_unavailable") => {
            "encrypted backup is missing a part it needs where your journal runs. open backup for details."
        }
        Some("restic_unavailable") => {
            "encrypted backup can't run where your journal runs. open backup for details."
        }
        Some("hosted_entitlement_inactive") => {
            "encrypted backup isn't turned on in the services portal. open backup for details."
        }
        Some("binding_invalid") => {
            "your journal's link to encrypted backup was refused. open backup for details."
        }
        Some("repository_unsupported") => {
            "the repository in your backup settings doesn't match its storage. open backup to change it."
        }
        Some("timeout") => "the latest attempt ran out of time. open backup for details.",
        Some("locked") => {
            "your backup destination is marked as in use by another backup. open backup for details."
        }
        _ => "the latest attempt stopped with an error. open backup for details.",
    }
}

fn working_message(latest: &Map<String, Value>) -> &'static str {
    if latest.get("error_reason").and_then(Value::as_str) == Some("incomplete") {
        "the latest backup attempt finished, but some of your journal couldn't be read. open backup for details."
    } else {
        "the latest backup attempt finished. open backup for details."
    }
}

fn key(setup: &str) -> String {
    // A short prefix of the setup's opaque identity is enough to name it.
    format!(
        "backup_failing:{}",
        setup.chars().take(16).collect::<String>()
    )
}

fn line(notice: &Notice) -> Value {
    match notice {
        Notice::Raise { setup, message } => json!({
            "tract": "owner_notice",
            "event": "raise",
            "key": key(setup),
            "title": TITLE_FAILING,
            "message": message,
            "action": ACTION,
        }),
        Notice::Clear {
            setup,
            worded: Some(message),
        } => json!({
            "tract": "owner_notice",
            "event": "clear",
            "key": key(setup),
            "title": TITLE_WORKING,
            "message": message,
            "action": ACTION,
        }),
        Notice::Clear {
            setup,
            worded: None,
        } => json!({
            "tract": "owner_notice",
            "event": "clear",
            "key": key(setup),
        }),
    }
}

fn send(journal: &Path, notice: &Notice) {
    let _ = CallosumOneShotSender::new(journal.join("health/callosum.sock"), SEND_TIMEOUT)
        .send_line(&format!("{}\n", line(notice)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_backup::{
        Destination, generate_and_store_keys, record_backup_result, set_backup_paused,
        set_destination, set_enabled,
    };

    const HOUR: i64 = 60 * 60;

    fn configured() -> (tempfile::TempDir, String) {
        let journal = tempfile::tempdir().unwrap();
        set_destination(
            journal.path(),
            &Destination {
                repository: "s3:fixture".into(),
                backend: "s3".into(),
                credentials: Map::new(),
            },
        )
        .unwrap();
        generate_and_store_keys(journal.path()).unwrap();
        set_enabled(journal.path(), true).unwrap();
        let setup = current(journal.path()).unwrap();
        (journal, setup)
    }

    fn current(journal: &Path) -> Option<String> {
        current_setup(journal, &get_backup_config(journal).unwrap())
    }

    fn attempt(journal: &Path, status: &str, time: i64, reason: Option<&str>, setup: Option<&str>) {
        record_backup_result(
            journal,
            status,
            json!(time),
            if status == "ok" {
                json!("copy")
            } else {
                Value::Null
            },
            reason.map_or(Value::Null, |reason| json!(reason)),
            None,
            setup,
        )
        .unwrap();
    }

    fn now(journal: &Path) -> Vec<Notice> {
        let config = get_backup_config(journal).unwrap();
        assess(&config, current_setup(journal, &config).as_deref())
    }

    fn raised(notices: &[Notice]) -> bool {
        notices
            .iter()
            .any(|notice| matches!(notice, Notice::Raise { .. }))
    }

    #[test]
    fn failures_that_persist_at_the_current_setup_raise_with_their_cause() {
        let (journal, setup) = configured();
        let start = 1_000_000;
        // Hourly failures: two days of them, and the third attempt, are needed.
        for hour in 0..48 {
            attempt(
                journal.path(),
                "error",
                start + hour * HOUR,
                Some("auth_failed"),
                Some(&setup),
            );
            assert!(!raised(&now(journal.path())), "hour {hour} is too soon");
        }
        attempt(
            journal.path(),
            "error",
            start + 48 * HOUR,
            Some("auth_failed"),
            Some(&setup),
        );
        let notices = now(journal.path());
        assert_eq!(
            notices,
            vec![Notice::Raise {
                setup: setup.clone(),
                message: failing_message(Some("auth_failed")),
            }]
        );
        let line = line(&notices[0]);
        assert_eq!(line["tract"], "owner_notice");
        assert_eq!(line["event"], "raise");
        assert_eq!(line["key"], format!("backup_failing:{}", &setup[..16]));
        assert_eq!(line["action"], "/app/backup/");
        let latest = &get_backup_config(journal.path()).unwrap()["last_backup"];
        assert_eq!(latest[FAILED_ATTEMPTS], 49);
        assert_eq!(latest[FAILING_SINCE], start);
        // While it lasts, every observation re-announces the same episode.
        attempt(
            journal.path(),
            "error",
            start + 49 * HOUR,
            Some("repo_missing"),
            Some(&setup),
        );
        assert_eq!(
            now(journal.path()),
            vec![Notice::Raise {
                setup,
                message: failing_message(Some("repo_missing")),
            }]
        );
    }

    #[test]
    fn two_attempts_far_apart_are_not_enough() {
        let (journal, setup) = configured();
        attempt(journal.path(), "error", 0, Some("timeout"), Some(&setup));
        attempt(
            journal.path(),
            "error",
            10 * 24 * HOUR,
            Some("timeout"),
            Some(&setup),
        );
        assert!(!raised(&now(journal.path())));
        attempt(
            journal.path(),
            "error",
            10 * 24 * HOUR + HOUR,
            Some("timeout"),
            Some(&setup),
        );
        assert!(raised(&now(journal.path())));
    }

    #[test]
    fn a_success_or_an_unattributed_attempt_ends_the_run_of_failures() {
        let (journal, setup) = configured();
        attempt(journal.path(), "error", 0, None, Some(&setup));
        attempt(journal.path(), "ok", HOUR, None, Some(&setup));
        attempt(journal.path(), "error", 2 * HOUR, None, Some(&setup));
        let latest = &get_backup_config(journal.path()).unwrap()["last_backup"];
        assert_eq!(latest[FAILED_ATTEMPTS], 1);
        assert_eq!(latest[FAILING_SINCE], 2 * HOUR);
        attempt(
            journal.path(),
            "error",
            3 * HOUR,
            Some("broker_error"),
            None,
        );
        let latest = &get_backup_config(journal.path()).unwrap()["last_backup"];
        assert!(latest.get(FAILED_ATTEMPTS).is_none());
        attempt(journal.path(), "error", 60 * HOUR, None, Some(&setup));
        attempt(journal.path(), "error", 61 * HOUR, None, Some(&setup));
        attempt(journal.path(), "error", 62 * HOUR, None, Some(&setup));
        assert!(
            !raised(&now(journal.path())),
            "the run restarted at hour 60"
        );
    }

    #[test]
    fn the_next_success_at_the_setup_clears_with_words() {
        let (journal, setup) = configured();
        for hour in 0..=48 {
            attempt(
                journal.path(),
                "error",
                hour * HOUR,
                Some("timeout"),
                Some(&setup),
            );
        }
        attempt(journal.path(), "ok", 49 * HOUR, None, Some(&setup));
        let notices = now(journal.path());
        assert_eq!(
            notices,
            vec![Notice::Clear {
                setup: setup.clone(),
                worded: Some(working_message(&Map::new())),
            }]
        );
        let line = line(&notices[0]);
        assert_eq!(line["event"], "clear");
        assert_eq!(line["key"], format!("backup_failing:{}", &setup[..16]));
        assert!(line["title"].is_string() && line["message"].is_string());
        // A copy that left files out still ends the failure, and says so.
        attempt(
            journal.path(),
            "ok",
            50 * HOUR,
            Some("incomplete"),
            Some(&setup),
        );
        let latest = get_backup_config(journal.path()).unwrap()["last_backup"].clone();
        assert_ne!(
            working_message(latest.as_object().unwrap()),
            working_message(&Map::new())
        );
    }

    #[test]
    fn a_pause_never_raises_ends_quietly_and_restarts_the_run() {
        let (journal, setup) = configured();
        for hour in 0..=48 {
            attempt(
                journal.path(),
                "error",
                hour * HOUR,
                Some("timeout"),
                Some(&setup),
            );
        }
        assert!(raised(&now(journal.path())));
        set_backup_paused(journal.path(), Some(49 * HOUR)).unwrap();
        let notices = now(journal.path());
        assert_eq!(
            notices,
            vec![Notice::Clear {
                setup: setup.clone(),
                worded: None,
            }]
        );
        let line = line(&notices[0]);
        assert!(line.get("title").is_none() && line.get("message").is_none());
        set_backup_paused(journal.path(), None).unwrap();
        attempt(
            journal.path(),
            "error",
            100 * HOUR,
            Some("timeout"),
            Some(&setup),
        );
        assert!(
            !raised(&now(journal.path())),
            "a resumed backup starts over"
        );
        assert_eq!(
            get_backup_config(journal.path()).unwrap()["last_backup"][FAILED_ATTEMPTS],
            1
        );
    }

    #[test]
    fn failures_at_a_replaced_setup_never_count_and_their_notice_ends() {
        let (journal, old) = configured();
        for hour in 0..=48 {
            attempt(
                journal.path(),
                "error",
                hour * HOUR,
                Some("auth_failed"),
                Some(&old),
            );
        }
        set_destination(
            journal.path(),
            &Destination {
                repository: "s3:elsewhere".into(),
                backend: "s3".into(),
                credentials: Map::new(),
            },
        )
        .unwrap();
        let new = current(journal.path()).unwrap();
        assert_ne!(new, old);
        // The latest attempt belongs to the old setup: nothing about now.
        assert!(now(journal.path()).is_empty());
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(
            replaced(&config, Some(&new), Some(&old), Some(&old)),
            vec![Notice::Clear {
                setup: old.clone(),
                worded: None,
            }]
        );
        // The first attempt at the new setup starts its own run.
        attempt(
            journal.path(),
            "error",
            49 * HOUR,
            Some("auth_failed"),
            Some(&new),
        );
        assert!(!raised(&now(journal.path())));
        // When the new setup works, the old notice ends with words.
        attempt(journal.path(), "ok", 50 * HOUR, None, Some(&new));
        let config = get_backup_config(journal.path()).unwrap();
        assert_eq!(
            replaced(&config, Some(&new), Some(&old), Some(&new)),
            vec![Notice::Clear {
                setup: old,
                worded: Some(working_message(&Map::new())),
            }]
        );
    }

    #[test]
    fn no_backup_or_backup_off_says_nothing_or_ends_quietly() {
        let journal = tempfile::tempdir().unwrap();
        assert!(now(journal.path()).is_empty());
        let (journal, setup) = configured();
        for hour in 0..=48 {
            attempt(
                journal.path(),
                "error",
                hour * HOUR,
                Some("timeout"),
                Some(&setup),
            );
        }
        set_enabled(journal.path(), false).unwrap();
        assert_eq!(
            now(journal.path()),
            vec![Notice::Clear {
                setup,
                worded: None,
            }]
        );
    }

    #[test]
    fn every_reason_has_words_and_one_journal_action() {
        for reason in [
            None,
            Some("auth_failed"),
            Some("repo_missing"),
            Some("broker_unreachable"),
            Some("rclone_unavailable"),
            Some("restic_unavailable"),
            Some("hosted_entitlement_inactive"),
            Some("binding_invalid"),
            Some("repository_unsupported"),
            Some("timeout"),
            Some("locked"),
            Some("failed"),
            Some("something new"),
        ] {
            let line = line(&Notice::Raise {
                setup: "f".repeat(64),
                message: failing_message(reason),
            });
            assert!(!line["message"].as_str().unwrap().is_empty(), "{reason:?}");
            let action = line["action"].as_str().unwrap();
            assert!(action.starts_with("/app/") && !action.contains("//"));
        }
    }
}

#[cfg(unix)]
#[cfg(all(test, feature = "full-tests"))]
mod bus_tests {
    use std::io::Read;
    use std::os::unix::net::UnixListener;

    use serde_json::{Map, Value, json};
    use solstone_core_backup::{
        Destination, generate_and_store_keys, get_backup_config, set_backup_paused,
        set_destination, set_enabled,
    };

    use crate::engine::{BackupResult, Clock, record_backup};

    struct At(i64);
    impl Clock for At {
        fn now_unix(&self) -> i64 {
            self.0
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }

    fn result(status: &str, reason: Option<&str>) -> BackupResult {
        BackupResult {
            status: status.into(),
            snapshot_id: (status == "ok").then(|| "copy".into()),
            error_reason: reason.map(str::to_owned),
            unreadable: None,
            detail: None,
        }
    }

    /// Lines the producer sent during `body`, read from a stand-in bus socket.
    fn sent(listener: &UnixListener, body: impl FnOnce()) -> Vec<Value> {
        body();
        listener.set_nonblocking(true).unwrap();
        let mut lines = Vec::new();
        while let Ok((mut stream, _)) = listener.accept() {
            stream.set_nonblocking(false).unwrap();
            let mut text = String::new();
            stream.read_to_string(&mut text).unwrap();
            lines.extend(text.lines().map(|line| serde_json::from_str(line).unwrap()));
        }
        lines
    }

    #[test]
    fn the_backup_attempt_path_announces_raise_repeat_and_clear_on_the_bus() {
        let journal = tempfile::tempdir().unwrap();
        set_destination(
            journal.path(),
            &Destination {
                repository: "s3:fixture".into(),
                backend: "s3".into(),
                credentials: Map::new(),
            },
        )
        .unwrap();
        generate_and_store_keys(journal.path()).unwrap();
        set_enabled(journal.path(), true).unwrap();
        let config = get_backup_config(journal.path()).unwrap();
        let setup = solstone_core_backup::backup_setup_fingerprint(&config, None).unwrap();
        std::fs::create_dir_all(journal.path().join("health")).unwrap();
        let listener = UnixListener::bind(journal.path().join("health/callosum.sock")).unwrap();
        let key = format!("backup_failing:{}", &setup[..16]);
        let hour = 3600;

        let early = sent(&listener, || {
            for at in 0..48 {
                record_backup(
                    journal.path(),
                    &At(at * hour),
                    &result("error", Some("repo_missing")),
                    Some(&setup),
                );
            }
        });
        assert!(early.is_empty(), "{early:?}");

        let raised = sent(&listener, || {
            for at in [48, 49] {
                record_backup(
                    journal.path(),
                    &At(at * hour),
                    &result("error", Some("repo_missing")),
                    Some(&setup),
                );
            }
        });
        assert_eq!(raised.len(), 2, "{raised:?}");
        for line in &raised {
            assert_eq!(line["tract"], "owner_notice");
            assert_eq!(line["event"], "raise");
            assert_eq!(line["key"], key);
            assert_eq!(line["action"], "/app/backup/");
        }

        let cleared = sent(&listener, || {
            record_backup(
                journal.path(),
                &At(50 * hour),
                &result("ok", None),
                Some(&setup),
            );
        });
        assert_eq!(cleared.len(), 1, "{cleared:?}");
        assert_eq!(cleared[0]["event"], "clear");
        assert_eq!(cleared[0]["key"], key);
        assert!(cleared[0]["title"].is_string());

        // A failure that can't be tied to a setup says nothing.
        let unattributed = sent(&listener, || {
            record_backup(
                journal.path(),
                &At(51 * hour),
                &result("error", Some("broker_error")),
                None,
            );
        });
        assert!(unattributed.is_empty(), "{unattributed:?}");

        // A paused backup is skipped before any attempt, and ends quietly.
        set_backup_paused(journal.path(), Some(52 * hour)).unwrap();
        let paused = sent(&listener, || {
            let skipped = crate::engine::prepare(journal.path(), &At(53 * hour));
            assert!(matches!(skipped, Err(ref r) if r.detail.as_deref() == Some("paused")));
        });
        assert_eq!(
            paused,
            vec![json!({"tract":"owner_notice","event":"clear","key":key})]
        );
    }
}
