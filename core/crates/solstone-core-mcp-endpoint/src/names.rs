// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Short names that stay the same for what an agent reads.
//!
//! | what | name |
//! |---|---|
//! | an activity | `<day>/<facet folder>/<activity id>` |
//! | a segment | `<day>/<segment key>/<source code>` |
//! | a person, project or other entity | `<entity id>@<facet folder>` |
//!
//! A name says which thing, never whether a connection may read it. Every read
//! decides that again, through [`crate::may_see`], so a name kept from an
//! earlier answer, copied from another connection or made up reads exactly
//! what this connection's own list would show now, and nothing else.
//!
//! ⛔ A name is untrusted input: an agent can write one for itself. No part of
//! it is ever joined into a path. A facet resolves against the facets the
//! connection reaches, a day and a segment against the journal's own listing
//! of them, and an id against the records a file holds.
//!
//! Paging cursors, spans and search entries stay sealed references.

use std::path::Path;

use ring::hmac;
use solstone_core_journal_io::is_day_key;
use solstone_core_journal_io::paths::{PathOrDay, iter_segments_skipping_unreadable_streams};
use solstone_core_journal_link::root::{WhenAbsent, sub_key};

use crate::references::{ActivityAddress, SegmentAddress};

/// The label source codes derive their sub-key under.
const CODE_KEY_LABEL: &[u8] = b"solstone agent-names source-code v1";
const CODE_KEY_BYTES: usize = 32;
/// 30 bits: two of a journal's sources sharing a code is about one chance in a
/// billion for each pair.
const CODE_CHARS: usize = 6;
const CODE_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Whether text can stand as one part of a name: it keeps its bytes through
/// release, and can't be mistaken for a separator or a sealed reference.
fn is_name_part(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('.')
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// Whether a reference an agent passed is written as a name, not sealed. A
/// sealed reference holds neither separator.
pub(crate) fn is_name(value: &str) -> bool {
    value.contains(['/', '@'])
}

/// An activity's name, or `None` when a part of it can't be written in one: its
/// caller then gives a sealed reference, as before.
pub(crate) fn activity_name(day: &str, facet_dir: &str, identity: &str) -> Option<String> {
    (is_day_key(day) && is_name_part(facet_dir) && is_name_part(identity))
        .then(|| format!("{day}/{facet_dir}/{identity}"))
}

/// The activity a name asks for. Nothing here says it exists.
pub(crate) fn parse_activity(value: &str) -> Option<ActivityAddress> {
    let mut parts = value.split('/');
    let (day, facet, identity) = (parts.next()?, parts.next()?, parts.next()?);
    (parts.next().is_none() && is_day_key(day) && is_name_part(facet) && is_name_part(identity))
        .then(|| ActivityAddress {
            facet: facet.to_owned(),
            day: day.to_owned(),
            record_identity: identity.to_owned(),
            file_digest: None,
            unit_index: None,
            source_binding: None,
        })
}

/// An entity's name in one facet, or `None` when a part can't be written.
pub(crate) fn entity_name(facet_dir: &str, entity_id: &str) -> Option<String> {
    (is_name_part(facet_dir) && is_name_part(entity_id)).then(|| format!("{entity_id}@{facet_dir}"))
}

/// The entity id and facet a name asks for.
pub(crate) fn parse_entity(value: &str) -> Option<(String, String)> {
    let (entity_id, facet) = value.split_once('@')?;
    (is_name_part(entity_id) && is_name_part(facet))
        .then(|| (entity_id.to_owned(), facet.to_owned()))
}

/// A segment's name, or `None` when it can't be written or the journal's code
/// secret can't be read or made.
pub(crate) fn segment_name(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Option<String> {
    if !is_day_key(day) || !is_name_part(segment) {
        return None;
    }
    Some(format!(
        "{day}/{segment}/{}",
        source_code(journal_root, stream)?
    ))
}

/// The segment a name asks for, found among the day's segments as the journal
/// lists them. Two sources sharing the code is refused, never settled by
/// picking one.
pub(crate) fn resolve_segment(journal_root: &Path, value: &str) -> Option<SegmentAddress> {
    let mut parts = value.split('/');
    let (day, key, code) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some()
        || !is_day_key(day)
        || !is_name_part(key)
        || code.len() != CODE_CHARS
        || !is_name_part(code)
    {
        return None;
    }
    let mut found = None;
    for segment in
        iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(day)).ok()?
    {
        let Ok(identity) = segment.record_identity() else {
            continue;
        };
        if identity.name != key
            || source_code(journal_root, identity.stream).as_deref() != Some(code)
        {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(SegmentAddress {
            day: day.to_owned(),
            stream: identity.stream.to_owned(),
            segment: identity.name.to_owned(),
            version: None,
        });
    }
    found
}

/// The code that stands for a stream in this journal's names.
///
/// ⛔ Never the stream's name, nor anything it can be worked back from: an
/// agent learns where something came from only in the owner's words.
pub(crate) fn source_code(journal_root: &Path, stream: &str) -> Option<String> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, &code_key(journal_root)?);
    let tag = hmac::sign(&key, stream.as_bytes());
    let bits = tag
        .as_ref()
        .iter()
        .take(4)
        .fold(0_u32, |bits, byte| (bits << 8) | u32::from(*byte));
    Some(
        (0..CODE_CHARS)
            .map(|index| CODE_ALPHABET[((bits >> (27 - 5 * index)) & 31) as usize] as char)
            .collect(),
    )
}

/// The sub-key source codes are made with.
///
/// ⚠ Read from the journal each time, never kept: a restore can put another
/// journal's root in place under a running process, and a kept key would go
/// on giving names that stop resolving at the next start.
fn code_key(journal_root: &Path) -> Option<[u8; CODE_KEY_BYTES]> {
    let mut key = [0_u8; CODE_KEY_BYTES];
    // The journal's one root secret, under its own label: no other use of
    // the root shares these bytes. Naming something makes the root when the
    // journal has none.
    sub_key(journal_root, CODE_KEY_LABEL, WhenAbsent::Create, &mut key).ok()?;
    Some(key)
}

#[cfg(test)]
mod tests {
    use solstone_core_journal_link::root::ROOT_KEY_PATH;

    use super::*;

    fn journal() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("solstone-mcp-names-")
            .tempdir_in(crate::test_scratch())
            .unwrap()
    }

    #[test]
    fn a_name_is_written_only_from_parts_that_keep_their_bytes() {
        assert_eq!(
            activity_name("20261109", "schema-bridge", "meeting_150000_232").as_deref(),
            Some("20261109/schema-bridge/meeting_150000_232")
        );
        assert_eq!(
            entity_name("schema-bridge", "benvolio_montague").as_deref(),
            Some("benvolio_montague@schema-bridge")
        );
        for (day, facet, id) in [
            ("2026110", "work", "a"),
            ("20261109", "", "a"),
            ("20261109", ".scratch", "a"),
            ("20261109", "two words", "a"),
            ("20261109", "work", "a/b"),
            ("20261109", "work", "a@b"),
            ("20261109", "work", "[a](b)"),
            ("20261109", "work", "<a>"),
        ] {
            assert_eq!(activity_name(day, facet, id), None, "{day} {facet} {id}");
        }
        assert_eq!(entity_name("work", "a@b"), None);
    }

    #[test]
    fn a_name_that_is_not_one_asks_for_nothing() {
        let parsed = parse_activity("20261109/work/meeting_1").unwrap();
        assert_eq!(
            (
                parsed.day.as_str(),
                parsed.facet.as_str(),
                parsed.record_identity.as_str()
            ),
            ("20261109", "work", "meeting_1")
        );
        assert!(parsed.file_digest.is_none());
        for value in [
            "20261109/work",
            "20261109/work/a/b",
            "20261109/../a",
            "20261109/.scratch/a",
            "../work/a",
            "20261109//a",
            "20261109/work/",
            "/20261109/work/a",
            "20261109\\work\\a",
            "20261109/wo rk/a",
        ] {
            assert!(parse_activity(value).is_none(), "{value}");
        }
        assert_eq!(
            parse_entity("alice@work"),
            Some(("alice".to_owned(), "work".to_owned()))
        );
        for value in [
            "alice",
            "alice@",
            "@work",
            "alice@work@x",
            "al/ice@work",
            "alice@../x",
        ] {
            assert!(parse_entity(value).is_none(), "{value}");
        }
        assert!(is_name("20261109/work/a") && is_name("alice@work"));
        assert!(!is_name("AbC-_09"));
    }

    #[test]
    fn a_source_code_is_stable_for_a_journal_and_says_nothing_of_the_stream() {
        let journal = journal();
        let root = journal.path();
        let code = source_code(root, "import.apple_health").unwrap();
        assert_eq!(code.len(), CODE_CHARS);
        assert!(code.bytes().all(|byte| CODE_ALPHABET.contains(&byte)));
        assert_eq!(source_code(root, "import.apple_health").unwrap(), code);
        assert_ne!(source_code(root, "desk").unwrap(), code);
        assert!(!"import.apple_health".contains(&code));
        // The secret is on disk for a backup to carry, owner-only.
        let path = root.join(ROOT_KEY_PATH);
        let stored = std::fs::read_to_string(&path).unwrap();
        assert_eq!(stored.trim().len(), CODE_KEY_BYTES * 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Another journal has its own, so two journals' codes can't be compared.
        let other = self::journal();
        std::fs::create_dir_all(other.path().join("link")).unwrap();
        std::fs::write(
            other.path().join(ROOT_KEY_PATH),
            "00".repeat(CODE_KEY_BYTES),
        )
        .unwrap();
        assert_ne!(
            source_code(other.path(), "import.apple_health").unwrap(),
            code
        );
        // A copy of the journal carrying the secret keeps every code.
        let moved = self::journal();
        std::fs::create_dir_all(moved.path().join("link")).unwrap();
        std::fs::write(moved.path().join(ROOT_KEY_PATH), stored).unwrap();
        assert_eq!(
            source_code(moved.path(), "import.apple_health").unwrap(),
            code
        );
    }

    /// The derivation is a contract: journal links take another sub-key of the
    /// same root, and a restored journal must make the codes it made before.
    #[test]
    fn a_source_code_is_derived_from_the_root_the_same_way_every_time() {
        let journal = journal();
        let root = journal.path();
        std::fs::create_dir_all(root.join("link")).unwrap();
        std::fs::write(root.join(ROOT_KEY_PATH), format!("{}\n", "0f".repeat(32))).unwrap();
        // HKDF-SHA256 (no salt) of the root under the label, then HMAC-SHA256
        // of the stream's name, first 30 bits in lowercase base32.
        let expected = {
            let prk = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &[0_u8; 32]), &[0x0f; 32]);
            let mut info = CODE_KEY_LABEL.to_vec();
            info.push(1);
            let okm = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, prk.as_ref()), &info);
            let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, okm.as_ref()), b"desk");
            let bits = tag.as_ref()[..4]
                .iter()
                .fold(0_u32, |bits, byte| (bits << 8) | u32::from(*byte));
            (0..6)
                .map(|index| CODE_ALPHABET[((bits >> (27 - 5 * index)) & 31) as usize] as char)
                .collect::<String>()
        };
        assert_eq!(source_code(root, "desk").unwrap(), expected);
    }

    #[test]
    fn a_root_replaced_under_a_running_journal_is_the_one_names_use() {
        let running = journal();
        let restarted = journal();
        let root = running.path();
        std::fs::create_dir_all(root.join("link")).unwrap();
        std::fs::write(root.join(ROOT_KEY_PATH), format!("{}\n", "0f".repeat(32))).unwrap();
        let before = source_code(root, "macbook").unwrap();
        // What a restore of another journal does while this one runs.
        std::fs::write(root.join(ROOT_KEY_PATH), format!("{}\n", "a5".repeat(32))).unwrap();
        let after = source_code(root, "macbook").unwrap();
        assert_ne!(before, after);
        std::fs::create_dir_all(restarted.path().join("link")).unwrap();
        std::fs::write(
            restarted.path().join(ROOT_KEY_PATH),
            format!("{}\n", "a5".repeat(32)),
        )
        .unwrap();
        assert_eq!(source_code(restarted.path(), "macbook").unwrap(), after);
    }

    #[test]
    fn a_secret_that_cant_be_read_names_nothing() {
        let journal = journal();
        let root = journal.path();
        std::fs::create_dir_all(root.join("link")).unwrap();
        std::fs::write(root.join(ROOT_KEY_PATH), "damaged").unwrap();
        assert!(source_code(root, "desk").is_none());
        assert!(segment_name(root, "20261109", "desk", "090000_300").is_none());
        // It is left as it is: replacing it would change every name.
        assert_eq!(
            std::fs::read_to_string(root.join(ROOT_KEY_PATH)).unwrap(),
            "damaged"
        );
    }

    #[test]
    fn a_segment_name_finds_its_segment_and_refuses_a_shared_code() {
        let journal = journal();
        let root = journal.path();
        let segment = |stream: &str, key: &str| {
            let dir = root.join("chronicle/20261109").join(stream).join(key);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("audio.jsonl"), "{}\n").unwrap();
        };
        segment("desk", "090000_300");
        segment("phone", "090000_300");
        let name = segment_name(root, "20261109", "desk", "090000_300").unwrap();
        assert!(!name.contains("desk"), "{name}");
        let found = resolve_segment(root, &name).unwrap();
        assert_eq!(
            (found.stream.as_str(), found.segment.as_str(), found.version),
            ("desk", "090000_300", None)
        );
        let other = segment_name(root, "20261109", "phone", "090000_300").unwrap();
        assert_ne!(name, other);
        assert_eq!(resolve_segment(root, &other).unwrap().stream, "phone");
        // Nothing an agent writes for itself finds a segment that isn't listed.
        for value in [
            "20261109/090000_300/aaaaaa".to_owned(),
            "20261109/091500_300/".to_owned() + &name[name.len() - CODE_CHARS..],
            "20261110/090000_300/".to_owned() + &name[name.len() - CODE_CHARS..],
            "20261109/desk/090000_300".to_owned(),
            "20261109/../090000_300".to_owned(),
            format!("{name}/audio.jsonl"),
        ] {
            assert!(resolve_segment(root, &value).is_none(), "{value}");
        }
    }
}
