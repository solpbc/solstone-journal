// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use solstone_core_format::content::{ChatLabels, Family, produce_chunks};

#[test]
fn ac6_read_old_ai_chat_fixtures_drop_import_metadata_except_supported_fields() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let fixtures = [
        "import.chatgpt/100000_300",
        "import.chatgpt/100500_300",
        "import.chatgpt/101000_300",
        "import.claude/140000_300",
        "import.claude/140500_300",
        "import.claude/141000_300",
        "import.gemini/110000_300",
        "import.gemini/110500_300",
    ];
    for fixture in fixtures {
        let relative = format!("20260101/{fixture}/conversation_transcript.jsonl");
        let text = fs::read_to_string(
            repository
                .join("tests/fixtures/journal/chronicle")
                .join(&relative),
        )
        .unwrap();
        let produced = produce_chunks(Family::AiChat, &relative, &text, &ChatLabels::default());
        let header = produced.header.unwrap();
        assert!(!header.contains("async debugging"));
        assert!(!header.contains("ai_conversation"));
        assert!(!header.contains("fixture-"));
        assert!(!produced.chunks.is_empty());
    }

    let produced = produce_chunks(
        Family::AiChat,
        "20260101/import.chatgpt/100000_300/conversation_transcript.jsonl",
        r#"{"raw":"../../../imports/id/t.txt","topics":"topic","setting":"setting","imported":{"id":"id","facet":"work"}}
{"speaker":"System","text":"metadata-like"}
{"start":"00:00:00","speaker":"Human","text":"kept"}"#,
        &ChatLabels::default(),
    );
    let header = produced.header.unwrap();
    assert_eq!(header, "# ChatGPT conversation\nFacet: work");
    assert_eq!(produced.chunks.len(), 1);
    assert_eq!(produced.chunks[0].content, "**Human:** kept");
}
