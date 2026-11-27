// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::content::screen_text::{produce_screen_text_chunks, screen_text_with_clock};
use crate::folder::*;

fn folder_meta(value: Value) -> Map<String, Value> {
    let Value::Object(map) = json!({ "folder": value }) else {
        unreachable!()
    };
    map
}

fn hash_name(bytes: &[u8]) -> String {
    format!("folder_{:x}.jsonl", Sha256::digest(bytes))
}

#[test]
fn valid_folder_meta_and_stream_source() {
    let meta = validate_folder_meta(&folder_meta(json!({
        "v": 1,
        "id": "downloads-a1b2c3d4",
        "name": "Downloads",
        "facet": "work",
        "kinds": ["notes", "documents", "screenshots"]
    })))
    .unwrap();
    assert_eq!(meta.id, "downloads-a1b2c3d4");
    assert_eq!(meta.name, "Downloads");
    assert_eq!(meta.facet.as_deref(), Some("work"));
    assert_eq!(meta.kinds, vec!["notes", "documents", "screenshots"]);

    let stream_src = folder_stream_source(&meta.id);
    assert!(stream_src.starts_with("folder_"));
    assert_eq!(stream_src.len(), 7 + 12);
    assert!(is_folder_source_name("folder"));
    assert!(is_folder_source_name(&stream_src));
    assert!(!is_folder_source_name("folder_nothex123456"));
    assert!(!is_folder_source_name("obsidian"));
}

#[test]
fn rejects_bad_folder_meta() {
    for bad in [
        json!(null),
        json!({"v": 2, "id": "dl", "name": "Downloads"}),
        json!({"v": 1, "id": "../evil", "name": "Downloads"}),
        json!({"v": 1, "id": "Upper", "name": "Downloads"}),
        json!({"v": 1, "id": "dl", "name": "   "}),
        json!({"v": 1, "id": "dl", "name": "bad\nname"}),
        json!({"v": 1, "id": "dl", "name": "Downloads", "facet": "Work"}),
        json!({"v": 1, "id": "dl", "name": "Downloads", "kinds": ["video"]}),
    ] {
        assert!(
            validate_folder_meta(&folder_meta(bad.clone())).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn golden_activity_lines_validate_and_render_deterministically() {
    let content_hash = format!("{:x}", Sha256::digest(b"hello pdf"));
    let lines = [
        json!({
            "v": 1,
            "event": "added",
            "path": "invoices/2026-11-statement.pdf",
            "previous_path": null,
            "at": "2026-11-27T14:02:15-07:00",
            "observed_at": "2026-11-27T14:02:18-07:00",
            "file_type": "pdf",
            "size": 184_320,
            "created_at": "2026-11-27T14:02:14-07:00",
            "modified_at": "2026-11-27T14:02:15-07:00",
            "content_hash": content_hash,
            "part": "c_01.pdf",
            "kind_hint": "documents",
            "platform": {
                "origin_url": "https://billing.example/statement.pdf?token=secret123456",
                "downloaded_by": "Firefox",
                "xattrs": {"user.xdg.origin.url": "https://billing.example/statement.pdf"}
            }
        }),
        json!({
            "v": 1,
            "event": "renamed",
            "path": "archive/installer.dmg",
            "previous_path": "installer.dmg",
            "at": "2026-11-27T14:04:01-07:00",
            "observed_at": "2026-11-27T14:04:02-07:00",
            "file_type": "dmg",
            "size": 52_428_800,
            "content_hash": null,
            "part": null,
            "kind_hint": null,
            "platform": null
        }),
        json!({
            "v": 1,
            "event": "removed",
            "path": "temp.tmp",
            "previous_path": null,
            "at": "2026-11-27T14:04:55-07:00",
            "observed_at": "2026-11-27T14:04:56-07:00"
        }),
    ];
    let mut payload = String::new();
    for line in &lines {
        payload.push_str(&serde_json::to_string(line).unwrap());
        payload.push('\n');
    }
    let name = hash_name(payload.as_bytes());
    let parsed = validate_folder_file(&name, payload.as_bytes(), "20261127", "140000_300").unwrap();
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0].event, FolderEventKind::Added);
    assert_eq!(
        parsed[0].origin_url(),
        Some("https://billing.example/statement.pdf?token=secret123456")
    );
    assert_eq!(parsed[1].event, FolderEventKind::Renamed);
    assert_eq!(parsed[2].event, FolderEventKind::Removed);

    let stored = StoredFolderActivity {
        v: 1,
        folder_id: "downloads-a1b2c3d4".into(),
        folder_name: "Downloads".into(),
        owner_facet: Some("work".into()),
        events: parsed
            .iter()
            .enumerate()
            .map(|(i, line)| StoredFolderEvent {
                key: format!("k{i}"),
                event: line.event,
                path: line.path.clone(),
                previous_path: line.previous_path.clone(),
                at: line.at.to_string(),
                at_ms: line.at_ms(),
                offset_ms: event_offset_ms("20261127", "140000_300", &line.at),
                file_type: line.file_type.clone(),
                size: line.size,
                content_hash: line.content_hash.clone(),
                origin_url: line.origin_url().map(str::to_owned),
                downloaded_by: line.downloaded_by().map(str::to_owned),
                source_app: line.source_app().map(str::to_owned),
                kind: line.kind_hint.clone(),
                landed_file: None,
            })
            .collect(),
    };
    let md = render_folder_activity_markdown(&stored);
    assert!(md.contains("## 02:15 · Folder: Downloads\n"), "{md}");
    assert!(md.contains("## 04:01 · Folder: Downloads\n"), "{md}");
    assert!(
        md.contains("Added invoices\\/2026\\-11\\-statement\\.pdf"),
        "{md}"
    );
    assert!(
        md.contains("token\\=\\[REDACTED\\]"),
        "secrets redacted: {md}"
    );
    assert!(!md.contains("secret123456"), "secret leaked: {md}");
    assert!(
        md.contains("Renamed installer\\.dmg \\-&gt; archive\\/installer\\.dmg"),
        "{md}"
    );
    assert!(md.contains("Removed temp\\.tmp"), "{md}");

    let clocked = screen_text_with_clock(&md, "140000_300");
    assert!(
        clocked.contains("## 14:02:15 · Folder: Downloads"),
        "{clocked}"
    );
    let chunks = produce_screen_text_chunks(&md);
    assert!(chunks.error.is_none());
    assert!(!chunks.chunks.is_empty());

    let combined =
        merge_folder_activity_with_screen_text(&stored, "## 00:00 · Screenshot\n- Hello OCR\n");
    assert!(combined.contains("## 02:15 · Folder: Downloads"));
    assert!(combined.contains("## 00:00 · Screenshot\n- Hello OCR"));
    assert_eq!(
        extract_non_folder_screen_text(&combined),
        "## 00:00 · Screenshot\n- Hello OCR"
    );
}

#[test]
fn rejects_out_of_window_and_invalid_paths() {
    let outside = json!({
        "v": 1,
        "event": "added",
        "path": "a.txt",
        "at": "2026-11-27T14:05:00-07:00",
        "observed_at": "2026-11-27T14:05:01-07:00"
    });
    let payload = format!("{outside}\n");
    let err = validate_folder_file(
        &hash_name(payload.as_bytes()),
        payload.as_bytes(),
        "20261127",
        "140000_300",
    )
    .unwrap_err();
    assert!(err.reason.contains("outside"), "{err}");

    let traversal = json!({
        "v": 1,
        "event": "added",
        "path": "../secret.txt",
        "at": "2026-11-27T14:01:00-07:00",
        "observed_at": "2026-11-27T14:01:01-07:00"
    });
    let payload = format!("{traversal}\n");
    assert!(
        validate_folder_file(
            &hash_name(payload.as_bytes()),
            payload.as_bytes(),
            "20261127",
            "140000_300",
        )
        .is_err()
    );
}
