// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

#[test]
fn vendored_text_generate_assets_match_live_python_sources() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository = manifest.join("../../..");
    for name in [
        "detect_transcript_segment.md",
        "detect_transcript_segment.schema.json",
        "detect_transcript_json.md",
        "detect_transcript_json.schema.json",
    ] {
        let live = fs::read(repository.join("solstone/think").join(name)).unwrap();
        let vendored = fs::read(manifest.join("src/text_assets").join(name)).unwrap();
        assert_eq!(vendored, live, "vendored {name} drifted from Python source");
    }
}
