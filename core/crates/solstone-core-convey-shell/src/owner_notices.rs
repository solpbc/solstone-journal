// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner notices arriving on Callosum: `owner_notice` / `raise` and `clear`.
//!
//! A producer announces a fact it positively knows, under a stable episode key,
//! with the words the owner reads. This turns it into at most one push and one
//! journal card per episode start, and one per end when the producer gives
//! words for it. The episode record lives in `solstone_core_push::owner_notice`.

use serde_json::{Map, Value, json};
use solstone_core_callosum::CallosumEnvelope;
use solstone_core_push::owner_notice::{self, NoticeText, Transition};
use time::OffsetDateTime;

pub(crate) const TRACT: &str = "owner_notice";

/// Card prefix, so an episode's start and end replace each other in place.
const CARD_KEY_PREFIX: &str = "owner_notice:";

/// Apply one announcement and return the journal card to show, if any.
pub(crate) fn handle(
    journal: &std::path::Path,
    portal: &str,
    envelope: &CallosumEnvelope,
    now: OffsetDateTime,
) -> Option<Map<String, Value>> {
    if envelope.tract != TRACT {
        return None;
    }
    let key = field(envelope, "key")?;
    match envelope.event.as_str() {
        "raise" => {
            let text = text(envelope)?;
            match owner_notice::raise(journal, portal, key, &text, now) {
                Transition::Raised => Some(card(key, &text)),
                _ => None,
            }
        }
        "clear" => {
            let text = text(envelope);
            match owner_notice::clear(journal, portal, key, text.as_ref(), now) {
                Transition::Cleared => text.map(|text| card(key, &text)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn field<'a>(envelope: &'a CallosumEnvelope, name: &str) -> Option<&'a str> {
    envelope
        .extra
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn text(envelope: &CallosumEnvelope) -> Option<NoticeText> {
    Some(NoticeText {
        title: field(envelope, "title")?.to_owned(),
        body: field(envelope, "message").unwrap_or_default().to_owned(),
        // Only a journal app path: a notice never opens anywhere else.
        open: field(envelope, "action")
            .filter(|path| {
                path.starts_with("/app/") && !path.contains("//") && !path.contains("..")
            })
            .map(str::to_owned),
    })
}

fn card(key: &str, text: &NoticeText) -> Map<String, Value> {
    let mut fields = Map::from_iter([
        ("app".to_owned(), json!("journal")),
        ("icon".to_owned(), json!("bell")),
        ("title".to_owned(), json!(text.title)),
        ("message".to_owned(), json!(text.body)),
        ("key".to_owned(), json!(format!("{CARD_KEY_PREFIX}{key}"))),
    ]);
    if let Some(open) = &text.open {
        fields.insert("action".to_owned(), json!(open));
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(event: &str, extra: Value) -> CallosumEnvelope {
        CallosumEnvelope {
            tract: TRACT.to_owned(),
            event: event.to_owned(),
            ts: None,
            extra: extra.as_object().cloned().unwrap_or_default(),
        }
    }

    #[test]
    fn a_raise_shows_once_and_a_worded_clear_replaces_it() {
        let journal = tempfile::tempdir().expect("journal");
        let now = OffsetDateTime::UNIX_EPOCH;
        let raise = envelope(
            "raise",
            json!({"key":"disk_low","title":"space is low","message":"free some up","action":"/app/health"}),
        );
        let first = handle(journal.path(), "", &raise, now).expect("first raise shows");
        assert_eq!(first["key"], "owner_notice:disk_low");
        assert_eq!(first["action"], "/app/health");
        assert!(handle(journal.path(), "", &raise, now).is_none());

        let clear = envelope("clear", json!({"key":"disk_low","title":"space is back"}));
        let cleared = handle(journal.path(), "", &clear, now).expect("worded clear shows");
        assert_eq!(cleared["key"], first["key"]);
        assert!(handle(journal.path(), "", &clear, now).is_none());
    }

    #[test]
    fn malformed_or_foreign_announcements_show_nothing() {
        let journal = tempfile::tempdir().expect("journal");
        let now = OffsetDateTime::UNIX_EPOCH;
        for event in [
            envelope("raise", json!({"title":"no key"})),
            envelope("raise", json!({"key":"k"})),
            envelope("other", json!({"key":"k","title":"t"})),
        ] {
            assert!(handle(journal.path(), "", &event, now).is_none());
        }
        for (key, action) in [("k1", "https://example.com"), ("k2", "/app/../secret")] {
            let offsite = envelope("raise", json!({"key":key,"title":"t","action":action}));
            let card = handle(journal.path(), "", &offsite, now).expect("shows");
            assert!(card.get("action").is_none(), "{action}");
        }
    }
}
