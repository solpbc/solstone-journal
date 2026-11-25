// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use solstone_core_convey_http::window_catalog::{strings, text};
use solstone_core_retention::{RawReleaseClass, recorded_original_deletions};

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub(crate) struct MediaRemoval {
    pub(crate) audio: Option<String>,
    pub(crate) screen: Option<String>,
    /// A trash item holds this modality's missing original: recoverable.
    pub(crate) audio_in_trash: bool,
    pub(crate) screen_in_trash: bool,
    /// Expected original file is absent with no deletion record and not in trash.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) audio_missing_unrecorded: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) screen_missing_unrecorded: bool,
}

/// Why a segment's original media is missing. `in_trash` says whether a
/// trash item holds a missing original, by its bare name.
pub(crate) fn compute_media_removal(
    locale: &str,
    dir: &Path,
    purged: &BTreeMap<String, bool>,
    missing_referenced_files: &BTreeMap<String, BTreeSet<String>>,
    in_trash: &dyn Fn(&str) -> bool,
) -> MediaRemoval {
    let audio_purged = purged.get("audio").copied().unwrap_or(false);
    let screen_purged = purged.get("screen").copied().unwrap_or(false);

    if !audio_purged && !screen_purged {
        return MediaRemoval::default();
    }

    let recorded = recorded_original_deletions(dir);
    let trashed = |modality: &str| {
        missing_referenced_files
            .get(modality)
            .is_some_and(|missing| missing.iter().any(|name| in_trash(name)))
    };
    let (audio_in_trash, screen_in_trash) = (
        audio_purged && trashed("audio"),
        screen_purged && trashed("screen"),
    );
    let reason = |modality: &str, in_trash: bool| -> (String, bool) {
        if in_trash {
            (in_trash_reason(locale, modality), false)
        } else {
            reason_for_modality(
                locale,
                modality,
                missing_referenced_files.get(modality),
                &recorded,
            )
        }
    };

    let (audio, audio_missing_unrecorded) = if audio_purged {
        let (text, unrecorded) = reason("audio", audio_in_trash);
        (Some(text), unrecorded)
    } else {
        (None, false)
    };
    let (screen, screen_missing_unrecorded) = if screen_purged {
        let (text, unrecorded) = reason("screen", screen_in_trash);
        (Some(text), unrecorded)
    } else {
        (None, false)
    };

    MediaRemoval {
        audio,
        screen,
        audio_in_trash,
        screen_in_trash,
        audio_missing_unrecorded,
        screen_missing_unrecorded,
    }
}

/// A missing original a trash item holds, which the owner can still recover.
fn in_trash_reason(locale: &str, modality: &str) -> String {
    match modality {
        "audio" => text(
            locale,
            "journal.window.transcripts.media.original.audio.waiting",
        ),
        _ => text(
            locale,
            "journal.window.transcripts.media.original.screen.waiting",
        ),
    }
}

fn missing_unrecorded_reason(locale: &str, modality: &str) -> String {
    match modality {
        "audio" => text(
            locale,
            "journal.window.transcripts.media.removal.missing.unrecorded.audio",
        ),
        _ => text(
            locale,
            "journal.window.transcripts.media.removal.missing.unrecorded.screen",
        ),
    }
}

fn reason_for_modality(
    locale: &str,
    modality: &str,
    missing: Option<&BTreeSet<String>>,
    recorded: &BTreeMap<String, RawReleaseClass>,
) -> (String, bool) {
    let what = match modality {
        "audio" => text(locale, "journal.window.transcripts.media.original.audio"),
        "screen" => text(locale, "journal.window.transcripts.media.original.screen"),
        _ => return (String::new(), false),
    };

    let Some(missing) = missing else {
        return (
            strings(
                locale,
                "journal.window.transcripts.media.removal.unavailable",
                [("what", &what)],
            ),
            false,
        );
    };

    if missing.is_empty()
        || missing
            .iter()
            .any(|name| name.contains('/') || name.contains('\\'))
    {
        return (
            strings(
                locale,
                "journal.window.transcripts.media.removal.unavailable",
                [("what", &what)],
            ),
            false,
        );
    }

    let mut classes = BTreeSet::new();
    for name in missing {
        match recorded.get(name) {
            Some(class) => {
                classes.insert(*class);
            }
            None => {
                return (missing_unrecorded_reason(locale, modality), true);
            }
        }
    }

    let collected: Vec<_> = classes.iter().copied().collect();
    let text = match collected.as_slice() {
        [RawReleaseClass::Policy] => strings(
            locale,
            "journal.window.transcripts.media.removal.policy",
            [("what", &what)],
        ),
        [RawReleaseClass::Offload] => strings(
            locale,
            "journal.window.transcripts.media.removal.offload",
            [("what", &what)],
        ),
        [RawReleaseClass::Owner] => strings(
            locale,
            "journal.window.transcripts.media.removal.owner",
            [("what", &what)],
        ),
        [RawReleaseClass::OwnerVoice] => strings(
            locale,
            "journal.window.transcripts.media.removal.owner.voice",
            [("what", &what)],
        ),
        [RawReleaseClass::Import] if modality == "audio" => {
            text(locale, "journal.window.transcripts.media.removal.import")
        }
        _ => strings(
            locale,
            "journal.window.transcripts.media.removal.unavailable",
            [("what", &what)],
        ),
    };
    (text, false)
}

#[cfg(test)]
mod tests {
    use solstone_core_convey_http::window_catalog::{ENGLISH, strings, text};
    use solstone_core_retention::marks::{Approval, RemovalClass};

    #[test]
    fn owner_voice_release_and_unrecorded_absence_route_distinctly() {
        use solstone_core_retention::RawReleaseClass;
        use std::collections::{BTreeMap, BTreeSet};

        let missing: BTreeSet<String> = ["120000_300.m4a".to_owned()].into();
        let reason = |classes: &[(&str, RawReleaseClass)]| {
            let recorded = classes
                .iter()
                .map(|(name, class)| ((*name).to_owned(), *class))
                .collect::<BTreeMap<_, _>>();
            super::reason_for_modality(ENGLISH, "audio", Some(&missing), &recorded)
        };
        let (unrecorded, is_unrecorded) = reason(&[]);
        let (owner, owner_unrecorded) = reason(&[("120000_300.m4a", RawReleaseClass::Owner)]);
        let (owner_voice, voice_unrecorded) =
            reason(&[("120000_300.m4a", RawReleaseClass::OwnerVoice)]);
        assert!(is_unrecorded);
        assert!(!owner_unrecorded);
        assert!(!voice_unrecorded);
        assert_eq!(
            unrecorded,
            text(
                ENGLISH,
                "journal.window.transcripts.media.removal.missing.unrecorded.audio"
            )
        );
        assert_ne!(owner_voice, unrecorded);
        assert_ne!(owner_voice, owner);

        // Mixed recorded release classes stay on generic unavailable, never claiming unrecorded absence.
        let both: BTreeSet<String> = ["a.m4a".to_owned(), "b.m4a".to_owned()].into();
        let recorded = [
            ("a.m4a".to_owned(), RawReleaseClass::OwnerVoice),
            ("b.m4a".to_owned(), RawReleaseClass::Owner),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let (mixed, mixed_unrecorded) =
            super::reason_for_modality(ENGLISH, "audio", Some(&both), &recorded);
        let what_audio = text(ENGLISH, "journal.window.transcripts.media.original.audio");
        let unavailable_audio = strings(
            ENGLISH,
            "journal.window.transcripts.media.removal.unavailable",
            [("what", &what_audio)],
        );
        assert!(!mixed_unrecorded);
        assert_eq!(mixed, unavailable_audio);

        // Non-audio Import release class stays on generic unavailable.
        let screen_missing: BTreeSet<String> = ["screen.webm".to_owned()].into();
        let screen_recorded = [("screen.webm".to_owned(), RawReleaseClass::Import)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let (screen_import, screen_import_unrecorded) =
            super::reason_for_modality(ENGLISH, "screen", Some(&screen_missing), &screen_recorded);
        let what_screen = text(ENGLISH, "journal.window.transcripts.media.original.screen");
        let unavailable_screen = strings(
            ENGLISH,
            "journal.window.transcripts.media.removal.unavailable",
            [("what", &what_screen)],
        );
        assert!(!screen_import_unrecorded);
        assert_eq!(screen_import, unavailable_screen);

        // Unrecorded screen absence routes to the screen unrecorded key.
        let (screen_missing_reason, screen_unrecorded) =
            super::reason_for_modality(ENGLISH, "screen", Some(&screen_missing), &BTreeMap::new());
        assert!(screen_unrecorded);
        assert_eq!(
            screen_missing_reason,
            text(
                ENGLISH,
                "journal.window.transcripts.media.removal.missing.unrecorded.screen"
            )
        );
    }

    #[test]
    fn policy_and_offload_raw_release_require_approval_copy_premise() {
        let (_, policy_approval, _) = RemovalClass::PolicyRawRelease.axes();
        let (_, offload_approval, _) = RemovalClass::OffloadRawRelease.axes();
        assert_eq!(
            policy_approval,
            Approval::Required,
            "transcripts-web copy assumes you deleted original media marked by retention policy; update transcripts sentences if approval requirement changes"
        );
        assert_eq!(
            offload_approval,
            Approval::Required,
            "transcripts-web copy assumes you deleted original media copied by backup offload; update transcripts sentences if approval requirement changes"
        );
    }
}
