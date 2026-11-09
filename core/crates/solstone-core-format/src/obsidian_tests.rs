// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};

use super::*;

const GOLDEN: &str = concat!(
    r#"{"change":"changed","changed_at":"2026-11-08T09:42:10-07:00","content_hash":"f0b347123a86bae3e1d4d8c48a7f0430bed090b37d8ed1873a63a97c0b13dfa9","observed_at":"2026-11-08T09:42:33-07:00","path":"daily/2026-11-08.md","previous_path":null,"text":"---\ntags: [walk]\n---\nMet Alex by the river.\n","v":1,"vault":"Notes"}"#,
    "\n",
    r#"{"change":"removed","changed_at":"2026-11-08T09:44:33-07:00","content_hash":null,"observed_at":"2026-11-08T09:44:33-07:00","path":"inbox/old idea.md","previous_path":null,"text":null,"v":1,"vault":"Notes"}"#,
    "\n",
    r#"{"change":"renamed","changed_at":"2026-11-08T09:43:02-07:00","content_hash":"14fece546a747b07dec10e2658310b2d224447158a8c6e1b3a65d467b9de22a3","observed_at":"2026-11-08T09:43:33-07:00","path":"people/Alex Rivera.md","previous_path":"inbox/Alex.md","text":"Alex likes rivers.\n","v":1,"vault":"Notes"}"#,
    "\n",
);
const GOLDEN_NAME: &str =
    "obsidian_3e100d06fb18b4f45302cb3cf2cad1881e7183360649f0543ce4acf565b9f81e.jsonl";

fn named(bytes: &str) -> String {
    format!("obsidian_{:x}.jsonl", Sha256::digest(bytes.as_bytes()))
}

fn line(path: &str, changed_at: &str, text: &str) -> String {
    format!(
        "{}\n",
        serde_json::json!({
            "v": 1, "change": "changed", "vault": "Notes", "path": path,
            "previous_path": null, "changed_at": changed_at,
            "observed_at": changed_at, "text": text,
            "content_hash": format!("{:x}", Sha256::digest(text.as_bytes())),
        })
    )
}

fn refused(bytes: &str, segment: &str) -> ObsidianFileError {
    validate_obsidian_file(&named(bytes), bytes.as_bytes(), "20261108", segment).unwrap_err()
}

#[test]
fn the_golden_file_is_accepted_as_written() {
    assert_eq!(named(GOLDEN), GOLDEN_NAME);
    let changes =
        validate_obsidian_file(GOLDEN_NAME, GOLDEN.as_bytes(), "20261108", "094000_300").unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0].change, Change::Changed);
    assert_eq!(changes[1].change, Change::Removed);
    assert_eq!(changes[2].change, Change::Renamed);
    assert_eq!(changes[2].previous_path.as_deref(), Some("inbox/Alex.md"));
}

#[test]
fn a_file_that_breaks_a_rule_is_refused_whole_and_names_it() {
    let good = line("a.md", "2026-11-08T09:42:10-07:00", "hello");
    // Outside the window.
    let late = line("a.md", "2026-11-08T09:50:10-07:00", "hello");
    assert!(refused(&late, "094000_300").reason.contains("outside"));
    // Off the grid.
    assert!(refused(&good, "094100_300").reason.contains("grid"));
    // A wrong name for the bytes.
    let error =
        validate_obsidian_file(GOLDEN_NAME, good.as_bytes(), "20261108", "094000_300").unwrap_err();
    assert!(error.reason.contains("SHA-256"));
    // A hash that does not match the text.
    let tampered = good.replace("hello", "hellp");
    assert!(
        refused(&tampered, "094000_300")
            .reason
            .contains("content_hash")
    );
    // No trailing newline.
    assert!(
        refused(good.trim_end(), "094000_300")
            .reason
            .contains("newline")
    );
    // A second bad line condemns the file and says which line.
    let bad = format!("{good}{late}");
    assert_eq!(refused(&bad, "094000_300").line, Some(2));
}

#[test]
fn paths_the_contract_forbids_are_refused() {
    for path in [
        "/abs.md",
        "../up.md",
        ".obsidian/x.md",
        "a/.trash/x.md",
        "templates/x.md",
        "a/_Templates/x.md",
        "a/note.txt",
        "a\\b.md",
    ] {
        let bytes = line(path, "2026-11-08T09:42:10-07:00", "x");
        assert!(
            refused(&bytes, "094000_300").reason.contains("path"),
            "{path}"
        );
    }
}

#[test]
fn an_edit_holds_what_was_written_and_nothing_before_it() {
    let old = "# Plan\n\nThe quick fox ran.\n\nSecond para stays.\n";
    let new = "# Plan\n\nThe quick brown fox ran home.\n\nSecond para stays.\n\nA new thought.\n";
    let edit = render_edit(old, new).unwrap();
    assert!(edit.contains("brown"), "{edit}");
    assert!(edit.contains("home."), "{edit}");
    assert!(edit.contains("A new thought."), "{edit}");
    assert!(!edit.contains("quick"), "{edit}");
    assert!(!edit.contains("Second para"), "{edit}");
    assert!(!edit.contains("Plan"), "{edit}");
}

#[test]
fn removed_text_is_counted_and_never_quoted() {
    let edit = render_edit("keep this\nsecret words here\n", "keep this\n").unwrap();
    assert_eq!(edit, "Removed 3 words.");
    let replaced = render_edit("alpha beta\n", "alpha gamma\n").unwrap();
    assert!(replaced.contains("gamma"));
    assert!(!replaced.contains("beta"));
    assert!(replaced.contains("Removed 1 word."));
}

#[test]
fn identical_text_and_line_ending_changes_are_no_edit() {
    assert_eq!(render_edit("same\n", "same\n"), None);
    assert_eq!(render_edit("a\r\nb\r\n", "a\nb\n"), None);
}

#[test]
fn a_frontmatter_edit_is_an_edit() {
    let edit = render_edit(
        "---\ntags: [a]\n---\nbody\n",
        "---\ntags: [a, b]\n---\nbody\n",
    )
    .unwrap();
    assert!(edit.contains('b'), "{edit}");
    assert!(!edit.contains("body"), "{edit}");
}

#[test]
fn link_rewrites_of_a_known_rename_are_folded() {
    let renames = [KnownRename {
        from: "inbox/Alex.md".to_owned(),
        to: "people/Alex Rivera.md".to_owned(),
    }];
    for (old, new) in [
        ("See [[Alex]] today\n", "See [[Alex Rivera]] today\n"),
        (
            "See [[inbox/Alex|him]] and [[Alex#Bio]]\n",
            "See [[people/Alex Rivera|him]] and [[people/Alex Rivera#Bio]]\n",
        ),
        (
            "See [him](inbox/Alex.md) today\n",
            "See [him](people/Alex%20Rivera.md) today\n",
        ),
    ] {
        assert!(only_link_rewrites(old, new, &renames), "{old}");
    }
    // A real edit beside the rewrite is not folded.
    assert!(!only_link_rewrites(
        "See [[Alex]]\n",
        "See [[Alex Rivera]] and more\n",
        &renames
    ));
    // A link changed to a note nobody renamed is the owner's edit.
    assert!(!only_link_rewrites(
        "See [[Alex]]\n",
        "See [[Sam]]\n",
        &renames
    ));
    // With no rename known, nothing is folded.
    assert!(!only_link_rewrites("[[A]]", "[[B]]", &[]));
}

#[test]
fn a_transcript_titles_each_note_and_names_its_path() {
    let text = render_transcript(&[
        NoteEntry {
            path: "people/Alex Rivera.md".to_owned(),
            status: Some("Renamed from inbox/Alex.md".to_owned()),
            body: String::new(),
        },
        NoteEntry {
            path: "daily/2026-11-08.md".to_owned(),
            status: None,
            body: "Met Alex.\n".to_owned(),
        },
    ]);
    assert_eq!(
        text,
        "## Alex Rivera\nSource: people/Alex Rivera.md\nRenamed from inbox/Alex.md\n\n## 2026-11-08\nSource: daily/2026-11-08.md\n\nMet Alex.\n"
    );
}
