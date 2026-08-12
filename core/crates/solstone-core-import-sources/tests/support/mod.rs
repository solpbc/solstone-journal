// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(dead_code)]

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use zip::write::SimpleFileOptions;

static NEXT_TREE: AtomicUsize = AtomicUsize::new(0);

pub struct TempTree {
    path: PathBuf,
}

impl TempTree {
    pub fn new() -> Self {
        let index = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-import-sources-w7-{}-{index}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

pub fn write_zip(path: &Path, members: &[(String, Vec<u8>)]) {
    let file = File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    for (name, bytes) in members {
        writer
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

pub fn claude_archive(tree: &TempTree) -> PathBuf {
    let path = tree.path().join("claude.zip");
    let conversations = json!([
        {
            "created_at": "2026-03-11T12:00:00",
            "chat_messages": [
                {"sender": "human", "text": "Hello", "created_at": "2026-03-11T12:00:00"},
                {"sender": "assistant", "text": "Hi", "created_at": "2026-03-11T12:01:00"}
            ]
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

pub fn chatgpt_archive(tree: &TempTree) -> PathBuf {
    let path = tree.path().join("chatgpt.zip");
    let conversations = json!([
        {
            "mapping": {
                "root": {"message": {"author": {"role": "user"}, "content": {"parts": ["Hello"]}, "create_time": 1773230400.0}, "parent": null},
                "leaf": {"message": {"author": {"role": "assistant"}, "content": {"parts": ["Hi"]}, "create_time": 1773230460.0, "metadata": {"model_slug": "gpt-test"}}, "parent": "root"}
            },
            "current_node": "leaf"
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

pub fn gemini_archive(tree: &TempTree) -> PathBuf {
    let path = tree.path().join("gemini.zip");
    let activities = json!([
        {"time": "2026-03-11T12:00:00Z", "subtitles": [{"value": "prompt only"}], "products": ["Gemini"], "header": "Gemini"},
        {"time": "2026-03-11T12:01:00Z", "safeHtmlItem": [{"html": "<p>response only &amp;&nbsp;&#33;&#x3F;</p>"}], "products": ["Gemini"], "header": "Gemini"},
        {"time": "2026-03-11T12:02:00Z", "subtitles": [{"name": "name fallback"}], "safeHtmlItem": [{"html": "<p>both</p>"}], "products": ["Bard"], "header": "Gemini"},
        {"time": "2026-03-11T12:03:00Z", "safeHtmlItem": [{"html": "<p></p>"}], "products": ["Gemini"], "header": "Bard activity"},
        {"time": "2026-03-11T12:04:00Z", "subtitles": [], "safeHtmlItem": []},
        {"subtitles": [{"value": "missing time"}]},
        {"time": "not-a-time", "subtitles": [{"value": "invalid time"}]}
    ]);
    write_zip(
        &path,
        &[(
            "Takeout/My Activity/Gemini Apps/MyActivity.json".to_owned(),
            activities.to_string().into_bytes(),
        )],
    );
    path
}

pub fn kindle_clippings(tree: &TempTree) -> PathBuf {
    tree.file(
        "My Clippings.txt",
        b"A Book (An Author)\n- Your Highlight on page 1 | Added on Wednesday, March 11, 2026 12:00:00 PM\n\nA highlight\n==========\n",
    )
}
