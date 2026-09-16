// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use solstone_core_retention::{RawReleaseClass, recorded_original_deletions};

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub(crate) struct MediaRemoval {
    pub(crate) audio: Option<String>,
    pub(crate) screen: Option<String>,
}

pub(crate) fn compute_media_removal(
    dir: &Path,
    purged: &BTreeMap<String, bool>,
    missing_referenced_files: &BTreeMap<String, BTreeSet<String>>,
) -> MediaRemoval {
    let audio_purged = purged.get("audio").copied().unwrap_or(false);
    let screen_purged = purged.get("screen").copied().unwrap_or(false);

    if !audio_purged && !screen_purged {
        return MediaRemoval::default();
    }

    let recorded = recorded_original_deletions(dir);

    let audio = if audio_purged {
        Some(reason_for_modality(
            "audio",
            missing_referenced_files.get("audio"),
            &recorded,
        ))
    } else {
        None
    };

    let screen = if screen_purged {
        Some(reason_for_modality(
            "screen",
            missing_referenced_files.get("screen"),
            &recorded,
        ))
    } else {
        None
    };

    MediaRemoval { audio, screen }
}

fn reason_for_modality(
    modality: &str,
    missing: Option<&BTreeSet<String>>,
    recorded: &BTreeMap<String, RawReleaseClass>,
) -> String {
    let what = match modality {
        "audio" => "this segment's original audio",
        "screen" => "this segment's original screen media",
        _ => return String::new(),
    };

    let Some(missing) = missing else {
        return format!("{what} is no longer in your journal");
    };

    if missing.is_empty() {
        return format!("{what} is no longer in your journal");
    }

    let mut classes = BTreeSet::new();
    for name in missing {
        if name.contains('/') || name.contains('\\') {
            return format!("{what} is no longer in your journal");
        }
        match recorded.get(name) {
            Some(class) => {
                classes.insert(*class);
            }
            None => {
                return format!("{what} is no longer in your journal");
            }
        }
    }

    let collected: Vec<_> = classes.iter().copied().collect();
    match collected.as_slice() {
        [RawReleaseClass::Policy] => {
            format!("you deleted {what} after your retention settings marked it")
        }
        [RawReleaseClass::Offload] => {
            format!("you deleted {what} after your backup copied it")
        }
        [RawReleaseClass::Owner] => {
            format!("you deleted {what}")
        }
        _ => format!("{what} is no longer in your journal"),
    }
}

#[cfg(test)]
mod tests {
    use solstone_core_retention::marks::{Approval, RemovalClass};

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
