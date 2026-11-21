// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Format evidence for a generic text import's conversation file.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use serde_json::Value;

use super::{AdmittedCategory, IMPORT_KIND_FILENAME};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationMedium {
    Written,
    Spoken,
    Unresolved,
}

impl ConversationMedium {
    pub fn required_permissions(self) -> &'static [AdmittedCategory] {
        match self {
            Self::Written => &[AdmittedCategory::Messages],
            Self::Spoken => &[AdmittedCategory::Transcripts],
            Self::Unresolved => &[AdmittedCategory::Messages, AdmittedCategory::Transcripts],
        }
    }
}

/// Markdown written before the importer recorded a conversation's format.
pub fn unresolved_markdown(relative: &str) -> bool {
    let Some((parent, name)) = relative.rsplit_once('/') else {
        return false;
    };
    if name != "imported.md" && !name.ends_with("_transcript.md") {
        return false;
    }
    if matches!(
        name,
        "note_transcript.md"
            | "document_transcript.md"
            | "event_transcript.md"
            | "image_transcript.md"
            | "screenshot_transcript.md"
    ) {
        return false;
    }
    let stream = parent.rsplit('/').nth(1);
    // Older speech importers wrote generic Markdown. Other source names
    // alone cannot establish what a residual file contains.
    if matches!(
        stream,
        Some("import.plaud" | "import.limitless" | "import.granola" | "import.audio")
    ) || stream.is_some_and(|s| s.starts_with(super::granola::STREAM_PREFIX))
    {
        return false;
    }
    super::file_spec(relative).is_some_and(|spec| {
        let generic =
            spec.pattern.ends_with("/*_transcript.md") || spec.pattern.ends_with("/imported.md");
        let transcript = matches!(
            spec.kind,
            super::KindAssignment::Settled(super::Kind::Transcripts)
                | super::KindAssignment::Legacy(super::Kind::Transcripts)
        );
        // The prior text importer assigned its wildcard row to Notes, even
        // for residual dialogue. Exact note and document rows precede it.
        generic && (transcript || stream == Some("import.text"))
    })
}

/// Resolve a conversation already offered by the segment reader. The caller
/// checks ordinary scope and the file's eligibility before using this result.
pub fn file_medium(path: &Path, relative: &str) -> Option<ConversationMedium> {
    use ConversationMedium::{Spoken, Unresolved, Written};
    if unresolved_markdown(relative) {
        return Some(Unresolved);
    }
    let mut parts = relative.rsplit('/');
    let name = parts.next()?;
    let _segment = parts.next()?;
    let stream = parts.next().unwrap_or_default();
    let native_chat = matches!(stream, "import.chatgpt" | "import.claude" | "import.gemini");
    if name == "imported_audio.jsonl" && !native_chat {
        return None;
    }
    if !matches!(
        name,
        "conversation_transcript.jsonl" | "imported_audio.jsonl"
    ) {
        return None;
    }
    if solstone_core_processing_record::is_transcribe_output(path) {
        return Some(Spoken);
    }
    if native_chat {
        return Some(Written);
    }
    let Some(header) = solstone_core_processing_record::read_jsonl_header(path) else {
        return Some(Unresolved);
    };
    if stream == "import.granola"
        && header["imported"]["id"]
            .as_str()
            .and_then(super::granola::legacy_id)
            .is_some()
        && header.get("start").is_none()
    {
        return Some(Spoken);
    }
    if stream.starts_with(super::granola::STREAM_PREFIX)
        && header[super::granola::HEADER_KEY]["id"].is_string()
        && header["granola_start_ms"].is_i64()
    {
        return Some(Spoken);
    }
    if header.get("start").is_some()
        || !(stream == "import.text" || header["imported"]["triage"].is_object())
    {
        return Some(Unresolved);
    }
    let sidecar = path.with_file_name(IMPORT_KIND_FILENAME);
    match sidecar.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if header["imported"].get("triage").is_some() {
                Some(imported_text_medium(&header, None))
            } else {
                Some(retained_original_medium(path, &header).unwrap_or(Unresolved))
            }
        }
        Ok(metadata) if metadata.is_file() => {
            let legacy = solstone_core_journal_io::read_regular_file_capped(sidecar, 64 * 1024)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            Some(legacy.as_ref().map_or(Unresolved, |metadata| {
                imported_text_medium(&header, Some(metadata))
            }))
        }
        _ => Some(Unresolved),
    }
}

// Before triage the header carried an import id and raw locator. Treat the
// locator only as an identity check; all reads are bound under the journal.
fn retained_original_medium(path: &Path, header: &Value) -> Option<ConversationMedium> {
    use sha2::{Digest, Sha256};
    use solstone_core_journal_io::{JournalRoot, read_relative_file_bounded};

    let chronicle = path
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|n| n == "chronicle"))?;
    let journal = chronicle.parent()?;
    let root = JournalRoot::open(journal).ok()?;
    let id = header["imported"]["id"].as_str()?;
    let raw = header["raw"].as_str()?.strip_prefix("../../../imports/")?;
    let (directory, name) = raw.split_once('/')?;
    // Do not reinterpret a separator or a platform-specific path in an archive.
    let component =
        |s: &str| !s.is_empty() && s != "." && s != ".." && !s.contains(['/', '\\', ':']);
    if directory != id || !component(id) || !component(name) {
        return None;
    }
    let base = Path::new("imports").join(id);
    let manifest =
        read_relative_file_bounded(&root, &base.join("manifest.json"), 1024 * 1024).ok()??;
    let meta: Value = serde_json::from_slice(&manifest.bytes).ok()?;
    let relative = path.strip_prefix(journal).ok()?;
    let created = meta["files_created"].as_array()?;
    if meta["import_id"].as_str() != Some(id)
        || meta["source_type"] != "text"
        || !created
            .iter()
            .filter_map(Value::as_str)
            .any(|entry| Path::new(entry).ends_with(relative))
    {
        return None;
    }
    // This is a fallback for reading old imports; a very large original stays
    // unresolved until newer evidence is available. Its stored turns survive.
    let original = read_relative_file_bounded(&root, &base.join(name), 1024 * 1024).ok()??;
    if meta["source_hash"].as_str()? != format!("{:x}", Sha256::digest(&original.bytes)) {
        return None;
    }
    let text = std::str::from_utf8(&original.bytes)
        .ok()?
        .trim_start_matches('\u{feff}');
    let lines: Vec<_> = text.lines().collect();
    if subtitle(&lines, true) || subtitle(&lines, false) {
        return Some(ConversationMedium::Spoken);
    }
    static ZOOM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(ZOOM_CHAT_LINE).expect("Zoom chat pattern"));
    let mut rows = lines.iter().filter(|line| !line.trim().is_empty());
    if rows.clone().take(2).count() == 2 && rows.all(|line| ZOOM.is_match(line)) {
        return Some(ConversationMedium::Written);
    }
    None
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
        if legacy.is_some_and(|legacy| verdict_medium(legacy) != medium) {
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
            Some("otter_export" | "macwhisper_stacked") if version >= 3 => Spoken,
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
                | "irc_angle_nick"
                | "dated_bold_speaker",
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

/// The saved-chat row header, also used by text import triage.
pub const ZOOM_CHAT_LINE: &str = r"^[0-9]{2}:[0-9]{2}:[0-9]{2}[ \t]+From (?P<speaker>[^:]{1,100}?)(?:\s+to\s+[^:]{1,100}?)?\s*[:：](?:\s|$)";

/// Recognize caption timing and payload blocks from a complete source.
pub fn subtitle(lines: &[&str], vtt: bool) -> bool {
    static VTT_TIME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:[0-9]{2,}:)?[0-5][0-9]:[0-5][0-9]\.[0-9]{3}[ \t]+-->[ \t]+(?:[0-9]{2,}:)?[0-5][0-9]:[0-5][0-9]\.[0-9]{3}(?:[ \t]+.*)?$").unwrap()
    });
    static SRT_TIME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^[0-9]{2,}:[0-5][0-9]:[0-5][0-9][,.][0-9]{3}[ \t]+-->[ \t]+[0-9]{2,}:[0-5][0-9]:[0-5][0-9][,.][0-9]{3}(?:[ \t]+.*)?$").unwrap()
    });
    let lines = if vtt {
        let Some((header, rest)) = lines.split_first() else {
            return false;
        };
        if !(*header == "WEBVTT" || header.starts_with("WEBVTT ") || header.starts_with("WEBVTT\t"))
        {
            return false;
        }
        // The header and any header text end at the first blank line.
        let Some(split) = rest.iter().position(|line| line.trim().is_empty()) else {
            return false;
        };
        &rest[split + 1..]
    } else {
        lines
    };
    let mut cues = 0usize;
    let mut malformed_cues = 0usize;
    for block in lines
        .split(|line| line.trim().is_empty())
        .filter(|block| !block.is_empty())
    {
        if vtt && (matches!(block[0], "STYLE" | "REGION" | "NOTE") || block[0].starts_with("NOTE "))
        {
            continue;
        }
        let timing = if vtt { &*VTT_TIME } else { &*SRT_TIME };
        let index = usize::from(!vtt || !timing.is_match(block[0]));
        if !vtt && (block[0].is_empty() || !block[0].bytes().all(|byte| byte.is_ascii_digit())) {
            return false;
        }
        if block.len() <= index + 1 || !timing.is_match(block[index]) {
            if vtt
                && (block[0].contains("-->")
                    || block.get(1).is_some_and(|line| line.contains("-->")))
            {
                malformed_cues += 1;
                continue;
            }
            return false;
        }
        cues += 1;
    }
    if vtt {
        cues >= 1 && (malformed_cues == 0 || cues >= 4 && malformed_cues * 4 <= cues)
    } else {
        cues >= 2
    }
}

#[cfg(test)]
mod tests {
    use super::ConversationMedium::{Spoken, Unresolved, Written};
    use super::*;
    use serde_json::json;

    #[test]
    fn residual_markdown_respects_specific_kind_assignments() {
        for (stream, name, unresolved) in [
            ("import.text", "note_transcript.md", false),
            ("import.text", "saved_transcript.md", true),
            ("import.kindle", "highlight_transcript.md", false),
            ("import.apple_health", "activity_transcript.md", false),
            ("import.audio", "imported.md", false),
            ("import.unknown", "imported.md", true),
            ("import.document", "unknown_transcript.md", true),
        ] {
            let path = format!("20260914/{stream}/090000_300/{name}");
            assert_eq!(unresolved_markdown(&path), unresolved, "{path}");
        }
    }

    #[test]
    fn retained_original_must_match_the_import_file_record_and_digest() {
        use sha2::{Digest, Sha256};
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let relative = "20260914/import.text/090000_300/conversation_transcript.jsonl";
        let path = tmp.path().join("chronicle").join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "{\"imported\":{\"id\":\"one\"},\"raw\":\"../../../imports/one/meeting.txt\"}\n",
        )
        .unwrap();
        let base = tmp.path().join("imports/one");
        fs::create_dir_all(&base).unwrap();
        let raw = base.join("meeting.txt");
        for (input, expected) in [
            ("WEBVTT\n\n00:01.000 --> 00:03.000\nA bell rings.\n", Spoken),
            (
                "10:07:10 From Ivo : Hold it.\n10:07:12 From Pia to Everyone : Holding.\n",
                Written,
            ),
            (
                "[9:01] Lio: Ready?\n[9:02] Val: Yes.\n[9:03] Lio: Go.\n",
                Unresolved,
            ),
        ] {
            fs::write(&raw, input).unwrap();
            let mut manifest = json!({"import_id":"one","source_type":"text",
                "source_hash":format!("{:x}",Sha256::digest(input.as_bytes())),
                "files_created":[path]});
            let m = base.join("manifest.json");
            fs::write(&m, manifest.to_string()).unwrap();
            assert_eq!(file_medium(&path, relative), Some(expected));
            fs::write(&raw, format!("{input}changed")).unwrap();
            assert_eq!(file_medium(&path, relative), Some(Unresolved));
            fs::write(&raw, input).unwrap();
            manifest["files_created"] = json!(["another-segment"]);
            fs::write(&m, manifest.to_string()).unwrap();
            assert_eq!(file_medium(&path, relative), Some(Unresolved));
            fs::remove_file(raw.as_path()).unwrap();
            assert_eq!(file_medium(&path, relative), Some(Unresolved));
        }
    }

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
