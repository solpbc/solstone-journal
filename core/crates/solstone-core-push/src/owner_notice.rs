// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner notices: one notice when something the owner should know starts, and
//! at most one when it clears.
//!
//! Each notice has a stable key naming its episode. The journal records which
//! episodes are active, so a fact that is reported again while it lasts, or
//! across a restart, notifies once. A notice that starts an episode is pushed,
//! best effort, to the devices where the owner turned push on; the caller shows
//! the same words in the journal. A failed push is not retried: the journal's
//! own pages keep showing the fact.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};
use time::OffsetDateTime;

use crate::envelope::{Notification, format_notification_at};
use crate::relay::{RelayTransport, UreqRelay};
use crate::router::deliver_notification;

const OWNER_ONLY_MODE: u32 = 0o600;
const STATE_VERSION: u32 = 1;

/// The words of one notice, as the owner reads them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoticeText {
    pub title: String,
    pub body: String,
    /// Same-origin journal path the notice opens.
    pub open: Option<String>,
}

/// What a raise or clear did to its episode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transition {
    /// The episode started now; the notice was pushed and should be shown.
    Raised,
    /// The episode was already active; nothing was sent.
    AlreadyActive,
    /// The episode ended now; a clearing notice, if any, was pushed.
    Cleared,
    /// No active episode had this key; nothing was sent.
    AlreadyClear,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct State {
    version: u32,
    #[serde(default)]
    active: BTreeMap<String, ActiveEpisode>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ActiveEpisode {
    since: String,
}

fn state_path(journal: &Path) -> PathBuf {
    journal.join("health/owner-notices.json")
}

fn lock_path(journal: &Path) -> PathBuf {
    journal.join("health/owner-notices.lock")
}

/// Start the episode `key` and push `text` if it was not already active.
pub fn raise(
    journal: &Path,
    portal: &str,
    key: &str,
    text: &NoticeText,
    now: OffsetDateTime,
) -> Transition {
    raise_with_transport(journal, portal, key, text, now, &UreqRelay::default())
}

/// End the episode `key`, pushing `text` when it was active and one is given.
pub fn clear(
    journal: &Path,
    portal: &str,
    key: &str,
    text: Option<&NoticeText>,
    now: OffsetDateTime,
) -> Transition {
    clear_with_transport(journal, portal, key, text, now, &UreqRelay::default())
}

/// The active episode keys in `family`, the part of a key before its first
/// `:`, so a producer can end episodes for facts that no longer hold.
#[must_use]
pub fn active_keys(journal: &Path, family: &str) -> Vec<String> {
    fs::read(state_path(journal))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<State>(&bytes).ok())
        .map(|state| {
            state
                .active
                .into_keys()
                .filter(|key| kind_for(key) == family)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn raise_with_transport(
    journal: &Path,
    portal: &str,
    key: &str,
    text: &NoticeText,
    now: OffsetDateTime,
    transport: &dyn RelayTransport,
) -> Transition {
    let changed = update(journal, |state| {
        if state.active.contains_key(key) {
            return false;
        }
        state.active.insert(
            key.to_owned(),
            ActiveEpisode {
                since: format_notification_at(now),
            },
        );
        true
    });
    match changed {
        Some(true) => {
            push(journal, portal, key, text, now, transport);
            Transition::Raised
        }
        _ => Transition::AlreadyActive,
    }
}

pub(crate) fn clear_with_transport(
    journal: &Path,
    portal: &str,
    key: &str,
    text: Option<&NoticeText>,
    now: OffsetDateTime,
    transport: &dyn RelayTransport,
) -> Transition {
    let changed = update(journal, |state| state.active.remove(key).is_some());
    match changed {
        Some(true) => {
            if let Some(text) = text {
                push(journal, portal, key, text, now, transport);
            }
            Transition::Cleared
        }
        _ => Transition::AlreadyClear,
    }
}

/// Read, change and write the episode record under its lock. `None` means the
/// record could not be locked or written, so nothing is announced: a notice
/// that cannot be remembered would repeat.
fn update(journal: &Path, change: impl FnOnce(&mut State) -> bool) -> Option<bool> {
    let _ = fs::create_dir_all(journal.join("health"));
    let _lock = hold_lock(
        lock_path(journal),
        LockOptions {
            mode: Some(OWNER_ONLY_MODE),
            ..LockOptions::default()
        },
    )
    .ok()?;
    let path = state_path(journal);
    // An unreadable record is replaced: the worst case is one repeated notice.
    let mut state = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<State>(&bytes).ok())
        .unwrap_or_default();
    state.version = STATE_VERSION;
    if !change(&mut state) {
        return Some(false);
    }
    write_json(
        &path,
        &state,
        JsonWriteOptions {
            mode: Some(OWNER_ONLY_MODE),
            indent: Some(2),
            ..JsonWriteOptions::default()
        },
    )
    .ok()?;
    Some(true)
}

fn push(
    journal: &Path,
    portal: &str,
    key: &str,
    text: &NoticeText,
    now: OffsetDateTime,
    transport: &dyn RelayTransport,
) {
    let notification = Notification {
        at: now,
        kind: kind_for(key).to_owned(),
        title: text.title.clone(),
        body: text.body.clone(),
        open: text.open.clone(),
    };
    // Best effort: no recipient, a refused ledger or a relay fault leaves the
    // journal's own pages as the record.
    let _ = deliver_notification(journal, portal, now, &notification, transport, None);
}

/// The push kind is the key's family, the part before the first `:`.
fn kind_for(key: &str) -> &str {
    key.split(':').next().unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tempfile::TempDir;

    use super::*;
    use crate::relay::{RelayFault, RelayReply};

    struct CountingTransport {
        calls: AtomicUsize,
    }

    impl RelayTransport for CountingTransport {
        fn post_json(
            &self,
            _url: &str,
            _body: &[u8],
            _bearer_token: Option<&str>,
        ) -> Result<RelayReply, RelayFault> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(RelayReply {
                status: 200,
                body: Vec::new(),
            })
        }

        fn post_bytes(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
            _body: &[u8],
        ) -> Result<u16, RelayFault> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(201)
        }
    }

    fn text(title: &str) -> NoticeText {
        NoticeText {
            title: title.to_owned(),
            body: String::new(),
            open: None,
        }
    }

    #[test]
    fn an_episode_notifies_once_until_it_clears_and_once_more_when_it_returns() {
        let journal = TempDir::new().expect("journal");
        let relay = CountingTransport {
            calls: AtomicUsize::new(0),
        };
        let now = OffsetDateTime::UNIX_EPOCH;
        let key = "disk_low:journal";
        let started = text("started");
        let ended = text("ended");

        assert_eq!(
            raise_with_transport(journal.path(), "", key, &started, now, &relay),
            Transition::Raised
        );
        assert_eq!(
            raise_with_transport(journal.path(), "", key, &started, now, &relay),
            Transition::AlreadyActive
        );
        assert_eq!(
            clear_with_transport(journal.path(), "", key, Some(&ended), now, &relay),
            Transition::Cleared
        );
        assert_eq!(
            clear_with_transport(journal.path(), "", key, Some(&ended), now, &relay),
            Transition::AlreadyClear
        );
        assert_eq!(
            raise_with_transport(journal.path(), "", key, &started, now, &relay),
            Transition::Raised
        );
    }

    #[test]
    fn episodes_are_independent_by_key() {
        let journal = TempDir::new().expect("journal");
        let relay = CountingTransport {
            calls: AtomicUsize::new(0),
        };
        let now = OffsetDateTime::UNIX_EPOCH;
        let notice = text("n");
        assert_eq!(
            raise_with_transport(journal.path(), "", "a:1", &notice, now, &relay),
            Transition::Raised
        );
        assert_eq!(
            raise_with_transport(journal.path(), "", "a:2", &notice, now, &relay),
            Transition::Raised
        );
        assert_eq!(
            clear_with_transport(journal.path(), "", "a:1", None, now, &relay),
            Transition::Cleared
        );
        assert_eq!(
            raise_with_transport(journal.path(), "", "a:2", &notice, now, &relay),
            Transition::AlreadyActive
        );
    }

    #[test]
    fn active_keys_lists_one_family_and_follows_clears() {
        let journal = TempDir::new().expect("journal");
        let relay = CountingTransport {
            calls: AtomicUsize::new(0),
        };
        let now = OffsetDateTime::UNIX_EPOCH;
        let notice = text("n");
        assert!(active_keys(journal.path(), "a").is_empty());
        for key in ["a:1", "a:2", "b:1"] {
            raise_with_transport(journal.path(), "", key, &notice, now, &relay);
        }
        assert_eq!(active_keys(journal.path(), "a"), ["a:1", "a:2"]);
        clear_with_transport(journal.path(), "", "a:1", None, now, &relay);
        assert_eq!(active_keys(journal.path(), "a"), ["a:2"]);
    }

    #[test]
    fn the_push_kind_is_the_key_family() {
        assert_eq!(kind_for("access:device:abc"), "access");
        assert_eq!(kind_for("disk_low"), "disk_low");
    }
}
