// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Possible names through the agent read paths: get_transcript, search and fetch.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_format::markdown::literal_text;
use solstone_core_possible_names::{Sidecar, SidecarEntry, SidecarSource, source_sha256, write};

use crate::dispatch::{McpProbeError, run_mcp_probe};
use crate::permissions::{PermissionStore, ReadPermission, ReadScope};

const CONNECTION: &str = "bearer:possible-names-test";
const FACET_A: &str = "123e4567-e89b-42d3-a456-426614174000";
const FACET_B: &str = "123e4567-e89b-42d3-a456-426614174001";
const DAY: &str = "20260914";
const STREAM: &str = "default";
const SPOKEN: &str = "ask Fryer to call anna";

struct Journal {
    root: tempfile::TempDir,
}

impl Journal {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("solstone-mcp-possible-names-")
            .tempdir_in(crate::test_scratch())
            .unwrap();
        let journal = Self { root };
        for (name, id, title) in [("alpha", FACET_A, "Alpha"), ("beta", FACET_B, "Beta")] {
            let directory = journal.path().join("facets").join(name);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("facet.json"),
                json!({"id": id, "title": title, "description": title}).to_string(),
            )
            .unwrap();
        }
        journal
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn segment_dir(&self, key: &str) -> PathBuf {
        self.path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(key)
    }

    /// A segment assigned to alpha whose first statement is `SPOKEN`.
    fn segment(&self, key: &str, brief: Option<&str>) {
        let dir = self.segment_dir(key);
        fs::create_dir_all(dir.join("talents")).unwrap();
        fs::write(dir.join("talents/facets.json"), "[{\"facet\":\"alpha\"}]").unwrap();
        fs::write(
            dir.join("audio.jsonl"),
            format!(
                "{{\"raw\":\"audio.flac\"}}\n{}\n",
                json!({"start": "00:00:01", "speaker": 1, "text": SPOKEN})
            ),
        )
        .unwrap();
        if let Some(brief) = brief {
            fs::write(dir.join("talents/brief.md"), format!("{brief}\n")).unwrap();
        }
    }

    fn entity(&self, id: &str, name: &str, facet: &str) {
        let dir = self.path().join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("entity.json"),
            json!({"id": id, "name": name, "aka": ["Fryer"], "description": "private"}).to_string(),
        )
        .unwrap();
        let link = self
            .path()
            .join("facets")
            .join(facet)
            .join("entities")
            .join(id);
        fs::create_dir_all(&link).unwrap();
        fs::write(
            link.join("entity.json"),
            json!({"entity_id": id}).to_string(),
        )
        .unwrap();
    }

    fn possible_names(&self, key: &str, entries: &[(usize, &str, &str, &str)]) {
        let dir = self.segment_dir(key);
        let entries = entries
            .iter()
            .map(|(start, heard, id, name)| SidecarEntry {
                sentence: 1,
                start_byte: *start,
                end_byte: start + heard.len(),
                heard: (*heard).to_owned(),
                entity_id: (*id).to_owned(),
                name: (*name).to_owned(),
            })
            .collect();
        let sidecar = Sidecar::new(
            "test-1",
            SidecarSource {
                file: "audio.jsonl".to_owned(),
                sha256: source_sha256(&dir, "audio.jsonl").unwrap(),
            },
            "rev",
            1,
            entries,
        );
        write(&dir, &sidecar).unwrap();
    }

    fn index(&self) {
        solstone_core_indexer_store::scan::scan_journal(self.path(), true).unwrap();
    }

    fn grant(&self, scope: ReadScope) {
        PermissionStore::open(self.path())
            .set_permission(
                CONNECTION,
                ReadPermission {
                    categories: vec!["transcripts".to_owned(), "entities".to_owned()],
                    scope,
                },
            )
            .unwrap();
    }

    fn probe(&self, tool: &str, arguments: Value) -> Result<Value, McpProbeError> {
        run_mcp_probe(self.path(), CONNECTION, tool, &arguments)
    }

    /// The first transcript entry of the day's only (or first) segment.
    fn first_line(&self) -> (String, String) {
        let listed = self.probe("list_transcripts", json!({"day": DAY})).unwrap();
        let reference = listed["transcripts"][0]["reference"]
            .as_str()
            .unwrap()
            .to_owned();
        let page = self
            .probe("get_transcript", json!({"reference": reference}))
            .unwrap();
        (reference, page["entries"][0].as_str().unwrap().to_owned())
    }

    fn search(&self, query: &str, limit: usize, cursor: Option<&str>) -> Value {
        let mut arguments = json!({"query": query, "limit": limit});
        if let Some(cursor) = cursor {
            arguments["cursor"] = json!(cursor);
        }
        self.probe("search", arguments).unwrap()
    }
}

fn whole() -> ReadScope {
    ReadScope::WholeJournal
}

fn alpha() -> ReadScope {
    ReadScope::Facets {
        ids: vec![FACET_A.to_owned()],
    }
}

fn line(text: &str) -> String {
    literal_text(&format!("[00:00:01] Speaker 1: {text}"))
}

fn possible_name_hits(response: &Value) -> Vec<Value> {
    response["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|hit| hit["possible_name"] == json!(true))
        .cloned()
        .collect()
}

#[test]
fn get_transcript_labels_live_names_for_this_connection_and_binds_them_into_the_version() {
    let journal = Journal::new();
    journal.segment("090000_300", None);
    journal.entity("friar", "Friar", "alpha");
    journal.entity("ana", "Ana Lima", "beta");
    journal.possible_names(
        "090000_300",
        &[
            (4, "Fryer", "friar", "Friar"),
            (18, "anna", "ana", "Ana Lima"),
        ],
    );

    journal.grant(whole());
    let (reference, first) = journal.first_line();
    assert_eq!(
        first,
        line("ask Fryer (possible name: Friar) to call anna (possible name: Ana Lima)")
    );
    // Aliases and descriptions never reach the agent.
    assert!(!first.contains("private"));

    // Ana is attached only to beta: an alpha connection reads the transcript
    // but not her name.
    journal.grant(alpha());
    let (_, scoped) = journal.first_line();
    assert_eq!(
        scoped,
        line("ask Fryer (possible name: Friar) to call anna")
    );

    // A rename hides Ana's label, and a reference minted before it no longer reads.
    journal.grant(whole());
    fs::write(
        journal.path().join("entities/ana/entity.json"),
        json!({"id": "ana", "name": "Ana L."}).to_string(),
    )
    .unwrap();
    assert_eq!(
        journal.probe("get_transcript", json!({"reference": reference})),
        Err(McpProbeError::Unavailable)
    );
    let (_, renamed) = journal.first_line();
    assert_eq!(
        renamed,
        line("ask Fryer (possible name: Friar) to call anna")
    );

    // New transcript bytes hide every label.
    let audio = journal.segment_dir("090000_300").join("audio.jsonl");
    let mut bytes = fs::read(&audio).unwrap();
    bytes.extend_from_slice(b"{\"start\":\"00:00:09\",\"speaker\":1,\"text\":\"more\"}\n");
    fs::write(&audio, bytes).unwrap();
    let (_, stale) = journal.first_line();
    assert_eq!(stale, line(SPOKEN));
}

#[test]
fn a_query_for_the_name_finds_the_recording_through_its_possible_name() {
    let journal = Journal::new();
    journal.segment("090000_300", None);
    journal.entity("friar", "Friar", "alpha");
    journal.possible_names("090000_300", &[(4, "Fryer", "friar", "Friar")]);
    journal.index();
    journal.grant(whole());

    let response = journal.search("Friar", 10, None);
    let hits = possible_name_hits(&response);
    assert_eq!(hits.len(), 1, "{response}");
    let hit = &hits[0];
    assert_eq!(
        hit["snippet"].as_str().unwrap(),
        literal_text("possible name: Friar (heard as \"Fryer\")")
    );
    // It links to the segment's transcript, which shows the label in place.
    let segment = hit["segments"][0]["reference"].as_str().unwrap();
    let page = journal
        .probe("get_transcript", json!({"reference": segment}))
        .unwrap();
    assert_eq!(
        page["entries"][0].as_str().unwrap(),
        line("ask Fryer (possible name: Friar) to call anna")
    );
    let fetched = journal
        .probe("fetch", json!({"reference": hit["reference"]}))
        .unwrap();
    assert_eq!(
        fetched["text"],
        json!("possible name: Friar (heard as \"Fryer\")")
    );

    // Nothing about the words is searchable: they are not in the index.
    assert!(possible_name_hits(&journal.search("Fryer", 10, None)).is_empty());

    // Once the name changes the row stops matching, and fetch refuses it too.
    fs::write(
        journal.path().join("entities/friar/entity.json"),
        json!({"id": "friar", "name": "Brother Friar"}).to_string(),
    )
    .unwrap();
    assert!(possible_name_hits(&journal.search("Friar", 10, None)).is_empty());
    assert_eq!(
        journal.probe("fetch", json!({"reference": hit["reference"]})),
        Err(McpProbeError::Unavailable)
    );
}

#[test]
fn an_unseen_entity_leaves_no_hit_count_or_cursor_trace_across_pages() {
    let journal = Journal::new();
    journal.entity("friar", "Friar", "beta");
    let keys = (0..12)
        .map(|index| format!("09{:02}00_300", index * 5))
        .collect::<Vec<_>>();
    for (index, key) in keys.iter().enumerate() {
        journal.segment(key, Some(&format!("brief {index} about Friar")));
        journal.possible_names(key, &[(4, "Fryer", "friar", "Friar")]);
    }
    journal.index();

    // A whole-journal connection sees both kinds of hit.
    journal.grant(whole());
    let everything = journal.search("Friar", 100, None);
    assert_eq!(everything["results"].as_array().unwrap().len(), 24);
    assert_eq!(possible_name_hits(&everything).len(), 12);

    // Alpha holds every segment but not the entity: its pages are exactly the
    // briefs, as if the possible names were never indexed.
    journal.grant(alpha());
    let mut cursor: Option<String> = None;
    let mut sizes = Vec::new();
    let mut snippets = Vec::new();
    loop {
        let page = journal.search("Friar", 5, cursor.as_deref());
        let results = page["results"].as_array().unwrap();
        assert!(possible_name_hits(&page).is_empty(), "{page}");
        assert_eq!(page["coverage"]["live_examination_complete"], json!(true));
        sizes.push(results.len());
        snippets.extend(
            results
                .iter()
                .map(|hit| hit["snippet"].as_str().unwrap().to_owned()),
        );
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(sizes, [5, 5, 2]);
    snippets.sort();
    snippets.dedup();
    assert_eq!(snippets.len(), 12);
    assert!(snippets.iter().all(|snippet| snippet.starts_with("brief")));

    // A query only the possible names match finds nothing, and reads as complete.
    let none = journal.search("possible name", 5, None);
    assert!(none["results"].as_array().unwrap().is_empty());
    assert_eq!(none["next_cursor"], Value::Null);
    assert_eq!(none["coverage"]["live_examination_complete"], json!(true));
    assert_eq!(none["relaxed"], json!(false));

    // Nor does its transcript carry the label.
    let (_, first) = journal.first_line();
    assert_eq!(first, line(SPOKEN));
}

#[test]
fn a_deleted_segment_shows_no_possible_name() {
    let journal = Journal::new();
    journal.entity("friar", "Friar", "alpha");
    for key in ["090000_300", "091000_300"] {
        journal.segment(key, None);
        journal.possible_names(key, &[(4, "Fryer", "friar", "Friar")]);
    }
    journal.index();
    journal.grant(whole());
    assert_eq!(
        possible_name_hits(&journal.search("Friar", 10, None)).len(),
        2
    );

    let outcome = solstone_core_retention::door::remove_segments(
        journal.path(),
        &[solstone_core_retention::Target {
            day: DAY.to_owned(),
            stream: STREAM.to_owned(),
            dir: "090000_300".to_owned(),
        }],
        "2026-10-17T12:00:00Z",
        solstone_core_retention::RemovalReason::OwnerSegmentDelete,
        "possible-names-delete",
    );
    assert!(outcome.targets[0].not_removed.is_empty());
    // The index still holds the deleted segment's rows until its next pass.
    let hits = possible_name_hits(&journal.search("Friar", 10, None));
    assert_eq!(hits.len(), 1);
    let page = journal
        .probe(
            "get_transcript",
            json!({"reference": hits[0]["segments"][0]["reference"]}),
        )
        .unwrap();
    assert!(
        page["entries"][0]
            .as_str()
            .unwrap()
            .contains("possible name")
    );
    let listed = journal
        .probe("list_transcripts", json!({"day": DAY}))
        .unwrap();
    assert_eq!(listed["transcripts"].as_array().unwrap().len(), 1);
}
