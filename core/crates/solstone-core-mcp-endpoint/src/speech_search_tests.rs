// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! End-to-end MCP boundary, speaker display, clock, link, and BM25 ranking tests
//! for indexed speech transcripts.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};
use solstone_core_possible_names::{Sidecar, SidecarEntry, SidecarSource, source_sha256};

use crate::dispatch::{McpProbeError, run_mcp_probe};
use crate::permissions::{
    ConnectionReadSnapshot, PermissionDecision, PermissionStore, ReadPermission, ReadScope,
};

const CONNECTION: &str = "bearer:speech-search-test";
const FACET_A: &str = "123e4567-e89b-42d3-a456-426614174010";
const FACET_B: &str = "123e4567-e89b-42d3-a456-426614174011";

fn fixture(prefix: &str) -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(crate::test_scratch())
        .unwrap();
    fs::create_dir_all(journal.path().join("config")).unwrap();
    fs::write(
        journal.path().join("config/journal.json"),
        json!({"identity": {"name": "William Shakespeare", "timezone": "UTC"}}).to_string(),
    )
    .unwrap();
    for (name, id, title) in [("alpha", FACET_A, "Alpha"), ("beta", FACET_B, "Beta")] {
        let dir = journal.path().join("facets").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("facet.json"),
            json!({"id": id, "title": title, "description": title}).to_string(),
        )
        .unwrap();
    }
    journal
}

fn segment_dir(journal: &Path, day: &str, stream: &str, seg: &str) -> PathBuf {
    let dir = journal.join("chronicle").join(day).join(stream).join(seg);
    fs::create_dir_all(dir.join("talents")).unwrap();
    dir
}

fn add_entity(journal: &Path, id: &str, name: &str, principal: bool, facet: &str) {
    let dir = journal.join("entities").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("entity.json"),
        json!({"id": id, "name": name, "type": "Person", "is_principal": principal}).to_string(),
    )
    .unwrap();
    let link = journal.join("facets").join(facet).join("entities").join(id);
    fs::create_dir_all(&link).unwrap();
    fs::write(
        link.join("entity.json"),
        json!({"entity_id": id}).to_string(),
    )
    .unwrap();
}

fn grant(journal: &Path, categories: &[&str], scope: ReadScope) {
    PermissionStore::open(journal)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: categories.iter().map(|c| (*c).to_owned()).collect(),
                scope,
            },
        )
        .unwrap();
}

fn probe(journal: &Path, tool: &str, arguments: Value) -> Result<Value, McpProbeError> {
    run_mcp_probe(journal, CONNECTION, tool, &arguments)
}

#[test]
fn speech_search_enforces_transcripts_permission_chosen_facets_and_deletion() {
    let journal = fixture("solstone-mcp-speech-boundary-");
    let alpha_dir = segment_dir(journal.path(), "20250220", "talk", "163500_720");
    fs::write(
        alpha_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(
        alpha_dir.join("audio.jsonl"),
        concat!(
            "{\"raw\":\"audio.flac\"}\n",
            "{\"start\":\"16:37:14\",\"speaker\":1,\"text\":\"false beards in the tiring house\"}\n",
        ),
    )
    .unwrap();

    let beta_dir = segment_dir(journal.path(), "20250220", "talk", "173500_720");
    fs::write(
        beta_dir.join("talents/facets.json"),
        json!([{"facet": "beta"}]).to_string(),
    )
    .unwrap();
    fs::write(
        beta_dir.join("audio.jsonl"),
        concat!(
            "{\"raw\":\"audio.flac\"}\n",
            "{\"start\":\"17:37:14\",\"speaker\":1,\"text\":\"false beards secret beta remark\"}\n",
        ),
    )
    .unwrap();

    let unassigned_dir = segment_dir(journal.path(), "20250220", "talk", "183500_720");
    fs::write(
        unassigned_dir.join("audio.jsonl"),
        concat!(
            "{\"raw\":\"audio.flac\"}\n",
            "{\"start\":\"18:37:14\",\"speaker\":1,\"text\":\"false beards unassigned remark\"}\n",
        ),
    )
    .unwrap();

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    // 1. Without transcripts permission (even with facets + entities), zero speech results leak.
    grant(
        journal.path(),
        &["facets", "entities"],
        ReadScope::WholeJournal,
    );
    let denied = probe(journal.path(), "search", json!({"query": "beards"})).unwrap();
    assert_eq!(denied["results"].as_array().unwrap().len(), 0);

    // 2. With transcripts permission limited to alpha, only alpha matches.
    grant(
        journal.path(),
        &["transcripts"],
        ReadScope::Facets {
            ids: vec![FACET_A.to_owned()],
        },
    );
    let scoped = probe(journal.path(), "search", json!({"query": "beards"})).unwrap();
    let hits = scoped["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{scoped}");
    assert_eq!(hits[0]["time"], "16:37:14");
    assert!(hits[0]["snippet"].as_str().unwrap().contains("tiring"));
    let alpha_ref = hits[0]["reference"].as_str().unwrap().to_owned();

    // 3. Removing the transcript file after indexing drops the hit live on search and fetch.
    fs::remove_file(alpha_dir.join("audio.jsonl")).unwrap();
    let after_delete = probe(journal.path(), "search", json!({"query": "beards"})).unwrap();
    assert_eq!(after_delete["results"].as_array().unwrap().len(), 0);
    assert!(probe(journal.path(), "fetch", json!({"reference": alpha_ref})).is_err());
}

#[test]
fn speech_search_and_fetch_match_get_transcript_speaker_display_clock_links_and_possible_names() {
    let journal = fixture("solstone-mcp-speech-display-");
    add_entity(
        journal.path(),
        "william_shakespeare",
        "William Shakespeare",
        true,
        "alpha",
    );
    add_entity(
        journal.path(),
        "nick_sorrell",
        "Nick Sorrell",
        false,
        "alpha",
    );
    add_entity(
        journal.path(),
        "robert_southwell",
        "Robert Southwell",
        false,
        "alpha",
    );

    let dir = segment_dir(journal.path(), "20250220", "talk", "163500_720");
    fs::write(
        dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("audio.jsonl"),
        concat!(
            "{\"raw\":\"audio.flac\"}\n",
            "{\"start\":\"16:37:14\",\"speaker\":\"nick_sorrell\",\"sentence_id\":1,\"text\":\"false beards for Fryer tonight\"}\n",
            "{\"start\":\"16:38:02\",\"speaker\":\"(the templar)\",\"sentence_id\":2,\"text\":\"benchers knew the verses by heart\"}\n",
            "{\"start\":\"16:39:05\",\"speaker\":1,\"sentence_id\":3,\"text\":\"cleared speaker fallback check\"}\n",
        ),
    )
    .unwrap();
    fs::write(
        dir.join("talents/speaker_labels.json"),
        json!({"labels": [
            {"sentence_id": 1, "speaker": "nick_sorrell", "method": "acoustic"},
            {"sentence_id": 3, "speaker": "william_shakespeare", "method": "user_assigned"}
        ]})
        .to_string(),
    )
    .unwrap();
    // Owner explicitly clears sentence 3 back to anonymous (null).
    let cleared_sha = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(b"cleared speaker fallback check"))
    };
    fs::write(
        dir.join("talents/owner_line_labels.json"),
        json!({
            "version": 1,
            "entries": [{
                "source": "audio",
                "sentence_id": 3,
                "start": "16:39:05",
                "text_sha256": cleared_sha,
                "speaker": null,
                "set_at": "2026-11-25T00:00:00Z"
            }]
        })
        .to_string(),
    )
    .unwrap();

    let sidecar = Sidecar::new(
        "test-1",
        SidecarSource {
            file: "audio.jsonl".to_owned(),
            sha256: source_sha256(&dir, "audio.jsonl").unwrap(),
        },
        "rev",
        1,
        vec![SidecarEntry {
            sentence: 1,
            start_byte: "false beards for ".len(),
            end_byte: "false beards for Fryer".len(),
            heard: "Fryer".to_owned(),
            entity_id: "robert_southwell".to_owned(),
            name: "Robert Southwell".to_owned(),
        }],
    );
    fs::write(
        dir.join("talents/possible_names.json"),
        serde_json::to_string(&sidecar).unwrap(),
    )
    .unwrap();

    // Untimed recording omits `time` and sets `untimed: true`.
    let untimed_dir = segment_dir(journal.path(), "20250221", "talk", "100000_300");
    fs::write(
        untimed_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(
        untimed_dir.join("audio.jsonl"),
        concat!(
            "{\"raw\":\"audio.flac\",\"untimed\":true}\n",
            "{\"start\":\"10:01:00\",\"speaker\":1,\"text\":\"untimed lantern rehearsal\"}\n",
        ),
    )
    .unwrap();

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    // Transcripts WITHOUT entities: human-readable voice name shows, possible_names does NOT leak.
    grant(journal.path(), &["transcripts"], ReadScope::WholeJournal);
    let beards = probe(journal.path(), "search", json!({"query": "beards"})).unwrap();
    let hit = &beards["results"].as_array().unwrap()[0];
    assert_eq!(hit["speaker"], "Nick Sorrell");
    assert_eq!(hit["time"], "16:37:14");
    assert!(!hit["snippet"].as_str().unwrap().contains("nick_sorrell"));
    assert!(
        !hit["snippet"]
            .as_str()
            .unwrap()
            .contains("Robert Southwell")
    );
    assert!(
        hit["link"]
            .as_str()
            .unwrap()
            .starts_with("https://go.solstone.app/j#")
    );
    assert!(!hit["segments"].as_array().unwrap().is_empty());

    let fetched = probe(
        journal.path(),
        "fetch",
        json!({"reference": hit["reference"]}),
    )
    .unwrap();
    assert_eq!(fetched["speaker"], "Nick Sorrell");
    assert_eq!(fetched["time"], "16:37:14");
    assert!(
        fetched["link"]
            .as_str()
            .unwrap()
            .starts_with("https://go.solstone.app/j#")
    );
    assert!(!fetched["text"].as_str().unwrap().contains("nick_sorrell"));
    assert!(
        !fetched["text"]
            .as_str()
            .unwrap()
            .contains("Robert Southwell")
    );

    // Unnamed parenthetical speaker preserved verbatim.
    let templar = probe(journal.path(), "search", json!({"query": "benchers"})).unwrap();
    let templar_hit = &templar["results"].as_array().unwrap()[0];
    assert_eq!(templar_hit["speaker"], "(the templar)");
    assert_eq!(templar_hit["time"], "16:38:02");

    // Owner-cleared line falls back to anonymous speaker, matching get_transcript.
    let cleared = probe(journal.path(), "search", json!({"query": "fallback check"})).unwrap();
    let cleared_hit = &cleared["results"].as_array().unwrap()[0];
    assert_eq!(cleared_hit["speaker"], "Speaker 1");
    assert!(!cleared_hit["snippet"].as_str().unwrap().contains("William"));

    // Untimed recording carries untimed: true and omits time.
    let untimed = probe(
        journal.path(),
        "search",
        json!({"query": "lantern rehearsal"}),
    )
    .unwrap();
    let untimed_hit = &untimed["results"].as_array().unwrap()[0];
    assert_eq!(untimed_hit["untimed"], true);
    assert!(untimed_hit.get("time").is_none());

    // Transcripts WITH entities: possible_names annotation appears on both search and fetch.
    grant(
        journal.path(),
        &["transcripts", "entities"],
        ReadScope::WholeJournal,
    );
    let beards_with_entities = probe(journal.path(), "search", json!({"query": "beards"})).unwrap();
    let speech_hit = beards_with_entities["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row.get("possible_name").is_none())
        .expect("speech line hit");
    assert!(
        speech_hit["snippet"]
            .as_str()
            .unwrap()
            .contains("Robert Southwell"),
        "{speech_hit}"
    );
    let fetched_with_entities = probe(
        journal.path(),
        "fetch",
        json!({"reference": speech_hit["reference"]}),
    )
    .unwrap();
    assert!(
        fetched_with_entities["text"]
            .as_str()
            .unwrap()
            .contains("Robert Southwell"),
        "{fetched_with_entities}"
    );
}

#[test]
fn speech_search_handles_split_audio_and_named_transcript_files() {
    let journal = fixture("solstone-mcp-speech-split-");
    let split_dir = segment_dir(journal.path(), "20250301", "talk", "110000_300");
    fs::write(
        split_dir.join("mic_audio.jsonl"),
        "{\"start\":\"11:01:02\",\"speaker\":1,\"text\":\"split microphone phrase\"}\n",
    )
    .unwrap();
    fs::write(
        split_dir.join("sys_audio.jsonl"),
        "{\"start\":\"11:02:03\",\"text\":\"split computer audio phrase\"}\n",
    )
    .unwrap();
    let named_dir = segment_dir(journal.path(), "20250301", "talk", "120000_300");
    fs::write(
        named_dir.join("session_transcript.jsonl"),
        "{\"start\":\"12:03:04\",\"speaker\":2,\"text\":\"named transcript phrase\"}\n",
    )
    .unwrap();

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant(journal.path(), &["transcripts"], ReadScope::WholeJournal);

    for (query, expected_time, expected_speaker) in [
        ("microphone phrase", "11:01:02", "Speaker 1"),
        ("computer audio phrase", "11:02:03", "Someone else"),
        ("named transcript phrase", "12:03:04", "Speaker 2"),
    ] {
        let res = probe(journal.path(), "search", json!({"query": query})).unwrap();
        let hits = res["results"].as_array().unwrap();
        assert_eq!(hits.len(), 1, "{query}: {res}");
        assert_eq!(hits[0]["time"], expected_time, "{query}");
        assert_eq!(hits[0]["speaker"], expected_speaker, "{query}");
        let fetched = probe(
            journal.path(),
            "fetch",
            json!({"reference": hits[0]["reference"]}),
        )
        .unwrap();
        assert_eq!(fetched["time"], expected_time, "{query}");
        assert_eq!(fetched["speaker"], expected_speaker, "{query}");
    }
}

#[test]
fn mcp_search_ranks_multi_term_relaxed_or_ahead_of_recency_flood_and_paginates_without_dups() {
    let journal = fixture("solstone-mcp-bm25-");
    for day in 1..=10 {
        let dir = segment_dir(
            journal.path(),
            &format!("202512{day:02}"),
            "talk",
            "180000_300",
        );
        fs::write(
            dir.join("talents/audio.md"),
            format!("December day {day} quiet supper at home.\n"),
        )
        .unwrap();
    }
    let feb_dir = segment_dir(journal.path(), "20250220", "talk", "194000_720");
    fs::write(
        feb_dir.join("audio.jsonl"),
        concat!(
            "{\"raw\":\"audio.flac\"}\n",
            "{\"start\":\"19:41:16\",\"speaker\":\"(the templar)\",\"text\":\"Southwell supper stranger verses by heart\"}\n",
        ),
    )
    .unwrap();

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant(journal.path(), &["transcripts"], ReadScope::WholeJournal);

    let full = probe(
        journal.path(),
        "search",
        json!({"query": "Southwell supper stranger quicksilver", "limit": 20}),
    )
    .unwrap();
    assert_eq!(full["relaxed"], true);
    let full_hits = full["results"].as_array().unwrap();
    assert_eq!(full_hits.len(), 11);
    assert_eq!(full_hits[0]["date"], "20250220");
    assert_eq!(full_hits[0]["time"], "19:41:16");
    assert_eq!(full_hits[0]["speaker"], "(the templar)");

    // Paginate limit=2 via opaque next_cursor and verify zero duplicates and zero skips.
    let mut seen_snippets = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = json!({"query": "Southwell supper stranger quicksilver", "limit": 2});
        if let Some(token) = &cursor {
            args["cursor"] = Value::String(token.clone());
        }
        let page = probe(journal.path(), "search", args).unwrap();
        for row in page["results"].as_array().unwrap() {
            seen_snippets.push((
                row["date"].as_str().unwrap().to_owned(),
                row["snippet"].as_str().unwrap().to_owned(),
            ));
        }
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    let expected: Vec<(String, String)> = full_hits
        .iter()
        .map(|row| {
            (
                row["date"].as_str().unwrap().to_owned(),
                row["snippet"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(seen_snippets, expected);
    let unique: BTreeSet<_> = seen_snippets.iter().collect();
    assert_eq!(unique.len(), expected.len());
}

#[test]
fn tools_list_preserves_13_tools_plain_object_root_schemas_and_annotations() {
    let journal = fixture("solstone-mcp-tools-list-");
    let snapshot = ConnectionReadSnapshot {
        categories: [
            AdmittedCategory::Transcripts,
            AdmittedCategory::Messages,
            AdmittedCategory::Facets,
            AdmittedCategory::Entities,
            AdmittedCategory::ScreenText,
            AdmittedCategory::Body,
            AdmittedCategory::WomensHealth,
            AdmittedCategory::Notes,
            AdmittedCategory::Documents,
            AdmittedCategory::Transactions,
            AdmittedCategory::Calendar,
            AdmittedCategory::Places,
            AdmittedCategory::Photos,
        ]
        .into_iter()
        .collect(),
        scope: ConnectionScope::WholeJournal,
        generation: 1,
        places_precision: Some(solstone_core_format::content::kinds::PlacesPrecision::Full),
    };
    let listed = crate::registry::advertised_tools_list(
        journal.path(),
        &PermissionDecision::Snapshot(snapshot),
    );
    let tools = listed["tools"].as_array().unwrap();
    let expected_count = if crate::suggestions::suggestions_ready() {
        13
    } else {
        12
    };
    assert_eq!(tools.len(), expected_count);
    assert_eq!(crate::registry::TOOLS.len(), 13);
    for tool in tools {
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{tool}");
        for forbidden in ["oneOf", "anyOf", "allOf"] {
            assert!(
                schema.get(forbidden).is_none(),
                "{} must keep plain-object root schema without top-level {forbidden}",
                tool["name"]
            );
        }
    }
}

#[test]
fn mcp_search_collapses_chronicle_segments_preserves_boundaries_fidelity_and_pagination() {
    let journal = fixture("solstone-mcp-segment-collapse-");
    add_entity(
        journal.path(),
        "nick_sorrell",
        "Nick Sorrell",
        false,
        "alpha",
    );
    add_entity(
        journal.path(),
        "robert_southwell",
        "Robert Southwell",
        false,
        "alpha",
    );

    // Segment 1 (alpha): audio.jsonl (2 lines), topics.md (multi-chunk), activity.md, sense.md.
    let seg1 = segment_dir(journal.path(), "20250211", "talk", "082200_480");
    fs::write(
        seg1.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(
        seg1.join("audio.jsonl"),
        concat!(
            "{\"raw\":\"audio.flac\"}\n",
            "{\"start\":\"08:22:16\",\"speaker\":\"nick_sorrell\",\"sentence_id\":1,\"text\":\"Raleigh gold in Guiana for Fryer\"}\n",
            "{\"start\":\"08:23:26\",\"speaker\":\"(the sailor)\",\"sentence_id\":2,\"text\":\"faces in their chests and gold\"}\n",
        ),
    )
    .unwrap();
    fs::write(
        seg1.join("talents/topics.md"),
        "## Topics\n\n- Raleigh city of gold faces in their chests\n\n## Quotes\n\n- Raleigh gold faces chests\n",
    )
    .unwrap();
    fs::write(
        seg1.join("talents/activity.md"),
        "Heard about Raleigh city of gold and faces in their chests.\n",
    )
    .unwrap();
    fs::write(
        seg1.join("talents/sense.md"),
        "Raleigh gold mention at the stairs.\n",
    )
    .unwrap();
    let sidecar = Sidecar::new(
        "seg1-pn",
        SidecarSource {
            file: "audio.jsonl".to_owned(),
            sha256: source_sha256(&seg1, "audio.jsonl").unwrap(),
        },
        "rev",
        1,
        vec![SidecarEntry {
            sentence: 1,
            start_byte: "Raleigh gold in Guiana for ".len(),
            end_byte: "Raleigh gold in Guiana for Fryer".len(),
            heard: "Fryer".to_owned(),
            entity_id: "robert_southwell".to_owned(),
            name: "Robert Southwell".to_owned(),
        }],
    );
    fs::write(
        seg1.join("talents/possible_names.json"),
        serde_json::to_string(&sidecar).unwrap(),
    )
    .unwrap();

    // Segment 2 (alpha): exact BM25 tie between audio.jsonl and talents/sense.md
    // (identical rendered tokens since audio.jsonl indexes `[start] speaker: text`).
    // audio.jsonl must win the tie and deliver exact clock + unnamed speaker.
    let seg2 = segment_dir(journal.path(), "20250922", "talk", "091500_840");
    fs::write(
        seg2.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(
        seg2.join("audio.jsonl"),
        concat!(
            "{\"raw\":\"audio.flac\"}\n",
            "{\"start\":\"09:15:44\",\"speaker\":\"(a clerk by the pillar)\",\"text\":\"Raleigh pillar rumour\"}\n",
        ),
    )
    .unwrap();
    fs::write(
        seg2.join("talents/sense.md"),
        "[09:15:44] (a clerk by the pillar): Raleigh pillar rumour\n",
    )
    .unwrap();

    // Segment 3 (alpha): separate conversation.
    let seg3 = segment_dir(journal.path(), "20250315", "talk", "073000_480");
    fs::write(
        seg3.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(
        seg3.join("audio.jsonl"),
        concat!(
            "{\"raw\":\"audio.flac\"}\n",
            "{\"start\":\"07:30:50\",\"speaker\":\"nick_sorrell\",\"text\":\"Raleigh sailed away\"}\n",
        ),
    )
    .unwrap();
    fs::write(seg3.join("talents/activity.md"), "Raleigh sailing note.\n").unwrap();

    // Segment 4 (beta — out of scope for alpha-only grant): must never leak.
    let seg4_beta = segment_dir(journal.path(), "20250401", "talk", "110000_300");
    fs::write(
        seg4_beta.join("talents/facets.json"),
        json!([{"facet": "beta"}]).to_string(),
    )
    .unwrap();
    fs::write(
        seg4_beta.join("audio.jsonl"),
        concat!(
            "{\"raw\":\"audio.flac\"}\n",
            "{\"start\":\"11:01:00\",\"speaker\":1,\"text\":\"Raleigh gold secret beta remark\"}\n",
        ),
    )
    .unwrap();

    // Segment 5 (hidden body day on alpha): must never leak even when body + facets are granted.
    let hidden_day = "20250402";
    let seg5_hidden = segment_dir(
        journal.path(),
        hidden_day,
        "import.apple_health",
        "080000_300",
    );
    fs::write(
        seg5_hidden.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(
        seg5_hidden.join("body.jsonl"),
        "{\"type\":\"workout\",\"activity\":\"Raleigh gold hidden body workout\"}\n",
    )
    .unwrap();
    fs::write(
        seg5_hidden.join("talents/activity.md"),
        "Raleigh gold hidden body day activity summary.\n",
    )
    .unwrap();
    fs::write(
        journal
            .path()
            .join(format!("chronicle/{hidden_day}/body.md")),
        "Raleigh gold hidden body day summary.\n",
    )
    .unwrap();
    let hidden_path =
        solstone_core_format::content::body_hidden_days::hidden_days_path(journal.path());
    fs::create_dir_all(hidden_path.parent().unwrap()).unwrap();
    fs::write(
        hidden_path,
        solstone_core_format::content::body_hidden_days::hidden_days_bytes(
            &[hidden_day.to_owned()].into(),
        ),
    )
    .unwrap();

    // Two distinct photos on the same day to verify blank-query chronological photo browse.
    for (seg, time_label, desc) in [
        ("090000_0", "09:00", "Morning stage sketch"),
        (
            "170500_0",
            "17:05",
            "Evening yellow slippers by the cittern",
        ),
    ] {
        let pdir = segment_dir(journal.path(), "20250508", "import.image", seg);
        fs::write(
            pdir.join("talents/facets.json"),
            json!([{"facet": "alpha"}]).to_string(),
        )
        .unwrap();
        fs::write(
            pdir.join("image_transcript.md"),
            format!("# Photo {time_label}\n\n## Description\n\n{desc}\n"),
        )
        .unwrap();
    }

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    // 1. Full categories including messages (matching Claude's OAuth connection) scoped to alpha:
    //    3 distinct alpha segments -> exactly 3 hits (never 7+ duplicated chunks), 0 beta leaks.
    grant(
        journal.path(),
        &[
            "transcripts",
            "messages",
            "facets",
            "entities",
            "screen_text",
            "body",
            "notes",
            "documents",
            "calendar",
            "photos",
        ],
        ReadScope::Facets {
            ids: vec![FACET_A.to_owned()],
        },
    );
    let full = probe(
        journal.path(),
        "search",
        json!({"query": "Raleigh city gold faces chests ElDorado", "limit": 10}),
    )
    .unwrap();
    let full_rows = full["results"].as_array().unwrap();
    assert_eq!(full_rows.len(), 3, "{full}");
    assert!(
        full_rows.iter().all(|row| {
            let snippet = row["snippet"].as_str().unwrap_or("");
            !snippet.contains("secret beta") && !snippet.contains("hidden body")
        }),
        "{full}"
    );
    let full_dates: BTreeSet<&str> = full_rows
        .iter()
        .map(|row| row["date"].as_str().unwrap())
        .collect();
    assert_eq!(
        full_dates,
        BTreeSet::from(["20250211", "20250315", "20250922"]),
        "{full}"
    );

    // Single-term strict-AND query across all 8 matching chunks also collapses cleanly to 3 segments.
    let single_term = probe(
        journal.path(),
        "search",
        json!({"query": "Raleigh", "limit": 10}),
    )
    .unwrap();
    assert_eq!(
        single_term["results"].as_array().unwrap().len(),
        3,
        "{single_term}"
    );

    // 2. Exact BM25 tie on Segment 2: audio.jsonl wins with exact clock, unnamed speaker, link, segments.
    let tie = probe(
        journal.path(),
        "search",
        json!({"query": "pillar rumour", "limit": 10}),
    )
    .unwrap();
    let tie_rows = tie["results"].as_array().unwrap();
    assert_eq!(tie_rows.len(), 1, "{tie}");
    assert_eq!(tie_rows[0]["date"], "20250922");
    assert_eq!(tie_rows[0]["time"], "09:15:44");
    assert_eq!(tie_rows[0]["speaker"], "(a clerk by the pillar)");
    assert!(
        tie_rows[0]["link"]
            .as_str()
            .unwrap()
            .starts_with("https://go.solstone.app/j#")
    );
    assert!(!tie_rows[0]["segments"].as_array().unwrap().is_empty());

    // 3. Page 1 -> Page 2 -> Page 3 cursor pagination (limit=1) produces exact same 3 segments,
    //    zero cross-page duplicates, and zero skips.
    let mut paged_dates = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = json!({"query": "Raleigh city gold faces chests ElDorado", "limit": 1});
        if let Some(token) = &cursor {
            args["cursor"] = Value::String(token.clone());
        }
        let page = probe(journal.path(), "search", args).unwrap();
        let rows = page["results"].as_array().unwrap();
        assert!(rows.len() <= 1, "{page}");
        for row in rows {
            paged_dates.push(row["date"].as_str().unwrap().to_owned());
        }
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    let expected_dates: Vec<String> = full_rows
        .iter()
        .map(|row| row["date"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(paged_dates, expected_dates);

    // 4. Messages denied (topics.md rejected in row_clause) & entities denied (possible_names blocked):
    //    Segment 1 still surfaces cleanly (not dropped!) and possible_names never leaks.
    grant(
        journal.path(),
        &["transcripts", "facets"],
        ReadScope::Facets {
            ids: vec![FACET_A.to_owned()],
        },
    );
    let no_msg = probe(
        journal.path(),
        "search",
        json!({"query": "Raleigh city gold faces chests ElDorado", "limit": 10}),
    )
    .unwrap();
    let no_msg_rows = no_msg["results"].as_array().unwrap();
    assert_eq!(no_msg_rows.len(), 3, "{no_msg}");
    let no_msg_pn = probe(
        journal.path(),
        "search",
        json!({"query": "Robert Southwell", "limit": 10}),
    )
    .unwrap();
    assert_eq!(no_msg_pn["results"].as_array().unwrap().len(), 0);

    // 5. Transcripts denied (facets only): zero audio.jsonl/topics.md leaks, all 3 segments
    //    still surface via admitted activity.md / sense.md.
    grant(
        journal.path(),
        &["facets"],
        ReadScope::Facets {
            ids: vec![FACET_A.to_owned()],
        },
    );
    let no_tx = probe(
        journal.path(),
        "search",
        json!({"query": "Raleigh", "limit": 10}),
    )
    .unwrap();
    let no_tx_rows = no_tx["results"].as_array().unwrap();
    assert_eq!(no_tx_rows.len(), 3, "{no_tx}");
    assert!(no_tx_rows.iter().all(|row| row.get("speaker").is_none()));

    // 6. Blank-query chronological photo browse preserves both distinct photos on 20250508 in order.
    grant(
        journal.path(),
        &["photos"],
        ReadScope::Facets {
            ids: vec![FACET_A.to_owned()],
        },
    );
    let photos = probe(
        journal.path(),
        "search",
        json!({"day": "20250508", "kind": ["photos"], "limit": 10}),
    )
    .unwrap();
    let photo_rows = photos["results"].as_array().unwrap();
    assert_eq!(photo_rows.len(), 2, "{photos}");
    assert_eq!(photo_rows[0]["time"], "09:00:00");
    assert_eq!(photo_rows[1]["time"], "17:05:00");
}
