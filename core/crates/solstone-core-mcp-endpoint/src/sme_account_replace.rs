// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's request to bring a solstone.me address back to this journal.
//!
//! When the account service holds a different certificate account for this
//! journal's address (a journal restored or set up again makes a new one), it
//! refuses the address until the journal asks it to move the pin, in a signed
//! request. The owner's click in the agents app is that ask: the web process
//! writes a short-lived request here, and the service process carries it on its
//! next address request. Nothing here moves the pin without that click.
//!
//! The owner is told whenever the service reports a move this journal has not
//! shown yet, whichever request learned of it, so a move that lands while its
//! answer is lost or refused is still shown.

use std::path::Path;
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{JsonWriteOptions, write_json};

use crate::bridge_carrier::RegistrationHold;

const REQUEST_PATH: &str = "mcp-endpoint/sme-account-replace.json";
const NOTICE_PATH: &str = "mcp-endpoint/sme-account-replaced.json";
const SEEN_PATH: &str = "mcp-endpoint/sme-account-replaced-seen.json";

/// How long one click stays usable while no answer comes back. Long enough to
/// outlast the longest hold between address requests; short enough that an
/// unanswered click never moves the pin much later on its own.
const REQUEST_LIFETIME_SECONDS: i64 = 600;
/// A request stamped this far ahead of the clock is treated as unusable.
const REQUEST_CLOCK_SLACK_SECONDS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceRequest {
    schema: u32,
    #[serde(with = "solstone_core_time::serde::timestamp")]
    requested_at: Timestamp,
}

/// What the owner sees after the address came back to this journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplacedNotice {
    schema: u32,
    pub(crate) address: String,
    #[serde(with = "solstone_core_time::serde::timestamp")]
    pub(crate) replaced_at: Timestamp,
}

/// The last replacement the service reported, as it reported it. Outlives the
/// owner dismissing the notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeenReplacement {
    schema: u32,
    reported: String,
}

fn private() -> JsonWriteOptions {
    JsonWriteOptions {
        mode: Some(0o600),
        ..JsonWriteOptions::default()
    }
}

/// Record the owner's click. The caller has already checked that the address
/// is refused for a changed account.
pub(crate) fn record_request(journal_root: &Path, now: Timestamp) -> Result<(), String> {
    write_json(
        journal_root.join(REQUEST_PATH),
        &ReplaceRequest {
            schema: 1,
            requested_at: now,
        },
        private(),
    )
    .map_err(|error| error.to_string())
}

/// The stamp of the owner's click, if one is still usable. A stale or
/// unreadable request is removed so it can never be carried later.
pub(crate) fn pending_request(journal_root: &Path, now: Timestamp) -> Option<Timestamp> {
    let path = journal_root.join(REQUEST_PATH);
    let bytes = std::fs::read(&path).ok()?;
    let usable = serde_json::from_slice::<ReplaceRequest>(&bytes)
        .ok()
        .filter(|request| request.schema == 1)
        .map(|request| request.requested_at)
        .filter(|requested_at| {
            let age = now.duration_since(*requested_at);
            age >= -jiff::SignedDuration::from_secs(REQUEST_CLOCK_SLACK_SECONDS)
                && age <= jiff::SignedDuration::from_secs(REQUEST_LIFETIME_SECONDS)
        });
    if usable.is_none() {
        let _ = std::fs::remove_file(&path);
    }
    usable
}

/// The service answered a request that carried the click stamped `stamp`:
/// whatever it said, that click is spent. A newer click written meanwhile is
/// kept.
pub(crate) fn clear_request(journal_root: &Path, stamp: Timestamp) {
    let path = journal_root.join(REQUEST_PATH);
    let still_this_click = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ReplaceRequest>(&bytes).ok())
        .is_some_and(|request| request.requested_at == stamp);
    if still_this_click {
        let _ = std::fs::remove_file(path);
    }
}

/// Withdraw any click, as when the owner turns solstone.me off.
pub(crate) fn withdraw_request(journal_root: &Path) {
    let _ = std::fs::remove_file(journal_root.join(REQUEST_PATH));
}

/// The service served the address and reported when its account was last
/// replaced (`reported`, verbatim). A report this journal has not seen before
/// is a move to this journal's account: the owner is told, with the service's
/// time when it reads as one and otherwise the time the journal learned of it.
pub(crate) fn observe_replacement(
    journal_root: &Path,
    address: &str,
    reported: &str,
    replaced_at: Option<Timestamp>,
    now: Timestamp,
) {
    let seen_path = journal_root.join(SEEN_PATH);
    let already_seen = std::fs::read(&seen_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SeenReplacement>(&bytes).ok())
        .is_some_and(|seen| seen.schema == 1 && seen.reported == reported);
    if already_seen {
        return;
    }
    let notice = write_json(
        journal_root.join(NOTICE_PATH),
        &ReplacedNotice {
            schema: 1,
            address: address.to_owned(),
            replaced_at: replaced_at.unwrap_or(now),
        },
        private(),
    );
    // Only a notice that reached disk counts as shown; otherwise the next
    // answer tries again.
    if notice.is_ok() {
        let _ = write_json(
            seen_path,
            &SeenReplacement {
                schema: 1,
                reported: reported.to_owned(),
            },
            private(),
        );
    }
}

pub(crate) fn read_notice(journal_root: &Path) -> Option<ReplacedNotice> {
    let notice: ReplacedNotice =
        serde_json::from_slice(&std::fs::read(journal_root.join(NOTICE_PATH)).ok()?).ok()?;
    (notice.schema == 1).then_some(notice)
}

pub(crate) fn dismiss_notice(journal_root: &Path) -> Result<(), String> {
    match std::fs::remove_file(journal_root.join(NOTICE_PATH)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Wait out a registration hold. A changed-account hold ends early when the
/// owner clicks, once per click: a click already pending when the hold began
/// was carried by the request that led here and does not end it again.
pub(crate) async fn wait_out_hold(journal_root: &Path, hold: RegistrationHold, delay: Duration) {
    let deadline = tokio::time::Instant::now() + delay;
    if hold != RegistrationHold::AccountChanged {
        tokio::time::sleep_until(deadline).await;
        return;
    }
    let carried = pending_request(journal_root, Timestamp::now());
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return,
            _ = ticker.tick() => {
                if new_click(journal_root, carried) {
                    return;
                }
            }
        }
    }
}

/// Whether a usable click other than `carried` is waiting.
pub(crate) fn new_click(journal_root: &Path, carried: Option<Timestamp>) -> bool {
    pending_request(journal_root, Timestamp::now()).is_some_and(|stamp| Some(stamp) != carried)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal() -> tempfile::TempDir {
        let dir = tempfile::tempdir_in(crate::test_scratch()).expect("tempdir");
        std::fs::create_dir_all(dir.path().join("mcp-endpoint")).expect("mkdir");
        dir
    }

    #[test]
    fn a_click_is_usable_for_ten_minutes_and_then_removed() {
        let dir = journal();
        let clicked = Timestamp::now();
        assert_eq!(pending_request(dir.path(), clicked), None);
        record_request(dir.path(), clicked).expect("records");
        assert_eq!(pending_request(dir.path(), clicked), Some(clicked));
        assert!(
            pending_request(
                dir.path(),
                clicked + jiff::SignedDuration::from_secs(REQUEST_LIFETIME_SECONDS)
            )
            .is_some()
        );
        assert_eq!(
            pending_request(
                dir.path(),
                clicked + jiff::SignedDuration::from_secs(REQUEST_LIFETIME_SECONDS + 1)
            ),
            None
        );
        assert!(!dir.path().join(REQUEST_PATH).exists());
    }

    #[test]
    fn a_click_stamped_in_the_future_or_unreadable_is_removed_not_carried() {
        let dir = journal();
        let now = Timestamp::now();
        record_request(dir.path(), now + jiff::SignedDuration::from_secs(120)).expect("records");
        assert_eq!(pending_request(dir.path(), now), None);
        assert!(!dir.path().join(REQUEST_PATH).exists());
        std::fs::write(dir.path().join(REQUEST_PATH), b"{not json").expect("write");
        assert_eq!(pending_request(dir.path(), now), None);
        assert!(!dir.path().join(REQUEST_PATH).exists());
    }

    #[test]
    fn spending_a_click_keeps_a_newer_one() {
        let dir = journal();
        let first = Timestamp::now();
        let second = first + jiff::SignedDuration::from_secs(5);
        record_request(dir.path(), second).expect("records");
        clear_request(dir.path(), first);
        assert_eq!(pending_request(dir.path(), second), Some(second));
        clear_request(dir.path(), second);
        assert_eq!(pending_request(dir.path(), second), None);
    }

    #[test]
    fn each_reported_replacement_is_shown_once_even_after_dismissal() {
        let dir = journal();
        let now = Timestamp::now();
        let at = solstone_core_time::parse_rfc3339("2026-10-16T18:00:00Z")
            .expect("time")
            .timestamp();
        observe_replacement(
            dir.path(),
            "aaaqeaye.solstone.me",
            "2026-10-16T18:00:00Z",
            Some(at),
            now,
        );
        let notice = read_notice(dir.path()).expect("notice");
        assert_eq!(notice.address, "aaaqeaye.solstone.me");
        assert_eq!(notice.replaced_at, at);
        dismiss_notice(dir.path()).expect("dismisses");
        assert_eq!(read_notice(dir.path()), None);
        observe_replacement(
            dir.path(),
            "aaaqeaye.solstone.me",
            "2026-10-16T18:00:00Z",
            Some(at),
            now,
        );
        assert_eq!(read_notice(dir.path()), None, "the same report is not news");
        observe_replacement(
            dir.path(),
            "aaaqeaye.solstone.me",
            "a later report the journal cannot read as a time",
            None,
            now,
        );
        let notice = read_notice(dir.path()).expect("a new report is shown");
        assert_eq!(notice.replaced_at, now);
        dismiss_notice(dir.path()).expect("dismissing again is fine");
        dismiss_notice(dir.path()).expect("dismissing nothing is fine");
    }

    #[tokio::test(start_paused = true)]
    async fn a_changed_account_hold_ends_on_a_new_click_only() {
        let dir = journal();
        let root = dir.path().to_path_buf();
        // A click already carried does not end the hold again.
        record_request(&root, Timestamp::now()).expect("records");
        let waiting = tokio::spawn({
            let root = root.clone();
            async move {
                wait_out_hold(
                    &root,
                    RegistrationHold::AccountChanged,
                    Duration::from_secs(300),
                )
                .await
            }
        });
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        record_request(&root, Timestamp::now() + jiff::SignedDuration::from_secs(1))
            .expect("records");
        tokio::time::advance(Duration::from_secs(2)).await;
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(waiting.is_finished());

        // Any other hold runs its full length whatever the owner clicks.
        let other = tokio::spawn({
            let root = root.clone();
            async move {
                wait_out_hold(
                    &root,
                    RegistrationHold::AddressNotReady,
                    Duration::from_secs(60),
                )
                .await
            }
        });
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        record_request(&root, Timestamp::now() + jiff::SignedDuration::from_secs(2))
            .expect("records");
        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::task::yield_now().await;
        assert!(!other.is_finished());
        tokio::time::advance(Duration::from_secs(31)).await;
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(other.is_finished());
    }
}
