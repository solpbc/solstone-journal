// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, path::Path};

use serde_json::json;
use tempfile::TempDir;

pub(crate) const OK: &str = "20260801_120000";
pub(crate) const FAILED: &str = "20260802_130000";
pub(crate) const PENDING: &str = "20260803_140000";
pub(crate) const CONTENT: &str = "20260804_150000";

pub(crate) fn phase_root(phase: &str) -> TempDir {
    let temporary_parent = fs::canonicalize(std::env::temp_dir()).expect("temporary directory");
    let root = TempDir::new_in(temporary_parent).expect("temporary journal");
    match phase {
        "unestablished" => seed_source(root.path()),
        "corrupt" => {
            fs::create_dir_all(root.path().join("config")).expect("config");
            fs::write(
                root.path().join("config/journal.json"),
                b"{\"setup\": {\"completed_at\": 17672256",
            )
            .expect("corrupt config");
        }
        "empty" => seed_established(root.path()),
        "populated" => seed_populated(root.path()),
        _ => panic!("unknown test phase: {phase}"),
    }
    root
}

pub(crate) fn populated_root() -> TempDir {
    phase_root("populated")
}

fn seed_established(root: &Path) {
    fs::create_dir_all(root.join("config")).expect("config");
    fs::write(
        root.join("config/journal.json"),
        b"{\n  \"setup\": {\n    \"completed_at\": 1767225600\n  }\n}\n",
    )
    .expect("config writes");
    seed_source(root);
}

fn seed_populated(root: &Path) {
    seed_established(root);
    for (timestamp, filename, mime, client_item_id, imported, payload) in [
        (
            OK,
            "notes.txt",
            "text/plain",
            "corpus-item-1",
            Some(json!({"processed":true,"files_written":1,"days":["20260801"]})),
            b"corpus import payload\n".as_slice(),
        ),
        (
            FAILED,
            "broken.ics",
            "text/calendar",
            "corpus-item-2",
            Some(
                json!({"processed":false,"error":"calendar payload could not be parsed","error_stage":"detect"}),
            ),
            b"not really an ics\n".as_slice(),
        ),
        (
            PENDING,
            "waiting.md",
            "text/plain",
            "corpus-item-3",
            None,
            b"# waiting\n".as_slice(),
        ),
        (
            CONTENT,
            "conversations.json",
            "application/json",
            "corpus-item-4",
            Some(
                json!({"processed":true,"files_written":3,"source_type":"chatgpt","days":["20260801","20260802","20260901"]}),
            ),
            b"[]\n".as_slice(),
        ),
    ] {
        seed_import(
            root,
            timestamp,
            filename,
            mime,
            client_item_id,
            imported,
            payload,
        );
    }
    fs::write(root.join("imports").join(CONTENT).join("content_manifest.jsonl"), [
        json!({"id":"corpus-entry-1","date":"20260801","title":"first conversation","preview":"a short preview of the first entry","body":"the full body of the first entry"}),
        json!({"id":"corpus-entry-2","date":"20260802","title":"second conversation","preview":"a short preview of the second entry","body":"the full body of the second entry"}),
        json!({"id":"corpus-entry-3","date":"20260901","title":"a September conversation","preview":"a short preview of the third entry","body":"the full body of the third entry"}),
    ].iter().map(serde_json::to_string).collect::<Result<Vec<_>, _>>().expect("manifest rows").join("\n") + "\n").expect("manifest");
}

pub(crate) fn seed_import(
    root: &Path,
    timestamp: &str,
    filename: &str,
    mime: &str,
    client_item_id: &str,
    imported: Option<serde_json::Value>,
    payload: &[u8],
) {
    let directory = root.join("imports").join(timestamp);
    fs::create_dir_all(&directory).expect("import directory");
    fs::write(directory.join(filename), payload).expect("payload");
    fs::write(directory.join("import.json"), serde_json::to_vec(&json!({"original_filename":filename,"file_size":42,"mime_type":mime,"facet":null,"setting":null,"user_timestamp":null,"imported_via":"web_dashboard","link_id":null,"observer_handle":null,"source":"corpus","source_hash":"sha256:0000000000000000000000000000000000000000000000000000000000000000","client_item_id":client_item_id})).expect("metadata serializes")).expect("metadata");
    if let Some(imported) = imported {
        fs::write(
            directory.join("imported.json"),
            serde_json::to_vec(&imported).expect("result serializes"),
        )
        .expect("result");
    }
}

pub(crate) fn seed_source(root: &Path) {
    let sources = root.join("apps/import/journal_sources");
    fs::create_dir_all(&sources).expect("sources");
    fs::write(sources.join("corpus_peer.json"), serde_json::to_vec(&json!({"key":"corpusSourceKey0000000000000000000000000000","name":"corpus_peer","created_at":1767225600000_i64,"enabled":true,"revoked":false,"revoked_at":null,"stats":{"segments_received":0,"entities_received":0,"facets_received":0,"imports_received":0,"config_received":0}})).expect("source serializes")).expect("source");
    let state = root.join("imports/corpusSo");
    fs::create_dir_all(&state).expect("state");
    fs::write(state.join("source.json"), "{}").expect("source marker");
    for area in ["segments", "entities", "facets", "imports", "config"] {
        fs::create_dir_all(state.join(area)).expect("state area");
    }
}
