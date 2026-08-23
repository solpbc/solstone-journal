// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_entity::ambiguity_id;
use solstone_core_speaker_resolve::evidence::{
    compute_segment_candidate_evidence_readonly, extract_meeting_participants_with_gaps,
    extract_screen_participants_with_gaps, load_segment_speakers_with_gaps,
    load_setting_field_with_gaps, parse_setting_names,
};

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-speaker-id-evidence-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temporary journal");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn segment(temporary: &TempDir) -> PathBuf {
    let path = temporary.path().join("chronicle/20260808/mic/120000_300");
    fs::create_dir_all(path.join("talents")).expect("create segment");
    path
}

fn write_entity(root: &Path, id: &str, name: &str) {
    write_entity_typed(root, id, name, Some("Person"), false);
}

fn write_entity_typed(root: &Path, id: &str, name: &str, entity_type: Option<&str>, blocked: bool) {
    let path = root.join("entities").join(id).join("entity.json");
    fs::create_dir_all(path.parent().expect("entity parent")).expect("create entity parent");
    let mut value = json!({"id": id, "name": name});
    let object = value.as_object_mut().expect("entity object");
    if let Some(entity_type) = entity_type {
        object.insert("type".to_owned(), Value::String(entity_type.to_owned()));
    }
    if blocked {
        object.insert("blocked".to_owned(), Value::Bool(true));
    }
    fs::write(path, value.to_string()).expect("write entity");
}

fn write_resolved_choice(root: &Path, query: &str, entity_id: &str) {
    let normalized = solstone_core_entity_matching::normalize_resolution_query(query);
    let row = json!({
        "schema_version": 1,
        "ambiguity_id": ambiguity_id(&format!("journal|{normalized}")),
        "scope": {"kind": "journal"},
        "normalized_query": normalized,
        "original_query": query,
        "latest_query": query,
        "first_seen": "2026-08-01T00:00:00Z",
        "last_seen": "2026-08-01T00:00:00Z",
        "observed_tier": 8,
        "status": "resolved",
        "resolved_entity_id": entity_id,
        "resolved_at": "2026-08-01T00:00:00Z",
        "ranked_candidates": [{
            "id": entity_id,
            "name": query,
            "tier": 8,
            "score": 90.0
        }],
        "origins": [{"lane": "test"}],
        "origin_keys": ["test"],
        "occurrence_count": 1,
        "audit": {"prior_choices": []}
    });
    let path = root.join("entities/ambiguities.jsonl");
    fs::create_dir_all(path.parent().expect("ambiguities parent")).expect("create entities");
    fs::write(&path, format!("{row}\n")).expect("write resolved choice");
}

fn evidence_for_speakers(
    temporary: &TempDir,
    speakers: &str,
) -> (
    Vec<solstone_core_speaker_resolve::evidence::CandidateEvidence>,
    Vec<solstone_core_speaker_resolve::evidence::EvidenceGap>,
) {
    let segment = segment(temporary);
    fs::write(segment.join("talents/speakers.json"), speakers).expect("write speakers");
    compute_segment_candidate_evidence_readonly(temporary.path(), "20260808", "mic", "120000_300")
        .expect("compute evidence")
}

#[test]
fn ac1_setting_and_meeting_distinguish_missing_empty_and_invalid_sources() {
    let temporary = TempDir::new();
    let segment = segment(&temporary);
    assert_eq!(load_setting_field_with_gaps(&segment), (None, vec![]));
    assert_eq!(
        extract_meeting_participants_with_gaps(temporary.path(), "20260808"),
        (Vec::new(), vec![])
    );

    fs::write(segment.join("imported_audio.jsonl"), b"\n").expect("write empty setting");
    assert_eq!(load_setting_field_with_gaps(&segment), (None, vec![]));
    fs::write(segment.join("imported_audio.jsonl"), b"{").expect("write malformed setting");
    assert_eq!(
        load_setting_field_with_gaps(&segment).1[0].reason,
        "malformed_json"
    );

    let meetings = temporary
        .path()
        .join("chronicle/20260808/talents/meetings.md");
    fs::create_dir_all(&meetings).expect("make unreadable meetings path");
    let (_, gaps) = extract_meeting_participants_with_gaps(temporary.path(), "20260808");
    assert_eq!(gaps[0].source, "meeting_day");
    assert_eq!(gaps[0].reason, "unreadable");
}

#[test]
fn ac2_speakers_partial_data_keeps_names_and_reports_wrong_shape() {
    let temporary = TempDir::new();
    let segment = segment(&temporary);
    fs::write(
        segment.join("talents/speakers.json"),
        br#"["Alice", 4, "  "]"#,
    )
    .expect("write speakers");
    let (names, gaps) = load_segment_speakers_with_gaps(&segment);
    assert_eq!(names, ["Alice"]);
    assert_eq!(gaps[0].source, "speakers");
    assert_eq!(gaps[0].reason, "wrong_shape");
}

#[test]
fn ac3_screen_only_keeps_person_attendees() {
    let temporary = TempDir::new();
    let segment = segment(&temporary);
    fs::write(
        segment.join("talents/screen.json"),
        json!({"entities": [
            {"type": "Person", "role": "attendee", "name": " Alice "},
            {"type": "Project", "role": "attendee", "name": "Ignored"},
            {"type": "Person", "role": "host", "name": "Ignored"}
        ]})
        .to_string(),
    )
    .expect("write screen");
    let (names, gaps) = extract_screen_participants_with_gaps(&segment);
    assert_eq!(names, ["Alice"]);
    assert!(gaps.is_empty());
}

#[test]
fn ac4_setting_parser_drops_configured_owner_variants() {
    let temporary = TempDir::new();
    let config = temporary.path().join("config/journal.json");
    fs::create_dir_all(config.parent().expect("config parent")).expect("create config parent");
    fs::write(
        config,
        json!({"identity": {"preferred": "Avery", "name": "Avery Stone", "aliases": ["A. Stone"]}})
            .to_string(),
    )
    .expect("write config");
    assert_eq!(
        parse_setting_names(temporary.path(), "Avery and Jordan at coffee").expect("parse setting"),
        ["Jordan"]
    );
    assert_eq!(
        parse_setting_names(temporary.path(), "Meeting with Priya and Mateo")
            .expect("parse setting"),
        ["Priya", "Mateo"]
    );
}

#[test]
fn ac5_readonly_evidence_assembles_resolved_candidates_in_canonical_order() {
    let temporary = TempDir::new();
    let segment = segment(&temporary);
    write_entity(temporary.path(), "alice", "Alice");
    fs::write(segment.join("talents/speakers.json"), br#"["Alice"]"#).expect("write speakers");
    fs::write(
        segment.join("imported_audio.jsonl"),
        b"{\"setting\":\"Call with Alice\"}\n",
    )
    .expect("write setting");
    fs::write(
        segment.join("talents/screen.json"),
        json!({"entities": [{"type": "Person", "role": "attendee", "name": "Alice"}]}).to_string(),
    )
    .expect("write screen");

    let (evidence, gaps) = compute_segment_candidate_evidence_readonly(
        temporary.path(),
        "20260808",
        "mic",
        "120000_300",
    )
    .expect("compute evidence");
    assert!(gaps.is_empty());
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].entity_id, "alice");
    assert_eq!(evidence[0].sources, ["screen", "setting", "speakers"]);
}

#[test]
fn readonly_evidence_does_not_emit_a_written_id_slug_collision() {
    let temporary = TempDir::new();
    let segment = segment(&temporary);
    write_entity(temporary.path(), "new_person", "Someone Else");
    fs::write(segment.join("talents/speakers.json"), br#"["New Person"]"#).expect("write speakers");

    let (evidence, gaps) = compute_segment_candidate_evidence_readonly(
        temporary.path(),
        "20260808",
        "mic",
        "120000_300",
    )
    .expect("compute evidence");
    assert!(gaps.is_empty());
    assert!(evidence.is_empty());
}

#[test]
fn ac6_readonly_evidence_reports_all_five_gap_sources() {
    let temporary = TempDir::new();
    let segment = segment(&temporary);
    fs::write(segment.join("talents/speakers.json"), br#"["Alice", 1]"#).expect("write speakers");
    fs::write(segment.join("imported_audio.jsonl"), b"{").expect("write setting");
    fs::write(segment.join("talents/screen.json"), b"{").expect("write screen");
    let meetings = temporary
        .path()
        .join("chronicle/20260808/talents/meetings.md");
    fs::create_dir_all(&meetings).expect("make unreadable meetings path");
    let ambiguities = temporary.path().join("entities/ambiguities.jsonl");
    fs::create_dir_all(ambiguities.parent().expect("ambiguities parent"))
        .expect("create ambiguities parent");
    fs::write(ambiguities, b"not json\n").expect("write corrupt ambiguities");

    let (_, gaps) = compute_segment_candidate_evidence_readonly(
        temporary.path(),
        "20260808",
        "mic",
        "120000_300",
    )
    .expect("compute evidence");
    assert_eq!(
        gaps.iter()
            .map(|gap| (gap.source.as_str(), gap.reason.as_str()))
            .collect::<Vec<_>>(),
        [
            ("speakers", "wrong_shape"),
            ("setting", "malformed_json"),
            ("screen", "malformed_json"),
            ("meeting_day", "unreadable"),
            ("resolution", "stale_resolution"),
        ]
    );
}

#[test]
fn ac7_missing_segment_short_circuits_without_evidence() {
    let temporary = TempDir::new();
    assert_eq!(
        compute_segment_candidate_evidence_readonly(
            temporary.path(),
            "20260808",
            "mic",
            "120000_300",
        )
        .expect("missing segment"),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn non_person_and_unresolved_names_produce_no_candidate_evidence() {
    let temporary = TempDir::new();
    write_entity_typed(temporary.path(), "tool", "Terminal", Some("Tool"), false);
    write_entity_typed(temporary.path(), "project", "Atlas", Some("Project"), false);
    write_entity_typed(temporary.path(), "company", "Acme", Some("Company"), false);
    write_entity_typed(temporary.path(), "blocked", "Blocked", Some("Person"), true);
    write_entity_typed(temporary.path(), "untyped", "Untyped", None, false);
    write_entity_typed(
        temporary.path(),
        "sam-one",
        "Sam Person",
        Some("Person"),
        false,
    );
    write_entity_typed(
        temporary.path(),
        "sam-two",
        "Sam Person",
        Some("Person"),
        false,
    );
    let (evidence, gaps) = evidence_for_speakers(
        &temporary,
        r#"["Terminal", "Atlas", "Acme", "Blocked", "Untyped", "Sam Person", "Nobody"]"#,
    );
    assert!(evidence.is_empty());
    assert!(gaps.is_empty());
}

#[test]
fn same_name_person_and_tool_emits_the_person_as_sole_candidate() {
    let temporary = TempDir::new();
    write_entity_typed(temporary.path(), "alex", "Alex", Some("Person"), false);
    write_entity_typed(temporary.path(), "tool", "Alex", Some("Tool"), false);
    let (evidence, gaps) = evidence_for_speakers(&temporary, r#"["Alex"]"#);
    assert!(gaps.is_empty());
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].entity_id, "alex");
}

#[test]
fn saved_choice_naming_a_tool_is_silent() {
    let temporary = TempDir::new();
    write_entity_typed(temporary.path(), "tool", "Terminal", Some("Tool"), false);
    write_resolved_choice(temporary.path(), "Terminal", "tool");
    let (evidence, gaps) = evidence_for_speakers(&temporary, r#"["Terminal"]"#);
    assert!(evidence.is_empty());
    assert!(gaps.is_empty());
}

#[test]
fn saved_choice_naming_an_absent_or_blocked_entity_is_stale() {
    let absent = TempDir::new();
    write_resolved_choice(absent.path(), "Alice", "missing");
    let (evidence, gaps) = evidence_for_speakers(&absent, r#"["Alice"]"#);
    assert!(evidence.is_empty());
    assert_eq!(
        gaps.iter()
            .map(|gap| (gap.source.as_str(), gap.reason.as_str()))
            .collect::<Vec<_>>(),
        [("resolution", "stale_resolution")]
    );

    let blocked = TempDir::new();
    write_entity_typed(blocked.path(), "alice", "Alice", Some("Person"), true);
    write_resolved_choice(blocked.path(), "Alice", "alice");
    let (evidence, gaps) = evidence_for_speakers(&blocked, r#"["Alice"]"#);
    assert!(evidence.is_empty());
    assert_eq!(
        gaps.iter()
            .map(|gap| (gap.source.as_str(), gap.reason.as_str()))
            .collect::<Vec<_>>(),
        [("resolution", "stale_resolution")]
    );
}
