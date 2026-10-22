// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{FixedOffset, TimeZone, Utc};
use serde_json::{Value, json};
use solstone_core_format::content::{
    Family, OccurrenceTimeMs, RawPerceptFamily, produce_chunks, produce_raw_percept_chunks,
};
use solstone_core_import::RegistrySource;
use solstone_core_import_sources::{chatgpt, claude, conversations, gemini, save};
use support::{TempTree, write_zip};

#[test]
fn conversation_entries_keep_roles_timestamps_and_five_minute_windows() {
    let tree = TempTree::new();
    let claude_plan = claude::plan(&support::claude_archive(&tree), &Utc).unwrap();
    let chatgpt_plan = chatgpt::plan(&support::chatgpt_archive(&tree), &Utc).unwrap();
    for plan in [&claude_plan, &chatgpt_plan] {
        assert_eq!(
            plan.date_range,
            ("20260311".to_owned(), "20260311".to_owned())
        );
        assert_eq!(plan.segments[0].day, "20260311");
        assert_eq!(plan.segments[0].segment_key, "120000_300");
        assert_eq!(plan.segments[0].entries[0].speaker, "Human");
        assert_eq!(plan.segments[0].entries[1].speaker, "Assistant");
        assert_eq!(plan.segments[0].entries[0].start, "00:00:00");
        assert_eq!(plan.segments[0].entries[1].start, "00:01:00");
    }
}

#[test]
fn imported_user_messages_reach_talent_sources_as_the_owner() {
    let tree = TempTree::new();
    let cases = [
        (
            RegistrySource::Chatgpt,
            chatgpt::plan(&support::chatgpt_archive(&tree), &Utc).unwrap(),
            vec![("You", "Hello"), ("Assistant", "Hi")],
        ),
        (
            RegistrySource::Claude,
            claude::plan(&support::claude_archive(&tree), &Utc).unwrap(),
            vec![("You", "Hello"), ("Assistant", "Hi")],
        ),
        (
            RegistrySource::Gemini,
            gemini::plan(&support::gemini_archive(&tree), &Utc).unwrap(),
            vec![
                ("You", "prompt only"),
                ("Assistant", "response only &\u{a0}!?"),
                ("You", "name fallback"),
                ("Assistant", "both"),
            ],
        ),
    ];

    for (source, plan, expected) in cases {
        let rendered = conversations::render(source, plan, "owner-test");
        assert_eq!(rendered.files.len(), 1, "{}", source.name());
        let file = &rendered.files[0];
        let rows = file
            .contents
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), expected.len(), "{}", source.name());
        let rel = format!(
            "{}/import.{}/{}/{}",
            file.day,
            source.name(),
            file.segment,
            file.name
        );
        // The transcript source loader uses the raw audio renderer for these rows.
        let presented = produce_raw_percept_chunks(RawPerceptFamily::Audio, &rel, &file.contents);
        assert_eq!(presented.error, None);
        assert_eq!(presented.chunks.len(), expected.len());
        for ((row, chunk), (speaker, text)) in rows.iter().zip(&presented.chunks).zip(&expected) {
            assert_eq!(row["speaker"], *speaker, "{}", source.name());
            assert_eq!(row["text"], *text);
            assert_eq!(row["source"], "import");
            assert!(chunk.content.contains(&format!("{speaker}: {text}")));
        }
        assert_eq!(rendered.items[0]["preview"], expected[0].1);
    }
}

#[test]
fn claude_unfamiliar_or_missing_senders_are_not_assigned_to_either_participant() {
    let tree = TempTree::new();
    let path = tree.path().join("unknown-senders.zip");
    let messages = [
        json!({"sender": "human", "text": "owner message"}),
        json!({"sender": "assistant", "text": "assistant message"}),
        json!({"sender": "tool", "text": "tool message"}),
        json!({"sender": "user", "text": "unrecognized native sender"}),
        json!({"text": "missing sender"}),
        json!({"sender": null, "text": "null sender"}),
        json!({"sender": 7, "text": "non-string sender"}),
    ];
    write_zip(
        &path,
        &[(
            "conversations.json".to_owned(),
            json!([{
                "created_at": "2026-03-11T12:00:00Z",
                "chat_messages": messages
            }])
            .to_string()
            .into_bytes(),
        )],
    );
    let plan = claude::plan(&path, &Utc).unwrap();
    let rendered = conversations::render(RegistrySource::Claude, plan, "unknown-test");
    let rows = rendered.files[0]
        .contents
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), messages.len());
    let expected = [
        "You",
        "Assistant",
        "Unknown",
        "Unknown",
        "Unknown",
        "Unknown",
        "Unknown",
    ];
    for ((row, message), speaker) in rows.iter().zip(&messages).zip(expected) {
        assert_eq!(row["speaker"], speaker);
        assert_eq!(row["text"], message["text"]);
    }
    assert_eq!(rendered.items[0]["preview"], "owner message");
}

#[test]
fn a_message_lands_on_the_day_and_minute_of_the_zone_it_is_planned_in() {
    let tree = TempTree::new();
    let path = tree.path().join("offset.zip");
    write_zip(
        &path,
        &[(
            "conversations.json".to_owned(),
            json!([{
                "chat_messages": [{
                    "sender": "human",
                    "text": "offset message",
                    "created_at": "2026-03-11T23:30:00-08:00"
                }]
            }])
            .to_string()
            .into_bytes(),
        )],
    );

    // Late evening in the owner's zone is the next morning in UTC. The journal is kept in
    // the owner's local time, so the owner's zone decides the day.
    let pacific = FixedOffset::west_opt(8 * 3600).unwrap();
    let local = claude::plan(&path, &pacific).unwrap();
    assert_eq!(
        local.date_range,
        ("20260311".to_owned(), "20260311".to_owned())
    );
    assert_eq!(local.segments[0].day, "20260311");
    assert_eq!(local.segments[0].segment_key, "233000_300");

    let utc = claude::plan(&path, &Utc).unwrap();
    assert_eq!(utc.segments[0].day, "20260312");
    assert_eq!(utc.segments[0].segment_key, "073000_300");
}

fn setup_journal(tree: &TempTree, zone: &str) -> PathBuf {
    let journal = tree.path().join("journal");
    fs::create_dir_all(journal.join("config")).unwrap();
    fs::write(
        journal.join("config/journal.json"),
        json!({"identity": {"timezone": zone}}).to_string(),
    )
    .unwrap();
    journal
}

fn chatgpt_archive_with_messages(tree: &TempTree, messages: &[(i64, &str, &str)]) -> PathBuf {
    let path = tree.path().join(format!("chatgpt-{}.zip", messages[0].0));
    let mut mapping = serde_json::Map::new();
    let mut parent = None;
    let mut current_node = String::new();
    for (i, (ts, role, text)) in messages.iter().enumerate() {
        let node_id = format!("node_{i}");
        let mut msg_obj = serde_json::Map::new();
        msg_obj.insert("create_time".to_owned(), json!(*ts as f64));
        msg_obj.insert("author".to_owned(), json!({"role": role}));
        msg_obj.insert("content".to_owned(), json!({"parts": [text]}));
        if *role == "assistant" {
            msg_obj.insert("metadata".to_owned(), json!({"model_slug": "gpt-test"}));
        }
        let mut node = serde_json::Map::new();
        node.insert("message".to_owned(), Value::Object(msg_obj));
        node.insert(
            "parent".to_owned(),
            parent.map(Value::String).unwrap_or(Value::Null),
        );
        mapping.insert(node_id.clone(), Value::Object(node));
        parent = Some(node_id.clone());
        current_node = node_id;
    }
    let conversations = json!([
        {
            "mapping": mapping,
            "current_node": current_node
        }
    ]);
    write_zip(
        &path,
        &[(
            "conversations.json".to_owned(),
            conversations.to_string().into_bytes(),
        )],
    );
    path
}

fn rows_from_file(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn chatgpt_fall_back_allocates_stepped_window_and_preserves_instants() {
    let tree = TempTree::new();
    let journal = setup_journal(&tree, "America/Denver");
    let zone = solstone_core_journal_config::owner_zone(&journal);
    let t1 = Utc.with_ymd_and_hms(2026, 11, 1, 7, 58, 0).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 11, 1, 8, 58, 0).unwrap();
    let messages = [
        (t1.timestamp(), "user", "earlier"),
        (t2.timestamp(), "assistant", "later"),
    ];
    let archive = chatgpt_archive_with_messages(&tree, &messages);
    let rendered = save::render(RegistrySource::Chatgpt, &archive, "imp-chatgpt", zone, None)
        .unwrap()
        .unwrap();

    assert_eq!(rendered.files.len(), 2);
    assert_eq!(rendered.files[0].day, "20261101");
    assert_eq!(rendered.files[0].segment, "015800_300");
    assert_eq!(rendered.files[1].day, "20261101");
    assert_eq!(rendered.files[1].segment, "015800_301");

    let preview = save::preview_size(&journal, &rendered).unwrap();
    assert_eq!(preview.recordings.len(), 2);
    let preview_segments: Vec<_> = preview
        .recordings
        .iter()
        .map(|r| (&r.day, &r.segment))
        .collect();
    assert_eq!(
        preview_segments,
        vec![
            (&"20261101".to_string(), &"015800_300".to_string()),
            (&"20261101".to_string(), &"015800_301".to_string()),
        ]
    );

    let outcome = save::write_rendered(&journal, Some("imp-chatgpt"), &rendered);
    assert!(outcome.error.is_none());
    assert_eq!(outcome.created.len(), 2);
    assert_eq!(outcome.created[0].day, "20261101");
    assert_eq!(outcome.created[0].segment, "015800_300");
    assert_eq!(outcome.created[1].day, "20261101");
    assert_eq!(outcome.created[1].segment, "015800_301");

    let manifest_path = journal.join("imports/imp-chatgpt/content_manifest.jsonl");
    let manifest_text = fs::read_to_string(&manifest_path).unwrap();
    let mut manifest_segments = Vec::new();
    for line in manifest_text.lines() {
        let item: Value = serde_json::from_str(line).unwrap();
        if let Some(segs) = item.get("segments").and_then(Value::as_array) {
            for seg in segs {
                let day = seg["day"].as_str().unwrap().to_string();
                let key = seg["key"].as_str().unwrap().to_string();
                manifest_segments.push((day, key));
            }
        }
    }
    assert_eq!(
        manifest_segments,
        vec![
            ("20261101".to_string(), "015800_300".to_string()),
            ("20261101".to_string(), "015800_301".to_string()),
        ]
    );

    let rows0 = rows_from_file(&outcome.created[0].path);
    assert_eq!(rows0[1]["start"], "00:00:00");
    let rows1 = rows_from_file(&outcome.created[1].path);
    assert_eq!(rows1[1]["start"], "00:00:00");

    let text0 = fs::read_to_string(&outcome.created[0].path).unwrap();
    let rel0 = format!(
        "20261101/import.chatgpt/{}/conversation_transcript.jsonl",
        outcome.created[0].segment
    );
    let prod0 = produce_chunks(Family::AiChat, &rel0, &text0);
    assert_eq!(
        prod0.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(t1.timestamp_millis())),
    );

    let text1 = fs::read_to_string(&outcome.created[1].path).unwrap();
    let rel1 = format!(
        "20261101/import.chatgpt/{}/conversation_transcript.jsonl",
        outcome.created[1].segment
    );
    let prod1 = produce_chunks(Family::AiChat, &rel1, &text1);
    assert_eq!(
        prod1.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(t2.timestamp_millis())),
    );
}

#[test]
fn ordinary_day_control_groups_first_two_and_steps_third() {
    let tree = TempTree::new();
    let journal = setup_journal(&tree, "America/Denver");
    let zone = solstone_core_journal_config::owner_zone(&journal);
    let t1 = Utc.with_ymd_and_hms(2026, 3, 11, 7, 58, 0).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 3, 11, 8, 1, 0).unwrap();
    let t3 = Utc.with_ymd_and_hms(2026, 3, 11, 8, 3, 0).unwrap();
    let messages = [
        (t1.timestamp(), "user", "first"),
        (t2.timestamp(), "assistant", "second"),
        (t3.timestamp(), "user", "third"),
    ];
    let archive = chatgpt_archive_with_messages(&tree, &messages);
    let rendered = save::render(
        RegistrySource::Chatgpt,
        &archive,
        "imp-ordinary",
        zone,
        None,
    )
    .unwrap()
    .unwrap();

    assert_eq!(rendered.files.len(), 2);
    assert_eq!(rendered.files[0].day, "20260311");
    assert_eq!(rendered.files[0].segment, "015800_300");
    assert_eq!(rendered.files[1].day, "20260311");
    assert_eq!(rendered.files[1].segment, "020300_300");

    let outcome = save::write_rendered(&journal, Some("imp-ordinary"), &rendered);
    assert!(outcome.error.is_none());

    let rows0 = rows_from_file(&outcome.created[0].path);
    assert_eq!(rows0[1]["start"], "00:00:00");
    assert_eq!(rows0[2]["start"], "00:03:00");

    let text0 = fs::read_to_string(&outcome.created[0].path).unwrap();
    let rel0 = format!(
        "20260311/import.chatgpt/{}/conversation_transcript.jsonl",
        outcome.created[0].segment
    );
    let prod0 = produce_chunks(Family::AiChat, &rel0, &text0);
    assert_eq!(
        prod0.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(t1.timestamp_millis())),
    );
    assert_eq!(
        prod0.chunks[1].occurrence_time_ms,
        Some(OccurrenceTimeMs(t2.timestamp_millis())),
    );

    let rows1 = rows_from_file(&outcome.created[1].path);
    assert_eq!(rows1[1]["start"], "00:00:00");

    let text1 = fs::read_to_string(&outcome.created[1].path).unwrap();
    let rel1 = format!(
        "20260311/import.chatgpt/{}/conversation_transcript.jsonl",
        outcome.created[1].segment
    );
    let prod1 = produce_chunks(Family::AiChat, &rel1, &text1);
    assert_eq!(
        prod1.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(t3.timestamp_millis())),
    );
}

#[test]
fn spring_pair_inside_one_window_keeps_both_instants() {
    let tree = TempTree::new();
    let journal = setup_journal(&tree, "America/Denver");
    let zone = solstone_core_journal_config::owner_zone(&journal);
    let t1 = Utc.with_ymd_and_hms(2026, 3, 8, 8, 58, 0).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 3, 8, 9, 1, 0).unwrap();
    let messages = [
        (t1.timestamp(), "user", "first"),
        (t2.timestamp(), "assistant", "second"),
    ];
    let archive = chatgpt_archive_with_messages(&tree, &messages);
    let rendered = save::render(RegistrySource::Chatgpt, &archive, "imp-spring", zone, None)
        .unwrap()
        .unwrap();

    assert_eq!(rendered.files.len(), 1);
    assert_eq!(rendered.files[0].day, "20260308");
    assert_eq!(rendered.files[0].segment, "015800_300");

    let outcome = save::write_rendered(&journal, Some("imp-spring"), &rendered);
    assert!(outcome.error.is_none());

    let rows = rows_from_file(&outcome.created[0].path);
    assert_eq!(rows[1]["start"], "00:00:00");
    assert_eq!(rows[2]["start"], "00:03:00");

    let text0 = fs::read_to_string(&outcome.created[0].path).unwrap();
    let rel0 = format!(
        "20260308/import.chatgpt/{}/conversation_transcript.jsonl",
        outcome.created[0].segment
    );
    let prod = produce_chunks(Family::AiChat, &rel0, &text0);
    assert_eq!(
        prod.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(t1.timestamp_millis())),
    );
    assert_eq!(
        prod.chunks[1].occurrence_time_ms,
        Some(OccurrenceTimeMs(t2.timestamp_millis())),
    );
}

#[test]
fn reimport_leaves_paths_source_ms_and_sibling_marker_unchanged() {
    let tree = TempTree::new();
    let journal = setup_journal(&tree, "America/Denver");
    let zone = solstone_core_journal_config::owner_zone(&journal);
    let t1 = Utc.with_ymd_and_hms(2026, 11, 1, 7, 58, 0).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 11, 1, 8, 58, 0).unwrap();
    let messages = [
        (t1.timestamp(), "user", "earlier"),
        (t2.timestamp(), "assistant", "later"),
    ];
    let archive = chatgpt_archive_with_messages(&tree, &messages);
    let rendered = save::render(
        RegistrySource::Chatgpt,
        &archive,
        "imp-reimport",
        zone,
        None,
    )
    .unwrap()
    .unwrap();

    let marker_path = journal.join("imports/keep-me/marker");
    fs::create_dir_all(marker_path.parent().unwrap()).unwrap();
    fs::write(&marker_path, b"keep-me-bytes").unwrap();

    let outcome1 = save::write_rendered(&journal, Some("imp-reimport"), &rendered);
    let rows1 = rows_from_file(&outcome1.created[0].path);
    let source_ms1 = rows1[1]["source_ms"].as_i64().unwrap();
    let dir_count1 = fs::read_dir(journal.join("chronicle/20261101/import.chatgpt"))
        .unwrap()
        .count();

    let outcome2 = save::write_rendered(&journal, Some("imp-reimport"), &rendered);
    assert_eq!(outcome1.created[0].path, outcome2.created[0].path);
    assert_eq!(outcome1.created[1].path, outcome2.created[1].path);

    let dir_count2 = fs::read_dir(journal.join("chronicle/20261101/import.chatgpt"))
        .unwrap()
        .count();
    assert_eq!(dir_count1, dir_count2);

    let rows2 = rows_from_file(&outcome2.created[0].path);
    let source_ms2 = rows2[1]["source_ms"].as_i64().unwrap();
    assert_eq!(source_ms1, source_ms2);

    assert_eq!(fs::read(&marker_path).unwrap(), b"keep-me-bytes");
}

fn chatgpt_archive_with_conversations(
    tree: &TempTree,
    conversations_data: &[&[(i64, &str, &str)]],
) -> PathBuf {
    let path = tree.path().join("chatgpt-convs.zip");
    let mut convs = Vec::new();
    for conv_messages in conversations_data {
        let mut mapping = serde_json::Map::new();
        let mut parent = None;
        let mut current_node = String::new();
        for (i, (ts, role, text)) in conv_messages.iter().enumerate() {
            let node_id = format!("node_{i}");
            let mut msg_obj = serde_json::Map::new();
            msg_obj.insert("create_time".to_owned(), json!(*ts as f64));
            msg_obj.insert("author".to_owned(), json!({"role": role}));
            msg_obj.insert("content".to_owned(), json!({"parts": [text]}));
            if *role == "assistant" {
                msg_obj.insert("metadata".to_owned(), json!({"model_slug": "gpt-test"}));
            }
            let mut node = serde_json::Map::new();
            node.insert("message".to_owned(), Value::Object(msg_obj));
            node.insert(
                "parent".to_owned(),
                parent.map(Value::String).unwrap_or(Value::Null),
            );
            mapping.insert(node_id.clone(), Value::Object(node));
            parent = Some(node_id.clone());
            current_node = node_id;
        }
        convs.push(json!({
            "mapping": mapping,
            "current_node": current_node
        }));
    }
    write_zip(
        &path,
        &[(
            "conversations.json".to_owned(),
            json!(convs).to_string().into_bytes(),
        )],
    );
    path
}

#[test]
fn tombstone_skips_only_that_key_and_leaves_second_window() {
    let tree = TempTree::new();
    let journal = setup_journal(&tree, "America/Denver");
    let zone = solstone_core_journal_config::owner_zone(&journal);
    let t1 = Utc.with_ymd_and_hms(2026, 11, 1, 7, 58, 0).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 11, 1, 8, 58, 0).unwrap();
    let archive = chatgpt_archive_with_conversations(
        &tree,
        &[
            &[(t1.timestamp(), "user", "earlier")],
            &[(t2.timestamp(), "user", "later")],
        ],
    );
    let rendered = save::render(RegistrySource::Chatgpt, &archive, "imp-tomb", zone, None)
        .unwrap()
        .unwrap();

    let tomb_dir = journal.join("chronicle/20261101/import.chatgpt/015800_300");
    fs::create_dir_all(&tomb_dir).unwrap();
    fs::write(tomb_dir.join("tombstone.json"), b"{}").unwrap();

    let outcome = save::write_rendered(&journal, Some("imp-tomb"), &rendered);
    assert_eq!(
        outcome.skipped_deleted,
        vec![("20261101".to_string(), "015800_300".to_string())]
    );
    assert_eq!(outcome.created.len(), 1);
    assert_eq!(outcome.created[0].segment, "015800_301");
    assert!(outcome.created[0].path.exists());
    assert!(
        !journal
            .join("chronicle/20261101/import.chatgpt/015800_302")
            .exists()
    );

    let manifest_path = journal.join("imports/imp-tomb/content_manifest.jsonl");
    let manifest_text = fs::read_to_string(&manifest_path).unwrap();
    let mut manifest_segments = Vec::new();
    for line in manifest_text.lines() {
        let item: Value = serde_json::from_str(line).unwrap();
        if let Some(segs) = item.get("segments").and_then(Value::as_array) {
            for seg in segs {
                manifest_segments.push(seg["key"].as_str().unwrap().to_string());
            }
        }
    }
    assert_eq!(manifest_segments, vec!["015800_301".to_string()]);
}

#[test]
fn claude_and_gemini_fall_back_archives_render_distinct_keys_and_instants() {
    let tree = TempTree::new();
    let journal = setup_journal(&tree, "America/Denver");
    let zone = solstone_core_journal_config::owner_zone(&journal);
    let t1 = Utc.with_ymd_and_hms(2026, 11, 1, 7, 58, 0).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 11, 1, 8, 58, 0).unwrap();

    // Claude
    let claude_path = tree.path().join("claude-fallback.zip");
    let claude_conv = json!([
        {
            "created_at": "2026-11-01T07:58:00Z",
            "chat_messages": [
                {"sender": "human", "text": "earlier", "created_at": "2026-11-01T07:58:00Z"},
                {"sender": "assistant", "text": "later", "created_at": "2026-11-01T08:58:00Z"}
            ]
        }
    ]);
    write_zip(
        &claude_path,
        &[(
            "conversations.json".to_owned(),
            claude_conv.to_string().into_bytes(),
        )],
    );
    let claude_rendered = save::render(
        RegistrySource::Claude,
        &claude_path,
        "imp-claude",
        zone,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(claude_rendered.files.len(), 2);
    assert_eq!(claude_rendered.files[0].segment, "015800_300");
    assert_eq!(claude_rendered.files[1].segment, "015800_301");
    let claude_outcome = save::write_rendered(&journal, Some("imp-claude"), &claude_rendered);
    let c_text0 = fs::read_to_string(&claude_outcome.created[0].path).unwrap();
    let c_rel0 = format!(
        "20261101/import.claude/{}/conversation_transcript.jsonl",
        claude_outcome.created[0].segment
    );
    let c_prod0 = produce_chunks(Family::AiChat, &c_rel0, &c_text0);
    assert_eq!(
        c_prod0.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(t1.timestamp_millis())),
    );
    let c_text1 = fs::read_to_string(&claude_outcome.created[1].path).unwrap();
    let c_rel1 = format!(
        "20261101/import.claude/{}/conversation_transcript.jsonl",
        claude_outcome.created[1].segment
    );
    let c_prod1 = produce_chunks(Family::AiChat, &c_rel1, &c_text1);
    assert_eq!(
        c_prod1.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(t2.timestamp_millis())),
    );

    // Gemini
    let gemini_path = tree.path().join("gemini-fallback.zip");
    let gemini_activities = json!([
        {
            "time": "2026-11-01T07:58:00Z",
            "subtitles": [{"name": "earlier"}]
        },
        {
            "time": "2026-11-01T08:58:00Z",
            "subtitles": [{"name": "later"}]
        }
    ]);
    write_zip(
        &gemini_path,
        &[(
            "Takeout/My Activity/Gemini Apps/MyActivity.json".to_owned(),
            gemini_activities.to_string().into_bytes(),
        )],
    );
    let gemini_rendered = save::render(
        RegistrySource::Gemini,
        &gemini_path,
        "imp-gemini",
        zone,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(gemini_rendered.files.len(), 2);
    assert_eq!(gemini_rendered.files[0].segment, "015800_300");
    assert_eq!(gemini_rendered.files[1].segment, "015800_301");
    let gemini_outcome = save::write_rendered(&journal, Some("imp-gemini"), &gemini_rendered);
    let g_text0 = fs::read_to_string(&gemini_outcome.created[0].path).unwrap();
    let g_rel0 = format!(
        "20261101/import.gemini/{}/conversation_transcript.jsonl",
        gemini_outcome.created[0].segment
    );
    let g_prod0 = produce_chunks(Family::AiChat, &g_rel0, &g_text0);
    assert_eq!(
        g_prod0.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(t1.timestamp_millis())),
    );
    let g_text1 = fs::read_to_string(&gemini_outcome.created[1].path).unwrap();
    let g_rel1 = format!(
        "20261101/import.gemini/{}/conversation_transcript.jsonl",
        gemini_outcome.created[1].segment
    );
    let g_prod1 = produce_chunks(Family::AiChat, &g_rel1, &g_text1);
    assert_eq!(
        g_prod1.chunks[0].occurrence_time_ms,
        Some(OccurrenceTimeMs(t2.timestamp_millis())),
    );
}

#[cfg(feature = "full-tests")]
#[test]
#[allow(unsafe_code)]
fn owner_zone_beats_the_host_zone_for_repeated_clocks() {
    struct TzGuard(Option<String>);
    impl Drop for TzGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(val) => unsafe { std::env::set_var("TZ", val) },
                None => unsafe { std::env::remove_var("TZ") },
            }
        }
    }
    let _guard = TzGuard(std::env::var("TZ").ok());
    unsafe { std::env::set_var("TZ", "UTC") };

    let tree = TempTree::new();
    let journal_denver = setup_journal(&tree, "America/Denver");
    let zone_denver = solstone_core_journal_config::owner_zone(&journal_denver);
    let t1 = Utc.with_ymd_and_hms(2026, 11, 1, 7, 58, 0).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 11, 1, 8, 58, 0).unwrap();
    let messages = [
        (t1.timestamp(), "user", "earlier"),
        (t2.timestamp(), "assistant", "later"),
    ];
    let archive = chatgpt_archive_with_messages(&tree, &messages);

    let rendered_denver = save::render(
        RegistrySource::Chatgpt,
        &archive,
        "imp-denver",
        zone_denver,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(rendered_denver.files.len(), 2);
    assert_eq!(rendered_denver.files[0].segment, "015800_300");
    assert_eq!(rendered_denver.files[1].segment, "015800_301");

    // TZ=UTC and no config
    let tree_utc = TempTree::new();
    let journal_utc = tree_utc.path().join("journal");
    fs::create_dir_all(&journal_utc).unwrap();
    let zone_utc = solstone_core_journal_config::owner_zone(&journal_utc);
    let rendered_utc = save::render(RegistrySource::Chatgpt, &archive, "imp-utc", zone_utc, None)
        .unwrap()
        .unwrap();
    assert_eq!(rendered_utc.files.len(), 2);
    assert_eq!(rendered_utc.files[0].segment, "075800_300");
    assert_eq!(rendered_utc.files[1].segment, "085800_300");
}
