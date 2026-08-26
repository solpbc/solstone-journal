// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_segment::{Kind, StreamHints, StreamRecord, resolve_stream};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-segment-readcompat-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
/// Legacy records remain deserializable, while native records demonstrate the
/// resolver's `created_at`-preserving monotonic advance. The public resolver
/// intentionally cannot advance a legacy record without a `cid`, because that
/// would adopt an unattributed stream.
fn python_stream_record_fixture_loads_and_advances_monotonically() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/stream-record-readcompat.json"
    ))
    .unwrap();
    let records = fixture["records"].as_object().unwrap();
    let parsed: Vec<StreamRecord> = records
        .values()
        .map(|value| serde_json::from_value(value.clone()).unwrap())
        .collect();
    assert_eq!(parsed.len(), 4);
    let import_apple = parsed
        .iter()
        .find(|record| record.name == "import.apple")
        .unwrap();
    assert_eq!(import_apple.kind, "import");
    assert!(import_apple.host.is_none());
    assert!(import_apple.platform.is_none());
    assert_eq!(import_apple.created_at, 1_785_891_124);
    assert_eq!(import_apple.seq, 1);

    let recovered = parsed
        .iter()
        .find(|record| record.name == "recovered")
        .unwrap();
    assert_eq!(recovered.kind, "unknown");
    assert!(recovered.host.is_none());
    assert!(recovered.platform.is_none());
    assert_eq!(recovered.created_at, 1_785_891_124);
    assert_eq!(recovered.seq, 7);

    let workstation = parsed
        .iter()
        .find(|record| record.name == "workstation")
        .unwrap();
    assert_eq!(workstation.kind, "observer");
    assert_eq!(workstation.host.as_deref(), Some("workstation.local"));
    assert_eq!(workstation.platform.as_deref(), Some("linux"));
    assert_eq!(workstation.created_at, 1_785_891_124);
    assert_eq!(workstation.seq, 3);
    assert!(
        parsed
            .iter()
            .filter(|record| record.name != "iphone")
            .all(|record| record.cid.is_none() && record.source.is_none())
    );

    let iphone = parsed
        .iter()
        .find(|record| record.name == "iphone")
        .unwrap();
    assert_eq!(iphone.kind, "observer");
    assert_eq!(iphone.host.as_deref(), Some("iphone.local"));
    assert_eq!(iphone.platform.as_deref(), Some("ios"));
    assert_eq!(iphone.created_at, 1_785_891_124);
    assert_eq!(iphone.seq, 2);
    assert_eq!(
        iphone.cid.as_deref(),
        Some("sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
    );
    assert!(iphone.source.is_none());
    let emitted = serde_json::to_value(iphone).unwrap();
    assert_eq!(
        emitted["cid"],
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    );
    assert!(emitted.get("did").is_none());

    let temporary = TempDir::new();
    let first = resolve_stream(
        temporary.path(),
        "20260804",
        "120000_60",
        "workstation",
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "",
        StreamHints {
            kind: Some(Kind::Observed),
            host: None,
            platform: None,
        },
    )
    .unwrap();
    let state = temporary.path().join("streams/workstation.json");
    let initial: StreamRecord = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    assert_eq!(first.advance.seq, 1);
    let second = resolve_stream(
        temporary.path(),
        "20260804",
        "120100_60",
        "workstation",
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "",
        StreamHints::default(),
    )
    .unwrap();
    let updated: StreamRecord = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    assert_eq!(updated.created_at, initial.created_at);
    assert_eq!(updated.seq, initial.seq + 1);
    assert_eq!(second.advance.seq, updated.seq);
}
