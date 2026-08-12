// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use chrono::{DateTime, Local, TimeZone, Utc};
use serde_json::Value;
use solstone_core_import_sources::obsidian;

static NEXT_TREE: AtomicUsize = AtomicUsize::new(0);
const ORACLE: &str = include_str!("../../../fixtures/import_source_preview_oracle.json");
const MTIME_CAVEAT: &str = "; date range reflects file modification time";

#[test]
fn obsidian_oracle_detect_and_preview_match_fixture() {
    let tree = Tree::new();
    let vault = tree.directory("vault-oracle");
    tree.file("vault-oracle/.obsidian/app.json", "{}");
    let daily = tree.file("vault-oracle/2026-08-11.md", "# Daily\n[[Shared Topic]]\n");
    let idea = tree.file("vault-oracle/Idea.md", "# Idea\n");
    let reference = tree.file("vault-oracle/Reference.md", "# Reference\n");
    for path in [&daily, &idea, &reference] {
        set_modified(path, 2026, 8, 11);
    }
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    let expected = &oracle["cases"]["obsidian"];

    assert_eq!(
        obsidian::detect(&vault),
        expected["detect"].as_bool().unwrap()
    );
    let preview = obsidian::preview(&vault).unwrap();
    assert_eq!(
        preview.date_range.0,
        expected["preview"]["date_range"][0].as_str().unwrap()
    );
    assert_eq!(
        preview.date_range.1,
        expected["preview"]["date_range"][1].as_str().unwrap()
    );
    assert_eq!(
        preview.item_count,
        expected["preview"]["item_count"].as_u64().unwrap()
    );
    assert_eq!(
        preview.entity_count,
        expected["preview"]["entity_count"].as_u64().unwrap()
    );
    assert_eq!(
        preview.summary,
        format!(
            "{}{MTIME_CAVEAT}",
            expected["preview"]["summary"].as_str().unwrap()
        )
    );
}

#[test]
fn obsidian_preview_uses_constructed_mtimes_not_the_clock() {
    let tree = Tree::new();
    let vault = tree.directory("vault-mtime-range");
    let daily = tree.file("vault-mtime-range/2024-01-02.md", "# Daily\n");
    let knowledge = tree.file("vault-mtime-range/Knowledge.md", "# Knowledge\n");
    let daily_day = set_modified(&daily, 2024, 1, 2);
    let knowledge_day = set_modified(&knowledge, 2024, 3, 4);

    let preview = obsidian::preview(&vault).unwrap();
    assert_eq!(
        preview.date_range,
        (local_day(daily_day), local_day(knowledge_day))
    );
}

#[test]
fn collect_notes_and_wikilink_entities_preserve_type_precedence() {
    let tree = Tree::new();
    let vault = tree.directory("vault-entities");
    tree.file("vault-entities/2024_01_03.md", "# Daily\n");
    tree.file(
        "vault-entities/People/Aster Placeholder.md",
        "---\ntags: [sample, \"person\"]\n---\n# Aster\n",
    );
    tree.file("vault-entities/Projects/Nova Placeholder.md", "# Nova\n");
    tree.file(
        "vault-entities/Organizations/Topic Placeholder.md",
        "# Topic\n",
    );
    tree.file("vault-entities/@Offline Placeholder.md", "# Offline\n");
    tree.file(
        "vault-entities/Reference.md",
        "[[Aster Placeholder|Aster]] [[Nova Placeholder]] [[Topic Placeholder]] [[@Topic Placeholder]] [[Loose Topic]]\n",
    );
    tree.file(
        "vault-entities/Templates/Skipped.md",
        "[[Skipped Template]]\n",
    );
    tree.file(
        "vault-entities/logseq/.recycle/Skipped.md",
        "[[Skipped Recycle]]\n",
    );

    let notes = obsidian::collect_notes(&vault).unwrap();
    assert_eq!(notes.len(), 6);
    let daily = notes
        .iter()
        .find(|note| note.title == "2024_01_03")
        .unwrap();
    assert!(daily.is_daily);
    assert_eq!(daily.daily_note_day.as_deref(), Some("20240103"));
    let aster = notes
        .iter()
        .find(|note| note.title == "Aster Placeholder")
        .unwrap();
    assert_eq!(aster.inferred_entity_type.as_deref(), Some("Person"));
    assert_eq!(aster.tags, vec!["sample", "person"]);

    let entities = obsidian::wikilink_entities(&notes);
    assert_eq!(
        entities
            .iter()
            .map(|entity| (entity.name.as_str(), entity.entity_type.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("Aster Placeholder", "Person"),
            ("Loose Topic", "Topic"),
            ("Nova Placeholder", "Project"),
            ("Offline Placeholder", "Person"),
            ("Topic Placeholder", "Person"),
        ]
    );
}

#[test]
fn folder_type_numeric_prefix_requires_whitespace() {
    let tree = Tree::new();
    let vault = tree.directory("vault-numeric-folders");
    tree.file("vault-numeric-folders/01Projects/Joined.md", "# Joined\n");
    tree.file(
        "vault-numeric-folders/01 Projects/Separated.md",
        "# Separated\n",
    );
    tree.file(
        "vault-numeric-folders/Reference.md",
        "[[Joined]] [[Separated]]\n",
    );

    let notes = obsidian::collect_notes(&vault).unwrap();
    assert_eq!(
        notes
            .iter()
            .find(|note| note.title == "Joined")
            .unwrap()
            .inferred_entity_type,
        None
    );
    assert_eq!(
        notes
            .iter()
            .find(|note| note.title == "Separated")
            .unwrap()
            .inferred_entity_type
            .as_deref(),
        Some("Project")
    );
    assert_eq!(
        obsidian::wikilink_entities(&notes)
            .iter()
            .map(|entity| (entity.name.as_str(), entity.entity_type.as_str()))
            .collect::<Vec<_>>(),
        vec![("Joined", "Topic"), ("Separated", "Project")]
    );
}

#[test]
fn preview_and_collection_include_empty_notes() {
    let tree = Tree::new();
    let vault = tree.directory("vault-empty-note");
    let normal = tree.file(
        "vault-empty-note/2024-01-02.md",
        "# Daily\n[[Visible Topic]]\n",
    );
    let empty = tree.file("vault-empty-note/Empty.md", " \n\t");
    let normal_day = set_modified(&normal, 2024, 1, 2);
    let empty_day = set_modified(&empty, 2024, 1, 3);

    let notes = obsidian::collect_notes(&vault).unwrap();
    assert_eq!(notes.len(), 2);
    let empty_note = notes.iter().find(|note| note.title == "Empty").unwrap();
    assert!(empty_note.content.trim().is_empty());
    assert!(empty_note.wikilinks.is_empty());

    let preview = obsidian::preview(&vault).unwrap();
    assert_eq!(preview.item_count, 2);
    assert_eq!(
        preview.date_range,
        (local_day(normal_day), local_day(empty_day))
    );
    assert_eq!(preview.entity_count, 1);
}

#[test]
fn note_entries_use_mtime_day_for_daily_and_knowledge_notes() {
    let tree = Tree::new();
    let vault = tree.directory("vault-writer-days");
    let daily = tree.file("vault-writer-days/1999-12-31.md", "# Daily\n");
    let knowledge = tree.file("vault-writer-days/Knowledge.md", "# Knowledge\n");
    let daily_day = set_modified(&daily, 2024, 4, 5);
    let knowledge_day = set_modified(&knowledge, 2024, 6, 7);

    let notes = obsidian::collect_notes(&vault).unwrap();
    let daily = notes
        .iter()
        .find(|note| note.title == "1999-12-31")
        .unwrap();
    let knowledge = notes.iter().find(|note| note.title == "Knowledge").unwrap();
    assert_eq!(daily.daily_note_day.as_deref(), Some("19991231"));
    assert_eq!(daily.day, local_day(daily_day));
    assert_eq!(knowledge.day, local_day(knowledge_day));
}

fn set_modified(path: &Path, year: i32, month: u32, day: u32) -> SystemTime {
    let modified: SystemTime = Utc
        .with_ymd_and_hms(year, month, day, 12, 0, 0)
        .single()
        .unwrap()
        .into();
    fs::File::open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    modified
}

fn local_day(modified: SystemTime) -> String {
    DateTime::<Local>::from(modified)
        .format("%Y%m%d")
        .to_string()
}

struct Tree(PathBuf);

impl Tree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-core-import-sources-obsidian-{}-{}",
            std::process::id(),
            NEXT_TREE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.path());
    }
}
