// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

//! Purpose-built MCP read-boundary fixtures. These never use the development
//! journal fixture because each case needs isolated, adversarial journal state.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::content::{Family, produce_chunks};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};

use crate::dispatch::{DispatchPrincipal, McpProbeError, run_mcp_probe};
use crate::permissions::{
    ConnectionReadSnapshot, PermissionDecision, PermissionStore, ReadPermission, ReadScope,
    evaluate_connection_read,
};
use crate::segment_links::{LinkSubject, TranscriptPageCheck, issue_segment_links};

const CONNECTION: &str = "bearer:boundary-test";
const FACET_A: &str = "123e4567-e89b-42d3-a456-426614174000";
const FACET_B: &str = "123e4567-e89b-42d3-a456-426614174001";
const DAY: &str = "20260914";
const STREAM: &str = "default";
const SEGMENT: &str = "090000_300";
const PATH: &str = "20260914/default/090000_300/talents/audio.md";

#[test]
fn topic_provenance_gates_search_and_fetch_of_existing_index_rows() {
    use solstone_core_format::content::AdmittedCategory as Kind;
    use solstone_core_format::content::kinds::permission_words;
    use solstone_core_format::content::topics::{POSSIBLE_INPUTS, PROVENANCE_FILE, provenance};

    let journal = tempfile::tempdir_in(crate::test_scratch()).unwrap();
    let root = journal.path();
    type TopicCase<'a> = (&'a str, &'a [Kind], &'a [(&'a str, &'a str)]);
    let cases: &[TopicCase<'_>] = &[
        (
            "screentopic",
            &[Kind::ScreenText],
            &[("screen.jsonl", "{}")],
        ),
        (
            "speechtopic",
            &[Kind::Transcripts],
            &[("audio.jsonl", "{}")],
        ),
        (
            "phototopic",
            &[Kind::Photos],
            &[("image_transcript.md", "A flower photograph")],
        ),
        (
            "notetopic",
            &[Kind::Notes],
            &[("note_transcript.md", "A note about a flower")],
        ),
        (
            "mixedtopic",
            &[Kind::Transcripts, Kind::ScreenText],
            &[("audio.jsonl", "{}"), ("screen.jsonl", "{}")],
        ),
    ];
    let mut fixtures = Vec::new();
    for (i, (word, kinds, sources)) in cases.iter().enumerate() {
        for (suffix, known) in [("old", false), ("new", true)] {
            let key = format!("{:02}{}00_0", 9 + i, if known { "10" } else { "00" });
            let rel = format!("{DAY}/desk/{key}/talents/topics.md");
            let path = root.join("chronicle").join(&rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let segment = path.parent().unwrap().parent().unwrap();
            for &(name, text) in *sources {
                fs::write(segment.join(name), text).unwrap();
            }
            let term = format!("{word}{suffix}");
            let text = format!("## Topics\n\n- {term}\n");
            fs::write(&path, &text).unwrap();
            fixtures.push((
                rel,
                path,
                term,
                text,
                known.then(|| kinds.iter().copied().collect()),
            ));
        }
    }
    // The classification and chunks were built before provenance existed.
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    let index = solstone_core_indexer_store::open_index_reader(root).unwrap();
    for (rel, path, _, text, kinds) in &fixtures {
        let category: String = index
            .query_row(
                "SELECT category FROM chunk_classification WHERE path IN (?1, ?2)",
                params![rel, format!("chronicle/{rel}")],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(category, "transcripts");
        if let Some(kinds) = kinds {
            fs::write(
                path.with_file_name(PROVENANCE_FILE),
                provenance(text, Some(kinds)).to_string(),
            )
            .unwrap();
        }
        // The source set on disk is neither proof nor a prerequisite to reading the topic.
        let segment = path.parent().unwrap().parent().unwrap();
        if segment.join("screen.jsonl").exists() {
            fs::remove_file(segment.join("screen.jsonl")).unwrap();
        }
    }
    let set_grant = |grant: &[&str]| {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: grant.iter().map(|word| (*word).to_owned()).collect(),
                    scope: ReadScope::WholeJournal,
                },
            )
            .unwrap();
    };
    let every_kind = permission_words().collect::<Vec<_>>();
    let topic_inputs = POSSIBLE_INPUTS
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>();
    let all_kinds = |grant: &[&str]| {
        for (_, _, term, _, kinds) in &fixtures {
            let search = probe(&journal, "search", json!({"query": term})).unwrap();
            let hits = search["results"].as_array().unwrap();
            let expected = kinds
                .as_ref()
                .map_or_else(
                    || topic_inputs.clone(),
                    solstone_core_format::content::topics::kind_words,
                )
                .iter()
                .all(|kind| grant.contains(kind));
            assert_eq!(
                hits.len(),
                usize::from(expected),
                "{grant:?} {term}: {search}"
            );
            if let Some(hit) = hits.first() {
                let fetched =
                    probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
                assert!(fetched["text"].as_str().unwrap().contains(term));
                for item in [hit, &fetched] {
                    match kinds
                        .as_ref()
                        .map(solstone_core_format::content::topics::kind_words)
                        .as_deref()
                    {
                        Some([kind]) => assert_eq!(item["content_kind"], *kind),
                        Some(kinds) => {
                            assert_eq!(item["content_kinds"], json!(kinds));
                            assert!(item.get("content_kind").is_none());
                        }
                        None => {
                            assert!(item.get("content_kind").is_none());
                            assert!(item.get("content_kinds").is_none());
                        }
                    }
                }
            }
        }
    };
    for grant in [
        every_kind.as_slice(),
        &["transcripts"][..],
        &["screen_text"][..],
        &["transcripts", "screen_text"][..],
        &["photos"][..],
        &["notes"][..],
        topic_inputs.as_slice(),
    ] {
        set_grant(grant);
        all_kinds(grant);
    }

    // Updating an existing grandfathered connection to all permissions uses the
    // same stored grant on the reader as a new all-permissions connection.
    set_grant(&["transcripts", "facets", "entities"]);
    set_grant(&every_kind);
    all_kinds(&every_kind);

    let (_, path, term, _, _) = fixtures
        .iter()
        .find(|(_, _, term, _, _)| term == "mixedtopicnew")
        .unwrap();
    let search = probe(&journal, "search", json!({"query": term})).unwrap();
    let reference = search["results"][0]["reference"].clone();
    let replacement = "## Topics\n\n- a fresh entry\n";
    fs::write(path, replacement).unwrap();
    fs::write(
        path.with_file_name(PROVENANCE_FILE),
        provenance(replacement, Some(&[Kind::Transcripts].into())).to_string(),
    )
    .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
    for grant in [&["transcripts"][..], every_kind.as_slice()] {
        set_grant(grant);
        let stale = probe(&journal, "search", json!({"query": term, "limit": 1})).unwrap();
        assert_eq!(stale["results"], json!([]), "{stale}");
        assert!(stale["next_cursor"].is_null(), "{stale}");
    }
}

#[test]
fn transcript_time_window_includes_points_and_omits_empty_segments() {
    let journal = tempfile::tempdir_in(crate::test_scratch()).unwrap();
    let root = journal.path();
    for (place, name) in [
        ("import.image/113000_0", "image_transcript.md"),
        ("import.image/123000_0", "screenshot_transcript.md"),
        ("import.document/130000_0", "document_transcript.md"),
        ("import.document/133000_0", "document_transcript.md"),
        ("desk/120000_300", "location.jsonl"),
        ("desk/121000_300", "screen.jsonl"),
    ] {
        let path = root.join("chronicle").join(DAY).join(place);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(name), "source line\n").unwrap();
    }
    for permissions in [
        vec!["transcripts"],
        vec![
            "transcripts",
            "photos",
            "screen_text",
            "documents",
            "places",
        ],
    ] {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: permissions.iter().map(|word| (*word).to_owned()).collect(),
                    scope: ReadScope::WholeJournal,
                },
            )
            .unwrap();
        let result = probe(
            &journal,
            "list_transcripts",
            json!({
                "day": DAY, "time_from": "11:30", "time_to": "13:30",
            }),
        )
        .unwrap();
        let rows = result["transcripts"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            if permissions.len() == 1 { 0 } else { 3 },
            "{result}"
        );
        for (row, clock) in rows.iter().zip(["11:30", "12:30", "13:00"]) {
            assert!(row["time"].as_str().unwrap().starts_with(clock));
            assert_eq!(row["seconds"], 0);
            assert_eq!(row["content_kinds"].as_array().unwrap().len(), 1);
        }
        if !rows.is_empty() {
            let span = probe(
                &journal,
                "get_transcript",
                json!({"reference": result["span_reference"]}),
            )
            .unwrap();
            let segments = span["segments"].as_array().unwrap();
            assert_eq!(segments.len(), rows.len());
            for (item, row) in segments.iter().zip(rows) {
                assert_eq!(item["time"], row["time"]);
                assert_eq!(item["content_kinds"], row["content_kinds"]);
            }
        }
    }
}

#[test]
fn ordinary_reads_keep_cross_source_memory_grants_and_own_recall_stays_scoped() {
    use crate::memory::{AppendResult, AuthenticatedMemorySource, append_connection_memory};
    let journal = tempfile::Builder::new()
        .prefix("ordinary-memory-grant-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let source = solstone_core_format::agent_memory::SourceKey::from_verified_id(
        "bearer:other-memory-writer",
    );
    let AppendResult::Stored(receipt) = append_connection_memory(
        journal.path(),
        AuthenticatedMemorySource {
            verified_id: "bearer:other-memory-writer",
            creation_label: "other writer",
            source: &source,
        },
        "crossmemoryneedle other agent original",
        "ordinary-grant-operation",
        "2026-01-02T12:00:00Z".parse::<jiff::Timestamp>().unwrap(),
    )
    .unwrap() else {
        panic!("fixture writer must store");
    };
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query":"crossmemoryneedle"})),
        Err(McpProbeError::PermissionDenied)
    );
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let without_notes = probe(&journal, "search", json!({"query":"crossmemoryneedle"})).unwrap();
    assert!(without_notes["results"].as_array().unwrap().is_empty());
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["notes".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let result = probe(&journal, "search", json!({"query":"crossmemoryneedle"})).unwrap();
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["origin"]["kind"], "agent_memory");
    assert_eq!(results[0]["origin"]["creation_label"], "other writer");
    assert!(results[0]["origin"].get("stream").is_none());
    assert!(results[0]["origin"].get("segment").is_none());
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference":results[0]["reference"]}),
    )
    .unwrap();
    assert_eq!(fetched["origin"], results[0]["origin"]);
    assert!(
        fetched["text"]
            .as_str()
            .unwrap()
            .contains("crossmemoryneedle other agent original")
    );
    let codec = crate::references::ReferenceCodec::new().unwrap();
    let own = crate::memory_recall::recall(
        journal.path(),
        &codec,
        CONNECTION,
        CONNECTION,
        "reader-credential",
        crate::memory_recall::RecallArgs::default(),
        jiff::civil::date(2026, 1, 2),
    );
    assert!(own.complete);
    assert!(own.notes.is_empty());
    fs::remove_file(
        journal
            .path()
            .join("chronicle/20260102")
            .join(receipt.origin.stream)
            .join(receipt.origin.segment)
            .join("ready.json"),
    )
    .unwrap();
    assert!(
        probe(&journal, "search", json!({"query":"crossmemoryneedle"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        probe(
            &journal,
            "fetch",
            json!({"reference":results[0]["reference"]})
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn kinds_keep_old_grants_strict_across_segment_search_and_fetch() {
    let journal = fixture();
    let root = journal.path();
    // Expected word is independent of the registry and of the old SQL category.
    let mut paths = vec![
        (
            "20260914/default/090000_300/audio.jsonl",
            "speechneedle",
            "transcripts",
            true,
            false,
        ),
        (
            "20260914/default/090000_300/note_transcript.md",
            "linkednoteneedle",
            "notes",
            true,
            true,
        ),
        (
            "20260308/import.obsidian/090000_300/note_transcript.md",
            "obsidianneedle",
            "notes",
            true,
            true,
        ),
        (
            "20260308/import.evernote/100000_300/note_transcript.md",
            "evernoteneedle",
            "notes",
            true,
            true,
        ),
        (
            "20260308/import.document/110000_300/document_transcript.md",
            "documentneedle",
            "documents",
            true,
            true,
        ),
        (
            "20260308/import.ics/120000_300/event_transcript.md",
            "calendarneedle",
            "calendar",
            true,
            true,
        ),
        (
            "20260308/import.image/130000_300/image_transcript.md",
            "photoneedle",
            "photos",
            true,
            true,
        ),
        (
            "20260308/import.text/140000_300/note_transcript.md",
            "manualnoteneedle",
            "notes",
            true,
            true,
        ),
        (
            "20260308/import.text/150000_300/document_transcript.md",
            "manualdocumentneedle",
            "documents",
            true,
            true,
        ),
        (
            "20260308/import.image/160000_300/screenshot_transcript.md",
            "manualscreenshotneedle",
            "screen_text",
            true,
            true,
        ),
        ("20260308/location.md", "placeneedle", "places", false, true),
        (
            "20260914/default/090000_300/talents/screen.md",
            "screenneedle",
            "screen_text",
            false,
            true,
        ),
        (
            "20260914/default/090000_300/talents/activity.md",
            "activityneedle",
            "facets",
            false,
            true,
        ),
        (
            "20260914/default/090000_300/talents/sense.md",
            "senseneedle",
            "facets",
            false,
            true,
        ),
        (
            "20260914/default/090000_300/talents/objects.md",
            "objectsneedle",
            "facets",
            false,
            true,
        ),
        (
            "20260914/default/090000_300/talents/custom.md",
            "customneedle",
            "facets",
            false,
            true,
        ),
    ];
    for &(path, token, _, _, _) in &paths {
        let path = root.join("chronicle").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = if path.ends_with("audio.jsonl") {
            format!("{}\n", json!({"start":"09:00:00", "text":token}))
        } else {
            format!("{token}\n")
        };
        fs::write(path, bytes).unwrap();
    }
    use crate::memory::{AppendResult, AuthenticatedMemorySource, append_connection_memory};
    let source = solstone_core_format::agent_memory::SourceKey::from_verified_id(
        "bearer:fixture-other-agent",
    );
    let AppendResult::Stored(receipt) = append_connection_memory(
        root,
        AuthenticatedMemorySource {
            verified_id: "bearer:fixture-other-agent",
            creation_label: "fixture other agent",
            source: &source,
        },
        "otheragentnoteneedle",
        "fixture-kind-grant",
        "2026-03-08T12:00:00Z".parse().unwrap(),
    )
    .unwrap() else {
        panic!("the fixture note must be written");
    };
    let memory_path = format!(
        "20260308/{}/{}/note.txt",
        receipt.origin.stream, receipt.origin.segment
    );
    paths.push((&memory_path, "otheragentnoteneedle", "notes", false, true));
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    for (grant, words) in [
        ("transcripts-only", &["transcripts"][..]),
        (
            "all-five",
            &["transcripts", "entities", "facets", "screen_text", "body"][..],
        ),
        ("notes-only", &["notes"][..]),
        ("documents-only", &["documents"][..]),
        ("calendar-only", &["calendar"][..]),
        ("places-only", &["places"][..]),
        ("photos-only", &["photos"][..]),
        ("screen-only", &["screen_text"][..]),
        ("summaries-only", &["facets"][..]),
    ] {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: words.iter().map(|word| (*word).to_owned()).collect(),
                    scope: ReadScope::WholeJournal,
                },
            )
            .unwrap();
        let segment_text = probe(&journal, "list_transcripts", json!({"limit": 100}))
            .map(|list| {
                list["transcripts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|segment| {
                        probe(
                            &journal,
                            "get_transcript",
                            json!({"reference": segment["reference"]}),
                        )
                        .unwrap()["entries"]
                            .to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        for &(path, token, word, direct, indexed) in &paths {
            let segment_read = segment_text.contains(token);
            let results = probe(&journal, "search", json!({"query":token})).unwrap()["results"]
                .as_array()
                .unwrap()
                .clone();
            let search_read = !results.is_empty();
            let fetch_read = results.iter().any(|hit| {
                probe(&journal, "fetch", json!({"reference":hit["reference"]})).is_ok_and(
                    |fetched| {
                        fetched["text"]
                            .as_str()
                            .is_some_and(|text| text.contains(token))
                    },
                )
            });
            assert_eq!(
                (segment_read, search_read, fetch_read),
                (
                    // Screen-only connections retain the existing search/fetch tool list.
                    direct && words.contains(&word) && grant != "screen-only",
                    indexed && words.contains(&word),
                    indexed && words.contains(&word)
                ),
                "{grant} {path}",
            );
        }
    }
}

#[test]
fn a_named_missing_kind_has_a_closed_permission_refusal() {
    let journal = fixture();
    let entry = crate::registry::tool_by_wire_name("search").unwrap();
    let arguments = json!({"query":"unknown material","category":"notes"});
    let result = crate::dispatch::dispatch_authenticated_tool_call(
        journal.path(),
        DispatchPrincipal {
            connection: CONNECTION,
            agent_identity: CONNECTION,
        },
        entry.tool_name,
        Some(&arguments),
        jiff::Timestamp::now(),
    );
    assert!(matches!(
        result,
        Err(crate::dispatch::DispatchError::KindDenied(
            AdmittedCategory::Notes
        ))
    ));
}

#[test]
fn import_summary_search_and_saved_reference_follow_the_current_source() {
    let journal = fixture();
    let root = journal.path();
    let dir = root.join("imports/20261120_090000");
    fs::create_dir_all(&dir).unwrap();
    let manifest = dir.join("manifest.json");
    let summary = dir.join("summary.md");
    fs::write(&summary, "summarystamp telescope").unwrap();
    let file = "chronicle/20261120/import.text/090000_300/document_transcript.md";
    let tile = root.join(file).parent().unwrap().to_owned();
    fs::create_dir_all(&tile).unwrap();
    fs::write(root.join(file), "source document").unwrap();
    fs::write(
        tile.join("import_kind.json"),
        r#"{"kind":"documents","import_id":"20261120_090000"}"#,
    )
    .unwrap();
    fs::write(
        &manifest,
        json!({"import_id":"20261120_090000","source_type":"text","files_created":[file]})
            .to_string(),
    )
    .unwrap();
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    let search = || {
        probe(&journal, "search", json!({"query":"summarystamp"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .clone()
    };
    let grant = |word: &str| {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: [word.to_owned()].into_iter().collect(),
                    scope: ReadScope::WholeJournal,
                },
            )
            .unwrap()
    };
    for word in ["notes", "transcripts", "messages"] {
        grant(word);
        assert!(search().is_empty(), "{word}");
    }
    grant("documents");
    let hits = search();
    assert_eq!(hits.len(), 1);
    let reference = &hits[0]["reference"];
    assert!(
        probe(&journal, "fetch", json!({"reference":reference})).unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("summarystamp")
    );
    fs::remove_file(root.join(file)).unwrap();
    assert!(search().is_empty());
    assert!(probe(&journal, "fetch", json!({"reference":reference})).is_err());
    fs::write(root.join(file), "source document").unwrap();
    for (other, kind) in [
        (
            "chronicle/20261120/import.strava/090000_300/document_transcript.md",
            "documents",
        ),
        (
            "chronicle/20261120/import.text/100000_300/note_transcript.md",
            "notes",
        ),
    ] {
        let path = root.join(other);
        let dir = path.parent().unwrap();
        fs::create_dir_all(dir).unwrap();
        fs::write(&path, "separate source").unwrap();
        fs::write(
            dir.join("import_kind.json"),
            json!({"kind":kind,"import_id":"20261120_090000"}).to_string(),
        )
        .unwrap();
        fs::write(
            &manifest,
            json!({"source_type":"text","files_created":[file,other]}).to_string(),
        )
        .unwrap();
        assert!(search().is_empty(), "{other}");
        assert!(probe(&journal, "fetch", json!({"reference":reference})).is_err());
    }
    for source in ["chatgpt", "granola", "unregistered"] {
        fs::write(&manifest, json!({"source_type":source}).to_string()).unwrap();
        grant("transcripts");
        assert!(search().is_empty(), "{source}");
        grant("messages");
        assert!(search().is_empty(), "{source}");
    }
    for job in [
        json!({"source_type":"chatgpt"}),
        json!({"source_type":"claude"}),
        json!({"source_type":"gemini"}),
        json!({"source_type":"text","files_created":["chronicle/20261120/import.text/110000_300/conversation_transcript.jsonl"]}),
    ] {
        fs::write(&manifest, job.to_string()).unwrap();
        grant("facets");
        let hits = search();
        assert_eq!(hits.len(), 1, "{job}");
        let summary = probe(&journal, "fetch", json!({"reference":hits[0]["reference"]})).unwrap();
        assert!(summary["text"].as_str().unwrap().contains("summarystamp"));
        grant("messages");
        assert!(search().is_empty());
    }
    grant("documents");
    fs::remove_file(&manifest).unwrap();
    assert!(search().is_empty());
    assert!(probe(&journal, "fetch", json!({"reference":reference})).is_err());
    fs::write(&manifest, r#"{"source_type":"document"}"#).unwrap();
    fs::write(&summary, "replacement text").unwrap();
    assert!(search().is_empty());
    assert!(probe(&journal, "fetch", json!({"reference":reference})).is_err());
}

#[test]
fn summaries_only_cannot_search_weekly_passages_and_the_summaries_filter_keeps_both_grants() {
    let journal = fixture();
    let root = journal.path();
    fs::create_dir_all(root.join("reflections/weekly")).unwrap();
    fs::write(
        root.join("reflections/weekly/2026-W37.md"),
        "A uniquewitness weekly passage.\n",
    )
    .unwrap();
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    for (words, expected) in [
        (&["facets"][..], false),
        (&["transcripts"][..], false),
        (&["facets", "transcripts"][..], true),
    ] {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: words.iter().map(|word| (*word).to_owned()).collect(),
                    scope: ReadScope::WholeJournal,
                },
            )
            .unwrap();
        let mut arguments = json!({"query":"uniquewitness"});
        if words.contains(&"facets") {
            arguments["category"] = json!("facets");
        }
        let results = probe(&journal, "search", arguments).unwrap();
        let hits = results["results"].as_array().unwrap();
        assert_eq!(!hits.is_empty(), expected, "{words:?} {results}");
        if let Some(hit) = hits.first() {
            assert!(
                probe(&journal, "fetch", json!({"reference":hit["reference"]})).unwrap()["text"]
                    .as_str()
                    .unwrap()
                    .contains("uniquewitness")
            );
        }
    }
}

// These paging, scope and source-release fixtures use legacy Markdown without
// a proven medium. Opt the fixture connection into it explicitly. Single-grant
// access tests use the original default or a source-specific recording.
fn legacy_markdown_whole_journal() -> ReadPermission {
    let mut permission = ReadPermission::default_whole_journal();
    permission.categories.push("messages".into());
    permission
}

fn allow_legacy_markdown(journal: &tempfile::TempDir) {
    let store = PermissionStore::open(journal.path());
    let mut permission = store
        .get_permission(CONNECTION)
        .unwrap()
        .unwrap()
        .read
        .unwrap();
    permission.categories.push("messages".into());
    store.set_permission(CONNECTION, permission).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("solstone-mcp-boundary-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    for (name, id, title) in [("alpha", FACET_A, "Alpha"), ("beta", FACET_B, "Beta")] {
        let directory = journal.path().join("facets").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("facet.json"),
            json!({"id": id, "title": title, "description": format!("{title} details")})
                .to_string(),
        )
        .unwrap();
    }
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::create_dir_all(segment.join("talents")).unwrap();
    fs::write(
        segment.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        segment.join("meeting_transcript.md"),
        "approved transcript\n",
    )
    .unwrap();
    fs::write(segment.join("audio.jsonl"), "percept material\n").unwrap();
    fs::write(segment.join("talents/audio.md"), "disk bytes\n").unwrap();
    for (directory, id, detached) in [
        ("primary", "entity-primary", false),
        ("same-name", "entity-same-name", false),
        ("detached", "entity-detached", true),
    ] {
        let entity = journal.path().join("entities").join(directory);
        fs::create_dir_all(&entity).unwrap();
        fs::write(
            entity.join("entity.json"),
            json!({
                "id": id,
                "name": "Same Name",
                "aka": ["forbidden aka"],
                "description": "out-of-scope description",
                "detached_facets": ["forbidden detached-facet list"],
                "decisions": ["forbidden cross-facet aggregate"],
            })
            .to_string(),
        )
        .unwrap();
        let link = journal.path().join("facets/alpha/entities").join(directory);
        fs::create_dir_all(&link).unwrap();
        fs::write(
            link.join("entity.json"),
            json!({"entity_id": id, "detached": detached}).to_string(),
        )
        .unwrap();
    }

    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed bytes', ?1, ?2, '', 'fixture-stream', ?3, 0, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) \
             VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
            params![PATH],
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
            params![PATH, FACET_A],
        )
        .unwrap();
    index
        .execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) \
             VALUES (1, '', 1, 0, NULL, 0)",
            [],
        )
        .unwrap();
    index
        .execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) \
             VALUES (1, 1, 'complete', 1, 1)",
            [],
        )
        .unwrap();
    drop(index);

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    journal
}

fn probe(
    journal: &tempfile::TempDir,
    tool: &str,
    arguments: Value,
) -> Result<Value, McpProbeError> {
    run_mcp_probe(journal.path(), CONNECTION, tool, &arguments)
}

fn literal_display_text(text: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag};

    text.split('\n')
        .map(|line| {
            let mut visible = String::new();
            for event in Parser::new_ext(line, Options::all()) {
                match event {
                    Event::Start(Tag::Image { .. } | Tag::Link { .. })
                    | Event::Html(_)
                    | Event::InlineHtml(_) => panic!("active tool markup in {text:?}"),
                    Event::Text(text) | Event::Code(text) => visible.push_str(&text),
                    _ => {}
                }
            }
            visible
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn visible_search_page(result: &Value) -> Value {
    let results = result["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| {
            json!({
                "title": result["title"],
                "date": result["date"],
                "snippet": result["snippet"],
            })
        })
        .collect::<Vec<_>>();
    json!({"results": results, "coverage": result["coverage"]})
}

fn assert_no_coordinate_keys(value: &Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                assert_no_coordinate_keys(value);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                assert!(
                    !matches!(key.as_str(), "path" | "idx" | "row_id" | "rowid" | "stream"),
                    "agent result exposed internal key {key}"
                );
                assert_no_coordinate_keys(value);
            }
        }
        _ => {}
    }
}

#[test]
fn ac5_live_check_rejects_reassigned_segment_without_rescanning_the_index() {
    let journal = fixture();
    assert_eq!(
        probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let assignments = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("talents/facets.json");
    fs::write(assignments, "[{\"facet\":\"beta\"}]").unwrap();

    assert!(
        probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn ac10_fetch_returns_indexed_bytes_not_the_current_source_file() {
    let journal = fixture();
    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let reference = search["results"][0]["reference"].as_str().unwrap();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    assert_eq!(fetched["text"], "indexed bytes");
    assert_ne!(fetched["text"], "disk bytes");
}

#[test]
fn ac23_chosen_scope_excludes_unassigned_segments_while_whole_journal_includes_them() {
    let journal = fixture();
    let assignments = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("talents/facets.json");
    fs::write(&assignments, "[]").unwrap();
    assert!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn ac25_a_refusal_is_recorded_and_the_owner_can_tell_it_from_a_served_call() {
    // 🔑 This supersedes the three-field assertion: EVIDENCE.md § 5.3's gap was
    // that an owner asking "what did this connection try?" could not be told
    // "and it was refused". A denial and an admission differed only by
    // timestamp. They no longer do.
    let journal = fixture();
    let served = probe(&journal, "search", json!({"query": "indexed"}));
    assert!(served.is_ok());
    PermissionStore::open(journal.path())
        .clear_permission(CONNECTION)
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query": "indexed"})),
        Err(McpProbeError::PermissionDenied)
    );

    let page = crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 10,
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap();
    assert_eq!(page.entries.len(), 2);
    let outcomes = page
        .entries
        .iter()
        .map(|entry| entry.outcome)
        .collect::<Vec<_>>();
    assert!(outcomes.contains(&crate::activity::RecordedOutcome::Refused));
    assert!(outcomes.contains(&crate::activity::RecordedOutcome::Served));
    let refused = page
        .entries
        .iter()
        .find(|entry| entry.outcome == crate::activity::RecordedOutcome::Refused)
        .unwrap();
    assert_eq!(refused.tool_name, solstone_core_mcp_audit::ToolName::Search);
    assert_eq!(refused.connection.as_deref(), Some(CONNECTION));
    assert_eq!(
        refused.reason.as_deref(),
        Some("this connection has no read permission")
    );
    // 🔑 And the wire keeps the other half of the asymmetry: both refusals
    // — no permission at all, and a permission that lacks the category —
    // render identically to the agent.
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query": "indexed"})),
        Err(McpProbeError::PermissionDenied)
    );
    let page = crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 10,
            outcome: Some(crate::activity::RecordedOutcome::Refused),
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap();
    // ⚠ Two refusals, indistinguishable on the wire, distinguishable here.
    assert_eq!(page.entries.len(), 2);
    let mut reasons = page
        .entries
        .iter()
        .filter_map(|entry| entry.reason.clone())
        .collect::<Vec<_>>();
    reasons.sort();
    assert_eq!(
        reasons,
        [
            "this connection does not have the transcripts or messages or screen_text or body or facets or notes or documents or calendar or places or photos category",
            "this connection has no read permission",
        ]
    );
}

#[test]
fn ac25b_a_search_that_matches_nothing_is_recorded_empty_rather_than_served() {
    let journal = fixture();
    assert!(
        probe(&journal, "search", json!({"query": "zzzznotpresent"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let page = crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 10,
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(
        page.entries[0].outcome,
        crate::activity::RecordedOutcome::Empty
    );
    assert_eq!(
        page.entries[0].request.as_ref().unwrap().arguments["query"],
        "zzzznotpresent"
    );
}

fn dispatch_output(
    journal: &tempfile::TempDir,
    tool: &str,
    arguments: Value,
) -> crate::dispatch::ToolOutput {
    let entry = crate::registry::tool_by_wire_name(tool).unwrap();
    crate::dispatch::dispatch_authenticated_tool_call(
        journal.path(),
        crate::dispatch::DispatchPrincipal {
            connection: CONNECTION,
            agent_identity: CONNECTION,
        },
        entry.tool_name,
        Some(&arguments),
        jiff::Timestamp::now(),
    )
    .unwrap()
}

#[test]
fn an_empty_result_says_so_in_words_and_a_served_one_does_not() {
    let journal = fixture();
    let empty = dispatch_output(&journal, "search", json!({"query": "zzzznotpresent"}));
    assert!(empty.value["results"].as_array().unwrap().is_empty());
    let note = empty
        .empty_note
        .clone()
        .expect("an empty search carries a plain note");
    assert!(note.starts_with("No results"), "{note}");

    let rendered = crate::jsonrpc::tool_result(empty.value, empty.empty_note.as_deref());
    // The note rides in the typed result: a client reading structuredContent
    // alone drops any further text block.
    assert_eq!(rendered["structuredContent"]["note"], note);
    assert_eq!(rendered["content"].as_array().unwrap().len(), 1);
    let text: Value =
        serde_json::from_str(rendered["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, rendered["structuredContent"]);

    let served = dispatch_output(&journal, "list_facets", json!({}));
    assert!(!served.value["facets"].as_array().unwrap().is_empty());
    assert!(served.empty_note.is_none());
}

#[test]
fn a_journal_that_has_recorded_nothing_lists_no_transcripts_rather_than_failing() {
    let journal = fixture();
    fs::remove_dir_all(journal.path().join("chronicle")).unwrap();
    let output = dispatch_output(&journal, "list_transcripts", json!({}));
    assert!(output.value["transcripts"].as_array().unwrap().is_empty());
    assert!(output.empty_note.is_some());
}

#[test]
fn ac26_the_owners_activity_log_is_unreachable_through_the_boundary_it_audits() {
    // 🔴 The recursion, measured rather than argued. A connection with the
    // widest grant this build can express searches for a term that exists only
    // inside its own interaction records.
    let journal = fixture();
    probe(
        &journal,
        "search",
        json!({"query": "supercalifragilisticexpialidocious"}),
    )
    .unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "transcripts".to_owned(),
                    "entities".to_owned(),
                    "facets".to_owned(),
                ],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    // Control: the term really is on disk, in this journal, right now.
    let recorded = crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 10,
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap();
    assert!(
        recorded.entries.iter().any(|entry| {
            entry.request.as_ref().is_some_and(|request| {
                request.arguments["query"] == "supercalifragilisticexpialidocious"
            })
        }),
        "control failed: the term was never recorded, so the negative below proves nothing"
    );
    // Second control, on the coordinate that could be wrong: the same query
    // shape does find the fixture's indexed material.
    assert_eq!(
        probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    assert!(
        probe(
            &journal,
            "search",
            json!({"query": "supercalifragilisticexpialidocious"})
        )
        .unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty(),
        "an agent reached the owner's log of its own queries"
    );
    assert!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|segment| !segment.to_string().contains("mcp.agent"))
    );
}

#[test]
fn ac4_entities_without_facets_can_list_only_facet_id_and_name() {
    let journal = fixture();
    let result = probe(&journal, "list_facets", json!({})).unwrap();
    let facet = &result["facets"][0];
    assert_eq!(facet["id"], FACET_A);
    assert_eq!(facet["name"], "Alpha");
    assert!(facet.get("description").is_none());
    assert!(facet.get("summary").is_none());
}

#[test]
fn a_malformed_or_undeclared_facet_directory_is_skipped_not_fatal() {
    let journal = fixture();
    let malformed = journal.path().join("facets").join("gamma");
    fs::create_dir_all(&malformed).unwrap();
    fs::write(malformed.join("facet.json"), "not json").unwrap();
    let undeclared = journal.path().join("facets").join("delta");
    fs::create_dir_all(&undeclared).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let result = probe(&journal, "list_facets", json!({})).unwrap();
    let ids = result["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|facet| facet["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![FACET_A.to_owned(), FACET_B.to_owned()]);
}

#[test]
fn ac3_search_and_fetch_require_the_transcripts_category() {
    let journal = fixture();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query": "indexed"})),
        Err(McpProbeError::PermissionDenied)
    );
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": "not-a-reference"})),
        Err(McpProbeError::PermissionDenied)
    );
}

#[test]
fn ac15_ac16_exam_bound_is_unquantified_when_live_drops_cannot_fill_the_page() {
    let journal = fixture();
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    for idx in 1..=crate::dispatch::MAX_SEARCH_EXAMINED_ROWS as i64 {
        index
            .execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
                 VALUES ('indexed', ?1, ?2, '', 'fixture-stream', ?3, ?4, '')",
                params![PATH, DAY, STREAM, idx],
            )
            .unwrap();
    }
    drop(index);
    let assignments = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("talents/facets.json");
    fs::write(assignments, "[{\"facet\":\"beta\"}]").unwrap();

    let result = probe(&journal, "search", json!({"query": "indexed", "limit": 1})).unwrap();
    assert_eq!(result["coverage"]["live_examination_complete"], false);
    let serialized = result.to_string();
    for forbidden in ["count", "percent", "range", "remaining", "dropped"] {
        assert!(
            !serialized.contains(forbidden),
            "coverage leaked {forbidden}"
        );
    }
}

#[test]
fn ac14_unauthorized_chunks_do_not_change_visible_search_page_or_coverage() {
    let journal = fixture();
    let before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let before = visible_search_page(&before);

    let unauthorized_segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("100000_300");
    fs::create_dir_all(unauthorized_segment.join("talents")).unwrap();
    fs::write(
        unauthorized_segment.join("talents/facets.json"),
        "[{\"facet\":\"beta\"}]",
    )
    .unwrap();
    let unauthorized_path = "20260914/default/100000_300/talents/audio.md";
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed unauthorized', ?1, ?2, '', 'fixture-stream', ?3, 0, '')",
            params![unauthorized_path, DAY, STREAM],
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) \
             VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
            params![unauthorized_path],
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
            params![unauthorized_path, FACET_B],
        )
        .unwrap();
    drop(index);

    let after = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    assert_eq!(visible_search_page(&after), before);
}

#[test]
fn ac13_cursor_rejects_a_replaced_anchor_and_valid_cursor_continues_without_repeats() {
    let journal = fixture();
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed next', ?1, ?2, '', 'fixture-stream', ?3, 1, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    drop(index);

    let first = probe(&journal, "search", json!({"query": "indexed", "limit": 1})).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    let second = probe(
        &journal,
        "search",
        json!({"query": "indexed", "limit": 1, "cursor": cursor}),
    )
    .unwrap();
    assert_ne!(
        first["results"][0]["snippet"],
        second["results"][0]["snippet"]
    );

    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index.execute("DELETE FROM chunks", []).unwrap();
    index
        .execute(
            "INSERT INTO chunks(rowid, content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES (2, 'replacement indexed bytes', ?1, ?2, '', 'fixture-stream', ?3, 1, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    drop(index);
    assert_eq!(
        probe(
            &journal,
            "search",
            json!({"query": "indexed", "limit": 1, "cursor": cursor}),
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn ac13_cursor_rejects_a_reset_rebuild_with_rowids_restarted_at_one() {
    let journal = fixture();
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed next', ?1, ?2, '', 'fixture-stream', ?3, 1, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    drop(index);
    let first = probe(&journal, "search", json!({"query": "indexed", "limit": 1})).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();

    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index.execute("DELETE FROM chunks", []).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed next', ?1, ?2, '', 'fixture-stream', ?3, 1, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('indexed bytes', ?1, ?2, '', 'fixture-stream', ?3, 0, '')",
            params![PATH, DAY, STREAM],
        )
        .unwrap();
    let row_ids = index
        .prepare("SELECT rowid FROM chunks ORDER BY rowid")
        .unwrap()
        .query_map([], |row| row.get::<_, i64>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(row_ids, [1, 2]);
    drop(index);

    assert_eq!(
        probe(
            &journal,
            "search",
            json!({"query": "indexed", "limit": 1, "cursor": cursor}),
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn ac28_generation_recheck_refuses_a_narrowed_permission_before_release() {
    let journal = fixture();
    let PermissionDecision::Snapshot(snapshot) =
        evaluate_connection_read(journal.path(), CONNECTION)
    else {
        panic!("fixture permission is enforceable");
    };
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert!(!crate::dispatch::permission_generation_is_current(
        journal.path(),
        CONNECTION,
        &snapshot,
    ));
}

#[test]
fn ac24_get_transcript_pages_a_huge_segment_without_returning_the_whole_file() {
    let journal = fixture();
    allow_legacy_markdown(&journal);
    let transcript = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("meeting_transcript.md");
    fs::write(&transcript, "pageable transcript\n".repeat(10_000)).unwrap();
    let listed = probe(&journal, "list_transcripts", json!({})).unwrap();
    let reference = listed["transcripts"][0]["reference"].as_str().unwrap();
    let page = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
    assert!(page["entries"].as_array().unwrap().len() <= 100);
    assert!(page["next_cursor"].is_string());
}

#[test]
fn get_transcript_reads_recorded_speech_from_audio_jsonl() {
    let journal = fixture();
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::remove_file(segment.join("meeting_transcript.md")).unwrap();
    fs::write(
        segment.join("audio.jsonl"),
        "{\"source\":\"recording\"}\n{\"start\":\"00:00:02\",\"text\":\"spoken words\"}\n",
    )
    .unwrap();

    let listed = probe(&journal, "list_transcripts", json!({})).unwrap();
    let reference = listed["transcripts"][0]["reference"].as_str().unwrap();
    let page = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
    assert_eq!(
        literal_display_text(page["entries"][0].as_str().unwrap()),
        "[00:00:02] spoken words"
    );
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert!(page["next_cursor"].is_null());
}

#[test]
fn legacy_image_markup_is_literal_in_search_and_every_transcript_page() {
    let journal = fixture();
    allow_legacy_markdown(&journal);
    let payload = "![legacycover](https://example.test/cover.png) <img src=x> &amp; 日本語";
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index
        .execute(
            "UPDATE chunks SET content = ?1 WHERE path = ?2",
            params![payload, PATH],
        )
        .unwrap();
    drop(index);
    let response = probe(&journal, "search", json!({"query":"legacycover"})).unwrap();
    let hit = &response["results"][0];
    assert_eq!(
        literal_display_text(hit["snippet"].as_str().unwrap()),
        payload
    );
    let text_channel = crate::jsonrpc::tool_result(response.clone(), None);
    literal_display_text(text_channel["content"][0]["text"].as_str().unwrap());
    assert_eq!(
        visible_search_page(&response),
        visible_search_page(&probe(&journal, "search", json!({"query":"legacycover"})).unwrap())
    );
    assert!(probe(&journal, "fetch", json!({"reference": hit["reference"]})).is_ok());

    let transcript = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("meeting_transcript.md");
    // Put an opening fence on the last row of a page. Each next-page entry
    // must be literal without relying on the preceding page's fence.
    let source = format!(
        "{}\n```markdown\n{payload}\n```\n",
        "ordinary line\n".repeat(99).trim_end()
    );
    fs::write(&transcript, &source).unwrap();
    let listed = probe(&journal, "list_transcripts", json!({})).unwrap();
    let mut args = json!({"reference":listed["transcripts"][0]["reference"]});
    let mut visible_entries = Vec::new();
    let mut pages = 0;
    loop {
        let page = probe(&journal, "get_transcript", args.clone()).unwrap();
        pages += 1;
        for entry in page["entries"].as_array().unwrap() {
            visible_entries.push(literal_display_text(entry.as_str().unwrap()));
        }
        let text_channel = crate::jsonrpc::tool_result(page.clone(), None);
        literal_display_text(text_channel["content"][0]["text"].as_str().unwrap());
        if page["next_cursor"].is_null() {
            break;
        }
        args["cursor"] = page["next_cursor"].clone();
    }
    assert_eq!(pages, 2);
    assert_eq!(visible_entries, source.lines().collect::<Vec<_>>());
    assert_eq!(fs::read_to_string(&transcript).unwrap(), source);
}

#[test]
fn fetched_documents_and_activity_records_keep_journal_markup_literal() {
    let payload = "![legacycover](https://example.test/cover.png) <img src=x> 日本語";
    let journal = fixture();
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    index
        .execute(
            "UPDATE chunks SET content = ?1 WHERE path = ?2",
            params![payload, PATH],
        )
        .unwrap();
    drop(index);
    let hit =
        probe(&journal, "search", json!({"query":"legacycover"})).unwrap()["results"][0].clone();
    let document = probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
    assert_eq!(
        literal_display_text(document["text"].as_str().unwrap()),
        payload
    );
    let served = crate::jsonrpc::literal_tool_result(document, None);
    literal_display_text(served["content"][0]["text"].as_str().unwrap());

    let journal = tempfile::Builder::new()
        .prefix("activity-literal-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_activity(
        journal.path(),
        "facets/alpha/activities/20261010.jsonl",
        &format!(
            "{}\n",
            json!({"id":"one","title":payload,"story":{"body":"literalstoryneedle"}})
        ),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_whole_journal(journal.path());
    let hits = activity_hits_for(&journal, "literalstoryneedle");
    assert_eq!(hits.len(), 1);
    let record = probe(
        &journal,
        "fetch",
        json!({"reference": hits[0]["reference"]}),
    )
    .unwrap();
    let text = record["text"].as_str().unwrap();
    // The record stays JSON an agent can parse, and inert if shown whole.
    literal_display_text(text);
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["id"], "one");
    assert_eq!(
        literal_display_text(parsed["title"].as_str().unwrap()),
        payload
    );
    let served = crate::jsonrpc::literal_tool_result(record, None);
    literal_display_text(served["content"][0]["text"].as_str().unwrap());
}

#[test]
fn transcript_local_names_page_under_a_transcripts_only_facet_grant() {
    let journal = fixture();
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::remove_file(segment.join("meeting_transcript.md")).unwrap();
    let mut text = String::from("{\"raw\":\"audio.flac\"}\n\ninvalid\n");
    for id in 1..=99 {
        text.push_str(&format!(
            "{{\"start\":\"00:00:01\",\"speaker\":1,\"text\":\"sentence {id}\"}}\n"
        ));
    }
    // This occupies ordinal 100, although the owner view skips its non-string start.
    text.push_str("{\"start\":null,\"text\":\"non-string start\"}\n");
    for id in 101..=103 {
        text.push_str(&format!("{{\"start\":\"00:00:01\",\"sentence_id\":999,\"speaker\":1,\"text\":\"sentence {id}\"}}\n"));
    }
    fs::write(segment.join("audio.jsonl"), text).unwrap();
    for (id, name) in [("local-person-a", "Ada"), ("local-person-b", "Bea")] {
        let dir = journal.path().join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("entity.json"),
            json!({"id":id,"name":name,"type":"Person",
            "description":"private profile", "aka":["private alias"]})
            .to_string(),
        )
        .unwrap();
    }
    let owner = journal.path().join("entities/local-owner");
    fs::create_dir_all(&owner).unwrap();
    fs::write(
        owner.join("entity.json"),
        json!({"id":"local-owner","name":"Olu","type":"Person","is_principal":true}).to_string(),
    )
    .unwrap();
    fs::write(
        segment.join("talents/speaker_labels.json"),
        json!({"labels":[
            {"sentence_id":99,"speaker":"local-person-a"},
            {"sentence_id":101,"speaker":"local-person-b","confidence":"high","method":"acoustic"},
            {"sentence_id":102,"speaker":"local-person-a"}
        ]})
        .to_string(),
    )
    .unwrap();
    fs::create_dir_all(journal.path().join("awareness")).unwrap();
    fs::write(journal.path().join("awareness/speaker_candidates.json"), json!({"next_id":2,"candidates":[{
        "cand_id":1,"status":"confirmed","confirmed_entity":"local-person-b","source_segments":[{"day":DAY,"stream_layout":"named",
        "stream":STREAM,"segment_key":SEGMENT,"source":"audio","cluster_label":1,"sentence_ids":[103]}]
    }]}).to_string()).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".into()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.into()],
                },
            },
        )
        .unwrap();
    let listed = probe(&journal, "list_transcripts", json!({})).unwrap();
    let reference = listed["transcripts"][0]["reference"].as_str().unwrap();
    let first = probe(&journal, "get_transcript", json!({"reference":reference})).unwrap();
    // Only a voice-identified label names a speaker; a label without one names no one.
    assert_eq!(
        literal_display_text(first["entries"][98].as_str().unwrap()),
        "[00:00:01] Speaker 1: sentence 99"
    );
    let second = probe(
        &journal,
        "get_transcript",
        json!({"reference":reference,"cursor":first["next_cursor"]}),
    )
    .unwrap();
    assert_eq!(
        second["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| literal_display_text(entry.as_str().unwrap()))
            .collect::<Vec<_>>(),
        vec![
            "[00:00:01] Bea: sentence 101",
            "[00:00:01] Speaker 1: sentence 102",
            "[00:00:01] Speaker 1: sentence 103"
        ]
    );
    assert!(second["next_cursor"].is_null());
    for forbidden in [
        "local-person",
        "private profile",
        "private alias",
        "confidence",
        "acoustic",
    ] {
        assert!(!second.to_string().contains(forbidden));
        assert!(
            second["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| !literal_display_text(entry.as_str().unwrap()).contains(forbidden))
        );
    }
    assert_eq!(
        probe(&journal, "list_entities", json!({})),
        Err(McpProbeError::PermissionDenied)
    );
    for assignments in [
        json!([{"facet":"beta"}]),
        json!([{"facet":"alpha"},{"facet":"beta"}]),
        json!([]),
    ] {
        fs::write(segment.join("talents/facets.json"), assignments.to_string()).unwrap();
        assert_eq!(
            probe(&journal, "get_transcript", json!({"reference":reference})),
            Err(McpProbeError::Unavailable)
        );
    }
}

#[test]
fn ac7_ac21_ac22_entities_are_scoped_stable_and_omit_forbidden_fields() {
    let journal = fixture();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let listed = probe(&journal, "list_entities", json!({})).unwrap();
    assert_eq!(listed["entities"].as_array().unwrap().len(), 2);
    let reference = listed["entities"][0]["reference"].as_str().unwrap();
    let entity = probe(&journal, "get_entity", json!({"reference": reference})).unwrap();
    // Photos of people are on by default, so the entity says it has no photo.
    assert_eq!(
        entity,
        json!({"name": "Same Name", "notes": [], "photo": {"state": "none"}})
    );
    let serialized = entity.to_string();
    for forbidden in ["aka", "out-of-scope", "detached", "aggregate", "decision"] {
        assert!(!serialized.contains(forbidden));
    }
}

#[test]
fn ac19_ac20_all_agent_results_omit_coordinates_and_keep_index_coverage_statement() {
    let journal = fixture();
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let facets = probe(&journal, "list_facets", json!({})).unwrap();
    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let fetch = probe(
        &journal,
        "fetch",
        json!({"reference": search["results"][0]["reference"]}),
    )
    .unwrap();
    let transcripts = probe(&journal, "list_transcripts", json!({})).unwrap();
    let transcript = probe(
        &journal,
        "get_transcript",
        json!({"reference": transcripts["transcripts"][0]["reference"]}),
    )
    .unwrap();
    let entities = probe(&journal, "list_entities", json!({})).unwrap();
    let entity = probe(
        &journal,
        "get_entity",
        json!({"reference": entities["entities"][0]["reference"]}),
    )
    .unwrap();

    for result in [
        &facets,
        &search,
        &fetch,
        &transcripts,
        &transcript,
        &entities,
        &entity,
    ] {
        assert!(!result.to_string().contains("20260914/default/090000_300"));
        assert_no_coordinate_keys(result);
    }
    assert!(search["coverage"]["transcript_index"].is_string());
}

#[test]
fn ac6_delete_recreate_and_ac9_rename_keep_stable_facet_ids_real() {
    let journal = fixture();
    let alpha = journal.path().join("facets/alpha");
    fs::rename(&alpha, journal.path().join("facets/alpha-renamed")).unwrap();
    assert_eq!(
        probe(&journal, "list_entities", json!({})),
        Err(McpProbeError::PermissionDenied)
    );
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "list_entities", json!({})).unwrap()["entities"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        probe(&journal, "search", json!({"query":"indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(journal.path().join("facets/alpha-renamed")).unwrap();
    fs::create_dir_all(&alpha).unwrap();
    fs::write(
        alpha.join("facet.json"),
        json!({"id": FACET_B, "title":"Alpha"}).to_string(),
    )
    .unwrap();
    assert!(
        probe(&journal, "list_facets", json!({})).unwrap()["facets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

fn grant(journal: &tempfile::TempDir, scope: ReadScope) {
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned(), "transcripts".to_owned()],
                scope,
            },
        )
        .unwrap();
}

fn reachable(journal: &tempfile::TempDir) -> (usize, usize) {
    let search = probe(journal, "search", json!({"query":"indexed"}))
        .map(|value| value["results"].as_array().map_or(0, Vec::len))
        .unwrap_or(0);
    let transcripts = probe(journal, "list_transcripts", json!({}))
        .map(|value| value["transcripts"].as_array().map_or(0, Vec::len))
        .unwrap_or(0);
    (search, transcripts)
}

fn reconcile(journal: &tempfile::TempDir) {
    let mut snapshot = || {
        solstone_core_indexer_store::classification::FacetDeclarationSet::from_journal(
            journal.path(),
        )
    };
    solstone_core_indexer_store::reconcile::reconcile_stale_classifications(
        journal.path(),
        &mut snapshot,
    )
    .unwrap();
}

#[test]
fn a_merged_facet_name_reaches_the_survivor_in_search_and_transcripts_and_never_its_old_grant() {
    let journal = fixture();
    // What `journal facet merge alpha --into beta` leaves: alpha's folder is
    // gone and its name resolves to beta. The index was built before it.
    fs::remove_dir_all(journal.path().join("facets/alpha")).unwrap();
    fs::write(
        journal.path().join("facets/retired.json"),
        json!({"names": {"alpha": {"state": "merged", "id": FACET_A, "successor": FACET_B}}})
            .to_string(),
    )
    .unwrap();
    grant(
        &journal,
        ReadScope::Facets {
            ids: vec![FACET_B.to_owned()],
        },
    );
    // Transcripts re-check live, so they follow at once; search needs the
    // stored classification brought up to date.
    assert_eq!(reachable(&journal), (0, 1));
    reconcile(&journal);
    assert_eq!(reachable(&journal), (1, 1));
    grant(
        &journal,
        ReadScope::Facets {
            ids: vec![FACET_A.to_owned()],
        },
    );
    assert_eq!(reachable(&journal), (0, 0));
    grant(
        &journal,
        ReadScope::Facets {
            ids: vec!["123e4567-e89b-42d3-a456-426614174009".to_owned()],
        },
    );
    assert_eq!(reachable(&journal), (0, 0));
}

#[test]
fn a_deleted_facet_waits_in_the_trash_and_its_name_can_be_taken_again() {
    let journal = fixture();
    assert!(
        solstone_core_facets::delete_facet(
            &solstone_core_removals::Removal::begin(
                journal.path(),
                solstone_core_removals::Action::FacetDelete,
                solstone_core_removals::Surface::Cli
            ),
            "alpha"
        )
        .unwrap()
    );
    // While it waits, a grant for it reaches nothing.
    reconcile(&journal);
    grant(
        &journal,
        ReadScope::Facets {
            ids: vec![FACET_A.to_owned()],
        },
    );
    assert_eq!(reachable(&journal), (0, 0));
    // A new facet may take the name; what names `alpha` is its now, as the
    // name with no folder named no facet before.
    solstone_core_facets::create_facet(journal.path(), "alpha", "Alpha", "", "", "", None).unwrap();
    let new_id = solstone_core_facets::read_facet_declaration(journal.path(), "alpha")
        .unwrap()
        .unwrap()
        .value()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(new_id, FACET_A);
    reconcile(&journal);
    grant(&journal, ReadScope::Facets { ids: vec![new_id] });
    let scoped = reachable(&journal);
    grant(&journal, ReadScope::WholeJournal);
    assert_eq!(scoped, reachable(&journal));
}

#[test]
fn ac8_ac11_ac12_ac30_bad_live_scope_and_references_fail_closed() {
    let journal = fixture();
    let search = probe(&journal, "search", json!({"query":"indexed"})).unwrap();
    let reference = search["results"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference":"notes/a:b.txt:42"})),
        Err(McpProbeError::Unavailable)
    );
    let tampered = probe(&journal, "fetch", json!({"reference":"tampered"}));
    assert_eq!(tampered, Err(McpProbeError::Unavailable));
    PermissionStore::open(journal.path())
        .set_permission("bearer:other", ReadPermission::default_whole_journal())
        .unwrap();
    assert_eq!(
        run_mcp_probe(
            journal.path(),
            "bearer:other",
            "fetch",
            &json!({"reference":reference})
        ),
        Err(McpProbeError::Unavailable)
    );
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        tampered
    );
    let facets = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT)
        .join("talents/facets.json");
    fs::write(facets, "not json").unwrap();
    assert!(
        probe(&journal, "list_transcripts", json!({})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        probe(&journal, "search", json!({"query":"indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    PermissionStore::open(journal.path())
        .clear_permission(CONNECTION)
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query":"indexed"})),
        Err(McpProbeError::PermissionDenied)
    );
}

fn result_count(result: &Value, key: &str) -> usize {
    result[key].as_array().unwrap().len()
}

#[test]
fn an_iso_day_finds_what_the_journal_day_key_finds() {
    // Hosted agents send ISO dates. Before this, `2026-09-14` reached the index
    // as a string comparison and every entry fell outside it: a full journal
    // read back as "nothing matched".
    let journal = fixture();
    for (day_key, iso) in [
        ("day", "day"),
        ("day_from", "day_from"),
        ("day_to", "day_to"),
    ] {
        let keyed = probe(
            &journal,
            "search",
            json!({"query": "indexed", day_key: DAY}),
        )
        .unwrap();
        let dashed = probe(
            &journal,
            "search",
            json!({"query": "indexed", iso: "2026-09-14"}),
        )
        .unwrap();
        assert_eq!(result_count(&keyed, "results"), 1, "{day_key}");
        assert_eq!(
            visible_search_page(&keyed),
            visible_search_page(&dashed),
            "{iso}"
        );
    }
    assert_eq!(
        result_count(
            &probe(&journal, "list_transcripts", json!({"day": "2026-09-14"})).unwrap(),
            "transcripts"
        ),
        1
    );
}

#[test]
fn a_day_that_is_not_a_calendar_day_is_refused_rather_than_matching_nothing() {
    let journal = fixture();
    for day in [
        "Sept 14",
        "2026-9-14",
        "20261340",
        "2026-02-30",
        "14/09/2026",
    ] {
        assert_eq!(
            probe(&journal, "search", json!({"query": "indexed", "day": day})),
            Err(McpProbeError::InvalidInput),
            "{day}"
        );
        assert_eq!(
            probe(&journal, "list_transcripts", json!({"day": day})),
            Err(McpProbeError::InvalidInput),
            "{day}"
        );
    }
}

#[test]
fn a_sentence_with_no_whole_match_falls_back_to_its_words_and_says_so() {
    let journal = fixture();
    let strict = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    assert_eq!(strict["relaxed"], false);
    let sentence = probe(
        &journal,
        "search",
        json!({"query": "what indexed notes did I write about the launch"}),
    )
    .unwrap();
    assert_eq!(result_count(&sentence, "results"), 1);
    assert_eq!(sentence["relaxed"], true);
    let absent = probe(
        &journal,
        "search",
        json!({"query": "zzzznotpresent qqqqmissing"}),
    )
    .unwrap();
    assert_eq!(result_count(&absent, "results"), 0);
}

#[test]
fn get_entity_says_who_someone_is_from_the_granted_facet_only() {
    // An agent asked "who is X" needs the facet's description and notes, not
    // just a name. ⛔ The same entity's notes in another facet, and its
    // journal-level description and aliases, stay out.
    let journal = fixture();
    let root = journal.path();
    for (facet, description, note) in [
        ("alpha", "Advisor in alpha", "alpha note about coffee"),
        ("beta", "Something only beta knows", "beta-only secret note"),
    ] {
        let link = root.join("facets").join(facet).join("entities/primary");
        fs::create_dir_all(&link).unwrap();
        fs::write(
            link.join("entity.json"),
            json!({"entity_id": "entity-primary", "description": description, "last_seen": "20260913"})
                .to_string(),
        )
        .unwrap();
        fs::write(
            link.join("observations.jsonl"),
            format!("{{\"content\":\"{note}\"}}\n"),
        )
        .unwrap();
    }
    fs::write(
        root.join("entities/primary/entity.json"),
        json!({"id": "entity-primary", "name": "Same Name", "type": "Person",
               "aka": ["forbidden aka"], "description": "out-of-scope description"})
        .to_string(),
    )
    .unwrap();
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let listed = probe(&journal, "list_entities", json!({})).unwrap();
    let primary = listed["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entity| {
            probe(
                &journal,
                "get_entity",
                json!({"reference": entity["reference"]}),
            )
            .unwrap()
        })
        .find(|entity| entity["description"] == "Advisor in alpha")
        .expect("the alpha relationship is served");
    assert_eq!(primary["type"], "Person");
    assert_eq!(primary["last_seen"], "20260913");
    assert_eq!(primary["notes"], json!(["alpha note about coffee"]));
    let serialized = primary.to_string();
    for forbidden in ["beta", "secret", "aka", "out-of-scope"] {
        assert!(
            !serialized.contains(forbidden),
            "{forbidden} leaked: {serialized}"
        );
    }
}

fn setup_facets(journal: &Path) {
    for (name, id, title) in [("alpha", FACET_A, "Alpha"), ("beta", FACET_B, "Beta")] {
        let directory = journal.join("facets").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("facet.json"),
            json!({"id": id, "title": title, "description": format!("{title} details")})
                .to_string(),
        )
        .unwrap();
    }
}

#[allow(clippy::too_many_arguments)]
fn plant_chunk(
    journal: &Path,
    path: &str,
    day: &str,
    stream: &str,
    idx: i64,
    content: &str,
    category: &str,
    basis: &str,
    facet_ids: &[&str],
) {
    let index = solstone_core_indexer_store::db::open_index(journal).unwrap();
    index
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES (?1, ?2, ?3, '', 'fixture-stream', ?4, ?5, '')",
            params![content, path, day, stream, idx],
        )
        .unwrap();
    index
        .execute(
            "INSERT OR REPLACE INTO chunk_classification(path, category, basis, eligible, unclassified) \
             VALUES (?1, ?2, ?3, 1, 0)",
            params![path, category, basis],
        )
        .unwrap();
    for facet_id in facet_ids {
        index
            .execute(
                "INSERT OR REPLACE INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
                params![path, facet_id],
            )
            .unwrap();
    }
    index
        .execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) \
             VALUES (1, '', 1, 0, NULL, 0)",
            [],
        )
        .unwrap();
    index
        .execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) \
             VALUES (1, 1, 'complete', 1, 1)",
            [],
        )
        .unwrap();
}

fn plant_activity_proof(journal: &Path, path: &str, idx: i64, hidden: bool) {
    let bytes = fs::read(journal.join(path)).unwrap();
    let digest = Sha256::digest(&bytes);
    let index = solstone_core_indexer_store::db::open_index(journal).unwrap();
    let rowid: i64 = index
        .query_row(
            "SELECT rowid FROM chunks WHERE path = ?1 AND idx = ?2",
            params![path, idx],
            |row| row.get(0),
        )
        .unwrap();
    index
        .execute(
            "INSERT INTO activity_provenance(rowid, digest, hidden) VALUES (?1, ?2, ?3)",
            params![rowid, digest.as_slice(), i64::from(hidden)],
        )
        .unwrap();
}

fn indexed_content(journal: &Path, path: &str, idx: i64) -> String {
    let index = solstone_core_indexer_store::db::open_index(journal).unwrap();
    index
        .query_row(
            "SELECT content FROM chunks WHERE path = ?1 AND idx = ?2",
            params![path, idx],
            |row| row.get(0),
        )
        .unwrap()
}

fn setup_shared_link_journal() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("link-test-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let chronicle = journal.path().join("chronicle").join("20261010");

    let alpha_seg = chronicle.join("alpha").join("090000_300");
    fs::create_dir_all(alpha_seg.join("talents")).unwrap();
    fs::write(
        alpha_seg.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        alpha_seg.join("meeting_transcript.md"),
        "alpha_transcript_token\n",
    )
    .unwrap();
    fs::write(alpha_seg.join("talents/audio.md"), "brief bytes\n").unwrap();
    fs::write(
        alpha_seg.join("stream.json"),
        json!({"stream": "not-alpha"}).to_string(),
    )
    .unwrap();

    let beta_seg = chronicle.join("beta").join("090000_300");
    fs::create_dir_all(beta_seg.join("talents")).unwrap();
    fs::write(
        beta_seg.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        beta_seg.join("meeting_transcript.md"),
        "beta_transcript_token\n",
    )
    .unwrap();
    fs::write(beta_seg.join("talents/audio.md"), "brief bytes\n").unwrap();

    let flat_dup = chronicle.join("090000_300");
    fs::create_dir_all(flat_dup.join("talents")).unwrap();
    fs::write(
        flat_dup.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        flat_dup.join("meeting_transcript.md"),
        "flat_dup_transcript_token\n",
    )
    .unwrap();

    let flat_only = chronicle.join("100000_300");
    fs::create_dir_all(flat_only.join("talents")).unwrap();
    fs::write(
        flat_only.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        flat_only.join("meeting_transcript.md"),
        "flat_only_transcript_token\n",
    )
    .unwrap();

    let x_seg = chronicle.join("alpha").join("x-120000_300");
    fs::create_dir_all(x_seg.join("talents")).unwrap();
    fs::write(x_seg.join("talents/facets.json"), "[{\"facet\":\"alpha\"}]").unwrap();
    fs::write(x_seg.join("meeting_transcript.md"), "x_transcript_token\n").unwrap();

    journal
}

fn find_audit_files(journal: &Path, filename: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let chronicle = journal.join("chronicle");
    if let Ok(days) = fs::read_dir(chronicle) {
        for day in days.filter_map(Result::ok) {
            let stream_dir = day.path().join("mcp.agent");
            if let Ok(segs) = fs::read_dir(stream_dir) {
                for seg in segs.filter_map(Result::ok) {
                    let file_path = seg.path().join(filename);
                    if file_path.is_file() {
                        files.push(file_path);
                    }
                }
            }
        }
    }
    files.sort();
    files
}

struct CountingChecker {
    count: usize,
}

impl TranscriptPageCheck for CountingChecker {
    fn check_first_page(
        &mut self,
        _journal_root: &Path,
        _segment: &solstone_core_journal_io::paths::Segment,
    ) -> Result<Option<u64>, ()> {
        self.count += 1;
        Ok(Some(12345))
    }
}

#[test]
fn a_summary_hit_links_only_to_its_own_streams_segment() {
    let journal = setup_shared_link_journal();
    let path = "20261010/beta/090000_300/talents/audio.md";
    plant_chunk(
        journal.path(),
        path,
        "20261010",
        "",
        0,
        "sharedneedle1 test 1 beta hit",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle1", "limit": 10}),
    )
    .unwrap();
    let results = search["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let hit = &results[0];
    let segments = hit["segments"].as_array().expect("hit has segments");
    assert_eq!(segments.len(), 1);
    let link = &segments[0];
    assert_eq!(link["time"], "09:00-09:05");
    assert!(link["reference"].is_string());
    assert!(link["source"].is_string());
    assert_eq!(link.as_object().unwrap().len(), 4);
    assert!(link["link"].is_string());

    let transcript = probe(
        &journal,
        "get_transcript",
        json!({"reference": link["reference"]}),
    )
    .unwrap();
    let entries = transcript["entries"].as_array().unwrap();
    let text: String = entries
        .iter()
        .map(|e| literal_display_text(e.as_str().unwrap()))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(text.contains("beta_transcript_token"));
    assert!(!text.contains("alpha_transcript_token"));
}

#[test]
fn activity_hits_link_their_segments_within_the_per_result_budget() {
    let journal = setup_shared_link_journal();
    let chronicle = journal.path().join("chronicle/20261010");
    for (stream, name, token) in [
        ("alpha", "110000_300", "alpha_11_token"),
        ("beta", "110000_300", "beta_11_token"),
    ] {
        let seg = chronicle.join(stream).join(name);
        fs::create_dir_all(seg.join("talents")).unwrap();
        fs::write(seg.join("talents/facets.json"), "[{\"facet\":\"alpha\"}]").unwrap();
        fs::write(seg.join("meeting_transcript.md"), format!("{token}\n")).unwrap();
    }

    let act_path = "facets/alpha/activities/20261010.jsonl";
    let line0 =
        json!({"id": "act-stream-beta", "stream": "beta", "segments": ["090000_300"]}).to_string();
    let line1 = json!({
        "id": "act-no-stream",
        "segments": ["090000_300", "100000_300", "110000_300", "120000_300"]
    })
    .to_string();
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    fs::write(journal.path().join(act_path), format!("{line0}\n{line1}\n")).unwrap();

    let produced0 = produce_chunks(Family::Activity, act_path, &line0);
    let produced1 = produce_chunks(Family::Activity, act_path, &line1);
    let content0 = format!("{}\nsharedneedle2", produced0.chunks[0].content);
    let content1 = format!("{}\nsharedneedle2", produced1.chunks[0].content);

    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        0,
        &content0,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        1,
        &content1,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);
    plant_activity_proof(journal.path(), act_path, 1, false);

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle2", "limit": 10}),
    )
    .unwrap();
    let results = search["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);

    let hit0 = results
        .iter()
        .find(|h| literal_display_text(h["snippet"].as_str().unwrap()).contains("Act-stream-beta"))
        .unwrap();
    let segments0 = hit0["segments"].as_array().expect("hit0 has segments");
    assert_eq!(segments0.len(), 1);
    assert!(hit0.get("more_segments").is_none());
    let t0 = probe(
        &journal,
        "get_transcript",
        json!({"reference": segments0[0]["reference"]}),
    )
    .unwrap();
    let entries0: String = t0["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| literal_display_text(e.as_str().unwrap()))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(entries0.contains("beta_transcript_token"));
    assert!(!entries0.contains("alpha_transcript_token"));

    let hit1 = results
        .iter()
        .find(|h| literal_display_text(h["snippet"].as_str().unwrap()).contains("Act-no-stream"))
        .unwrap();
    let segments1 = hit1["segments"].as_array().expect("hit1 has segments");
    assert_eq!(segments1.len(), 3);
    assert_eq!(hit1["more_segments"], true);

    let mut all_texts = Vec::new();
    for seg in segments1 {
        let t = probe(
            &journal,
            "get_transcript",
            json!({"reference": seg["reference"]}),
        )
        .unwrap();
        all_texts.push(
            t["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| literal_display_text(e.as_str().unwrap()))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    let combined = all_texts.join(" ");
    assert!(combined.contains("alpha_transcript_token"));
    assert!(combined.contains("beta_transcript_token"));
    assert!(combined.contains("flat_only_transcript_token"));
    assert!(!combined.contains("flat_dup_transcript_token"));
    assert!(!combined.contains("alpha_11_token"));
}

#[test]
fn activity_links_require_the_live_record_to_match_the_indexed_one() {
    let journal = setup_shared_link_journal();
    let act_path = "facets/alpha/activities/20261010.jsonl";
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();

    let line0 = json!({"id": "launch-sync", "segments": ["090000_300"]}).to_string();
    let line1 =
        json!({"id": "twin-different", "title": "Actual Title", "segments": ["090000_300"]})
            .to_string();
    let line2 =
        json!({"id": "twin-unrewritten", "title": "Twin", "segments": ["090000_300"]}).to_string();

    let file_content = format!("\n{line0}\n{line1}\n{line2}\n");
    fs::write(journal.path().join(act_path), &file_content).unwrap();

    let p0 = produce_chunks(Family::Activity, act_path, &line0);
    let time_line = p0.chunks[0]
        .content
        .lines()
        .find(|l| l.starts_with("- Time:"))
        .unwrap();
    let content0 =
        format!("### Launch-Sync\n{time_line}\n- Level: high\nSaved 2 items\nsharedneedle3");
    let content1 = format!("### Mismatched Title\n{time_line}\nsharedneedle3");
    let content2 = format!("### Twin\n{time_line}\nsharedneedle3");

    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        0,
        &content0,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        1,
        &content1,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        2,
        &content2,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);
    plant_activity_proof(journal.path(), act_path, 1, false);
    plant_activity_proof(journal.path(), act_path, 2, false);

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle3", "limit": 10}),
    )
    .unwrap();
    let results = search["results"].as_array().unwrap();
    let hit_launch = results
        .iter()
        .find(|h| literal_display_text(h["snippet"].as_str().unwrap()).contains("Launch-Sync"))
        .unwrap();
    let hit_mismatch = results
        .iter()
        .find(|h| literal_display_text(h["snippet"].as_str().unwrap()).contains("Mismatched Title"))
        .unwrap();
    let hit_twin = results
        .iter()
        .find(|h| literal_display_text(h["snippet"].as_str().unwrap()).contains("Twin"))
        .unwrap();
    assert_eq!(
        literal_display_text(hit_launch["snippet"].as_str().unwrap()),
        content0
    );
    assert_eq!(
        literal_display_text(hit_mismatch["snippet"].as_str().unwrap()),
        content1
    );
    assert_eq!(
        literal_display_text(hit_twin["snippet"].as_str().unwrap()),
        content2
    );
    assert_eq!(hit_launch["hidden"], false);
    assert_eq!(hit_mismatch["hidden"], false);
    assert_eq!(hit_twin["hidden"], false);
    assert!(hit_launch.get("segments").is_some());
    assert!(hit_mismatch.get("segments").is_some());
    assert!(hit_twin.get("segments").is_some());
    let launch_reference = hit_launch["reference"].as_str().unwrap().to_owned();
    let fetched = probe(&journal, "fetch", json!({"reference": launch_reference})).unwrap();
    let launch_obj: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(launch_obj["id"], "launch-sync");
    assert_eq!(launch_obj["segments"], json!(["090000_300"]));
    assert!(launch_obj.get("stream").is_none());
    assert_eq!(fetched["hidden"], false);

    let line0_new =
        json!({"id": "completely-different-id", "segments": ["090000_300"]}).to_string();
    let rewritten_content = format!("\n{line0_new}\n{line1}\n{line2}\n");
    fs::write(journal.path().join(act_path), &rewritten_content).unwrap();

    let search2 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle3", "limit": 10}),
    )
    .unwrap();
    assert!(search2["results"].as_array().unwrap().is_empty());
    assert_eq!(search2["coverage"]["live_examination_complete"], false);
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": launch_reference})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn search_narrowing_does_not_change_which_links_are_issued() {
    let journal = setup_shared_link_journal();
    let act_path = "facets/alpha/activities/20261010.jsonl";
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    let line0 = json!({"id": "launch-sync", "segments": ["090000_300"]}).to_string();
    fs::write(journal.path().join(act_path), format!("{line0}\n")).unwrap();
    let p0 = produce_chunks(Family::Activity, act_path, &line0);
    let content0 = format!("{}\nsharedneedle4", p0.chunks[0].content);
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        0,
        &content0,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();

    let s1 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle4", "category": "facets", "limit": 10}),
    )
    .unwrap();
    let hit1 = &s1["results"].as_array().unwrap()[0];
    let segs1 = hit1["segments"].as_array().expect("hit1 has segments");
    assert!(!segs1.is_empty());

    let s2 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle4", "facet": FACET_A, "limit": 10}),
    )
    .unwrap();
    let hit2 = &s2["results"].as_array().unwrap()[0];
    let segs2 = hit2["segments"].as_array().expect("hit2 has segments");
    assert!(!segs2.is_empty());

    let t = probe(
        &journal,
        "get_transcript",
        json!({"reference": segs2[0]["reference"]}),
    )
    .unwrap();
    assert!(!t["entries"].as_array().unwrap().is_empty());
}

#[test]
fn chosen_facets_withhold_links_to_segments_partly_outside_scope() {
    let journal = setup_shared_link_journal();
    fs::write(
        journal
            .path()
            .join("chronicle/20261010/beta/090000_300/talents/facets.json"),
        "[{\"facet\":\"alpha\"},{\"facet\":\"beta\"}]",
    )
    .unwrap();
    fs::write(
        journal
            .path()
            .join("chronicle/20261010/beta/090000_300/meeting_transcript.md"),
        "token_x_and_y\n",
    )
    .unwrap();

    let talent_path = "20261010/alpha/090000_300/talents/alpha/brief.md";
    fs::create_dir_all(
        journal
            .path()
            .join("chronicle/20261010/alpha/090000_300/talents/alpha"),
    )
    .unwrap();
    fs::write(
        journal.path().join("chronicle").join(talent_path),
        "talent bytes\n",
    )
    .unwrap();
    plant_chunk(
        journal.path(),
        talent_path,
        "20261010",
        "alpha",
        0,
        "sharedneedle5 talent hit",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );

    let act_path = "facets/alpha/activities/20261010.jsonl";
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    let line0 = json!({"id": "launch-sync", "segments": ["090000_300"]}).to_string();
    fs::write(journal.path().join(act_path), format!("{line0}\n")).unwrap();
    let p0 = produce_chunks(Family::Activity, act_path, &line0);
    let content0 = format!("{}\nsharedneedle5", p0.chunks[0].content);
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        0,
        &content0,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "transcripts".to_owned(),
                    "messages".to_owned(),
                    "facets".to_owned(),
                ],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle5", "limit": 10}),
    )
    .unwrap();
    let search_str = search.to_string();
    assert!(
        !search_str.contains("token_x_and_y"),
        "token_x_and_y leaked: {search_str}"
    );

    let results = search["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    for hit in results {
        let segs = hit["segments"].as_array().expect("hit has segments");
        assert_eq!(segs.len(), 1);
        let t = probe(
            &journal,
            "get_transcript",
            json!({"reference": segs[0]["reference"]}),
        )
        .unwrap();
        let entries: String = t["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| literal_display_text(e.as_str().unwrap()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(entries.contains("alpha_transcript_token"));
    }
}

#[test]
fn unreadable_or_deleted_segments_get_no_link_and_search_still_succeeds() {
    let journal = setup_shared_link_journal();
    let chronicle = journal.path().join("chronicle/20261010/alpha");

    let seg_screen = chronicle.join("110000_300");
    fs::create_dir_all(seg_screen.join("talents")).unwrap();
    fs::write(
        seg_screen.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(seg_screen.join("screen.png"), b"fake image bytes").unwrap();

    let seg_header = chronicle.join("120000_300");
    fs::create_dir_all(seg_header.join("talents")).unwrap();
    fs::write(
        seg_header.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        seg_header.join("conversation_transcript.jsonl"),
        "{\"imported\":{}}\n",
    )
    .unwrap();

    let seg_huge = chronicle.join("130000_300");
    fs::create_dir_all(seg_huge.join("talents")).unwrap();
    fs::write(
        seg_huge.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    let huge_line = "a".repeat(70_000);
    fs::write(
        seg_huge.join("meeting_transcript.md"),
        format!("{huge_line}\n"),
    )
    .unwrap();

    let paths = [
        "20261010/alpha/090000_300/talents/audio.md",
        "20261010/alpha/100000_300/talents/audio.md",
        "20261010/alpha/110000_300/talents/audio.md",
        "20261010/alpha/120000_300/talents/audio.md",
        "20261010/alpha/130000_300/talents/audio.md",
    ];
    for (i, p) in paths.iter().enumerate() {
        plant_chunk(
            journal.path(),
            p,
            "20261010",
            "alpha",
            i as i64,
            "sharedneedle6 hit",
            "transcripts",
            "segment_assigned",
            &[FACET_A],
        );
    }

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle6", "limit": 10}),
    )
    .unwrap();
    let results = search["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let readable_hit = &results[0];
    assert!(readable_hit.get("segments").is_some());

    let old_link = readable_hit["segments"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();
    // A link's name outlives a permission set again to the same thing.
    assert!(probe(&journal, "get_transcript", json!({"reference": old_link})).is_ok());

    let search2 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle6", "limit": 10}),
    )
    .unwrap();
    let results2 = search2["results"].as_array().unwrap();
    assert_eq!(results2.len(), 1);
    let readable_hit2 = &results2[0];
    assert!(readable_hit2.get("segments").is_some());
    let new_link = readable_hit2["segments"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::remove_dir_all(journal.path().join("chronicle/20261010/alpha/090000_300")).unwrap();
    assert_eq!(
        probe(&journal, "get_transcript", json!({"reference": new_link})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn links_keep_the_replay_digest_stable() {
    let journal = setup_shared_link_journal();
    plant_chunk(
        journal.path(),
        "20261010/beta/090000_300/talents/audio.md",
        "20261010",
        "",
        0,
        "sharedneedle7 hit",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let search1 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle7", "limit": 10}),
    )
    .unwrap();
    let _search2 = probe(
        &journal,
        "search",
        json!({"query": "sharedneedle7", "limit": 10}),
    )
    .unwrap();

    let outcomes = find_audit_files(journal.path(), solstone_core_mcp_audit::OUTCOME_FILE);
    assert_eq!(outcomes.len(), 2);
    let o1: Value = serde_json::from_str(&fs::read_to_string(&outcomes[0]).unwrap()).unwrap();
    let o2: Value = serde_json::from_str(&fs::read_to_string(&outcomes[1]).unwrap()).unwrap();

    assert_eq!(o1["result"]["digest"], o2["result"]["digest"]);
    let targets = o1["result"]["targets"].as_array().unwrap();
    assert!(!targets.is_empty());
    for t in targets {
        let t_str = t.as_str().unwrap();
        assert!(t_str.contains('#'));
        assert!(!t_str.contains("/transcript.md"));
    }

    for hit in search1["results"].as_array().unwrap() {
        if let Some(segments) = hit.get("segments").and_then(Value::as_array) {
            for seg in segments {
                assert!(seg.get("time").is_some());
                assert!(seg.get("reference").is_some());
                assert!(seg.get("source").is_some());
                assert_eq!(seg.as_object().unwrap().len(), 4);
                assert!(seg["link"].is_string());
            }
        }
    }
    assert_no_coordinate_keys(&search1);
}

#[test]
fn list_transcripts_offers_no_cursor_when_only_withheld_segments_remain() {
    let journal = tempfile::Builder::new()
        .prefix("list-withheld-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "messages".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let day = journal.path().join("chronicle/20261010");
    let write_segment = |stream: &str, name: &str, facet: &str| {
        let seg = day.join(stream).join(name);
        fs::create_dir_all(seg.join("talents")).unwrap();
        fs::write(
            seg.join("talents/facets.json"),
            format!("[{{\"facet\":\"{facet}\"}}]"),
        )
        .unwrap();
        fs::write(
            seg.join("meeting_transcript.md"),
            format!("{stream} {name}\n"),
        )
        .unwrap();
    };
    write_segment("alpha", "090000_300", "alpha");
    write_segment("beta", "100000_300", "beta");

    // The page is full and only a segment outside the scope remains: no cursor.
    let page = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "limit": 1}),
    )
    .unwrap();
    assert_eq!(page["transcripts"].as_array().unwrap().len(), 1);
    assert!(page["next_cursor"].is_null(), "{page}");

    // Positive twin: an in-scope segment remaining does earn a cursor.
    write_segment("alpha", "110000_300", "alpha");
    let page = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "limit": 1}),
    )
    .unwrap();
    assert_eq!(page["transcripts"].as_array().unwrap().len(), 1);
    assert!(page["next_cursor"].is_string(), "{page}");
}

#[test]
fn list_transcripts_filters_by_time_and_pages_with_a_bound_cursor() {
    let journal = tempfile::Builder::new()
        .prefix("list-test-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();

    let chronicle = journal.path().join("chronicle");
    for (name, start, end) in [
        ("100000_300", "10:00", "10:05"),
        ("100200_300", "10:02", "10:07"),
        ("110000_300", "11:00", "11:05"),
    ] {
        let seg = chronicle.join("20261010/default").join(name);
        fs::create_dir_all(&seg).unwrap();
        fs::write(
            seg.join("meeting_transcript.md"),
            format!("text for {start}-{end}\n"),
        )
        .unwrap();
    }

    let r_range = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "time_from": "10:05", "time_to": "11:00"}),
    )
    .unwrap();
    let tr = r_range["transcripts"].as_array().unwrap();
    assert_eq!(tr.len(), 1);
    assert_eq!(tr[0]["time"], "10:02-10:07");

    let r_empty = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "time_from": "10:05", "time_to": "10:05"}),
    )
    .unwrap();
    assert!(r_empty["transcripts"].as_array().unwrap().is_empty());

    for stream in ["alpha", "beta"] {
        let seg = chronicle.join("20261010").join(stream).join("090000_300");
        fs::create_dir_all(&seg).unwrap();
        fs::write(
            seg.join("meeting_transcript.md"),
            format!("{stream} token\n"),
        )
        .unwrap();
    }

    let p1 = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "limit": 1}),
    )
    .unwrap();
    assert_eq!(p1["transcripts"].as_array().unwrap().len(), 1);
    let cur1 = p1["next_cursor"].as_str().expect("has cursor").to_owned();

    let p2 = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "limit": 1, "cursor": cur1}),
    )
    .unwrap();
    assert_eq!(p2["transcripts"].as_array().unwrap().len(), 1);
    assert_ne!(
        p1["transcripts"][0]["reference"],
        p2["transcripts"][0]["reference"]
    );

    for day in ["20261005", "20261012", "20261001"] {
        let seg = chronicle.join(day).join("default/090000_300");
        fs::create_dir_all(&seg).unwrap();
        fs::write(seg.join("meeting_transcript.md"), "words\n").unwrap();
    }
    let all_days = probe(&journal, "list_transcripts", json!({"limit": 10})).unwrap();
    let dates: Vec<&str> = all_days["transcripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["date"].as_str().unwrap())
        .collect();
    let mut sorted_dates = dates.clone();
    sorted_dates.sort_by(|a, b| b.cmp(a));
    assert_eq!(dates, sorted_dates);

    let seg_prior = chronicle.join("20261010/default/080000_300");
    fs::create_dir_all(&seg_prior).unwrap();
    let p2_again = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "limit": 1, "cursor": cur1}),
    )
    .unwrap();
    assert_eq!(
        p2_again["transcripts"][0]["date"],
        p2["transcripts"][0]["date"]
    );
    assert_eq!(
        p2_again["transcripts"][0]["time"],
        p2["transcripts"][0]["time"]
    );
    let t1 = probe(
        &journal,
        "get_transcript",
        json!({"reference": p2["transcripts"][0]["reference"]}),
    )
    .unwrap();
    let t2 = probe(
        &journal,
        "get_transcript",
        json!({"reference": p2_again["transcripts"][0]["reference"]}),
    )
    .unwrap();
    assert_eq!(t1, t2);

    assert_eq!(
        probe(&journal, "list_transcripts", json!({"time_from": "10:05"})),
        Err(McpProbeError::InvalidInput)
    );
    assert_eq!(
        probe(
            &journal,
            "list_transcripts",
            json!({"day": "20261010", "time_from": "11:00", "time_to": "10:00"})
        ),
        Err(McpProbeError::InvalidInput)
    );
    assert_eq!(
        probe(
            &journal,
            "list_transcripts",
            json!({"day": "20261012", "cursor": cur1})
        ),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(
            &journal,
            "list_transcripts",
            json!({"day": "20261010", "time_from": "10:00", "cursor": cur1})
        ),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(
            &journal,
            "list_transcripts",
            json!({"day": "20261010", "facet": FACET_B, "cursor": cur1})
        ),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(&journal, "search", json!({"query": "test", "cursor": cur1})),
        Err(McpProbeError::Unavailable)
    );

    let huge_line = "b".repeat(70_000);
    let seg_huge_in = chronicle.join("20261020/alpha/090000_300");
    fs::create_dir_all(seg_huge_in.join("talents")).unwrap();
    fs::write(
        seg_huge_in.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        seg_huge_in.join("meeting_transcript.md"),
        format!("{huge_line}\n"),
    )
    .unwrap();

    let seg_huge_out = chronicle.join("20261020/beta/090000_300");
    fs::create_dir_all(seg_huge_out.join("talents")).unwrap();
    fs::write(
        seg_huge_out.join("talents/facets.json"),
        "[{\"facet\":\"beta\"}]",
    )
    .unwrap();
    fs::write(
        seg_huge_out.join("meeting_transcript.md"),
        format!("{huge_line}\n"),
    )
    .unwrap();

    let seg_ok = chronicle.join("20261020/alpha/090500_300");
    fs::create_dir_all(seg_ok.join("talents")).unwrap();
    fs::write(
        seg_ok.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(seg_ok.join("meeting_transcript.md"), "ok words\n").unwrap();

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "messages".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let chosen_resp = probe(&journal, "list_transcripts", json!({"day": "20261020"})).unwrap();
    assert_eq!(chosen_resp["unreadable"], 1);
    assert_eq!(chosen_resp["transcripts"].as_array().unwrap().len(), 1);

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();
    let p_time1 = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "time_from": "08:00", "time_to": "11:30", "limit": 1}),
    )
    .unwrap();
    let cur_time = p_time1["next_cursor"]
        .as_str()
        .expect("has cursor")
        .to_owned();

    let p_audit = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "time_from": "08:00", "time_to": "11:30", "cursor": cur_time}),
    )
    .unwrap();
    assert!(p_audit["transcripts"].is_array());

    let interactions = find_audit_files(journal.path(), solstone_core_mcp_audit::INTERACTION_FILE);
    let matched_interaction = interactions
        .iter()
        .filter_map(|p| {
            let val: Value = serde_json::from_str(&fs::read_to_string(p).ok()?).ok()?;
            if val["request"]["arguments"].get("time_from") == Some(&json!("08:00"))
                && val["request"]["arguments"].get("cursor").is_some()
            {
                Some(val)
            } else {
                None
            }
        })
        .next()
        .expect("found interaction with time_from 08:00 and cursor");
    assert_eq!(
        matched_interaction["request"]["arguments"]["time_from"],
        "08:00"
    );
    assert_eq!(
        matched_interaction["request"]["arguments"]["time_to"],
        "11:30"
    );
    let recorded_cursor = matched_interaction["request"]["arguments"]["cursor"]
        .as_str()
        .unwrap();
    assert_eq!(recorded_cursor.len(), 16);
    assert!(recorded_cursor.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(recorded_cursor, cur_time);
}

#[test]
fn raw_transcript_text_never_reaches_search_or_fetch() {
    let journal = setup_shared_link_journal();
    let seg = journal.path().join("chronicle/20261010/alpha/090000_300");
    fs::write(
        seg.join("audio.jsonl"),
        "{\"start\":\"00:00:02\",\"text\":\"tokenonlyinaudio\"}\n",
    )
    .unwrap();
    fs::write(
        seg.join("imported_audio.jsonl"),
        "{\"start\":\"00:00:05\",\"text\":\"tokenonlyinimportedaudio\"}\n",
    )
    .unwrap();

    plant_chunk(
        journal.path(),
        "20261010/alpha/090000_300/talents/audio.md",
        "20261010",
        "alpha",
        0,
        "brief text",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let s1 = probe(&journal, "search", json!({"query": "tokenonlyinaudio"})).unwrap();
    assert!(s1["results"].as_array().unwrap().is_empty());
    let s2 = probe(
        &journal,
        "search",
        json!({"query": "tokenonlyinimportedaudio"}),
    )
    .unwrap();
    assert!(s2["results"].as_array().unwrap().is_empty());

    let s_brief = probe(&journal, "search", json!({"query": "brief text"})).unwrap();
    let results = s_brief["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let ref_brief = &results[0]["reference"];
    let fetched = probe(&journal, "fetch", json!({"reference": ref_brief})).unwrap();
    let fetched_str = fetched.to_string();
    assert!(!fetched_str.contains("tokenonlyinaudio"));
    assert!(!fetched_str.contains("tokenonlyinimportedaudio"));
}

#[test]
fn link_checks_stay_within_budget_and_more_segments_reflects_readable_segments() {
    let journal = tempfile::Builder::new()
        .prefix("issuer-test-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());

    let mut keys = Vec::new();
    for i in 0..288 {
        let sec = i * 300;
        let h = sec / 3600;
        let m = (sec % 3600) / 60;
        let s = sec % 60;
        let key = format!("{h:02}{m:02}{s:02}_300");
        fs::create_dir_all(journal.path().join("chronicle/20261010").join(&key)).unwrap();
        keys.push(key);
    }

    let act_path = "facets/alpha/activities/20261010.jsonl";
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    let record_line = json!({"id": "act-288", "segments": keys}).to_string();
    fs::write(journal.path().join(act_path), format!("{record_line}\n")).unwrap();

    let chunks = produce_chunks(Family::Activity, act_path, &record_line);
    let text = chunks.chunks[0].content.clone();
    let record: Map<String, Value> = serde_json::from_str(&record_line).unwrap();

    let principal = DispatchPrincipal {
        connection: CONNECTION,
        agent_identity: CONNECTION,
    };
    let mut categories = BTreeSet::new();
    categories.insert(AdmittedCategory::Transcripts);
    let snapshot = ConnectionReadSnapshot {
        categories,
        scope: ConnectionScope::WholeJournal,
        generation: 1,
    };

    let subject = LinkSubject {
        path: act_path,
        idx: 0,
        text: &text,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record),
    };
    let mut checker = CountingChecker { count: 0 };
    let mut subjects = [subject];
    let mut response_checks = 0;
    issue_segment_links(
        journal.path(),
        principal,
        &snapshot,
        &mut subjects,
        48,
        48,
        &mut response_checks,
        &mut checker,
    );
    assert_eq!(checker.count, 48);
    assert_eq!(subjects[0].segments.len(), 48);
    assert!(subjects[0].more_segments);

    let subject30 = LinkSubject {
        path: act_path,
        idx: 0,
        text: &text,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record),
    };
    let mut checker30 = CountingChecker { count: 0 };
    let mut subjects30 = [subject30];
    let mut response_checks30 = 0;
    issue_segment_links(
        journal.path(),
        principal,
        &snapshot,
        &mut subjects30,
        48,
        30,
        &mut response_checks30,
        &mut checker30,
    );
    assert_eq!(checker30.count, 30);
    assert_eq!(subjects30[0].segments.len(), 30);
    assert!(subjects30[0].more_segments);

    let chronicle2 = journal.path().join("chronicle/20261011");
    let seg_in = chronicle2.join("alpha/000000_300");
    fs::create_dir_all(seg_in.join("talents")).unwrap();
    fs::write(
        seg_in.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();

    let seg_out = chronicle2.join("beta/000500_300");
    fs::create_dir_all(seg_out.join("talents")).unwrap();
    fs::write(
        seg_out.join("talents/facets.json"),
        "[{\"facet\":\"beta\"}]",
    )
    .unwrap();

    let act_path2 = "facets/alpha/activities/20261011.jsonl";
    let record_line2 =
        json!({"id": "act-two", "segments": ["000000_300", "000500_300"]}).to_string();
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    fs::write(journal.path().join(act_path2), format!("{record_line2}\n")).unwrap();
    let chunks2 = produce_chunks(Family::Activity, act_path2, &record_line2);
    let text2 = chunks2.chunks[0].content.clone();
    let record2: Map<String, Value> = serde_json::from_str(&record_line2).unwrap();

    let mut cats_chosen = BTreeSet::new();
    cats_chosen.insert(AdmittedCategory::Transcripts);
    let snapshot_chosen = ConnectionReadSnapshot {
        categories: cats_chosen,
        scope: ConnectionScope::ChosenFacets {
            ids: BTreeSet::from([FACET_A.to_owned()]),
        },
        generation: 1,
    };
    let subject_budget1 = LinkSubject {
        path: act_path2,
        idx: 0,
        text: &text2,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record2),
    };
    let mut checker_budget1 = CountingChecker { count: 0 };
    let mut subjects_budget1 = [subject_budget1];
    let mut response_checks_budget1 = 0;
    issue_segment_links(
        journal.path(),
        principal,
        &snapshot_chosen,
        &mut subjects_budget1,
        48,
        1,
        &mut response_checks_budget1,
        &mut checker_budget1,
    );
    assert_eq!(checker_budget1.count, 1);
    assert_eq!(subjects_budget1[0].segments.len(), 1);
    // The only candidate past the budget is withheld, so nothing says more exist.
    assert!(!subjects_budget1[0].more_segments);
}

#[test]
fn no_links_without_the_transcripts_grant() {
    let journal = setup_shared_link_journal();
    let act_path = "facets/alpha/activities/20261010.jsonl";
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    let line0 = json!({"id": "launch-sync", "segments": ["090000_300"]}).to_string();
    fs::write(journal.path().join(act_path), format!("{line0}\n")).unwrap();
    let chunks = produce_chunks(Family::Activity, act_path, &line0);
    let text = chunks.chunks[0].content.clone();
    let record: Map<String, Value> = serde_json::from_str(&line0).unwrap();

    let principal = DispatchPrincipal {
        connection: CONNECTION,
        agent_identity: CONNECTION,
    };

    let mut cats_with_tr = BTreeSet::new();
    cats_with_tr.insert(AdmittedCategory::Transcripts);
    let snap_tr = ConnectionReadSnapshot {
        categories: cats_with_tr,
        scope: ConnectionScope::WholeJournal,
        generation: 1,
    };
    let sub_tr = LinkSubject {
        path: act_path,
        idx: 0,
        text: &text,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record),
    };
    let mut chk_tr = CountingChecker { count: 0 };
    let mut subs_tr = [sub_tr];
    let mut response_checks_tr = 0;
    issue_segment_links(
        journal.path(),
        principal,
        &snap_tr,
        &mut subs_tr,
        48,
        48,
        &mut response_checks_tr,
        &mut chk_tr,
    );
    assert_eq!(chk_tr.count, 2);
    assert_eq!(subs_tr[0].segments.len(), 2);

    let mut cats_no_tr = BTreeSet::new();
    cats_no_tr.insert(AdmittedCategory::Entities);
    let snap_no_tr = ConnectionReadSnapshot {
        categories: cats_no_tr,
        scope: ConnectionScope::WholeJournal,
        generation: 1,
    };
    let sub_no_tr = LinkSubject {
        path: act_path,
        idx: 0,
        text: &text,
        segments: Vec::new(),
        more_segments: false,
        activity_record: Some(&record),
    };
    let mut chk_no_tr = CountingChecker { count: 0 };
    let mut subs_no_tr = [sub_no_tr];
    let mut response_checks_no_tr = 0;
    issue_segment_links(
        journal.path(),
        principal,
        &snap_no_tr,
        &mut subs_no_tr,
        48,
        48,
        &mut response_checks_no_tr,
        &mut chk_no_tr,
    );
    assert_eq!(chk_no_tr.count, 0);
    assert!(subs_no_tr[0].segments.is_empty());
    assert!(!subs_no_tr[0].more_segments);
}

fn write_chronicle_segment(
    journal: &Path,
    day: &str,
    stream: &str,
    name: &str,
    facet: &str,
    text: &str,
) {
    let seg = journal.join("chronicle").join(day).join(stream).join(name);
    fs::create_dir_all(seg.join("talents")).unwrap();
    fs::write(
        seg.join("talents/facets.json"),
        format!("[{{\"facet\":\"{facet}\"}}]"),
    )
    .unwrap();
    fs::write(seg.join("talents/audio.md"), format!("{text}\n")).unwrap();
}

#[allow(clippy::too_many_arguments)]
fn write_entity_fixture(
    journal: &Path,
    facet: &str,
    rel_dir: &str,
    entity_id: &str,
    name: &str,
    kind: Option<&str>,
    description: Option<&str>,
    note: Option<&str>,
) {
    let link = journal
        .join("facets")
        .join(facet)
        .join("entities")
        .join(rel_dir);
    fs::create_dir_all(&link).unwrap();
    let mut rel_val = json!({"entity_id": entity_id, "last_seen": "20261010"});
    if let Some(desc) = description {
        rel_val["description"] = json!(desc);
    }
    fs::write(link.join("entity.json"), rel_val.to_string()).unwrap();
    if let Some(n) = note {
        fs::write(
            link.join("observations.jsonl"),
            format!("{{\"content\":\"{n}\"}}\n"),
        )
        .unwrap();
    }
    let ent_dir = journal.join("entities").join(rel_dir);
    fs::create_dir_all(&ent_dir).unwrap();
    let mut id_val = json!({"id": entity_id, "name": name});
    if let Some(k) = kind {
        id_val["type"] = json!(k);
    }
    fs::write(ent_dir.join("entity.json"), id_val.to_string()).unwrap();
}

#[test]
fn entity_name_derivation_walk_across_pages() {
    let journal = tempfile::Builder::new()
        .prefix("entity-derivation-walk-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core infrastructure project"),
        Some("Telemetry pipeline updates"),
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper initial kick-off announcement.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper first milestone completed on schedule.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261003",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper deployment to canary group verified.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261004",
        "stream1",
        "090000_300",
        "alpha",
        "Unrelated maintenance window scheduled.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let mut walk_full = Vec::new();
    let mut cursor = None;
    loop {
        let mut args = json!({"query": "Project Juniper", "limit": 1});
        if let Some(c) = cursor {
            args["cursor"] = json!(c);
        }
        let page = probe(&journal, "search", args).unwrap();
        let results = page["results"].as_array().unwrap();
        if results.is_empty() {
            break;
        }
        assert_eq!(results.len(), 1);
        walk_full.push((
            results[0]["date"].as_str().unwrap().to_owned(),
            results[0]["snippet"].as_str().unwrap().to_owned(),
        ));
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }

    let mut walk_short = Vec::new();
    cursor = None;
    loop {
        let mut args = json!({"query": "Juniper", "limit": 1});
        if let Some(c) = cursor {
            args["cursor"] = json!(c);
        }
        let page = probe(&journal, "search", args).unwrap();
        let results = page["results"].as_array().unwrap();
        if results.is_empty() {
            break;
        }
        assert_eq!(results.len(), 1);
        walk_short.push((
            results[0]["date"].as_str().unwrap().to_owned(),
            results[0]["snippet"].as_str().unwrap().to_owned(),
        ));
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(walk_full.len(), 3);
    assert_eq!(walk_full, walk_short);
    assert!(!walk_full.iter().any(|(_, s)| s.contains("Unrelated")));
}

#[test]
fn project_space_juniper_whitespace_and_case_folding() {
    let journal = tempfile::Builder::new()
        .prefix("entity-case-fold-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper launched.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper second day update.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res1 = probe(&journal, "search", json!({"query": "Project Juniper"})).unwrap();
    let res2 = probe(&journal, "search", json!({"query": "project  juniper"})).unwrap();
    assert_eq!(
        visible_search_page(&res1)["results"],
        visible_search_page(&res2)["results"]
    );
    assert_eq!(res1["results"].as_array().unwrap().len(), 2);
}

#[test]
fn canonical_name_equal_to_type_does_not_become_empty() {
    let journal = tempfile::Builder::new()
        .prefix("entity-name-equal-type-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "milestone",
        "milestone-1",
        "Milestone",
        Some("Milestone"),
        Some("Key milestone entity"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Q3 Milestone achieved successfully.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Milestone"})).unwrap();
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Milestone")
    );
}

#[test]
fn type_prefix_token_boundary_prevents_partial_word_match() {
    let journal = tempfile::Builder::new()
        .prefix("entity-prefix-token-boundary-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "projector",
        "tool-projector",
        "Projector",
        Some("Project"),
        Some("Hardware display device"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Projector installed in conference room.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Project status review meeting.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Projector"})).unwrap();
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Projector")
    );
    assert!(!results[0]["snippet"].as_str().unwrap().contains("meeting"));
}

#[test]
fn person_card_emitted_when_type_not_leading_token() {
    let description = "Lead architect. ![marker](https://example.invalid/image)";
    let literal = solstone_core_format::markdown::literal_text(description);
    let journal = tempfile::Builder::new()
        .prefix("entity-person-card-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "alice",
        "entity-alice",
        "Alice Smith",
        Some("Person"),
        Some(description),
        Some("Joined the foundation team."),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Alice Smith"})).unwrap();
    assert_no_coordinate_keys(&res);
    assert!(res["results"].as_array().unwrap().is_empty());
    let matches = res["entity_matches"]
        .as_array()
        .expect("emits entity_matches");
    assert_eq!(matches.len(), 1);
    let card = &matches[0];
    assert_eq!(card["name"], "Alice Smith");
    assert_eq!(card["type"], "Person");
    assert_eq!(card["facet"], FACET_A);
    assert_eq!(card["description"], literal);
    assert_eq!(card["observations"], "Joined the foundation team.");
    assert!(card.get("entity_id").is_none());
    assert!(card.get("aka").is_none());
    let card_ref = card["reference"].as_str().unwrap();

    let entity = probe(&journal, "get_entity", json!({"reference": card_ref})).unwrap();
    assert_eq!(entity["name"], "Alice Smith");
    assert_eq!(entity["type"], "Person");
    assert_eq!(entity["description"], literal);

    let output = dispatch_output(&journal, "search", json!({"query": "Alice Smith"}));
    assert!(output.empty_note.is_none());
    let rendered = crate::jsonrpc::tool_result(output.value, output.empty_note.as_deref());
    let rendered_text = rendered.to_string();
    assert!(!rendered_text.contains("No results"));

    let activity = crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 10,
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap();
    assert_eq!(
        activity.entries[0].outcome,
        crate::activity::RecordedOutcome::Served
    );
    let shape = activity.entries[0].result.as_ref().unwrap();
    assert_eq!(shape.count, 1);
    assert!(
        shape
            .targets
            .iter()
            .any(|t| t == &format!("facet:{FACET_A}/entity:entity-alice"))
    );
    assert!(
        shape
            .targets
            .iter()
            .any(|t| t.contains("observations.jsonl"))
    );
}

#[test]
fn card_description_and_observations_truncation() {
    let journal = tempfile::Builder::new()
        .prefix("entity-card-truncation-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let long_desc = "D".repeat(450);
    let long_note = "N".repeat(500);
    write_entity_fixture(
        journal.path(),
        "alpha",
        "longitem",
        "entity-long",
        "Long Entity",
        Some("Project"),
        Some(&long_desc),
        Some(&long_note),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Long Entity"})).unwrap();
    let matches = res["entity_matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    let card = &matches[0];
    assert_eq!(card["truncated"], true);
    assert_eq!(card["description"].as_str().unwrap().chars().count(), 400);
    assert_eq!(card["observations"].as_str().unwrap().chars().count(), 400);
}

#[test]
fn entity_matches_truncated_flag_when_exceeding_limit() {
    let journal = tempfile::Builder::new()
        .prefix("entity-matches-truncated-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "shared-a",
        "entity-shared-a",
        "Shared Name",
        None,
        Some("Alpha relation"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "beta",
        "shared-b",
        "entity-shared-b",
        "Shared Name",
        None,
        Some("Beta relation"),
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let res = probe(
        &journal,
        "search",
        json!({"query": "Shared Name", "limit": 1}),
    )
    .unwrap();
    assert_eq!(res["entity_matches_truncated"], true);
    let matches = res["entity_matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["facet"], FACET_A);
}

#[test]
fn identity_aka_and_description_sentinels_ignored() {
    let journal = tempfile::Builder::new()
        .prefix("entity-aka-sentinel-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let root = journal.path();
    let link = root.join("facets/alpha/entities/sentinel");
    fs::create_dir_all(&link).unwrap();
    fs::write(
        link.join("entity.json"),
        json!({"entity_id": "entity-sentinel", "description": "Known description"}).to_string(),
    )
    .unwrap();
    let ent_dir = root.join("entities/sentinel");
    fs::create_dir_all(&ent_dir).unwrap();
    fs::write(
        ent_dir.join("entity.json"),
        json!({
            "id": "entity-sentinel",
            "name": "Canonical Project",
            "type": "Project",
            "aka": ["Sentinel Alias"],
            "description": "Sentinel Description"
        })
        .to_string(),
    )
    .unwrap();
    write_chronicle_segment(
        root,
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Canonical Project kick-off line 1.",
    );
    write_chronicle_segment(
        root,
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Canonical Project line 2.",
    );
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res_aka = probe(&journal, "search", json!({"query": "Sentinel Alias"})).unwrap();
    assert!(res_aka.get("entity_matches").is_none());

    let res_canon = probe(
        &journal,
        "search",
        json!({"query": "Canonical Project", "limit": 1}),
    )
    .unwrap();
    let card = &res_canon["entity_matches"].as_array().unwrap()[0];
    let card_str = card.to_string();
    assert!(!card_str.contains("Sentinel Alias"));
    assert!(!card_str.contains("Sentinel Description"));
    let cursor = res_canon["next_cursor"].as_str().unwrap().to_owned();

    fs::write(
        ent_dir.join("entity.json"),
        json!({
            "id": "entity-sentinel",
            "name": "Canonical Project",
            "type": "Project",
            "aka": ["Updated Another Alias"],
            "description": "Sentinel Description"
        })
        .to_string(),
    )
    .unwrap();

    let p2 = probe(
        &journal,
        "search",
        json!({"query": "Canonical Project", "limit": 1, "cursor": cursor}),
    )
    .unwrap();
    assert_eq!(p2["results"].as_array().unwrap().len(), 1);
}

#[test]
fn transcripts_only_and_category_transcripts_do_not_derive() {
    let journal = tempfile::Builder::new()
        .prefix("transcripts-only-no-derive-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Alpha project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper full name line.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper short name only.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let res = probe(&journal, "search", json!({"query": "Project Juniper"})).unwrap();
    assert!(res.get("entity_matches").is_none());
    assert_eq!(res["results"].as_array().unwrap().len(), 1);
    assert!(
        res["results"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Project Juniper")
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let res_cat = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "category": "transcripts"}),
    )
    .unwrap();
    assert!(res_cat.get("entity_matches").is_none());
    assert_eq!(res_cat["results"].as_array().unwrap().len(), 1);
}

#[test]
fn entities_only_grant_denied_search_and_fetch() {
    let journal = tempfile::Builder::new()
        .prefix("entities-only-denied-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "search", json!({"query": "Project Juniper"})),
        Err(McpProbeError::PermissionDenied)
    );
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": "dummy-ref"})),
        Err(McpProbeError::PermissionDenied)
    );
}

#[test]
fn facet_filter_and_chosen_facet_isolation() {
    let journal = tempfile::Builder::new()
        .prefix("facet-isolation-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "alpha-tool",
        "tool-alpha",
        "Alpha Tool",
        Some("Tool"),
        Some("Tool for alpha"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "beta",
        "beta-tool",
        "tool-beta",
        "Beta Tool",
        Some("Tool"),
        Some("Tool for beta"),
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let res = probe(
        &journal,
        "search",
        json!({"query": "Beta Tool", "facet": FACET_A}),
    )
    .unwrap();
    assert!(res.get("entity_matches").is_none());

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let res_scoped = probe(&journal, "search", json!({"query": "Beta Tool"})).unwrap();
    assert!(res_scoped.get("entity_matches").is_none());
}

#[test]
fn same_name_in_two_facets_emits_two_cards() {
    let journal = tempfile::Builder::new()
        .prefix("same-name-two-facets-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "shared",
        "entity-shared",
        "Shared Identity",
        None,
        Some("Alpha unique relation"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "beta",
        "shared",
        "entity-shared",
        "Shared Identity",
        None,
        Some("Beta unique relation"),
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Shared Identity"})).unwrap();
    let cards = res["entity_matches"].as_array().unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0]["description"], "Alpha unique relation");
    assert_eq!(cards[1]["description"], "Beta unique relation");
}

#[test]
fn disagreeing_derivations_keep_original_query() {
    let journal = tempfile::Builder::new()
        .prefix("disagreeing-derivations-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "ent-a",
        "ent-alpha",
        "Project Alpha Team",
        Some("Project"),
        Some("Alpha desc"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "beta",
        "ent-b",
        "ent-beta",
        "Project Alpha Team",
        Some("Team"),
        Some("Beta desc"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Alpha Team full name.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Alpha Team short name only.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Project Alpha Team"})).unwrap();
    assert_eq!(res["entity_matches"].as_array().unwrap().len(), 2);
    let hits = res["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Project Alpha Team")
    );
}

#[test]
fn detached_blocked_and_removed_relationships_excluded() {
    let journal = tempfile::Builder::new()
        .prefix("detached-blocked-removed-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "active",
        "entity-active",
        "Active Entity",
        Some("Person"),
        Some("Active desc"),
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    let card_ref = res["entity_matches"].as_array().unwrap()[0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(probe(&journal, "get_entity", json!({"reference": card_ref})).is_ok());

    // Detach relationship
    fs::write(
        journal
            .path()
            .join("facets/alpha/entities/active/entity.json"),
        json!({"entity_id": "entity-active", "detached": true}).to_string(),
    )
    .unwrap();
    let res_detached = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    assert!(res_detached.get("entity_matches").is_none());
    assert_eq!(
        probe(&journal, "get_entity", json!({"reference": card_ref})),
        Err(McpProbeError::Unavailable)
    );

    // Re-attach, then block identity
    fs::write(
        journal
            .path()
            .join("facets/alpha/entities/active/entity.json"),
        json!({"entity_id": "entity-active", "detached": false}).to_string(),
    )
    .unwrap();
    fs::write(
        journal.path().join("entities/active/entity.json"),
        json!({"id": "entity-active", "name": "Active Entity", "blocked": true}).to_string(),
    )
    .unwrap();
    let res_blocked = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    assert!(res_blocked.get("entity_matches").is_none());
    assert_eq!(
        probe(&journal, "get_entity", json!({"reference": card_ref})),
        Err(McpProbeError::Unavailable)
    );

    // Unblock, then remove relationship directory
    fs::write(
        journal.path().join("entities/active/entity.json"),
        json!({"id": "entity-active", "name": "Active Entity", "blocked": false}).to_string(),
    )
    .unwrap();
    fs::remove_dir_all(journal.path().join("facets/alpha/entities/active")).unwrap();
    let res_removed = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    assert!(res_removed.get("entity_matches").is_none());
    assert_eq!(
        probe(&journal, "get_entity", json!({"reference": card_ref})),
        Err(McpProbeError::Unavailable)
    );

    write_entity_fixture(
        journal.path(),
        "alpha",
        "active",
        "entity-active",
        "Active Entity",
        Some("Person"),
        Some("Active desc"),
        None,
    );
    let res_recreated = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    assert!(res_recreated.get("entity_matches").is_some());
    assert_eq!(res_recreated["entity_matches"].as_array().unwrap().len(), 1);

    fs::write(journal.path().join("facets/alpha/facet.json"), b"not json").unwrap();
    let res_corrupt = probe(&journal, "search", json!({"query": "Active Entity"})).unwrap();
    assert!(res_corrupt.get("entity_matches").is_none());
}

#[test]
fn date_filters_suppress_cards_and_bound_derivation() {
    let journal = tempfile::Builder::new()
        .prefix("date-filters-suppress-cards-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper kick-off.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper day two.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261003",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper day three.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "day": "20261002"}),
    )
    .unwrap();
    assert!(res.get("entity_matches").is_none());
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["date"], "20261002");
    assert!(
        results[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Juniper day two")
    );

    let res_range = probe(
        &journal,
        "search",
        json!({
            "query": "Project Juniper",
            "day_from": "20261001",
            "day_to": "20261002"
        }),
    )
    .unwrap();
    assert!(res_range.get("entity_matches").is_none());
    let range_results = res_range["results"].as_array().unwrap();
    assert_eq!(range_results.len(), 2);
    let range_dates: Vec<_> = range_results
        .iter()
        .map(|r| r["date"].as_str().unwrap())
        .collect();
    assert!(range_dates.contains(&"20261001"));
    assert!(range_dates.contains(&"20261002"));
    assert!(!range_dates.contains(&"20261003"));

    let res_to = probe(
        &journal,
        "search",
        json!({
            "query": "Project Juniper",
            "day_to": "20261001"
        }),
    )
    .unwrap();
    assert!(res_to.get("entity_matches").is_none());
    let to_results = res_to["results"].as_array().unwrap();
    assert_eq!(to_results.len(), 1);
    assert_eq!(to_results[0]["date"], "20261001");

    let res_from = probe(
        &journal,
        "search",
        json!({
            "query": "Project Juniper",
            "day_from": "20261003"
        }),
    )
    .unwrap();
    assert!(res_from.get("entity_matches").is_none());
    let from_results = res_from["results"].as_array().unwrap();
    assert_eq!(from_results.len(), 1);
    assert_eq!(from_results[0]["date"], "20261003");
}

#[test]
fn empty_query_with_day_bound_lists_indexed_without_cards() {
    let journal = tempfile::Builder::new()
        .prefix("empty-query-day-bound-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Day one content item.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "", "day": "20261001"})).unwrap();
    assert!(res.get("entity_matches").is_none());
    assert_eq!(res["results"].as_array().unwrap().len(), 1);
}

#[test]
fn syntax_and_operators_bypass_resolution() {
    let journal = tempfile::Builder::new()
        .prefix("syntax-operators-bypass-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper name:Project kick-off announcement.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper second day update without prefix.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    for query in [
        "\"Project Juniper\"",
        "Project AND Juniper",
        "name:Project Juniper",
    ] {
        let res = probe(&journal, "search", json!({"query": query})).unwrap();
        assert!(
            res.get("entity_matches").is_none(),
            "query {query} emitted cards"
        );
        let results = res["results"].as_array().unwrap();
        assert!(
            !results
                .iter()
                .any(|r| r["snippet"].as_str().unwrap().contains("second day update")),
            "query {query} derived short name"
        );
    }
}

#[test]
fn cursor_unavailable_after_rename_removal_or_type_change() {
    let journal = tempfile::Builder::new()
        .prefix("cursor-unavailable-shift-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper item one.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper item two.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let p1 = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "limit": 1}),
    )
    .unwrap();
    let cursor = p1["next_cursor"].as_str().unwrap().to_owned();

    let p2_ok = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "limit": 1, "cursor": cursor}),
    )
    .unwrap();
    assert_eq!(p2_ok["results"].as_array().unwrap().len(), 1);

    let p1_for_rename = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "limit": 1}),
    )
    .unwrap();
    let cursor_rename = p1_for_rename["next_cursor"].as_str().unwrap().to_owned();
    fs::write(
        journal.path().join("entities/juniper/entity.json"),
        json!({"id": "project-juniper", "name": "Project Cedar", "type": "Project"}).to_string(),
    )
    .unwrap();
    assert_eq!(
        probe(
            &journal,
            "search",
            json!({"query": "Project Juniper", "limit": 1, "cursor": cursor_rename})
        ),
        Err(McpProbeError::Unavailable)
    );

    fs::write(
        journal.path().join("entities/juniper/entity.json"),
        json!({"id": "project-juniper", "name": "Project Juniper", "type": "Project"}).to_string(),
    )
    .unwrap();
    let p1_for_type = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "limit": 1}),
    )
    .unwrap();
    let cursor_type = p1_for_type["next_cursor"].as_str().unwrap().to_owned();
    fs::write(
        journal.path().join("entities/juniper/entity.json"),
        json!({"id": "project-juniper", "name": "Project Juniper", "type": "Initiative"})
            .to_string(),
    )
    .unwrap();
    assert_eq!(
        probe(
            &journal,
            "search",
            json!({"query": "Project Juniper", "limit": 1, "cursor": cursor_type})
        ),
        Err(McpProbeError::Unavailable)
    );

    fs::write(
        journal.path().join("entities/juniper/entity.json"),
        json!({"id": "project-juniper", "name": "Project Juniper", "type": "Project"}).to_string(),
    )
    .unwrap();
    let p1_for_removal = probe(
        &journal,
        "search",
        json!({"query": "Project Juniper", "limit": 1}),
    )
    .unwrap();
    let cursor_removal = p1_for_removal["next_cursor"].as_str().unwrap().to_owned();
    fs::remove_dir_all(journal.path().join("facets/alpha/entities/juniper")).unwrap();
    assert_eq!(
        probe(
            &journal,
            "search",
            json!({"query": "Project Juniper", "limit": 1, "cursor": cursor_removal})
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn malformed_relationship_in_granted_facet_sets_incomplete() {
    let journal = tempfile::Builder::new()
        .prefix("malformed-relationship-fallback-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Project Juniper",
        Some("Project"),
        Some("Core project"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Project Juniper full name line.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper short name only.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let link_bad = journal.path().join("facets/alpha/entities/badrel");
    fs::create_dir_all(&link_bad).unwrap();
    fs::write(link_bad.join("entity.json"), "[]").unwrap();

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Project Juniper"})).unwrap();
    assert_eq!(res["entity_context_incomplete"], true);
    assert!(res.get("entity_matches").is_none());
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Project Juniper")
    );

    fs::remove_dir_all(&link_bad).unwrap();
    let link_bad_beta = journal.path().join("facets/beta/entities/badrel");
    fs::create_dir_all(&link_bad_beta).unwrap();
    fs::write(link_bad_beta.join("entity.json"), "[]").unwrap();

    let res_beta_bad = probe(&journal, "search", json!({"query": "Project Juniper"})).unwrap();
    assert!(res_beta_bad.get("entity_context_incomplete").is_none());
    assert_eq!(res_beta_bad["results"].as_array().unwrap().len(), 2);

    let ent_bad = journal.path().join("entities/badid");
    fs::create_dir_all(&ent_bad).unwrap();
    fs::write(ent_bad.join("entity.json"), "[]").unwrap();
    let link_bad_id = journal.path().join("facets/alpha/entities/badid");
    fs::create_dir_all(&link_bad_id).unwrap();
    fs::write(
        link_bad_id.join("entity.json"),
        json!({"entity_id": "bad-id"}).to_string(),
    )
    .unwrap();

    let res_id_bad = probe(&journal, "search", json!({"query": "Project Juniper"})).unwrap();
    assert!(res_id_bad.get("entity_context_incomplete").is_none());
    assert_eq!(res_id_bad["results"].as_array().unwrap().len(), 2);
}

#[test]
fn card_responses_adhere_to_coordinate_and_security_invariants() {
    let journal = tempfile::Builder::new()
        .prefix("card-security-invariants-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "project",
        "entity-proj",
        "Secure Project",
        Some("Project"),
        Some("Detailed relationship description"),
        Some("Note observation line"),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Secure Project"})).unwrap();
    assert_no_coordinate_keys(&res);
    let card = &res["entity_matches"].as_array().unwrap()[0];
    assert!(card.get("entity_id").is_none());
    assert!(card.get("aka").is_none());
    assert!(card.get("path").is_none());
    assert!(card.get("stream").is_none());
    assert!(card.get("idx").is_none());
    assert!(card.get("row_id").is_none());
    assert!(card.get("last_seen").is_none());
}

#[test]
fn a_multi_word_type_derives_the_remainder() {
    let journal = tempfile::Builder::new()
        .prefix("multi-word-type-derive-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "juniper",
        "project-juniper",
        "Research Project Juniper",
        Some("Research Project"),
        Some("Core research initiative"),
        None,
    );
    write_chronicle_segment(
        journal.path(),
        "20261001",
        "stream1",
        "090000_300",
        "alpha",
        "Research Project Juniper kick-off announcement.",
    );
    write_chronicle_segment(
        journal.path(),
        "20261002",
        "stream1",
        "090000_300",
        "alpha",
        "Juniper second day update without prefix.",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(
        &journal,
        "search",
        json!({"query": "Research Project Juniper"}),
    )
    .unwrap();
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    let dates: Vec<_> = results
        .iter()
        .map(|r| r["date"].as_str().unwrap())
        .collect();
    assert!(dates.contains(&"20261001"));
    assert!(dates.contains(&"20261002"));
}

#[test]
fn apostrophe_name_returns_its_granted_facet_context() {
    let journal = tempfile::Builder::new()
        .prefix("entity-apostrophe-name-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "renee",
        "entity-renee",
        "Renée O'Neill",
        Some("Person"),
        Some("Research lead."),
        Some("Ask about the pilot results."),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "search", json!({"query": "Renée O'Neill"})).unwrap();
    assert!(res["results"].as_array().unwrap().is_empty());
    let matches = res["entity_matches"].as_array().expect("person facet card");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["name"], "Renée O'Neill");
    assert_eq!(matches[0]["facet"], FACET_A);
    assert_eq!(matches[0]["description"], "Research lead.");
    assert_eq!(matches[0]["observations"], "Ask about the pilot results.");
}

fn resolve_entity_reference(journal: &Path, reference: &str) -> (String, String) {
    // A name is written with the facet's folder; the tests compare its id.
    if let Some((entity_id, folder)) = crate::names::parse_entity(reference) {
        let declaration =
            fs::read_to_string(journal.join("facets").join(&folder).join("facet.json"))
                .expect("the named facet's declaration");
        let facet_id = serde_json::from_str::<Value>(&declaration).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        return (facet_id, entity_id);
    }
    let snapshot = match evaluate_connection_read(journal, CONNECTION) {
        PermissionDecision::Snapshot(s) => s,
        _ => panic!("must have snapshot"),
    };
    match crate::dispatch::codec().resolve(
        reference,
        crate::references::ReferenceKind::Entity,
        CONNECTION,
        snapshot.generation,
    ) {
        Ok(crate::references::ReferenceTarget::Entity(e)) => (e.facet_id, e.entity_id),
        other => panic!("expected entity target, got {other:?}"),
    }
}

#[test]
fn list_pagination_walks_and_limits_over_100_items() {
    let journal = tempfile::Builder::new()
        .prefix("list-pagination-")
        .tempdir_in(crate::test_scratch())
        .unwrap();

    for i in 0..105 {
        let dir_name = format!("dir-{:03}", 105 - i);
        let facet_id = format!("00000000-0000-4000-8000-{i:012}");
        let title = format!("Title-{:03}", (i * 37) % 105);
        let dir = journal.path().join("facets").join(&dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("facet.json"),
            json!({"id": facet_id, "title": title}).to_string(),
        )
        .unwrap();
    }

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    // Exercise real admission and outcomes with a distinct synthetic instant
    // per call; a rapid page walk must not depend on random audit-name collisions.
    let start = "2026-03-09T12:00:00Z".parse::<jiff::Timestamp>().unwrap();
    let mut call_number = 0;
    let mut call = |tool: &str, arguments: Value| {
        let entry = crate::registry::tool_by_wire_name(tool).unwrap();
        let now = start + jiff::SignedDuration::from_secs(call_number);
        call_number += 1;
        crate::dispatch::dispatch_authenticated_tool_call(
            journal.path(),
            crate::dispatch::DispatchPrincipal {
                connection: CONNECTION,
                agent_identity: CONNECTION,
            },
            entry.tool_name,
            Some(&arguments),
            now,
        )
        .map(|output| output.value)
    };

    let mut current_cursor: Option<String> = None;
    let mut collected_facet_ids = Vec::new();
    loop {
        let args = if let Some(ref cur) = current_cursor {
            json!({"limit": 1, "cursor": cur})
        } else {
            json!({"limit": 1})
        };
        let page = call("list_facets", args).unwrap();
        assert_no_coordinate_keys(&page);
        let facets = page["facets"].as_array().unwrap();
        assert_eq!(facets.len(), 1);
        collected_facet_ids.push(facets[0]["id"].as_str().unwrap().to_owned());
        if let Some(next) = page.get("next_cursor").and_then(Value::as_str) {
            current_cursor = Some(next.to_owned());
        } else {
            break;
        }
    }
    assert_eq!(collected_facet_ids.len(), 105);
    let expected_facet_ids: Vec<String> = (0..105)
        .map(|i| format!("00000000-0000-4000-8000-{i:012}"))
        .collect();
    assert_eq!(collected_facet_ids, expected_facet_ids);

    let page1 = call("list_facets", json!({})).unwrap();
    assert_no_coordinate_keys(&page1);
    assert_eq!(page1["facets"].as_array().unwrap().len(), 100);
    let cur1 = page1["next_cursor"]
        .as_str()
        .expect("next cursor on page 1");

    let page2 = call("list_facets", json!({"cursor": cur1})).unwrap();
    assert_no_coordinate_keys(&page2);
    assert_eq!(page2["facets"].as_array().unwrap().len(), 5);
    assert!(page2.get("next_cursor").is_none());

    let mut combined_facets = Vec::new();
    for f in page1["facets"]
        .as_array()
        .unwrap()
        .iter()
        .chain(page2["facets"].as_array().unwrap().iter())
    {
        combined_facets.push(f["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(combined_facets, expected_facet_ids);

    let page1_lim100 = call("list_facets", json!({"limit": 100})).unwrap();
    assert_no_coordinate_keys(&page1_lim100);
    assert_eq!(page1_lim100["facets"].as_array().unwrap().len(), 100);
    let cur1_lim100 = page1_lim100["next_cursor"]
        .as_str()
        .expect("next cursor on page 1 with limit 100");

    let page2_lim100 = call("list_facets", json!({"cursor": cur1_lim100, "limit": 100})).unwrap();
    assert_no_coordinate_keys(&page2_lim100);
    assert_eq!(page2_lim100["facets"].as_array().unwrap().len(), 5);
    assert!(page2_lim100.get("next_cursor").is_none());

    let mut combined_facets_lim100 = Vec::new();
    for f in page1_lim100["facets"]
        .as_array()
        .unwrap()
        .iter()
        .chain(page2_lim100["facets"].as_array().unwrap().iter())
    {
        combined_facets_lim100.push(f["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(combined_facets_lim100, expected_facet_ids);

    let empty_journal = tempfile::Builder::new()
        .prefix("empty-list-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    PermissionStore::open(empty_journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let empty_facets = dispatch_output(&empty_journal, "list_facets", json!({}));
    assert!(empty_facets.value["facets"].as_array().unwrap().is_empty());
    assert_eq!(
        empty_facets.empty_note.as_deref(),
        Some("No facets. This connection can see none.")
    );
    assert!(empty_facets.value.get("next_cursor").is_none());

    let empty_entities = dispatch_output(&empty_journal, "list_entities", json!({}));
    assert!(
        empty_entities.value["entities"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        empty_entities.empty_note.as_deref(),
        Some(
            "No entities. This connection can see none that match. This is a complete answer, not an error."
        )
    );
    assert!(empty_entities.value.get("next_cursor").is_none());

    for i in 0..105 {
        let facet_idx = i % 5;
        let facet_dir = format!("dir-{:03}", 105 - facet_idx);
        let entity_id = format!("entity-id-{:03}", i);
        let rel_dir = format!("rel-{:03}", i);
        write_entity_fixture(
            journal.path(),
            &facet_dir,
            &rel_dir,
            &entity_id,
            &format!("Person {:03}", i),
            Some("Person"),
            Some("Description"),
            None,
        );
    }

    let mut expected_entities = Vec::new();
    for f in 0..5 {
        let f_id = format!("00000000-0000-4000-8000-{f:012}");
        for i in (0..105).filter(|i| i % 5 == f) {
            expected_entities.push((f_id.clone(), format!("entity-id-{:03}", i)));
        }
    }
    assert_eq!(expected_entities.len(), 105);

    let mut current_ent_cursor: Option<String> = None;
    let mut collected_entities = Vec::new();
    loop {
        let args = if let Some(ref cur) = current_ent_cursor {
            json!({"limit": 1, "cursor": cur})
        } else {
            json!({"limit": 1})
        };
        let page = call("list_entities", args).unwrap();
        assert_no_coordinate_keys(&page);
        let entities = page["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 1);
        let ref_str = entities[0]["reference"].as_str().unwrap();
        let (f_id, e_id) = resolve_entity_reference(journal.path(), ref_str);
        collected_entities.push((f_id, e_id));
        if let Some(next) = page.get("next_cursor").and_then(Value::as_str) {
            current_ent_cursor = Some(next.to_owned());
        } else {
            break;
        }
    }
    assert_eq!(collected_entities, expected_entities);

    let ent_page1 = call("list_entities", json!({})).unwrap();
    assert_no_coordinate_keys(&ent_page1);
    assert_eq!(ent_page1["entities"].as_array().unwrap().len(), 100);
    let ent_cur1 = ent_page1["next_cursor"]
        .as_str()
        .expect("next cursor on ent page 1");

    let ent_page2 = call("list_entities", json!({"cursor": ent_cur1})).unwrap();
    assert_no_coordinate_keys(&ent_page2);
    assert_eq!(ent_page2["entities"].as_array().unwrap().len(), 5);
    assert!(ent_page2.get("next_cursor").is_none());

    let mut combined_ents = Vec::new();
    for e in ent_page1["entities"]
        .as_array()
        .unwrap()
        .iter()
        .chain(ent_page2["entities"].as_array().unwrap().iter())
    {
        let ref_str = e["reference"].as_str().unwrap();
        combined_ents.push(resolve_entity_reference(journal.path(), ref_str));
    }
    assert_eq!(combined_ents, expected_entities);

    let ent_page1_lim100 = call("list_entities", json!({"limit": 100})).unwrap();
    assert_no_coordinate_keys(&ent_page1_lim100);
    assert_eq!(ent_page1_lim100["entities"].as_array().unwrap().len(), 100);
    let ent_cur1_lim100 = ent_page1_lim100["next_cursor"]
        .as_str()
        .expect("next cursor on ent page 1 with limit 100");

    let ent_page2_lim100 = call(
        "list_entities",
        json!({"cursor": ent_cur1_lim100, "limit": 100}),
    )
    .unwrap();
    assert_no_coordinate_keys(&ent_page2_lim100);
    assert_eq!(ent_page2_lim100["entities"].as_array().unwrap().len(), 5);
    assert!(ent_page2_lim100.get("next_cursor").is_none());

    let mut combined_ents_lim100 = Vec::new();
    for e in ent_page1_lim100["entities"]
        .as_array()
        .unwrap()
        .iter()
        .chain(ent_page2_lim100["entities"].as_array().unwrap().iter())
    {
        let ref_str = e["reference"].as_str().unwrap();
        combined_ents_lim100.push(resolve_entity_reference(journal.path(), ref_str));
    }
    assert_eq!(combined_ents_lim100, expected_entities);
}

#[test]
fn facet_and_entity_ordering_and_scoping() {
    let journal = tempfile::Builder::new()
        .prefix("order-scoping-")
        .tempdir_in(crate::test_scratch())
        .unwrap();

    let f1_dir = journal.path().join("facets/dir_z");
    fs::create_dir_all(&f1_dir).unwrap();
    fs::write(
        f1_dir.join("facet.json"),
        json!({"id": "10000000-0000-4000-8000-000000000001", "title": "Zeta", "description": "Desc A"}).to_string(),
    )
    .unwrap();

    let f2_dir = journal.path().join("facets/dir_a");
    fs::create_dir_all(&f2_dir).unwrap();
    fs::write(
        f2_dir.join("facet.json"),
        json!({"id": "10000000-0000-4000-8000-000000000002", "title": "Alpha", "description": "Desc B"}).to_string(),
    )
    .unwrap();

    write_entity_fixture(
        journal.path(),
        "dir_z",
        "rel_1",
        "entity_shared",
        "Common Name",
        Some("Person"),
        Some("Rel in A"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "dir_a",
        "rel_2",
        "entity_shared",
        "Common Name",
        Some("Person"),
        Some("Rel in B"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "dir_z",
        "rel_3",
        "entity_other",
        "Other Name",
        Some("Person"),
        Some("Other in A"),
        None,
    );

    let detached_dir = f1_dir.join("entities/detached");
    fs::create_dir_all(&detached_dir).unwrap();
    fs::write(
        detached_dir.join("entity.json"),
        json!({"entity_id": "entity_detached", "detached": true}).to_string(),
    )
    .unwrap();
    let ent_detached = journal.path().join("entities/detached");
    fs::create_dir_all(&ent_detached).unwrap();
    fs::write(
        ent_detached.join("entity.json"),
        json!({"id": "entity_detached", "name": "Detached Entity"}).to_string(),
    )
    .unwrap();

    let blocked_dir = f1_dir.join("entities/blocked");
    fs::create_dir_all(&blocked_dir).unwrap();
    fs::write(
        blocked_dir.join("entity.json"),
        json!({"entity_id": "entity_blocked"}).to_string(),
    )
    .unwrap();
    let ent_blocked = journal.path().join("entities/blocked");
    fs::create_dir_all(&ent_blocked).unwrap();
    fs::write(
        ent_blocked.join("entity.json"),
        json!({"id": "entity_blocked", "name": "Blocked Entity", "blocked": true}).to_string(),
    )
    .unwrap();

    let blocked_zzz_dir = f2_dir.join("entities/blocked_zzz");
    fs::create_dir_all(&blocked_zzz_dir).unwrap();
    fs::write(
        blocked_zzz_dir.join("entity.json"),
        json!({"entity_id": "entity_zzz"}).to_string(),
    )
    .unwrap();
    let ent_zzz = journal.path().join("entities/blocked_zzz");
    fs::create_dir_all(&ent_zzz).unwrap();
    fs::write(
        ent_zzz.join("entity.json"),
        json!({"id": "entity_zzz", "name": "Tail Hidden", "blocked": true}).to_string(),
    )
    .unwrap();

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let facets_no_details = probe(&journal, "list_facets", json!({})).unwrap();
    let f_arr = facets_no_details["facets"].as_array().unwrap();
    assert_eq!(f_arr.len(), 2);
    assert_eq!(f_arr[0]["id"], "10000000-0000-4000-8000-000000000001");
    assert_eq!(f_arr[0]["name"], "Zeta");
    assert!(f_arr[0].get("description").is_none());
    assert_eq!(f_arr[1]["id"], "10000000-0000-4000-8000-000000000002");
    assert_eq!(f_arr[1]["name"], "Alpha");
    assert!(f_arr[1].get("description").is_none());

    let facets_lim2 = probe(&journal, "list_facets", json!({"limit": 2})).unwrap();
    assert_eq!(facets_lim2["facets"].as_array().unwrap().len(), 2);
    assert_eq!(
        facets_lim2["facets"][0]["id"],
        "10000000-0000-4000-8000-000000000001"
    );
    assert_eq!(
        facets_lim2["facets"][1]["id"],
        "10000000-0000-4000-8000-000000000002"
    );
    assert!(facets_lim2.get("next_cursor").is_none());

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let facets_with_details = probe(&journal, "list_facets", json!({})).unwrap();
    let f_det = facets_with_details["facets"].as_array().unwrap();
    assert_eq!(f_det[0]["description"], "Desc A");
    assert_eq!(f_det[1]["description"], "Desc B");

    let ent_page_lim3 = probe(&journal, "list_entities", json!({"limit": 3})).unwrap();
    assert_eq!(ent_page_lim3["entities"].as_array().unwrap().len(), 3);
    assert_eq!(
        resolve_entity_reference(
            journal.path(),
            ent_page_lim3["entities"][0]["reference"].as_str().unwrap()
        ),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_other".to_owned()
        )
    );
    assert_eq!(
        resolve_entity_reference(
            journal.path(),
            ent_page_lim3["entities"][1]["reference"].as_str().unwrap()
        ),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_shared".to_owned()
        )
    );
    assert_eq!(
        resolve_entity_reference(
            journal.path(),
            ent_page_lim3["entities"][2]["reference"].as_str().unwrap()
        ),
        (
            "10000000-0000-4000-8000-000000000002".to_owned(),
            "entity_shared".to_owned()
        )
    );
    assert!(ent_page_lim3.get("next_cursor").is_none());
    let page_lim3_str = ent_page_lim3.to_string();
    assert!(!page_lim3_str.contains("Tail Hidden"));
    assert!(!page_lim3_str.contains("Blocked Entity"));
    assert!(!page_lim3_str.contains("Detached Entity"));

    let ent_page = probe(&journal, "list_entities", json!({"limit": 2})).unwrap();
    assert_eq!(ent_page["entities"].as_array().unwrap().len(), 2);
    let ref0 = ent_page["entities"][0]["reference"].as_str().unwrap();
    let ref1 = ent_page["entities"][1]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), ref0),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_other".to_owned()
        )
    );
    assert_eq!(
        resolve_entity_reference(journal.path(), ref1),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_shared".to_owned()
        )
    );

    let next_cur = ent_page["next_cursor"].as_str().unwrap();
    let ent_page2 = probe(
        &journal,
        "list_entities",
        json!({"cursor": next_cur, "limit": 2}),
    )
    .unwrap();
    assert_eq!(ent_page2["entities"].as_array().unwrap().len(), 1);
    let ref2 = ent_page2["entities"][0]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), ref2),
        (
            "10000000-0000-4000-8000-000000000002".to_owned(),
            "entity_shared".to_owned()
        )
    );
    assert!(ent_page2.get("next_cursor").is_none());

    let ser = ent_page.to_string() + &ent_page2.to_string();
    assert!(!ser.contains("Detached Entity"));
    assert!(!ser.contains("Blocked Entity"));
    assert!(!ser.contains("Tail Hidden"));

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned(), "facets".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec!["10000000-0000-4000-8000-000000000001".to_owned()],
                },
            },
        )
        .unwrap();
    let scoped_facets = probe(&journal, "list_facets", json!({})).unwrap();
    assert_eq!(scoped_facets["facets"].as_array().unwrap().len(), 1);
    assert_eq!(
        scoped_facets["facets"][0]["id"],
        "10000000-0000-4000-8000-000000000001"
    );

    let scoped_entities = probe(&journal, "list_entities", json!({})).unwrap();
    assert_eq!(scoped_entities["entities"].as_array().unwrap().len(), 2);
    let ref_s0 = scoped_entities["entities"][0]["reference"]
        .as_str()
        .unwrap();
    let ref_s1 = scoped_entities["entities"][1]["reference"]
        .as_str()
        .unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), ref_s0),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_other".to_owned()
        )
    );
    assert_eq!(
        resolve_entity_reference(journal.path(), ref_s1),
        (
            "10000000-0000-4000-8000-000000000001".to_owned(),
            "entity_shared".to_owned()
        )
    );
}

#[test]
fn entity_context_and_relationship_error_handling() {
    let journal = tempfile::Builder::new()
        .prefix("entity-error-handling-")
        .tempdir_in(crate::test_scratch())
        .unwrap();

    let f1_dir = journal.path().join("facets/facet1");
    fs::create_dir_all(&f1_dir).unwrap();
    fs::write(
        f1_dir.join("facet.json"),
        json!({"id": "20000000-0000-4000-8000-000000000001", "title": "Facet 1"}).to_string(),
    )
    .unwrap();

    let f2_dir = journal.path().join("facets/facet2");
    fs::create_dir_all(&f2_dir).unwrap();
    fs::write(
        f2_dir.join("facet.json"),
        json!({"id": "20000000-0000-4000-8000-000000000002", "title": "Facet 2"}).to_string(),
    )
    .unwrap();

    write_entity_fixture(
        journal.path(),
        "facet1",
        "rel_common",
        "entity_common",
        "Common Person",
        Some("Person"),
        Some("Role in Facet 1"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "facet2",
        "rel_common",
        "entity_common",
        "Common Person",
        Some("Person"),
        Some("Role in Facet 2"),
        None,
    );

    let f_bad = journal.path().join("facets/facet_bad");
    fs::create_dir_all(&f_bad).unwrap();
    fs::write(f_bad.join("facet.json"), "not json").unwrap();

    let bad_ident_link = f1_dir.join("entities/bad_ident");
    fs::create_dir_all(&bad_ident_link).unwrap();
    fs::write(
        bad_ident_link.join("entity.json"),
        json!({"entity_id": "ent_bad_ident"}).to_string(),
    )
    .unwrap();
    let bad_ident_dir = journal.path().join("entities/bad_ident");
    fs::create_dir_all(&bad_ident_dir).unwrap();
    fs::write(bad_ident_dir.join("entity.json"), "invalid json").unwrap();

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let listed = probe(&journal, "list_entities", json!({})).unwrap();
    assert_eq!(listed["entities"].as_array().unwrap().len(), 2);
    let r0 = listed["entities"][0]["reference"].as_str().unwrap();
    let r1 = listed["entities"][1]["reference"].as_str().unwrap();
    // The same person in two facets: each row names the facet it is from,
    // as list_facets identifies it.
    let facets = probe(&journal, "list_facets", json!({})).unwrap();
    let facet_id = |title: &str| {
        facets["facets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|facet| facet["name"] == title)
            .unwrap()["id"]
            .clone()
    };
    assert_eq!(listed["entities"][0]["facet"], facet_id("Facet 1"));
    assert_eq!(listed["entities"][1]["facet"], facet_id("Facet 2"));

    let g0 = probe(&journal, "get_entity", json!({"reference": r0})).unwrap();
    let g1 = probe(&journal, "get_entity", json!({"reference": r1})).unwrap();
    assert_eq!(g0["description"], "Role in Facet 1");
    assert_eq!(g1["description"], "Role in Facet 2");
    assert!(!g0.to_string().contains("Role in Facet 2"));
    assert!(!g1.to_string().contains("Role in Facet 1"));

    let malformed_rel = f2_dir.join("entities/corrupt_rel");
    fs::create_dir_all(&malformed_rel).unwrap();
    fs::write(malformed_rel.join("entity.json"), "not json").unwrap();

    assert_eq!(
        probe(&journal, "list_entities", json!({})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(&journal, "list_entities", json!({"limit": 1})),
        Err(McpProbeError::Unavailable)
    );

    let filtered_f1 = probe(
        &journal,
        "list_entities",
        json!({"facet": "20000000-0000-4000-8000-000000000001"}),
    )
    .unwrap();
    assert_eq!(filtered_f1["entities"].as_array().unwrap().len(), 1);
    let r_f1 = filtered_f1["entities"][0]["reference"].as_str().unwrap();
    let g_f1 = probe(&journal, "get_entity", json!({"reference": r_f1})).unwrap();
    assert_eq!(g_f1["description"], "Role in Facet 1");
    assert!(!g_f1.to_string().contains("Role in Facet 2"));

    assert_eq!(
        probe(
            &journal,
            "list_entities",
            json!({"facet": "20000000-0000-4000-8000-000000000002"})
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn cursor_validation_and_invalidation() {
    let journal = tempfile::Builder::new()
        .prefix("cursor-invalidation-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_1",
        "ent_1",
        "Entity 1",
        Some("Person"),
        Some("Desc 1"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_2",
        "ent_2",
        "Entity 2",
        Some("Person"),
        Some("Desc 2"),
        None,
    );

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let p_facets = probe(&journal, "list_facets", json!({"limit": 1})).unwrap();
    let facet_cursor = p_facets["next_cursor"].as_str().unwrap();

    let p_entities = probe(
        &journal,
        "list_entities",
        json!({"limit": 1, "facet": FACET_A}),
    )
    .unwrap();
    let entity_cursor = p_entities["next_cursor"].as_str().unwrap();
    let ent_ref = p_entities["entities"][0]["reference"].as_str().unwrap();

    assert_eq!(
        probe(&journal, "list_facets", json!({"cursor": ""})),
        Err(McpProbeError::InvalidInput)
    );
    assert_eq!(
        probe(&journal, "list_entities", json!({"cursor": ""})),
        Err(McpProbeError::InvalidInput)
    );

    let long_cur = "x".repeat(2049);
    assert_eq!(
        probe(&journal, "list_facets", json!({"cursor": long_cur})),
        Err(McpProbeError::InvalidInput)
    );
    assert_eq!(
        probe(&journal, "list_entities", json!({"cursor": long_cur})),
        Err(McpProbeError::InvalidInput)
    );

    assert_eq!(
        probe(&journal, "list_facets", json!({"cursor": "garbage-token"})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(
            &journal,
            "list_entities",
            json!({"cursor": "garbage-token"})
        ),
        Err(McpProbeError::Unavailable)
    );

    assert_eq!(
        probe(&journal, "list_facets", json!({"cursor": entity_cursor})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(&journal, "list_entities", json!({"cursor": facet_cursor})),
        Err(McpProbeError::Unavailable)
    );

    assert_eq!(
        probe(
            &journal,
            "list_entities",
            json!({"cursor": entity_cursor, "facet": FACET_B})
        ),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(&journal, "list_entities", json!({"cursor": entity_cursor})),
        Err(McpProbeError::Unavailable)
    );

    let p_no_filter = probe(&journal, "list_entities", json!({"limit": 1})).unwrap();
    let no_filter_cur = p_no_filter["next_cursor"].as_str().unwrap();
    assert_eq!(
        probe(
            &journal,
            "list_entities",
            json!({"cursor": no_filter_cur, "facet": FACET_A})
        ),
        Err(McpProbeError::Unavailable)
    );

    let other_connection = "bearer:other-connection";
    PermissionStore::open(journal.path())
        .set_permission(other_connection, ReadPermission::default_whole_journal())
        .unwrap();

    assert_eq!(
        run_mcp_probe(
            journal.path(),
            other_connection,
            "list_facets",
            &json!({"cursor": facet_cursor})
        ),
        Err(McpProbeError::Unavailable)
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned(), "facets".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    assert_eq!(
        probe(&journal, "list_facets", json!({"cursor": facet_cursor})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(
            &journal,
            "list_entities",
            json!({"cursor": entity_cursor, "facet": FACET_A})
        ),
        Err(McpProbeError::Unavailable)
    );
    // A cursor dies with the permission it was given under. An entity's name
    // doesn't: it reads while the permission still covers it, and not after.
    assert!(probe(&journal, "get_entity", json!({"reference": ent_ref})).is_ok());
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned(), "facets".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_B.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "get_entity", json!({"reference": ent_ref})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn continuation_limits_and_audit_logging() {
    let journal = tempfile::Builder::new()
        .prefix("continuation-audit-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    for i in 0..10 {
        write_entity_fixture(
            journal.path(),
            "alpha",
            &format!("rel_{:02}", i),
            &format!("ent_{:02}", i),
            &format!("Person {:02}", i),
            Some("Person"),
            Some("Desc"),
            None,
        );
    }
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let p1 = probe(
        &journal,
        "list_entities",
        json!({"limit": 2, "facet": FACET_A}),
    )
    .unwrap();
    assert_eq!(p1["entities"].as_array().unwrap().len(), 2);
    let cur = p1["next_cursor"].as_str().unwrap();

    let p2 = probe(
        &journal,
        "list_entities",
        json!({"limit": 5, "cursor": cur, "facet": FACET_A}),
    )
    .unwrap();
    assert_eq!(p2["entities"].as_array().unwrap().len(), 5);
    let r0 = p2["entities"][0]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), r0),
        (FACET_A.to_owned(), "ent_02".to_owned())
    );

    let interactions = find_audit_files(journal.path(), solstone_core_mcp_audit::INTERACTION_FILE);
    let matched_interaction = interactions
        .iter()
        .filter_map(|p| {
            let val: Value = serde_json::from_str(&fs::read_to_string(p).ok()?).ok()?;
            if val["request"]["arguments"].get("limit") == Some(&json!(5))
                && val["request"]["arguments"].get("cursor").is_some()
            {
                Some(val)
            } else {
                None
            }
        })
        .next()
        .expect("found interaction with limit 5 and cursor");

    assert_eq!(matched_interaction["request"]["arguments"]["limit"], 5);
    assert_eq!(
        matched_interaction["request"]["arguments"]["facet"],
        FACET_A
    );
    let rec_cur = matched_interaction["request"]["arguments"]["cursor"]
        .as_str()
        .unwrap();
    assert_eq!(rec_cur.len(), 16);
    assert!(rec_cur.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(rec_cur, cur);

    let f1 = probe(&journal, "list_facets", json!({"limit": 1})).unwrap();
    let f_cur = f1["next_cursor"].as_str().unwrap();
    let _f2 = probe(
        &journal,
        "list_facets",
        json!({"limit": 1, "cursor": f_cur}),
    )
    .unwrap();

    let interactions_f =
        find_audit_files(journal.path(), solstone_core_mcp_audit::INTERACTION_FILE);
    let matched_f = interactions_f
        .iter()
        .filter_map(|p| {
            let val: Value = serde_json::from_str(&fs::read_to_string(p).ok()?).ok()?;
            if val["tool_name"] == "list_facets"
                && val["request"]["arguments"].get("cursor").is_some()
            {
                Some(val)
            } else {
                None
            }
        })
        .next()
        .expect("found list_facets interaction with cursor");
    let rec_f_cur = matched_f["request"]["arguments"]["cursor"]
        .as_str()
        .unwrap();
    assert_eq!(rec_f_cur.len(), 16);
    assert!(rec_f_cur.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(rec_f_cur, f_cur);
}

#[test]
fn stable_key_resilience_across_mutations() {
    let journal = tempfile::Builder::new()
        .prefix("stable-key-mutations-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    for (i, name) in [(1, "Alice"), (2, "Bob"), (3, "Charlie")] {
        write_entity_fixture(
            journal.path(),
            "alpha",
            &format!("rel_{}", i),
            &format!("ent_{}", i),
            name,
            Some("Person"),
            Some("Desc"),
            None,
        );
    }
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let p1 = probe(&journal, "list_entities", json!({"limit": 2})).unwrap();
    assert_eq!(p1["entities"].as_array().unwrap().len(), 2);
    let cur = p1["next_cursor"].as_str().unwrap();

    let ent3_file = journal.path().join("entities/rel_3/entity.json");
    fs::write(
        ent3_file,
        json!({"id": "ent_3", "name": "Charlie Renamed"}).to_string(),
    )
    .unwrap();

    let p2 = probe(&journal, "list_entities", json!({"cursor": cur})).unwrap();
    assert_eq!(p2["entities"].as_array().unwrap().len(), 1);
    assert_eq!(p2["entities"][0]["name"], "Charlie Renamed");
    let r3 = p2["entities"][0]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), r3),
        (FACET_A.to_owned(), "ent_3".to_owned())
    );

    fs::remove_dir_all(journal.path().join("facets/alpha/entities/rel_2")).unwrap();
    let p2_again = probe(&journal, "list_entities", json!({"cursor": cur})).unwrap();
    assert_eq!(p2_again["entities"].as_array().unwrap().len(), 1);
    assert_eq!(p2_again["entities"][0]["name"], "Charlie Renamed");

    let p_all = probe(&journal, "list_entities", json!({"limit": 2})).unwrap();
    assert!(p_all.get("next_cursor").is_none());
    let cur_charlie = cur;
    fs::remove_dir_all(journal.path().join("facets/alpha/entities/rel_3")).unwrap();
    let empty_out = dispatch_output(&journal, "list_entities", json!({"cursor": cur_charlie}));
    assert!(empty_out.value["entities"].as_array().unwrap().is_empty());
    assert_eq!(
        empty_out.empty_note.as_deref(),
        Some(
            "No entities. This connection can see none that match. This is a complete answer, not an error."
        )
    );

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_2",
        "ent_2",
        "Bob",
        Some("Person"),
        Some("Desc"),
        None,
    );
    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_3",
        "ent_3",
        "Charlie",
        Some("Person"),
        Some("Desc"),
        None,
    );
    let p_base = probe(&journal, "list_entities", json!({"limit": 2})).unwrap();
    let cur_base = p_base["next_cursor"].as_str().unwrap();

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_1b",
        "ent_1b",
        "Alice B",
        Some("Person"),
        Some("Desc"),
        None,
    );

    let cont = probe(&journal, "list_entities", json!({"cursor": cur_base})).unwrap();
    assert_eq!(cont["entities"].as_array().unwrap().len(), 1);
    let cont_resolved: Vec<(String, String)> = cont["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| resolve_entity_reference(journal.path(), e["reference"].as_str().unwrap()))
        .collect();
    assert_eq!(
        cont_resolved,
        vec![(FACET_A.to_owned(), "ent_3".to_owned())]
    );
    assert!(!cont_resolved.contains(&(FACET_A.to_owned(), "ent_1b".to_owned())));

    let fresh = probe(&journal, "list_entities", json!({})).unwrap();
    assert_eq!(fresh["entities"].as_array().unwrap().len(), 4);
    let fresh_resolved: Vec<(String, String)> = fresh["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| resolve_entity_reference(journal.path(), e["reference"].as_str().unwrap()))
        .collect();
    assert!(fresh_resolved.contains(&(FACET_A.to_owned(), "ent_1b".to_owned())));

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_4",
        "ent_4",
        "David",
        Some("Person"),
        Some("Desc"),
        None,
    );
    let cont_after = probe(&journal, "list_entities", json!({"cursor": cur_base})).unwrap();
    assert_eq!(cont_after["entities"].as_array().unwrap().len(), 2);
    let r_e3 = cont_after["entities"][0]["reference"].as_str().unwrap();
    let r_e4 = cont_after["entities"][1]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), r_e3),
        (FACET_A.to_owned(), "ent_3".to_owned())
    );
    assert_eq!(
        resolve_entity_reference(journal.path(), r_e4),
        (FACET_A.to_owned(), "ent_4".to_owned())
    );

    let facet_mut_journal = tempfile::Builder::new()
        .prefix("facet-mutations-")
        .tempdir_in(crate::test_scratch())
        .unwrap();

    let f_c_dir = facet_mut_journal.path().join("facets/dir_a");
    fs::create_dir_all(&f_c_dir).unwrap();
    fs::write(
        f_c_dir.join("facet.json"),
        json!({"id": "30000000-0000-4000-8000-000000000004", "title": "Title 2"}).to_string(),
    )
    .unwrap();

    let f_b_dir = facet_mut_journal.path().join("facets/dir_m");
    fs::create_dir_all(&f_b_dir).unwrap();
    fs::write(
        f_b_dir.join("facet.json"),
        json!({"id": "30000000-0000-4000-8000-000000000003", "title": "Title 1"}).to_string(),
    )
    .unwrap();

    let f_a_dir = facet_mut_journal.path().join("facets/dir_z");
    fs::create_dir_all(&f_a_dir).unwrap();
    fs::write(
        f_a_dir.join("facet.json"),
        json!({"id": "30000000-0000-4000-8000-000000000002", "title": "Title 3"}).to_string(),
    )
    .unwrap();

    PermissionStore::open(facet_mut_journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let p_fa = probe(&facet_mut_journal, "list_facets", json!({"limit": 1})).unwrap();
    let fa_ids: Vec<&str> = p_fa["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(fa_ids, vec!["30000000-0000-4000-8000-000000000002"]);
    let cur_fa = p_fa["next_cursor"].as_str().unwrap();

    // Removing the anchor facet still returns the later ids, with no duplicate and no skip.
    fs::remove_dir_all(&f_a_dir).unwrap();
    let p_fb = probe(
        &facet_mut_journal,
        "list_facets",
        json!({"cursor": cur_fa, "limit": 1}),
    )
    .unwrap();
    let fb_ids: Vec<&str> = p_fb["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(fb_ids, vec!["30000000-0000-4000-8000-000000000003"]);
    let cur_fb = p_fb["next_cursor"].as_str().unwrap();

    let p_fc = probe(
        &facet_mut_journal,
        "list_facets",
        json!({"cursor": cur_fb, "limit": 1}),
    )
    .unwrap();
    let fc_ids: Vec<&str> = p_fc["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(fc_ids, vec!["30000000-0000-4000-8000-000000000004"]);
    assert!(p_fc.get("next_cursor").is_none());

    // Removing the last remaining facet after a cursor that pointed at it is the empty facet result:
    // empty array, empty note "No facets. This connection can see none.", no next_cursor, not Unavailable.
    fs::remove_dir_all(&f_c_dir).unwrap();
    let empty_facet_out =
        dispatch_output(&facet_mut_journal, "list_facets", json!({"cursor": cur_fb}));
    assert!(
        empty_facet_out.value["facets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        empty_facet_out.empty_note.as_deref(),
        Some("No facets. This connection can see none.")
    );
    assert!(empty_facet_out.value.get("next_cursor").is_none());

    // An id inserted before the cursor key is absent from that continuation and present on a fresh list_facets.
    let f_before_dir = facet_mut_journal.path().join("facets/dir_before");
    fs::create_dir_all(&f_before_dir).unwrap();
    fs::write(
        f_before_dir.join("facet.json"),
        json!({"id": "30000000-0000-4000-8000-000000000001", "title": "Title Before"}).to_string(),
    )
    .unwrap();

    let cont_before = probe(&facet_mut_journal, "list_facets", json!({"cursor": cur_fb})).unwrap();
    let cont_before_ids: Vec<&str> = cont_before["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert!(cont_before_ids.is_empty());

    let fresh_facets = probe(&facet_mut_journal, "list_facets", json!({})).unwrap();
    let fresh_facet_ids: Vec<&str> = fresh_facets["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        fresh_facet_ids,
        vec![
            "30000000-0000-4000-8000-000000000001",
            "30000000-0000-4000-8000-000000000003"
        ]
    );

    // An id inserted after the key is present on the continuation. Assert ids, not counts.
    let f_after_dir = facet_mut_journal.path().join("facets/dir_after");
    fs::create_dir_all(&f_after_dir).unwrap();
    fs::write(
        f_after_dir.join("facet.json"),
        json!({"id": "30000000-0000-4000-8000-000000000005", "title": "Title After"}).to_string(),
    )
    .unwrap();

    let cont_after = probe(&facet_mut_journal, "list_facets", json!({"cursor": cur_fb})).unwrap();
    let cont_after_ids: Vec<&str> = cont_after["facets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(cont_after_ids, vec!["30000000-0000-4000-8000-000000000005"]);
}

#[test]
fn get_entity_description_truncation_scalars() {
    let journal = tempfile::Builder::new()
        .prefix("get-entity-truncation-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());

    let desc_2001 = format!("a{}\u{00e9}", "x".repeat(1999));
    assert_eq!(desc_2001.chars().count(), 2001);

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_long",
        "ent_long",
        "Long Description",
        Some("Person"),
        Some(&desc_2001),
        None,
    );

    let desc_2000 = "\u{00e9}".repeat(2000);
    assert_eq!(desc_2000.chars().count(), 2000);
    assert_eq!(desc_2000.len(), 4000);

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_exact",
        "ent_exact",
        "Exact Description",
        Some("Person"),
        Some(&desc_2000),
        None,
    );

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_short",
        "ent_short",
        "Short Description",
        Some("Person"),
        Some("Short description text."),
        None,
    );

    write_entity_fixture(
        journal.path(),
        "alpha",
        "rel_none",
        "ent_none",
        "No Description",
        Some("Person"),
        None,
        None,
    );

    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let listed = probe(&journal, "list_entities", json!({})).unwrap();
    let find_ref = |name: &str| {
        listed["entities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let g_long = probe(
        &journal,
        "get_entity",
        json!({"reference": find_ref("Long Description")}),
    )
    .unwrap();
    assert_eq!(g_long["description_truncated"], true);
    let returned_desc = g_long["description"].as_str().unwrap();
    assert_eq!(returned_desc.chars().count(), 2000);
    assert_eq!(
        returned_desc,
        &desc_2001[..desc_2001.len() - "\u{00e9}".len()]
    );

    let g_exact = probe(
        &journal,
        "get_entity",
        json!({"reference": find_ref("Exact Description")}),
    )
    .unwrap();
    assert!(g_exact.get("description_truncated").is_none());
    assert_eq!(
        g_exact["description"].as_str().unwrap().chars().count(),
        2000
    );

    let g_short = probe(
        &journal,
        "get_entity",
        json!({"reference": find_ref("Short Description")}),
    )
    .unwrap();
    assert!(g_short.get("description_truncated").is_none());
    assert_eq!(g_short["description"], "Short description text.");

    let g_none = probe(
        &journal,
        "get_entity",
        json!({"reference": find_ref("No Description")}),
    )
    .unwrap();
    assert!(g_none.get("description_truncated").is_none());
    assert!(g_none.get("description").is_none());
}

#[test]
fn legacy_duplicate_entity_links_have_one_logical_page_row() {
    let journal = tempfile::Builder::new()
        .prefix("entity-link-continuation-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    for (folder, id, description) in [
        ("a-legacy", "entity-one", "Legacy description"),
        ("entity-one", "entity-one", "Canonical description"),
        ("z-legacy", "entity-one", "Other legacy description"),
        ("entity-two", "entity-two", "Second description"),
    ] {
        write_entity_fixture(
            journal.path(),
            "alpha",
            folder,
            id,
            id,
            None,
            Some(description),
            None,
        );
    }
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let first = probe(&journal, "list_entities", json!({"limit": 1})).unwrap();
    let reference = first["entities"][0]["reference"].as_str().unwrap();
    let entity = probe(&journal, "get_entity", json!({"reference": reference})).unwrap();
    assert_eq!(entity["description"], "Canonical description");
    let cursor = first["next_cursor"].as_str().unwrap();
    fs::remove_dir_all(journal.path().join("facets/alpha/entities/entity-one")).unwrap();
    let continued = probe(
        &journal,
        "list_entities",
        json!({"limit": 1, "cursor": cursor}),
    )
    .unwrap();
    let reference = continued["entities"][0]["reference"].as_str().unwrap();
    assert_eq!(
        resolve_entity_reference(journal.path(), reference),
        (FACET_A.to_owned(), "entity-two".to_owned())
    );
    assert!(continued.get("next_cursor").is_none());
    let fresh = probe(&journal, "list_entities", json!({})).unwrap();
    assert_eq!(fresh["entities"].as_array().unwrap().len(), 2);
    let reference = fresh["entities"][0]["reference"].as_str().unwrap();
    let entity = probe(&journal, "get_entity", json!({"reference": reference})).unwrap();
    assert_eq!(entity["description"], "Legacy description");
}

fn write_ingest_receipt(
    seg_dir: &Path,
    stream: &str,
    day: &str,
    segment: &str,
    tz: Option<&str>,
    utc_offset_seconds: Option<i32>,
) {
    let mut meta = serde_json::Map::new();
    if let Some(tz) = tz {
        meta.insert("tz".to_owned(), json!(tz));
    }
    if let Some(offset) = utc_offset_seconds {
        meta.insert("utc_offset_seconds".to_owned(), json!(offset));
    }
    let event = json!({
        "record_type": "device_ingest",
        "record_version": 1,
        "outcome": "accepted",
        "protocol_version": 3,
        "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "source": "",
        "stream": stream,
        "day": day,
        "segment": segment,
        "files": [],
        "meta": meta,
    });
    fs::write(seg_dir.join("events.jsonl"), format!("{event}\n")).unwrap();
}

fn write_journal_timezone(journal_path: &Path, tz: &str) {
    fs::create_dir_all(journal_path.join("config")).unwrap();
    fs::write(
        journal_path.join("config/journal.json"),
        json!({"identity": {"timezone": tz}}).to_string(),
    )
    .unwrap();
}

#[test]
fn capture_zone_reports_differing_device_clock_across_all_read_surfaces() {
    let journal = fixture();
    allow_legacy_markdown(&journal);
    write_journal_timezone(journal.path(), "America/Denver");
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    write_ingest_receipt(
        &seg_dir,
        STREAM,
        DAY,
        SEGMENT,
        Some("Asia/Tokyo"),
        Some(32400),
    );
    let transcript_105 = (1..=105).map(|i| format!("line {i}\n")).collect::<String>();
    fs::write(seg_dir.join("meeting_transcript.md"), transcript_105).unwrap();

    let expected_zone = json!({"tz": "Asia/Tokyo", "utc_offset_seconds": 32400, "label": "Tokyo"});

    // 1. list_transcripts
    let list = probe(&journal, "list_transcripts", json!({})).unwrap();
    let item = &list["transcripts"][0];
    assert_eq!(item["capture_zone"], expected_zone);
    assert_eq!(item["time"], "09:00-09:05");
    let seg_ref = item["reference"].as_str().unwrap();

    // 2. get_transcript (first page)
    let p1 = probe(&journal, "get_transcript", json!({"reference": seg_ref})).unwrap();
    assert_eq!(p1["entries"].as_array().unwrap().len(), 100);
    assert_eq!(p1["capture_zone"], expected_zone);
    assert!(p1.get("time").is_none());
    let cursor = p1["next_cursor"].as_str().unwrap();

    // 3. get_transcript (continuation page)
    let p2 = probe(
        &journal,
        "get_transcript",
        json!({"reference": seg_ref, "cursor": cursor}),
    )
    .unwrap();
    assert_eq!(p2["entries"].as_array().unwrap().len(), 5);
    assert!(p2["next_cursor"].is_null());
    assert_eq!(p2["capture_zone"], expected_zone);
    assert!(p2.get("time").is_none());

    // 4. search segment link
    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let search_link = &search["results"][0]["segments"][0];
    assert_eq!(search_link["capture_zone"], expected_zone);
    assert_eq!(search_link["time"], "09:00-09:05");

    // 5. fetch segment link
    let entry_ref = search["results"][0]["reference"].as_str().unwrap();
    let fetch = probe(&journal, "fetch", json!({"reference": entry_ref})).unwrap();
    let fetch_link = &fetch["segments"][0];
    assert_eq!(fetch_link["capture_zone"], expected_zone);
    assert_eq!(fetch_link["time"], "09:00-09:05");
}

#[test]
fn capture_zone_omitted_when_device_matches_configured_owner_timezone() {
    let journal = fixture();
    allow_legacy_markdown(&journal);
    write_journal_timezone(journal.path(), "Asia/Tokyo");
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    write_ingest_receipt(
        &seg_dir,
        STREAM,
        DAY,
        SEGMENT,
        Some("Asia/Tokyo"),
        Some(32400),
    );
    let transcript_105 = (1..=105).map(|i| format!("line {i}\n")).collect::<String>();
    fs::write(seg_dir.join("meeting_transcript.md"), transcript_105).unwrap();

    let list = probe(&journal, "list_transcripts", json!({})).unwrap();
    let item = &list["transcripts"][0];
    assert!(item.get("capture_zone").is_none());
    assert_eq!(item["time"], "09:00-09:05");
    let seg_ref = item["reference"].as_str().unwrap();

    let p1 = probe(&journal, "get_transcript", json!({"reference": seg_ref})).unwrap();
    assert!(p1.get("capture_zone").is_none());
    let cursor = p1["next_cursor"].as_str().unwrap();

    let p2 = probe(
        &journal,
        "get_transcript",
        json!({"reference": seg_ref, "cursor": cursor}),
    )
    .unwrap();
    assert!(p2.get("capture_zone").is_none());

    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let search_link = &search["results"][0]["segments"][0];
    assert!(search_link.get("capture_zone").is_none());

    let entry_ref = search["results"][0]["reference"].as_str().unwrap();
    let fetch = probe(&journal, "fetch", json!({"reference": entry_ref})).unwrap();
    let fetch_link = &fetch["segments"][0];
    assert!(fetch_link.get("capture_zone").is_none());
}

#[test]
fn capture_zone_persists_across_paged_list_transcripts() {
    let journal = fixture();
    allow_legacy_markdown(&journal);
    write_journal_timezone(journal.path(), "America/Denver");
    let seg_dir1 = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    write_ingest_receipt(
        &seg_dir1,
        STREAM,
        DAY,
        SEGMENT,
        Some("Asia/Tokyo"),
        Some(32400),
    );

    let seg_dir2 = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join("100000_300");
    fs::create_dir_all(seg_dir2.join("talents")).unwrap();
    fs::write(
        seg_dir2.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        seg_dir2.join("meeting_transcript.md"),
        "second transcript\n",
    )
    .unwrap();
    write_ingest_receipt(
        &seg_dir2,
        STREAM,
        DAY,
        "100000_300",
        Some("Asia/Kolkata"),
        Some(19800),
    );

    let page1 = probe(&journal, "list_transcripts", json!({"limit": 1})).unwrap();
    let transcripts1 = page1["transcripts"].as_array().unwrap();
    assert_eq!(transcripts1.len(), 1);
    assert_eq!(transcripts1[0]["time"], "09:00-09:05");
    assert_eq!(
        transcripts1[0]["capture_zone"],
        json!({"tz": "Asia/Tokyo", "utc_offset_seconds": 32400, "label": "Tokyo"})
    );
    let cursor = page1["next_cursor"].as_str().unwrap();

    let page2 = probe(
        &journal,
        "list_transcripts",
        json!({"limit": 1, "cursor": cursor}),
    )
    .unwrap();
    let transcripts2 = page2["transcripts"].as_array().unwrap();
    assert_eq!(transcripts2.len(), 1);
    assert_eq!(transcripts2[0]["time"], "10:00-10:05");
    assert_eq!(
        transcripts2[0]["capture_zone"],
        json!({"tz": "Asia/Kolkata", "utc_offset_seconds": 19800, "label": "Kolkata"})
    );
}

#[test]
fn capture_zone_omitted_for_home_offset_twin() {
    let journal = fixture();
    allow_legacy_markdown(&journal);
    write_journal_timezone(journal.path(), "America/Denver");
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join("20260929")
        .join(STREAM)
        .join(SEGMENT);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(seg_dir.join("meeting_transcript.md"), "transcript text\n").unwrap();
    write_ingest_receipt(
        &seg_dir,
        STREAM,
        "20260929",
        SEGMENT,
        Some("America/Denver"),
        Some(-21600),
    );

    let list = probe(&journal, "list_transcripts", json!({"day": "20260929"})).unwrap();
    let item = &list["transcripts"][0];
    assert_eq!(item["time"], "09:00-09:05");
    assert!(item.get("capture_zone").is_none());
}

#[test]
fn capture_zone_omitted_when_receipt_is_missing_or_unusable() {
    let journal = fixture();
    write_journal_timezone(journal.path(), "America/Denver");
    // Case A: missing events.jsonl
    let list_missing = probe(&journal, "list_transcripts", json!({})).unwrap();
    assert!(list_missing["transcripts"][0].get("capture_zone").is_none());

    // Case B: unusable receipt (no parsable zone and no offset)
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    write_ingest_receipt(&seg_dir, STREAM, DAY, SEGMENT, None, None);
    let list_unusable = probe(&journal, "list_transcripts", json!({})).unwrap();
    assert!(
        list_unusable["transcripts"][0]
            .get("capture_zone")
            .is_none()
    );
}

#[test]
fn capture_zone_projects_null_tz_when_iana_name_is_invalid_but_offset_is_valid() {
    let journal = fixture();
    write_journal_timezone(journal.path(), "America/Denver");
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    write_ingest_receipt(
        &seg_dir,
        STREAM,
        DAY,
        SEGMENT,
        Some("Not/A_Zone"),
        Some(-12600),
    );

    let list = probe(&journal, "list_transcripts", json!({})).unwrap();
    let item = &list["transcripts"][0];
    assert_eq!(
        item["capture_zone"],
        json!({"tz": null, "utc_offset_seconds": -12600, "label": "UTC-3:30"})
    );
}

#[test]
fn capture_zone_fails_closed_on_malformed_receipts_preserving_transcript_read() {
    let journal = fixture();
    allow_legacy_markdown(&journal);
    write_journal_timezone(journal.path(), "America/Denver");
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    let transcript_105 = (1..=105).map(|i| format!("line {i}\n")).collect::<String>();
    fs::write(seg_dir.join("meeting_transcript.md"), transcript_105).unwrap();

    // 1. Non-JSON line in events.jsonl
    fs::write(seg_dir.join("events.jsonl"), b"not valid json at all\n").unwrap();
    let list = probe(&journal, "list_transcripts", json!({})).unwrap();
    let item = &list["transcripts"][0];
    assert!(item.get("capture_zone").is_none());
    let seg_ref = item["reference"].as_str().unwrap();

    let p1 = probe(&journal, "get_transcript", json!({"reference": seg_ref})).unwrap();
    assert_eq!(p1["entries"].as_array().unwrap().len(), 100);
    assert!(p1["next_cursor"].is_string());
    assert!(p1.get("capture_zone").is_none());

    // 2. events.jsonl replaced with a directory
    fs::remove_file(seg_dir.join("events.jsonl")).unwrap();
    fs::create_dir(seg_dir.join("events.jsonl")).unwrap();

    let p1_dir = probe(&journal, "get_transcript", json!({"reference": seg_ref})).unwrap();
    assert_eq!(p1_dir["entries"].as_array().unwrap().len(), 100);
    assert!(p1_dir["next_cursor"].is_string());
    assert!(p1_dir.get("capture_zone").is_none());
}

/// Every string an agent receives for `value`, over both wire forms.
fn rendered_bytes(value: &Value) -> String {
    crate::jsonrpc::tool_result(value.clone(), None).to_string()
}

#[test]
fn same_time_segments_from_two_macs_carry_distinct_sources_and_no_stream_name() {
    // Two Macs recording one room. Their streams carry the host names the
    // dashboard shows in parentheses, which an agent must never read.
    const STREAMS: [&str; 2] = ["zqboronscarlett", "zqmvecairmic"];
    const SAID: [&str; 2] = ["firstroomtoken", "secondroomtoken"];
    let journal = tempfile::Builder::new()
        .prefix("source-test-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    for (created_at, stream, said) in [(20, STREAMS[0], SAID[0]), (10, STREAMS[1], SAID[1])] {
        let record = json!({
            "name": stream, "kind": "observer", "host": null, "platform": "macos",
            "created_at": created_at, "last_day": null, "last_segment": null, "seq": 1
        });
        fs::create_dir_all(journal.path().join("streams")).unwrap();
        fs::write(
            journal.path().join(format!("streams/{stream}.json")),
            record.to_string(),
        )
        .unwrap();
        let segment = journal
            .path()
            .join("chronicle/20261023")
            .join(stream)
            .join("213000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(
            segment.join("talents/facets.json"),
            "[{\"facet\":\"alpha\"}]",
        )
        .unwrap();
        fs::write(segment.join("meeting_transcript.md"), format!("{said}\n")).unwrap();
    }
    let act_path = "facets/alpha/activities/20261023.jsonl";
    let act_line = json!({"id": "act-room", "segments": ["213000_300"]}).to_string();
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    fs::write(journal.path().join(act_path), format!("{act_line}\n")).unwrap();
    let produced = produce_chunks(Family::Activity, act_path, &act_line);
    let content = format!("{}\nroomneedle", produced.chunks[0].content);
    plant_chunk(
        journal.path(),
        act_path,
        "20261023",
        "",
        0,
        &content,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();

    let mut served = Vec::new();
    let mut read = |reference: &str| {
        let get =
            dispatch_output(&journal, "get_transcript", json!({"reference": reference})).value;
        let text = get["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| literal_display_text(e.as_str().unwrap()))
            .collect::<String>();
        let said = SAID
            .iter()
            .find(|said| text.contains(**said))
            .expect("transcript carries a fixture token");
        let source = get["source"].as_str().unwrap().to_owned();
        served.push(get);
        (*said, source)
    };

    let list = dispatch_output(&journal, "list_transcripts", json!({"day": "20261023"})).value;
    let items = list["transcripts"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let mut sources = std::collections::BTreeMap::new();
    for item in items {
        assert_eq!(item["time"], items[0]["time"]);
        let (said, source) = read(item["reference"].as_str().unwrap());
        assert_eq!(item["source"], source, "a listed source matches its text");
        sources.insert(said, source);
    }
    assert_eq!(
        sources.values().collect::<BTreeSet<_>>().len(),
        2,
        "two devices read as two sources"
    );

    let search = dispatch_output(&journal, "search", json!({"query": "roomneedle"})).value;
    let fetch = dispatch_output(
        &journal,
        "fetch",
        json!({"reference": search["results"][0]["reference"]}),
    )
    .value;
    for links in [&search["results"][0]["segments"], &fetch["segments"]] {
        let links = links.as_array().unwrap();
        assert_eq!(links.len(), 2);
        for link in links {
            let (said, source) = read(link["reference"].as_str().unwrap());
            assert_eq!(link["source"], source, "a linked source matches its text");
            assert_eq!(sources[said], source, "one device keeps one source");
        }
    }

    for value in served.iter().chain([&list, &search, &fetch]) {
        assert_no_coordinate_keys(value);
        let bytes = rendered_bytes(value).to_lowercase();
        for stream in STREAMS {
            assert!(
                !bytes.contains(stream),
                "{stream} reached an agent: {bytes}"
            );
        }
    }
}

#[test]
fn capture_zone_isolates_multi_stream_segments_and_respects_access_grants() {
    let journal = setup_shared_link_journal();
    write_journal_timezone(journal.path(), "America/Denver");
    let chronicle = journal.path().join("chronicle/20261010");
    let alpha_seg = chronicle.join("alpha").join("090000_300");
    let beta_seg = chronicle.join("beta").join("090000_300");

    write_ingest_receipt(
        &alpha_seg,
        "alpha",
        "20261010",
        "090000_300",
        Some("Asia/Tokyo"),
        Some(32400),
    );
    write_ingest_receipt(
        &beta_seg,
        "beta",
        "20261010",
        "090000_300",
        Some("Europe/London"),
        Some(3600),
    );

    let act_path = "facets/alpha/activities/20261010.jsonl";
    let act_line = json!({"id": "act-multi", "segments": ["090000_300"]}).to_string();
    fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
    fs::write(journal.path().join(act_path), format!("{act_line}\n")).unwrap();
    let produced = produce_chunks(Family::Activity, act_path, &act_line);
    let content = format!("{}\nmultistreamneedle", produced.chunks[0].content);
    plant_chunk(
        journal.path(),
        act_path,
        "20261010",
        "",
        0,
        &content,
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), act_path, 0, false);
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();

    let tokyo_zone = json!({"tz": "Asia/Tokyo", "utc_offset_seconds": 32400, "label": "Tokyo"});
    let london_zone = json!({"tz": "Europe/London", "utc_offset_seconds": 3600, "label": "London"});

    // list_transcripts shows each stream's zone
    let list = probe(&journal, "list_transcripts", json!({"day": "20261010"})).unwrap();
    let mut alpha_ref = None;
    let mut beta_ref = None;
    for item in list["transcripts"].as_array().unwrap() {
        let reference = item["reference"].as_str().unwrap();
        let get = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
        let text = get["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| literal_display_text(e.as_str().unwrap()))
            .collect::<Vec<_>>()
            .join(" ");
        if text.contains("alpha_transcript_token") {
            assert_eq!(item["capture_zone"], tokyo_zone);
            assert_eq!(get["capture_zone"], tokyo_zone);
            alpha_ref = Some(reference.to_owned());
        } else if text.contains("beta_transcript_token") {
            assert_eq!(item["capture_zone"], london_zone);
            assert_eq!(get["capture_zone"], london_zone);
            beta_ref = Some(reference.to_owned());
        }
    }
    let alpha_ref = alpha_ref.expect("alpha segment in list");
    let _beta_ref = beta_ref.expect("beta segment in list");

    // Search and fetch activity links carry those two zones
    let search = probe(&journal, "search", json!({"query": "multistreamneedle"})).unwrap();
    let hit = &search["results"][0];
    let seg_links = hit["segments"].as_array().unwrap();
    assert_eq!(seg_links.len(), 2);
    for link in seg_links {
        let reference = link["reference"].as_str().unwrap();
        let get = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
        let text = get["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| literal_display_text(e.as_str().unwrap()))
            .collect::<Vec<_>>()
            .join(" ");
        if text.contains("alpha_transcript_token") {
            assert_eq!(link["capture_zone"], tokyo_zone);
        } else if text.contains("beta_transcript_token") {
            assert_eq!(link["capture_zone"], london_zone);
        } else {
            panic!("unexpected transcript in search segment links: {text}");
        }
    }

    let fetch = probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
    let fetch_links = fetch["segments"].as_array().unwrap();
    assert_eq!(fetch_links.len(), 2);
    for link in fetch_links {
        let reference = link["reference"].as_str().unwrap();
        let get = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
        let text = get["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| literal_display_text(e.as_str().unwrap()))
            .collect::<Vec<_>>()
            .join(" ");
        if text.contains("alpha_transcript_token") {
            assert_eq!(link["capture_zone"], tokyo_zone);
        } else if text.contains("beta_transcript_token") {
            assert_eq!(link["capture_zone"], london_zone);
        } else {
            panic!("unexpected transcript in fetch segment links: {text}");
        }
    }

    // Grant exclusion: point beta segment at facet beta, grant only facet alpha
    fs::write(
        beta_seg.join("talents/facets.json"),
        "[{\"facet\":\"beta\"}]",
    )
    .unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "transcripts".to_owned(),
                    "messages".to_owned(),
                    "facets".to_owned(),
                ],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let list_scoped = probe(&journal, "list_transcripts", json!({"day": "20261010"})).unwrap();
    for item in list_scoped["transcripts"].as_array().unwrap() {
        let reference = item["reference"].as_str().unwrap();
        let get = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
        let text = get["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| literal_display_text(e.as_str().unwrap()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!text.contains("beta_transcript_token"));
        if text.contains("alpha_transcript_token") {
            assert_eq!(item["capture_zone"], tokyo_zone);
        }
    }

    let search_scoped = probe(&journal, "search", json!({"query": "multistreamneedle"})).unwrap();
    let hit_scoped = &search_scoped["results"][0];
    let links_scoped = hit_scoped["segments"].as_array().unwrap();
    assert_eq!(links_scoped.len(), 1);
    assert_eq!(links_scoped[0]["capture_zone"], tokyo_zone);

    // Narrowed grant: connection only granted FACET_B -> alpha_ref returns Unavailable
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "messages".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_B.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "get_transcript", json!({"reference": alpha_ref})),
        Err(McpProbeError::Unavailable)
    );

    // Revoked / tampered reference
    assert_eq!(
        probe(
            &journal,
            "get_transcript",
            json!({"reference": "tampered_ref"})
        ),
        Err(McpProbeError::Unavailable)
    );
    PermissionStore::open(journal.path())
        .set_permission("bearer:other-bearer", legacy_markdown_whole_journal())
        .unwrap();
    // A segment's name is the same for every connection, and each one's own
    // permission decides the read: one that may read it does, one that may
    // not finds nothing.
    assert!(
        run_mcp_probe(
            journal.path(),
            "bearer:other-bearer",
            "get_transcript",
            &json!({"reference": alpha_ref}),
        )
        .is_ok()
    );
    PermissionStore::open(journal.path())
        .set_permission(
            "bearer:other-bearer",
            ReadPermission {
                categories: vec!["transcripts".to_owned(), "messages".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_B.to_owned()],
                },
            },
        )
        .unwrap();
    assert_eq!(
        run_mcp_probe(
            journal.path(),
            "bearer:other-bearer",
            "get_transcript",
            &json!({"reference": alpha_ref}),
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[cfg(unix)]
#[test]
fn capture_zone_does_not_follow_an_excluded_segments_receipt() {
    let journal = fixture();
    allow_legacy_markdown(&journal);
    write_journal_timezone(journal.path(), "America/Denver");
    let alpha = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    let beta = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join("beta")
        .join(SEGMENT);
    fs::create_dir_all(beta.join("talents")).unwrap();
    fs::write(beta.join("talents/facets.json"), r#"[{"facet":"beta"}]"#).unwrap();
    fs::write(beta.join("meeting_transcript.md"), "excluded transcript").unwrap();
    write_ingest_receipt(&beta, "beta", DAY, SEGMENT, Some("Asia/Tokyo"), Some(32400));
    std::os::unix::fs::symlink(beta.join("events.jsonl"), alpha.join("events.jsonl")).unwrap();

    let listed = probe(&journal, "list_transcripts", json!({})).unwrap();
    let segments = listed["transcripts"].as_array().unwrap();
    assert_eq!(segments.len(), 1);
    assert!(segments[0].get("capture_zone").is_none());
    let transcript = probe(
        &journal,
        "get_transcript",
        json!({"reference": segments[0]["reference"]}),
    )
    .unwrap();
    assert!(transcript.get("capture_zone").is_none());
    assert!(transcript.to_string().contains("approved transcript"));
    let found = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let hit = &found["results"][0];
    assert!(hit["segments"][0].get("capture_zone").is_none());
    let fetched = probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
    assert!(fetched["segments"][0].get("capture_zone").is_none());
    assert!(!fetched.to_string().contains("Asia/Tokyo"));
}

fn setup_freeze_barrier_journal() -> tempfile::TempDir {
    let journal = fixture();
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, legacy_markdown_whole_journal())
        .unwrap();

    let seg1_name = "100000_300";
    let seg1_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(seg1_name);
    fs::create_dir_all(seg1_dir.join("talents")).unwrap();
    fs::write(
        seg1_dir.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        seg1_dir.join("meeting_transcript.md"),
        "first approved transcript\n",
    )
    .unwrap();
    fs::write(seg1_dir.join("audio.jsonl"), "first percept\n").unwrap();
    fs::write(seg1_dir.join("talents/audio.md"), "first disk bytes\n").unwrap();

    let seg1_path = format!("{DAY}/{STREAM}/{seg1_name}/talents/audio.md");
    plant_chunk(
        journal.path(),
        &seg1_path,
        DAY,
        STREAM,
        0,
        "freezetestneedle hit 100000",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );

    let seg2_path = format!("{DAY}/{STREAM}/{SEGMENT}/talents/audio.md");
    plant_chunk(
        journal.path(),
        &seg2_path,
        DAY,
        STREAM,
        0,
        "freezetestneedle hit 090000",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );

    journal
}

#[cfg(feature = "test-hooks")]
#[test]
fn removal_after_marker_sampling_never_advertises_a_tombstone_page() {
    use crate::source_freeze_barrier::{FreezePoint, run_with_source_freeze_barrier};

    let journal = setup_freeze_barrier_journal();
    let root = journal.path().to_path_buf();
    let (result, fired) = run_with_source_freeze_barrier(
        FreezePoint::MarkerSampled,
        1,
        move || {
            let target = solstone_core_retention::Target {
                day: DAY.to_owned(),
                stream: STREAM.to_owned(),
                dir: SEGMENT.to_owned(),
            };
            let outcome = solstone_core_retention::door::remove_segments(
                &root,
                &[target],
                "2026-10-17T12:00:00Z",
                solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                "marker-sampling-removal",
            );
            assert!(outcome.targets[0].not_removed.is_empty());
            assert!(
                root.join(format!("chronicle/{DAY}/{STREAM}/{SEGMENT}/tombstone.json"))
                    .is_file()
            );
        },
        || probe(&journal, "list_transcripts", json!({"day": DAY})),
    );
    assert!(fired, "the removal must run after observing a live marker");
    let listed = result.unwrap();
    let transcripts = listed["transcripts"].as_array().unwrap();
    assert_eq!(transcripts.len(), 1, "only the surviving sibling is live");
    assert!(
        transcripts[0]["time"]
            .as_str()
            .unwrap()
            .starts_with("10:00")
    );
    let reference = transcripts[0]["reference"].as_str().unwrap();
    assert!(probe(&journal, "get_transcript", json!({"reference": reference})).is_ok());
}

#[cfg(feature = "test-hooks")]
#[test]
fn removal_after_marker_sampling_refuses_an_old_live_empty_reference() {
    use crate::references::{ReferenceTarget, SegmentReference};
    use crate::source_freeze_barrier::{FreezePoint, run_with_source_freeze_barrier};

    let journal = fixture();
    let root = journal.path().to_path_buf();
    let segment = root.join(format!("chronicle/{DAY}/{STREAM}/{SEGMENT}"));
    fs::remove_file(segment.join("meeting_transcript.md")).unwrap();
    if segment.join("audio.jsonl").exists() {
        fs::remove_file(segment.join("audio.jsonl")).unwrap();
    }
    let list = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    assert_eq!(list["transcripts"], json!([]));
    let segment_object =
        solstone_core_journal_io::Segment::resolve_exact(&root, DAY, STREAM, SEGMENT)
            .unwrap()
            .unwrap();
    let page =
        solstone_core_transcripts::read_segment_transcript_page(&segment_object, None, None, None)
            .unwrap();
    let permission = PermissionStore::open(&root)
        .get_permission(CONNECTION)
        .unwrap()
        .unwrap();
    let reference = crate::dispatch::codec()
        .mint(
            CONNECTION,
            permission.generation,
            ReferenceTarget::Segment(SegmentReference {
                day: DAY.to_owned(),
                stream: STREAM.to_owned(),
                segment: SEGMENT.to_owned(),
                version: page.version.fingerprint(),
                source_index: None,
                byte_offset: None,
                statement_ordinal: None,
                message: None,
            }),
        )
        .unwrap();
    let before = probe(&journal, "get_transcript", json!({"reference": reference})).unwrap();
    assert!(before["entries"].as_array().unwrap().is_empty());
    let (result, fired) = run_with_source_freeze_barrier(
        FreezePoint::MarkerSampled,
        1,
        move || {
            let target = solstone_core_retention::Target {
                day: DAY.to_owned(),
                stream: STREAM.to_owned(),
                dir: SEGMENT.to_owned(),
            };
            let outcome = solstone_core_retention::door::remove_segments(
                &root,
                &[target],
                "2026-10-17T12:00:00Z",
                solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                "empty-marker-sampling-removal",
            );
            assert!(outcome.targets[0].not_removed.is_empty());
            assert!(segment.join("tombstone.json").is_file());
        },
        || probe(&journal, "get_transcript", json!({"reference": reference})),
    );
    assert!(fired, "the removal must run after observing a live marker");
    assert_eq!(result, Err(McpProbeError::Unavailable));
}

#[cfg(unix)]
#[test]
fn symlink_tombstone_withholds_all_segment_reads() {
    let journal = fixture();
    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let fetch_reference = &search["results"][0]["reference"];
    let listed = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let transcript_reference = &listed["transcripts"][0]["reference"];
    let segment = journal
        .path()
        .join(format!("chronicle/{DAY}/{STREAM}/{SEGMENT}"));
    std::os::unix::fs::symlink(
        segment.join("meeting_transcript.md"),
        segment.join("tombstone.json"),
    )
    .unwrap();
    assert_eq!(
        solstone_core_segment::owner_deletion_state(&segment).unwrap(),
        solstone_core_segment::OwnerDeletionState::Occupied,
    );
    assert!(
        probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap()["transcripts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": fetch_reference})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(
        probe(
            &journal,
            "get_transcript",
            json!({"reference": transcript_reference})
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn removed_segment_marker_matrix_withheld_from_all_read_tools() {
    use crate::references::{ReferenceTarget, SegmentReference};

    // 1. tombstone
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        let fetch_ref = search_before["results"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let seg_dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT);
        fs::write(seg_dir.join("tombstone.json"), "{}").unwrap();

        let search_after = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        assert!(search_after["results"].as_array().unwrap().is_empty());

        assert_eq!(
            probe(&journal, "fetch", json!({"reference": fetch_ref})),
            Err(McpProbeError::Unavailable)
        );

        let list_after = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
        assert!(list_after["transcripts"].as_array().unwrap().is_empty());

        let seg_obj =
            solstone_core_journal_io::Segment::resolve_exact(journal.path(), DAY, STREAM, SEGMENT)
                .unwrap()
                .unwrap();
        let page = solstone_core_transcripts::read_segment_transcript_page(
            &seg_obj,
            None,
            solstone_core_talent_runtime::agent_speakers(journal.path()).as_ref(),
            None,
        )
        .unwrap();
        let perm_record = PermissionStore::open(journal.path())
            .get_permission(CONNECTION)
            .unwrap()
            .unwrap();
        let token = crate::dispatch::codec()
            .mint(
                CONNECTION,
                perm_record.generation,
                ReferenceTarget::Segment(SegmentReference {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    segment: SEGMENT.to_owned(),
                    version: page.version.fingerprint(),
                    source_index: None,
                    byte_offset: None,
                    statement_ordinal: None,
                    message: None,
                }),
            )
            .unwrap();
        assert_eq!(
            probe(&journal, "get_transcript", json!({"reference": token})),
            Err(McpProbeError::Unavailable)
        );
    }

    // 2. staged
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        let fetch_ref = search_before["results"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let seg_dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT);
        fs::rename(
            &seg_dir,
            seg_dir
                .parent()
                .unwrap()
                .join(format!(".removing_{SEGMENT}")),
        )
        .unwrap();

        assert!(
            probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            probe(&journal, "fetch", json!({"reference": fetch_ref})),
            Err(McpProbeError::Unavailable)
        );
        assert!(
            probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap()["transcripts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    // 3. absent
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        let fetch_ref = search_before["results"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let seg_dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT);
        fs::remove_dir_all(&seg_dir).unwrap();

        assert!(
            probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            probe(&journal, "fetch", json!({"reference": fetch_ref})),
            Err(McpProbeError::Unavailable)
        );
        assert!(
            probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap()["transcripts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    // 4. occupied
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        let fetch_ref = search_before["results"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();
        let list_before = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
        let transcript_ref = list_before["transcripts"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let seg_dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT);
        fs::write(
            seg_dir
                .parent()
                .unwrap()
                .join(format!(".removing_{SEGMENT}")),
            b"occupied staged coordinate",
        )
        .unwrap();
        assert_eq!(
            solstone_core_segment::owner_deletion_state(&seg_dir).unwrap(),
            solstone_core_segment::OwnerDeletionState::Occupied,
        );

        assert!(
            probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            probe(&journal, "fetch", json!({"reference": fetch_ref})),
            Err(McpProbeError::Unavailable)
        );
        assert!(
            probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap()["transcripts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            probe(
                &journal,
                "get_transcript",
                json!({"reference": transcript_ref})
            ),
            Err(McpProbeError::Unavailable)
        );
    }

    // 5. unreadable
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
        let fetch_ref = search_before["results"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let seg_dir = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(SEGMENT);
        fs::remove_dir_all(&seg_dir).unwrap();
        fs::write(&seg_dir, b"regular file not a directory").unwrap();

        assert!(
            probe(&journal, "search", json!({"query": "indexed"})).unwrap()["results"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            probe(&journal, "fetch", json!({"reference": fetch_ref})),
            Err(McpProbeError::Unavailable)
        );
    }
}

#[test]
fn live_empty_segment_is_omitted_from_listing_but_derived_text_is_searchable() {
    let journal = fixture();
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);

    fs::remove_file(seg_dir.join("meeting_transcript.md")).unwrap();
    if seg_dir.join("audio.jsonl").exists() {
        fs::remove_file(seg_dir.join("audio.jsonl")).unwrap();
    }

    let list = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let transcripts = list["transcripts"].as_array().unwrap();
    assert!(transcripts.is_empty(), "{list}");

    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    assert_eq!(search["results"].as_array().unwrap().len(), 1);
    let fetch_ref = search["results"][0]["reference"].as_str().unwrap();
    let fetched = probe(&journal, "fetch", json!({"reference": fetch_ref})).unwrap();
    assert_eq!(fetched["text"], "indexed bytes");
}

#[test]
fn raw_media_offload_preserves_derived_fetch_and_transcript() {
    let journal = fixture();
    allow_legacy_markdown(&journal);
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);

    fs::write(seg_dir.join("audio.flac"), b"audio flac bytes\n").unwrap();

    let search = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let fetch_ref = search["results"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    let list = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let transcript_ref = list["transcripts"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();

    fs::remove_file(seg_dir.join("audio.flac")).unwrap();

    let fetched = probe(&journal, "fetch", json!({"reference": fetch_ref})).unwrap();
    assert_eq!(fetched["text"], "indexed bytes");

    let transcript = probe(
        &journal,
        "get_transcript",
        json!({"reference": transcript_ref}),
    )
    .unwrap();
    assert!(!transcript["entries"].as_array().unwrap().is_empty());

    let search_after = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    assert_eq!(search_after["results"].as_array().unwrap().len(), 1);
}

#[test]
fn unindexed_planted_sibling_outside_facet_does_not_affect_grant() {
    let journal = fixture();

    let search_before = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    let coverage_before = search_before["coverage"].clone();
    let cursor_before = search_before.get("next_cursor").cloned();

    let parent_dir = journal.path().join("chronicle").join(DAY).join(STREAM);

    let s1 = parent_dir.join("100000_300");
    fs::create_dir_all(&s1).unwrap();
    fs::write(s1.join("tombstone.json"), "{}").unwrap();

    let s2 = parent_dir.join(".removing_110000_300");
    fs::create_dir_all(&s2).unwrap();

    let s3 = parent_dir.join("120000_300");
    fs::create_dir_all(s3.join("talents")).unwrap();
    fs::write(s3.join("talents/audio.md"), "disk bytes\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(s3.join("talents/audio.md"), s3.join("tombstone.json")).unwrap();

    fs::write(parent_dir.join("130000_300"), b"regular file").unwrap();

    let search_after = probe(&journal, "search", json!({"query": "indexed"})).unwrap();
    assert_eq!(
        visible_search_page(&search_after),
        visible_search_page(&search_before)
    );
    let extract_segments_times = |res: &Value| {
        res["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r["segments"].as_array().map(|segs| {
                    segs.iter()
                        .map(|s| s["time"].as_str().unwrap().to_owned())
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        extract_segments_times(&search_after),
        extract_segments_times(&search_before)
    );
    assert_eq!(search_after["coverage"], coverage_before);
    assert_eq!(search_after.get("next_cursor"), cursor_before.as_ref());
}

#[test]
fn door_seam_remove_segments_withholds_retained_references_without_leak() {
    let journal = fixture();
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();

    let seg2_name = "090500_300";
    let seg2_dir = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(seg2_name);
    fs::create_dir_all(seg2_dir.join("talents")).unwrap();
    fs::write(
        seg2_dir.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    fs::write(
        seg2_dir.join("meeting_transcript.md"),
        "second approved transcript\n",
    )
    .unwrap();
    fs::write(seg2_dir.join("audio.jsonl"), "second percept\n").unwrap();
    fs::write(seg2_dir.join("talents/audio.md"), "second disk bytes\n").unwrap();

    let seg2_path = format!("{DAY}/{STREAM}/{seg2_name}/talents/audio.md");
    plant_chunk(
        journal.path(),
        &seg2_path,
        DAY,
        STREAM,
        0,
        "second indexed bytes",
        "transcripts",
        "segment_assigned",
        &[FACET_A],
    );

    let search1 = probe(&journal, "search", json!({"query": "indexed bytes"})).unwrap();
    let results1 = search1["results"].as_array().unwrap();
    assert_eq!(results1.len(), 2);
    let ref1 = results1
        .iter()
        .find(|r| {
            let s = r["snippet"].as_str().unwrap();
            s.contains("indexed bytes") && !s.contains("second")
        })
        .unwrap()["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    let ref2 = results1
        .iter()
        .find(|r| r["snippet"].as_str().unwrap().contains("second"))
        .unwrap()["reference"]
        .as_str()
        .unwrap()
        .to_owned();

    let list1 = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let tr_list1 = list1["transcripts"].as_array().unwrap();
    assert_eq!(tr_list1.len(), 2);
    let tr_ref1 = tr_list1
        .iter()
        .find(|t| {
            t["time"]
                .as_str()
                .map(|s| s.starts_with("09:00"))
                .unwrap_or(false)
        })
        .unwrap()["reference"]
        .as_str()
        .unwrap()
        .to_owned();

    let target1 = solstone_core_retention::Target {
        day: DAY.to_owned(),
        stream: STREAM.to_owned(),
        dir: SEGMENT.to_owned(),
    };
    let outcome = solstone_core_retention::door::remove_segments(
        journal.path(),
        &[target1],
        "2026-10-17T12:00:00Z",
        solstone_core_retention::RemovalReason::OwnerSegmentDelete,
        "door-seam-test",
    );
    assert_eq!(outcome.targets.len(), 1);
    assert!(
        outcome.targets[0].not_removed.is_empty(),
        "segment 1 removal must succeed: {:?}",
        outcome
    );

    fn walk_rel_paths(root: &Path) -> BTreeSet<PathBuf> {
        let mut set = BTreeSet::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if let Ok(rel) = path.strip_prefix(root) {
                        if rel.components().any(|c| c.as_os_str() == "mcp.agent") {
                            continue;
                        }
                        set.insert(rel.to_path_buf());
                    }
                    if path.is_dir() {
                        stack.push(path);
                    }
                }
            }
        }
        set
    }

    let paths_before_probes = walk_rel_paths(journal.path());

    let search_after = probe(&journal, "search", json!({"query": "indexed bytes"})).unwrap();
    let results_after = search_after["results"].as_array().unwrap();
    assert_eq!(results_after.len(), 1);
    assert!(
        results_after[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("second indexed bytes")
    );

    assert_eq!(
        probe(&journal, "fetch", json!({"reference": ref1})),
        Err(McpProbeError::Unavailable)
    );
    let fetched2 = probe(&journal, "fetch", json!({"reference": ref2})).unwrap();
    assert_eq!(fetched2["text"], "second indexed bytes");

    let list_after = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
    let tr_list_after = list_after["transcripts"].as_array().unwrap();
    assert_eq!(tr_list_after.len(), 1);
    assert!(
        tr_list_after[0]["time"]
            .as_str()
            .unwrap()
            .starts_with("09:05")
    );

    assert_eq!(
        probe(&journal, "get_transcript", json!({"reference": tr_ref1})),
        Err(McpProbeError::Unavailable)
    );

    let seg1_obj =
        solstone_core_journal_io::Segment::resolve_exact(journal.path(), DAY, STREAM, SEGMENT)
            .unwrap()
            .unwrap();
    let page1 = solstone_core_transcripts::read_segment_transcript_page(
        &seg1_obj,
        None,
        solstone_core_talent_runtime::agent_speakers(journal.path()).as_ref(),
        None,
    )
    .unwrap();
    let perm_record = PermissionStore::open(journal.path())
        .get_permission(CONNECTION)
        .unwrap()
        .unwrap();
    let fresh_token = crate::dispatch::codec()
        .mint(
            CONNECTION,
            perm_record.generation,
            crate::references::ReferenceTarget::Segment(crate::references::SegmentReference {
                day: DAY.to_owned(),
                stream: STREAM.to_owned(),
                segment: SEGMENT.to_owned(),
                version: page1.version.fingerprint(),
                source_index: None,
                byte_offset: None,
                statement_ordinal: None,
                message: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(
            &journal,
            "get_transcript",
            json!({"reference": fresh_token})
        ),
        Err(McpProbeError::Unavailable)
    );

    let paths_after_probes = walk_rel_paths(journal.path());
    assert_eq!(paths_before_probes, paths_after_probes);
}

#[test]
fn search_examination_budget_continuation_over_withheld_candidates() {
    use crate::references::{ReferenceKind, ReferenceTarget};

    // Block 1: 1001 removed rows, names 000000_0001 through 000000_1001, no later live row.
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let mut index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
        let tx = index.transaction().unwrap();
        for i in 1..=1001 {
            let seg = format!("000000_{i:04}");
            let path = format!("20260914/default/{seg}/talents/audio.md");
            let seg_dir = journal
                .path()
                .join("chronicle")
                .join("20260914")
                .join("default")
                .join(&seg);
            fs::create_dir_all(&seg_dir).unwrap();
            fs::write(seg_dir.join("tombstone.json"), "{}").unwrap();

            let content = "budgetexamneedle removed";
            tx.execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
                 VALUES (?1, ?2, '20260914', '', 'fixture-stream', 'default', 0, '')",
                params![content, path],
            )
            .unwrap();
            tx.execute(
                "INSERT OR REPLACE INTO chunk_classification(path, category, basis, eligible, unclassified) \
                 VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
                params![path],
            )
            .unwrap();
            tx.execute(
                "INSERT OR REPLACE INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
                params![path, FACET_A],
            )
            .unwrap();
        }
        tx.execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) \
             VALUES (1, '', 1, 0, NULL, 0)",
            [],
        )
        .unwrap();
        tx.execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) \
             VALUES (1, 1, 'complete', 1002, 1002)",
            [],
        )
        .unwrap();
        tx.commit().unwrap();

        let page1 = probe(&journal, "search", json!({"query": "budgetexamneedle"})).unwrap();
        assert!(page1["results"].as_array().unwrap().is_empty());
        assert_eq!(page1["coverage"]["live_examination_complete"], false);
        for res in page1["results"].as_array().unwrap() {
            assert!(
                !res["snippet"]
                    .as_str()
                    .unwrap()
                    .contains("budgetexamneedle removed")
            );
        }
        let token = page1["next_cursor"]
            .as_str()
            .expect("must return continuation cursor at examination budget");

        let perm_record = PermissionStore::open(journal.path())
            .get_permission(CONNECTION)
            .unwrap()
            .unwrap();
        let ReferenceTarget::Cursor(cursor) = crate::dispatch::codec()
            .resolve(
                token,
                ReferenceKind::Cursor,
                CONNECTION,
                perm_record.generation,
            )
            .unwrap()
        else {
            panic!("decoded target must be Cursor");
        };
        assert!(
            cursor.path.contains("000000_0002"),
            "1000th examined row path must be 000000_0002, got {}",
            cursor.path
        );
        assert!(
            !cursor.path.contains("000000_0001"),
            "000000_0001 must not have been reached yet, got {}",
            cursor.path
        );
    }

    // Block 2: 000000_0001 through 000000_1000 removed, plus one live segment on day 20260913
    {
        let journal = fixture();
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, ReadPermission::default_whole_journal())
            .unwrap();

        let mut index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
        let tx = index.transaction().unwrap();
        for i in 1..=1000 {
            let seg = format!("000000_{i:04}");
            let path = format!("20260914/default/{seg}/talents/audio.md");
            let seg_dir = journal
                .path()
                .join("chronicle")
                .join("20260914")
                .join("default")
                .join(&seg);
            fs::create_dir_all(&seg_dir).unwrap();
            fs::write(seg_dir.join("tombstone.json"), "{}").unwrap();

            let content = "budgetexamneedle removed";
            tx.execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
                 VALUES (?1, ?2, '20260914', '', 'fixture-stream', 'default', 0, '')",
                params![content, path],
            )
            .unwrap();
            tx.execute(
                "INSERT OR REPLACE INTO chunk_classification(path, category, basis, eligible, unclassified) \
                 VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
                params![path],
            )
            .unwrap();
            tx.execute(
                "INSERT OR REPLACE INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
                params![path, FACET_A],
            )
            .unwrap();
        }

        let live_seg = journal
            .path()
            .join("chronicle")
            .join("20260913")
            .join("default")
            .join("090000_300");
        fs::create_dir_all(live_seg.join("talents")).unwrap();
        fs::write(live_seg.join("talents/audio.md"), "live brief bytes\n").unwrap();
        fs::write(live_seg.join("meeting_transcript.md"), "live transcript\n").unwrap();

        let live_path = "20260913/default/090000_300/talents/audio.md";
        tx.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES ('budgetexamneedle live', ?1, '20260913', '', 'fixture-stream', 'default', 0, '')",
            params![live_path],
        )
        .unwrap();
        tx.execute(
            "INSERT OR REPLACE INTO chunk_classification(path, category, basis, eligible, unclassified) \
             VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
            params![live_path],
        )
        .unwrap();
        tx.execute(
            "INSERT OR REPLACE INTO chunk_classification_facets(path, facet_id) VALUES (?1, ?2)",
            params![live_path, FACET_A],
        )
        .unwrap();

        tx.execute(
            "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) \
             VALUES (1, '', 1, 0, NULL, 0)",
            [],
        )
        .unwrap();
        tx.execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) \
             VALUES (1, 1, 'complete', 1002, 1002)",
            [],
        )
        .unwrap();
        tx.commit().unwrap();

        let page1 = probe(&journal, "search", json!({"query": "budgetexamneedle"})).unwrap();
        assert!(page1["results"].as_array().unwrap().is_empty());
        assert_eq!(page1["coverage"]["live_examination_complete"], false);
        let cursor = page1["next_cursor"]
            .as_str()
            .expect("must return continuation cursor at examination budget");

        let page2 = probe(
            &journal,
            "search",
            json!({"query": "budgetexamneedle", "cursor": cursor}),
        )
        .unwrap();
        let results2 = page2["results"].as_array().unwrap();
        assert_eq!(results2.len(), 1, "page 2 must find the live hit");
        assert!(
            results2[0]["snippet"]
                .as_str()
                .unwrap()
                .contains("budgetexamneedle live")
        );
        assert!(!results2[0]["snippet"].as_str().unwrap().contains("removed"));
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn source_freeze_barrier_atomicity_withhold_vs_commit() {
    use crate::source_freeze_barrier::{FreezePoint, run_with_source_freeze_barrier};

    // 1. Search, Before, ordinal 2: remove_segments on 090000_300
    {
        let journal = setup_freeze_barrier_journal();
        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::Before,
            2,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-search-before",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "search", json!({"query": "freezetestneedle"})),
        );
        assert!(fired, "Before ordinal 2 barrier must fire");
        let results = res.unwrap()["results"].as_array().unwrap().clone();
        assert_eq!(results.len(), 1);
        assert!(results[0]["snippet"].as_str().unwrap().contains("100000"));
        assert!(!results[0]["snippet"].as_str().unwrap().contains("090000"));
        assert!(!results[0]["segments"].as_array().unwrap().is_empty());
    }

    // 2. Search, After, ordinal 2: remove_segments on 090000_300
    {
        let journal = setup_freeze_barrier_journal();
        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::After,
            2,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-search-after",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "search", json!({"query": "freezetestneedle"})),
        );
        assert!(fired, "After ordinal 2 barrier must fire");
        let results = res.unwrap()["results"].as_array().unwrap().clone();
        assert_eq!(results.len(), 2);
        let hit_09 = results
            .iter()
            .find(|r| r["snippet"].as_str().unwrap().contains("090000"))
            .unwrap();
        assert!(!hit_09["segments"].as_array().unwrap().is_empty());
    }

    // 3. Fetch of 090000_300 entry reference, Before ordinal 1
    {
        let journal = setup_freeze_barrier_journal();
        let search = probe(&journal, "search", json!({"query": "freezetestneedle"})).unwrap();
        let results = search["results"].as_array().unwrap();
        let ref_09 = results
            .iter()
            .find(|r| r["snippet"].as_str().unwrap().contains("090000"))
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::Before,
            1,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-fetch-before",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "fetch", json!({"reference": ref_09})),
        );
        assert!(fired, "Before ordinal 1 barrier must fire");
        assert_eq!(res, Err(McpProbeError::Unavailable));
    }

    // 4. Fetch of 090000_300 entry reference, After ordinal 1
    {
        let journal = setup_freeze_barrier_journal();
        let search = probe(&journal, "search", json!({"query": "freezetestneedle"})).unwrap();
        let results = search["results"].as_array().unwrap();
        let ref_09 = results
            .iter()
            .find(|r| r["snippet"].as_str().unwrap().contains("090000"))
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::After,
            1,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-fetch-after",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "fetch", json!({"reference": ref_09})),
        );
        assert!(fired, "After ordinal 1 barrier must fire");
        let val = res.unwrap();
        assert_eq!(val["text"], "freezetestneedle hit 090000");
        assert!(!val["segments"].as_array().unwrap().is_empty());
    }

    // 5. Get of 090000_300 transcript reference, Before ordinal 1
    {
        let journal = setup_freeze_barrier_journal();
        let list = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
        let transcripts = list["transcripts"].as_array().unwrap();
        let tr_ref_09 = transcripts
            .iter()
            .find(|t| {
                t["time"]
                    .as_str()
                    .map(|s| s.starts_with("09:00"))
                    .unwrap_or(false)
            })
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::Before,
            1,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-get-before",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "get_transcript", json!({"reference": tr_ref_09})),
        );
        assert!(fired, "Before ordinal 1 barrier must fire");
        assert_eq!(res, Err(McpProbeError::Unavailable));
    }

    // 6. Get of 090000_300 transcript reference, After ordinal 1
    {
        let journal = setup_freeze_barrier_journal();
        let list = probe(&journal, "list_transcripts", json!({"day": DAY})).unwrap();
        let transcripts = list["transcripts"].as_array().unwrap();
        let tr_ref_09 = transcripts
            .iter()
            .find(|t| {
                t["time"]
                    .as_str()
                    .map(|s| s.starts_with("09:00"))
                    .unwrap_or(false)
            })
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned();

        let journal_path = journal.path().to_path_buf();
        let (res, fired) = run_with_source_freeze_barrier(
            FreezePoint::After,
            1,
            move || {
                let target = solstone_core_retention::Target {
                    day: DAY.to_owned(),
                    stream: STREAM.to_owned(),
                    dir: SEGMENT.to_owned(),
                };
                let outcome = solstone_core_retention::door::remove_segments(
                    &journal_path,
                    &[target],
                    "2026-10-17T12:00:00Z",
                    solstone_core_retention::RemovalReason::OwnerSegmentDelete,
                    "freeze-get-after",
                );
                assert!(outcome.targets[0].not_removed.is_empty());
            },
            || probe(&journal, "get_transcript", json!({"reference": tr_ref_09})),
        );
        assert!(fired, "After ordinal 1 barrier must fire");
        let val = res.unwrap();
        let entries = val["entries"].as_array().unwrap();
        assert!(!entries.is_empty());
        assert!(entries[0].as_str().unwrap().contains("approved transcript"));
    }
}

fn grant_whole_journal(journal: &Path) {
    PermissionStore::open(journal)
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
}

fn write_activity(journal: &Path, rel: &str, body: &str) {
    let path = journal.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn activity_hits_for(journal: &tempfile::TempDir, query: &str) -> Vec<Value> {
    probe(journal, "search", json!({"query": query, "limit": 10})).unwrap()["results"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn activity_hits_leave_out_a_hidden_activity_and_keep_saved_prose() {
    let journal = tempfile::Builder::new()
        .prefix("activity-hidden-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let hidden_body = format!("hiddenstorytoken {}", "z".repeat(2_000));
    let visible_body = "visiblestorytoken - Hidden: yes and the activity stays visible";
    let day = "facets/alpha/activities/20261010.jsonl";
    let body = format!(
        "{}\n{}\n",
        json!({
            "id": "hid",
            "title": "Concealed",
            "hidden": true,
            "story": {"body": hidden_body}
        }),
        json!({
            "id": "vis",
            "title": "Shown",
            "hidden": false,
            "story": {"body": visible_body}
        })
    );
    write_activity(journal.path(), day, &body);
    write_chronicle_segment(
        journal.path(),
        "20261010",
        "alpha",
        "090000_300",
        "alpha",
        "transcriptonlyneedle",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_whole_journal(journal.path());
    let source = fs::read(journal.path().join(day)).unwrap();

    // A hidden activity isn't indexed, and isn't returned; the visible one
    // keeps its own position.
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    let positions: Vec<i64> = index
        .prepare("SELECT idx FROM chunks WHERE path = ?1 ORDER BY idx")
        .unwrap()
        .query_map([day], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    drop(index);
    assert_eq!(positions, [1]);
    assert!(activity_hits_for(&journal, "hiddenstorytoken").is_empty());

    let visible_hits = activity_hits_for(&journal, "visiblestorytoken");
    assert_eq!(visible_hits.len(), 1);
    assert_eq!(visible_hits[0]["hidden"], false);
    let visible_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": visible_hits[0]["reference"]}),
    )
    .unwrap();
    assert_eq!(visible_fetch["hidden"], false);
    let vis_obj: Value = serde_json::from_str(visible_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(vis_obj["id"], "vis");
    assert!(
        vis_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("visiblestorytoken")
    );
    assert!(
        !visible_fetch["text"]
            .as_str()
            .unwrap()
            .contains("hiddenstorytoken")
    );

    let transcript_hits = activity_hits_for(&journal, "transcriptonlyneedle");
    assert_eq!(transcript_hits.len(), 1);
    assert!(transcript_hits[0].get("hidden").is_none());
    let transcript_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": transcript_hits[0]["reference"]}),
    )
    .unwrap();
    assert!(transcript_fetch.get("hidden").is_none());
    assert_eq!(fs::read(journal.path().join(day)).unwrap(), source);
}

#[test]
fn activity_changes_stay_out_until_the_file_is_indexed_again() {
    let journal = tempfile::Builder::new()
        .prefix("activity-change-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let day = "facets/alpha/activities/20261010.jsonl";
    let twin_a = "facets/alpha/activities/20261011.jsonl";
    let twin_b = "facets/alpha/activities/20261012.jsonl";
    write_activity(
        journal.path(),
        day,
        &format!(
            "{}\n",
            json!({"id":"one","title":"First","story":{"body":"firststoryneedle"}})
        ),
    );
    let twin_body = |id: &str| {
        format!(
            "{}\n",
            json!({"id": id, "title": "Twin", "story": {"body": "sameprose"}})
        )
    };
    write_activity(journal.path(), twin_a, &twin_body("a"));
    write_activity(journal.path(), twin_b, &twin_body("b"));
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_whole_journal(journal.path());

    let hits = activity_hits_for(&journal, "firststoryneedle");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["hidden"], false);
    let reference = hits[0]["reference"].as_str().unwrap().to_owned();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    let f_obj: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(f_obj["id"], "one");
    assert!(
        f_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("firststoryneedle")
    );
    assert!(!fetched["text"].as_str().unwrap().contains("sameprose"));

    let index_path = solstone_core_indexer_store::db::db_path(journal.path());
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    conn.execute("DELETE FROM activity_provenance", []).unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "firststoryneedle").is_empty());
    let missing = probe(&journal, "search", json!({"query": "firststoryneedle"})).unwrap();
    assert_eq!(missing["coverage"]["live_examination_complete"], false);
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );

    let day_path = journal.path().join(day);
    let mtime = fs::metadata(&day_path).unwrap().modified().unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let repaired_hits = activity_hits_for(&journal, "firststoryneedle");
    assert_eq!(repaired_hits.len(), 1);
    let repaired_reference = repaired_hits[0]["reference"].as_str().unwrap().to_owned();
    let repaired_fetch =
        probe(&journal, "fetch", json!({"reference": repaired_reference})).unwrap();
    let rep_obj: Value = serde_json::from_str(repaired_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(rep_obj["id"], "one");
    assert!(
        rep_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("firststoryneedle")
    );
    // The reference from before the repair names rows the repair replaced.
    // The file's bytes and the record still match its proof, so it reads the
    // same activity, marked as indexed again since that search.
    let earlier = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    assert_eq!(earlier["text"], repaired_fetch["text"]);
    assert_eq!(earlier["reindexed_since_search"], true);

    let conn = rusqlite::Connection::open(&index_path).unwrap();
    conn.execute(
        "UPDATE chunks SET content = 'legacysavedprose' WHERE path = ?1",
        [day],
    )
    .unwrap();
    drop(conn);
    let legacy_hits = activity_hits_for(&journal, "legacysavedprose");
    assert_eq!(legacy_hits.len(), 1);
    assert_eq!(legacy_hits[0]["snippet"], "legacysavedprose");
    assert_eq!(legacy_hits[0]["hidden"], false);
    let legacy_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": legacy_hits[0]["reference"]}),
    )
    .unwrap();
    let leg_obj: Value = serde_json::from_str(legacy_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(leg_obj["id"], "one");
    assert!(
        leg_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("firststoryneedle")
    );
    assert_ne!(legacy_fetch["text"], "legacysavedprose");
    let legacy_reference = legacy_hits[0]["reference"].as_str().unwrap().to_owned();

    restore_mtime(&day_path, mtime);
    write_activity(
        journal.path(),
        day,
        &format!(
            "{}\n",
            json!({
                "id": "two",
                "title": "Second",
                "story": {"body": format!("secondstoryneedle {}", "q".repeat(50))}
            })
        ),
    );
    restore_mtime(&day_path, mtime);
    assert!(activity_hits_for(&journal, "legacysavedprose").is_empty());
    assert!(activity_hits_for(&journal, "secondstoryneedle").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": legacy_reference})),
        Err(McpProbeError::Unavailable)
    );

    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let second_hits = activity_hits_for(&journal, "secondstoryneedle");
    assert_eq!(second_hits.len(), 1);
    assert_eq!(second_hits[0]["hidden"], false);
    let second_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": second_hits[0]["reference"]}),
    )
    .unwrap();
    let sec_obj: Value = serde_json::from_str(second_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(sec_obj["id"], "two");
    assert!(
        sec_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("secondstoryneedle")
    );
    assert_eq!(second_fetch["hidden"], false);
    assert_ne!(second_fetch["text"], "legacysavedprose");

    let second_mtime = fs::metadata(&day_path).unwrap().modified().unwrap();
    write_activity(
        journal.path(),
        day,
        &format!(
            "{}\n",
            json!({"id":"three","title":"Third","story":{"body":"thirdstoryneedle"}})
        ),
    );
    restore_mtime(&day_path, second_mtime);
    assert!(activity_hits_for(&journal, "secondstoryneedle").is_empty());
    assert!(activity_hits_for(&journal, "thirdstoryneedle").is_empty());
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let third_hits = activity_hits_for(&journal, "thirdstoryneedle");
    assert_eq!(third_hits.len(), 1);
    assert_eq!(third_hits[0]["hidden"], false);
    let third_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": third_hits[0]["reference"]}),
    )
    .unwrap();
    let third_obj: Value = serde_json::from_str(third_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(third_obj["id"], "three");
    assert!(
        third_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("thirdstoryneedle")
    );
    assert_ne!(third_fetch["text"], "legacysavedprose");

    let twin_hits = activity_hits_for(&journal, "sameprose");
    assert_eq!(twin_hits.len(), 2);
    let twin_refs: Vec<String> = twin_hits
        .iter()
        .map(|hit| hit["reference"].as_str().unwrap().to_owned())
        .collect();
    write_activity(journal.path(), twin_a, &twin_body("a-changed"));
    let remaining = activity_hits_for(&journal, "sameprose");
    assert_eq!(remaining.len(), 1);
    let remaining_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": remaining[0]["reference"]}),
    )
    .unwrap();
    let rem_obj: Value = serde_json::from_str(remaining_fetch["text"].as_str().unwrap()).unwrap();
    assert_eq!(rem_obj["id"], "b");
    assert!(
        rem_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("sameprose")
    );
    assert!(
        !remaining_fetch["text"]
            .as_str()
            .unwrap()
            .contains("\"id\":\"a\"")
    );
    let mut still_reads = 0;
    let mut withheld = 0;
    for reference in &twin_refs {
        match probe(&journal, "fetch", json!({"reference": reference})) {
            Ok(value) => {
                still_reads += 1;
                let val_obj: Value = serde_json::from_str(value["text"].as_str().unwrap()).unwrap();
                assert_eq!(val_obj["id"], "b");
                assert!(
                    val_obj["story"]["body"]
                        .as_str()
                        .unwrap()
                        .contains("sameprose")
                );
                assert!(!value["text"].as_str().unwrap().contains("\"id\":\"a\""));
            }
            Err(McpProbeError::Unavailable) => withheld += 1,
            Err(error) => panic!("{error:?}"),
        }
    }
    assert_eq!(still_reads, 1);
    assert_eq!(withheld, 1);

    let sym = "facets/alpha/activities/20261013.jsonl";
    let sym_body = "{\"id\":\"sym\",\"title\":\"Sym\"}\n";
    write_activity(journal.path(), sym, sym_body);
    plant_chunk(
        journal.path(),
        sym,
        "20261013",
        "",
        0,
        "symactivityneedle",
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), sym, 0, false);
    assert_eq!(activity_hits_for(&journal, "symactivityneedle").len(), 1);
    let sym_target = journal.path().join("symlink-target.jsonl");
    fs::write(&sym_target, sym_body).unwrap();
    #[cfg(unix)]
    {
        let sym_path = journal.path().join(sym);
        let sym_bytes = fs::read(&sym_path).unwrap();
        fs::remove_file(&sym_path).unwrap();
        std::os::unix::fs::symlink(&sym_target, &sym_path).unwrap();
        assert!(activity_hits_for(&journal, "symactivityneedle").is_empty());
        assert_eq!(fs::read(&sym_target).unwrap(), sym_bytes);
    }

    let ugly = "facets/alpha/activities/20261014.jsonl";
    let mut ugly_bytes = b"{\"id\":\"ok\",\"title\":\"Ok\"}\n".to_vec();
    ugly_bytes.push(0xff);
    write_activity(journal.path(), ugly, "");
    fs::write(journal.path().join(ugly), &ugly_bytes).unwrap();
    plant_chunk(
        journal.path(),
        ugly,
        "20261014",
        "",
        0,
        "utf8needle",
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    let digest = Sha256::digest(&ugly_bytes);
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    let ugly_row: i64 = conn
        .query_row(
            "SELECT rowid FROM chunks WHERE path = ?1 AND idx = 0",
            [ugly],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO activity_provenance(rowid, digest, hidden) VALUES (?1, ?2, 0)",
        params![ugly_row, digest.as_slice()],
    )
    .unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "utf8needle").is_empty());

    let big = "facets/alpha/activities/20261015.jsonl";
    let story = "y".repeat(crate::activity_admit::ACTIVITY_SOURCE_BUDGET as usize);
    let big_line = format!(
        "{}\n",
        json!({"id":"big","title":"Big","story":{"body": story}})
    );
    assert!(big_line.len() as u64 > crate::activity_admit::ACTIVITY_SOURCE_BUDGET);
    write_activity(journal.path(), big, &big_line);
    plant_chunk(
        journal.path(),
        big,
        "20261015",
        "",
        0,
        "bigactivityneedle",
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    let big_digest = Sha256::digest(big_line.as_bytes());
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    let big_row: i64 = conn
        .query_row(
            "SELECT rowid FROM chunks WHERE path = ?1 AND idx = 0",
            [big],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO activity_provenance(rowid, digest, hidden) VALUES (?1, ?2, 0)",
        params![big_row, big_digest.as_slice()],
    )
    .unwrap();
    drop(conn);
    let big_before = fs::read(journal.path().join(big)).unwrap();
    assert!(activity_hits_for(&journal, "bigactivityneedle").is_empty());
    assert_eq!(fs::read(journal.path().join(big)).unwrap(), big_before);

    let quiet = "facets/alpha/activities/20261016.jsonl";
    let quiet_body = "{\"id\":\"quiet\",\"title\":\"Quiet\"}\n";
    write_activity(journal.path(), quiet, quiet_body);
    plant_chunk(
        journal.path(),
        quiet,
        "20261016",
        "",
        0,
        "quietactivityneedle",
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), quiet, 0, false);
    assert_eq!(activity_hits_for(&journal, "quietactivityneedle").len(), 1);
    let quiet_reference = activity_hits_for(&journal, "quietactivityneedle")[0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    conn.execute(
        "UPDATE activity_provenance SET digest = ?1 WHERE rowid = (SELECT rowid FROM chunks WHERE path = ?2 AND idx = 0)",
        params![vec![0x44_u8; 32], quiet],
    )
    .unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "quietactivityneedle").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": quiet_reference})),
        Err(McpProbeError::Unavailable)
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !nix::unistd::Uid::effective().is_root() {
            let sealed = "facets/alpha/activities/20261017.jsonl";
            let sealed_body = "{\"id\":\"sealed\",\"title\":\"Sealed\"}\n";
            write_activity(journal.path(), sealed, sealed_body);
            plant_chunk(
                journal.path(),
                sealed,
                "20261017",
                "",
                0,
                "sealedactivityneedle",
                "facets",
                "facet_owned",
                &[FACET_A],
            );
            plant_activity_proof(journal.path(), sealed, 0, false);
            assert_eq!(activity_hits_for(&journal, "sealedactivityneedle").len(), 1);
            let sealed_path = journal.path().join(sealed);
            let mut permissions = fs::metadata(&sealed_path).unwrap().permissions();
            permissions.set_mode(0o0);
            fs::set_permissions(&sealed_path, permissions.clone()).unwrap();
            assert!(activity_hits_for(&journal, "sealedactivityneedle").is_empty());
            permissions.set_mode(0o644);
            fs::set_permissions(&sealed_path, permissions).unwrap();
            assert_eq!(fs::read(&sealed_path).unwrap(), sealed_body.as_bytes());
        }
    }
}

#[test]
fn activity_read_withholds_when_the_index_changes_before_the_proof() {
    use crate::activity_admit::{
        arm_activity_read_seam, release_activity_read_seam, wait_until_activity_read_seamed,
    };

    let journal = tempfile::Builder::new()
        .prefix("activity-seam-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let rel = "facets/alpha/activities/20261010.jsonl";
    let body = "{\"id\":\"seam\",\"title\":\"Seam\",\"story\":{\"body\":\"earlierstoryneedle\"}}\n";
    write_activity(journal.path(), rel, body);
    write_chronicle_segment(
        journal.path(),
        "20261010",
        "alpha",
        "090000_300",
        "alpha",
        "seamlinktoken",
    );
    plant_chunk(
        journal.path(),
        rel,
        "20261010",
        "",
        0,
        "earlierstoryneedle",
        "facets",
        "facet_owned",
        &[FACET_A],
    );
    plant_activity_proof(journal.path(), rel, 0, false);
    grant_whole_journal(journal.path());
    let source = journal.path().join(rel);
    let before = fs::read(&source).unwrap();
    let lock = journal.path().join("indexer/journal.sqlite.lock");
    assert!(!lock.exists());

    let hits = activity_hits_for(&journal, "earlierstoryneedle");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["hidden"], false);
    assert_eq!(hits[0]["snippet"], "earlierstoryneedle");
    let reference = hits[0]["reference"].as_str().unwrap().to_owned();
    assert!(!lock.exists());
    assert_eq!(fs::read(&source).unwrap(), before);

    let root = journal.path().to_path_buf();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let worker_root = root.clone();
    let worker = std::thread::spawn(move || {
        let id = std::thread::current().id();
        ready_tx.send(id).unwrap();
        let _guard = arm_activity_read_seam(id);
        crate::dispatch::run_mcp_probe(
            &worker_root,
            CONNECTION,
            "search",
            &json!({"query": "earlierstoryneedle", "limit": 10}),
        )
    });
    let id = ready_rx.recv().unwrap();
    wait_until_activity_read_seamed(id);
    let index_path = solstone_core_indexer_store::db::db_path(&root);
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    let rowid: i64 = conn
        .query_row(
            "SELECT rowid FROM chunks WHERE path = ?1 AND idx = 0",
            [rel],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE chunks SET content = 'replacementstoryneedle' WHERE rowid = ?1",
        [rowid],
    )
    .unwrap();
    conn.execute(
        "UPDATE activity_provenance SET digest = ?1, hidden = 1 WHERE rowid = ?2",
        params![vec![0x22_u8; 32], rowid],
    )
    .unwrap();
    drop(conn);
    release_activity_read_seam(id);
    let during = worker.join().unwrap().unwrap();
    let rendered = during.to_string();
    assert!(!rendered.contains("earlierstoryneedle"), "{rendered}");
    assert!(!rendered.contains("replacementstoryneedle"), "{rendered}");
    assert!(!rendered.contains("seamlinktoken"), "{rendered}");
    assert!(during["results"].as_array().unwrap().is_empty());
    assert_eq!(during["coverage"]["live_examination_complete"], false);
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
    assert!(!lock.exists());
    assert_eq!(fs::read(&source).unwrap(), before);
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    let stored: String = conn
        .query_row(
            "SELECT content FROM chunks WHERE rowid = ?1",
            [rowid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, "replacementstoryneedle");
}

fn grant_chosen_facet(journal: &Path, facet_id: &str) {
    PermissionStore::open(journal)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "transcripts".to_owned(),
                    "entities".to_owned(),
                    "facets".to_owned(),
                ],
                scope: ReadScope::Facets {
                    ids: vec![facet_id.to_owned()],
                },
            },
        )
        .unwrap();
}

fn restore_mtime(path: &Path, mtime: std::time::SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[test]
fn shaped_activity_follows_the_indexed_family_under_a_chosen_facet_grant() {
    let journal = tempfile::Builder::new()
        .prefix("shaped-activity-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let shaped = "facets/alpha/events/20261010.jsonl";
    let alias = "facets/alpha/events/20261011.jsonl";
    let named = "facets/alpha/activities/20261010/notes/activity.md";
    let beta = "facets/beta/events/20261010.jsonl";
    write_activity(
        journal.path(),
        "facets/alpha/events/shape.json",
        r#"{"20261010.jsonl":"Activity","20261011.jsonl":"Event"}"#,
    );
    write_activity(
        journal.path(),
        shaped,
        &format!(
            "{}\n",
            json!({
                "id": "shaped",
                "title": "Shaped",
                "hidden": false,
                "story": {"body": "shapedactivityneedle"}
            })
        ),
    );
    write_activity(
        journal.path(),
        alias,
        &format!(
            "{}\n",
            json!({"title": "Alias", "summary": "aliascontrolneedle"})
        ),
    );
    write_activity(journal.path(), named, "# Activity\n\nagentactivityneedle\n");
    write_activity(
        journal.path(),
        "facets/beta/events/shape.json",
        r#"{"20261010.jsonl":"Activity"}"#,
    );
    write_activity(
        journal.path(),
        beta,
        &format!(
            "{}\n",
            json!({
                "id": "beta",
                "title": "Beta",
                "hidden": false,
                "story": {"body": "betashapedneedle"}
            })
        ),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_chosen_facet(journal.path(), FACET_A);

    let hits = activity_hits_for(&journal, "shapedactivityneedle");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0]["hidden"], false);
    let reference = hits[0]["reference"].as_str().unwrap().to_owned();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    assert_eq!(fetched["hidden"], false);
    let shaped_obj: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(shaped_obj["id"], "shaped");
    assert!(
        shaped_obj["story"]["body"]
            .as_str()
            .unwrap()
            .contains("shapedactivityneedle")
    );
    assert!(
        !fetched["text"]
            .as_str()
            .unwrap()
            .contains("betashapedneedle")
    );

    let alias_hits = activity_hits_for(&journal, "aliascontrolneedle");
    assert_eq!(alias_hits.len(), 1, "{alias_hits:?}");
    assert!(alias_hits[0].get("hidden").is_none());
    let alias_saved = indexed_content(journal.path(), alias, 0);
    let alias_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": alias_hits[0]["reference"]}),
    )
    .unwrap();
    assert!(alias_fetch.get("hidden").is_none());
    assert_eq!(alias_fetch["text"].as_str(), Some(alias_saved.as_str()));

    let named_hits = activity_hits_for(&journal, "agentactivityneedle");
    assert_eq!(named_hits.len(), 1, "{named_hits:?}");
    assert!(named_hits[0].get("hidden").is_none());
    let named_fetch = probe(
        &journal,
        "fetch",
        json!({"reference": named_hits[0]["reference"]}),
    )
    .unwrap();
    assert!(named_fetch.get("hidden").is_none());
    assert!(activity_hits_for(&journal, "betashapedneedle").is_empty());

    let shaped_path = journal.path().join(shaped);
    let shaped_mtime = fs::metadata(&shaped_path).unwrap().modified().unwrap();
    write_activity(
        journal.path(),
        shaped,
        &format!(
            "{}\n",
            json!({
                "id": "shaped",
                "title": "Shaped",
                "hidden": false,
                "story": {"body": "shapedactivitychanged"}
            })
        ),
    );
    restore_mtime(&shaped_path, shaped_mtime);
    assert!(activity_hits_for(&journal, "shapedactivityneedle").is_empty());
    assert!(activity_hits_for(&journal, "shapedactivitychanged").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
    assert_eq!(activity_hits_for(&journal, "aliascontrolneedle").len(), 1);

    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let refreshed = activity_hits_for(&journal, "shapedactivitychanged");
    assert_eq!(refreshed.len(), 1, "{refreshed:?}");
    assert_eq!(refreshed[0]["hidden"], false);
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": refreshed[0]["reference"]}),
    )
    .unwrap();
    assert_eq!(fetched["hidden"], false);
    let shaped_obj: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(shaped_obj["id"], "shaped");
    assert_eq!(shaped_obj["story"]["body"], "shapedactivitychanged");

    let rescanned_mtime = fs::metadata(&shaped_path).unwrap().modified().unwrap();
    write_activity(
        journal.path(),
        shaped,
        &format!(
            "{}\n",
            json!({
                "id": "shaped",
                "title": "Shaped",
                "hidden": false,
                "story": {"body": "shapedactivityrescanned"}
            })
        ),
    );
    restore_mtime(&shaped_path, rescanned_mtime);
    assert!(activity_hits_for(&journal, "shapedactivityrescanned").is_empty());
    solstone_core_indexer_store::scan::rescan_file(journal.path(), Path::new(shaped)).unwrap();
    let rescanned = activity_hits_for(&journal, "shapedactivityrescanned");
    assert_eq!(rescanned.len(), 1, "{rescanned:?}");
    assert_eq!(rescanned[0]["hidden"], false);

    let alias_path = journal.path().join(alias);
    let alias_before = indexed_content(journal.path(), alias, 0);
    let alias_mtime = fs::metadata(&alias_path).unwrap().modified().unwrap();
    write_activity(
        journal.path(),
        alias,
        &format!(
            "{}\n",
            json!({"title": "Alias", "summary": "aliaschangedneedle"})
        ),
    );
    restore_mtime(&alias_path, alias_mtime);
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    assert_eq!(indexed_content(journal.path(), alias, 0), alias_before);
    assert!(activity_hits_for(&journal, "aliaschangedneedle").is_empty());
    let alias_after = activity_hits_for(&journal, "aliascontrolneedle");
    assert_eq!(alias_after.len(), 1);
    assert!(alias_after[0].get("hidden").is_none());

    write_activity(
        journal.path(),
        "facets/alpha/events/shape.json",
        r#"{"20261010.jsonl":"Event","20261011.jsonl":"Activity"}"#,
    );
    let still_activity = activity_hits_for(&journal, "shapedactivityrescanned");
    assert_eq!(still_activity.len(), 1, "{still_activity:?}");
    assert_eq!(still_activity[0]["hidden"], false);
    let still_alias = activity_hits_for(&journal, "aliascontrolneedle");
    assert_eq!(still_alias.len(), 1);
    assert!(still_alias[0].get("hidden").is_none());

    let index_path = solstone_core_indexer_store::db::db_path(journal.path());
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    conn.execute(
        "UPDATE activity_provenance SET digest = ?1 WHERE rowid = (SELECT rowid FROM chunks WHERE path = ?2 AND idx = 0)",
        params!["text-digest-is-not-a-blob", shaped],
    )
    .unwrap();
    drop(conn);
    let broken = probe(
        &journal,
        "search",
        json!({"query": "shapedactivityrescanned"}),
    )
    .unwrap();
    assert!(broken["results"].as_array().unwrap().is_empty(), "{broken}");
    assert_eq!(broken["coverage"]["live_examination_complete"], false);
}

#[test]
fn malformed_activity_source_withholds_the_whole_file_after_indexing() {
    let journal = tempfile::Builder::new()
        .prefix("malformed-activity-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let rel = "facets/alpha/activities/20261010.jsonl";
    let body = "{\"id\":\"kept\",\"title\":\"Kept\",\"story\":{\"body\":\"validrowneedle\"}}\n\n{\"id\":\"torn\",\"title\":\"Torn\"\n";
    write_activity(journal.path(), rel, body);
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_whole_journal(journal.path());

    let saved = indexed_content(journal.path(), rel, 0);
    assert!(saved.contains("validrowneedle"), "{saved}");
    let index = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    let chunks: i64 = index
        .query_row(
            "SELECT count(*) FROM chunks WHERE path = ?1",
            [rel],
            |row| row.get(0),
        )
        .unwrap();
    let proofs: i64 = index
        .query_row(
            "SELECT count(*) FROM activity_provenance AS p \
             JOIN chunks AS c ON c.rowid = p.rowid WHERE c.path = ?1",
            [rel],
            |row| row.get(0),
        )
        .unwrap();
    let row_id: i64 = index
        .query_row(
            "SELECT rowid FROM chunks WHERE path = ?1 AND idx = 0",
            [rel],
            |row| row.get(0),
        )
        .unwrap();
    let digest: Vec<u8> = index
        .query_row(
            "SELECT digest FROM activity_provenance WHERE rowid = ?1",
            [row_id],
            |row| row.get(0),
        )
        .unwrap();
    drop(index);
    assert_eq!(chunks, 1);
    assert_eq!(proofs, 1);

    let search = probe(&journal, "search", json!({"query": "validrowneedle"})).unwrap();
    assert!(search["results"].as_array().unwrap().is_empty(), "{search}");
    assert_eq!(search["coverage"]["live_examination_complete"], false);

    let decision = evaluate_connection_read(journal.path(), CONNECTION);
    let PermissionDecision::Snapshot(snapshot) = decision else {
        panic!("grant snapshot, got {decision:?}");
    };
    let proof = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let reference = crate::dispatch::codec()
        .mint(
            CONNECTION,
            snapshot.generation,
            crate::references::ReferenceTarget::Entry(crate::references::EntryReference {
                day: "20261010".to_string(),
                stream: String::new(),
                path: rel.to_string(),
                idx: 0,
                row_id,
                activity_proof: Some(proof),
                record_identity: None,
                markdown_fingerprint: None,
                source_digest: None,
                byte_offset: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
}

fn membership_journal() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("activity-membership-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    write_activity(
        journal.path(),
        "facets/alpha/events/shape.json",
        r#"{"20261010.jsonl":"Activity"}"#,
    );
    write_activity(
        journal.path(),
        "facets/alpha/events/20261010.jsonl",
        &format!(
            "{}\n",
            json!({"id":"one", "hidden":false, "story":{"body":"membershipneedle"}})
        ),
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_whole_journal(journal.path());
    journal
}

#[test]
fn activity_membership_legacy_unknown_is_withheld_and_writer_recovers() {
    let journal = membership_journal();
    let hits = activity_hits_for(&journal, "membershipneedle");
    assert_eq!(hits.len(), 1);
    let reference = hits[0]["reference"].clone();
    let conn = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    conn.execute("DELETE FROM activity_provenance", []).unwrap();
    if conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='activity_membership'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 1
    {
        conn.execute("DELETE FROM activity_membership", []).unwrap();
    }
    drop(conn);
    assert!(activity_hits_for(&journal, "membershipneedle").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference":reference})),
        Err(McpProbeError::Unavailable)
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    assert_eq!(
        activity_hits_for(&journal, "membershipneedle")[0]["hidden"],
        false
    );
    let conn = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    conn.execute("DROP TABLE activity_provenance", []).unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "membershipneedle").is_empty());
}

#[test]
fn activity_membership_bound_reference_rejects_a_non_activity_replacement() {
    let journal = membership_journal();
    let hits = activity_hits_for(&journal, "membershipneedle");
    assert_eq!(hits.len(), 1);
    let reference = hits[0]["reference"].clone();
    let conn = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    conn.execute("DELETE FROM activity_provenance", []).unwrap();
    if conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='activity_membership'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 1
    {
        conn.execute("UPDATE activity_membership SET is_activity=0", [])
            .unwrap();
    }
    conn.execute(
        "UPDATE chunks SET agent='event', content='replacement event' WHERE agent='activity'",
        [],
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        probe(&journal, "fetch", json!({"reference":reference})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn activity_membership_index_reset_cannot_release_an_earlier_candidate() {
    use crate::activity_admit::{
        arm_activity_read_seam, release_activity_read_seam, wait_until_activity_read_seamed,
    };
    let journal = membership_journal();
    let root = journal.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker_root = root.clone();
    let worker = std::thread::spawn(move || {
        let id = std::thread::current().id();
        let _guard = arm_activity_read_seam(id);
        tx.send(id).unwrap();
        crate::dispatch::run_mcp_probe(
            &worker_root,
            CONNECTION,
            "search",
            &json!({"query":"membershipneedle"}),
        )
    });
    let id = rx.recv().unwrap();
    wait_until_activity_read_seamed(id);
    let conn = solstone_core_indexer_store::db::open_index(&root).unwrap();
    conn.execute("DELETE FROM chunks", []).unwrap();
    conn.execute("DELETE FROM activity_provenance", []).unwrap();
    drop(conn);
    release_activity_read_seam(id);
    let response = worker.join().unwrap().unwrap();
    assert!(
        response["results"].as_array().unwrap().is_empty(),
        "{response}"
    );
}

#[test]
fn activity_membership_malformed_digest_does_not_block_normal_refresh() {
    let journal = membership_journal();
    let conn = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    conn.execute(
        "UPDATE activity_provenance SET digest='bad text digest'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "membershipneedle").is_empty());
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let hits = activity_hits_for(&journal, "membershipneedle");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["hidden"], false);
}

#[test]
fn activity_membership_ordinary_named_note_refreshes_without_adding_hidden() {
    let journal = membership_journal();
    let rel = "facets/alpha/activities/20261010/notes/activity.md";
    write_activity(
        journal.path(),
        rel,
        "# Note\n\nordinaryactivitynamedneedle\n",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let hits = activity_hits_for(&journal, "ordinaryactivitynamedneedle");
    assert_eq!(hits.len(), 1);
    assert!(hits[0].get("hidden").is_none());
    let reference = hits[0]["reference"].clone();
    let conn = solstone_core_indexer_store::db::open_index(journal.path()).unwrap();
    conn.execute(
        "DELETE FROM activity_membership WHERE rowid IN (SELECT rowid FROM chunks WHERE path=?1)",
        [rel],
    )
    .unwrap();
    drop(conn);
    assert!(activity_hits_for(&journal, "ordinaryactivitynamedneedle").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference":reference})),
        Err(McpProbeError::Unavailable)
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let hits = activity_hits_for(&journal, "ordinaryactivitynamedneedle");
    assert_eq!(hits.len(), 1);
    assert!(hits[0].get("hidden").is_none());
    let fetched = probe(&journal, "fetch", json!({"reference":hits[0]["reference"]})).unwrap();
    assert!(fetched.get("hidden").is_none());
    assert!(
        fetched["text"]
            .as_str()
            .unwrap()
            .contains("ordinaryactivitynamedneedle")
    );
}

fn assert_no_leaked_keys(value: &Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                assert!(
                    k != "path" && k != "idx" && k != "row_id" && k != "rowid" && k != "stream",
                    "Leaked forbidden key '{k}' in value: {value:?}"
                );
                assert_no_leaked_keys(v);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                assert_no_leaked_keys(v);
            }
        }
        _ => {}
    }
}

fn base_activity_journal() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("activity-boundary-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let config_dir = journal.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("journal.json"),
        json!({"identity":{"timezone":"America/New_York"}}).to_string(),
    )
    .unwrap();
    for (name, id, title) in [("alpha", FACET_A, "Alpha"), ("beta", FACET_B, "Beta")] {
        let directory = journal.path().join("facets").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("facet.json"),
            json!({"id": id, "title": title, "description": format!("{title} details")})
                .to_string(),
        )
        .unwrap();
    }
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    journal
}

#[allow(clippy::too_many_arguments)]
fn write_activity_line(
    journal: &Path,
    facet_dir: &str,
    day: &str,
    id: &str,
    title: &str,
    created_at: i64,
    activity: Option<&str>,
    segments: Option<&[&str]>,
    active_entities: Option<&[&str]>,
    participation: Option<&[(&str, &str)]>,
    stream: Option<&str>,
    hidden: Option<bool>,
    story_body: Option<&str>,
) {
    let act_dir = journal.join("facets").join(facet_dir).join("activities");
    fs::create_dir_all(&act_dir).unwrap();
    let mut obj = json!({
        "id": id,
        "title": title,
        "created_at": created_at,
    });
    if let Some(act) = activity {
        obj["activity"] = json!(act);
    }
    if let Some(segs) = segments {
        obj["segments"] = json!(segs);
    }
    if let Some(ents) = active_entities {
        obj["active_entities"] = json!(ents);
    }
    if let Some(parts) = participation {
        let parts_json: Vec<Value> = parts
            .iter()
            .map(|(name, role)| json!({"name": name, "role": role}))
            .collect();
        obj["participation"] = json!(parts_json);
    }
    if let Some(st) = stream {
        obj["stream"] = json!(st);
    }
    if let Some(h) = hidden {
        obj["hidden"] = json!(h);
    }
    if let Some(story) = story_body {
        obj["story"] = json!({"body": story});
    }
    let mut line = obj.to_string();
    line.push('\n');
    let file_path = act_dir.join(format!("{day}.jsonl"));
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
        .unwrap();
    f.write_all(line.as_bytes()).unwrap();
}

fn write_segment_transcript(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    facet: &str,
    transcript: &str,
) {
    let seg_dir = journal
        .join("chronicle")
        .join(day)
        .join(stream)
        .join(segment);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": facet}]).to_string(),
    )
    .unwrap();
    fs::write(seg_dir.join("meeting_transcript.md"), transcript).unwrap();
    fs::write(seg_dir.join("talents/transcript.md"), transcript).unwrap();
}

#[test]
fn list_activities_range_order_and_pagination() {
    let journal = base_activity_journal();
    let oz = solstone_core_journal_config::owner_zone(journal.path());
    assert_eq!(oz.iana_name(), Some("America/New_York"));
    let expected_zone = solstone_core_journal_config::zone_label(Some(&oz), 0);

    write_activity_line(
        journal.path(),
        "alpha",
        "20260307",
        "act_before",
        "Before",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_a2",
        "Alpha 2",
        2,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_a1",
        "Alpha 1",
        1,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "beta",
        "20260308",
        "act_b1",
        "Beta 1",
        1,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260316",
        "act_after",
        "After",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );

    let res = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260308",
            "to": "20260314",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res);
    let acts = res["activities"].as_array().unwrap();
    assert_eq!(acts.len(), 3);
    for act in acts {
        assert_eq!(act["zone"], expected_zone);
    }
    assert_eq!(acts[0]["title"], "Alpha 1");
    assert_eq!(acts[1]["title"], "Alpha 2");
    assert_eq!(acts[2]["title"], "Beta 1");

    let p1 = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260308",
            "to": "20260314",
            "limit": 1,
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&p1);
    let p1_acts = p1["activities"].as_array().unwrap();
    assert_eq!(p1_acts.len(), 1);
    assert_eq!(p1_acts[0]["title"], "Alpha 1");
    let c1 = p1["next_cursor"].as_str().unwrap();

    let p2 = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260308",
            "to": "20260314",
            "limit": 1,
            "cursor": c1,
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&p2);
    let p2_acts = p2["activities"].as_array().unwrap();
    assert_eq!(p2_acts.len(), 1);
    assert_eq!(p2_acts[0]["title"], "Alpha 2");
    let c2 = p2["next_cursor"].as_str().unwrap();

    let p3 = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260308",
            "to": "20260314",
            "limit": 1,
            "cursor": c2,
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&p3);
    let p3_acts = p3["activities"].as_array().unwrap();
    assert_eq!(p3_acts.len(), 1);
    assert_eq!(p3_acts[0]["title"], "Beta 1");
    assert!(p3.get("next_cursor").is_none());

    assert_eq!(
        probe(
            &journal,
            "list_activities",
            json!({
                "from": "20260301",
                "to": "20260314",
                "cursor": c1,
            })
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn list_activities_empty_range_and_hidden() {
    let journal = base_activity_journal();
    let res = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260322",
            "to": "20260328",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res);
    assert!(res["activities"].as_array().unwrap().is_empty());

    write_activity_line(
        journal.path(),
        "alpha",
        "20260323",
        "act_hidden",
        "Hidden Activity Title",
        100,
        None,
        None,
        None,
        None,
        None,
        Some(true),
        None,
    );
    let res2 = probe(
        &journal,
        "list_activities",
        json!({
            "from": "20260322",
            "to": "20260328",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res2);
    assert!(res2["activities"].as_array().unwrap().is_empty());
    assert!(!res2.to_string().contains("Hidden Activity Title"));
}

#[test]
fn list_activities_fields_and_fetch() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_nodur",
        "No Duration",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_dur",
        "With Duration",
        200,
        None,
        Some(&["090000_300"]),
        Some(&["Entity One"]),
        Some(&[
            ("Participant Alpha", "attendee"),
            ("Mentioned Beta", "mentioned"),
            ("Shown Gamma", "visible"),
        ]),
        None,
        None,
        None,
    );

    let res = probe(&journal, "list_activities", json!({"day": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    let acts = res["activities"].as_array().unwrap();
    assert_eq!(acts.len(), 2);

    assert!(acts[0]["duration"].is_null());

    assert_eq!(acts[1]["start"], "09:00:00");
    assert_eq!(acts[1]["duration"], 300);

    let parts = acts[1]["participants"].as_array().unwrap();
    let ents = acts[1]["entities"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0], "Participant Alpha");
    // A person only mentioned is not a participant.
    assert_eq!(acts[1]["mentioned"], json!(["Mentioned Beta"]));
    // A name only on screen was neither said nor attended.
    assert_eq!(acts[1]["on_screen"], json!(["Shown Gamma"]));
    assert_eq!(ents.len(), 1);
    assert_eq!(ents[0], "Entity One");
    assert_ne!(parts, ents);

    assert_eq!(acts[1]["facet"], "Alpha");

    let fetch_res = probe(
        &journal,
        "fetch",
        json!({"reference": acts[1]["reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&fetch_res);
    let parsed: Value = serde_json::from_str(fetch_res["text"].as_str().unwrap()).unwrap();
    assert!(parsed.is_object());
    assert_eq!(parsed["id"], "act_dur");
    assert_no_leaked_keys(&parsed);
}

#[test]
fn list_activities_unindexed_returns_and_fetches() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_unindexed",
        "Unindexed Title",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let res = probe(&journal, "list_activities", json!({"day": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    let acts = res["activities"].as_array().unwrap();
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0]["title"], "Unindexed Title");

    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": acts[0]["reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&fetched);
    let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["id"], "act_unindexed");
    assert_no_leaked_keys(&parsed);
}

#[test]
fn list_activities_stale_index_fresh_file_behavior() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_stale",
        "Original Title needlefirst",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let search_res = probe(&journal, "search", json!({"query": "needlefirst"})).unwrap();
    assert_no_leaked_keys(&search_res);
    let search_ref = search_res["results"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();

    let file_path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::write(
        &file_path,
        json!({"id": "act_stale", "title": "Updated Title needlenew", "created_at": 100})
            .to_string()
            + "\n",
    )
    .unwrap();

    let list_res = probe(&journal, "list_activities", json!({"day": "20260308"})).unwrap();
    assert_no_leaked_keys(&list_res);
    assert_eq!(
        list_res["activities"][0]["title"],
        "Updated Title needlenew"
    );
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": list_res["activities"][0]["reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&fetched);
    let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["title"], "Updated Title needlenew");

    assert_eq!(
        probe(&journal, "fetch", json!({"reference": search_ref})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn list_activities_malformed_jsonl_fails() {
    let journal = base_activity_journal();
    let file_path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(
        &file_path,
        "{\"id\": \"valid1\", \"title\": \"Valid\"}\nnot-json\n",
    )
    .unwrap();

    assert_eq!(
        probe(&journal, "list_activities", json!({"day": "20260308"})),
        Err(McpProbeError::Unavailable)
    );

    let journal2 = base_activity_journal();
    let file_path2 = journal2
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::create_dir_all(file_path2.parent().unwrap()).unwrap();
    fs::write(&file_path2, "{\"id\": \"valid1\", \"title\": \"Valid\"}\n").unwrap();
    let res = probe(&journal2, "list_activities", json!({"day": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    assert_eq!(res["activities"][0]["title"], "Valid");
}

#[test]
fn list_activities_entity_filtering_and_tokens() {
    let journal = base_activity_journal();
    for (dir, id, name, aka) in [
        ("ada", "entity-ada", "Ada Lovelace", vec!["secret alias"]),
        ("grace", "entity-grace", "Grace Hopper", vec![]),
    ] {
        let e_dir = journal.path().join("entities").join(dir);
        fs::create_dir_all(&e_dir).unwrap();
        fs::write(
            e_dir.join("entity.json"),
            json!({
                "id": id,
                "name": name,
                "aka": aka,
            })
            .to_string(),
        )
        .unwrap();

        let l_dir = journal.path().join("facets/alpha/entities").join(dir);
        fs::create_dir_all(&l_dir).unwrap();
        fs::write(
            l_dir.join("entity.json"),
            json!({"entity_id": id, "detached": false}).to_string(),
        )
        .unwrap();
    }

    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_ada",
        "Ada Activity",
        100,
        None,
        None,
        Some(&["entity-ada"]),
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_grace",
        "Grace Activity",
        200,
        None,
        None,
        Some(&["Grace Hopper"]),
        None,
        None,
        None,
        None,
    );

    let ent_list = probe(&journal, "list_entities", json!({})).unwrap();
    assert_no_leaked_keys(&ent_list);
    let ada_ent = ent_list["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "Ada Lovelace")
        .unwrap();
    let ada_ref = ada_ent["reference"].as_str().unwrap();

    let res_ada = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": ada_ref,
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_ada);
    let acts_ada = res_ada["activities"].as_array().unwrap();
    assert_eq!(acts_ada.len(), 1);
    assert_eq!(acts_ada[0]["title"], "Ada Activity");

    let res_grace = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "grace hopper",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_grace);
    let acts_grace = res_grace["activities"].as_array().unwrap();
    assert_eq!(acts_grace.len(), 1);
    assert_eq!(acts_grace[0]["title"], "Grace Activity");

    let res_alias = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "secret alias",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_alias);
    assert!(res_alias["activities"].as_array().unwrap().is_empty());
    assert!(!res_alias.to_string().contains("secret alias"));

    let res_sub = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "ada",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_sub);
    let acts_sub = res_sub["activities"].as_array().unwrap();
    assert_eq!(acts_sub.len(), 1);
    assert_eq!(acts_sub[0]["title"], "Ada Activity");

    let res_grace_part = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "grace",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_grace_part);
    let acts_grace_part = res_grace_part["activities"].as_array().unwrap();
    assert_eq!(acts_grace_part.len(), 1);
    assert_eq!(acts_grace_part[0]["title"], "Grace Activity");

    // A record that names Ada by her id, as an older merge wrote it, is
    // found by her name.
    let res_ada_name = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "ada lovelace",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_ada_name);
    let acts_ada_name = res_ada_name["activities"].as_array().unwrap();
    assert_eq!(acts_ada_name.len(), 1);
    assert_eq!(acts_ada_name[0]["title"], "Ada Activity");

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let res_grace_no_ent = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "Grace Hopper",
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&res_grace_no_ent);
    assert_eq!(res_grace_no_ent["activities"].as_array().unwrap().len(), 1);

    // Without entities, a name is only the words a record holds.
    let res_ada_no_ent = probe(
        &journal,
        "list_activities",
        json!({
            "day": "20260308",
            "entity": "Ada Lovelace",
        }),
    )
    .unwrap();
    assert!(res_ada_no_ent["activities"].as_array().unwrap().is_empty());

    assert_eq!(
        probe(
            &journal,
            "list_activities",
            json!({
                "day": "20260308",
                "entity": ada_ref,
            })
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn list_activities_denied_facet_scoped_and_forged_references() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "beta",
        "20260308",
        "act_beta_only",
        "Beta Activity Title",
        100,
        None,
        None,
        Some(&["Beta Entity"]),
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_alpha",
        "Alpha Activity Title",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "facets".to_owned(),
                    "entities".to_owned(),
                    "transcripts".to_owned(),
                ],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let res = probe(&journal, "list_activities", json!({"day": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    let s = res.to_string();
    assert!(!s.contains("Beta Activity Title"));
    assert!(!s.contains("Beta Entity"));

    let res_fb = probe(
        &journal,
        "list_activities",
        json!({"day": "20260308", "facet": "beta"}),
    )
    .unwrap();
    assert_no_leaked_keys(&res_fb);
    assert!(res_fb["activities"].as_array().unwrap().is_empty());

    let res_fid = probe(
        &journal,
        "list_activities",
        json!({"day": "20260308", "facet": FACET_B}),
    )
    .unwrap();
    assert_no_leaked_keys(&res_fid);
    assert!(res_fid["activities"].as_array().unwrap().is_empty());

    assert_eq!(
        probe(&journal, "fetch", json!({"reference": "aaaa"})),
        Err(McpProbeError::Unavailable)
    );

    let codec = crate::dispatch::codec();
    let perm = PermissionStore::open(journal.path())
        .get_permission(CONNECTION)
        .unwrap()
        .unwrap();
    let bad_dir = codec
        .mint(
            CONNECTION,
            perm.generation,
            crate::references::ReferenceTarget::Activity(crate::references::ActivityReference {
                facet_id: FACET_A.to_string(),
                facet_dir: "..".to_string(),
                day: "20260308".to_string(),
                record_identity: "rec".to_string(),
                file_digest: "digest".to_string(),
                unit_index: None,
                source_binding: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": bad_dir})),
        Err(McpProbeError::Unavailable)
    );

    let bad_gen = codec
        .mint(
            CONNECTION,
            perm.generation + 1,
            crate::references::ReferenceTarget::Activity(crate::references::ActivityReference {
                facet_id: FACET_A.to_string(),
                facet_dir: "alpha".to_string(),
                day: "20260308".to_string(),
                record_identity: "rec".to_string(),
                file_digest: "digest".to_string(),
                unit_index: None,
                source_binding: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": bad_gen})),
        Err(McpProbeError::Unavailable)
    );

    let diff_conn = codec
        .mint(
            "bearer:other",
            perm.generation,
            crate::references::ReferenceTarget::Activity(crate::references::ActivityReference {
                facet_id: FACET_A.to_string(),
                facet_dir: "alpha".to_string(),
                day: "20260308".to_string(),
                record_identity: "rec".to_string(),
                file_digest: "digest".to_string(),
                unit_index: None,
                source_binding: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": diff_conn})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn list_activities_without_facets_category_is_denied() {
    let journal = base_activity_journal();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "list_activities", json!({"day": "20260308"})),
        Err(McpProbeError::PermissionDenied)
    );
}

fn advertised_tool_names(journal: &tempfile::TempDir) -> Vec<String> {
    let decision = evaluate_connection_read(journal.path(), CONNECTION);
    crate::registry::advertised_tools_list(journal.path(), &decision)["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

fn refused_fetch_reasons(journal: &tempfile::TempDir) -> Vec<String> {
    crate::activity::read_activity(
        journal.path(),
        &crate::activity::ActivityQuery {
            limit: 20,
            outcome: Some(crate::activity::RecordedOutcome::Refused),
            ..crate::activity::ActivityQuery::default()
        },
    )
    .unwrap()
    .entries
    .into_iter()
    .filter(|entry| entry.tool_name == solstone_core_mcp_audit::ToolName::Fetch)
    .filter_map(|entry| entry.reason)
    .collect()
}

#[test]
fn facets_only_listing_fetches_the_activity_and_refuses_an_entry() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_facets",
        "Facets Only Activity",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let names = advertised_tool_names(&journal);
    assert!(names.iter().any(|name| name == "fetch"));
    assert!(names.iter().any(|name| name == "list_activities"));
    assert!(names.iter().any(|name| name == "search"));
    assert!(!names.iter().any(|name| name == "get_activity_sources"));
    assert!(!names.iter().any(|name| name == "get_transcript"));

    let listed = probe(&journal, "list_activities", json!({"day": "20260308"})).unwrap();
    let reference = listed["activities"][0]["reference"].as_str().unwrap();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    assert_no_leaked_keys(&fetched);
    assert!(fetched["text"].as_str().unwrap().contains("act_facets"));
    assert!(probe(&journal, "search", json!({"query": "Facets"})).is_ok());
    assert_eq!(
        probe(
            &journal,
            "get_activity_sources",
            json!({"reference": reference})
        ),
        Err(McpProbeError::PermissionDenied)
    );

    let PermissionDecision::Snapshot(snapshot) =
        evaluate_connection_read(journal.path(), CONNECTION)
    else {
        panic!("facets grant");
    };
    let entry = crate::dispatch::codec()
        .mint(
            CONNECTION,
            snapshot.generation,
            crate::references::ReferenceTarget::Entry(crate::references::EntryReference {
                day: "20260308".to_owned(),
                stream: "default".to_owned(),
                path: "chronicle/20260308/default/090000_300/talents/audio.md".to_owned(),
                idx: 0,
                row_id: 1,
                activity_proof: None,
                record_identity: None,
                markdown_fingerprint: None,
                source_digest: None,
                byte_offset: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": entry})),
        Err(McpProbeError::PermissionDenied)
    );
    assert!(
        refused_fetch_reasons(&journal)
            .iter()
            .any(|reason| reason == "this connection does not have permission to read transcripts")
    );
}

#[test]
fn transcripts_entities_and_paired_listing_grants_round_trip() {
    let transcripts = fixture();
    fs::write(
        transcripts
            .path()
            .join(format!("chronicle/{DAY}/{STREAM}/{SEGMENT}/audio.jsonl")),
        "{\"start\":\"00:00:01\",\"text\":\"approved speech\"}\n",
    )
    .unwrap();
    let transcript_names = advertised_tool_names(&transcripts);
    assert!(transcript_names.iter().any(|name| name == "fetch"));
    assert!(
        transcript_names
            .iter()
            .any(|name| name == "list_transcripts")
    );
    assert!(transcript_names.iter().any(|name| name == "search"));
    assert!(
        !transcript_names
            .iter()
            .any(|name| name == "list_activities")
    );
    let listed = probe(&transcripts, "list_transcripts", json!({})).unwrap();
    let transcript_ref = listed["transcripts"][0]["reference"].as_str().unwrap();
    let page = probe(
        &transcripts,
        "get_transcript",
        json!({"reference": transcript_ref}),
    )
    .unwrap();
    assert!(!page["entries"].as_array().unwrap().is_empty());
    let search = probe(&transcripts, "search", json!({"query": "indexed"})).unwrap();
    let entry_ref = search["results"][0]["reference"].as_str().unwrap();
    let fetched = probe(&transcripts, "fetch", json!({"reference": entry_ref})).unwrap();
    assert!(fetched["text"].is_string());
    assert_eq!(
        probe(&transcripts, "list_activities", json!({"day": DAY})),
        Err(McpProbeError::PermissionDenied)
    );
    let PermissionDecision::Snapshot(snapshot) =
        evaluate_connection_read(transcripts.path(), CONNECTION)
    else {
        panic!("transcript grant");
    };
    let activity = crate::dispatch::codec()
        .mint(
            CONNECTION,
            snapshot.generation,
            crate::references::ReferenceTarget::Activity(crate::references::ActivityReference {
                facet_id: FACET_A.to_owned(),
                facet_dir: "alpha".to_owned(),
                day: DAY.to_owned(),
                record_identity: "rec".to_owned(),
                file_digest: "digest".to_owned(),
                unit_index: None,
                source_binding: None,
            }),
        )
        .unwrap();
    assert_eq!(
        probe(&transcripts, "fetch", json!({"reference": activity})),
        Err(McpProbeError::PermissionDenied)
    );
    assert!(
        refused_fetch_reasons(&transcripts)
            .iter()
            .any(|reason| reason == "this connection does not have permission to read summaries")
    );

    let entities = fixture();
    PermissionStore::open(entities.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let entity_names = advertised_tool_names(&entities);
    assert!(entity_names.iter().any(|name| name == "list_entities"));
    assert!(entity_names.iter().any(|name| name == "get_entity"));
    assert!(!entity_names.iter().any(|name| name == "fetch"));
    let listed_entities = probe(&entities, "list_entities", json!({})).unwrap();
    let entity_ref = listed_entities["entities"][0]["reference"]
        .as_str()
        .unwrap();
    let entity = probe(&entities, "get_entity", json!({"reference": entity_ref})).unwrap();
    assert!(entity["name"].is_string());
    assert_eq!(
        probe(&entities, "fetch", json!({"reference": entity_ref})),
        Err(McpProbeError::PermissionDenied)
    );

    let paired = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    write_segment_transcript(
        paired.path(),
        day,
        "default",
        seg,
        "alpha",
        "pairedsourceneedle\n",
    );
    write_activity_line(
        paired.path(),
        "alpha",
        day,
        "act_paired",
        "Paired Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );
    let speech = paired
        .path()
        .join(format!("chronicle/{day}/default/{seg}/audio.jsonl"));
    fs::write(
        speech,
        "{\"start\":\"00:00:01\",\"text\":\"pairedsourceneedle\"}\n",
    )
    .unwrap();
    PermissionStore::open(paired.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let paired_names = advertised_tool_names(&paired);
    assert!(paired_names.iter().any(|name| name == "list_activities"));
    assert!(paired_names.iter().any(|name| name == "fetch"));
    assert!(
        paired_names
            .iter()
            .any(|name| name == "get_activity_sources")
    );
    let listed_paired = probe(&paired, "list_activities", json!({"day": day})).unwrap();
    let paired_ref = listed_paired["activities"][0]["reference"]
        .as_str()
        .unwrap();
    let fetched_paired = probe(&paired, "fetch", json!({"reference": paired_ref})).unwrap();
    assert!(
        fetched_paired["text"]
            .as_str()
            .unwrap()
            .contains("act_paired")
    );
    let sources = probe(
        &paired,
        "get_activity_sources",
        json!({"reference": paired_ref}),
    )
    .unwrap();
    assert!(sources.to_string().contains("pairedsourceneedle"));
}

#[test]
fn get_activity_sources_names_speakers_only_through_the_voice_gate() {
    let journal = base_activity_journal();
    let day = "20260309";
    let seg = "090000_300";
    let dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(dir.join("talents")).unwrap();
    fs::write(
        dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("audio.jsonl"),
        [
            r#"{"raw":"audio.flac"}"#,
            r#"{"start":"00:00:01","speaker":1,"sentence_id":1,"text":"ownerline"}"#,
            r#"{"start":"00:00:02","speaker":2,"sentence_id":2,"text":"acousticline"}"#,
            r#"{"start":"00:00:03","speaker":3,"sentence_id":3,"text":"guessedline"}"#,
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    fs::write(
        dir.join("talents/speaker_labels.json"),
        json!({"labels": [
            {"sentence_id": 1, "speaker": "owner-e", "method": "user_identified"},
            {"sentence_id": 2, "speaker": "mina-e", "method": "acoustic"},
            {"sentence_id": 3, "speaker": "ravi-e", "method": "contextual"}
        ]})
        .to_string(),
    )
    .unwrap();
    for (id, name, principal) in [
        ("owner-e", "Olu", true),
        ("mina-e", "Mina", false),
        ("ravi-e", "Ravi", false),
    ] {
        let entity = journal.path().join("entities").join(id);
        fs::create_dir_all(&entity).unwrap();
        fs::write(
            entity.join("entity.json"),
            json!({"id": id, "name": name, "type": "Person", "is_principal": principal})
                .to_string(),
        )
        .unwrap();
    }
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_voice",
        "Voice Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let reference = listed["activities"][0]["reference"].as_str().unwrap();
    let sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference}),
    )
    .unwrap();
    let text = literal_display_text(sources["units"][0]["text"].as_str().unwrap());
    // The owner's own line says so; a person is named by voice; a guess is not a name.
    assert!(text.contains("Olu (owner): ownerline"), "{text}");
    assert!(text.contains("Mina: acousticline"), "{text}");
    assert!(text.contains("Speaker 3: guessedline"), "{text}");
    assert!(!text.contains("Ravi"), "{text}");
}

#[test]
fn get_activity_sources_returns_verbatim_long_unit() {
    let journal = base_activity_journal();
    allow_legacy_markdown(&journal);
    let day = "20260308";
    let seg = "090000_300";
    let long_unit = format!("startlongneedle {} endlongneedle\n", "word ".repeat(1000));
    assert!(long_unit.len() > 3840);
    write_segment_transcript(journal.path(), day, "default", seg, "alpha", &long_unit);

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_src",
        "Activity With Sources",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    assert_no_leaked_keys(&list_res);
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let sources_res = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&sources_res);
    let units = sources_res["units"].as_array().unwrap();
    assert_eq!(units.len(), 1);
    let text = literal_display_text(units[0]["text"].as_str().unwrap());
    assert!(text.contains("startlongneedle"));
    assert!(text.contains("endlongneedle"));
    assert!(text.len() > 3840);

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert_eq!(
        probe(
            &journal,
            "get_activity_sources",
            json!({"reference": act_ref})
        ),
        Err(McpProbeError::PermissionDenied)
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert_eq!(
        probe(
            &journal,
            "get_activity_sources",
            json!({"reference": act_ref})
        ),
        Err(McpProbeError::PermissionDenied)
    );
}

#[test]
fn get_activity_sources_pagination_over_max_bytes() {
    let journal = base_activity_journal();
    allow_legacy_markdown(&journal);
    let day = "20260308";
    let seg = "090000_300";
    let mut large_transcript = String::new();
    large_transcript.push_str("firstpageblockneedle paragraph 0\n\n");
    for i in 1..80 {
        large_transcript.push_str(&format!("paragraph {i} text {}\n\n", "data ".repeat(200)));
    }
    large_transcript.push_str("finalpageneedle paragraph final\n\n");
    assert!(large_transcript.len() > solstone_core_transcripts::MAX_TRANSCRIPT_PAGE_BYTES);

    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg,
        "alpha",
        &large_transcript,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_page",
        "Large Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let p1 = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&p1);
    assert!(p1.get("next_cursor").is_some());
    let p1_str = p1.to_string();
    assert!(p1_str.contains("firstpageblockneedle"));
    assert!(!p1_str.contains("finalpageneedle"));
    let next_c = p1["next_cursor"].as_str().unwrap();

    let p2 = probe(
        &journal,
        "get_activity_sources",
        json!({
            "reference": act_ref,
            "cursor": next_c,
        }),
    )
    .unwrap();
    assert_no_leaked_keys(&p2);
    let p2_str = p2.to_string();
    assert!(p2_str.contains("finalpageneedle"));

    let seg_short = "100000_100";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg_short,
        "alpha",
        "short transcript line\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_short",
        "Short Activity",
        200,
        None,
        Some(&[seg_short]),
        None,
        None,
        Some("default"),
        None,
        None,
    );
    let list_res2 = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let short_act = list_res2["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Short Activity")
        .unwrap();
    let short_sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": short_act["reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&short_sources);
    assert!(short_sources.get("next_cursor").is_none());
}

fn write_direct_segment_file(
    journal: &Path,
    day: &str,
    segment: &str,
    facet: &str,
    name: &str,
    body: &str,
) {
    let seg_dir = journal.join("chronicle").join(day).join(segment);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": facet}]).to_string(),
    )
    .unwrap();
    fs::write(seg_dir.join(name), body).unwrap();
}

fn segment_stream(journal: &Path, reference: &str) -> String {
    if crate::names::is_name(reference) {
        return crate::names::resolve_segment(journal, reference)
            .expect("a segment's name")
            .stream;
    }
    let PermissionDecision::Snapshot(snapshot) = evaluate_connection_read(journal, CONNECTION)
    else {
        panic!("connection snapshot");
    };
    match crate::dispatch::codec().resolve(
        reference,
        crate::references::ReferenceKind::Segment,
        CONNECTION,
        snapshot.generation,
    ) {
        Ok(crate::references::ReferenceTarget::Segment(segment)) => segment.stream,
        other => panic!("expected a segment reference, got {other:?}"),
    }
}

fn activity_identity(journal: &Path, reference: &str) -> String {
    if crate::names::is_name(reference) {
        return crate::names::parse_activity(reference)
            .expect("an activity's name")
            .record_identity;
    }
    let PermissionDecision::Snapshot(snapshot) = evaluate_connection_read(journal, CONNECTION)
    else {
        panic!("connection snapshot");
    };
    match crate::dispatch::codec().resolve(
        reference,
        crate::references::ReferenceKind::Activity,
        CONNECTION,
        snapshot.generation,
    ) {
        Ok(crate::references::ReferenceTarget::Activity(activity)) => activity.record_identity,
        other => panic!("expected an activity reference, got {other:?}"),
    }
}

fn write_segment_file(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    facet: &str,
    name: &str,
    body: &str,
) {
    let seg_dir = journal
        .join("chronicle")
        .join(day)
        .join(stream)
        .join(segment);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": facet}]).to_string(),
    )
    .unwrap();
    fs::write(seg_dir.join(name), body).unwrap();
}

#[test]
fn an_activity_can_quote_a_message_while_the_original_is_private() {
    let journal = base_activity_journal();
    let root = journal.path();
    let day = "20260308";
    let stream = "import.chatgpt";
    let segment = "090000_300";
    let quote = "Meet at the north gate";
    write_segment_file(
        root,
        day,
        stream,
        segment,
        "alpha",
        "conversation_transcript.jsonl",
        &format!(
            "{}\n{}\n",
            json!({"untimed":true,"imported":{}}),
            json!({"start":"00:00:00","speaker":"Ana","text":format!("{quote}. Take the spare key.")})
        ),
    );
    write_activity_line(
        root,
        "alpha",
        day,
        "gate",
        quote,
        100,
        Some("import"),
        Some(&[segment]),
        None,
        None,
        Some(stream),
        None,
        None,
    );
    for can_open in [false, true] {
        let mut grant = ReadPermission::default_whole_journal();
        if can_open {
            grant.categories.push("messages".into());
        }
        PermissionStore::open(root)
            .set_permission(CONNECTION, grant)
            .unwrap();
        let listed = probe(&journal, "list_activities", json!({"day":day})).unwrap();
        let activity = listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["title"] == quote)
            .unwrap();
        let source = probe(
            &journal,
            "get_activity_sources",
            json!({"reference":activity["reference"]}),
        )
        .unwrap();
        assert_eq!(
            source["units"]
                .as_array()
                .unwrap()
                .iter()
                .any(|unit| unit["text"]
                    .as_str()
                    .is_some_and(|s| literal_display_text(s).contains("Take the spare key"))),
            can_open,
            "{source}"
        );
    }
}

#[test]
fn get_activity_sources_keeps_family_and_omits_untimed_clocks() {
    let journal = base_activity_journal();
    let mut permission = ReadPermission::default_whole_journal();
    permission.categories.push("messages".into());
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, permission)
        .unwrap();
    let day = "20260308";
    write_segment_file(
        journal.path(),
        day,
        "import.chatgpt",
        "090000_300",
        "alpha",
        "conversation_transcript.jsonl",
        "{\"untimed\":true,\"imported\":{}}\n{\"start\":\"00:00:00\",\"speaker\":\"Ana\",\"text\":\"untimedchatneedle\"}\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "default",
        "100000_300",
        "alpha",
        "audio.jsonl",
        "{\"source\":\"recording\"}\n{\"start\":\"00:00:02\",\"text\":\"spokenaudioneedle\"}\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "import.text",
        "110000_300",
        "alpha",
        "imported.md",
        "markdownneedlenotspeech\n",
    );
    for (id, seg, stream) in [
        ("act_chat", "090000_300", "import.chatgpt"),
        ("act_audio", "100000_300", "default"),
        ("act_md", "110000_300", "import.text"),
    ] {
        write_activity_line(
            journal.path(),
            "alpha",
            day,
            id,
            id,
            100,
            Some("import"),
            Some(&[seg]),
            None,
            None,
            Some(stream),
            None,
            None,
        );
    }

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let reference = |title: &str| {
        listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["title"] == title)
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let chat = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_chat")}),
    )
    .unwrap();
    let chat_unit = &chat["units"][0];
    assert_eq!(chat_unit["kind"], "import");
    assert_eq!(chat_unit["untimed"], true);
    assert!(chat_unit.get("timestamp").is_none());
    let chat_text = literal_display_text(chat_unit["text"].as_str().unwrap());
    assert!(chat_text.contains("Ana: untimedchatneedle"), "{chat_text}");
    assert!(!chat_text.contains("[00:00:00]"));
    assert_eq!(chat_unit["content_kind"], "messages");
    assert!(chat_text.contains("ChatGPT Conversation"));
    assert!(
        !chat_unit["text"]
            .as_str()
            .unwrap()
            .contains("### Transcript")
    );

    let audio = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_audio")}),
    )
    .unwrap();
    let audio_unit = &audio["units"][0];
    assert_eq!(audio_unit["kind"], "audio");
    assert!(audio_unit.get("untimed").is_none());
    assert_eq!(audio_unit["timestamp"], "2026-03-08 10:00:00 - 10:05:00");
    let audio_text = literal_display_text(audio_unit["text"].as_str().unwrap());
    assert!(audio_text.contains("spokenaudioneedle"));
    assert_eq!(audio_unit["content_kind"], "transcripts");
    assert!(
        audio_text.contains("Source channel: Transcript"),
        "{audio_text}"
    );

    let markdown = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_md")}),
    )
    .unwrap();
    let markdown_unit = &markdown["units"][0];
    assert_eq!(markdown_unit["kind"], "import");
    assert!(markdown_unit.get("untimed").is_none());
    assert_eq!(markdown_unit["timestamp"], "2026-03-08 11:00:00 - 11:05:00");
    let markdown_text = literal_display_text(markdown_unit["text"].as_str().unwrap());
    assert!(markdown_text.contains("markdownneedlenotspeech"));
    assert!(markdown_text.contains("Source channel: Imported Conversation"));
    assert_eq!(markdown_unit["conversation_state"], "unresolved");
    assert!(markdown_unit.get("content_kind").is_none());
}

#[test]
fn get_activity_sources_untimed_qualifier_survives_continuation() {
    let journal = base_activity_journal();
    let mut permission = ReadPermission::default_whole_journal();
    permission.categories.push("messages".into());
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, permission)
        .unwrap();
    let day = "20260308";
    let mut chat = String::from("{\"untimed\":true,\"imported\":{}}\n");
    chat.push_str("{\"start\":\"00:00:00\",\"speaker\":\"Ana\",\"text\":\"pagestartuntimed\"}\n");
    for index in 0..400 {
        chat.push_str(&format!(
            "{{\"start\":\"00:00:00\",\"speaker\":\"Ana\",\"text\":\"untimed filler {index} {}\"}}\n",
            "word ".repeat(40)
        ));
    }
    chat.push_str("{\"start\":\"00:00:00\",\"speaker\":\"Ana\",\"text\":\"pageenduntimed\"}\n");
    write_segment_file(
        journal.path(),
        day,
        "import.chatgpt",
        "090000_300",
        "alpha",
        "conversation_transcript.jsonl",
        &chat,
    );

    let mut audio = String::from("{\"source\":\"recording\"}\n");
    audio.push_str("{\"start\":\"00:00:02\",\"text\":\"pagestartaudio\"}\n");
    for index in 0..400 {
        audio.push_str(&format!(
            "{{\"start\":\"00:00:02\",\"text\":\"timed filler {index} {}\"}}\n",
            "word ".repeat(40)
        ));
    }
    audio.push_str("{\"start\":\"00:00:02\",\"text\":\"pageendaudio\"}\n");
    write_segment_file(
        journal.path(),
        day,
        "default",
        "100000_300",
        "alpha",
        "audio.jsonl",
        &audio,
    );
    for (id, segment, stream) in [
        ("act_untimed_pages", "090000_300", "import.chatgpt"),
        ("act_timed_pages", "100000_300", "default"),
    ] {
        write_activity_line(
            journal.path(),
            "alpha",
            day,
            id,
            id,
            100,
            Some("import"),
            Some(&[segment]),
            None,
            None,
            Some(stream),
            None,
            None,
        );
    }
    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let reference = |title: &str| {
        listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["title"] == title)
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let chat_page = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_untimed_pages")}),
    )
    .unwrap();
    assert_untimed_source_page(&chat_page, "pagestartuntimed");
    let chat_cursor = chat_page["next_cursor"].as_str().unwrap();
    let chat_next = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_untimed_pages"), "cursor": chat_cursor}),
    )
    .unwrap();
    assert_untimed_source_page(&chat_next, "pageenduntimed");

    let audio_page = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_timed_pages")}),
    )
    .unwrap();
    assert_timed_speech_page(&audio_page, "pagestartaudio");
    let audio_cursor = audio_page["next_cursor"].as_str().unwrap();
    let audio_next = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference("act_timed_pages"), "cursor": audio_cursor}),
    )
    .unwrap();
    assert_timed_speech_page(&audio_next, "pageendaudio");
}

fn assert_untimed_source_page(page: &serde_json::Value, needle: &str) {
    let units = page["units"].as_array().unwrap();
    assert!(!units.is_empty());
    let mut saw_needle = false;
    for unit in units {
        assert_eq!(unit["untimed"], true);
        assert!(unit.get("timestamp").is_none());
        assert_eq!(unit["kind"], "import");
        let text = literal_display_text(unit["text"].as_str().unwrap());
        assert!(!text.contains("[00:00:00]"));
        saw_needle |= text.contains(needle);
    }
    assert!(saw_needle);
}

fn assert_timed_speech_page(page: &serde_json::Value, needle: &str) {
    let units = page["units"].as_array().unwrap();
    assert!(!units.is_empty());
    let mut saw_needle = false;
    for unit in units {
        assert!(unit.get("untimed").is_none());
        assert_eq!(unit["kind"], "audio");
        assert_eq!(unit["timestamp"], "2026-03-08 10:00:00 - 10:05:00");
        saw_needle |= literal_display_text(unit["text"].as_str().unwrap()).contains(needle);
    }
    assert!(saw_needle);
}

#[test]
fn get_activity_sources_cursor_rejects_a_changed_transcript() {
    let journal = base_activity_journal();
    allow_legacy_markdown(&journal);
    let day = "20260308";
    let seg = "090000_300";
    let mut large_transcript = String::from("firstpageblockneedle paragraph 0\n\n");
    for i in 1..80 {
        large_transcript.push_str(&format!("paragraph {i} text {}\n\n", "data ".repeat(200)));
    }
    large_transcript.push_str("finalpageneedle paragraph final\n\n");
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg,
        "alpha",
        &large_transcript,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_cursor",
        "Cursor Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );
    let activity_path = journal
        .path()
        .join("facets/alpha/activities")
        .join(format!("{day}.jsonl"));
    let activity_before = fs::read(&activity_path).unwrap();

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = listed["activities"][0]["reference"].as_str().unwrap();
    let first = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    let cursor = first["next_cursor"].as_str().unwrap();

    let transcript_path = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg)
        .join("meeting_transcript.md");
    let mut changed = fs::read_to_string(&transcript_path).unwrap();
    changed.insert_str(0, "prependedmutateneedle\n");
    fs::write(&transcript_path, changed).unwrap();
    assert_eq!(fs::read(&activity_path).unwrap(), activity_before);

    assert_eq!(
        probe(
            &journal,
            "get_activity_sources",
            json!({"reference": act_ref, "cursor": cursor})
        ),
        Err(McpProbeError::Unavailable)
    );
    let fresh = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert!(fresh.to_string().contains("prependedmutateneedle"));
}

#[test]
fn get_activity_sources_staged_marker_is_unavailable() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg,
        "alpha",
        "transcript content\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_staged",
        "Staged Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::rename(
        &seg_dir,
        seg_dir.parent().unwrap().join(format!(".removing_{seg}")),
    )
    .unwrap();

    assert_eq!(
        probe(
            &journal,
            "get_activity_sources",
            json!({"reference": act_ref})
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn get_activity_sources_cross_facet_permissions() {
    let journal = base_activity_journal();
    allow_legacy_markdown(&journal);
    let day = "20260308";
    let seg_b = "090000_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg_b,
        "beta",
        "betatranscriptneedle\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_only_beta_seg",
        "Beta Seg Activity",
        100,
        None,
        Some(&[seg_b]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"]
        .as_str()
        .unwrap()
        .to_owned();

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "facets".to_owned(),
                    "transcripts".to_owned(),
                    "messages".to_owned(),
                ],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    // Every source outside this connection's facets is a complete answer, the
    // same on every retry, never a file that failed to read.
    let out_of_reach = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&out_of_reach);
    assert_eq!(out_of_reach["units"], json!([]));
    assert_eq!(out_of_reach["unreadable"], true);
    assert_eq!(
        out_of_reach["note"],
        crate::dispatch::ACTIVITY_SOURCES_OUT_OF_REACH
    );
    assert!(!out_of_reach.to_string().contains("betatranscriptneedle"));

    let seg_a = "090500_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg_a,
        "alpha",
        "alphatranscriptneedle\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_mixed",
        "Mixed Seg Activity",
        200,
        None,
        Some(&[seg_a, seg_b]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res_mixed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let mixed_act = list_res_mixed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Mixed Seg Activity")
        .unwrap();
    let mixed_ref = mixed_act["reference"].as_str().unwrap();

    let mixed_sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": mixed_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&mixed_sources);
    let s = mixed_sources.to_string();
    assert!(s.contains("alphatranscriptneedle"));
    assert!(!s.contains("betatranscriptneedle"));
    assert_eq!(mixed_sources["unreadable"], true);
}

/// A facet that can't be read leaves a segment's scope undecided, which a
/// read reports as an error to try again, never as out of reach for good.
#[cfg(unix)]
#[test]
fn segment_scope_with_an_unreadable_facet_is_undecided() {
    use std::os::unix::fs::PermissionsExt;
    let journal = base_activity_journal();
    let day = "20260308";
    write_segment_transcript(journal.path(), day, "default", "090000_300", "alpha", "a\n");
    write_segment_transcript(journal.path(), day, "default", "090500_300", "beta", "b\n");
    let snapshot = crate::permissions::ConnectionReadSnapshot {
        categories: [solstone_core_indexer_query::AdmittedCategory::Transcripts]
            .into_iter()
            .collect(),
        scope: solstone_core_indexer_query::ConnectionScope::ChosenFacets {
            ids: [FACET_A.to_owned()].into_iter().collect(),
        },
        generation: 1,
    };
    let scope = |segment: &str| {
        crate::may_see::segment_scope(journal.path(), &snapshot, day, "default", segment)
    };
    assert_eq!(scope("090000_300"), Some(true));
    assert_eq!(scope("090500_300"), Some(false));
    let beta = journal.path().join("facets/beta/facet.json");
    fs::set_permissions(&beta, fs::Permissions::from_mode(0o000)).unwrap();
    let undecided = scope("090500_300");
    fs::set_permissions(&beta, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(undecided, None);
}

#[test]
fn get_activity_sources_omits_desktop_activity_notes() {
    let journal = base_activity_journal();
    allow_legacy_markdown(&journal);
    let day = "20260308";
    let seg = "090000_300";
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(seg_dir.join("meeting_transcript.md"), "transcriptneedle\n").unwrap();
    fs::write(
        seg_dir.join("notes_desktop_activity.jsonl"),
        "{\"text\":\"desktopneedle1\"}\n",
    )
    .unwrap();
    fs::write(
        seg_dir.join("other_desktop_activity.jsonl"),
        "{\"text\":\"desktopneedle2\"}\n",
    )
    .unwrap();
    fs::write(
        seg_dir.join("shape.json"),
        json!({"notes_desktop_activity.jsonl": "Desktop"}).to_string(),
    )
    .unwrap();

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_desk",
        "Desktop Test",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&sources);
    let s = sources.to_string();
    assert!(s.contains("transcriptneedle"));
    assert!(!s.contains("desktopneedle1"));
    assert!(!s.contains("desktopneedle2"));

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    let search1 = probe(&journal, "search", json!({"query": "desktopneedle1"})).unwrap();
    assert!(search1["results"].as_array().unwrap().is_empty());
}

fn write_three_section_weekly(journal: &tempfile::TempDir) -> std::path::PathBuf {
    let refl_dir = journal.path().join("reflections/weekly");
    fs::create_dir_all(&refl_dir).unwrap();
    let filler_1 = (0..60)
        .map(|i| {
            format!(
                "Ordinary paragraph {i} with filler text for spacing between markdown sections.\n\n"
            )
        })
        .collect::<String>();
    let filler_2 = (60..120)
        .map(|i| {
            format!(
                "Ordinary paragraph {i} with filler text for spacing between markdown sections.\n\n"
            )
        })
        .collect::<String>();
    assert!(filler_1.len() > 4096);
    assert!(filler_2.len() > 4096);
    let body = format!(
        "# Section One\n\nstartmdneedle\n\n{filler_1}\n\n# Section Two\n\nmiddlemdneedle\n\n{filler_2}\n\n# Section Three\n\nendmdneedle\n"
    );
    let md_path = refl_dir.join("20260308.md");
    fs::write(&md_path, &body).unwrap();
    let produced = produce_chunks(Family::Markdown, "reflections/weekly/20260308.md", &body);
    assert!(produced.chunks.len() >= 3);
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    md_path
}

fn fetch_weekly_reference(
    journal: &tempfile::TempDir,
    query: &str,
    absent_from_snippet: &str,
) -> String {
    let search_res = probe(journal, "search", json!({"query": query})).unwrap();
    assert_no_leaked_keys(&search_res);
    let results = search_res["results"].as_array().unwrap();
    assert!(!results.is_empty());
    let hit = &results[0];
    let snippet = hit["snippet"].as_str().unwrap();
    assert!(snippet.contains(query));
    assert!(!snippet.contains(absent_from_snippet));
    let reference = hit["reference"].as_str().unwrap().to_owned();
    let fetched = probe(journal, "fetch", json!({"reference": reference})).unwrap();
    assert_no_leaked_keys(&fetched);
    let text = fetched["text"].as_str().unwrap();
    assert!(text.contains("startmdneedle"));
    assert!(text.contains("endmdneedle"));
    reference
}

#[test]
fn fetch_markdown_whole_record_and_chunk_verification() {
    let journal = base_activity_journal();
    let md_path = write_three_section_weekly(&journal);
    let reference = fetch_weekly_reference(&journal, "middlemdneedle", "startmdneedle");

    fs::write(&md_path, "# Short\n\nreplacementonlyneedle\n").unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn fetch_markdown_first_chunk_refuses_when_expansion_cannot_be_verified() {
    let journal = base_activity_journal();
    let md_path = write_three_section_weekly(&journal);
    let reference = fetch_weekly_reference(&journal, "startmdneedle", "endmdneedle");

    fs::write(&md_path, "# Short\n\nreplacementonlyneedle\n").unwrap();
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn fetch_after_markdown_reindex_reads_the_document_and_rejects_a_shifted_reference() {
    let journal = base_activity_journal();
    let rel = "reflections/weekly/20260308.md";
    let path = journal.path().join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let legacy = "# Report\n\n[Content too large to index: 5,473 chars]\n\nfollowingneedle";
    fs::write(&path, legacy).unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    let search = |query| probe(&journal, "search", json!({"query": query})).unwrap();
    let old_hit = search("followingneedle");
    let old_reference = old_hit["results"][0]["reference"].as_str().unwrap();
    assert!(probe(&journal, "fetch", json!({"reference": old_reference})).is_ok());

    let line = "ordinary ".repeat(200);
    let updated = format!("# Report\n\n{line}\n{line}\n{line}secondpieceneedle\n\nfollowingneedle");
    fs::write(&path, &updated).unwrap();
    let conn = rusqlite::Connection::open(solstone_core_indexer_store::db::db_path(journal.path()))
        .unwrap();
    let mtime = fs::metadata(&path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    conn.execute(
        "UPDATE files SET mtime=?1 WHERE path=?2",
        rusqlite::params![mtime, rel],
    )
    .unwrap();
    conn.execute("DELETE FROM index_meta WHERE key='long_markdown_split'", [])
        .unwrap();
    drop(conn);
    assert!(
        search("secondpieceneedle")["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
    let new_hit = search("secondpieceneedle");
    let hits = new_hit["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    let reference = hits[0]["reference"].as_str().unwrap();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    assert_eq!(
        fetched["text"],
        solstone_core_format::markdown::literal_markup(&updated)
    );
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": old_reference})),
        Err(McpProbeError::Unavailable)
    );
    let batch = probe(
        &journal,
        "fetch",
        json!({"references": [old_reference, reference]}),
    )
    .unwrap();
    assert_eq!(batch["items"][0]["error"]["code"], "not_found");
    assert_eq!(batch["items"][1]["result"]["text"], fetched["text"]);
    assert_eq!(fs::read_to_string(path).unwrap(), updated);
}

#[test]
fn fetch_activity_jsonl_neighbor_isolation() {
    let journal = base_activity_journal();
    let day = "20260308";
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_iso1",
        "First Title",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("firstdescriptionunique"),
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_iso2",
        "Second Title",
        200,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("seconddescriptionunique"),
    );

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let search_res = probe(
        &journal,
        "search",
        json!({"query": "firstdescriptionunique"}),
    )
    .unwrap();
    assert_no_leaked_keys(&search_res);
    let hit = &search_res["results"].as_array().unwrap()[0];

    let fetched = probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
    assert_no_leaked_keys(&fetched);
    let text = fetched["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert!(parsed.is_object());
    assert_eq!(parsed["id"], "act_iso1");
    assert!(!text.contains("seconddescriptionunique"));
}

#[test]
fn fetch_hidden_activity_and_stale_file() {
    let journal = base_activity_journal();
    let day = "20260308";
    let rel = "facets/alpha/activities/20260308.jsonl";
    let write = |hidden: bool| {
        // The line is appended: start the day afresh each time.
        fs::create_dir_all(journal.path().join("facets/alpha/activities")).unwrap();
        fs::write(journal.path().join(rel), "").unwrap();
        write_activity_line(
            journal.path(),
            "alpha",
            day,
            "act_hidden_fetch",
            "Hidden Activity",
            100,
            None,
            None,
            None,
            None,
            None,
            Some(hidden),
            Some("hiddenfetchdescriptionneedle"),
        );
    };
    write(false);
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    // Before the hide, search finds it and its reference fetches.
    let search_res = probe(
        &journal,
        "search",
        json!({"query": "hiddenfetchdescriptionneedle"}),
    )
    .unwrap();
    assert_no_leaked_keys(&search_res);
    let hit = search_res["results"].as_array().unwrap()[0].clone();
    assert_eq!(hit["hidden"], false);
    let fetched = probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
    assert_no_leaked_keys(&fetched);
    let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["id"], "act_hidden_fetch");

    // Hidden and indexed again, it isn't returned, and the old reference finds
    // nothing.
    write(true);
    solstone_core_indexer_store::scan::rescan_file(journal.path(), Path::new(rel)).unwrap();
    let hidden_res = probe(
        &journal,
        "search",
        json!({"query": "hiddenfetchdescriptionneedle"}),
    )
    .unwrap();
    assert!(
        hidden_res["results"].as_array().unwrap().is_empty(),
        "{hidden_res}"
    );
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": hit["reference"]})),
        Err(McpProbeError::Unavailable)
    );

    // Unhidden, it is back.
    write(false);
    solstone_core_indexer_store::scan::rescan_file(journal.path(), Path::new(rel)).unwrap();
    let back = probe(
        &journal,
        "search",
        json!({"query": "hiddenfetchdescriptionneedle"}),
    )
    .unwrap();
    let back_hit = back["results"].as_array().unwrap()[0].clone();
    assert!(
        probe(
            &journal,
            "fetch",
            json!({"reference": back_hit["reference"]})
        )
        .is_ok()
    );

    let file_path = journal.path().join(rel);
    fs::write(&file_path, "modified bytes\n").unwrap();
    assert_eq!(
        probe(
            &journal,
            "fetch",
            json!({"reference": back_hit["reference"]})
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn fetch_source_links_scoped_by_facet() {
    let journal = base_activity_journal();
    allow_legacy_markdown(&journal);
    let day = "20260308";
    let seg_a = "090000_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg_a,
        "alpha",
        "transcript for alpha\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_src_link",
        "Alpha Linked Activity",
        100,
        None,
        Some(&[seg_a]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    let search_res = probe(
        &journal,
        "search",
        json!({"query": "Alpha Linked Activity"}),
    )
    .unwrap();
    let hit_ref = search_res["results"][0]["reference"].as_str().unwrap();

    let fetched = probe(&journal, "fetch", json!({"reference": hit_ref})).unwrap();
    assert_no_leaked_keys(&fetched);
    let segs = fetched["segments"].as_array().unwrap();
    assert!(!segs.is_empty());
    for s in segs {
        let obj = s.as_object().unwrap();
        for k in obj.keys() {
            assert!(
                k == "time"
                    || k == "reference"
                    || k == "source"
                    || k == "capture_zone"
                    || k == "link",
                "Unexpected key {k} in segment link"
            );
        }
    }

    let seg_b = "090500_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg_b,
        "beta",
        "betatranscriptunique\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_beta_seg",
        "Beta Seg Link Activity",
        200,
        None,
        Some(&[seg_b]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "facets".to_owned(),
                    "transcripts".to_owned(),
                    "messages".to_owned(),
                ],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let beta_seg_act = list_res["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Beta Seg Link Activity")
        .unwrap();
    let fetched_b = probe(
        &journal,
        "fetch",
        json!({"reference": beta_seg_act["reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&fetched_b);
    assert!(fetched_b.get("segments").is_none());
    assert!(!fetched_b.to_string().contains("betatranscriptunique"));
}

#[test]
fn fetch_same_key_links_stay_on_the_admitted_stream() {
    let journal = base_activity_journal();
    allow_legacy_markdown(&journal);
    let day = "20260308";
    let key = "090000_300";
    write_segment_transcript(
        journal.path(),
        day,
        "east",
        key,
        "alpha",
        "eaststreamwordneedle\n",
    );
    write_segment_transcript(
        journal.path(),
        day,
        "west",
        key,
        "alpha",
        "weststreamwordneedle\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_east",
        "East Stream Activity eaststreamactivityneedle",
        100,
        None,
        Some(&[key]),
        None,
        None,
        Some("east"),
        None,
        None,
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let search = probe(
        &journal,
        "search",
        json!({"query": "eaststreamactivityneedle"}),
    )
    .unwrap();
    let entry_ref = search["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hit| {
            hit["snippet"]
                .as_str()
                .unwrap_or("")
                .contains("eaststreamactivityneedle")
                || hit["title"]
                    .as_str()
                    .unwrap_or("")
                    .contains("eaststreamactivityneedle")
        })
        .unwrap()["reference"]
        .as_str()
        .unwrap();
    assert_stream_link_words(
        &journal,
        entry_ref,
        "east",
        "eaststreamwordneedle",
        "weststreamwordneedle",
    );

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let activity_ref = listed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| {
            activity["title"]
                .as_str()
                .unwrap()
                .contains("eaststreamactivityneedle")
        })
        .unwrap()["reference"]
        .as_str()
        .unwrap();
    assert_stream_link_words(
        &journal,
        activity_ref,
        "east",
        "eaststreamwordneedle",
        "weststreamwordneedle",
    );
}

fn assert_stream_link_words(
    journal: &tempfile::TempDir,
    reference: &str,
    stream: &str,
    kept: &str,
    excluded: &str,
) {
    let fetched = probe(journal, "fetch", json!({"reference": reference})).unwrap();
    assert_no_leaked_keys(&fetched);
    let text = fetched["text"].as_str().unwrap();
    assert!(!text.contains(excluded));
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert!(parsed.get("stream").is_none());
    assert_no_leaked_keys(&parsed);
    let links = fetched["segments"].as_array().unwrap();
    assert_eq!(links.len(), 1);
    let link_ref = links[0]["reference"].as_str().unwrap();
    assert_eq!(segment_stream(journal.path(), link_ref), stream);
    let page = probe(journal, "get_transcript", json!({"reference": link_ref})).unwrap();
    assert_no_leaked_keys(&page);
    let words = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(words.contains(kept));
    assert!(!words.contains(excluded));
    assert!(!fetched.to_string().contains(excluded));
}

#[test]
fn get_activity_sources_streamless_direct_and_named_stay_isolated() {
    let journal = base_activity_journal();
    allow_legacy_markdown(&journal);
    let day = "20260308";
    write_direct_segment_file(
        journal.path(),
        day,
        "100000_300",
        "alpha",
        "meeting_transcript.md",
        "directsourceneedle\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "default",
        "160000_300",
        "alpha",
        "meeting_transcript.md",
        "unrelateddefaultneedle\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "phone",
        "120000_300",
        "alpha",
        "meeting_transcript.md",
        "phonesourceneedle\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "phone",
        "140000_300",
        "alpha",
        "meeting_transcript.md",
        "explicitphoneneedle\n",
    );
    write_segment_file(
        journal.path(),
        day,
        "default",
        "140000_300",
        "alpha",
        "meeting_transcript.md",
        "explicitdefaultneedle\n",
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_direct",
        "Direct Streamless",
        100,
        None,
        Some(&["100000_300"]),
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_named",
        "Named Streamless",
        200,
        None,
        Some(&["120000_300"]),
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_explicit",
        "Explicit Phone",
        300,
        None,
        Some(&["140000_300"]),
        None,
        None,
        Some("phone"),
        None,
        None,
    );

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let reference = |title: &str| {
        listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["title"] == title)
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let direct_ref = reference("Direct Streamless");
    let named_ref = reference("Named Streamless");
    let explicit_ref = reference("Explicit Phone");

    for (activity_ref, id) in [
        (direct_ref.as_str(), "act_direct"),
        (named_ref.as_str(), "act_named"),
        (explicit_ref.as_str(), "act_explicit"),
    ] {
        let fetched = probe(&journal, "fetch", json!({"reference": activity_ref})).unwrap();
        assert_no_leaked_keys(&fetched);
        let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["id"], id);
        assert!(parsed.get("stream").is_none());
        assert!(!fetched.to_string().contains("unrelateddefaultneedle"));
    }

    assert_source_words(
        &journal,
        &direct_ref,
        "_default",
        "directsourceneedle",
        &[
            "unrelateddefaultneedle",
            "phonesourceneedle",
            "explicitdefaultneedle",
        ],
    );
    assert_source_words(
        &journal,
        &named_ref,
        "phone",
        "phonesourceneedle",
        &[
            "unrelateddefaultneedle",
            "directsourceneedle",
            "explicitdefaultneedle",
        ],
    );
    assert_source_words(
        &journal,
        &explicit_ref,
        "phone",
        "explicitphoneneedle",
        &[
            "explicitdefaultneedle",
            "unrelateddefaultneedle",
            "directsourceneedle",
        ],
    );
}

fn assert_source_words(
    journal: &tempfile::TempDir,
    reference: &str,
    stream: &str,
    kept: &str,
    excluded: &[&str],
) {
    let sources = probe(
        journal,
        "get_activity_sources",
        json!({"reference": reference}),
    )
    .unwrap();
    assert_no_leaked_keys(&sources);
    let units = sources["units"].as_array().unwrap();
    assert_eq!(units.len(), 1);
    let text = literal_display_text(units[0]["text"].as_str().unwrap());
    assert!(text.contains(kept), "{text}");
    for needle in excluded {
        assert!(!text.contains(needle), "{text}");
    }
    let link_ref = units[0]["source_link"]["reference"].as_str().unwrap();
    assert_eq!(segment_stream(journal.path(), link_ref), stream);
    let page = probe(journal, "get_transcript", json!({"reference": link_ref})).unwrap();
    let words = page["entries"].to_string();
    assert!(words.contains(kept));
    for needle in excluded {
        assert!(!words.contains(needle));
    }
}

#[test]
fn list_activities_normalized_hidden_matches_include_hidden_and_fetch_finds_none() {
    let journal = base_activity_journal();
    let day = "20260308";
    let raw_identity_line = r#"{"title":"Visible Raw Identity","created_at":400,"hidden":false}"#;
    let body = format!(
        "{}\n{}\n{}\n{raw_identity_line}\n",
        r#"{"id":"hid_yes","title":"Hidden Yes Title","created_at":100,"hidden":"yes"}"#,
        r#"{"id":"hid_true","title":"Hidden True Title","created_at":200,"hidden":true}"#,
        r#"{"id":"vis_false","title":"Visible False Title","created_at":300,"hidden":false}"#,
    );
    let path = journal
        .path()
        .join("facets/alpha/activities")
        .join(format!("{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &body).unwrap();

    let titles = |include_hidden: bool| {
        solstone_core_facets::load_activity_records(journal.path(), "alpha", day, include_hidden)
            .unwrap()
            .into_iter()
            .map(|row| row.get("title").and_then(Value::as_str).unwrap().to_owned())
            .collect::<BTreeSet<_>>()
    };
    let domain_visible = titles(false);
    let domain_all = titles(true);
    assert!(domain_visible.contains("Visible False Title"));
    assert!(domain_visible.contains("Visible Raw Identity"));
    assert!(!domain_visible.contains("Hidden Yes Title"));
    assert!(!domain_visible.contains("Hidden True Title"));
    assert!(domain_all.contains("Hidden Yes Title"));
    assert!(domain_all.contains("Hidden True Title"));

    let listed = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    assert_no_leaked_keys(&listed);
    let listed_titles = listed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|activity| activity["title"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(listed_titles, domain_visible);

    let activity_ref = |title: &str| {
        listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["title"] == title)
            .unwrap()["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let visible_ref = activity_ref("Visible False Title");
    let visible = probe(&journal, "fetch", json!({"reference": visible_ref})).unwrap();
    assert_no_leaked_keys(&visible);
    assert!(visible.get("hidden").is_none());
    let visible_text: Value = serde_json::from_str(visible["text"].as_str().unwrap()).unwrap();
    assert_eq!(visible_text["id"], "vis_false");
    assert_eq!(visible_text["hidden"], false);

    let raw_record: Map<String, Value> = serde_json::from_str(raw_identity_line).unwrap();
    let expected_identity =
        crate::activity_admit::compute_record_identity(&raw_record, raw_identity_line);
    let raw_ref = activity_ref("Visible Raw Identity");
    assert_eq!(
        activity_identity(journal.path(), &raw_ref),
        expected_identity
    );
    let raw_fetched = probe(&journal, "fetch", json!({"reference": raw_ref})).unwrap();
    assert!(raw_fetched.get("hidden").is_none());
    assert!(
        raw_fetched["text"]
            .as_str()
            .unwrap()
            .contains("Visible Raw Identity")
    );

    let digest = crate::activity_admit::hex_encode(&Sha256::digest(fs::read(&path).unwrap()));
    let PermissionDecision::Snapshot(snapshot) =
        evaluate_connection_read(journal.path(), CONNECTION)
    else {
        panic!("connection snapshot");
    };
    // A hidden activity's reference finds nothing, as a missing one doesn't,
    // however its file spells hidden.
    for identity in ["hid_yes", "hid_true"] {
        let reference = crate::dispatch::codec()
            .mint(
                CONNECTION,
                snapshot.generation,
                crate::references::ReferenceTarget::Activity(
                    crate::references::ActivityReference {
                        facet_id: FACET_A.to_owned(),
                        facet_dir: "alpha".to_owned(),
                        day: day.to_owned(),
                        record_identity: identity.to_owned(),
                        file_digest: digest.clone(),
                        unit_index: None,
                        source_binding: None,
                    },
                ),
            )
            .unwrap();
        assert_eq!(
            probe(&journal, "fetch", json!({"reference": reference})),
            Err(McpProbeError::Unavailable),
            "{identity}"
        );
    }
}

#[test]
fn search_derived_flags_across_record_types() {
    use crate::memory::{AppendResult, AuthenticatedMemorySource, append_connection_memory};

    let journal = base_activity_journal();
    let day = "20260308";

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_story",
        "Story Activity Title",
        100,
        None,
        None,
        None,
        None,
        None,
        Some(false),
        Some("storybodyneedle content"),
    );

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_nostory",
        "No Story Title nostoryneedle",
        200,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );

    let refl_dir = journal.path().join("reflections/weekly");
    fs::create_dir_all(&refl_dir).unwrap();
    fs::write(
        refl_dir.join("20260308.md"),
        "# Weekly Reflection\n\nweeklyreflectionneedle\n",
    )
    .unwrap();

    let source = solstone_core_format::agent_memory::SourceKey::from_verified_id("bearer:writer");
    let AppendResult::Stored(_) = append_connection_memory(
        journal.path(),
        AuthenticatedMemorySource {
            verified_id: "bearer:writer",
            creation_label: "writer",
            source: &source,
        },
        "agentmemoryneedle note content",
        "test-op",
        "2026-03-08T12:00:00Z".parse::<jiff::Timestamp>().unwrap(),
    )
    .unwrap() else {
        panic!("failed to store memory");
    };

    let seg = "090000_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg,
        "alpha",
        "transcriptsearchneedle text\n",
    );

    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let s1 = probe(&journal, "search", json!({"query": "storybodyneedle"})).unwrap();
    assert_no_leaked_keys(&s1);
    let hit1 = &s1["results"].as_array().unwrap()[0];
    assert_eq!(hit1["derived"], true);
    assert_eq!(hit1["hidden"], false);

    let s2 = probe(&journal, "search", json!({"query": "nostoryneedle"})).unwrap();
    assert_no_leaked_keys(&s2);
    let hit2 = &s2["results"].as_array().unwrap()[0];
    assert!(hit2.get("derived").is_none());

    let s3 = probe(
        &journal,
        "search",
        json!({"query": "weeklyreflectionneedle"}),
    )
    .unwrap();
    assert_no_leaked_keys(&s3);
    let hit3 = &s3["results"].as_array().unwrap()[0];
    assert_eq!(hit3["derived"], true);

    let without_notes = probe(&journal, "search", json!({"query": "agentmemoryneedle"})).unwrap();
    assert!(without_notes["results"].as_array().unwrap().is_empty());
    let mut read = ReadPermission::default_whole_journal();
    read.categories.push("notes".to_owned());
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, read)
        .unwrap();
    let s4 = probe(&journal, "search", json!({"query": "agentmemoryneedle"})).unwrap();
    assert_no_leaked_keys(&s4);
    let hit4 = &s4["results"].as_array().unwrap()[0];
    assert_eq!(hit4["origin"]["kind"], "agent_memory");
    assert!(hit4.get("derived").is_none());

    let s5 = probe(
        &journal,
        "search",
        json!({"query": "transcriptsearchneedle"}),
    )
    .unwrap();
    assert_no_leaked_keys(&s5);
    let hit5 = &s5["results"].as_array().unwrap()[0];
    assert!(hit5.get("derived").is_none());
}

/// The morning briefing and the weekly page are read from the whole journal.
/// A connection limited to chosen facets finds neither through search, and a
/// reference to either is refused by `fetch`, even one minted for its own grant.
/// The facet's own activity is the control that the chosen grant can read.
#[test]
fn a_chosen_facet_connection_reaches_neither_the_briefing_nor_the_weekly_page() {
    use crate::references::{EntryReference, ReferenceTarget};

    let journal = base_activity_journal();
    let day = "20260308";
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_alpha",
        "Alpha Activity facetcontrolneedle",
        100,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let briefing = format!("{day}/talents/morning_briefing.json");
    write_activity(
        journal.path(),
        &format!("chronicle/{briefing}"),
        &json!({
            "metadata": {"coverage_preamble": "Daily briefing."},
            "yesterday": ["Sleep from 22:10 to 06:20 briefingbodyneedle."],
            "forward_look": ["Prepare."]
        })
        .to_string(),
    );
    let weekly = format!("reflections/weekly/{day}.md");
    write_activity(
        journal.path(),
        &weekly,
        "# Weekly Reflection\n\nSleep from 22:10 to 06:20 weeklybodyneedle.\n",
    );
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let row_of = |path: &str| -> i64 {
        solstone_core_indexer_store::db::open_index(journal.path())
            .unwrap()
            .query_row(
                "SELECT rowid FROM chunks WHERE path = ?1 AND idx = 0",
                [path],
                |row| row.get(0),
            )
            .unwrap()
    };
    let briefing_row = row_of(&briefing);
    let weekly_row = row_of(&weekly);
    let reference_now = |path: &str, row_id: i64| -> String {
        let PermissionDecision::Snapshot(snapshot) =
            evaluate_connection_read(journal.path(), CONNECTION)
        else {
            panic!("the connection has a grant");
        };
        crate::dispatch::codec()
            .mint(
                CONNECTION,
                snapshot.generation,
                ReferenceTarget::Entry(EntryReference {
                    day: day.to_owned(),
                    stream: String::new(),
                    path: path.to_owned(),
                    idx: 0,
                    row_id,
                    activity_proof: None,
                    record_identity: None,
                    markdown_fingerprint: None,
                    source_digest: None,
                    byte_offset: None,
                }),
            )
            .unwrap()
    };
    let hits = |query: &str| -> usize {
        probe(&journal, "search", json!({"query": query, "limit": 10})).unwrap()["results"]
            .as_array()
            .unwrap()
            .len()
    };

    // The whole journal: the weekly page is found and fetched, so the
    // references below are well formed. The briefing is never a connection's read.
    assert_eq!(hits("weeklybodyneedle"), 1);
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": reference_now(&weekly, weekly_row)}),
    )
    .unwrap();
    assert!(
        fetched["text"]
            .as_str()
            .unwrap()
            .contains("weeklybodyneedle")
    );
    assert_eq!(hits("briefingbodyneedle"), 0);
    assert_eq!(
        probe(
            &journal,
            "fetch",
            json!({"reference": reference_now(&briefing, briefing_row)})
        ),
        Err(McpProbeError::Unavailable)
    );

    let mut chosen = ReadPermission::default_whole_journal();
    chosen.scope = ReadScope::Facets {
        ids: vec![FACET_A.to_owned()],
    };
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, chosen)
        .unwrap();

    assert_eq!(hits("facetcontrolneedle"), 1);
    assert_eq!(hits("weeklybodyneedle"), 0);
    assert_eq!(hits("briefingbodyneedle"), 0);
    for (path, row_id) in [(&weekly, weekly_row), (&briefing, briefing_row)] {
        assert_eq!(
            probe(
                &journal,
                "fetch",
                json!({"reference": reference_now(path, row_id)})
            ),
            Err(McpProbeError::Unavailable),
            "{path}"
        );
    }
    assert!(probe(&journal, "get_week", json!({"week": day})).is_err());
}

#[test]
fn get_week_grounded_activities_and_memories() {
    let journal = base_activity_journal();
    write_activity_line(
        journal.path(),
        "alpha",
        "20260308",
        "act_sun",
        "Sunday Activity",
        100,
        None,
        Some(&["090000_300"]),
        None,
        None,
        None,
        None,
        Some("hidden story body"),
    );
    write_activity_line(
        journal.path(),
        "alpha",
        "20260309",
        "act_mon",
        "Monday Activity",
        200,
        None,
        Some(&["100000_300"]),
        None,
        None,
        None,
        None,
        None,
    );

    let res = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    assert_eq!(res["week"]["start"], "20260308");
    assert_eq!(res["week"]["end"], "20260314");

    // No page for this week, so no day states and no quotes; the activities
    // and memories are still the week's own.
    assert_eq!(res["days"], json!([]));
    assert_eq!(res["said"], json!([]));

    let acts = res["activities"].as_array().unwrap();
    assert_eq!(acts.len(), 2);
    assert_eq!(res["activities_total"], 2);

    let mems = res["memories"].as_array().unwrap();
    assert_eq!(mems.len(), 2);
    for m in mems {
        let text = m["text"].as_str().unwrap();
        assert!(!text.contains("hidden story body"));
        let fetched = probe(&journal, "fetch", json!({"reference": m["source"]})).unwrap();
        assert_no_leaked_keys(&fetched);
        let parsed: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
        assert!(parsed["id"] == "act_sun" || parsed["id"] == "act_mon");
    }
}

#[test]
fn search_and_fetch_never_serve_a_retired_schedule_plan_among_events() {
    let journal = base_activity_journal();
    let events = journal.path().join("facets/alpha/events");
    fs::create_dir_all(&events).unwrap();
    fs::write(
        events.join("20260110.jsonl"),
        concat!(
            "{\"title\":\"Qday deadline\",\"type\":\"deadline\",\"agent\":\"schedule\",\"occurred\":false}\n",
            "{\"title\":\"Qday launch\",\"type\":\"milestone\",\"agent\":\"timeline\",\"occurred\":false}\n",
        ),
    )
    .unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let res = probe(&journal, "search", json!({"query": "Qday"})).unwrap();
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{res}");
    assert!(!res.to_string().contains("Qday deadline"), "{res}");
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": results[0]["reference"]}),
    )
    .unwrap();
    assert!(
        fetched["text"].as_str().unwrap().contains("Qday launch"),
        "{fetched}"
    );
}

#[test]
fn get_week_novelty_ignores_a_retired_plan_in_the_prior_week() {
    // Two otherwise equal activities: the earlier one leads unless the prior
    // week already had it, which makes the other one new.
    let ranked = |prior: &str| {
        let journal = base_activity_journal();
        let prior_dir = journal.path().join("facets/alpha/activities");
        fs::create_dir_all(&prior_dir).unwrap();
        fs::write(prior_dir.join("20260303.jsonl"), prior).unwrap();
        write_activity_line(
            journal.path(),
            "alpha",
            "20260308",
            "act_standup",
            "Standup",
            100,
            Some("standup"),
            Some(&["090000_300"]),
            None,
            None,
            None,
            None,
            None,
        );
        write_activity_line(
            journal.path(),
            "alpha",
            "20260309",
            "act_review",
            "Review",
            200,
            Some("review"),
            Some(&["090000_300"]),
            None,
            None,
            None,
            None,
            None,
        );
        let res = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
        res["activities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|activity| activity["title"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let held = r#"{"id":"prior","title":"Standup","activity":"standup","created_at":1}"#;
    assert_eq!(ranked(&format!("{held}\n")), ["Review", "Standup"]);
    let plan = r#"{"id":"plan","title":"Standup","activity":"standup","created_at":1,"source":"anticipated"}"#;
    assert_eq!(ranked(&format!("{plan}\n")), ["Standup", "Review"]);
    // Nor does a body record, which no default reader takes.
    let body = r#"{"id":"body_0123456789abcdef","title":"Standup","activity":"standup","created_at":1,"source":"body"}"#;
    assert_eq!(ranked(&format!("{body}\n")), ["Standup", "Review"]);
}

#[test]
fn search_and_fetch_skip_a_body_record_and_keep_the_next_rows_position() {
    let journal = base_activity_journal();
    let activities = journal.path().join("facets/alpha/activities");
    fs::create_dir_all(&activities).unwrap();
    fs::write(
        activities.join("20260110.jsonl"),
        concat!(
            "{\"id\":\"body_0123456789abcdef\",\"source\":\"body\",\"activity\":\"workout\",\"title\":\"Qrun body\",\"account\":{\"key\":\"import.apple_health\\u001fw1\",\"start\":\"2026-01-10 07:00:00 -0700\",\"end\":\"2026-01-10 07:40:00 -0700\"}}\n",
            "{\"id\":\"run_070000_300\",\"title\":\"Qrun review\",\"activity\":\"review\",\"created_at\":1,\"segments\":[\"070000_300\"]}\n",
        ),
    )
    .unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let res = probe(&journal, "search", json!({"query": "Qrun"})).unwrap();
    let results = res["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{res}");
    assert!(!res.to_string().contains("Qrun body"), "{res}");
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": results[0]["reference"]}),
    )
    .unwrap();
    let text = fetched["text"].as_str().unwrap();
    assert!(text.contains("Qrun review"), "{fetched}");
    assert!(!text.contains("Qrun body"), "{fetched}");
}

/// The body record of [`body_record_journal`], with a title its owner wrote.
fn hold_body_title(journal: &tempfile::TempDir, title: &str, extra: Value) {
    let path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    let text = fs::read_to_string(&path).unwrap();
    let mut record: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    record["title"] = json!(title);
    record["edits"] = json!([{"timestamp": "2026-03-09T00:00:00Z", "actor": "owner:update",
        "fields": ["title"], "note": ""}]);
    for (key, value) in extra.as_object().unwrap() {
        record[key] = value.clone();
    }
    fs::write(&path, format!("{record}\n")).unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
}

#[test]
fn search_finds_what_the_owner_wrote_on_a_body_record_and_fetch_serves_it_composed() {
    let journal = body_record_journal(true);
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    hold_body_title(&journal, "Zebrafinch loop", json!({}));
    let hits = activity_hits_for(&journal, "Zebrafinch");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].get("hidden").is_none_or(|hidden| hidden == false));
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": hits[0]["reference"]}),
    )
    .unwrap();
    let text = fetched["text"].as_str().unwrap();
    assert!(
        text.contains("Synthetic Watch") && text.contains("\"state\":\"live\""),
        "{fetched}"
    );
    // Never the file's row: its key and recorder stay home.
    assert!(
        !text.contains("recorder") && !text.contains("import.apple_health"),
        "{fetched}"
    );
}

#[test]
fn a_connection_that_may_not_read_the_window_gets_no_body_search_hit() {
    let journal = body_record_journal(true);
    hold_body_title(&journal, "Zebrafinch loop", json!({}));
    // Limited to the record's facet, while its window was refiled to another:
    // the record's own facet would pass, and the window gate doesn't.
    let facets_file = journal
        .path()
        .join(WATCH_WINDOW)
        .join("talents/facets.json");
    let filed = fs::read_to_string(&facets_file).unwrap();
    fs::write(&facets_file, r#"[{"facet":"beta"}]"#).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    assert!(activity_hits_for(&journal, "Zebrafinch").is_empty());
    fs::write(&facets_file, filed).unwrap();

    // The window deleted: nothing for any connection.
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    assert_eq!(activity_hits_for(&journal, "Zebrafinch").len(), 1);
    let window = journal.path().join(WATCH_WINDOW);
    fs::remove_file(window.join("body.jsonl")).unwrap();
    fs::write(
        window.join("tombstone.json"),
        r#"{"reason":"owner_segment_delete","manifest":["body.jsonl"]}"#,
    )
    .unwrap();
    assert!(activity_hits_for(&journal, "Zebrafinch").is_empty());
}

#[test]
fn a_body_entry_a_connection_may_not_read_doesnt_stop_search_relaxing_to_what_it_can() {
    let journal = body_record_journal(true);
    hold_body_title(&journal, "Zebrafinch loop", json!({}));
    let path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    let held = fs::read_to_string(&path).unwrap();
    let other = json!({"id": "run_080000_300", "title": "Zebrafinch review", "activity": "review",
        "created_at": 1, "segments": ["080000_300"]});
    fs::write(&path, format!("{held}{other}\n")).unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    // The body record's window is filed where this connection can't read it.
    fs::write(
        journal
            .path()
            .join(WATCH_WINDOW)
            .join("talents/facets.json"),
        r#"[{"facet":"beta"}]"#,
    )
    .unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    // Only the refused entry holds both words. Refused inside the query, it
    // doesn't stop the query relaxing to the record this connection can read.
    let hits = activity_hits_for(&journal, "Zebrafinch loop");
    assert_eq!(hits.len(), 1, "{hits:?}");
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": hits[0]["reference"]}),
    )
    .unwrap();
    assert!(
        fetched["text"]
            .as_str()
            .unwrap()
            .contains("Zebrafinch review"),
        "{fetched}"
    );
}

#[test]
fn a_body_search_hit_in_a_group_with_a_hidden_member_is_not_returned() {
    use solstone_core_format::content::body_records::{
        BodyRow, RowKind, decode_rows, encode_rows, recorder_id,
    };
    let journal = body_record_journal(true);
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    hold_body_title(&journal, "Zebrafinch loop", json!({}));
    assert_eq!(activity_hits_for(&journal, "Zebrafinch").len(), 1);
    // A second recorder's account of the same run, hidden by the owner: the
    // group reads hidden, though the held record's own line doesn't say so,
    // and a hidden activity isn't returned.
    let window = journal.path().join(WATCH_WINDOW);
    let mut rows = decode_rows(&fs::read_to_string(window.join("body.jsonl")).unwrap()).unwrap();
    let mut second = BodyRow::new(
        RowKind::Experience,
        "w2".to_owned(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "Running".to_owned(),
    );
    second.experience = Some("workout".to_owned());
    second.start = rows[0].start.clone();
    second.end = rows[0].end.clone();
    second.by = Some("Synthetic Phone".to_owned());
    rows.push(second.clone());
    fs::write(window.join("body.jsonl"), encode_rows(&rows)).unwrap();
    let path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(&format!(
        "{}\n",
        json!({"id": "body_second", "source": "body", "activity": "workout", "hidden": true,
            "account": {"key": "import.apple_health\u{1f}w2", "start": second.start, "end": second.end,
                "type": "running", "recorder": recorder_id("import.apple_health", second.by.as_deref())}})
    ));
    fs::write(&path, text).unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    let hits = activity_hits_for(&journal, "Zebrafinch");
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn a_hidden_body_record_leaves_search_and_a_body_entry_moves_no_other_entry() {
    let journal = body_record_journal(true);
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    hold_body_title(&journal, "Zebrafinch loop", json!({"hidden": true}));
    let hits = activity_hits_for(&journal, "Zebrafinch");
    assert!(hits.is_empty(), "{hits:?}");

    // A body record ahead of an ordinary one, and an unheld one before that.
    let path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    let held = fs::read_to_string(&path).unwrap();
    let bare = json!({"id": "body_bare", "source": "body", "activity": "sleep",
        "account": {"key": "import.apple_health\u{1f}none", "start": "2026-03-08 01:00:00 -0500",
            "end": "2026-03-08 02:00:00 -0500"}});
    let other = json!({"id": "run_080000_300", "title": "Zebrafinch review", "activity": "review",
        "created_at": 1, "segments": ["080000_300"]});
    let texts = |held: &str| {
        fs::write(&path, format!("{bare}\n{held}{other}\n")).unwrap();
        solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
        activity_hits_for(&journal, "Zebrafinch")
            .iter()
            .map(|hit| {
                probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap()["text"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>()
    };
    let hidden = texts(&held);
    assert_eq!(hidden.len(), 1, "{hidden:?}");
    assert!(hidden[0].contains("Zebrafinch review"));
    // Unhidden, the body record is back beside the ordinary one.
    let shown = texts(&held.replace("\"hidden\":true", "\"hidden\":false"));
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(shown.iter().any(|text| text.contains("Zebrafinch review")));
    assert!(shown.iter().any(|text| text.contains("Synthetic Watch")));
}

#[test]
fn get_week_empty_sunday() {
    let journal = base_activity_journal();
    let res = probe(&journal, "get_week", json!({"week": "20260315"})).unwrap();
    assert_no_leaked_keys(&res);
    assert_ne!(res, json!({"week": null}));
    assert!(res["activities"].as_array().unwrap().is_empty());
    assert!(res["memories"].as_array().unwrap().is_empty());
    assert!(res["said"].as_array().unwrap().is_empty());
}

#[test]
fn get_week_reads_the_week_any_day_falls_in() {
    let journal = base_activity_journal();
    let sunday = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
    for day in ["20260309", "2026-03-09", "20260314"] {
        let res = probe(&journal, "get_week", json!({"week": day})).unwrap();
        assert_eq!(
            res["week"],
            json!({"start": "20260308", "end": "20260314"}),
            "{day}"
        );
        assert_eq!(res["activities_total"], sunday["activities_total"], "{day}");
    }
}

#[test]
fn get_week_permission_denied_on_scoped_facets() {
    let journal = base_activity_journal();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();

    assert_eq!(
        probe(&journal, "get_week", json!({"week": "20260308"})),
        Err(McpProbeError::PermissionDenied)
    );

    let decision_scoped = evaluate_connection_read(journal.path(), CONNECTION);
    let tools_scoped = crate::registry::advertised_tools_list(journal.path(), &decision_scoped);
    let names_scoped: Vec<&str> = tools_scoped["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(!names_scoped.contains(&"get_week"));

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".to_owned(), "transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let decision_whole = evaluate_connection_read(journal.path(), CONNECTION);
    let tools_whole = crate::registry::advertised_tools_list(journal.path(), &decision_whole);
    let names_whole: Vec<&str> = tools_whole["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names_whole.contains(&"get_week"));
}

#[test]
fn get_week_malformed_activity_fails() {
    let journal = base_activity_journal();
    let file_path = journal
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(&file_path, "{\"id\": \"act1\"}\nmalformed-line\n").unwrap();

    assert_eq!(
        probe(&journal, "get_week", json!({"week": "20260308"})),
        Err(McpProbeError::Unavailable)
    );

    let journal2 = base_activity_journal();
    let file_path2 = journal2
        .path()
        .join("facets/alpha/activities/20260308.jsonl");
    fs::create_dir_all(file_path2.parent().unwrap()).unwrap();
    fs::write(
        &file_path2,
        "{\"id\": \"act1\", \"title\": \"Twin Valid Activity\"}\n",
    )
    .unwrap();
    let res = probe(&journal2, "get_week", json!({"week": "20260308"})).unwrap();
    assert_no_leaked_keys(&res);
    assert_eq!(res["activities"][0]["title"], "Twin Valid Activity");
}

#[test]
fn get_week_ranking_rules() {
    let journal = base_activity_journal();
    let day = "20260308";

    let names = [
        "act80", "act70", "act60", "act50", "act40", "act30", "act20", "act10",
    ];
    let lens = ["80", "70", "60", "50", "40", "30", "20", "10"];
    for (name, len) in names.iter().zip(lens.iter()) {
        let seg = format!("090000_{len}");
        write_activity_line(
            journal.path(),
            "alpha",
            day,
            name,
            name,
            100,
            Some(name),
            Some(&[&seg]),
            None,
            None,
            None,
            None,
            None,
        );
    }

    let res = probe(&journal, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res);
    let mems = res["memories"].as_array().unwrap();
    assert_eq!(mems.len(), 7);
    assert!(mems[0]["text"].as_str().unwrap().contains("act80"));
    assert!(!res.to_string().contains("act10"));

    let journal2 = base_activity_journal();
    for (name, len) in names.iter().zip(lens.iter()) {
        let seg = format!("090000_{len}");
        write_activity_line(
            journal2.path(),
            "alpha",
            day,
            name,
            name,
            100,
            Some(name),
            Some(&[&seg]),
            None,
            None,
            None,
            None,
            None,
        );
    }
    write_activity_line(
        journal2.path(),
        "alpha",
        "20260309",
        "act10_d2",
        "act10",
        100,
        Some("act10"),
        Some(&["090000_10"]),
        None,
        None,
        None,
        None,
        None,
    );

    let res2 = probe(&journal2, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res2);
    let mems2 = res2["memories"].as_array().unwrap();
    assert!(mems2[0]["text"].as_str().unwrap().contains("act10"));

    let journal3 = base_activity_journal();
    write_activity_line(
        journal3.path(),
        "alpha",
        day,
        "tied_few_ents",
        "tied_few_ents",
        100,
        Some("tied_few_ents"),
        Some(&["090000_50"]),
        Some(&["Ent1"]),
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal3.path(),
        "alpha",
        day,
        "tied_many_ents",
        "tied_many_ents",
        100,
        Some("tied_many_ents"),
        Some(&["090000_50"]),
        Some(&["Ent1", "Ent2"]),
        None,
        None,
        None,
        None,
    );
    let res3 = probe(&journal3, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res3);
    let mems3 = res3["memories"].as_array().unwrap();
    assert!(
        mems3[0]["text"]
            .as_str()
            .unwrap()
            .contains("tied_many_ents")
    );

    let journal4 = base_activity_journal();
    write_activity_line(
        journal4.path(),
        "alpha",
        "20260301",
        "act_prev",
        "stale_key",
        100,
        Some("stale_key"),
        Some(&["090000_50"]),
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal4.path(),
        "alpha",
        day,
        "act_stale_now",
        "stale_key",
        100,
        Some("stale_key"),
        Some(&["090000_50"]),
        None,
        None,
        None,
        None,
        None,
    );
    write_activity_line(
        journal4.path(),
        "alpha",
        day,
        "act_novel_now",
        "novel_key",
        100,
        Some("novel_key"),
        Some(&["090000_50"]),
        None,
        None,
        None,
        None,
        None,
    );
    let res4 = probe(&journal4, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res4);
    let mems4 = res4["memories"].as_array().unwrap();
    assert!(mems4[0]["text"].as_str().unwrap().contains("novel_key"));
    assert!(mems4[1]["text"].as_str().unwrap().contains("stale_key"));
}

#[test]
fn get_week_trusted_page_and_left_out() {
    let journal = base_activity_journal();
    let day = "20260308";
    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_trusted",
        "My Trusted Activity",
        100,
        None,
        Some(&["090000_300"]),
        None,
        None,
        Some("mic"),
        None,
        None,
    );

    let source = journal.path().join("chronicle/20260308/mic/090000_300");
    fs::create_dir_all(source.join("talents")).unwrap();
    fs::write(
        source.join("audio.jsonl"),
        json!({"sentence_id":1,"start":"00:00:01","speaker":1,"text":"Quote text from meeting"})
            .to_string(),
    )
    .unwrap();
    fs::write(
        source.join("talents/speaker_labels.json"),
        json!({"labels":[{"sentence_id":1,"speaker":"owner","method":"user_confirmed"}]})
            .to_string(),
    )
    .unwrap();
    let owner = journal.path().join("entities/owner");
    fs::create_dir_all(&owner).unwrap();
    fs::write(
        owner.join("entity.json"),
        json!({"id":"owner","type":"Person","name":"Ari","is_principal":true}).to_string(),
    )
    .unwrap();
    let refl_dir = journal.path().join("reflections/weekly");
    fs::create_dir_all(&refl_dir).unwrap();
    let trusted_json = json!({
        "version": 1,
        "days": [
            {"day":"20260308","state":"memory","memory_id":"m0"},
            {"day":"20260309","state":"nothing_shared"},
            {"day":"20260310","state":"nothing_shared"},
            {"day":"20260311","state":"nothing_shared"},
            {"day":"20260312","state":"nothing_shared"},
            {"day":"20260313","state":"nothing_shared"},
            {"day":"20260314","state":"nothing_shared"}
        ],
        "memories": [{
            "id": "m0",
            "key": "memory-key",
            "day": "20260308",
            "text": "Briefing Memory Description",
            "source": {"kind":"briefing","uri":"sol://chronicle/20260308/talents/morning_briefing"}
        }],
        "said": [{
            "id": "s0",
            "key": "said-key-quote1",
            "day": "20260308",
            "quote": "Quote text from meeting",
            "source": {"kind":"activity","uri":"sol://facets/alpha/activities/20260308#act_trusted"}
        }]
    });
    fs::write(
        refl_dir.join(format!("{day}.json")),
        trusted_json.to_string(),
    )
    .unwrap();

    let res = probe(&journal, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res);

    let said = res["said"].as_array().unwrap();
    assert_eq!(said.len(), 1);
    assert_eq!(said[0]["quote"], "Quote text from meeting");
    assert_eq!(
        said[0]["uri"],
        "sol://facets/alpha/activities/20260308#act_trusted"
    );

    let mems = res["memories"].as_array().unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0]["text"], "My Trusted Activity (09:00:00 - 09:05:00)");

    let health_dir = journal.path().join("health");
    fs::create_dir_all(&health_dir).unwrap();
    fs::write(
        health_dir.join("week-left-out.json"),
        json!({"keys": ["said-key-quote1"]}).to_string(),
    )
    .unwrap();

    let res_after = probe(&journal, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&res_after);

    assert!(res_after["said"].as_array().unwrap().is_empty());
    assert_eq!(res_after["activities"].as_array().unwrap().len(), 1);
    assert_eq!(res_after["activities"][0]["title"], "My Trusted Activity");
}

#[test]
fn get_activity_sources_empty_segment_returns_empty_units_without_unreadable() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_empty_seg",
        "Empty Seg Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let res = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&res);
    assert_eq!(res["units"], json!([]));
    assert!(res.get("unreadable").is_none());
    assert!(!res.to_string().contains("found in span"));
}

#[test]
fn get_activity_sources_malformed_leaf_is_unavailable() {
    let journal = base_activity_journal();
    allow_legacy_markdown(&journal);
    let day = "20260308";
    let seg = "090000_300";
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();
    fs::write(seg_dir.join("meeting_transcript.md"), b"\xff\xfe\xfd").unwrap();

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_malformed",
        "Malformed Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let err = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap_err();
    assert_eq!(err, McpProbeError::Unavailable);
}

#[test]
fn get_activity_sources_mixed_coverage_partial_units_and_unreadable_flag() {
    let journal = base_activity_journal();
    allow_legacy_markdown(&journal);
    let day = "20260308";
    let seg1 = "090000_300";
    let seg2 = "090500_300";
    write_segment_transcript(
        journal.path(),
        day,
        "default",
        seg1,
        "alpha",
        "mixedgoodneedle content\n",
    );

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_mixed_cov",
        "Mixed Coverage Activity",
        100,
        None,
        Some(&[seg1, seg2]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let res = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap();
    assert_no_leaked_keys(&res);
    let s = res.to_string();
    assert!(s.contains("mixedgoodneedle"));
    assert_eq!(res["unreadable"], true);
}

#[cfg(unix)]
#[test]
fn get_activity_sources_escaping_leaves_fail_closed_without_leaking() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();

    let ext_dir = tempfile::tempdir().unwrap();
    let sentinel = "sentinelext123_secret_token";
    let ext_imported = ext_dir.path().join("ext_imported.md");
    let ext_audio = ext_dir.path().join("ext_audio.jsonl");
    fs::write(&ext_imported, format!("# Secret\n{sentinel}\n")).unwrap();
    fs::write(&ext_audio, format!("{{\"text\":\"{sentinel}\"}}\n")).unwrap();

    std::os::unix::fs::symlink(&ext_imported, seg_dir.join("imported.md")).unwrap();
    std::os::unix::fs::symlink(&ext_audio, seg_dir.join("audio.jsonl")).unwrap();

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_escaping",
        "Escaping Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let err = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    )
    .unwrap_err();
    assert_eq!(err, McpProbeError::Unavailable);
    assert!(!format!("{err:?}").contains(sentinel));
}

#[cfg(unix)]
#[test]
fn get_activity_sources_poisoned_sibling_fails_closed_without_leaking() {
    let journal = base_activity_journal();
    let day = "20260308";
    let seg = "090000_300";
    let seg_dir = journal
        .path()
        .join("chronicle")
        .join(day)
        .join("default")
        .join(seg);
    fs::create_dir_all(seg_dir.join("talents")).unwrap();
    fs::write(
        seg_dir.join("talents/facets.json"),
        json!([{"facet": "alpha"}]).to_string(),
    )
    .unwrap();

    let good_needle = "goodleafneedle_present";
    fs::write(
        seg_dir.join("meeting_transcript.md"),
        format!("{good_needle}\n"),
    )
    .unwrap();

    let ext_dir = tempfile::tempdir().unwrap();
    let poison_sentinel = "poisonsentinel456_poison_token";
    let ext_imported = ext_dir.path().join("poison_imported.md");
    fs::write(&ext_imported, format!("{poison_sentinel}\n")).unwrap();

    std::os::unix::fs::symlink(&ext_imported, seg_dir.join("imported.md")).unwrap();

    write_activity_line(
        journal.path(),
        "alpha",
        day,
        "act_poisoned",
        "Poisoned Sibling Activity",
        100,
        None,
        Some(&[seg]),
        None,
        None,
        Some("default"),
        None,
        None,
    );

    let list_res = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    let act_ref = list_res["activities"][0]["reference"].as_str().unwrap();

    let res = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref}),
    );
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert_eq!(err, McpProbeError::Unavailable);
    let err_str = format!("{err:?}");
    assert!(!err_str.contains(poison_sentinel));
    assert!(!err_str.contains(good_needle));
}

/// An alpha-filed Apple workout window on 20260308 and its body record, beside
/// a capture activity in beta.
fn body_record_journal(with_body: bool) -> tempfile::TempDir {
    body_record_journal_by(with_body, "Synthetic Watch")
}

/// [`body_record_journal`], its workout recorded by `by`.
fn body_record_journal_by(with_body: bool, by: &str) -> tempfile::TempDir {
    use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows, recorder_id};
    let journal = base_activity_journal();
    let root = journal.path();
    write_activity_line(
        root,
        "beta",
        "20260308",
        "act_beta",
        "Beta work",
        100,
        Some("work"),
        Some(&["090000_300"]),
        None,
        None,
        None,
        None,
        None,
    );
    if !with_body {
        return journal;
    }
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.apple_health-zone.json"),
        r#"{"zone":"America/New_York"}"#,
    )
    .unwrap();
    let mut row = BodyRow::new(
        RowKind::Experience,
        "w1".to_owned(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "Running".to_owned(),
    );
    row.experience = Some("workout".to_owned());
    row.start = Some("2026-03-08 07:00:00 -0400".to_owned());
    row.end = Some("2026-03-08 07:40:00 -0400".to_owned());
    row.by = Some(by.to_owned());
    let window = root.join("chronicle/20260308/import.apple_health/070000_3600");
    fs::create_dir_all(window.join("talents")).unwrap();
    fs::write(
        window.join("body.jsonl"),
        encode_rows(std::slice::from_ref(&row)),
    )
    .unwrap();
    fs::write(window.join("stream.json"), "{}\n").unwrap();
    fs::write(window.join("talents/facets.json"), r#"[{"facet":"alpha"}]"#).unwrap();
    let record = json!({
        "id": "body_0123456789abcdef", "source": "body", "activity": "workout",
        "account": {"key": "import.apple_health\u{1f}w1", "start": row.start, "end": row.end,
            "type": "running", "recorder": recorder_id("import.apple_health", row.by.as_deref())},
    });
    let path = root.join("facets/alpha/activities/20260308.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("{record}\n")).unwrap();
    journal
}

fn listed(journal: &tempfile::TempDir, permission: Option<ReadPermission>) -> Value {
    if let Some(permission) = permission {
        PermissionStore::open(journal.path())
            .set_permission(CONNECTION, permission)
            .unwrap();
    }
    let mut res = probe(
        journal,
        "list_activities",
        json!({"from": "20260308", "to": "20260314"}),
    )
    .unwrap();
    // Compare content across independent journals; their pointer keys differ.
    for activity in res["activities"].as_array_mut().unwrap() {
        activity.as_object_mut().unwrap().remove("reference");
        activity.as_object_mut().unwrap().remove("link");
    }
    res
}

#[test]
fn list_activities_shows_a_body_record_only_to_a_connection_that_reads_its_window() {
    let with = body_record_journal(true);
    let res = listed(&with, None);
    let body = res["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| activity["source"] == "body")
        .unwrap_or_else(|| panic!("{res}"))
        .clone();
    assert_eq!(body["title"], "Running");
    assert_eq!(body["kind"], "workout");
    assert_eq!(body["accounts"][0]["by"], "Synthetic Watch");
    assert_eq!(body["facet"], "Alpha");

    let alpha_with_transcripts = ReadPermission {
        categories: vec![
            "facets".to_owned(),
            "transcripts".to_owned(),
            "body".to_owned(),
        ],
        scope: ReadScope::Facets {
            ids: vec![FACET_A.to_owned()],
        },
    };
    assert!(
        listed(&with, Some(alpha_with_transcripts))
            .to_string()
            .contains("Synthetic Watch")
    );

    // A beta-limited connection gets exactly what it would with no body data
    // at all: the record's window isn't in its scope.
    let beta = ReadPermission {
        categories: vec![
            "facets".to_owned(),
            "transcripts".to_owned(),
            "body".to_owned(),
        ],
        scope: ReadScope::Facets {
            ids: vec![FACET_B.to_owned()],
        },
    };
    assert_eq!(
        listed(&body_record_journal(true), Some(beta.clone())),
        listed(&body_record_journal(false), Some(beta)),
    );

    // Without the body permission the record isn't listed, and the answer says
    // why instead of reading as no body data at all.
    for categories in [vec!["facets"], vec!["facets", "transcripts"]] {
        let permission = ReadPermission {
            categories: categories.iter().map(|kind| (*kind).to_owned()).collect(),
            scope: ReadScope::WholeJournal,
        };
        let with_body = listed(&body_record_journal(true), Some(permission.clone()));
        let without = listed(&body_record_journal(false), Some(permission.clone()));
        assert_eq!(
            with_body["activities"], without["activities"],
            "{permission:?}"
        );
        assert!(without.get("body_note").is_none(), "{without}");
        assert_eq!(
            with_body["body_note"],
            crate::body_source::BODY_NOT_GRANTED,
            "{with_body}"
        );
    }
}

#[test]
fn a_body_record_left_in_one_facet_after_its_window_moved_reaches_no_limited_connection() {
    let journal = body_record_journal(true);
    let root = journal.path();
    // The record stays in beta's file; its window is now filed under alpha.
    let record = fs::read_to_string(root.join("facets/alpha/activities/20260308.jsonl")).unwrap();
    fs::remove_file(root.join("facets/alpha/activities/20260308.jsonl")).unwrap();
    let beta = root.join("facets/beta/activities/20260308.jsonl");
    let mut text = fs::read_to_string(&beta).unwrap();
    text.push_str(&record);
    fs::write(&beta, text).unwrap();

    let beta_only = ReadPermission {
        categories: vec!["facets".to_owned(), "transcripts".to_owned()],
        scope: ReadScope::Facets {
            ids: vec![FACET_B.to_owned()],
        },
    };
    assert!(
        !listed(&journal, Some(beta_only))
            .to_string()
            .contains("Synthetic Watch")
    );
    assert!(
        listed(&journal, Some(ReadPermission::default_whole_journal()))
            .to_string()
            .contains("Synthetic Watch")
    );
}

#[test]
fn fetch_on_a_body_reference_serves_its_composed_face_and_nothing_raw() {
    let journal = body_record_journal(true);
    let res = probe(
        &journal,
        "list_activities",
        json!({"from": "20260308", "to": "20260314"}),
    )
    .unwrap();
    let reference = res["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| activity["source"] == "body")
        .unwrap()["reference"]
        .clone();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    let text = fetched["text"].as_str().unwrap();
    assert!(
        text.contains("Synthetic Watch") && text.contains("\"state\":\"live\""),
        "{fetched}"
    );
    // The record's own bytes are never served: its key and recorder stay home.
    assert!(
        !text.contains("recorder") && !text.contains("import.apple_health"),
        "{fetched}"
    );
}

#[test]
fn get_activity_sources_on_a_body_reference_gives_its_accounts() {
    let journal = body_record_journal(true);
    let res = probe(
        &journal,
        "list_activities",
        json!({"from": "20260308", "to": "20260314"}),
    )
    .unwrap();
    let reference = res["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| activity["source"] == "body")
        .unwrap()["reference"]
        .clone();
    let sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference}),
    )
    .unwrap();
    assert_eq!(sources["units"][0]["by"], "Synthetic Watch", "{sources}");
    assert!(!sources.to_string().contains("recorder"), "{sources}");
}

#[test]
fn get_week_shows_a_body_record_composed() {
    let journal = body_record_journal(true);
    let week = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
    assert_eq!(
        week["body"][0]["accounts"][0]["by"], "Synthetic Watch",
        "{week}"
    );
    assert_eq!(week["body"][0]["kind"], "workout");
    // get_week itself reads only for a whole-journal connection with transcripts.
}

#[test]
fn beside_lines_go_both_ways_and_never_past_what_a_connection_reads() {
    let journal = body_record_journal(true);
    let root = journal.path();
    fs::create_dir_all(root.join("chronicle/20260308/phone/070000_300")).unwrap();
    for facet in ["alpha", "beta"] {
        write_activity_line(
            root,
            facet,
            "20260308",
            &format!("act_{facet}_run"),
            &format!("{facet} capture"),
            50,
            Some("running"),
            Some(&["070000_300"]),
            None,
            None,
            None,
            None,
            None,
        );
    }
    let res = listed(&journal, None);
    let activities = res["activities"].as_array().unwrap();
    let by_title = |title: &str| {
        activities
            .iter()
            .find(|activity| activity["title"] == title)
            .unwrap_or_else(|| panic!("{title}: {res}"))
            .clone()
    };
    assert_eq!(
        by_title("alpha capture")["beside"][0]["kind"],
        "workout",
        "{res}"
    );
    assert_eq!(
        by_title("Running")["beside"].as_array().unwrap().len(),
        2,
        "{res}"
    );

    let beta_only = ReadPermission {
        categories: vec!["facets".to_owned(), "transcripts".to_owned()],
        scope: ReadScope::Facets {
            ids: vec![FACET_B.to_owned()],
        },
    };
    let res = listed(&journal, Some(beta_only));
    let beta = res["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| activity["title"] == "beta capture")
        .unwrap()
        .clone();
    assert!(beta.get("beside").is_none(), "{res}");
    assert!(!res.to_string().contains("Running"), "{res}");
}

#[test]
fn pages_after_a_cursor_list_what_one_page_lists_beside_lines_included() {
    use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows, recorder_id};
    let journal = body_record_journal(true);
    let root = journal.path();
    // A late workout filed under 20260308 that runs past midnight.
    let mut row = BodyRow::new(
        RowKind::Experience,
        "w2".to_owned(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "Running".to_owned(),
    );
    row.experience = Some("workout".to_owned());
    row.start = Some("2026-03-08 23:30:00 -0400".to_owned());
    row.end = Some("2026-03-09 00:30:00 -0400".to_owned());
    row.by = Some("Synthetic Watch".to_owned());
    let window = root.join("chronicle/20260308/import.apple_health/230000_3600");
    fs::create_dir_all(window.join("talents")).unwrap();
    fs::write(
        window.join("body.jsonl"),
        encode_rows(std::slice::from_ref(&row)),
    )
    .unwrap();
    fs::write(window.join("stream.json"), "{}\n").unwrap();
    fs::write(window.join("talents/facets.json"), r#"[{"facet":"alpha"}]"#).unwrap();
    let record = json!({
        "id": "body_fedcba9876543210", "source": "body", "activity": "workout",
        "account": {"key": "import.apple_health\u{1f}w2", "start": row.start, "end": row.end,
            "type": "running", "recorder": recorder_id("import.apple_health", row.by.as_deref())},
    });
    let path = root.join("facets/alpha/activities/20260308.jsonl");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(&format!("{record}\n"));
    fs::write(&path, text).unwrap();
    // Two captures on 20260309, the second inside the late workout: its page
    // comes after a cursor on 20260309, and its beside line is the day before's.
    fs::create_dir_all(root.join("chronicle/20260309/phone/000000_300")).unwrap();
    for (facet, id, title) in [
        ("alpha", "act_early", "early capture"),
        ("beta", "act_late", "late capture"),
    ] {
        write_activity_line(
            root,
            facet,
            "20260309",
            id,
            title,
            50,
            Some("running"),
            Some(&["000000_300"]),
            None,
            None,
            None,
            None,
            None,
        );
    }
    PermissionStore::open(root)
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let strip = |page: &Value| {
        page["activities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|activity| {
                let mut activity = activity.clone();
                activity.as_object_mut().unwrap().remove("reference");
                activity
            })
            .collect::<Vec<_>>()
    };
    let args = json!({"from": "20260308", "to": "20260314"});
    let whole = strip(&probe(&journal, "list_activities", args.clone()).unwrap());
    let mut paged = Vec::new();
    let mut args = json!({"from": "20260308", "to": "20260314", "limit": 1});
    loop {
        let page = probe(&journal, "list_activities", args.clone()).unwrap();
        paged.extend(strip(&page));
        if page["next_cursor"].is_null() {
            break;
        }
        args["cursor"] = page["next_cursor"].clone();
    }
    assert_eq!(paged, whole);
    let late = whole
        .iter()
        .find(|activity| activity["title"] == "late capture")
        .unwrap_or_else(|| panic!("{whole:?}"));
    assert_eq!(late["day"], "20260309");
    assert_eq!(late["beside"][0]["kind"], "workout", "{late}");
    assert_eq!(
        late["beside"][0]["start"], "2026-03-08 23:30:00 -0400",
        "{late}"
    );
}

/// A synthetic workout account, its window and its record, filed under `day`.
fn file_workout(root: &Path, day: &str, row_id: &str, by: &str, start: &str, end: &str) {
    use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows, recorder_id};
    let mut row = BodyRow::new(
        RowKind::Experience,
        row_id.to_owned(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "Running".to_owned(),
    );
    row.experience = Some("workout".to_owned());
    row.start = Some(start.to_owned());
    row.end = Some(end.to_owned());
    row.by = Some(by.to_owned());
    let hour = &start[11..13];
    let window = root.join(format!(
        "chronicle/{day}/import.apple_health/{hour}0000_3600"
    ));
    fs::create_dir_all(window.join("talents")).unwrap();
    let rows = window.join("body.jsonl");
    let mut text = fs::read_to_string(&rows).unwrap_or_default();
    text.push_str(&encode_rows(std::slice::from_ref(&row)));
    fs::write(&rows, text).unwrap();
    fs::write(window.join("stream.json"), "{}\n").unwrap();
    fs::write(window.join("talents/facets.json"), r#"[{"facet":"alpha"}]"#).unwrap();
    let id = format!("body_{:0>16}", row_id);
    let record = json!({
        "id": id, "source": "body", "activity": "workout",
        "account": {"key": format!("import.apple_health\u{1f}{row_id}"), "start": start, "end": end,
            "type": "running", "recorder": recorder_id("import.apple_health", Some(by))},
    });
    let path = root.join(format!("facets/alpha/activities/{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{record}\n"));
    fs::write(&path, text).unwrap();
}

#[test]
fn a_long_range_pages_the_same_whatever_the_page_size_across_stretches_of_days() {
    let journal = body_record_journal(true);
    let root = journal.path();
    // From 20260308 the list reads 31 days at a time from 20260307. One run past
    // midnight told by two recorders, the app's starting after midnight, so filed
    // either side of the first boundary (20260406 and 20260407), groups once;
    // others sit a stretch or two apart.
    file_workout(
        root,
        "20260406",
        "a1",
        "Synthetic Watch",
        "2026-04-06 23:50:00 -0400",
        "2026-04-07 01:00:00 -0400",
    );
    file_workout(
        root,
        "20260407",
        "a2",
        "Synthetic App",
        "2026-04-07 00:01:00 -0400",
        "2026-04-07 01:00:00 -0400",
    );
    file_workout(
        root,
        "20260408",
        "b1",
        "Synthetic Watch",
        "2026-04-08 07:00:00 -0400",
        "2026-04-08 07:30:00 -0400",
    );
    file_workout(
        root,
        "20260520",
        "c1",
        "Synthetic Watch",
        "2026-05-20 07:00:00 -0400",
        "2026-05-20 07:30:00 -0400",
    );
    PermissionStore::open(root)
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let list = |limit: usize| {
        let mut args = json!({"from": "20260308", "to": "20260630", "limit": limit});
        let mut all = Vec::new();
        loop {
            let page = probe(&journal, "list_activities", args.clone()).unwrap();
            for activity in page["activities"].as_array().unwrap() {
                let mut activity = activity.clone();
                activity.as_object_mut().unwrap().remove("reference");
                all.push(activity);
            }
            if page["next_cursor"].is_null() {
                break all;
            }
            args["cursor"] = page["next_cursor"].clone();
        }
    };
    let whole = list(100);
    assert_eq!(list(1), whole);
    assert_eq!(list(2), whole);
    let starts = whole
        .iter()
        .filter(|activity| activity["source"] == "body")
        .map(|activity| {
            (
                activity["day"].clone(),
                activity["accounts"].as_array().unwrap().len(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts,
        [
            (json!("20260308"), 1),
            (json!("20260406"), 2),
            (json!("20260408"), 1),
            (json!("20260520"), 1)
        ],
        "{whole:?}"
    );
}

/// Every `by` a reader's output names, wherever it sits.
fn every_by(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                match (key.as_str(), value) {
                    ("by", Value::String(by)) => found.push(by.clone()),
                    _ => every_by(value, found),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| every_by(item, found)),
        _ => {}
    }
}

#[test]
fn a_source_apps_name_reaches_every_reader_literal() {
    let raw = "![x](https://evil.test/p.png)";
    let journal = body_record_journal_by(true, raw);
    let listed = probe(
        &journal,
        "list_activities",
        json!({"from": "20260308", "to": "20260314"}),
    )
    .unwrap();
    let reference = listed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| activity["source"] == "body")
        .unwrap()["reference"]
        .clone();
    let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
    let sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": reference}),
    )
    .unwrap();
    let week = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
    // fetch serves the face as JSON text; read what it says.
    let fetched_face = serde_json::from_str::<Value>(fetched["text"].as_str().unwrap()).unwrap();
    let literal = solstone_core_format::markdown::literal_markup(raw);
    assert_ne!(literal, raw);
    for (reader, output, shown) in [
        ("list_activities", &listed, &listed),
        ("fetch", &fetched, &fetched_face),
        ("get_activity_sources", &sources, &sources),
        ("get_week", &week, &week),
    ] {
        assert!(!output.to_string().contains(raw), "{reader}: {output}");
        let mut found = Vec::new();
        every_by(shown, &mut found);
        assert!(!found.is_empty(), "{reader}: {output}");
        assert!(found.iter().all(|by| by == &literal), "{reader}: {found:?}");
    }
}

#[test]
fn a_bad_line_on_a_neighbouring_day_fails_no_list() {
    let journal = body_record_journal(true);
    // A day outside the range and the week before it, read only to group:
    // one half-written line.
    fs::write(
        journal
            .path()
            .join("facets/alpha/activities/20260225.jsonl"),
        "{\"id\":\"body_q\",\"sou\n",
    )
    .unwrap();
    let listed = listed(&journal, None);
    assert!(
        listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|activity| activity["source"] == "body"),
        "{listed}"
    );
    let week = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
    assert_eq!(week["body"][0]["kind"], "workout", "{week}");
}

#[test]
fn a_vanished_window_shows_its_record_to_no_connection() {
    let journal = body_record_journal(true);
    // The window's directory is gone, with no tombstone left.
    fs::remove_dir_all(
        journal
            .path()
            .join("chronicle/20260308/import.apple_health/070000_3600"),
    )
    .unwrap();
    let listed = listed(&journal, None);
    assert!(
        listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .all(|activity| activity["source"] != "body"),
        "{listed}"
    );
    let week = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
    assert!(week.get("body").is_none(), "{week}");
    // The owner still sees it, saying so.
    let faces = solstone_core_location::body_records::body_faces_range(
        journal.path(),
        "alpha",
        "20260308",
        "20260308",
        None,
        true,
    )
    .unwrap();
    assert_eq!(
        faces["20260308"][0].anchor.account,
        solstone_core_location::body_records::Account::NotFound
    );
}

#[test]
fn every_body_answer_names_the_windows_it_read() {
    let window = "20260308/import.apple_health/070000_3600";
    for tool in [
        "list_activities",
        "fetch",
        "get_activity_sources",
        "get_week",
    ] {
        let journal = body_record_journal(true);
        let listed = probe(
            &journal,
            "list_activities",
            json!({"from": "20260308", "to": "20260314"}),
        )
        .unwrap();
        let reference = listed["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["source"] == "body")
            .unwrap()["reference"]
            .clone();
        match tool {
            "fetch" => drop(probe(&journal, "fetch", json!({"reference": reference})).unwrap()),
            "get_activity_sources" => drop(
                probe(
                    &journal,
                    "get_activity_sources",
                    json!({"reference": reference}),
                )
                .unwrap(),
            ),
            "get_week" => drop(probe(&journal, "get_week", json!({"week": "20260308"})).unwrap()),
            _ => {}
        }
        let outcomes = find_audit_files(journal.path(), solstone_core_mcp_audit::OUTCOME_FILE);
        assert!(!outcomes.is_empty(), "{tool}");
        for outcome in outcomes {
            let outcome: Value =
                serde_json::from_str(&fs::read_to_string(&outcome).unwrap()).unwrap();
            let targets = outcome["result"]["targets"].as_array().unwrap();
            assert!(
                targets.iter().any(|target| target == window),
                "{tool}: {targets:?}"
            );
        }
    }
}

/// A second account of the fixture's run, by another recorder, in alpha too.
fn add_second_account(root: &Path) {
    use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows, recorder_id};
    let mut row = BodyRow::new(
        RowKind::Experience,
        "w2".to_owned(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "Running".to_owned(),
    );
    row.experience = Some("workout".to_owned());
    row.start = Some("2026-03-08 07:01:00 -0400".to_owned());
    row.end = Some("2026-03-08 07:41:00 -0400".to_owned());
    row.by = Some("Synthetic App".to_owned());
    let window = root.join("chronicle/20260308/import.apple_health/070000_300");
    fs::create_dir_all(window.join("talents")).unwrap();
    fs::write(
        window.join("body.jsonl"),
        encode_rows(std::slice::from_ref(&row)),
    )
    .unwrap();
    fs::write(window.join("stream.json"), "{}\n").unwrap();
    fs::write(window.join("talents/facets.json"), r#"[{"facet":"alpha"}]"#).unwrap();
    let record = json!({
        "id": "body_fedcba9876543210", "source": "body", "activity": "workout",
        "account": {"key": "import.apple_health\u{1f}w2", "start": row.start, "end": row.end,
            "type": "running", "recorder": recorder_id("import.apple_health", row.by.as_deref())},
    });
    let path = root.join("facets/alpha/activities/20260308.jsonl");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(&format!("{record}\n"));
    fs::write(path, text).unwrap();
}

/// Hide one member of a group by id, as an owner's write leaves it.
fn hide_member(root: &Path, id: &str) {
    let path = root.join("facets/alpha/activities/20260308.jsonl");
    let text = fs::read_to_string(&path).unwrap().replace(
        &format!("\"id\":\"{id}\""),
        &format!("\"id\":\"{id}\",\"hidden\":true"),
    );
    assert!(text.contains("\"hidden\":true"), "{text}");
    fs::write(path, text).unwrap();
}

#[test]
fn get_week_leaves_out_a_group_any_member_of_which_is_hidden() {
    for hidden in ["body_0123456789abcdef", "body_fedcba9876543210"] {
        let journal = body_record_journal(true);
        add_second_account(journal.path());
        let week = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
        assert_eq!(week["body"].as_array().unwrap().len(), 1, "{week}");
        hide_member(journal.path(), hidden);
        let week = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
        assert!(
            week.get("body")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty),
            "{hidden}: {week}"
        );
    }
}

/// A live window that no longer holds the row: the face shows, and sits
/// beside nothing, either way.
#[test]
fn a_face_whose_row_is_gone_from_its_live_window_sits_beside_nothing() {
    let journal = body_record_journal(true);
    let root = journal.path();
    fs::create_dir_all(root.join("chronicle/20260308/phone/070000_300")).unwrap();
    write_activity_line(
        root,
        "alpha",
        "20260308",
        "act_alpha_run",
        "alpha capture",
        50,
        Some("running"),
        Some(&["070000_300"]),
        None,
        None,
        None,
        None,
        None,
    );
    let by_title = |res: &Value, title: &str| {
        res["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["title"] == title)
            .unwrap_or_else(|| panic!("{title}: {res}"))
            .clone()
    };
    let res = listed(&journal, None);
    assert!(
        by_title(&res, "alpha capture").get("beside").is_some(),
        "{res}"
    );
    fs::write(
        root.join("chronicle/20260308/import.apple_health/070000_3600/body.jsonl"),
        "",
    )
    .unwrap();
    let res = listed(&journal, None);
    let body = res["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| activity["source"] == "body")
        .unwrap_or_else(|| panic!("{res}"))
        .clone();
    assert!(body.to_string().contains("not_found"), "{body}");
    assert!(body.get("beside").is_none(), "{body}");
    assert!(
        by_title(&res, "alpha capture").get("beside").is_none(),
        "{res}"
    );
}

/// Where a device was during a body item reaches only a connection that reads
/// the whole journal.
#[test]
fn a_body_item_says_where_a_device_was_only_to_a_whole_journal_connection() {
    let journal = body_record_journal(true);
    let root = journal.path();
    // A visit the phone reported near Denver, over the run.
    let segment = root.join("chronicle/20260308/phone_mobile-segment/070000_300");
    fs::create_dir_all(&segment).unwrap();
    fs::write(
        segment.join("location.jsonl"),
        concat!(
            r#"{"accuracy":"full","fix_count":0,"gap":false,"kind":"location","platform":"ios","schema":"solstone.location.segment/1","source":"location","tier":"full"}"#,
            "\n",
            r#"{"arrival":"2026-03-08T11:00:00Z","departure":"2026-03-08T12:00:00Z","h_acc":40,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}"#,
            "\n"
        ),
    )
    .unwrap();
    solstone_core_location::reconcile_journal(
        root,
        &mut solstone_core_location::Reconciler::default(),
    )
    .unwrap();
    let place = |permission: ReadPermission| {
        listed(&journal, Some(permission))["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|activity| activity["source"] == "body")
            .map(|activity| activity.get("place").cloned().unwrap_or(Value::Null))
    };
    let mut whole = ReadPermission::default_whole_journal();
    assert_eq!(place(whole.clone()), Some(Value::Null));
    whole.categories.push("places".to_owned());
    let whole = place(whole).unwrap();
    assert!(
        whole["words"]
            .as_str()
            .is_some_and(|words| words.starts_with("near Denver")),
        "{whole}"
    );
    assert_eq!(place(alpha_reader()), Some(Value::Null));
}

/// Each way a list reads a body window names it, alone: a page holding only the
/// body item, and a page holding only a capture a body face could sit beside.
#[test]
fn a_list_page_names_the_body_windows_it_read_with_or_without_capture_on_it() {
    let window = "20260308/import.apple_health/070000_3600";
    let journal = body_record_journal(true);
    let root = journal.path();
    let outcomes = |root: &Path| {
        find_audit_files(root, solstone_core_mcp_audit::OUTCOME_FILE)
            .into_iter()
            .collect::<BTreeSet<_>>()
    };
    let mut cursor: Option<Value> = None;
    let mut pages = Vec::new();
    loop {
        let before = outcomes(root);
        let mut arguments = json!({"from": "20260308", "to": "20260314", "limit": 1});
        if let Some(cursor) = &cursor {
            arguments["cursor"] = cursor.clone();
        }
        let res = probe(&journal, "list_activities", arguments).unwrap();
        let new = outcomes(root)
            .difference(&before)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(new.len(), 1, "{new:?}");
        let outcome: Value = serde_json::from_str(&fs::read_to_string(&new[0]).unwrap()).unwrap();
        let named = outcome["result"]["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|target| target == window);
        let items = res["activities"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{res}");
        pages.push((items[0]["source"] == "body", named));
        match res.get("next_cursor").filter(|next| !next.is_null()) {
            Some(next) => cursor = Some(next.clone()),
            None => break,
        }
    }
    assert_eq!(pages.len(), 2, "{pages:?}");
    assert!(pages.iter().any(|(body, _)| *body), "{pages:?}");
    assert!(pages.iter().any(|(body, _)| !*body), "{pages:?}");
    assert!(pages.iter().all(|(_, named)| *named), "{pages:?}");
}

fn alpha_reader() -> ReadPermission {
    ReadPermission {
        categories: vec![
            "facets".to_owned(),
            "transcripts".to_owned(),
            "body".to_owned(),
        ],
        scope: ReadScope::Facets {
            ids: vec![FACET_A.to_owned()],
        },
    }
}

fn body_reference(journal: &tempfile::TempDir) -> Value {
    let res = probe(
        journal,
        "list_activities",
        json!({"from": "20260308", "to": "20260314"}),
    )
    .unwrap();
    res["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| activity["source"] == "body")
        .unwrap_or_else(|| panic!("{res}"))["reference"]
        .clone()
}

fn shows_body(journal: &tempfile::TempDir) -> bool {
    probe(
        journal,
        "list_activities",
        json!({"from": "20260308", "to": "20260314"}),
    )
    .unwrap()["activities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|activity| activity["source"] == "body")
}

const WATCH_WINDOW: &str = "chronicle/20260308/import.apple_health/070000_3600";

#[test]
fn fetch_and_sources_refuse_a_body_reference_whose_window_a_connection_no_longer_reads() {
    let journal = body_record_journal(true);
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, alpha_reader())
        .unwrap();
    let reference = body_reference(&journal);
    // The window is filed under beta now; the record stays in alpha's file.
    fs::write(
        journal
            .path()
            .join(WATCH_WINDOW)
            .join("talents/facets.json"),
        r#"[{"facet":"beta"}]"#,
    )
    .unwrap();
    for tool in ["fetch", "get_activity_sources"] {
        let answer = probe(&journal, tool, json!({"reference": reference}));
        assert!(
            answer.is_err()
                || !answer
                    .as_ref()
                    .unwrap()
                    .to_string()
                    .contains("Synthetic Watch"),
            "{tool}: {answer:?}"
        );
    }
}

#[test]
fn no_connection_sees_a_body_record_whose_window_was_deleted() {
    let journal = body_record_journal(true);
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    assert!(shows_body(&journal));
    let window = journal.path().join(WATCH_WINDOW);
    fs::remove_file(window.join("body.jsonl")).unwrap();
    fs::write(
        window.join("tombstone.json"),
        r#"{"reason":"owner_segment_delete","manifest":["body.jsonl"]}"#,
    )
    .unwrap();
    assert!(!shows_body(&journal));
}

#[test]
fn a_deleted_start_beside_a_live_hour_window_stays_unread() {
    let journal = body_record_journal(true);
    let root = journal.path();
    PermissionStore::open(root)
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    // The row sat in a five-minute window, now deleted; the hour window over
    // the same start is live and never held it.
    let hour = root.join(WATCH_WINDOW);
    let short = root.join("chronicle/20260308/import.apple_health/070000_300");
    fs::create_dir_all(short.join("talents")).unwrap();
    fs::copy(
        hour.join("talents/facets.json"),
        short.join("talents/facets.json"),
    )
    .unwrap();
    fs::write(short.join("stream.json"), "{}\n").unwrap();
    fs::write(
        short.join("tombstone.json"),
        r#"{"reason":"owner_segment_delete","manifest":["body.jsonl"]}"#,
    )
    .unwrap();
    fs::write(hour.join("body.jsonl"), "").unwrap();
    assert!(!shows_body(&journal));
}

#[test]
fn a_capture_shows_no_body_beside_it_from_a_window_its_connection_cannot_read() {
    let journal = body_record_journal(true);
    let root = journal.path();
    // The record moves to beta's file, its window stays alpha's.
    let record = fs::read_to_string(root.join("facets/alpha/activities/20260308.jsonl")).unwrap();
    fs::remove_file(root.join("facets/alpha/activities/20260308.jsonl")).unwrap();
    fs::create_dir_all(root.join("chronicle/20260308/phone/070000_300")).unwrap();
    write_activity_line(
        root,
        "beta",
        "20260308",
        "act_beta_run",
        "beta capture",
        50,
        Some("running"),
        Some(&["070000_300"]),
        None,
        None,
        None,
        None,
        None,
    );
    let beta = root.join("facets/beta/activities/20260308.jsonl");
    let mut text = fs::read_to_string(&beta).unwrap();
    text.push_str(&record);
    fs::write(&beta, text).unwrap();
    let beta_reader = ReadPermission {
        categories: vec!["facets".to_owned(), "transcripts".to_owned()],
        scope: ReadScope::Facets {
            ids: vec![FACET_B.to_owned()],
        },
    };
    let res = listed(&journal, Some(beta_reader));
    let capture = res["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| activity["title"] == "beta capture")
        .unwrap_or_else(|| panic!("{res}"))
        .clone();
    assert!(capture.get("beside").is_none(), "{res}");
    // The whole journal reads the window, and the line is there.
    let res = listed(&journal, Some(ReadPermission::default_whole_journal()));
    assert!(res.to_string().contains("\"kind\":\"workout\""), "{res}");
}

#[test]
fn a_hidden_capture_sits_beside_no_body_item_and_a_hidden_group_isnt_listed() {
    let journal = body_record_journal(true);
    let root = journal.path();
    fs::create_dir_all(root.join("chronicle/20260308/phone/070000_300")).unwrap();
    write_activity_line(
        root,
        "alpha",
        "20260308",
        "act_hidden_run",
        "hidden capture",
        50,
        Some("running"),
        Some(&["070000_300"]),
        None,
        None,
        None,
        Some(true),
        None,
    );
    let res = listed(&journal, Some(ReadPermission::default_whole_journal()));
    assert!(!res.to_string().contains("hidden capture"), "{res}");
    assert!(shows_body(&journal));
    // The same run from an app joins its group; the owner hides only the
    // app's account, which hides the group: no connection lists it.
    file_workout(
        root,
        "20260308",
        "x2",
        "Synthetic App",
        "2026-03-08 07:01:00 -0400",
        "2026-03-08 07:41:00 -0400",
    );
    assert!(shows_body(&journal));
    let path = root.join("facets/alpha/activities/20260308.jsonl");
    let text = fs::read_to_string(&path).unwrap().replace(
        "\"id\":\"body_00000000000000x2\",",
        "\"id\":\"body_00000000000000x2\",\"hidden\":true,",
    );
    assert!(text.contains("\"hidden\":true"), "{text}");
    fs::write(&path, text).unwrap();
    assert!(!shows_body(&journal));
}

#[test]
fn a_reference_reaches_its_face_after_another_member_on_another_day_became_its_anchor() {
    let journal = body_record_journal(true);
    let root = journal.path();
    PermissionStore::open(root)
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    // A late run past midnight from the watch, and the same run from an app
    // that started after midnight, so filed the next day.
    file_workout(
        root,
        "20260308",
        "lw",
        "Synthetic Watch",
        "2026-03-08 23:50:00 -0400",
        "2026-03-09 01:00:00 -0400",
    );
    file_workout(
        root,
        "20260309",
        "la",
        "Synthetic App",
        "2026-03-09 00:01:00 -0400",
        "2026-03-09 01:00:00 -0400",
    );
    let res = probe(
        &journal,
        "list_activities",
        json!({"from": "20260308", "to": "20260314"}),
    )
    .unwrap();
    let late = res["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|activity| {
            activity["source"] == "body" && activity["start"] == "2026-03-08 23:50:00 -0400"
        })
        .unwrap_or_else(|| panic!("{res}"))
        .clone();
    assert_eq!(late["accounts"].as_array().unwrap().len(), 2, "{late}");
    // The owner titles the app's account: it's the anchor now, on the next day.
    let mut patch = serde_json::Map::new();
    patch.insert("title".to_owned(), json!("Hill repeats"));
    solstone_core_facets::update_activity_record(
        root,
        "alpha",
        "20260309",
        "body_00000000000000la",
        &patch,
        "owner:web",
        "",
        "2026-03-10T00:00:00Z",
    )
    .unwrap()
    .expect("titled");
    let fetched = probe(&journal, "fetch", json!({"reference": late["reference"]})).unwrap();
    assert!(
        fetched["text"].as_str().unwrap().contains("Hill repeats"),
        "{fetched}"
    );
}

/// Grow a filed workout's account in its window, as a later export does, and
/// bring its record along as the importer's refresh does.
fn grow_workout(
    root: &Path,
    day: &str,
    row_id: &str,
    by: &str,
    start: &str,
    end: &str,
    refresh: bool,
) {
    use solstone_core_format::content::body_records::{decode_rows, encode_rows, recorder_id};
    let hour = &start[11..13];
    let rows = root.join(format!(
        "chronicle/{day}/import.apple_health/{hour}0000_3600/body.jsonl"
    ));
    let mut held = decode_rows(&fs::read_to_string(&rows).unwrap()).unwrap();
    for row in &mut held {
        if row.id == row_id {
            row.end = Some(end.to_owned());
        }
    }
    fs::write(&rows, encode_rows(&held)).unwrap();
    if refresh {
        let outcome = solstone_core_facets::refresh_body_account(
            root,
            "alpha",
            day,
            &format!("body_{:0>16}", row_id),
            "workout",
            start,
            end,
            &recorder_id("import.apple_health", Some(by)),
            Some("running"),
        )
        .unwrap();
        assert_eq!(outcome, solstone_core_facets::BodyAccountRefresh::Refreshed);
    }
}

/// How many accounts each body face holds, in each reader.
fn body_accounts_in_every_reader(
    journal: &tempfile::TempDir,
    day: &str,
) -> Vec<(&'static str, Vec<usize>)> {
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, ReadPermission::default_whole_journal())
        .unwrap();
    let listed = probe(journal, "list_activities", json!({"from": day, "to": day})).unwrap();
    let bodies: Vec<&Value> = listed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|activity| activity["source"] == "body")
        .collect();
    let counts = |value: &Value| value["accounts"].as_array().map_or(0, Vec::len);
    let listed_counts = bodies.iter().map(|activity| counts(activity)).collect();
    let mut sourced = Vec::new();
    let mut fetched = Vec::new();
    for activity in &bodies {
        let reference = activity["reference"].clone();
        let sources = probe(
            journal,
            "get_activity_sources",
            json!({"reference": reference}),
        )
        .unwrap();
        // Sources names a body record's accounts as its units.
        sourced.push(sources["units"].as_array().map_or(0, Vec::len));
        let text = probe(journal, "fetch", json!({"reference": reference})).unwrap();
        let face = serde_json::from_str::<Value>(text["text"].as_str().unwrap()).unwrap();
        fetched.push(counts(&face));
    }
    // The week the day is in: its Sunday.
    let week = probe(journal, "get_week", json!({"week": "20260308"})).unwrap();
    assert!(("20260308"..="20260314").contains(&day));
    let weekly = week["body"]
        .as_array()
        .map(|items| items.iter().map(counts).collect())
        .unwrap_or_default();
    vec![
        ("list_activities", listed_counts),
        ("get_activity_sources", sourced),
        ("fetch", fetched),
        ("get_week", weekly),
    ]
}

#[test]
fn a_workout_that_grows_into_another_recorders_groups_with_it_once_its_record_follows() {
    for refresh in [false, true] {
        let journal = body_record_journal(false);
        let root = journal.path();
        fs::create_dir_all(root.join("imports")).unwrap();
        fs::write(
            root.join("imports/.apple_health-zone.json"),
            r#"{"zone":"America/New_York"}"#,
        )
        .unwrap();
        // 20 minutes beside 40: all of the shorter but half the longer, so apart.
        file_workout(
            root,
            "20260310",
            "g1",
            "Synthetic Watch",
            "2026-03-10 07:00:00 -0400",
            "2026-03-10 07:20:00 -0400",
        );
        file_workout(
            root,
            "20260310",
            "g2",
            "Synthetic App",
            "2026-03-10 07:00:00 -0400",
            "2026-03-10 07:40:00 -0400",
        );
        // A later export ends the watch's at 07:40.
        grow_workout(
            root,
            "20260310",
            "g1",
            "Synthetic Watch",
            "2026-03-10 07:00:00 -0400",
            "2026-03-10 07:40:00 -0400",
            refresh,
        );
        let readers = body_accounts_in_every_reader(&journal, "20260310");
        for (reader, counts) in readers {
            let expected = if refresh { vec![2] } else { vec![1, 1] };
            assert_eq!(counts, expected, "{reader}, refresh {refresh}");
        }
    }
}

/// A synthetic night's account, its window and its record, filed under `day`,
/// with `extra` on the record.
fn file_night(
    root: &Path,
    day: &str,
    row_id: &str,
    by: &str,
    (start, end): (&str, &str),
    extra: Value,
) {
    use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows, recorder_id};
    let mut row = BodyRow::new(
        RowKind::Experience,
        row_id.to_owned(),
        "apple_health",
        "HKCategoryTypeIdentifierSleepAnalysis",
        "Sleep".to_owned(),
    );
    row.experience = Some("sleep".to_owned());
    row.start = Some(start.to_owned());
    row.end = Some(end.to_owned());
    row.by = Some(by.to_owned());
    let window = root.join(format!(
        "chronicle/{day}/import.apple_health/{}0000_3600",
        &start[11..13]
    ));
    fs::create_dir_all(window.join("talents")).unwrap();
    let rows = window.join("body.jsonl");
    let mut held = fs::read_to_string(&rows)
        .ok()
        .map(|text| solstone_core_format::content::body_records::decode_rows(&text).unwrap())
        .unwrap_or_default();
    held.retain(|stored| stored.id != row_id);
    held.push(row);
    fs::write(&rows, encode_rows(&held)).unwrap();
    fs::write(window.join("stream.json"), "{}\n").unwrap();
    fs::write(window.join("talents/facets.json"), r#"[{"facet":"alpha"}]"#).unwrap();
    let mut record = json!({
        "id": format!("body_{:0>16}", row_id), "source": "body", "activity": "sleep",
        "account": {"key": format!("import.apple_health\u{1f}{row_id}"), "start": start,
            "end": end, "recorder": recorder_id("import.apple_health", Some(by))},
    });
    for (key, value) in extra.as_object().unwrap() {
        record[key] = value.clone();
    }
    let path = root.join(format!("facets/alpha/activities/{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{record}\n"));
    fs::write(&path, text).unwrap();
}

#[test]
fn a_night_that_grows_into_another_recorders_keeps_its_hide_and_apart_in_every_reader() {
    for kept in ["plain", "hidden", "apart"] {
        let journal = body_record_journal(false);
        let root = journal.path();
        fs::create_dir_all(root.join("imports")).unwrap();
        fs::write(
            root.join("imports/.apple_health-zone.json"),
            r#"{"zone":"America/New_York"}"#,
        )
        .unwrap();
        let start = "2026-03-10 22:10:00 -0400";
        let extra = match kept {
            "hidden" => json!({"hidden": true}),
            "apart" => json!({"apart": ["import.apple_health\u{1f}p1"]}),
            _ => json!({}),
        };
        // The watch's night ends at 02:00: all of it inside the phone's, but
        // under half of the phone's, so apart.
        file_night(
            root,
            "20260310",
            "n1",
            "Synthetic Watch",
            (start, "2026-03-11 02:00:00 -0400"),
            extra,
        );
        file_night(
            root,
            "20260310",
            "p1",
            "Synthetic Phone",
            ("2026-03-10 22:12:00 -0400", "2026-03-11 06:18:00 -0400"),
            json!({}),
        );
        // Before it grows, each night stands alone, and the hidden one shows nowhere.
        for (reader, counts) in body_accounts_in_every_reader(&journal, "20260310") {
            let expected: Vec<usize> = match kept {
                "hidden" => vec![1],
                _ => vec![1, 1],
            };
            assert_eq!(counts, expected, "{reader}, {kept} before");
        }
        // A later export ends the watch's night at 06:20, and its record follows.
        let end = "2026-03-11 06:20:00 -0400";
        let rows = root.join("chronicle/20260310/import.apple_health/220000_3600/body.jsonl");
        let mut held = solstone_core_format::content::body_records::decode_rows(
            &fs::read_to_string(&rows).unwrap(),
        )
        .unwrap();
        for row in &mut held {
            if row.id == "n1" {
                row.end = Some(end.to_owned());
            }
        }
        fs::write(
            &rows,
            solstone_core_format::content::body_records::encode_rows(&held),
        )
        .unwrap();
        let outcome = solstone_core_facets::refresh_body_account(
            root,
            "alpha",
            "20260310",
            "body_00000000000000n1",
            "sleep",
            start,
            end,
            &solstone_core_format::content::body_records::recorder_id(
                "import.apple_health",
                Some("Synthetic Watch"),
            ),
            None,
        )
        .unwrap();
        assert_eq!(outcome, solstone_core_facets::BodyAccountRefresh::Refreshed);
        // A hidden group shows in no listing, so neither fetch nor sources is
        // handed a reference to it.
        for (reader, counts) in body_accounts_in_every_reader(&journal, "20260310") {
            let expected: Vec<usize> = match kept {
                "plain" => vec![2],
                // Grouped now, the phone's night is hidden with the watch's.
                "hidden" => Vec::new(),
                _ => vec![1, 1],
            };
            assert_eq!(counts, expected, "{reader}, {kept}");
        }
    }
}

#[test]
fn a_workout_that_grows_past_its_group_stands_alone_once_its_record_follows() {
    let journal = body_record_journal(false);
    let root = journal.path();
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.apple_health-zone.json"),
        r#"{"zone":"America/New_York"}"#,
    )
    .unwrap();
    file_workout(
        root,
        "20260310",
        "g1",
        "Synthetic Watch",
        "2026-03-10 07:00:00 -0400",
        "2026-03-10 07:40:00 -0400",
    );
    file_workout(
        root,
        "20260310",
        "g2",
        "Synthetic App",
        "2026-03-10 07:00:00 -0400",
        "2026-03-10 07:40:00 -0400",
    );
    for (reader, counts) in body_accounts_in_every_reader(&journal, "20260310") {
        assert_eq!(counts, vec![2], "{reader} before");
    }
    // The watch's account now runs to 09:00: a third of the longer.
    grow_workout(
        root,
        "20260310",
        "g1",
        "Synthetic Watch",
        "2026-03-10 07:00:00 -0400",
        "2026-03-10 09:00:00 -0400",
        true,
    );
    for (reader, counts) in body_accounts_in_every_reader(&journal, "20260310") {
        assert_eq!(counts, vec![1, 1], "{reader} after");
    }
}

/// Oura's walk, its meditation and its night on 20260308, each in an
/// alpha-filed window of `import.oura` with its record, holding what Oura
/// judges in `fields`.
fn oura_record_journal() -> tempfile::TempDir {
    use solstone_core_format::content::body_records::{
        BodyRow, RowKind, encode_rows, mapped_workout_type, recorder_id,
    };
    let journal = base_activity_journal();
    let root = journal.path();
    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.oura-zone.json"),
        r#"{"zone":"America/New_York"}"#,
    )
    .unwrap();
    let accounts = [
        (
            "ow",
            "070000_300",
            "workout",
            "walking",
            "walking",
            (
                "2026-03-08T07:00:00.000-04:00",
                "2026-03-08T07:36:00.000-04:00",
            ),
            json!({"activity": "walking", "calories": 148.5, "distance": 2412.9,
                "intensity": "hard", "source": "confirmed"}),
        ),
        (
            "om",
            "120000_300",
            "mindfulness",
            "meditation",
            "meditation session",
            (
                "2026-03-08T12:00:00.000-04:00",
                "2026-03-08T12:10:00.000-04:00",
            ),
            json!({"type": "meditation", "mood": "bad"}),
        ),
        (
            "on",
            "220000_3600",
            "sleep",
            "long_sleep",
            "long sleep",
            (
                "2026-03-08T22:30:00.000-04:00",
                "2026-03-09T06:30:00.000-04:00",
            ),
            json!({"type": "long_sleep", "efficiency": 91, "readiness": {"score": 82},
                "sleep_score_delta": 3}),
        ),
    ];
    for (id, key, experience, record_type, label, (start, end), document) in accounts {
        let mut row = BodyRow::new(
            RowKind::Experience,
            id.to_owned(),
            "oura",
            record_type,
            label.to_owned(),
        );
        row.experience = Some(experience.to_owned());
        row.start = Some(start.to_owned());
        row.end = Some(end.to_owned());
        row.by = Some("Oura".to_owned());
        row.fields = document
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        if experience == "workout" {
            row.values
                .insert("distance".to_owned(), "2412.9 m".to_owned());
            row.values
                .insert("energy".to_owned(), "148.5 kcal".to_owned());
        }
        let window = root.join("chronicle/20260308/import.oura").join(key);
        fs::create_dir_all(window.join("talents")).unwrap();
        fs::write(
            window.join("body.jsonl"),
            encode_rows(std::slice::from_ref(&row)),
        )
        .unwrap();
        fs::write(window.join("stream.json"), "{}\n").unwrap();
        fs::write(window.join("talents/facets.json"), r#"[{"facet":"alpha"}]"#).unwrap();
        let mut account = json!({"key": format!("import.oura\u{1f}{id}"), "start": start,
            "end": end, "recorder": recorder_id("import.oura", Some("Oura"))});
        if let Some(kind) =
            mapped_workout_type("oura", record_type).filter(|_| experience == "workout")
        {
            account["type"] = json!(kind);
        }
        let record = json!({"id": format!("body_{id:0>16}"), "source": "body",
            "activity": experience, "account": account});
        let path = root.join("facets/alpha/activities/20260308.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut text = fs::read_to_string(&path).unwrap_or_default();
        text.push_str(&format!("{record}\n"));
        fs::write(&path, text).unwrap();
    }
    journal
}

/// A reader's answer with every reference taken out, since a reference is
/// minted per call and holds arbitrary text.
fn without_references(mut value: Value) -> Value {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.remove("reference");
                map.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut value);
    value
}

/// Every record reader shows Oura's accounts as Oura recorded them, with the
/// workout's chosen values, and none of what Oura judges: no intensity, mood,
/// score or efficiency, by name or by value.
#[test]
fn every_record_reader_shows_an_oura_account_and_nothing_oura_judges() {
    let journal = oura_record_journal();
    let listed = probe(
        &journal,
        "list_activities",
        json!({"from": "20260308", "to": "20260308"}),
    )
    .unwrap();
    let bodies: Vec<Value> = listed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|activity| activity["source"] == "body")
        .cloned()
        .collect();
    let mut kinds: Vec<&str> = bodies
        .iter()
        .map(|activity| activity["kind"].as_str().unwrap())
        .collect();
    kinds.sort_unstable();
    assert_eq!(kinds, ["mindfulness", "sleep", "workout"], "{listed}");
    let walk = bodies
        .iter()
        .find(|activity| activity["kind"] == "workout")
        .unwrap();
    assert_eq!(walk["title"], "walking", "{walk}");
    assert_eq!(walk["accounts"][0]["by"], "Oura", "{walk}");
    assert_eq!(
        walk["accounts"][0]["values"]["distance"], "2412.9 m",
        "{walk}"
    );

    let mut answers = vec![("list_activities", without_references(listed.clone()))];
    for activity in &bodies {
        let reference = activity["reference"].clone();
        let sources = probe(
            &journal,
            "get_activity_sources",
            json!({"reference": reference}),
        )
        .unwrap();
        assert_eq!(sources["units"][0]["by"], "Oura", "{sources}");
        answers.push(("get_activity_sources", without_references(sources)));
        let fetched = probe(&journal, "fetch", json!({"reference": reference})).unwrap();
        let face = serde_json::from_str::<Value>(fetched["text"].as_str().unwrap()).unwrap();
        assert_eq!(face["accounts"][0]["by"], "Oura", "{face}");
        answers.push(("fetch", without_references(face)));
    }
    let week = probe(&journal, "get_week", json!({"week": "20260308"})).unwrap();
    let weekly = week["body"].as_array().unwrap();
    assert_eq!(weekly.len(), 3, "{week}");
    assert!(
        weekly
            .iter()
            .all(|item| item["accounts"][0]["by"] == "Oura"),
        "{week}"
    );
    answers.push(("get_week", without_references(week)));

    for (reader, answer) in answers {
        let text = answer.to_string();
        for judged in [
            "intensity",
            "\"hard\"",
            "mood",
            "\"bad\"",
            "score",
            "efficiency",
            "readiness",
            "import.oura",
        ] {
            assert!(!text.contains(judged), "{reader} says {judged}: {text}");
        }
    }
}

fn rescan(journal: &tempfile::TempDir, rel: &str) {
    solstone_core_indexer_store::scan::rescan_file(journal.path(), Path::new(rel)).unwrap();
}

#[test]
fn a_hidden_activity_between_two_leaves_its_neighbours_and_the_body_entry_found() {
    let journal = body_record_journal(true);
    grant_whole_journal(journal.path());
    hold_body_title(&journal, "Zebrafinch loop", json!({}));
    let rel = "facets/alpha/activities/20260308.jsonl";
    let path = journal.path().join(rel);
    let held = fs::read_to_string(&path).unwrap();
    let day = |bravo_hidden: bool| {
        let record = |id: &str, title: &str, hidden: bool| {
            json!({"id": id, "title": title, "activity": "review", "created_at": 1,
                "hidden": hidden})
        };
        fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n{held}",
                record("a", "Alphaneedle", false),
                record("b", "Bravoneedle", bravo_hidden),
                record("c", "Charlieneedle", false),
            ),
        )
        .unwrap();
        rescan(&journal, rel);
    };
    day(false);
    let bravo = activity_hits_for(&journal, "Bravoneedle");
    assert_eq!(bravo.len(), 1, "{bravo:?}");
    let bravo_reference = bravo[0]["reference"].clone();

    day(true);
    assert!(activity_hits_for(&journal, "Bravoneedle").is_empty());
    assert_eq!(
        probe(&journal, "fetch", json!({"reference": bravo_reference})),
        Err(McpProbeError::Unavailable)
    );
    let charlie = activity_hits_for(&journal, "Charlieneedle");
    assert_eq!(charlie.len(), 1, "{charlie:?}");
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": charlie[0]["reference"]}),
    )
    .unwrap();
    let record: Value = serde_json::from_str(fetched["text"].as_str().unwrap()).unwrap();
    assert_eq!(record["id"], "c");
    let body = activity_hits_for(&journal, "Zebrafinch");
    assert_eq!(body.len(), 1, "{body:?}");
    let face = probe(
        &journal,
        "fetch",
        json!({"reference": body[0]["reference"]}),
    )
    .unwrap();
    assert!(
        face["text"].as_str().unwrap().contains("Synthetic Watch"),
        "{face}"
    );
}

/// Alpha holds a person, Bob, linked and noted, and a day of detections
/// naming him beside Alice.
fn person_journal() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("hidden-person-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let root = journal.path();
    let write = |rel: &str, text: String| {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    for (id, name, note) in [
        ("bob", "Bob Smith", "bobnoteneedle"),
        ("ann", "Ann Lee", "annnoteneedle"),
    ] {
        write(
            &format!("entities/{id}/entity.json"),
            json!({"id": id, "name": name}).to_string(),
        );
        write(
            &format!("facets/alpha/entities/{id}/entity.json"),
            json!({"entity_id": id}).to_string(),
        );
        write(
            &format!("facets/alpha/entities/{id}/observations.jsonl"),
            format!(
                "{}\n",
                json!({"content": note, "observed_at": 1772658000000_i64, "source_day": "20260304"})
            ),
        );
    }
    write(
        "facets/alpha/entities/20260304.jsonl",
        format!(
            "{}\n{}\n",
            json!({"type": "Person", "name": "Bob Smith", "description": "bobdetectedneedle"}),
            json!({"type": "Person", "name": "Alice Jones", "description": "alicedetectedneedle"}),
        ),
    );
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    grant_whole_journal(root);
    journal
}

#[test]
fn a_hidden_person_leaves_search_and_fetch_with_their_notes_and_detections() {
    let journal = person_journal();
    let notes = activity_hits_for(&journal, "bobnoteneedle");
    let detected = activity_hits_for(&journal, "bobdetectedneedle");
    assert_eq!((notes.len(), detected.len()), (1, 1));

    let root = journal.path();
    fs::write(
        root.join("entities/bob/entity.json"),
        json!({"id": "bob", "name": "Bob Smith", "blocked": true}).to_string(),
    )
    .unwrap();
    fs::write(
        root.join("facets/alpha/entities/bob/entity.json"),
        json!({"entity_id": "bob", "detached": true, "detached_by": "hide"}).to_string(),
    )
    .unwrap();
    rescan(&journal, "facets/alpha/entities/bob/observations.jsonl");
    rescan(&journal, "facets/alpha/entities/20260304.jsonl");
    for (word, before) in [("bobnoteneedle", &notes), ("bobdetectedneedle", &detected)] {
        assert!(activity_hits_for(&journal, word).is_empty(), "{word}");
        assert_eq!(
            probe(
                &journal,
                "fetch",
                json!({"reference": before[0]["reference"]})
            ),
            Err(McpProbeError::Unavailable),
            "{word}"
        );
    }
    let listed = probe(&journal, "list_entities", json!({})).unwrap();
    assert!(!listed.to_string().contains("Bob Smith"), "{listed}");
    // The visible people beside him stay.
    assert_eq!(activity_hits_for(&journal, "annnoteneedle").len(), 1);
    assert_eq!(activity_hits_for(&journal, "alicedetectedneedle").len(), 1);
    assert!(listed.to_string().contains("Ann Lee"), "{listed}");
}

#[test]
fn a_name_hidden_in_a_facet_leaves_search_and_fetch_and_comes_back_unhidden() {
    let journal = person_journal();
    let detected = activity_hits_for(&journal, "bobdetectedneedle");
    assert_eq!(detected.len(), 1);
    let day = "facets/alpha/entities/20260304.jsonl";
    let hide = |names: Value| {
        fs::write(
            journal.path().join("facets/alpha/hidden-names.json"),
            json!({ "names": names }).to_string(),
        )
        .unwrap();
        rescan(&journal, day);
    };
    hide(json!([{"name": "Bob Smith", "hidden_at": "2026-11-11T07:00:00Z"}]));
    assert!(activity_hits_for(&journal, "bobdetectedneedle").is_empty());
    assert_eq!(
        probe(
            &journal,
            "fetch",
            json!({"reference": detected[0]["reference"]})
        ),
        Err(McpProbeError::Unavailable)
    );
    let alice = activity_hits_for(&journal, "alicedetectedneedle");
    assert_eq!(alice.len(), 1);
    assert!(
        probe(
            &journal,
            "fetch",
            json!({"reference": alice[0]["reference"]})
        )
        .is_ok()
    );
    // His notes are the person's, not the name's.
    assert_eq!(activity_hits_for(&journal, "bobnoteneedle").len(), 1);

    hide(json!([]));
    let back = activity_hits_for(&journal, "bobdetectedneedle");
    assert_eq!(back.len(), 1);
    assert!(
        probe(
            &journal,
            "fetch",
            json!({"reference": back[0]["reference"]})
        )
        .is_ok()
    );
}

#[test]
fn a_week_page_with_something_left_out_leaves_search_and_fetch() {
    let journal = tempfile::Builder::new()
        .prefix("week-left-out-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let root = journal.path();
    let weekly = root.join("reflections/weekly");
    fs::create_dir_all(&weekly).unwrap();
    fs::write(
        weekly.join("20260301.md"),
        "# Your week\n\nkestrelweekneedle came back on monday.\n",
    )
    .unwrap();
    fs::write(
        weekly.join("20260301.json"),
        json!({"memories": [{"key": "m1", "day": "20260302", "text": "kestrelweekneedle"}],
            "said": []})
        .to_string(),
    )
    .unwrap();
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    grant_whole_journal(root);
    let page = "reflections/weekly/20260301.md";
    let hits = activity_hits_for(&journal, "kestrelweekneedle");
    assert_eq!(hits.len(), 1);
    // Before anything is left out, the page fetches.
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": hits[0]["reference"]}),
    )
    .unwrap();
    assert!(
        fetched["text"]
            .as_str()
            .unwrap()
            .contains("kestrelweekneedle")
    );

    fs::create_dir_all(root.join("health")).unwrap();
    let leave_out = |keys: Value| {
        fs::write(
            root.join("health/week-left-out.json"),
            json!({ "keys": keys }).to_string(),
        )
        .unwrap();
        rescan(&journal, page);
    };
    leave_out(json!(["m1"]));
    assert!(activity_hits_for(&journal, "kestrelweekneedle").is_empty());
    assert_eq!(
        probe(
            &journal,
            "fetch",
            json!({"reference": hits[0]["reference"]})
        ),
        Err(McpProbeError::Unavailable)
    );

    leave_out(json!([]));
    let back = activity_hits_for(&journal, "kestrelweekneedle");
    assert_eq!(back.len(), 1);
    assert!(
        probe(
            &journal,
            "fetch",
            json!({"reference": back[0]["reference"]})
        )
        .is_ok()
    );
}

// ---- a day's list read in one go, one source at a time, one row per moment ----

fn span_journal(scope: Option<ReadScope>) -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("span-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    setup_facets(journal.path());
    let permission = match scope {
        Some(scope) => ReadPermission {
            categories: vec!["transcripts".to_owned(), "messages".to_owned()],
            scope,
        },
        None => legacy_markdown_whole_journal(),
    };
    PermissionStore::open(journal.path())
        .set_permission(CONNECTION, permission)
        .unwrap();
    journal
}

fn write_span_segment(journal: &Path, stream: &str, name: &str, facet: &str, text: &str) {
    let seg = journal.join("chronicle/20261010").join(stream).join(name);
    fs::create_dir_all(seg.join("talents")).unwrap();
    fs::write(
        seg.join("talents/facets.json"),
        format!("[{{\"facet\":\"{facet}\"}}]"),
    )
    .unwrap();
    fs::write(seg.join("meeting_transcript.md"), text).unwrap();
}

/// Every page of a span read, in order: (time, source, joined entries).
fn read_whole_span(journal: &tempfile::TempDir, span: &Value) -> (Vec<(String, String)>, usize) {
    let mut sections = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut arguments = json!({"reference": span});
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = probe(journal, "get_transcript", arguments).unwrap();
        pages += 1;
        for section in page["segments"].as_array().unwrap() {
            let text = section["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry.as_str().unwrap())
                .collect::<Vec<_>>()
                .join("\n");
            sections.push((section["time"].as_str().unwrap().to_owned(), text));
        }
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
        assert!(pages < 200, "a span read must end");
    }
    (sections, pages)
}

#[test]
fn a_days_list_is_read_in_one_go_through_its_span_reference() {
    let journal = span_journal(None);
    for (name, text) in [
        ("110000_300", "eleven words\n"),
        ("090000_300", "nine words\n"),
        ("100000_300", "ten words\n"),
    ] {
        write_span_segment(journal.path(), "default", name, "alpha", text);
    }
    // A segment with nothing said: listed days often hold many.
    fs::create_dir_all(journal.path().join("chronicle/20261010/default/093000_300")).unwrap();

    let list = probe(&journal, "list_transcripts", json!({"day": "20261010"})).unwrap();
    let said: Vec<&Value> = list["transcripts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["lines"] == 1)
        .collect();
    assert_eq!(said.len(), 3, "{list}");
    assert!(said.iter().all(|row| row["seconds"] == 300), "{list}");
    assert!(list["gaps"].is_array(), "{list}");

    let (sections, pages) = read_whole_span(&journal, &list["span_reference"]);
    assert_eq!(pages, 1);
    let times: Vec<&str> = sections.iter().map(|(time, _)| time.as_str()).collect();
    assert_eq!(times, ["09:00-09:05", "10:00-10:05", "11:00-11:05"]);
    assert!(sections[0].1.contains("nine words"));
    assert!(sections[2].1.contains("eleven words"));

    // The span is what the list matched: its time range comes with it.
    let narrowed = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "time_from": "09:30", "time_to": "10:30"}),
    )
    .unwrap();
    let (sections, _) = read_whole_span(&journal, &narrowed["span_reference"]);
    let times: Vec<&str> = sections.iter().map(|(time, _)| time.as_str()).collect();
    assert_eq!(times, ["10:00-10:05"]);
    assert_eq!(
        narrowed["gaps"],
        json!([{"from": "09:30", "to": "10:00"}, {"from": "10:05", "to": "10:30"}]),
        "{narrowed}"
    );

    // A list without a day names no span: there is no one day to read.
    let all = probe(&journal, "list_transcripts", json!({})).unwrap();
    assert!(all["span_reference"].is_null(), "{all}");
    assert!(all["gaps"].is_null(), "{all}");
}

#[test]
fn a_span_read_pages_through_long_segments_without_losing_or_repeating_a_line() {
    let journal = span_journal(None);
    let long = |tag: &str| {
        (0..260)
            .map(|line| format!("{tag} line {line:03} {}\n\n", "word ".repeat(60)))
            .collect::<String>()
    };
    write_span_segment(
        journal.path(),
        "default",
        "090000_300",
        "alpha",
        &long("first"),
    );
    write_span_segment(
        journal.path(),
        "default",
        "100000_300",
        "alpha",
        &long("second"),
    );
    write_span_segment(
        journal.path(),
        "default",
        "110000_300",
        "alpha",
        "the end\n",
    );

    let list = probe(&journal, "list_transcripts", json!({"day": "20261010"})).unwrap();
    let (sections, pages) = read_whole_span(&journal, &list["span_reference"]);
    assert!(pages > 1, "long segments take more than one page");
    let whole = sections
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut at = 0;
    for tag in ["first", "second"] {
        for line in 0..260 {
            let marker = format!("{tag} line {line:03} ");
            assert_eq!(whole.matches(&marker).count(), 1, "{marker}");
            let found = whole.find(&marker).unwrap();
            assert!(found >= at, "{marker} is out of order");
            at = found;
        }
    }
    assert!(whole.ends_with("the end"), "the last segment is read last");

    // A cursor belongs to its span.
    let other = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "time_from": "09:00", "time_to": "10:00"}),
    )
    .unwrap();
    let first = probe(
        &journal,
        "get_transcript",
        json!({"reference": list["span_reference"]}),
    )
    .unwrap();
    assert_eq!(
        probe(
            &journal,
            "get_transcript",
            json!({"reference": other["span_reference"], "cursor": first["next_cursor"]}),
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn a_source_label_narrows_a_days_list_and_its_span() {
    let journal = span_journal(None);
    write_span_segment(
        journal.path(),
        "default",
        "090000_300",
        "alpha",
        "nine words\n",
    );
    write_span_segment(
        journal.path(),
        "default",
        "100000_300",
        "alpha",
        "ten words\n",
    );

    let list = probe(&journal, "list_transcripts", json!({"day": "20261010"})).unwrap();
    let label = list["transcripts"][0]["source"]
        .as_str()
        .unwrap()
        .to_owned();

    // The label a row shows is the label the list takes.
    let same = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "source": label}),
    )
    .unwrap();
    assert_eq!(same["transcripts"].as_array().unwrap().len(), 2, "{same}");
    assert!(same["other_sources"].is_null(), "{same}");
    let (sections, _) = read_whole_span(&journal, &same["span_reference"]);
    assert_eq!(sections.len(), 2);

    // Paging a narrowed list keeps it narrowed.
    let first = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "source": label, "limit": 1}),
    )
    .unwrap();
    let second = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "source": label, "limit": 1, "cursor": first["next_cursor"]}),
    )
    .unwrap();
    assert_eq!(second["transcripts"][0]["time"], "10:00-10:05", "{second}");
    assert_eq!(
        probe(
            &journal,
            "list_transcripts",
            json!({"day": "20261010", "limit": 1, "cursor": first["next_cursor"]}),
        ),
        Err(McpProbeError::Unavailable)
    );

    // A label no row shows lists nothing, and names no span.
    let none = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "source": "a label nothing carries"}),
    )
    .unwrap();
    assert!(none["transcripts"].as_array().unwrap().is_empty(), "{none}");
    assert!(none["span_reference"].is_null(), "{none}");

    // The same answer whatever else is asked of it: a label can't be used
    // to ask what the journal holds.
    let none_with_kind = probe(
        &journal,
        "list_transcripts",
        json!({"day": "20261010", "source": "a label nothing carries", "kind": "sleep"}),
    )
    .unwrap();
    assert_eq!(none, none_with_kind);
    assert!(
        none["note"]
            .as_str()
            .unwrap()
            .starts_with("No source with that label"),
        "{none}"
    );

    // A kind belongs to a body source's entries.
    assert_eq!(
        probe(
            &journal,
            "list_transcripts",
            json!({"day": "20261010", "source": label, "kind": "sleep"}),
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn a_span_read_serves_only_what_the_connection_may_read_when_it_is_read() {
    let journal = span_journal(Some(ReadScope::Facets {
        ids: vec![FACET_A.to_owned()],
    }));
    write_span_segment(
        journal.path(),
        "alpha",
        "090000_300",
        "alpha",
        "alpha nine\n",
    );
    write_span_segment(journal.path(), "beta", "100000_300", "beta", "beta ten\n");
    write_span_segment(
        journal.path(),
        "alpha",
        "110000_300",
        "alpha",
        "alpha eleven\n",
    );

    let list = probe(&journal, "list_transcripts", json!({"day": "20261010"})).unwrap();
    assert_eq!(list["transcripts"].as_array().unwrap().len(), 2, "{list}");
    let span = list["span_reference"].clone();
    let (sections, _) = read_whole_span(&journal, &span);
    let whole = format!("{sections:?}");
    assert!(whole.contains("alpha nine") && whole.contains("alpha eleven"));
    assert!(!whole.contains("beta ten"), "outside the scope: {whole}");
    // The stretch only the other facet covers is a gap to this connection.
    assert!(
        list["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["from"] == "09:05" && gap["to"] == "11:00"),
        "{list}"
    );

    // A segment that moves out of scope after the list is not read.
    fs::write(
        journal
            .path()
            .join("chronicle/20261010/alpha/110000_300/talents/facets.json"),
        "[{\"facet\":\"beta\"}]",
    )
    .unwrap();
    let (sections, _) = read_whole_span(&journal, &span);
    let whole = format!("{sections:?}");
    assert!(whole.contains("alpha nine"), "{whole}");
    assert!(!whole.contains("alpha eleven"), "{whole}");

    // A span is bound to the connection it was given to.
    PermissionStore::open(journal.path())
        .set_permission("bearer:someone-else", legacy_markdown_whole_journal())
        .unwrap();
    assert_eq!(
        run_mcp_probe(
            journal.path(),
            "bearer:someone-else",
            "get_transcript",
            &json!({"reference": span}),
        ),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn a_moment_in_two_facets_is_listed_once_with_both_facets_people() {
    let journal = base_activity_journal();
    let segments = ["150000_600"];
    // The same call, recorded in each facet it belongs to.
    for (facet, title, level, people) in [
        (
            "alpha",
            "Alpha's reading of the call",
            0.4,
            &[("Speaker 1", "attendee")][..],
        ),
        (
            "beta",
            "Beta's reading of the call",
            0.9,
            &[
                ("Benvolio", "attendee"),
                ("Speaker 1", "attendee"),
                ("Speaker 2", "heard"),
                ("Romeo", "mentioned"),
            ][..],
        ),
    ] {
        write_activity_line(
            journal.path(),
            facet,
            "20260310",
            "meeting_150000_600",
            title,
            100,
            Some("meeting"),
            Some(&segments),
            // Only the copy that isn't shown names Tybalt as an entity.
            Some(if facet == "beta" {
                &["Benvolio"]
            } else {
                &["Tybalt"]
            }),
            Some(people),
            Some("device_audio"),
            None,
            None,
        );
        let path = journal
            .path()
            .join(format!("facets/{facet}/activities/20260310.jsonl"));
        let line = fs::read_to_string(&path).unwrap();
        let mut record: Value = serde_json::from_str(line.trim()).unwrap();
        record["level_avg"] = json!(level);
        fs::write(&path, format!("{record}\n")).unwrap();
    }
    // The same segment cut differently is a separate reading, and stays one.
    write_activity_line(
        journal.path(),
        "alpha",
        "20260310",
        "writing_150000_300",
        "Notes while the call ran",
        200,
        Some("writing"),
        Some(&["150000_300"]),
        None,
        None,
        Some("device_screen"),
        None,
        None,
    );

    let list = probe(&journal, "list_activities", json!({"day": "20260310"})).unwrap();
    assert_no_leaked_keys(&list);
    let rows = list["activities"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{list}");
    let call = rows
        .iter()
        .find(|row| row["kind"] == "meeting")
        .unwrap_or_else(|| panic!("{list}"));
    // The facet it belongs to most, with the other beside it.
    assert_eq!(call["facet"], "Beta");
    assert_eq!(call["title"], "Beta's reading of the call");
    assert_eq!(call["also_in"], json!(["Alpha"]));
    // A voice with no name is counted, never a person.
    assert_eq!(call["participants"], json!(["Benvolio"]));
    assert_eq!(call["mentioned"], json!(["Romeo"]));
    assert_eq!(call["unnamed_voices"], 2);
    assert!(call["source"].is_string(), "{call}");
    let notes = rows.iter().find(|row| row["kind"] == "writing").unwrap();
    assert!(notes["also_in"].is_null(), "{notes}");
    assert!(notes["unnamed_voices"].is_null(), "{notes}");

    // The moment passes a person filter when either facet's copy names them,
    // and a facet's own list still shows its copy.
    let by_person = probe(
        &journal,
        "list_activities",
        json!({"day": "20260310", "entity": "Benvolio"}),
    )
    .unwrap();
    assert_eq!(by_person["activities"].as_array().unwrap().len(), 1);
    let by_other_copy = probe(
        &journal,
        "list_activities",
        json!({"day": "20260310", "entity": "Tybalt"}),
    )
    .unwrap();
    let rows = by_other_copy["activities"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{by_other_copy}");
    assert_eq!(rows[0]["facet"], "Beta");
    let alpha = probe(
        &journal,
        "list_activities",
        json!({"day": "20260310", "facet": FACET_A}),
    )
    .unwrap();
    let alpha_rows = alpha["activities"].as_array().unwrap();
    assert_eq!(alpha_rows.len(), 2, "{alpha}");
    assert!(
        alpha_rows
            .iter()
            .any(|row| row["title"] == "Alpha's reading of the call"),
        "{alpha}"
    );

    // Paging never shows the moment twice.
    let mut titles = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut arguments = json!({"day": "20260310", "limit": 1});
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = probe(&journal, "list_activities", arguments).unwrap();
        for row in page["activities"].as_array().unwrap() {
            titles.push(row["title"].as_str().unwrap().to_owned());
        }
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(titles.len(), 2, "{titles:?}");

    // The week counts it once too.
    let week = probe(&journal, "get_week", json!({"week": "20260310"})).unwrap();
    assert_eq!(week["activities_total"], 2, "{week}");
}

#[test]
fn list_activities_takes_the_range_names_search_takes_and_the_ones_it_took_first() {
    let journal = base_activity_journal();
    for (day, id) in [("20260310", "a"), ("20260311", "b"), ("20260312", "c")] {
        write_activity_line(
            journal.path(),
            "alpha",
            day,
            id,
            &format!("Day {day}"),
            100,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
    }
    let new = probe(
        &journal,
        "list_activities",
        json!({"day_from": "20260310", "day_to": "2026-03-11"}),
    )
    .unwrap();
    let old = probe(
        &journal,
        "list_activities",
        json!({"from": "20260310", "to": "2026-03-11"}),
    )
    .unwrap();
    let titles = |value: &Value| -> Vec<String> {
        value["activities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["title"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(titles(&new), ["Day 20260310", "Day 20260311"]);
    assert_eq!(titles(&new), titles(&old));
    assert_eq!(
        probe(
            &journal,
            "list_activities",
            json!({"day": "20260310", "day_to": "20260311"}),
        ),
        Err(McpProbeError::InvalidInput)
    );
}

#[test]
fn a_span_read_pages_only_on_what_the_connection_reads() {
    let journal = span_journal(Some(ReadScope::Facets {
        ids: vec![FACET_A.to_owned()],
    }));
    // More than one call's worth of text in scope, then a long stretch of
    // another facet's segments.
    let long = (0..260)
        .map(|line| format!("alpha line {line:03} {}\n\n", "word ".repeat(60)))
        .collect::<String>();
    write_span_segment(journal.path(), "alpha", "090000_300", "alpha", &long);
    for minute in 0..30 {
        write_span_segment(
            journal.path(),
            "beta",
            &format!("10{minute:02}00_60"),
            "beta",
            "beta words\n",
        );
    }
    let list = probe(&journal, "list_transcripts", json!({"day": "20261010"})).unwrap();
    assert_eq!(list["transcripts"].as_array().unwrap().len(), 1, "{list}");

    // Every page carries something this connection reads: what it can't read
    // earns no cursor and no empty page.
    let mut cursor: Option<String> = None;
    let mut lines = 0;
    loop {
        let mut arguments = json!({"reference": list["span_reference"]});
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = probe(&journal, "get_transcript", arguments).unwrap();
        let sections = page["segments"].as_array().unwrap();
        assert!(!sections.is_empty(), "an empty page: {page}");
        assert!(!page.to_string().contains("beta words"), "{page}");
        lines += sections
            .iter()
            .map(|section| section["entries"].as_array().unwrap().len())
            .sum::<usize>();
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert!(lines >= 260, "{lines}");

    // A segment the owner deletes after the list is not read.
    let whole = span_journal(None);
    write_span_segment(
        whole.path(),
        "default",
        "090000_300",
        "alpha",
        "kept words\n",
    );
    write_span_segment(
        whole.path(),
        "default",
        "100000_300",
        "alpha",
        "deleted words\n",
    );
    let list = probe(&whole, "list_transcripts", json!({"day": "20261010"})).unwrap();
    fs::remove_dir_all(whole.path().join("chronicle/20261010/default/100000_300")).unwrap();
    let (sections, _) = read_whole_span(&whole, &list["span_reference"]);
    let text = format!("{sections:?}");
    assert!(
        text.contains("kept words") && !text.contains("deleted words"),
        "{text}"
    );
}

#[test]
fn entity_photos_follow_state_and_the_source_activity_facet() {
    use solstone_core_entity::faceprints as fp;
    let journal = fixture();
    let root = journal.path();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        r#"{"faces":{"enabled":true}}"#,
    )
    .unwrap();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode_image(&image::RgbImage::from_pixel(
            256,
            400,
            image::Rgb([20, 40, 60]),
        ))
        .unwrap();
    let encoder = fp::Encoder {
        id: "synthetic".into(),
        sha256: "0".repeat(64),
        width: 128,
    };
    let faces: Vec<_> = (0..5)
        .map(|i| {
            let mut embedding = vec![0.; 128];
            embedding[0] = 0.8;
            embedding[i + 1] = 0.6;
            let key = fp::FaceKey {
                day: DAY.into(),
                stream: STREAM.into(),
                stream_layout: "named".into(),
                segment_key: format!("{i:06}_300"),
                source_name: "screen.webm".into(),
                frame_id: i as u64,
                face_idx: 0,
            };
            let crop = format!(
                "{}-crops/{i}-0-test.jpg",
                fp::accumulation::source_prefix("screen.webm")
            );
            let path = fp::crop_path(root, &key, &crop).unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &jpeg).unwrap();
            fp::Face {
                embedding,
                metadata: fp::Metadata {
                    schema_version: 1,
                    key,
                    activities: BTreeSet::new(),
                    source: fp::Source::Anchored,
                    label_text: None,
                    quality: i as f32 / 10.,
                    det_score: 1.,
                    face_w: 100.,
                    category: "meeting".into(),
                    crop: Some(crop),
                    added_at: "now".into(),
                    rejected: false,
                },
            }
        })
        .collect();
    let coordinate = json!(["alpha", DAY, "test-meeting"]).to_string();
    let seed = |faces: Vec<fp::Face>| {
        fp::reconcile(
            root,
            &coordinate,
            &encoder,
            &std::collections::BTreeMap::from([("entity-primary".into(), faces)]),
        )
        .unwrap();
    };
    let listed = dispatch_output(&journal, "list_entities", json!({}));
    let reference = listed.value["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            let (_, id) = resolve_entity_reference(root, row["reference"].as_str().unwrap());
            id == "entity-primary"
        })
        .unwrap()["reference"]
        .clone();
    let get = || dispatch_output(&journal, "get_entity", json!({"reference":reference}));
    for (count, state) in [(0, "none"), (3, "possible"), (5, "suggested")] {
        seed(faces[..count].to_vec());
        let output = get();
        assert_eq!(output.value["photo"]["state"], state);
        assert_eq!(!output.images.is_empty(), count == 5);
        let rendered =
            crate::jsonrpc::image_tool_result(output.value.clone(), None, &output.images);
        if count == 5 {
            assert_eq!(rendered["content"][1]["type"], "image");
            assert_eq!(rendered["structuredContent"], output.value);
            let image = image::load_from_memory(output.images.first().unwrap()).unwrap();
            assert!(image.width() <= 256 && image.height() <= 256);
        } else {
            assert_eq!(rendered["structuredContent"], output.value);
        }
    }
    fp::choose(root, "entity-primary", &faces[4].metadata.key).unwrap();
    let output = get();
    assert_eq!(output.value["photo"]["state"], "confirmed");
    assert!(!output.images.is_empty());
    assert!(
        dispatch_output(&journal, "list_entities", json!({})).value["entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["has_photo"] == true)
    );
    // The owner took profile pics away from this connection: entities still read, the picture does not.
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".into(), "no_profile_pics".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let output = get();
    assert!(output.value.get("photo").is_none());
    assert!(output.images.is_empty());
    assert!(
        dispatch_output(&journal, "list_entities", json!({})).value["entities"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["has_photo"] == false)
    );
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".into(), "profile_pics".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    assert!(!get().images.is_empty());
    // The same entity linked from another granted facet must not expose alpha's photo.
    fs::create_dir_all(root.join("facets/beta/entities/primary")).unwrap();
    fs::write(
        root.join("facets/beta/entities/primary/entity.json"),
        r#"{"entity_id":"entity-primary"}"#,
    )
    .unwrap();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["entities".into()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_B.into()],
                },
            },
        )
        .unwrap();
    let listed = dispatch_output(&journal, "list_entities", json!({"facet":FACET_B}));
    let row = listed.value["entities"]
        .as_array()
        .unwrap()
        .first()
        .unwrap();
    assert_eq!(row["has_photo"], false);
    let output = dispatch_output(
        &journal,
        "get_entity",
        json!({"reference":row["reference"]}),
    );
    assert_eq!(output.value["photo"]["state"], "confirmed");
    assert!(output.images.is_empty());
    fp::dismiss(root, "entity-primary").unwrap();
    assert_eq!(
        dispatch_output(
            &journal,
            "get_entity",
            json!({"reference":row["reference"]})
        )
        .value["photo"]["state"],
        "dismissed"
    );
    assert!(
        dispatch_output(
            &journal,
            "get_entity",
            json!({"reference":row["reference"]})
        )
        .images
        .is_empty()
    );
    fs::write(
        root.join("config/journal.json"),
        r#"{"faces":{"enabled":false}}"#,
    )
    .unwrap();
    let output = dispatch_output(
        &journal,
        "get_entity",
        json!({"reference":row["reference"]}),
    );
    assert!(output.value.get("photo").is_none());
    assert!(output.images.is_empty());
    let rendered = crate::jsonrpc::image_tool_result(output.value.clone(), None, &[]);
    assert_eq!(rendered["structuredContent"], output.value);
    assert!(!output.value.to_string().contains("embedding"));
}

// ---- journal links are owner pointers, separate from tool references ----

fn opened_journal_link(journal: &Path, row: &Value) -> solstone_core_journal_link::Place {
    let link = row["link"].as_str().expect("eligible row has a link");
    let sealed = link
        .strip_prefix("https://go.solstone.app/j#")
        .expect("journal URL");
    assert!(!link.contains(['[', ']', '<', '&', '\\']));
    solstone_core_journal_link::open(journal, sealed).unwrap()
}

#[test]
fn journal_links_keep_recording_identity_and_stay_out_of_entries() {
    use solstone_core_journal_link::Place;
    let journal = span_journal(None);
    for stream in ["desk", "phone", "default"] {
        write_span_segment(
            journal.path(),
            stream,
            "090000_300",
            "alpha",
            "[09:00:20] a line\n",
        );
    }
    let direct = journal.path().join("chronicle/20261010/090000_300");
    fs::create_dir_all(&direct).unwrap();
    fs::write(
        direct.join("meeting_transcript.md"),
        "[09:00:20] a direct line\n",
    )
    .unwrap();
    let list = probe(&journal, "list_transcripts", json!({"day":"20261010"})).unwrap();
    let rows = list["transcripts"].as_array().unwrap();
    assert_eq!(rows.len(), 4, "{list}");
    let mut streams = BTreeSet::new();
    for row in rows {
        let stream = segment_stream(journal.path(), row["reference"].as_str().unwrap());
        streams.insert(stream.clone());
        let expected = Place::Recording {
            day: "20261010".into(),
            stream,
            segment: "090000_300".into(),
        };
        assert_eq!(opened_journal_link(journal.path(), row), expected);
        let read = probe(
            &journal,
            "get_transcript",
            json!({"reference":row["reference"]}),
        )
        .unwrap();
        assert_eq!(opened_journal_link(journal.path(), &read), expected);
        assert!(!read["entries"].to_string().contains("go.solstone.app"));
    }
    assert_eq!(
        streams,
        BTreeSet::from([
            "_default".into(),
            "default".into(),
            "desk".into(),
            "phone".into()
        ])
    );
    let span = probe(
        &journal,
        "get_transcript",
        json!({"reference":list["span_reference"]}),
    )
    .unwrap();
    assert_eq!(span["segments"].as_array().unwrap().len(), 4);
    for section in span["segments"].as_array().unwrap() {
        let read = probe(
            &journal,
            "get_transcript",
            json!({"reference":section["reference"]}),
        )
        .unwrap();
        assert_eq!(
            opened_journal_link(journal.path(), section),
            opened_journal_link(journal.path(), &read)
        );
        assert!(!section["entries"].to_string().contains("go.solstone.app"));
    }
}

#[test]
fn journal_links_keep_activity_facets_and_body_out_of_recording_links() {
    use solstone_core_journal_link::Place;
    let journal = body_record_journal(true);
    let root = journal.path();
    fs::create_dir_all(root.join("streams")).unwrap();
    fs::write(root.join("streams/import.apple_health.json"), json!({
        "name":"import.apple_health", "kind":"import:apple_health", "host":null, "platform":null,
        "created_at":0, "last_day":null, "last_segment":null, "seq":1,
    }).to_string()).unwrap();
    for facet in ["alpha", "beta"] {
        write_activity_line(
            root,
            facet,
            "20260308",
            "same_id",
            "Linkneedle",
            100,
            Some("work"),
            None,
            None,
            None,
            None,
            None,
            None,
        );
    }
    let list = probe(&journal, "list_activities", json!({"day":"20260308"})).unwrap();
    let mut places = BTreeSet::new();
    for row in list["activities"].as_array().unwrap() {
        let Place::Activity { facet, day, id } = opened_journal_link(root, row) else {
            panic!("activity place")
        };
        assert_eq!(day, "20260308");
        places.insert((facet, id));
    }
    assert!(places.contains(&("alpha".into(), "same_id".into())));
    assert!(places.contains(&("beta".into(), "same_id".into())));
    assert!(places.contains(&("alpha".into(), "body_0123456789abcdef".into())));
    let week = probe(&journal, "get_week", json!({"week":"20260308"})).unwrap();
    assert_eq!(
        opened_journal_link(root, &week["body"][0]),
        Place::Activity {
            facet: "alpha".into(),
            day: "20260308".into(),
            id: "body_0123456789abcdef".into(),
        }
    );
    for row in week["activities"].as_array().unwrap() {
        opened_journal_link(root, row);
    }
    for memory in week["memories"].as_array().unwrap() {
        assert!(memory.get("link").is_none());
    }
    let plain = probe(&journal, "list_transcripts", json!({"day":"20260308"})).unwrap();
    let source = plain["other_sources"][0]
        .as_str()
        .unwrap_or_else(|| panic!("{plain}"));
    let body = probe(
        &journal,
        "list_transcripts",
        json!({"day":"20260308", "source":source}),
    )
    .unwrap();
    assert!(
        !body["transcripts"].as_array().unwrap().is_empty(),
        "{body}"
    );
    assert!(
        body["transcripts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.get("link").is_none())
    );
    let facets = probe(&journal, "list_facets", json!({})).unwrap();
    assert!(!facets.to_string().contains("go.solstone.app"));
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    let hits = probe(&journal, "search", json!({"query":"Linkneedle"})).unwrap();
    assert_eq!(hits["results"].as_array().unwrap().len(), 2, "{hits}");
    for hit in hits["results"].as_array().unwrap() {
        let Place::Activity { id, .. } = opened_journal_link(root, hit) else {
            panic!("activity hit")
        };
        assert_eq!(id, "same_id");
    }
}

#[test]
fn journal_links_attach_to_entity_rows_without_changing_tool_references() {
    use solstone_core_journal_link::Place;
    let journal = person_journal();
    let listed = probe(&journal, "list_entities", json!({})).unwrap();
    let rows = listed["entities"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{listed}");
    for row in rows {
        let reference = row["reference"].as_str().unwrap();
        let (id, _) = crate::names::parse_entity(reference).unwrap();
        assert_eq!(
            opened_journal_link(journal.path(), row),
            Place::Person { id }
        );
        assert!(probe(&journal, "get_entity", json!({"reference":reference})).is_ok());
    }
    let searched = probe(&journal, "search", json!({"query":"Bob Smith"})).unwrap();
    let cards = searched["entity_matches"].as_array().unwrap();
    assert!(!cards.is_empty(), "{searched}");
    for row in cards {
        assert_eq!(
            opened_journal_link(journal.path(), row),
            Place::Person { id: "bob".into() }
        );
    }
}

#[test]
fn photos_and_imported_screenshots_follow_their_own_picture_permissions() {
    let journal = fixture();
    let root = journal.path();
    let photo = root.join("chronicle/20260308/import.image/130000_300");
    let gone = root.join("chronicle/20260308/import.image/140000_300");
    let shot = root.join("chronicle/20260308/import.image/160000_300");
    for (folder, file, token) in [
        (&photo, "image_transcript.md", "barnphotoneedle"),
        (&gone, "image_transcript.md", "gonephotoneedle"),
        (&shot, "screenshot_transcript.md", "shotneedle"),
    ] {
        fs::create_dir_all(folder).unwrap();
        fs::write(folder.join(file), format!("{token}\n")).unwrap();
    }
    let pixels = |width, height| {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([200, 120, 0]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        bytes
    };
    fs::write(photo.join("original.png"), pixels(3000, 1500)).unwrap();
    fs::write(shot.join("original.screenshot.png"), pixels(64, 64)).unwrap();
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();

    let grant = |words: &[&str]| {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: words.iter().map(|word| (*word).to_owned()).collect(),
                    scope: ReadScope::WholeJournal,
                },
            )
            .unwrap();
    };
    let fetched = |token: &str| {
        let results = probe(&journal, "search", json!({"query": token})).unwrap();
        let reference = results["results"][0]["reference"].clone();
        assert!(reference.is_string(), "{token} is found: {results}");
        dispatch_output(&journal, "fetch", json!({"reference": reference}))
    };

    grant(&["photos", "screen_text"]);
    let output = fetched("barnphotoneedle");
    assert_eq!(output.value["picture"]["state"], "attached");
    assert_eq!(output.value["picture"]["width"], 1024);
    assert_eq!(output.value["picture"]["height"], 512);
    let picture = image::load_from_memory(output.images.first().unwrap()).unwrap();
    assert_eq!((picture.width(), picture.height()), (1024, 512));
    let rendered = crate::jsonrpc::image_tool_result(output.value.clone(), None, &output.images);
    assert_eq!(rendered["content"][1]["type"], "image");
    assert_eq!(rendered["content"][1]["mimeType"], "image/jpeg");

    // A photo whose picture is no longer kept still reads, and says so.
    let output = fetched("gonephotoneedle");
    assert_eq!(output.value["picture"]["state"], "not_kept");
    assert!(output.images.is_empty());

    // A pre-existing screen grant can still read the description.
    let output = fetched("shotneedle");
    assert_eq!(output.value["picture"]["state"], "not_allowed");
    assert!(output.images.is_empty());

    grant(&["screen_text", "screenshots"]);
    let output = fetched("shotneedle");
    assert_eq!(output.value["picture"]["state"], "attached");
    assert_eq!(output.images.len(), 1);

    // Without photos, neither the description nor the picture is found.
    grant(&["screen_text"]);
    assert!(
        probe(&journal, "search", json!({"query": "barnphotoneedle"})).unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn screen_connection_lists_a_mixed_segment_and_reads_one_display_at_a_time() {
    let journal = fixture();
    let root = journal.path();
    let segment = root.join("chronicle").join(DAY).join(STREAM).join(SEGMENT);
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["screen_text".into(), "screenshots".into()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.into()],
                },
            },
        )
        .unwrap();
    let sidecar = "{\"raw\":\"left_screen.webm\"}\n{\"timestamp\":0,\"analysis\":{\"visual_description\":\"a coloured square\"}}\n";
    fs::write(segment.join("left_screen.jsonl"), sidecar).unwrap();
    fs::write(segment.join("right_screen.jsonl"), "{}\n").unwrap();
    fs::write(
        segment.join("left_screen.webm"),
        include_bytes!("../../../fixtures/grab_corpus/distinct.mkv"),
    )
    .unwrap();
    let list = probe(&journal, "list_transcripts", json!({"day":DAY})).unwrap();
    let reference = &list["transcripts"][0]["reference"];
    assert!(reference.is_string(), "{list}");
    for arguments in [
        json!({"references":[reference],"offset_ms":0}),
        json!({"reference":reference,"offset_ms":0,"cursor":"next"}),
        json!({"reference":reference,"display":"left_screen"}),
    ] {
        assert_eq!(
            probe(&journal, "get_transcript", arguments),
            Err(McpProbeError::InvalidInput)
        );
    }
    let read = |offset: u32| {
        dispatch_output(
            &journal,
            "get_transcript",
            json!({"reference":reference,"display":"left_screen","offset_ms":offset}),
        )
    };
    let choice = probe(
        &journal,
        "get_transcript",
        json!({"reference":reference,"offset_ms":0}),
    )
    .unwrap();
    assert_eq!(
        choice["picture"]["displays"],
        json!(["left_screen", "right_screen"])
    );
    let first = read(100);
    let last = read(2500);
    assert_eq!(first.value["picture"]["description"], "a coloured square");
    assert_eq!(first.value["picture"]["frame_offset_ms"], 0);
    assert_eq!(last.value["picture"]["frame_offset_ms"], 2000);
    assert_eq!(last.images.len(), 1);
    assert_ne!(first.images, last.images);
    assert_eq!(first.value["entries"], json!([]));
    assert!(!first.value.to_string().contains("approved transcript"));
    let result = crate::jsonrpc::image_tool_result(last.value, None, &last.images);
    assert_eq!(result["content"][1]["mimeType"], "image/jpeg");
    fs::remove_file(segment.join("left_screen.webm")).unwrap();
    let gone = read(2500);
    assert_eq!(gone.value["picture"]["state"], "not_kept");
    assert_eq!(gone.value["picture"]["description"], "a coloured square");
    assert!(
        gone.value["picture"]["message"]
            .as_str()
            .unwrap()
            .contains("no longer kept")
    );
    assert!(gone.images.is_empty());
    fs::write(segment.join("left_screen.webm"), b"broken").unwrap();
    assert_eq!(read(2500).value["picture"]["state"], "unreadable");
    fs::write(segment.join("talents/facets.json"), r#"[{"facet":"beta"}]"#).unwrap();
    assert!(
        probe(
            &journal,
            "get_transcript",
            json!({"reference":reference,"offset_ms":0})
        )
        .is_err()
    );
}

#[test]
fn native_screenshot_and_its_description_arrive_through_the_screen_connection() {
    let journal = fixture();
    let root = journal.path();
    let stream = "phone_screenshots";
    let folder = root
        .join("chronicle")
        .join(DAY)
        .join(stream)
        .join("120000_1");
    fs::create_dir_all(&folder).unwrap();
    fs::create_dir_all(root.join("streams")).unwrap();
    fs::write(root.join("streams/phone_screenshots.json"),json!({"name":stream,"kind":"observer","source":"screenshots","created_at":0,"last_day":null,"last_segment":null,"seq":1,
        "cid":format!("sha256:{}", "a".repeat(64))}).to_string()).unwrap();
    fs::write(
        folder.join("screenshot_screen.jsonl"),
        concat!(
            r#"{"raw":"screenshot_screen.png"}"#,
            "\n",
            r#"{"timestamp":0,"analysis":{"visual_description":"a yellow square"}}"#
        ),
    )
    .unwrap();
    image::RgbImage::from_pixel(16, 16, image::Rgb([255, 255, 0]))
        .save(folder.join("screenshot_screen.png"))
        .unwrap();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["screen_text".into(), "screenshots".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let listed = probe(&journal, "list_transcripts", json!({"day":DAY})).unwrap();
    let reference = &listed["transcripts"][0]["reference"];
    let got = dispatch_output(
        &journal,
        "get_transcript",
        json!({"reference":reference,"offset_ms":0}),
    );
    assert_eq!(got.value["picture"]["description"], "a yellow square");
    assert_eq!(got.images.len(), 1);
    let image = image::load_from_memory(&got.images[0]).unwrap().to_rgb8();
    let pixel = image.get_pixel(0, 0);
    assert!(pixel[0] > 245 && pixel[1] > 245 && pixel[2] < 10);

    #[cfg(feature = "test-hooks")]
    {
        // The owner can turn the option off even after a picture was prepared.
        let store = PermissionStore::open(root);
        let (result, fired) = crate::source_freeze_barrier::run_with_source_freeze_barrier(
            crate::source_freeze_barrier::FreezePoint::After,
            1,
            move || {
                store
                    .set_permission(
                        CONNECTION,
                        ReadPermission {
                            categories: vec!["screen_text".into()],
                            scope: ReadScope::WholeJournal,
                        },
                    )
                    .unwrap();
            },
            || {
                probe(
                    &journal,
                    "get_transcript",
                    json!({"reference":reference,"offset_ms":0}),
                )
            },
        );
        assert!(fired);
        assert!(result.is_err());
        assert!(matches!(
            probe(
                &journal,
                "get_transcript",
                json!({"reference":reference,"offset_ms":0})
            ),
            Err(McpProbeError::PermissionDenied)
        ));
    }
}

#[test]
fn week_summaries_and_typed_find_share_the_live_kind_and_facet_boundary() {
    use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows};
    let journal = body_record_journal(true);
    let root = journal.path();
    let day = "20260308";
    let photo_text = "a plum tree in bloom ![marker](https://example.invalid/image)";
    for (path, text) in [
        ("import.image/130000_300/image_transcript.md", photo_text),
        (
            "import.image/140000_300/screenshot_transcript.md",
            "screen only",
        ),
        (
            "import.text/150000_300/note_transcript.md",
            "# Orchard inventory\n\nFour trees.",
        ),
    ] {
        let file = root.join("chronicle").join(day).join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let location = root.join("chronicle/20260308/phone_mobile-segment/070000_300");
    fs::create_dir_all(&location).unwrap();
    fs::write(
        location.join("location.jsonl"),
        concat!(
            r#"{"accuracy":"full","fix_count":0,"gap":false,"kind":"location","platform":"ios","schema":"solstone.location.segment/1","source":"location","tier":"full"}"#,
            "\n",
            r#"{"arrival":"2026-03-08T11:00:00Z","departure":"2026-03-08T12:00:00Z","h_acc":40,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}"#,
            "\n"
        ),
    ).unwrap();
    solstone_core_location::reconcile_journal(
        root,
        &mut solstone_core_location::Reconciler::default(),
    )
    .unwrap();
    let mut row = BodyRow::new(
        RowKind::Count,
        "steps1".to_owned(),
        "apple_health",
        "HKQuantityTypeIdentifierStepCount",
        "step count".to_owned(),
    );
    row.start = Some("2026-03-08 07:00:00 -0400".to_owned());
    row.end = Some("2026-03-08 07:15:00 -0400".to_owned());
    row.unit = Some("count".to_owned());
    row.value = Some("432".to_owned());
    let body_file = root.join("chronicle/20260308/import.apple_health/070000_3600/body.jsonl");
    let mut body_text = fs::read_to_string(&body_file).unwrap();
    let mut moved = row.clone();
    moved.kind = RowKind::Moved;
    let earlier = root.join("chronicle/20260308/import.apple_health/060000_300");
    fs::create_dir_all(&earlier).unwrap();
    fs::write(earlier.join("stream.json"), "{}\n").unwrap();
    let mut reading = row.clone();
    reading.id = "heart-rate".into();
    reading.kind = RowKind::Reading;
    reading.label = "heart rate".into();
    fs::write(earlier.join("body.jsonl"), encode_rows(&[reading, moved])).unwrap();
    body_text.push_str(&encode_rows(&[row]));
    fs::write(body_file, body_text).unwrap();
    let photo_original = root.join("chronicle/20260308/import.image/130000_300/original.png");
    image::RgbImage::new(8, 8).save(photo_original).unwrap();
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();

    let grant = |words: &[&str], scope| {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: words.iter().map(|word| (*word).to_owned()).collect(),
                    scope,
                },
            )
            .unwrap();
    };
    let week_args =
        json!({"week": day, "include": ["body", "places", "photos", "notes", "entities"]});
    let all = [
        "facets",
        "transcripts",
        "body",
        "places",
        "photos",
        "notes",
        "entities",
    ];
    grant(&all, ReadScope::WholeJournal);
    let week = probe(&journal, "get_week", week_args.clone()).unwrap();
    assert!(week["said"].as_array().unwrap().is_empty());
    assert_eq!(week["days"].as_array().unwrap().len(), 7);
    let summary = &week["days"][0]["summary"];
    assert_eq!(summary["body"]["movement"][0]["steps"], 432.0, "{week}");
    assert!(
        summary["places"]["items"].to_string().contains("Denver"),
        "{summary}"
    );
    let range_week = probe(
        &journal,
        "get_week",
        json!({
            "day_from": "2026-03-08",
            "day_to": "2026-03-14",
            "include": ["body", "places", "photos", "notes", "entities"],
        }),
    )
    .unwrap();
    assert_eq!(range_week["days"].as_array().unwrap().len(), 7);
    assert_eq!(range_week["days"][0]["link"], week["days"][0]["link"]);
    assert_eq!(range_week["days"][0]["summary"]["body"], summary["body"]);
    assert_eq!(
        range_week["days"][0]["summary"]["places"],
        summary["places"]
    );
    assert_eq!(
        range_week["days"][0]["summary"]["photos"]["count"],
        summary["photos"]["count"]
    );
    assert_eq!(
        range_week["days"][0]["summary"]["notes"]["items"][0]["title"],
        "Orchard inventory"
    );
    assert_eq!(
        range_week["days"][0]["summary"]["entities"],
        summary["entities"]
    );
    let place_rows = probe(&journal, "search", json!({"day": day, "kind":"places"})).unwrap();
    assert!(
        !place_rows["results"].as_array().unwrap().is_empty(),
        "{place_rows}"
    );
    let place_reference = place_rows["results"][0]["reference"].clone();
    assert!(probe(&journal, "fetch", json!({"reference": place_reference})).is_ok());
    fs::remove_file(location.join("location.jsonl")).unwrap();
    let stale = probe(&journal, "search", json!({"day": day, "kind":"places"})).unwrap();
    assert_eq!(stale["results"], json!([]), "{stale}");
    let stale_fetch = probe(&journal, "fetch", json!({"references": [place_reference]})).unwrap();
    assert_eq!(stale_fetch["items"][0]["error"]["code"], "not_found");
    let current = probe(&journal, "get_week", week_args.clone()).unwrap();
    assert_eq!(current["days"][0]["summary"]["places"]["items"], json!([]));
    assert_eq!(summary["photos"]["count"], 1, "{summary}");
    let preview = summary["photos"]["items"][0]["description"]
        .as_str()
        .unwrap();
    assert!(preview.contains("a plum tree in bloom"), "{preview:?}");
    assert!(!preview.contains("![marker]"), "{preview:?}");
    assert_eq!(
        summary["notes"]["items"][0]["title"], "Orchard inventory",
        "{summary}"
    );

    let mut photos = Vec::new();
    for (tool, field) in [
        ("search", "results"),
        ("list_transcripts", "transcripts"),
        ("list_activities", "activities"),
    ] {
        let page = probe(
            &journal,
            tool,
            json!({
                "day_from": "2026-03-08", "day_to": "20260314", "kind": "photos"
            }),
        )
        .unwrap();
        let rows = page[field].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{tool}: {page}");
        let preview = rows[0]["description"].as_str().unwrap();
        assert!(preview.contains("a plum tree in bloom"), "{preview:?}");
        assert!(!preview.contains("![marker]"), "{preview:?}");
        photos.push(rows[0]["reference"].clone());
    }
    let by_kind = probe(&journal, "search", json!({"kind":"photos"})).unwrap();
    assert_eq!(by_kind["results"].as_array().unwrap().len(), 1, "{by_kind}");
    let fetched = dispatch_output(
        &journal,
        "fetch",
        json!({"references": [
            photos[0], "missing", photos[1],
        ]}),
    );
    assert_eq!(
        fetched.value["items"][0]["result"]["picture"]["state"],
        "attached"
    );
    assert_eq!(fetched.value["items"][1]["error"]["code"], "not_found");
    assert_eq!(fetched.value["items"][2]["image_content_index"], 2);
    assert_eq!(fetched.images.len(), 2);

    for denied in ["body", "places", "photos", "notes", "entities"] {
        let without: Vec<_> = all.into_iter().filter(|kind| *kind != denied).collect();
        grant(&without, ReadScope::WholeJournal);
        let week = probe(&journal, "get_week", week_args.clone()).unwrap();
        for day in week["days"].as_array().unwrap() {
            let refused = day["summary"][denied].as_object().unwrap();
            assert_eq!(refused.len(), 1, "{denied}: {refused:?}");
            assert!(refused["note"].is_string());
        }
        for tool in ["search", "list_transcripts", "list_activities"] {
            assert_eq!(
                probe(&journal, tool, json!({"day": day, "kind": denied})),
                Err(McpProbeError::PermissionDenied),
            );
        }
    }
    grant(&["transcripts", "notes"], ReadScope::WholeJournal);
    for (field, word) in [
        ("person", "Anne"),
        ("activity", "writing"),
        ("place", "Denver"),
    ] {
        assert_eq!(
            probe(&journal, "search", json!({"day": day, (field): word})),
            Err(McpProbeError::PermissionDenied),
        );
    }
    grant(
        &all,
        ReadScope::Facets {
            ids: vec![FACET_B.into()],
        },
    );
    assert_eq!(
        probe(&journal, "get_week", week_args),
        Err(McpProbeError::PermissionDenied),
    );
    for tool in ["search", "list_transcripts", "list_activities"] {
        assert_eq!(
            probe(&journal, tool, json!({"day": day, "place": "Denver"})),
            Err(McpProbeError::PermissionDenied),
        );
    }
    let listed = probe(
        &journal,
        "search",
        json!({
            "day": day, "kind": ["places", "photos", "notes"]
        }),
    )
    .unwrap();
    assert_eq!(listed["results"], json!([]));
    let fetched = probe(&journal, "fetch", json!({"references": [photos[0]]})).unwrap();
    assert_eq!(fetched["items"][0]["error"]["code"], "not_found");
}

#[test]
fn batched_reference_reads_match_single_pages_and_keep_individual_failures() {
    let journal = body_record_journal(true);
    allow_legacy_markdown(&journal);
    let day = "20260308";
    let segment = journal.path().join("chronicle/20260308/default/090000_300");
    fs::create_dir_all(segment.join("talents")).unwrap();
    fs::write(
        segment.join("meeting_transcript.md"),
        "a morning conversation",
    )
    .unwrap();
    fs::write(segment.join("talents/facets.json"), r#"[{"facet":"beta"}]"#).unwrap();
    let native = probe(&journal, "list_activities", json!({"from": day, "to": day})).unwrap();
    let row = native["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["title"] == "Beta work")
        .unwrap();
    let pairs =
        [
            ("get_activity_sources", row["reference"].clone()),
            (
                "get_transcript",
                probe(&journal, "list_transcripts", json!({"day": day})).unwrap()["transcripts"][0]
                    ["reference"]
                    .clone(),
            ),
        ];
    for (tool, reference) in &pairs {
        assert!(reference.is_string(), "{tool}");
        let single = probe(&journal, tool, json!({"reference":reference})).unwrap();
        let batch = probe(&journal, tool, json!({"references":[reference, "missing"]})).unwrap();
        assert_eq!(
            crate::dispatch::canonical_content(&batch["items"][0]["result"]),
            crate::dispatch::canonical_content(&single),
            "{tool}",
        );
        assert_eq!(batch["items"][1]["error"]["code"], "not_found");
        assert_eq!(
            probe(
                &journal,
                tool,
                json!({"reference":reference,"references":[reference]})
            ),
            Err(McpProbeError::InvalidInput),
        );
        assert_eq!(
            probe(&journal, tool, json!({"references":vec!["missing"; 13]})),
            Err(McpProbeError::InvalidInput),
        );
    }
    let alpha = journal.path().join("chronicle/20260308/desk/093000_300");
    fs::create_dir_all(alpha.join("talents")).unwrap();
    fs::write(alpha.join("meeting_transcript.md"), "a second conversation").unwrap();
    fs::write(alpha.join("talents/facets.json"), r#"[{"facet":"alpha"}]"#).unwrap();
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "transcripts".into(),
                    "messages".to_owned(),
                    "facets".into(),
                    "body".into(),
                ],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.into()],
                },
            },
        )
        .unwrap();
    let list = probe(&journal, "list_transcripts", json!({"day": day})).unwrap();
    let allowed = &list["transcripts"][0]["reference"];
    let denied = &pairs[1].1;
    assert!(allowed.as_str().unwrap().contains("/093000_300/"), "{list}");
    assert!(
        denied.as_str().unwrap().contains("/090000_300/"),
        "{denied}"
    );
    let mixed = probe(
        &journal,
        "get_transcript",
        json!({"references": [denied, allowed]}),
    )
    .unwrap();
    assert_eq!(mixed["items"][0]["error"]["code"], "not_found");
    assert!(
        mixed["items"][1]["result"]["entries"]
            .to_string()
            .contains("second conversation")
    );
}

#[test]
fn imported_material_previews_describe_content_and_photo_fetch_keeps_its_literal_text() {
    let journal = fixture();
    let root = journal.path();
    let day = "20260308";
    let photo_text =
        r"Frost on wood, a literal \ and &lt;tag&gt; ![marker](https://example.invalid)";
    let stored_photo_text = solstone_core_format::markdown::literal_text(photo_text);
    let photo = format!(
        "# Yard rail\n\n**Type:** Image\n**Format:** JPEG\n**Dimensions:** 1200×896\n\n---\n\n**Caption:**\ncaptionneedle on the rail\n\n---\n\n> [image description — model-derived]\n> {stored_photo_text}\n"
    );
    let footer =
        "> [image description — model-derived]\n> unavailable — No active provider configured\n";
    let caption_only = format!(
        "# Captured\n\n**Type:** Image\n\n---\n\n**Caption:**\nA handwritten caption\n\n---\n\n{footer}"
    );
    let unavailable = format!("# Untitled\n\n**Type:** Image\n\n---\n\n{footer}");
    let document = concat!(
        "# Workshop minutes\n\n**Type:** Document\n**Pages:** 3\n",
        "**Date:** 2026-03-08 (file modification time)\n\n---\n\n",
        "## Page 1\n\n> [page text unavailable — scan failed; no page image could be produced]\n\n",
        "## Page 2\n\nFirst readable page records the vote.\n\n",
        "## Page 3\n\nThe pageonlyneedle ends the workshop.\n",
    );
    for (path, entry) in [
        (
            "import.image/130000_300/image_transcript.md",
            photo.as_str(),
        ),
        (
            "import.image/140000_300/image_transcript.md",
            caption_only.as_str(),
        ),
        (
            "import.image/150000_300/image_transcript.md",
            unavailable.as_str(),
        ),
        (
            "import.document/160000_300/document_transcript.md",
            document,
        ),
    ] {
        let file = root.join("chronicle").join(day).join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, entry).unwrap();
    }
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "facets".into(),
                    "transcripts".into(),
                    "photos".into(),
                    "documents".into(),
                ],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let result = probe(&journal, "search", json!({"day":day,"kind":"photos"})).unwrap();
    let rows = result["results"].as_array().unwrap();
    let find = |title: &str| rows.iter().find(|row| row["title"] == title).unwrap();
    assert_eq!(rows.len(), 3, "{result}");
    let row = find("Yard rail");
    for field in ["description", "snippet"] {
        let preview = row[field].as_str().unwrap();
        assert!(preview.contains("Frost on wood"), "{preview}");
        assert!(
            !preview.contains("Type:") && !preview.contains("![marker]"),
            "{preview}"
        );
    }
    assert_eq!(find("Captured")["description"], "A handwritten caption");
    assert!(find("Untitled").get("description").is_none());
    let matched_caption = probe(
        &journal,
        "search",
        json!({"day":day,"kind":"photos","query":"captionneedle"}),
    )
    .unwrap();
    assert!(
        matched_caption["results"][0]["description"]
            .as_str()
            .unwrap()
            .contains("Frost on wood")
    );
    let week = probe(
        &journal,
        "get_week",
        json!({"week":day,"include":["photos"]}),
    )
    .unwrap();
    let items = week["days"][0]["summary"]["photos"]["items"]
        .as_array()
        .unwrap();
    for item in items {
        assert_eq!(
            item.get("description"),
            find(item["title"].as_str().unwrap()).get("description")
        );
    }

    let reference = &row["reference"];
    let single = probe(&journal, "fetch", json!({"reference":reference})).unwrap();
    let batch = probe(&journal, "fetch", json!({"references":[reference]})).unwrap();
    let text = single["text"].as_str().unwrap();
    assert!(text.contains(&stored_photo_text), "{text}");
    assert!(text.contains(r"\*\*Type\:\*\* Image"), "{text}");
    assert_eq!(batch["items"][0]["result"]["text"], text);

    let docs = probe(
        &journal,
        "search",
        json!({"day":day,"kind":"documents","query":"modification"}),
    )
    .unwrap();
    let header = &docs["results"][0];
    assert_eq!(header["title"], "Workshop minutes", "{docs}");
    assert_eq!(
        header["description"], "First readable page records the vote.",
        "{docs}"
    );
    assert!(!header["snippet"].as_str().unwrap().contains("Type"));
    let hit = probe(
        &journal,
        "search",
        json!({"day":day,"kind":"documents","query":"pageonlyneedle"}),
    )
    .unwrap();
    assert!(
        hit["results"][0]["description"]
            .as_str()
            .unwrap()
            .contains("pageonlyneedle"),
        "{hit}"
    );
}

#[test]
fn long_message_parts_identify_adjacent_turns_across_single_span_and_batch() {
    let journal = fixture();
    let root = journal.path();
    let day = "20260308";
    let turns = [("a", 75_000), ("b", 70_123), ("c", 2)];
    let rows = turns
        .iter()
        .map(|(word, count)| json!({"start":"00:00:00","text":word.repeat(*count)}).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    write_segment_file(
        root,
        day,
        "import.claude",
        "074500_300",
        "alpha",
        "conversation_transcript.jsonl",
        &format!("{{}}\n{rows}\n"),
    );
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["messages".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let listed = probe(&journal, "list_transcripts", json!({"day":day,"limit":100})).unwrap();
    let reference = listed["transcripts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["reference"].as_str().unwrap_or("").contains("074500_300"))
        .unwrap()["reference"]
        .clone();
    let direct = probe(&journal, "get_transcript", json!({"reference":reference})).unwrap();
    let batch = probe(
        &journal,
        "get_transcript",
        json!({"references":[reference]}),
    )
    .unwrap();
    assert_eq!(
        direct["turn_parts"],
        batch["items"][0]["result"]["turn_parts"]
    );
    assert_eq!(direct["turn_parts"][0]["part"]["byte_offset"], 0);
    assert_eq!(direct["turn_parts"][0]["part"]["complete"], false);

    let mut actual = std::collections::BTreeMap::<String, String>::new();
    let mut complete = std::collections::BTreeSet::new();
    let mut ordinary = Vec::new();
    let mut cursor = None::<String>;
    loop {
        let mut args = json!({"reference":listed["span_reference"]});
        if let Some(cursor) = cursor {
            args["cursor"] = json!(cursor);
        }
        let span = probe(&journal, "get_transcript", args).unwrap();
        for section in span["segments"].as_array().unwrap() {
            if section["reference"] != reference {
                continue;
            }
            for (index, entry) in section["entries"].as_array().unwrap().iter().enumerate() {
                let text = entry.as_str().unwrap();
                let part = section["turn_parts"]
                    .as_array()
                    .and_then(|parts| parts.iter().find(|p| p["entry"] == index));
                if let Some(part) = part {
                    let id = part["part"]["turn_id"].as_str().unwrap();
                    let body = actual.entry(id.into()).or_default();
                    assert_eq!(part["part"]["byte_offset"], body.len());
                    body.push_str(text);
                    if part["part"]["complete"] == true {
                        assert!(complete.insert(id.to_owned()));
                    }
                } else {
                    ordinary.push(text.to_string());
                }
            }
        }
        cursor = span["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(complete.len(), 2);
    let bodies: std::collections::BTreeSet<_> = actual.into_values().collect();
    assert_eq!(
        bodies,
        std::collections::BTreeSet::from(["a".repeat(75_000), "b".repeat(70_123)])
    );
    assert_eq!(ordinary, ["cc"]);
}

#[test]
fn conversation_grants_agree_across_listing_direct_batch_and_search() {
    use solstone_core_indexer_query::{Order, OwnerBoundary, SearchRequest, search};
    let journal = fixture();
    let root = journal.path();
    let day = "20260308";
    let cases = [
        (
            "import.chatgpt",
            "090000_300",
            "conversation_transcript.jsonl",
            None,
            "amberchat",
        ),
        (
            "import.claude",
            "100000_300",
            "imported_audio.jsonl",
            None,
            "legacychat",
        ),
        (
            "import.text",
            "110000_300",
            "conversation_transcript.jsonl",
            None,
            "neutralturn",
        ),
        (
            "import.text",
            "120000_300",
            "conversation_transcript.jsonl",
            Some("webvtt_cues"),
            "cobaltcaption",
        ),
        (
            "import.text",
            "130000_300",
            "conversation_transcript.jsonl",
            Some("zoom_chat"),
            "quartzwritten",
        ),
    ];
    for (stream, segment, file, reason, word) in cases {
        let mut header = json!({"imported":{"id":"synthetic"}});
        if let Some(reason) = reason {
            header["imported"]["triage"] =
                json!({"kind":"transcripts","ruleset_version":7,"reasons":[reason]});
        }
        let row = json!({"start":"00:00:00","speaker":"Rae","text":word});
        write_segment_file(
            root,
            day,
            stream,
            segment,
            "alpha",
            file,
            &format!("{header}\n{row}\n"),
        );
    }
    write_segment_file(
        root,
        day,
        "import.text",
        "110000_300",
        "alpha",
        "note_transcript.md",
        "adjacentnote",
    );
    write_segment_file(
        root,
        day,
        "import.text",
        "140000_300",
        "alpha",
        "imported.md",
        "oldneutralmarkdown",
    );
    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    for (_, _, _, _, word) in cases {
        let hits = search(
            root,
            OwnerBoundary,
            &SearchRequest::new(word, Order::Recency),
            jiff::civil::date(2026, 3, 8),
        )
        .unwrap();
        assert!(!hits.results.is_empty(), "{word}");
    }
    // Reopen the permission record a pre-Messages Journal wrote for its
    // default connection. Exercise the upgrade without rewriting the grant.
    let old_grant = json!({"schema":1,"permissions":[{
        "connection":CONNECTION, "schema":1, "generation":7, "evaluation":"enforce",
        "read":{"scope":{"kind":"whole_journal"},
                "categories":["transcripts","entities","profile_pics","facets","screen_text","body"]}
    }]}).to_string();
    let path = PermissionStore::open(root).permissions_path();
    fs::write(&path, &old_grant).unwrap();
    let listing = probe(&journal, "list_transcripts", json!({"day":day,"limit":100})).unwrap();
    let span = probe(
        &journal,
        "get_transcript",
        json!({"reference":listing["span_reference"]}),
    )
    .unwrap();
    let entries = span["segments"].to_string();
    assert!(entries.contains("cobaltcaption"));
    for withheld in ["amberchat", "legacychat", "neutralturn", "quartzwritten"] {
        assert!(!entries.contains(withheld), "{withheld}");
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), old_grant);
    assert!(
        !ReadPermission::default_whole_journal()
            .categories
            .iter()
            .any(|word| word == "messages")
    );
    for (kinds, visible) in [
        (
            vec!["messages"],
            vec!["amberchat", "legacychat", "quartzwritten"],
        ),
        (vec!["transcripts"], vec!["cobaltcaption"]),
        (
            vec!["transcripts", "messages"],
            vec![
                "cobaltcaption",
                "amberchat",
                "legacychat",
                "quartzwritten",
                "neutralturn",
            ],
        ),
        (
            vec!["transcripts", "messages", "notes"],
            vec![
                "cobaltcaption",
                "amberchat",
                "legacychat",
                "quartzwritten",
                "neutralturn",
                "adjacentnote",
            ],
        ),
        (vec!["notes"], vec!["adjacentnote"]),
    ] {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: kinds.iter().map(|kind| String::from(*kind)).collect(),
                    scope: ReadScope::WholeJournal,
                },
            )
            .unwrap();
        let list = probe(&journal, "list_transcripts", json!({"day":day,"limit":100})).unwrap();
        let span = probe(
            &journal,
            "get_transcript",
            json!({"reference":list["span_reference"]}),
        )
        .unwrap();
        let mut direct_text = String::new();
        for row in list["transcripts"].as_array().unwrap() {
            let reference = &row["reference"];
            let direct = probe(&journal, "get_transcript", json!({"reference":reference})).unwrap();
            let batch = probe(
                &journal,
                "get_transcript",
                json!({"references":[reference]}),
            )
            .unwrap();
            assert_eq!(direct["entries"], batch["items"][0]["result"]["entries"]);
            let entries = direct["entries"].to_string();
            let (expected, unresolved) =
                if entries.contains("neutralturn") || entries.contains("oldneutralmarkdown") {
                    (
                        if entries.contains("adjacentnote") {
                            json!(["notes"])
                        } else {
                            json!([])
                        },
                        true,
                    )
                } else if entries.contains("adjacentnote") {
                    (json!(["notes"]), false)
                } else if entries.contains("cobaltcaption") {
                    (json!(["transcripts"]), false)
                } else if ["amberchat", "legacychat", "quartzwritten"]
                    .iter()
                    .any(|word| entries.contains(word))
                {
                    (json!(["messages"]), false)
                } else {
                    direct_text.push_str(&entries);
                    continue;
                };
            let section = span["segments"]
                .as_array()
                .unwrap()
                .iter()
                .find(|section| section["entries"] == direct["entries"])
                .unwrap();
            for result in [row, &direct, &batch["items"][0]["result"], section] {
                assert_eq!(result["content_kinds"], expected, "{result}");
                assert_eq!(
                    result.get("conversation_state").and_then(Value::as_str),
                    unresolved.then_some("unresolved"),
                    "{result}"
                );
            }
            direct_text.push_str(&direct["entries"].to_string());
        }
        let both = kinds.contains(&"messages") && kinds.contains(&"transcripts");
        assert_eq!(direct_text.contains("oldneutralmarkdown"), both);
        let legacy = probe(&journal, "search", json!({"query":"oldneutralmarkdown"})).unwrap();
        assert_eq!(
            !legacy["results"].as_array().unwrap().is_empty(),
            both,
            "{kinds:?}"
        );
        if both {
            let hit = &legacy["results"][0];
            let fetched = probe(&journal, "fetch", json!({"reference":hit["reference"]})).unwrap();
            for row in [hit, &fetched] {
                assert_eq!(row["conversation_state"], "unresolved");
                assert!(row.get("content_kind").is_none());
            }
            for kind in ["messages", "transcripts"] {
                let filtered = probe(
                    &journal,
                    "search",
                    json!({"query":"oldneutralmarkdown","kind":kind}),
                )
                .unwrap();
                assert!(filtered["results"].as_array().unwrap().is_empty());
            }
        }
        for word in cases.iter().map(|case| case.4).chain(["adjacentnote"]) {
            assert_eq!(
                direct_text.contains(word),
                visible.contains(&word),
                "{kinds:?}: {word}"
            );
            let hits = probe(&journal, "search", json!({"query":word})).unwrap();
            assert_eq!(
                !hits["results"].as_array().unwrap().is_empty(),
                kinds.contains(&"notes") && word == "adjacentnote"
            );
        }
    }
}

#[test]
fn agent_rows_report_source_and_content_kind_across_materials() {
    use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows, recorder_id};

    let journal = base_activity_journal();
    let root = journal.path();
    let day = "20260407";

    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "transcripts".into(),
                    "entities".into(),
                    "facets".into(),
                    "screen_text".into(),
                    "screenshots".into(),
                    "body".into(),
                    "notes".into(),
                    "documents".into(),
                    "calendar".into(),
                    "places".into(),
                    "photos".into(),
                ],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    fs::create_dir_all(root.join("streams")).unwrap();
    for (idx, (stream, kind, cid, source, platform)) in [
        ("import.text", "import", None, None, None),
        ("import.document", "import", None, None, None),
        ("import.image", "import", None, None, None),
        ("import.obsidian", "import", None, None, None),
        ("import.apple_health", "import", None, None, None),
        (
            "macos_audio",
            "observer",
            Some("sha256:mac1"),
            Some("audio"),
            Some("macos"),
        ),
        (
            "macos_screen",
            "observer",
            Some("sha256:mac1"),
            Some("screen"),
            Some("macos"),
        ),
        (
            "macos_obsidian",
            "observer",
            Some("sha256:mac1"),
            Some("obsidian"),
            Some("macos"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut obj = json!({
            "name": stream,
            "kind": kind,
            "host": null,
            "platform": platform,
            "created_at": (idx as u64) + 1,
            "last_day": day,
            "last_segment": null,
            "seq": 1
        });
        if let Some(cid) = cid {
            obj["cid"] = json!(cid);
        }
        if let Some(source) = source {
            obj["source"] = json!(source);
        }
        fs::write(root.join(format!("streams/{stream}.json")), obj.to_string()).unwrap();
    }
    fs::create_dir_all(root.join("link")).unwrap();
    fs::write(
        root.join("link/client-descriptions.json"),
        json!({
            "sha256:mac1": {
                "protocol_version": 1,
                "revision": 1,
                "owner_label": null,
                "updated_at": null,
                "reported": {
                    "name": "studio",
                    "platform": "macos",
                    "device_type": "desktop",
                    "app_id": null,
                    "app_version": null
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let seed_seg = |stream: &str, seg: &str, file: &str, body: &str| {
        write_segment_file(root, day, stream, seg, "alpha", file, body);
        fs::write(
            root.join(format!("chronicle/{day}/{stream}/{seg}/stream.json")),
            json!({"stream": stream}).to_string(),
        )
        .unwrap();
    };

    seed_seg(
        "import.text",
        "090000_300",
        "note_transcript.md",
        "# Field Note\n\nCheck lanternvalveneedle before winter.\n",
    );
    seed_seg(
        "import.document",
        "091000_300",
        "document_transcript.md",
        "# Pump Manual\n\n**Type:** Document\n\nInspect pumpsealneedle every autumn.\n",
    );
    seed_seg(
        "import.text",
        "092000_300",
        "conversation_transcript.jsonl",
        "{\"untimed\":true,\"imported\":{\"id\":\"example\",\"triage\":{\"kind\":\"transcripts\",\"ruleset_version\":7,\"reasons\":[\"webvtt_cues\"]}}}\n{\"start\":\"00:00:00\",\"speaker\":\"Nora\",\"text\":\"Order sparegasketneedle next week.\"}\n",
    );
    seed_seg(
        "import.image",
        "093000_300",
        "screenshot_transcript.md",
        "# Receipt Screenshot\n\n**Type:** Screenshot\n\nOrder receipttotalneedle 42 dollars.\n",
    );
    seed_seg(
        "import.image",
        "094000_300",
        "image_transcript.md",
        "# Garden Shed\n\n**Type:** Image\n\nRed wateringcanneedle beside the bench.\n",
    );
    seed_seg(
        "import.obsidian",
        "095000_300",
        "note_transcript.md",
        "# Trail Ideas\n\nRidgeloopneedle starts at dawn.\n",
    );
    seed_seg(
        "macos_obsidian",
        "095500_300",
        "note_transcript.md",
        "# Bound Vault Note\n\nBoundobsidianneedle from sync.\n",
    );
    seed_seg(
        "macos_audio",
        "100000_300",
        "audio.jsonl",
        "{\"source\":\"recording\"}\n{\"start\":\"10:00:05\",\"text\":\"Pack campstoveneedle before sunrise.\"}\n",
    );
    let speech_topic = "Packing campstoveneedle before sunrise.\n";
    let talents = root.join(format!("chronicle/{day}/macos_audio/100000_300/talents"));
    fs::write(talents.join("topics.md"), speech_topic).unwrap();
    fs::write(
        talents.join(solstone_core_format::content::topics::PROVENANCE_FILE),
        solstone_core_format::content::topics::provenance(
            speech_topic,
            Some(&[solstone_core_format::content::AdmittedCategory::Transcripts].into()),
        )
        .to_string(),
    )
    .unwrap();
    // Screen-text segment (no speech file) for search/fetch/list verification.
    let screen_seg = root.join(format!("chronicle/{day}/macos_screen/101000_300"));
    fs::create_dir_all(screen_seg.join("talents")).unwrap();
    fs::write(
        screen_seg.join("talents/facets.json"),
        r#"[{"facet":"alpha"}]"#,
    )
    .unwrap();
    fs::write(
        screen_seg.join("stream.json"),
        json!({"stream": "macos_screen"}).to_string(),
    )
    .unwrap();
    fs::write(
        screen_seg.join("talents/screen_text.md"),
        "## 00:00:05\n\nEditor showing screentextpageneedle.\n",
    )
    .unwrap();
    // Mixed segment holding two imported kinds in one directory to verify unit splitting.
    seed_seg(
        "import.text",
        "102000_300",
        "note_transcript.md",
        "# Combined Note\n\nCombinednoteneedle line.\n",
    );
    fs::write(
        root.join(format!(
            "chronicle/{day}/import.text/102000_300/document_transcript.md"
        )),
        "# Combined Doc\n\nCombineddocneedle line.\n",
    )
    .unwrap();

    // Streamless day-level files: body.md and location.md carry content_kind and no source.
    fs::write(
        root.join(format!("chronicle/{day}/body.md")),
        "# body on Tuesday, April 7, 2026\n\n## Apple Health · imported\n\n- workout: running bodydayneedle\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("imports")).unwrap();
    fs::write(
        root.join("imports/.apple_health-zone.json"),
        r#"{"zone":"America/New_York"}"#,
    )
    .unwrap();
    let mut row = BodyRow::new(
        RowKind::Experience,
        "w1".to_owned(),
        "apple_health",
        "HKWorkoutActivityTypeRunning",
        "Running".to_owned(),
    );
    row.experience = Some("workout".to_owned());
    row.start = Some("2026-04-07 07:00:00 -0400".to_owned());
    row.end = Some("2026-04-07 07:40:00 -0400".to_owned());
    row.by = Some("Synthetic Watch".to_owned());
    let window = root.join(format!("chronicle/{day}/import.apple_health/070000_3600"));
    fs::create_dir_all(window.join("talents")).unwrap();
    fs::write(
        window.join("body.jsonl"),
        encode_rows(std::slice::from_ref(&row)),
    )
    .unwrap();
    fs::write(
        window.join("stream.json"),
        json!({"stream": "import.apple_health"}).to_string(),
    )
    .unwrap();
    fs::write(window.join("talents/facets.json"), r#"[{"facet":"alpha"}]"#).unwrap();

    for (idx, (id, title, kind, seg, stream)) in [
        (
            "act_note",
            "Field Note Review",
            "note",
            "090000_300",
            "import.text",
        ),
        (
            "act_doc",
            "Pump Manual Review",
            "document",
            "091000_300",
            "import.document",
        ),
        (
            "act_tx",
            "Imported Chat",
            "conversation",
            "092000_300",
            "import.text",
        ),
        (
            "act_shot",
            "Receipt Check",
            "import",
            "093000_300",
            "import.image",
        ),
        (
            "act_photo",
            "Garden Shed Photo",
            "import",
            "094000_300",
            "import.image",
        ),
        (
            "act_obs",
            "Trail Planning Note",
            "note",
            "095000_300",
            "import.obsidian",
        ),
        (
            "act_rec",
            "Sunrise Packing Call",
            "conversation",
            "100000_300",
            "macos_audio",
        ),
        (
            "act_two",
            "Combined Import",
            "import",
            "102000_300",
            "import.text",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        write_activity_line(
            root,
            "alpha",
            day,
            id,
            title,
            100 + idx as i64,
            Some(kind),
            Some(&[seg]),
            None,
            None,
            Some(stream),
            None,
            None,
        );
    }
    let body_record = json!({
        "id": "body_0123456789abcdef",
        "source": "body",
        "activity": "workout",
        "account": {
            "key": "import.apple_health\u{1f}w1",
            "start": row.start,
            "end": row.end,
            "type": "running",
            "recorder": recorder_id("import.apple_health", row.by.as_deref())
        },
    });
    let act_path = root.join(format!("facets/alpha/activities/{day}.jsonl"));
    let mut act_text = fs::read_to_string(&act_path).unwrap();
    act_text.push_str(&format!("{body_record}\n"));
    fs::write(&act_path, act_text).unwrap();

    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();

    // 1 & 2. Unfiltered search hits and fetched entries carry content_kind and source.
    for (query, expected_kind, expected_source) in [
        ("lanternvalveneedle", "notes", Some("imported file")),
        ("pumpsealneedle", "documents", Some("PDF")),
        ("receipttotalneedle", "screen_text", Some("imported file")),
        ("wateringcanneedle", "photos", Some("imported file")),
        ("ridgeloopneedle", "notes", Some("Obsidian")),
        ("boundobsidianneedle", "notes", Some("Obsidian")),
        ("campstoveneedle", "transcripts", Some("your mac · audio")),
        (
            "screentextpageneedle",
            "screen_text",
            Some("your mac · screen"),
        ),
        ("bodydayneedle", "body", None),
    ] {
        let search = probe(&journal, "search", json!({"query": query, "day": day})).unwrap();
        assert_no_leaked_keys(&search);
        let hit = &search["results"][0];
        assert_eq!(hit["content_kind"], expected_kind, "{query}: {hit}");
        assert_eq!(
            hit.get("source").and_then(Value::as_str),
            expected_source,
            "{query}: {hit}"
        );
        let fetched = probe(&journal, "fetch", json!({"reference": hit["reference"]})).unwrap();
        assert_no_leaked_keys(&fetched);
        assert_eq!(fetched["content_kind"], expected_kind, "{query}: {fetched}");
        assert_eq!(
            fetched.get("source").and_then(Value::as_str),
            expected_source,
            "{query}: {fetched}"
        );
    }

    // 3 & 6. list_transcripts, get_transcript, span, and non-speech titles.
    let list_day = probe(&journal, "list_transcripts", json!({"day": day})).unwrap();
    assert_no_leaked_keys(&list_day);
    let rows = list_day["transcripts"].as_array().unwrap();
    let row_at = |time: &str| rows.iter().find(|row| row["time"] == time).unwrap();
    for (time, expected_source, expected_kinds, speech) in [
        ("09:00-09:05", "imported file", json!(["notes"]), false),
        ("09:10-09:15", "PDF", json!(["documents"]), false),
        ("09:20-09:25", "imported file", json!(["transcripts"]), true),
        (
            "09:30-09:35",
            "imported file",
            json!(["screen_text"]),
            false,
        ),
        ("09:40-09:45", "imported file", json!(["photos"]), false),
        ("09:50-09:55", "Obsidian", json!(["notes"]), false),
        ("09:55-10:00", "Obsidian", json!(["notes"]), false),
        (
            "10:00-10:05",
            "your mac · audio",
            json!(["transcripts"]),
            true,
        ),
        (
            "10:10-10:15",
            "your mac · screen",
            json!(["screen_text"]),
            false,
        ),
        (
            "10:20-10:25",
            "imported file",
            json!(["documents", "notes"]),
            false,
        ),
    ] {
        let row = row_at(time);
        assert_eq!(row["source"], expected_source, "{time}: {row}");
        assert_eq!(row["content_kinds"], expected_kinds, "{time}: {row}");
        let title = row["title"].as_str().unwrap();
        if speech {
            assert!(title.starts_with("Transcript segment"), "{time}: {title}");
        } else {
            assert!(!title.starts_with("Transcript segment"), "{time}: {title}");
        }
        let read = probe(
            &journal,
            "get_transcript",
            json!({"reference": row["reference"]}),
        )
        .unwrap();
        assert_no_leaked_keys(&read);
        assert_eq!(read["source"], expected_source, "{time}: {read}");
        assert_eq!(read["content_kinds"], expected_kinds, "{time}: {read}");
    }

    let span = probe(
        &journal,
        "get_transcript",
        json!({"reference": list_day["span_reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&span);
    for section in span["segments"].as_array().unwrap() {
        assert!(section["content_kinds"].is_array(), "{section}");
        assert!(section["source"].is_string(), "{section}");
    }

    // Body source list and span carry ["body"].
    let list_body = probe(
        &journal,
        "list_transcripts",
        json!({"day": day, "source": "Apple Health · imported"}),
    )
    .unwrap();
    assert_no_leaked_keys(&list_body);
    assert_eq!(
        list_body["transcripts"][0]["content_kinds"],
        json!(["body"])
    );
    let span_body = probe(
        &journal,
        "get_transcript",
        json!({"reference": list_body["span_reference"]}),
    )
    .unwrap();
    assert_no_leaked_keys(&span_body);
    assert_eq!(span_body["content_kinds"], json!(["body"]));

    // 4. Activity source units carry kind-specific channel words and split two imported kinds.
    let activities = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    assert_no_leaked_keys(&activities);
    let act_rows = activities["activities"].as_array().unwrap();
    let act_ref =
        |title: &str| act_rows.iter().find(|a| a["title"] == title).unwrap()["reference"].clone();
    for act in act_rows {
        let expected = if act["source"] == "body" {
            "body"
        } else {
            "facets"
        };
        assert_eq!(act["content_kind"], expected, "{act}");
    }
    for (title, channel, kind, content_kind, source) in [
        (
            "Field Note Review",
            "Note",
            "import",
            "notes",
            "imported file",
        ),
        (
            "Pump Manual Review",
            "Document",
            "import",
            "documents",
            "PDF",
        ),
        (
            "Imported Chat",
            "Imported Transcript",
            "import",
            "transcripts",
            "imported file",
        ),
        (
            "Receipt Check",
            "Screenshot",
            "import",
            "screen_text",
            "imported file",
        ),
        (
            "Garden Shed Photo",
            "Photo",
            "import",
            "photos",
            "imported file",
        ),
        ("Trail Planning Note", "Note", "import", "notes", "Obsidian"),
        (
            "Sunrise Packing Call",
            "Transcript",
            "audio",
            "transcripts",
            "your mac · audio",
        ),
    ] {
        let sources = probe(
            &journal,
            "get_activity_sources",
            json!({"reference": act_ref(title)}),
        )
        .unwrap();
        assert_no_leaked_keys(&sources);
        let unit = &sources["units"][0];
        assert_eq!(unit["kind"], kind, "{title}: {unit}");
        assert_eq!(unit["content_kind"], content_kind, "{title}: {unit}");
        assert_eq!(unit["source_link"]["source"], source, "{title}: {unit}");
        let text = literal_display_text(unit["text"].as_str().unwrap());
        assert!(
            text.contains(&format!("Source channel: {channel}\n")),
            "{title} expected Source channel: {channel} in {text}"
        );
        assert!(
            !text.contains("Imported Text"),
            "{title} still said Imported Text: {text}"
        );
    }

    // Two imported kinds in one segment split into two source units.
    let two_sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref("Combined Import")}),
    )
    .unwrap();
    assert_no_leaked_keys(&two_sources);
    let units = two_sources["units"].as_array().unwrap();
    assert_eq!(units.len(), 2, "{two_sources}");
    assert_eq!(units[0]["content_kind"], "documents");
    assert_eq!(units[0]["kind"], "import");
    assert!(units[0]["text"].as_str().unwrap().contains("### Document"));
    assert_eq!(units[1]["content_kind"], "notes");
    assert_eq!(units[1]["kind"], "import");
    assert!(units[1]["text"].as_str().unwrap().contains("### Note"));

    // Body activity fetch and get_activity_sources carry content_kind: "body".
    let body_fetch = probe(&journal, "fetch", json!({"reference": act_ref("Running")})).unwrap();
    assert_no_leaked_keys(&body_fetch);
    assert_eq!(body_fetch["content_kind"], "body");
    assert!(body_fetch.get("source").is_none());
    let body_sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": act_ref("Running")}),
    )
    .unwrap();
    assert_no_leaked_keys(&body_sources);
    assert_eq!(body_sources["units"][0]["content_kind"], "body");

    // 7. get_week activity and body rows carry content_kind; memories do not.
    let week = probe(&journal, "get_week", json!({"week": day})).unwrap();
    assert_no_leaked_keys(&week);
    for act in week["activities"].as_array().unwrap() {
        assert_eq!(act["content_kind"], "facets", "{act}");
    }
    assert_eq!(week["body"][0]["content_kind"], "body");
    for mem in week["memories"].as_array().unwrap() {
        assert!(mem.get("content_kind").is_none(), "{mem}");
    }
}

#[test]
fn shared_import_source_labels_narrow_across_streams_and_disambiguate_by_kind() {
    let journal = base_activity_journal();
    let root = journal.path();
    let day = "20260408";
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".into(), "notes".into(), "photos".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    fs::create_dir_all(root.join("streams")).unwrap();
    for (idx, (stream, kind, cid, source)) in [
        ("import.text", "import", None, None),
        ("import.image", "import", None, None),
        ("import.obsidian", "import", None, None),
        (
            "mac_obsidian",
            "observer",
            Some("sha256:m1"),
            Some("obsidian"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut obj = json!({
            "name": stream,
            "kind": kind,
            "host": null,
            "platform": null,
            "created_at": (idx as u64) + 1,
            "last_day": day,
            "last_segment": null,
            "seq": 1
        });
        if let Some(cid) = cid {
            obj["cid"] = json!(cid);
        }
        if let Some(source) = source {
            obj["source"] = json!(source);
        }
        fs::write(root.join(format!("streams/{stream}.json")), obj.to_string()).unwrap();
    }

    // Same timestamp 09:00-09:05 on both import.text and import.image.
    write_segment_file(
        root,
        day,
        "import.text",
        "090000_300",
        "alpha",
        "note_transcript.md",
        "# Same-Time Note\n\nNote body.\n",
    );
    write_segment_file(
        root,
        day,
        "import.image",
        "090000_300",
        "alpha",
        "image_transcript.md",
        "# Same-Time Photo\n\nPhoto body.\n",
    );
    write_segment_file(
        root,
        day,
        "import.obsidian",
        "100000_300",
        "alpha",
        "note_transcript.md",
        "# Imported Vault Note\n\nVault body.\n",
    );
    write_segment_file(
        root,
        day,
        "mac_obsidian",
        "100500_300",
        "alpha",
        "note_transcript.md",
        "# Synced Vault Note\n\nSynced body.\n",
    );

    // Narrowing by "imported file" matches both import.text and import.image,
    // and content_kinds + reference distinguish the two same-time segments.
    let imported = probe(
        &journal,
        "list_transcripts",
        json!({"day": day, "source": "imported file"}),
    )
    .unwrap();
    assert_no_leaked_keys(&imported);
    let rows = imported["transcripts"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{imported}");
    assert!(rows.iter().all(|r| r["source"] == "imported file"));
    let kinds: std::collections::BTreeSet<&str> = rows
        .iter()
        .map(|r| r["content_kinds"][0].as_str().unwrap())
        .collect();
    assert_eq!(kinds, std::collections::BTreeSet::from(["notes", "photos"]));
    assert_ne!(rows[0]["reference"], rows[1]["reference"]);
    assert_ne!(rows[0]["title"], rows[1]["title"]);

    let span = probe(
        &journal,
        "get_transcript",
        json!({"reference": imported["span_reference"]}),
    )
    .unwrap();
    let sections = span["segments"].as_array().unwrap();
    assert_eq!(sections.len(), 2, "{span}");
    let span_kinds: std::collections::BTreeSet<&str> = sections
        .iter()
        .map(|s| s["content_kinds"][0].as_str().unwrap())
        .collect();
    assert_eq!(span_kinds, kinds);

    // Narrowing by "Obsidian" matches both import.obsidian and bound mac_obsidian.
    let obsidian = probe(
        &journal,
        "list_transcripts",
        json!({"day": day, "source": "Obsidian"}),
    )
    .unwrap();
    assert_no_leaked_keys(&obsidian);
    let obs_rows = obsidian["transcripts"].as_array().unwrap();
    assert_eq!(obs_rows.len(), 2, "{obsidian}");
    assert!(obs_rows.iter().all(|r| r["source"] == "Obsidian"));
}

#[test]
fn transcripts_only_connection_never_sees_unpermitted_kinds_or_sources() {
    use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows};

    let journal = base_activity_journal();
    let root = journal.path();
    let day = "20260409";

    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    fs::create_dir_all(root.join("streams")).unwrap();
    for (idx, (stream, kind, cid, source)) in [
        ("macos_audio", "observer", Some("sha256:m1"), Some("audio")),
        ("import.obsidian", "import", None, None),
        ("import.document", "import", None, None),
        ("import.image", "import", None, None),
        ("import.apple_health", "import", None, None),
    ]
    .into_iter()
    .enumerate()
    {
        let mut obj = json!({
            "name": stream,
            "kind": kind,
            "host": null,
            "platform": None::<&str>,
            "created_at": (idx as u64) + 1,
            "last_day": day,
            "last_segment": null,
            "seq": 1
        });
        if let Some(cid) = cid {
            obj["cid"] = json!(cid);
        }
        if let Some(source) = source {
            obj["source"] = json!(source);
        }
        fs::write(root.join(format!("streams/{stream}.json")), obj.to_string()).unwrap();
    }

    // Mixed segment holding speech alongside a note and a document.
    write_segment_file(
        root,
        day,
        "macos_audio",
        "100000_300",
        "alpha",
        "audio.jsonl",
        "{\"source\":\"recording\"}\n{\"start\":\"10:00:05\",\"text\":\"isolationneedle spoken line\"}\n",
    );
    fs::write(
        root.join(format!(
            "chronicle/{day}/macos_audio/100000_300/note_transcript.md"
        )),
        "isolationneedle hidden note\n",
    )
    .unwrap();
    fs::write(
        root.join(format!(
            "chronicle/{day}/macos_audio/100000_300/document_transcript.md"
        )),
        "isolationneedle hidden document\n",
    )
    .unwrap();
    fs::write(
        root.join(format!(
            "chronicle/{day}/macos_audio/100000_300/talents/topics.md"
        )),
        "isolationneedle topic summary\n",
    )
    .unwrap();
    fs::write(
        root.join(format!(
            "chronicle/{day}/macos_audio/100000_300/stream.json"
        )),
        json!({"stream": "macos_audio"}).to_string(),
    )
    .unwrap();

    // Unpermitted-kind segments under distinct sources.
    for (stream, seg, file, body) in [
        (
            "import.obsidian",
            "090000_300",
            "note_transcript.md",
            "isolationneedle obsidian note\n",
        ),
        (
            "import.document",
            "091000_300",
            "document_transcript.md",
            "isolationneedle pdf doc\n",
        ),
        (
            "import.image",
            "092000_300",
            "image_transcript.md",
            "isolationneedle photo text\n",
        ),
    ] {
        write_segment_file(root, day, stream, seg, "alpha", file, body);
        fs::write(
            root.join(format!("chronicle/{day}/{stream}/{seg}/stream.json")),
            json!({"stream": stream}).to_string(),
        )
        .unwrap();
    }
    let mut b_row = BodyRow::new(
        RowKind::Reading,
        "r1".to_owned(),
        "apple_health",
        "HKQuantityTypeIdentifierHeartRate",
        "heart rate".to_owned(),
    );
    b_row.at = Some("2026-04-09 08:00:00 -0400".to_owned());
    b_row.value = Some("72".to_owned());
    let b_dir = root.join(format!("chronicle/{day}/import.apple_health/080000_3600"));
    fs::create_dir_all(b_dir.join("talents")).unwrap();
    fs::write(b_dir.join("body.jsonl"), encode_rows(&[b_row])).unwrap();
    fs::write(
        b_dir.join("stream.json"),
        json!({"stream": "import.apple_health"}).to_string(),
    )
    .unwrap();
    fs::write(b_dir.join("talents/facets.json"), r#"[{"facet":"alpha"}]"#).unwrap();

    write_activity_line(
        root,
        "alpha",
        day,
        "act_mixed",
        "Mixed Segment Activity",
        100,
        Some("conversation"),
        Some(&["100000_300"]),
        None,
        None,
        Some("macos_audio"),
        None,
        None,
    );

    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();

    let assert_no_foreign_kind_or_source = |label: &str, value: &Value| {
        assert_no_leaked_keys(value);
        let serialized = value.to_string();
        for forbidden in [
            "Obsidian",
            "\"PDF\"",
            "imported file",
            "Apple Health",
            "\"notes\"",
            "\"documents\"",
            "\"photos\"",
            "\"screen_text\"",
            "hidden note",
            "hidden document",
            "topic summary",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "{label} leaked {forbidden} in {serialized}"
            );
        }
    };

    // search + fetch
    let search = probe(
        &journal,
        "search",
        json!({"query": "isolationneedle", "day": day}),
    )
    .unwrap();
    assert_no_foreign_kind_or_source("search", &search);
    let hits = search["results"].as_array().unwrap();
    assert!(hits.is_empty(), "{search}");

    // list_transcripts + get_transcript (single and span)
    let listed = probe(&journal, "list_transcripts", json!({"day": day})).unwrap();
    assert_no_foreign_kind_or_source("list_transcripts", &listed);
    let segs = listed["transcripts"].as_array().unwrap();
    assert_eq!(segs.len(), 1, "{listed}");
    assert_eq!(segs[0]["content_kinds"], json!(["transcripts"]));
    assert_eq!(segs[0]["lines"], 1);

    let single = probe(
        &journal,
        "get_transcript",
        json!({"reference": segs[0]["reference"]}),
    )
    .unwrap();
    assert_no_foreign_kind_or_source("get_transcript", &single);
    assert_eq!(single["content_kinds"], json!(["transcripts"]));
    assert_eq!(single["entries"].as_array().unwrap().len(), 1);

    let span = probe(
        &journal,
        "get_transcript",
        json!({"reference": listed["span_reference"]}),
    )
    .unwrap();
    assert_no_foreign_kind_or_source("get_transcript span", &span);
    assert_eq!(span["segments"].as_array().unwrap().len(), 1);
    assert_eq!(span["segments"][0]["content_kinds"], json!(["transcripts"]));

    // With facets + transcripts (still no notes/documents/photos/body):
    // list_activities, get_activity_sources, and get_week never leak the hidden kinds or sources.
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".into(), "facets".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let acts = probe(&journal, "list_activities", json!({"day": day})).unwrap();
    assert_no_foreign_kind_or_source("list_activities", &acts);
    assert_eq!(acts["activities"][0]["content_kind"], "facets");

    let sources = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": acts["activities"][0]["reference"]}),
    )
    .unwrap();
    assert_no_foreign_kind_or_source("get_activity_sources", &sources);
    assert_eq!(sources["units"].as_array().unwrap().len(), 1);
    assert_eq!(sources["units"][0]["content_kind"], "transcripts");
    assert_eq!(sources["units"][0]["kind"], "audio");

    let week = probe(&journal, "get_week", json!({"week": day})).unwrap();
    assert_no_foreign_kind_or_source("get_week", &week);
    assert_eq!(week["activities"][0]["content_kind"], "facets");
    assert!(week.get("body").is_none());
}

#[test]
fn get_activity_sources_keeps_embedded_headings_in_one_unit_and_splits_real_kinds_and_body_segments()
 {
    use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows};

    let journal = base_activity_journal();
    let root = journal.path();
    let day = "20260407";

    fs::create_dir_all(root.join("streams")).unwrap();
    for (idx, stream) in ["import.text", "import.chatgpt", "import.apple_health"]
        .into_iter()
        .enumerate()
    {
        fs::write(
            root.join(format!("streams/{stream}.json")),
            json!({
                "name": stream,
                "kind": "import",
                "host": null,
                "platform": null,
                "created_at": (idx as u64) + 1,
                "last_day": day,
                "last_segment": null,
                "seq": 1
            })
            .to_string(),
        )
        .unwrap();
    }

    // 1. A single note containing headings matching both the CHANNEL and SEGMENT patterns.
    let trip_note = concat!(
        "# Trip log\n",
        "### Photo\n",
        "the one by the lake, need to print it\n",
        "### Document\n",
        "passport renewal form is in the drawer\n",
        "### Screenshot\n",
        "saved the booking confirmation\n",
        "## 2026-04-07 10:00:00 - 10:05:00\n",
        "### Budget\n",
        "about 400\n",
    );
    write_segment_file(
        root,
        day,
        "import.text",
        "090000_300",
        "alpha",
        "note_transcript.md",
        trip_note,
    );
    fs::write(
        root.join(format!(
            "chronicle/{day}/import.text/090000_300/stream.json"
        )),
        json!({"stream": "import.text"}).to_string(),
    )
    .unwrap();

    // 2. A note containing older channel headings and an explicit Source channel line.
    let older_headings_note = concat!(
        "# Notes on headings\n",
        "### Note\n",
        "first section\n",
        "### Transcript\n",
        "second section\n",
        "### Highlights\n",
        "third section\n",
        "### Body Records\n",
        "fourth section\n",
        "Source channel: Photo\n",
    );
    write_segment_file(
        root,
        day,
        "import.text",
        "091000_300",
        "alpha",
        "note_transcript.md",
        older_headings_note,
    );
    fs::write(
        root.join(format!(
            "chronicle/{day}/import.text/091000_300/stream.json"
        )),
        json!({"stream": "import.text"}).to_string(),
    )
    .unwrap();

    // 3. Multiline imported chat message whose later lines look like channel and segment headings.
    write_segment_file(
        root,
        day,
        "import.chatgpt",
        "092000_300",
        "alpha",
        "conversation_transcript.jsonl",
        concat!(
            "{\"untimed\":true,\"imported\":{}}\n",
            "{\"start\":\"00:00:00\",\"speaker\":\"Ana\",\"text\":\"First line\\n### Photo\\nnot a photo\\n## 2026-04-07 10:00:00 - 10:05:00\\nstill chat\"}\n",
        ),
    );
    fs::write(
        root.join(format!(
            "chronicle/{day}/import.chatgpt/092000_300/stream.json"
        )),
        json!({"stream": "import.chatgpt"}).to_string(),
    )
    .unwrap();

    // 4. A real two-kind segment whose note also contains a ### Document and segment heading.
    write_segment_file(
        root,
        day,
        "import.text",
        "102000_300",
        "alpha",
        "document_transcript.md",
        "# Pump Manual\nCheck valve pressure.\n",
    );
    fs::write(
        root.join(format!(
            "chronicle/{day}/import.text/102000_300/note_transcript.md"
        )),
        "# Field Notes\n### Document\nstill part of the note\n## 2026-04-07 11:00:00 - 11:05:00\nend of note\n",
    )
    .unwrap();
    fs::write(
        root.join(format!(
            "chronicle/{day}/import.text/102000_300/stream.json"
        )),
        json!({"stream": "import.text"}).to_string(),
    )
    .unwrap();

    // 5. A body-only window segment referenced by an activity.
    let mut hr_row = BodyRow::new(
        RowKind::Reading,
        "hr1".to_owned(),
        "apple_health",
        "HKQuantityTypeIdentifierHeartRate",
        "heart rate".to_owned(),
    );
    hr_row.at = Some("2026-04-07 07:15:00 -0400".to_owned());
    hr_row.value = Some("64 bpm".to_owned());
    let body_dir = root.join(format!("chronicle/{day}/import.apple_health/070000_3600"));
    fs::create_dir_all(body_dir.join("talents")).unwrap();
    fs::write(body_dir.join("body.jsonl"), encode_rows(&[hr_row])).unwrap();
    fs::write(
        body_dir.join("stream.json"),
        json!({"stream": "import.apple_health"}).to_string(),
    )
    .unwrap();
    fs::write(
        body_dir.join("talents/facets.json"),
        r#"[{"facet":"alpha"}]"#,
    )
    .unwrap();

    for (idx, (id, title, kind, seg, stream)) in [
        ("act_trip", "Trip Log", "note", "090000_300", "import.text"),
        (
            "act_older",
            "Heading Notes",
            "note",
            "091000_300",
            "import.text",
        ),
        (
            "act_chat",
            "Chat With Headings",
            "conversation",
            "092000_300",
            "import.chatgpt",
        ),
        (
            "act_two",
            "Combined Segment",
            "import",
            "102000_300",
            "import.text",
        ),
        (
            "act_body_seg",
            "Morning Heart Rate",
            "health",
            "070000_3600",
            "import.apple_health",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        write_activity_line(
            root,
            "alpha",
            day,
            id,
            title,
            100 + idx as i64,
            Some(kind),
            Some(&[seg]),
            None,
            None,
            Some(stream),
            None,
            None,
        );
    }

    // Permissions are unchanged: a connection holding only `notes` (or `facets` + `notes`)
    // cannot invoke get_activity_sources (which requires facets + transcripts/body).
    for categories in [vec!["notes"], vec!["facets", "notes"]] {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: categories.iter().copied().map(str::to_owned).collect(),
                    scope: ReadScope::WholeJournal,
                },
            )
            .unwrap();
        assert!(
            probe(
                &journal,
                "get_activity_sources",
                json!({"reference": format!("{day}/alpha/act_trip")}),
            )
            .is_err()
        );
    }

    // Narrowest grants that can call get_activity_sources on a note without holding
    // photos, documents or screen_text: facets + body + notes (also excludes transcripts)
    // and facets + transcripts + notes (also excludes body).
    for (categories, forbidden) in [
        (
            vec!["facets", "body", "notes"],
            vec![
                "\"photos\"",
                "\"documents\"",
                "\"screen_text\"",
                "\"transcripts\"",
            ],
        ),
        (
            vec!["facets", "transcripts", "notes"],
            vec!["\"photos\"", "\"documents\"", "\"screen_text\"", "\"body\""],
        ),
    ] {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: categories.iter().copied().map(str::to_owned).collect(),
                    scope: ReadScope::WholeJournal,
                },
            )
            .unwrap();
        let res = probe(
            &journal,
            "get_activity_sources",
            json!({"reference": format!("{day}/alpha/act_trip")}),
        )
        .unwrap();
        assert_no_leaked_keys(&res);
        let serialized = res.to_string();
        for word in &forbidden {
            assert!(
                !serialized.contains(word),
                "unexpected kind {word} in {serialized}"
            );
        }
        let units = res["units"].as_array().unwrap();
        assert_eq!(units.len(), 1, "{res}");
        assert_eq!(units[0]["kind"], "import");
        assert_eq!(units[0]["content_kind"], "notes");
        assert_eq!(units[0]["timestamp"], "2026-04-07 09:00:00 - 09:05:00");
        assert_eq!(units[0]["source_link"]["source"], "imported file");
        assert_eq!(
            units[0]["text"].as_str().unwrap(),
            format!(
                "Activity type: note\n## 2026-04-07 09:00:00 - 09:05:00\n\nSource channel: Note\n### Note\n\n{trip_note}"
            )
        );

        let older = probe(
            &journal,
            "get_activity_sources",
            json!({"reference": format!("{day}/alpha/act_older")}),
        )
        .unwrap();
        assert_no_leaked_keys(&older);
        let older_units = older["units"].as_array().unwrap();
        assert_eq!(older_units.len(), 1, "{older}");
        assert_eq!(older_units[0]["kind"], "import");
        assert_eq!(older_units[0]["content_kind"], "notes");
        assert_eq!(
            older_units[0]["text"].as_str().unwrap(),
            format!(
                "Activity type: note\n## 2026-04-07 09:10:00 - 09:15:00\n\nSource channel: Note\n### Note\n\n{older_headings_note}"
            )
        );
    }

    // Multiline imported chat message with embedded headings stays one untimed unit.
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "facets".into(),
                    "transcripts".into(),
                    "notes".into(),
                    "documents".into(),
                ],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let withheld = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": format!("{day}/alpha/act_chat")}),
    )
    .unwrap();
    assert!(
        withheld["units"].as_array().unwrap().is_empty(),
        "{withheld}"
    );
    let store = PermissionStore::open(root);
    let mut approved = store
        .get_permission(CONNECTION)
        .unwrap()
        .unwrap()
        .read
        .unwrap();
    approved.categories.push("messages".into());
    store.set_permission(CONNECTION, approved).unwrap();
    let chat = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": format!("{day}/alpha/act_chat")}),
    )
    .unwrap();
    assert_no_leaked_keys(&chat);
    let chat_units = chat["units"].as_array().unwrap();
    assert_eq!(chat_units.len(), 1, "{chat}");
    assert_eq!(chat_units[0]["kind"], "import");
    assert_eq!(chat_units[0]["content_kind"], "messages");
    assert_eq!(chat_units[0]["untimed"], true);
    assert!(chat_units[0].get("timestamp").is_none());
    let chat_text = literal_display_text(chat_units[0]["text"].as_str().unwrap());
    assert!(chat_text.contains("Source channel: ChatGPT Conversation\n"));
    assert!(chat_text.contains("not a photo"));
    assert!(chat_text.contains("still chat"));

    // Real two-kind segment splits into exactly two units with the right kinds,
    // while keeping the note's own ### Document and segment heading inside the note unit.
    let two = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": format!("{day}/alpha/act_two")}),
    )
    .unwrap();
    assert_no_leaked_keys(&two);
    let two_units = two["units"].as_array().unwrap();
    assert_eq!(two_units.len(), 2, "{two}");
    assert_eq!(two_units[0]["kind"], "import");
    assert_eq!(two_units[0]["content_kind"], "documents");
    assert_eq!(two_units[0]["timestamp"], "2026-04-07 10:20:00 - 10:25:00");
    assert_eq!(
        two_units[0]["text"].as_str().unwrap(),
        "Activity type: import\n## 2026-04-07 10:20:00 - 10:25:00\n\nSource channel: Document\n### Document\n\n# Pump Manual\nCheck valve pressure.\n"
    );
    assert_eq!(two_units[1]["kind"], "import");
    assert_eq!(two_units[1]["content_kind"], "notes");
    assert_eq!(two_units[1]["timestamp"], "2026-04-07 10:20:00 - 10:25:00");
    assert_eq!(
        two_units[1]["text"].as_str().unwrap(),
        "Activity type: import\n## 2026-04-07 10:20:00 - 10:25:00\n\nSource channel: Note\n### Note\n\n# Field Notes\n### Document\nstill part of the note\n## 2026-04-07 11:00:00 - 11:05:00\nend of note\n"
    );

    // Body-only segment referenced by an activity yields one Body Records unit with content_kind "body".
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["facets".into(), "body".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let body = probe(
        &journal,
        "get_activity_sources",
        json!({"reference": format!("{day}/alpha/act_body_seg")}),
    )
    .unwrap();
    assert_no_leaked_keys(&body);
    let body_units = body["units"].as_array().unwrap();
    assert_eq!(body_units.len(), 1, "{body}");
    assert_eq!(body_units[0]["kind"], "import");
    assert_eq!(body_units[0]["content_kind"], "body");
    assert_eq!(body_units[0]["timestamp"], "2026-04-07 07:00:00 - 08:00:00");
    let body_text = literal_display_text(body_units[0]["text"].as_str().unwrap());
    assert!(
        body_text.contains("Source channel: Body Records\n"),
        "{body_text}"
    );
    assert!(body_text.contains("heart rate"), "{body_text}");
    assert!(body_text.contains("64 bpm"), "{body_text}");
}

#[test]
fn photo_week_summaries_preserve_many_photos_chronological_order_times_and_clean_punctuation() {
    let journal = fixture();
    let root = journal.path();

    let write_photo = |day: &str, seg: &str, title: &str, raw_desc: &str, meta: Option<&str>| {
        let dir = root.join(format!("chronicle/{day}/import.image/{seg}"));
        fs::create_dir_all(&dir).unwrap();
        let escaped = solstone_core_format::markdown::literal_text(raw_desc);
        let date = format!("{}-{}-{}", &day[0..4], &day[4..6], &day[6..8]);
        let doc = format!(
            "# {title}\n\n**Type:** Image\n**Format:** JPEG\n**Dimensions:** 1200×896\n**Date:** {date}\n\n---\n\n> [image description — model-derived]\n> {escaped}\n"
        );
        fs::write(dir.join("image_transcript.md"), doc).unwrap();
        if let Some(meta) = meta {
            fs::write(dir.join("image_meta.json"), meta).unwrap();
        }
    };

    // Day 1 (20260308): 13 capture photos + 1 owner-timed photo + 1 upload_fallback photo = 15.
    // Plus 1 imported screenshot that must stay excluded from photos.
    let times = [
        ("061500_0", "Dawn sill", "capture"),
        ("071500_0", "Second light", "capture"),
        ("084700_0", "Street walk", "capture"),
        ("101500_0", "Desk ledger", "capture"),
        ("111500_0", "Midday court", "owner"),
        ("134400_0", "River ferry", "capture"),
        ("141200_0", "Bookstall", "capture"),
        ("150500_0", "Rain eaves", "capture"),
        ("162800_0", "Lantern oil", "capture"),
        ("163600_0", "Timber arch", "capture"),
        ("164600_0", "Church bells", "capture"),
        ("204000_0", "Hearth coals", "capture"),
        ("215200_0", "Candle out", "capture"),
        ("090000_0", "Bridge arch", "capture"),
    ];
    for (seg, title, source) in times {
        let desc = if seg == "061500_0" {
            "The desk at first light: pounce-box, other men's shoulders."
        } else {
            "A clear autumn frame, sharp and bright."
        };
        let meta = format!(r#"{{"schema":1,"time_source":"{source}","utc_offset_seconds":0}}"#);
        write_photo("20260308", seg, title, desc, Some(&meta));
    }
    write_photo(
        "20260308",
        "120000_0",
        "Undated print",
        "An undated scan, placed at import time.",
        Some(r#"{"schema":1,"time_source":"upload_fallback"}"#),
    );
    let shot_dir = root.join("chronicle/20260308/import.image/123000_0");
    fs::create_dir_all(&shot_dir).unwrap();
    fs::write(
        shot_dir.join("screenshot_transcript.md"),
        "# Desktop shot\n\n> [image description — model-derived]\n> A terminal window\\.\n",
    )
    .unwrap();

    // Day 2 (20260309): one note to verify range prefetch covers notes too.
    let note_dir = root.join("chronicle/20260309/import.text/100000_0");
    fs::create_dir_all(&note_dir).unwrap();
    fs::write(
        note_dir.join("note_transcript.md"),
        "# Packing list\n\nBoots and lantern.\n",
    )
    .unwrap();

    // Day 3 (20260312): 52 photos to exercise the 50-per-day cap and `more: true`.
    for i in 0..52 {
        let hour = 8 + i / 6;
        let minute = (i % 6) * 10;
        let seg = format!("{hour:02}{minute:02}00_0");
        write_photo(
            "20260312",
            &seg,
            &format!("Frame {i:02}"),
            "Part of a long day.",
            Some(r#"{"schema":1,"time_source":"capture"}"#),
        );
    }

    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "facets".into(),
                    "transcripts".into(),
                    "photos".into(),
                    "notes".into(),
                    "screen_text".into(),
                ],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    for args in [
        json!({"week": "2026-03-08", "include": ["photos", "notes"]}),
        json!({"day_from": "2026-03-08", "day_to": "2026-03-14", "include": ["photos", "notes"]}),
    ] {
        let week = probe(&journal, "get_week", args.clone()).unwrap();
        let days = week["days"].as_array().unwrap();
        let day_row = |d: &str| days.iter().find(|row| row["day"] == d).unwrap();

        let d1 = &day_row("20260308")["summary"]["photos"];
        assert_eq!(d1["count"], 15, "{args}: {d1}");
        assert!(d1.get("more").is_none(), "{d1}");
        let items = d1["items"].as_array().unwrap();
        assert_eq!(items.len(), 15);
        assert!(
            items.iter().all(|item| item["title"] != "Desktop shot"),
            "screenshot leaked into photos"
        );
        // Earliest to latest across the 14 timed photos, then the upload_fallback photo last.
        assert_eq!(items[0]["title"], "Dawn sill");
        assert_eq!(items[0]["time"], "06:15:00");
        assert_eq!(items[0]["time_source"], "capture");
        assert_eq!(
            items[0]["description"],
            "The desk at first light: pounce-box, other men's shoulders."
        );
        assert_eq!(items[3]["title"], "Bridge arch");
        assert_eq!(items[3]["time"], "09:00:00");
        assert_eq!(items[5]["title"], "Midday court");
        assert_eq!(items[5]["time"], "11:15:00");
        assert_eq!(items[5]["time_source"], "owner");
        assert_eq!(items[13]["title"], "Candle out");
        assert_eq!(items[13]["time"], "21:52:00");
        assert_eq!(items[14]["title"], "Undated print");
        assert!(items[14].get("time").is_none(), "{:?}", items[14]);
        assert_eq!(items[14]["time_source"], "upload_fallback");

        let d2_notes = &day_row("20260309")["summary"]["notes"];
        assert_eq!(d2_notes["count"], 1);
        assert_eq!(d2_notes["items"][0]["title"], "Packing list");

        let d_empty = &day_row("20260310")["summary"]["photos"];
        assert_eq!(d_empty["count"], 0);
        assert_eq!(d_empty["items"], json!([]));

        let d3 = &day_row("20260312")["summary"]["photos"];
        assert_eq!(d3["count"], 52, "{d3}");
        assert_eq!(d3["more"], true, "{d3}");
        let d3_items = d3["items"].as_array().unwrap();
        assert_eq!(d3_items.len(), crate::dispatch::MAX_WEEK_PHOTOS_PER_DAY);
        assert_eq!(d3_items[0]["title"], "Frame 00");
        assert_eq!(d3_items[0]["time"], "08:00:00");
        assert_eq!(d3_items[49]["title"], "Frame 49");
    }

    // search(day, kind=["photos"]) also returns chronological order, clean punctuation, and time fields.
    let searched = probe(
        &journal,
        "search",
        json!({"day": "2026-03-08", "kind": ["photos"], "limit": 50}),
    )
    .unwrap();
    let rows = searched["results"].as_array().unwrap();
    assert_eq!(rows.len(), 15, "{searched}");
    assert_eq!(rows[0]["title"], "Dawn sill");
    assert_eq!(rows[0]["time"], "06:15:00");
    assert_eq!(rows[0]["time_source"], "capture");
    assert_eq!(
        rows[0]["description"],
        "The desk at first light: pounce-box, other men's shoulders."
    );
    assert_eq!(rows[13]["title"], "Candle out");
    assert_eq!(rows[13]["time"], "21:52:00");
    assert_eq!(rows[14]["title"], "Undated print");
    assert!(rows[14].get("time").is_none());
    assert_eq!(rows[14]["time_source"], "upload_fallback");

    // fetch returns structured time + time_source on capture photos and omits time on upload_fallback.
    let fetched_capture = probe(
        &journal,
        "fetch",
        json!({"reference": rows[0]["reference"]}),
    )
    .unwrap();
    assert_eq!(fetched_capture["time"], "06:15:00");
    assert_eq!(fetched_capture["time_source"], "capture");

    let fetched_fallback = probe(
        &journal,
        "fetch",
        json!({"reference": rows[14]["reference"]}),
    )
    .unwrap();
    assert!(fetched_fallback.get("time").is_none(), "{fetched_fallback}");
    assert_eq!(fetched_fallback["time_source"], "upload_fallback");
}

#[test]
fn fetch_size_returns_thumbnails_for_single_and_batched_photos_and_screenshots() {
    let journal = fixture();
    let root = journal.path();
    let day = "20260308";

    // Build a patterned 1200x900 RGB image so full vs thumb JPEG sizes differ markedly.
    let pattern_png = |shift: u8| {
        let mut img = image::RgbImage::new(1200, 900);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            let r = ((x.wrapping_mul(7) ^ y.wrapping_mul(13)) as u8).wrapping_add(shift);
            let g = ((x / 3 + y / 5) as u8).wrapping_add(shift);
            let b = ((x ^ y) as u8).wrapping_mul(3);
            *pixel = image::Rgb([r, g, b]);
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    };

    let p1 = root.join(format!("chronicle/{day}/import.image/080000_0"));
    let p2 = root.join(format!("chronicle/{day}/import.image/090000_0"));
    let shot = root.join(format!("chronicle/{day}/import.image/100000_0"));
    let note = root.join(format!("chronicle/{day}/import.text/110000_0"));
    for d in [&p1, &p2, &shot, &note] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(
        p1.join("image_transcript.md"),
        "# First photo\n\n> [image description — model-derived]\n> First\\.\n",
    )
    .unwrap();
    fs::write(p1.join("original.png"), pattern_png(10)).unwrap();
    fs::write(
        p2.join("image_transcript.md"),
        "# Second photo\n\n> [image description — model-derived]\n> Second\\.\n",
    )
    .unwrap();
    fs::write(p2.join("original.png"), pattern_png(90)).unwrap();
    fs::write(
        shot.join("screenshot_transcript.md"),
        "# Shot\n\n> [image description — model-derived]\n> Shot\\.\n",
    )
    .unwrap();
    fs::write(shot.join("original.screenshot.png"), pattern_png(170)).unwrap();
    fs::write(
        note.join("note_transcript.md"),
        "# Plain note\n\nJust text.\n",
    )
    .unwrap();

    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();
    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "photos".into(),
                    "screen_text".into(),
                    "screenshots".into(),
                    "notes".into(),
                    "transcripts".into(),
                ],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let photo_rows = probe(
        &journal,
        "search",
        json!({"day": day, "kind": ["photos"], "limit": 10}),
    )
    .unwrap();
    let ref1 = photo_rows["results"][0]["reference"].clone();
    let ref2 = photo_rows["results"][1]["reference"].clone();
    let shot_rows = probe(
        &journal,
        "search",
        json!({"day": day, "query": "Shot", "kind": ["screen_text"]}),
    )
    .unwrap();
    let ref_shot = shot_rows["results"][0]["reference"].clone();
    let note_rows = probe(&journal, "search", json!({"day": day, "kind": ["notes"]})).unwrap();
    let ref_note = note_rows["results"][0]["reference"].clone();

    let full_default = dispatch_output(&journal, "fetch", json!({"reference": ref1}));
    assert_eq!(full_default.value["picture"]["state"], "attached");
    assert_eq!(full_default.value["picture"]["size"], "full");
    assert_eq!(full_default.value["picture"]["width"], 1024);
    assert_eq!(full_default.value["picture"]["height"], 768);
    let full_bytes = full_default.images[0].len();

    let thumb_single = dispatch_output(
        &journal,
        "fetch",
        json!({"reference": ref1, "size": "thumb"}),
    );
    assert_eq!(thumb_single.value["picture"]["state"], "attached");
    assert_eq!(thumb_single.value["picture"]["size"], "thumb");
    assert_eq!(thumb_single.value["picture"]["width"], 320);
    assert_eq!(thumb_single.value["picture"]["height"], 240);
    let decoded = image::load_from_memory(&thumb_single.images[0]).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (320, 240));
    assert!(
        thumb_single.images[0].len() * 3 < full_bytes,
        "thumb {} B should be far smaller than full {} B",
        thumb_single.images[0].len(),
        full_bytes
    );

    // Batched fetch with size="thumb" returns thumbnails for both photos and screenshots and leaves notes intact.
    let full_batch = dispatch_output(
        &journal,
        "fetch",
        json!({"references": [&ref1, &ref2, &ref_shot, &ref_note]}),
    );
    let thumb_batch = dispatch_output(
        &journal,
        "fetch",
        json!({"references": [&ref1, &ref2, &ref_shot, &ref_note], "size": "thumb"}),
    );
    assert_eq!(thumb_batch.images.len(), 3);
    for idx in 0..3 {
        assert_eq!(
            thumb_batch.value["items"][idx]["result"]["picture"]["size"],
            "thumb"
        );
        assert_eq!(
            thumb_batch.value["items"][idx]["result"]["picture"]["width"],
            320
        );
        assert_eq!(
            thumb_batch.value["items"][idx]["image_content_index"],
            idx + 1
        );
    }
    assert!(
        thumb_batch.value["items"][3]["result"]["text"]
            .as_str()
            .unwrap()
            .contains("Just text.")
    );
    let thumb_batch_bytes: usize = thumb_batch.images.iter().map(Vec::len).sum();
    let full_batch_bytes: usize = full_batch.images.iter().map(Vec::len).sum();
    assert!(
        thumb_batch_bytes * 3 < full_batch_bytes,
        "thumb batch {thumb_batch_bytes} B vs full batch {full_batch_bytes} B"
    );

    // Omitted or null size defaults to full on both single and batched fetch.
    let null_single = probe(&journal, "fetch", json!({"reference": &ref1, "size": null})).unwrap();
    assert_eq!(null_single["picture"]["size"], "full");
    let null_batch = probe(
        &journal,
        "fetch",
        json!({"references": [&ref1], "size": null}),
    )
    .unwrap();
    assert_eq!(null_batch["items"][0]["result"]["picture"]["size"], "full");

    // Invalid size arguments are refused on both single and batched fetch, and never accepted on other batch tools.
    for bad in [
        json!({"reference": &ref1, "size": "huge"}),
        json!({"reference": &ref1, "size": 320}),
        json!({"references": [&ref1], "size": "huge"}),
        json!({"references": [&ref1], "size": 320}),
    ] {
        assert_eq!(
            probe(&journal, "fetch", bad.clone()),
            Err(McpProbeError::InvalidInput),
            "{bad}"
        );
    }
    assert_eq!(
        probe(
            &journal,
            "get_transcript",
            json!({"references": ["missing"], "size": "thumb"})
        ),
        Err(McpProbeError::InvalidInput)
    );
}

#[test]
fn get_week_note_and_photo_summaries_carry_source_and_note_first_line_titles() {
    use crate::memory::{AppendResult, AuthenticatedMemorySource, append_connection_memory};

    let journal = fixture();
    let root = journal.path();
    let day = "20260309";

    fs::create_dir_all(root.join("streams")).unwrap();
    fs::write(
        root.join("streams/obsidian.json"),
        json!({
            "name": "obsidian",
            "kind": "observer",
            "host": null,
            "platform": null,
            "created_at": 1,
            "last_day": null,
            "last_segment": null,
            "seq": 1,
            "cid": "sha256:obsidian-test",
            "source": "obsidian"
        })
        .to_string(),
    )
    .unwrap();
    for (idx, stream) in ["import.obsidian", "import.text", "import.image"]
        .into_iter()
        .enumerate()
    {
        fs::write(
            root.join(format!("streams/{stream}.json")),
            json!({
                "name": stream,
                "kind": "import",
                "host": null,
                "platform": null,
                "created_at": (idx as u64) + 2,
                "last_day": null,
                "last_segment": null,
                "seq": 1
            })
            .to_string(),
        )
        .unwrap();
    }

    // Obsidian note without a markdown heading: should take its first line and source "Obsidian".
    let obsidian_plain = root.join(format!("chronicle/{day}/obsidian/203000_480"));
    fs::create_dir_all(&obsidian_plain).unwrap();
    fs::write(
        obsidian_plain.join("note_transcript.md"),
        "The book. Saturday night. The columns ruled:\n\nEntry one.\n",
    )
    .unwrap();

    // Imported Obsidian note with a heading: should keep its heading and source "Obsidian".
    let obsidian_headed = root.join(format!("chronicle/{day}/import.obsidian/180000_0"));
    fs::create_dir_all(&obsidian_headed).unwrap();
    fs::write(
        obsidian_headed.join("note_transcript.md"),
        "# Ledger notes\n\nSecond line.\n",
    )
    .unwrap();

    // Agent-memory note: plain text, should take its first line and source "your agent".
    let mem_source =
        solstone_core_format::agent_memory::SourceKey::from_verified_id("bearer:memory-writer");
    let AppendResult::Stored(_) = append_connection_memory(
        root,
        AuthenticatedMemorySource {
            verified_id: "bearer:memory-writer",
            creation_label: "memory writer",
            source: &mem_source,
        },
        "Keep both figures with their source.\nSecond detail line.",
        "week-note-summary-op",
        "2026-03-09T15:00:00Z".parse::<jiff::Timestamp>().unwrap(),
    )
    .unwrap() else {
        panic!("agent memory fixture must store");
    };

    // Imported text note with a heading: source "imported file".
    let imported_note = root.join(format!("chronicle/{day}/import.text/130500_300"));
    fs::create_dir_all(&imported_note).unwrap();
    fs::write(
        imported_note.join("note_transcript.md"),
        "# Travel list\n\nBoots and coat.\n",
    )
    .unwrap();

    // Imported photo: source "imported file", keeps heading and photo fields.
    let imported_photo = root.join(format!("chronicle/{day}/import.image/080000_0"));
    fs::create_dir_all(&imported_photo).unwrap();
    fs::write(
        imported_photo.join("image_transcript.md"),
        "# Morning porch\n\n> [image description — model-derived]\n> Quiet steps in frost\\.\n",
    )
    .unwrap();

    // Streamless note import summary: carries no `source` anywhere.
    let summary_dir = root.join("imports/20260309_100000");
    fs::create_dir_all(&summary_dir).unwrap();
    fs::write(
        summary_dir.join("manifest.json"),
        json!({"import_id": "20260309_100000", "source_type": "obsidian"}).to_string(),
    )
    .unwrap();
    fs::write(
        summary_dir.join("summary.md"),
        "# Obsidian import summary\n\nstreamlessnoteimportmarker\n",
    )
    .unwrap();

    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();

    // Also attach a day to the streamless import summary row in the index to verify
    // `get_week` omits `source` when no stream stands behind an item.
    let index = solstone_core_indexer_store::db::open_index(root).unwrap();
    index
        .execute(
            "UPDATE chunks SET day = ?1 WHERE path = 'imports/20260309_100000/summary.md'",
            params![day],
        )
        .unwrap();
    drop(index);

    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "facets".into(),
                    "transcripts".into(),
                    "notes".into(),
                    "photos".into(),
                ],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    for args in [
        json!({"week": "2026-03-08", "include": ["notes", "photos"]}),
        json!({"day_from": "2026-03-08", "day_to": "2026-03-14", "include": ["notes", "photos"]}),
    ] {
        let week = probe(&journal, "get_week", args.clone()).unwrap();
        let day_row = week["days"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["day"] == day)
            .unwrap();

        let notes = &day_row["summary"]["notes"];
        assert_eq!(notes["count"], 5, "{args}: {notes}");
        let note_items = notes["items"].as_array().unwrap();
        let find_note = |title: &str| {
            note_items
                .iter()
                .find(|item| item["title"] == title)
                .unwrap_or_else(|| panic!("missing note titled {title:?} in {note_items:?}"))
        };
        assert_eq!(
            find_note("The book. Saturday night. The columns ruled:")["source"],
            "Obsidian"
        );
        assert_eq!(find_note("Ledger notes")["source"], "Obsidian");
        assert_eq!(
            find_note("Keep both figures with their source.")["source"],
            "your agent"
        );
        assert_eq!(find_note("Travel list")["source"], "imported file");
        assert!(
            find_note("Obsidian import summary").get("source").is_none(),
            "streamless summary must omit source: {note_items:?}"
        );
        assert!(
            note_items.iter().all(|item| !item["title"]
                .as_str()
                .unwrap()
                .starts_with("journal entry from")),
            "no note should fall back to generic day title: {note_items:?}"
        );

        let photos = &day_row["summary"]["photos"];
        assert_eq!(photos["count"], 1, "{args}: {photos}");
        let photo_item = &photos["items"][0];
        assert_eq!(photo_item["title"], "Morning porch");
        assert_eq!(photo_item["source"], "imported file");
        assert_eq!(photo_item["description"], "Quiet steps in frost.");
    }
}

#[test]
fn fetch_refuses_list_transcripts_references_and_names_get_transcript() {
    use crate::dispatch::{DispatchError, DispatchPrincipal, dispatch_authenticated_tool_call};
    use crate::jsonrpc::ToolName;
    use crate::references::{ReferenceTarget, SegmentReference};
    use crate::tools::ToolError;

    let journal = fixture();
    let root = journal.path();
    let day = "20260315";

    let audio = root.join(format!("chronicle/{day}/phone/090000_300"));
    let photo = root.join(format!("chronicle/{day}/import.image/100000_0"));
    let doc = root.join(format!("chronicle/{day}/import.document/110000_0"));
    let note = root.join(format!("chronicle/{day}/import.text/120000_0"));
    let body = root.join(format!("chronicle/{day}/import.oura/000000_86400"));
    for dir in [&audio, &photo, &doc, &note, &body] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(
        audio.join("audio.jsonl"),
        "{\"schema\":\"solstone.transcribe.v1\",\"kind\":\"header\"}\n\
         {\"kind\":\"statement\",\"start_ms\":1000,\"end_ms\":2000,\"text\":\"morning check\"}\n",
    )
    .unwrap();
    fs::write(
        photo.join("image_transcript.md"),
        "# Yard gate\n\n> [image description — model-derived]\n> Wooden gate\\.\n",
    )
    .unwrap();
    fs::write(
        doc.join("document_transcript.md"),
        "# Lease notice\n\n## Page 1\n\nSigned copy.\n",
    )
    .unwrap();
    fs::write(
        note.join("note_transcript.md"),
        "# Errands\n\nPick up ink.\n",
    )
    .unwrap();
    fs::write(
        body.join("body.jsonl"),
        "{\"schema\":\"solstone.body.v1\",\"kind\":\"sleep\",\"day\":\"20260315\",\"source\":\"oura\",\"duration_min\":420}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("streams")).unwrap();
    fs::write(
        root.join("streams/import.oura.json"),
        json!({
            "name": "import.oura",
            "kind": "import",
            "host": null,
            "platform": null,
            "created_at": 1,
            "last_day": null,
            "last_segment": null,
            "seq": 1
        })
        .to_string(),
    )
    .unwrap();

    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();

    let grant = |words: &[&str]| {
        PermissionStore::open(root)
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: words.iter().map(|word| (*word).to_owned()).collect(),
                    scope: ReadScope::WholeJournal,
                },
            )
            .unwrap();
    };
    grant(&[
        "facets",
        "transcripts",
        "photos",
        "documents",
        "notes",
        "body",
    ]);

    let listed = probe(&journal, "list_transcripts", json!({"day": day})).unwrap();
    let rows = listed["transcripts"].as_array().unwrap();
    let ref_for_title = |title: &str| -> String {
        rows.iter()
            .find(|row| row["title"] == title)
            .unwrap_or_else(|| panic!("missing row titled {title:?} in {rows:?}"))["reference"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let photo_ref = ref_for_title("Photo — 20260315");
    let doc_ref = ref_for_title("Document — 20260315");
    let note_ref = ref_for_title("Note — 20260315");
    let day_span = listed["span_reference"].as_str().unwrap().to_owned();
    let body_source_label = listed["other_sources"][0]
        .as_str()
        .unwrap_or_else(|| panic!("expected body other_source in {listed}"));
    let body_listed = probe(
        &journal,
        "list_transcripts",
        json!({"day": day, "source": body_source_label}),
    )
    .unwrap();
    let body_span = body_listed["span_reference"]
        .as_str()
        .unwrap_or_else(|| panic!("expected body span_reference in {body_listed}"))
        .to_owned();

    let PermissionDecision::Snapshot(snapshot) = evaluate_connection_read(root, CONNECTION) else {
        panic!("expected snapshot");
    };
    let sealed_segment = crate::dispatch::codec()
        .mint(
            CONNECTION,
            snapshot.generation,
            ReferenceTarget::Segment(SegmentReference {
                day: day.to_owned(),
                stream: "phone".to_owned(),
                segment: "090000_300".to_owned(),
                version: 1,
                source_index: None,
                byte_offset: None,
                statement_ordinal: None,
                message: None,
            }),
        )
        .unwrap();
    let foreign_segment = crate::dispatch::codec()
        .mint(
            "bearer:other-connection",
            snapshot.generation,
            ReferenceTarget::Segment(SegmentReference {
                day: day.to_owned(),
                stream: "phone".to_owned(),
                segment: "090000_300".to_owned(),
                version: 1,
                source_index: None,
                byte_offset: None,
                statement_ordinal: None,
                message: None,
            }),
        )
        .unwrap();
    let nonexistent_segment_name = format!("{day}/235900_60/aaaaaa");
    let missing_activity_name = format!("{day}/work/act_missing");

    let call_fetch = |args: Value| {
        dispatch_authenticated_tool_call(
            root,
            DispatchPrincipal {
                connection: CONNECTION,
                agent_identity: CONNECTION,
            },
            ToolName::Fetch,
            Some(&args),
            jiff::Timestamp::now(),
        )
    };

    // Single fetch refuses every list_transcripts reference shape with UseGetTranscript.
    for reference in [
        &photo_ref,
        &doc_ref,
        &note_ref,
        &day_span,
        &body_span,
        &sealed_segment,
        &nonexistent_segment_name,
    ] {
        let err = call_fetch(json!({"reference": reference}))
            .err()
            .expect("expected error");
        assert!(
            matches!(err, DispatchError::Tool(ToolError::UseGetTranscript)),
            "expected UseGetTranscript for {reference}, got {err:?}"
        );
        assert_eq!(ToolError::UseGetTranscript.reason(), "invalid_tool_input");
        assert!(
            ToolError::UseGetTranscript
                .agent_message()
                .contains("get_transcript")
        );
    }

    // Foreign sealed tokens and ordinary missing activity names stay ReferenceNotFound.
    for reference in [&foreign_segment, &missing_activity_name] {
        let err = call_fetch(json!({"reference": reference}))
            .err()
            .expect("expected error");
        assert!(
            matches!(err, DispatchError::Tool(ToolError::ReferenceNotFound)),
            "expected ReferenceNotFound for {reference}, got {err:?}"
        );
    }

    // Batched fetch carries the same per-item error naming get_transcript.
    let batch = probe(
        &journal,
        "fetch",
        json!({
            "references": [
                &photo_ref,
                &doc_ref,
                &note_ref,
                &day_span,
                &body_span,
                &sealed_segment,
                &nonexistent_segment_name,
                &foreign_segment,
                &missing_activity_name,
            ]
        }),
    )
    .unwrap();
    let items = batch["items"].as_array().unwrap();
    assert_eq!(items.len(), 9);
    for (idx, item) in items.iter().enumerate().take(7) {
        assert_eq!(item["index"], idx);
        assert_eq!(item["error"]["code"], "invalid_tool_input", "{item}");
        assert!(
            item["error"]["message"]
                .as_str()
                .unwrap()
                .contains("get_transcript"),
            "{item}"
        );
    }
    for idx in [7, 8] {
        assert_eq!(items[idx]["error"]["code"], "not_found", "{}", items[idx]);
    }

    // Without the facets grant, segment names still direct to get_transcript
    // instead of reporting a missing facets permission, while activity names still deny facets.
    grant(&["transcripts", "photos", "documents", "notes", "body"]);
    let err = call_fetch(json!({"reference": &photo_ref}))
        .err()
        .expect("expected error");
    assert!(
        matches!(err, DispatchError::Tool(ToolError::UseGetTranscript)),
        "expected UseGetTranscript without facets, got {err:?}"
    );
    let act_err = call_fetch(json!({"reference": &missing_activity_name}))
        .err()
        .expect("expected error");
    assert!(
        matches!(act_err, DispatchError::KindDenied(AdmittedCategory::Facets)),
        "activity name without facets should still deny facets, got {act_err:?}"
    );
    let no_facets_batch = probe(
        &journal,
        "fetch",
        json!({"references": [&photo_ref, &missing_activity_name]}),
    )
    .unwrap();
    assert_eq!(
        no_facets_batch["items"][0]["error"]["code"],
        "invalid_tool_input"
    );
    assert!(
        no_facets_batch["items"][0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("get_transcript")
    );
    assert_eq!(
        no_facets_batch["items"][1]["error"]["code"],
        "permission_denied"
    );

    // Passing the same non-speech list_transcripts reference to get_transcript reads it.
    let read = probe(&journal, "get_transcript", json!({"reference": &photo_ref})).unwrap();
    assert!(
        read["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap().contains("Wooden gate")),
        "{read}"
    );
}

#[test]
fn multi_chunk_pdf_collapses_to_one_search_hit_across_browse_query_and_pagination() {
    let journal = fixture();
    let root = journal.path();
    let day = "20260315";

    let build_pdf = |title: &str, token: &str| {
        let mut body = format!("# {title}\n\nType: Document\n\n");
        for page in 1..=4 {
            body.push_str(&format!(
                "## Page {page}\n\n\
                 Clause {page} records the {token} covenant and rent schedule in full detail, \
                 naming the parcel bounds, annual payment dates, timber rights, and renewal terms \
                 agreed between the parties for the ground plot and surrounding yard.\n\n"
            ));
        }
        body
    };

    let doc_a_dir = root.join(format!("chronicle/{day}/import.document/130000_0"));
    let doc_b_dir = root.join(format!("chronicle/{day}/import.document/140000_0"));
    let screen_dir = root.join(format!("chronicle/{day}/mac/150000_300/talents"));
    for dir in [&doc_a_dir, &doc_b_dir, &screen_dir] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::create_dir_all(root.join("streams")).unwrap();
    fs::write(
        root.join("streams/import.document.json"),
        json!({
            "name": "import.document",
            "kind": "import",
            "host": null,
            "platform": null,
            "created_at": 1,
            "last_day": null,
            "last_segment": null,
            "seq": 1
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        doc_a_dir.join("document_transcript.md"),
        build_pdf("Greenwich warrant", "holywellcovenant"),
    )
    .unwrap();
    fs::write(
        doc_b_dir.join("document_transcript.md"),
        build_pdf("Theatre lease terms", "holywellcovenant"),
    )
    .unwrap();
    fs::write(
        screen_dir.join("screen_text.md"),
        "## 00:00 · Browser\n\nFirst screen minute mentioning holywellcovenant.\n\n\
         ## 01:00 · Editor\n\nSecond screen minute mentioning holywellcovenant.\n",
    )
    .unwrap();

    solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();

    // Confirm both PDFs actually split into multiple indexed chunks.
    let index = solstone_core_indexer_store::db::open_index(root).unwrap();
    for seg in ["130000_0", "140000_0"] {
        let path = format!("{day}/import.document/{seg}/document_transcript.md");
        let count: i64 = index
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            count >= 4,
            "expected multi-chunk PDF for {seg}, got {count} chunks"
        );
    }
    drop(index);

    PermissionStore::open(root)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["documents".into(), "screen_text".into()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    // Empty-query browse with kind=["documents"]: one hit per PDF, not one per chunk.
    let browse_docs = probe(
        &journal,
        "search",
        json!({"day": day, "kind": ["documents"], "limit": 20}),
    )
    .unwrap();
    let browse_rows = browse_docs["results"].as_array().unwrap();
    assert_eq!(browse_rows.len(), 2, "{browse_docs}");
    assert_eq!(browse_rows[0]["title"], "Theatre lease terms");
    assert_eq!(browse_rows[1]["title"], "Greenwich warrant");
    assert_eq!(browse_rows[0]["content_kind"], "documents");
    assert_eq!(browse_rows[0]["source"], "PDF");

    // Unfiltered empty-query browse also emits one hit per PDF while keeping both screen minutes.
    let browse_all = probe(&journal, "search", json!({"day": day, "limit": 20})).unwrap();
    let doc_hits: Vec<&Value> = browse_all["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["content_kind"] == "documents")
        .collect();
    let screen_hits: Vec<&Value> = browse_all["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["content_kind"] == "screen_text")
        .collect();
    assert_eq!(doc_hits.len(), 2, "{browse_all}");
    assert_eq!(screen_hits.len(), 2, "{browse_all}");

    // Keyword search matching every chunk of both PDFs collapses each PDF to one hit,
    // while non-document multi-chunk material (screen_text) keeps every matching minute.
    let searched = probe(
        &journal,
        "search",
        json!({"query": "holywellcovenant", "limit": 20}),
    )
    .unwrap();
    let search_rows = searched["results"].as_array().unwrap();
    let search_docs: Vec<&Value> = search_rows
        .iter()
        .filter(|row| row["content_kind"] == "documents")
        .collect();
    let search_screens: Vec<&Value> = search_rows
        .iter()
        .filter(|row| row["content_kind"] == "screen_text")
        .collect();
    assert_eq!(search_docs.len(), 2, "{searched}");
    assert_eq!(search_screens.len(), 2, "{searched}");
    assert_eq!(search_docs[0]["title"], "Theatre lease terms");
    assert_eq!(search_docs[1]["title"], "Greenwich warrant");

    // Fetching the collapsed hit still returns the entire document across all pages.
    let fetched = probe(
        &journal,
        "fetch",
        json!({"reference": search_docs[0]["reference"]}),
    )
    .unwrap();
    let text = fetched["text"].as_str().unwrap();
    assert!(
        text.contains("## Page 1") && text.contains("## Page 4"),
        "{fetched}"
    );

    // Paginated keyword search with limit=1 crosses the document boundary without repeating
    // remaining chunks of the first document on page 2.
    let page1 = probe(
        &journal,
        "search",
        json!({"query": "holywellcovenant", "kind": ["documents"], "limit": 1}),
    )
    .unwrap();
    assert_eq!(page1["results"].as_array().unwrap().len(), 1, "{page1}");
    assert_eq!(page1["results"][0]["title"], "Theatre lease terms");
    let cursor1 = page1["next_cursor"].as_str().expect("cursor to second PDF");
    let page2 = probe(
        &journal,
        "search",
        json!({"query": "holywellcovenant", "kind": ["documents"], "limit": 1, "cursor": cursor1}),
    )
    .unwrap();
    assert_eq!(page2["results"].as_array().unwrap().len(), 1, "{page2}");
    assert_eq!(page2["results"][0]["title"], "Greenwich warrant");
    if let Some(cursor2) = page2["next_cursor"].as_str() {
        let page3 = probe(
            &journal,
            "search",
            json!({"query": "holywellcovenant", "kind": ["documents"], "limit": 1, "cursor": cursor2}),
        )
        .unwrap();
        assert!(page3["results"].as_array().unwrap().is_empty(), "{page3}");
        assert!(page3["next_cursor"].is_null(), "{page3}");
    }

    // Paginated empty-query browse with limit=1 returns each PDF once cleanly.
    let bpage1 = probe(
        &journal,
        "search",
        json!({"day": day, "kind": ["documents"], "limit": 1}),
    )
    .unwrap();
    assert_eq!(bpage1["results"].as_array().unwrap().len(), 1, "{bpage1}");
    assert_eq!(bpage1["results"][0]["title"], "Theatre lease terms");
    let bcursor1 = bpage1["next_cursor"].as_str().expect("browse cursor");
    let bpage2 = probe(
        &journal,
        "search",
        json!({"day": day, "kind": ["documents"], "limit": 1, "cursor": bcursor1}),
    )
    .unwrap();
    assert_eq!(bpage2["results"].as_array().unwrap().len(), 1, "{bpage2}");
    assert_eq!(bpage2["results"][0]["title"], "Greenwich warrant");
    assert!(bpage2["next_cursor"].is_null(), "{bpage2}");
}
