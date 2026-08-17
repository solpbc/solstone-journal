// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_observer::store::record::ObserverRecord;
use solstone_core_observer::store::write::save_observer;

/// 2026-01-01T03:00:00.000Z — UTC date and clock are unambiguous; all list/status
/// offsets stay on 2026-01-01.
pub const NOW_MS: i64 = 1_767_236_400_000;

pub fn write_record(root: &Path, value: Value) -> ObserverRecord {
    let record = ObserverRecord::from_value(value).expect("record");
    save_observer(root, &record).expect("save record");
    record
}

pub fn write_raw(root: &Path, filename: &str, value: Value) {
    let path = root.join("apps/observer/observers").join(filename);
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(path, value.to_string()).expect("raw record");
}

pub fn write_history(root: &Path, prefix: &str, day: &str, rows: &[Value]) {
    let path = root
        .join("apps/observer/observers")
        .join(prefix)
        .join("hist")
        .join(format!("{day}.jsonl"));
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    let contents = rows
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(path, contents).expect("history");
}

pub fn segment_dir(root: &Path, day: &str, stream: &str, segment: &str) -> PathBuf {
    root.join("chronicle").join(day).join(stream).join(segment)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

pub fn write_segment(
    root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    seq: u64,
    prev_segment: Option<&str>,
    audio: &[u8],
) -> PathBuf {
    let dir = segment_dir(root, day, stream, segment);
    fs::create_dir_all(&dir).expect("segment dir");
    fs::write(dir.join("audio.flac"), audio).expect("audio");
    let manifest = json!({
        "schema_version": 1,
        "files": {"audio.flac": {"sha256": sha256_hex(audio), "size": audio.len()}},
    });
    fs::write(dir.join("ingest.json"), manifest.to_string()).expect("manifest");
    let marker = json!({
        "stream": stream,
        "prev_day": prev_segment.map(|_| day),
        "prev_segment": prev_segment,
        "seq": seq,
    });
    fs::write(dir.join("stream.json"), marker.to_string()).expect("marker");
    dir
}

pub fn seed_observer_owning_stream(root: &Path, prefix: &str, stream: &str) -> ObserverRecord {
    write_record(
        root,
        json!({"key": format!("{prefix}12345678"), "name": stream, "stream": stream}),
    )
}

pub fn seed_full_fixture(root: &Path) {
    write_record(
        root,
        json!({"key":"aaaaaaaa111", "name":"bound-live", "device_binding":{"device":format!("sha256:{}", "a".repeat(64)),"kind":"cert"}, "created_at":NOW_MS - 1_800_000, "last_seen":NOW_MS - 30_000, "last_segment":null, "last_segment_received_at":null, "last_segment_day":null, "enabled":true, "revoked":false, "revoked_at":null, "stats":{"segments_received":2,"bytes_received":1024}}),
    );
    write_record(
        root,
        json!({"key":"bbbbbbbb222", "name":"unbound-stale", "created_at":NOW_MS - 1_200_000, "last_seen":NOW_MS - 300_000, "last_segment":null, "last_segment_received_at":null, "last_segment_day":null, "enabled":true, "revoked":false, "revoked_at":null, "stats":{"segments_received":3,"bytes_received":2048}}),
    );
    write_record(
        root,
        json!({"key":"cccccccc333", "name":"revoked-never", "created_at":NOW_MS - 600_000, "last_seen":null, "last_segment":null, "last_segment_received_at":null, "last_segment_day":null, "enabled":true, "revoked":true, "revoked_at":NOW_MS - 60_000, "stats":{"segments_received":4,"bytes_received":4096}}),
    );
    write_raw(
        root,
        "dddddddd.json",
        json!({"key":"dddddddd444", "name":"fingerprint-rejected", "fingerprint":"legacy", "created_at":NOW_MS}),
    );
    write_raw(
        root,
        "eeeeeeee.json",
        json!({"name":"missing-key-rejected", "created_at":NOW_MS}),
    );
    write_raw(
        root,
        "wrongname.json",
        json!({"key":"ffffffff666", "name":"filename-rejected", "created_at":NOW_MS}),
    );
}
