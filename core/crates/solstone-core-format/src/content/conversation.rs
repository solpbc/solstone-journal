// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Format evidence for a generic text import's conversation file.

use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationMedium {
    Written,
    Spoken,
    Unresolved,
}

/// Read the verdict bound to this conversation's header. Older generic text
/// imports stored it beside the file. Only accept that copy for the same id.
/// This describes this imported source, never a sibling in the segment.
pub fn imported_text_medium(header: &Value, sidecar: Option<&Value>) -> ConversationMedium {
    use ConversationMedium::Unresolved;
    let imported = &header["imported"];
    let Some(id) = imported["id"].as_str().filter(|id| !id.trim().is_empty()) else {
        return Unresolved;
    };
    let legacy = sidecar.filter(|metadata| metadata["import_id"].as_str() == Some(id));
    if let Some(embedded) = imported.get("triage") {
        let medium = verdict_medium(embedded);
        if legacy.is_some_and(|legacy| {
            ["kind", "ruleset_version", "reasons"]
                .iter()
                .any(|key| legacy.get(key) != embedded.get(key))
        }) {
            return Unresolved;
        }
        return medium;
    }
    legacy.map(verdict_medium).unwrap_or(Unresolved)
}

fn verdict_medium(verdict: &Value) -> ConversationMedium {
    use ConversationMedium::{Spoken, Unresolved, Written};
    let Some(version @ 1..=7) = verdict["ruleset_version"].as_u64() else {
        return Unresolved;
    };
    let Some(reasons) = verdict["reasons"].as_array() else {
        return Unresolved;
    };
    if verdict["kind"] != "transcripts" {
        return Unresolved;
    }
    let mut medium = Unresolved;
    for reason in reasons {
        let next = match reason.as_str() {
            Some(
                "webvtt_cues" | "subrip_cues" | "whisper_tsv" | "whisper_json" | "childes_chat"
                | "bracketed_offsets",
            ) => Spoken,
            Some("otter_export") if version >= 3 => Spoken,
            Some("macwhisper_stacked") if version >= 5 => Spoken,
            Some("dated_bold_speaker") if version >= 5 => continue,
            Some(
                "zoom_chat" | "discord_export" | "signal_sigtop_long" | "sms_backup_xml"
                | "slack_text",
            ) => Written,
            // Other rules describe shapes shared by more than one medium.
            // They contribute no positive evidence. Ignore known generic
            // shapes alongside a specific format; unfamiliar reasons fail closed.
            Some(
                "bracketed_speaker"
                | "bare_speaker"
                | "role_labels"
                | "role_headers"
                | "bold_speaker"
                | "clock_dash_speaker"
                | "stacked_speaker_time"
                | "clock_then_speaker"
                | "speaker_parenthesized_clock"
                | "existing_timed_layout"
                | "dated_tab_nick"
                | "signal_dated"
                | "signal_sigtop_short"
                | "irc_angle_nick",
            ) => continue,
            _ => return Unresolved,
        };
        if medium != Unresolved && medium != next {
            return Unresolved;
        }
        medium = next;
    }
    medium
}

#[cfg(test)]
mod tests {
    use super::ConversationMedium::{Spoken, Unresolved, Written};
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_proof_belongs_to_the_same_import_and_version() {
        let header = json!({"imported":{"id":"first"}});
        let mut sidecar = json!({"import_id":"first","kind":"transcripts","ruleset_version":7,
            "reasons":["bracketed_speaker","zoom_chat"]});
        assert_eq!(imported_text_medium(&header, Some(&sidecar)), Written);
        sidecar["import_id"] = json!("neighbor");
        assert_eq!(imported_text_medium(&header, Some(&sidecar)), Unresolved);
        sidecar["import_id"] = json!("first");
        sidecar["ruleset_version"] = json!(8);
        assert_eq!(imported_text_medium(&header, Some(&sidecar)), Unresolved);
        assert_eq!(imported_text_medium(&header, None), Unresolved);
    }

    #[test]
    fn a_reason_must_exist_in_the_recorded_ruleset() {
        for (reason, version, expected) in [
            ("macwhisper_stacked", 4, Unresolved),
            ("macwhisper_stacked", 5, Spoken),
            ("dated_bold_speaker", 4, Unresolved),
            ("dated_bold_speaker", 5, Written),
        ] {
            let reasons = if reason == "dated_bold_speaker" {
                json!(["zoom_chat", reason])
            } else {
                json!([reason])
            };
            let header = json!({"imported":{"id":"one","triage":{"ruleset_version":version, "kind":"transcripts","reasons":reasons}}});
            assert_eq!(imported_text_medium(&header, None), expected);
        }
    }

    #[test]
    fn same_medium_does_not_reconcile_two_different_source_verdicts() {
        let header = json!({"imported":{"id":"one","triage":{"kind":"transcripts","ruleset_version":7,"reasons":["zoom_chat"]}}});
        let mut sidecar = header["imported"]["triage"].clone();
        sidecar["import_id"] = json!("one");
        assert_eq!(imported_text_medium(&header, Some(&sidecar)), Written);
        sidecar["reasons"] = json!(["discord_export"]);
        assert_eq!(imported_text_medium(&header, Some(&sidecar)), Unresolved);
        sidecar["reasons"] = json!(["zoom_chat"]);
        sidecar["ruleset_version"] = json!(6);
        assert_eq!(imported_text_medium(&header, Some(&sidecar)), Unresolved);
    }

    #[test]
    fn corrupted_or_conflicting_evidence_cannot_establish_one_medium() {
        let mut proof = json!({"kind":"transcripts","ruleset_version":7,"reasons":["webvtt_cues"]});
        let mut header = json!({"imported":{"id":"one","triage":proof}});
        assert_eq!(imported_text_medium(&header, None), Spoken);
        proof["import_id"] = json!("one");
        proof["reasons"] = json!(["zoom_chat"]);
        assert_eq!(imported_text_medium(&header, Some(&proof)), Unresolved);
        for reasons in [
            json!(["zoom_chat", "subrip_cues"]),
            json!(["bare_speaker"]),
            json!(["webvtt_cues", null]),
            json!([]),
        ] {
            header["imported"]["triage"]["reasons"] = reasons;
            assert_eq!(imported_text_medium(&header, None), Unresolved);
        }
        header["imported"]["triage"] = Value::Null;
        assert_eq!(imported_text_medium(&header, Some(&proof)), Unresolved);
    }
}
