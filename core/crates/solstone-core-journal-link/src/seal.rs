// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use aes_siv::KeyInit;
use aes_siv::siv::Aes256Siv;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::root::{RootError, WhenAbsent, sub_key};

/// The label link sealing derives its sub-key under. No other use of the
/// root shares it, and no other primitive uses this sub-key.
const SEAL_KEY_LABEL: &[u8] = b"solstone journal link seal v1";
/// AES-256-SIV takes two 256-bit keys.
const SEAL_KEY_BYTES: usize = 64;
/// The first byte of every sealed link. A later construction or key
/// generation takes another value, and this one keeps opening.
const VERSION: u8 = 1;
/// Bound into every seal beside the version byte.
const LINK_LABEL: &[u8] = b"solstone journal link";
const SIV_TAG_BYTES: usize = 16;

/// The longest sealed text the journal makes or will look at.
pub const MAX_SEALED_CHARS: usize = 512;

/// A place in a journal that a link can name.
///
/// ⛔ These parts are what the journal was asked to open, not where it is on
/// disk. A resolver looks each one up; it never joins them into a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Day {
        day: String,
    },
    /// One recording. `stream` is the stream's own name: a segment key is
    /// unique only within a day and a stream.
    Recording {
        day: String,
        stream: String,
        segment: String,
    },
    Activity {
        facet: String,
        day: String,
        id: String,
    },
    Person {
        id: String,
    },
}

/// Why sealed text did not open to a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// This journal's key does not open it: it is another journal's, it was
    /// altered, or it is not a link at all. These can't be told apart.
    NotOpenedHere,
    /// This journal sealed it, and it names a kind of place this build
    /// doesn't know.
    UnknownPlace,
    /// The journal's key couldn't be read, so nothing can be said about the
    /// link either way.
    KeyUnusable,
}

/// Seal a place into link text: one version byte and the AES-SIV output, in
/// base64url without padding.
///
/// The same place always seals to the same text. `None` when a part can't be
/// written in a link, the text would pass [`MAX_SEALED_CHARS`], or the
/// journal's key can't be had.
pub fn seal(journal_root: &Path, place: &Place) -> Option<String> {
    let plaintext = encode(place)?;
    let key = seal_key(journal_root, WhenAbsent::Create).ok()?;
    let mut sealed = vec![VERSION];
    sealed.extend(
        Aes256Siv::new(&key.into())
            .encrypt([LINK_LABEL, &[VERSION]], plaintext.as_bytes())
            .ok()?,
    );
    let text = URL_SAFE_NO_PAD.encode(sealed);
    (text.len() <= MAX_SEALED_CHARS).then_some(text)
}

/// Open sealed link text to the place it names. Writes nothing: a journal
/// with no key has sealed nothing, so nothing opens.
pub fn open(journal_root: &Path, sealed: &str) -> Result<Place, OpenError> {
    if sealed.is_empty()
        || sealed.len() > MAX_SEALED_CHARS
        || !sealed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(OpenError::NotOpenedHere);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(sealed)
        .map_err(|_| OpenError::NotOpenedHere)?;
    let Some((&VERSION, ciphertext)) = bytes.split_first() else {
        return Err(OpenError::NotOpenedHere);
    };
    if ciphertext.len() < SIV_TAG_BYTES {
        return Err(OpenError::NotOpenedHere);
    }
    let key = match seal_key(journal_root, WhenAbsent::Refuse) {
        Ok(key) => key,
        Err(RootError::Absent) => return Err(OpenError::NotOpenedHere),
        Err(RootError::Unusable) => return Err(OpenError::KeyUnusable),
    };
    let plaintext = Aes256Siv::new(&key.into())
        .decrypt([LINK_LABEL, &[VERSION]], ciphertext)
        .map_err(|_| OpenError::NotOpenedHere)?;
    std::str::from_utf8(&plaintext)
        .ok()
        .and_then(decode)
        .ok_or(OpenError::UnknownPlace)
}

/// The sealing sub-key, read from the journal's root each time. Nothing is
/// kept between calls: a journal restored in place under a running process
/// opens its own links at once, and never seals under a key that is gone.
fn seal_key(
    journal_root: &Path,
    when_absent: WhenAbsent,
) -> Result<[u8; SEAL_KEY_BYTES], RootError> {
    let mut key = [0_u8; SEAL_KEY_BYTES];
    sub_key(journal_root, SEAL_KEY_LABEL, when_absent, &mut key)?;
    Ok(key)
}

/// Whether text can stand as one part of a sealed place.
fn is_part(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn encode(place: &Place) -> Option<String> {
    let (kind, parts): (&str, Vec<&str>) = match place {
        Place::Day { day } => ("d", vec![day]),
        Place::Recording {
            day,
            stream,
            segment,
        } => ("r", vec![day, stream, segment]),
        Place::Activity { facet, day, id } => ("a", vec![facet, day, id]),
        Place::Person { id } => ("p", vec![id]),
    };
    parts
        .iter()
        .all(|part| is_part(part))
        .then(|| format!("{kind}/{}", parts.join("/")))
}

fn decode(text: &str) -> Option<Place> {
    let mut parts = text.split('/');
    let kind = parts.next()?;
    let parts: Vec<&str> = parts.collect();
    if !parts.iter().all(|part| is_part(part)) {
        return None;
    }
    let owned = |part: &str| part.to_owned();
    match (kind, parts.as_slice()) {
        ("d", [day]) => Some(Place::Day { day: owned(day) }),
        ("r", [day, stream, segment]) => Some(Place::Recording {
            day: owned(day),
            stream: owned(stream),
            segment: owned(segment),
        }),
        ("a", [facet, day, id]) => Some(Place::Activity {
            facet: owned(facet),
            day: owned(day),
            id: owned(id),
        }),
        ("p", [id]) => Some(Place::Person { id: owned(id) }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::ROOT_KEY_PATH;

    fn journal_with_root(hex_byte: &str) -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("link")).unwrap();
        std::fs::write(
            journal.path().join(ROOT_KEY_PATH),
            format!("{}\n", hex_byte.repeat(32)),
        )
        .unwrap();
        journal
    }

    fn places() -> Vec<Place> {
        let s = str::to_owned;
        vec![
            Place::Day { day: s("20261111") },
            Place::Recording {
                day: s("20261111"),
                stream: s("desk"),
                segment: s("173000_57"),
            },
            Place::Activity {
                facet: s("schema-bridge"),
                day: s("20261109"),
                id: s("meeting_150000_232"),
            },
            Place::Person {
                id: s("benvolio_montague"),
            },
        ]
    }

    /// The construction is a contract: a restored journal must open the
    /// links it made before, and another implementation of RFC 5297 must
    /// agree. Expected values are from Python `cryptography`'s AESSIV under
    /// HKDF-SHA256 (no salt, the label as info, 64 bytes) of a root of
    /// thirty-two 0x0f bytes, with the link label and the version byte as
    /// associated data.
    #[test]
    fn a_place_seals_the_same_way_every_time_and_as_rfc_5297_says() {
        let journal = journal_with_root("0f");
        let expected = [
            "AY3zI6DprkllmQAtPdMXadj47MD-UjZitXQD",
            "AdaDgrowUItp7QtO14aarahKeoXPgeULcGeRF4azPzjaUV9CLzsleULK",
            "ATglv52gKwHCnB5Tm21OlV203GS7mNxk2P1Vumcyfz9zDWozmM4nRhN0vFRoxAeX1qwiyqtDv9R43nHH",
            "Ae_XFxIYIsdN5h_yJgo8PodCmjpEUGIPGTJ8w9jqRaMz28gs",
        ];
        for (place, expected) in places().iter().zip(expected) {
            assert_eq!(seal(journal.path(), place).as_deref(), Some(expected));
            assert_eq!(open(journal.path(), expected).as_ref(), Ok(place));
        }
    }

    #[test]
    fn another_journal_and_altered_text_refuse_alike() {
        let journal = journal_with_root("0f");
        let other = journal_with_root("1e");
        for place in places() {
            let sealed = seal(journal.path(), &place).unwrap();
            assert_eq!(
                open(other.path(), &sealed),
                Err(OpenError::NotOpenedHere),
                "another journal's key"
            );
            // Every single-character change, at every position.
            for index in 0..sealed.len() {
                let mut altered = sealed.clone().into_bytes();
                altered[index] = if altered[index] == b'A' { b'B' } else { b'A' };
                let altered = String::from_utf8(altered).unwrap();
                assert_eq!(
                    open(journal.path(), &altered),
                    Err(OpenError::NotOpenedHere),
                    "{altered}"
                );
            }
        }
    }

    #[test]
    fn text_that_is_not_a_link_refuses_before_any_key_is_read() {
        // No root at all: a damaged one would answer KeyUnusable.
        let journal = tempfile::tempdir().unwrap();
        let long = "A".repeat(MAX_SEALED_CHARS + 1);
        for text in [
            "",
            "AQ",
            "not a link",
            "AY3z+6Dp",
            "AY3z/6Dp",
            "AY3z=",
            &long,
        ] {
            assert_eq!(
                open(journal.path(), text),
                Err(OpenError::NotOpenedHere),
                "{text}"
            );
        }
        assert!(!journal.path().join("link").exists());
    }

    #[test]
    fn opening_never_makes_a_key_and_sealing_makes_it_once() {
        let journal = tempfile::tempdir().unwrap();
        let day = Place::Day {
            day: "20261111".to_owned(),
        };
        let foreign = seal(journal_with_root("0f").path(), &day).unwrap();
        assert_eq!(
            open(journal.path(), &foreign),
            Err(OpenError::NotOpenedHere)
        );
        assert!(!journal.path().join(ROOT_KEY_PATH).exists());

        let sealed = seal(journal.path(), &day).unwrap();
        let stored = std::fs::read_to_string(journal.path().join(ROOT_KEY_PATH)).unwrap();
        assert_eq!(stored.trim().len(), 64);
        assert_eq!(open(journal.path(), &sealed), Ok(day.clone()));
        assert_eq!(seal(journal.path(), &day).unwrap(), sealed);
        assert_eq!(
            std::fs::read_to_string(journal.path().join(ROOT_KEY_PATH)).unwrap(),
            stored
        );
    }

    #[test]
    fn a_journal_restored_in_place_opens_its_own_links_at_once() {
        let journal = journal_with_root("0f");
        let day = Place::Day {
            day: "20261111".to_owned(),
        };
        let before = seal(journal.path(), &day).unwrap();
        // The restore puts another journal's key where this one's was.
        std::fs::write(
            journal.path().join(ROOT_KEY_PATH),
            format!("{}\n", "1e".repeat(32)),
        )
        .unwrap();
        let after = seal(journal.path(), &day).unwrap();
        assert_ne!(before, after);
        assert_eq!(open(journal.path(), &after), Ok(day));
        assert_eq!(open(journal.path(), &before), Err(OpenError::NotOpenedHere));
    }

    #[test]
    fn a_key_that_cant_be_read_is_never_replaced() {
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(journal.path().join("link")).unwrap();
        std::fs::write(journal.path().join(ROOT_KEY_PATH), "damaged").unwrap();
        let day = Place::Day {
            day: "20261111".to_owned(),
        };
        assert_eq!(seal(journal.path(), &day), None);
        let foreign = seal(journal_with_root("0f").path(), &day).unwrap();
        assert_eq!(open(journal.path(), &foreign), Err(OpenError::KeyUnusable));
        assert_eq!(
            std::fs::read_to_string(journal.path().join(ROOT_KEY_PATH)).unwrap(),
            "damaged"
        );
    }

    #[test]
    fn a_part_that_cant_be_written_in_a_link_seals_nothing() {
        let journal = journal_with_root("0f");
        for part in ["", ".", "..", "a/b", "a b", "caf\u{e9}", "a~b"] {
            assert_eq!(
                seal(
                    journal.path(),
                    &Place::Person {
                        id: part.to_owned()
                    }
                ),
                None,
                "{part:?}"
            );
        }
        let long = Place::Person {
            id: "a".repeat(MAX_SEALED_CHARS),
        };
        assert_eq!(seal(journal.path(), &long), None);
    }

    /// A place this build doesn't know, sealed by this journal's own key: a
    /// later build's link opened after a downgrade.
    #[test]
    fn a_kind_this_build_does_not_know_is_told_apart_from_a_foreign_link() {
        let journal = journal_with_root("0f");
        let key = seal_key(journal.path(), WhenAbsent::Refuse).unwrap();
        for plaintext in ["z/20261111", "d/20261111/extra", "d/../x", "d"] {
            let mut sealed = vec![VERSION];
            sealed.extend(
                Aes256Siv::new(&key.into())
                    .encrypt([LINK_LABEL, &[VERSION]], plaintext.as_bytes())
                    .unwrap(),
            );
            assert_eq!(
                open(journal.path(), &URL_SAFE_NO_PAD.encode(sealed)),
                Err(OpenError::UnknownPlace),
                "{plaintext}"
            );
        }
    }
}
