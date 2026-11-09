// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::DateTime;
use sha2::{Digest, Sha256};
use solstone_core_format::obsidian::{Change, NoteChange};

use super::*;

fn at(clock: &str) -> chrono::DateTime<chrono::FixedOffset> {
    DateTime::parse_from_rfc3339(&format!("2026-11-08T{clock}-07:00")).unwrap()
}

fn change(kind: Change, path: &str, clock: &str, text: Option<&str>) -> NoteChange {
    NoteChange {
        change: kind,
        vault: "Notes".to_owned(),
        path: path.to_owned(),
        previous_path: None,
        changed_at: at(clock),
        observed_at: at(clock),
        text: text.map(str::to_owned),
        content_hash: text.map(|t| format!("{:x}", Sha256::digest(t.as_bytes()))),
    }
}

fn changed(path: &str, clock: &str, text: &str) -> NoteChange {
    change(Change::Changed, path, clock, Some(text))
}

fn renamed(from: &str, to: &str, clock: &str, text: &str) -> NoteChange {
    NoteChange {
        previous_path: Some(from.to_owned()),
        ..change(Change::Renamed, to, clock, Some(text))
    }
}

fn removed(path: &str, clock: &str) -> NoteChange {
    change(Change::Removed, path, clock, None)
}

/// Apply an upload and commit it, returning its memory as text.
fn apply(journal: &Path, cid: &str, changes: Vec<NoteChange>) -> Option<String> {
    let prepared = prepare(journal, changes).unwrap();
    let text = prepared
        .transcript
        .clone()
        .map(|bytes| String::from_utf8(bytes).unwrap());
    prepared.commit(journal, cid).unwrap();
    text
}

#[test]
fn a_note_seen_for_the_first_time_is_the_whole_note_frontmatter_included() {
    let journal = tempfile::tempdir().unwrap();
    let text = "---\ntags: [walk]\n---\nMet Alex by the river.\n";
    let memory = apply(
        journal.path(),
        "a",
        vec![changed("daily/today.md", "09:42:10", text)],
    )
    .unwrap();
    assert_eq!(
        memory,
        "## today\nSource: daily/today.md\n\n---\ntags: [walk]\n---\nMet Alex by the river.\n"
    );
}

#[test]
fn an_edit_holds_only_what_changed() {
    let journal = tempfile::tempdir().unwrap();
    apply(
        journal.path(),
        "a",
        vec![changed("a.md", "09:42:10", "first thoughts here\n")],
    );
    let memory = apply(
        journal.path(),
        "a",
        vec![changed(
            "a.md",
            "10:02:10",
            "first thoughts here\n\nsecond thoughts arrive\n",
        )],
    )
    .unwrap();
    assert!(memory.contains("Edited"), "{memory}");
    assert!(memory.contains("second thoughts arrive"), "{memory}");
    assert!(!memory.contains("first thoughts"), "{memory}");
}

#[test]
fn forty_saves_in_one_window_are_one_entry_the_net_change() {
    let journal = tempfile::tempdir().unwrap();
    apply(
        journal.path(),
        "a",
        vec![changed("a.md", "09:00:10", "start\n")],
    );
    let saves = (0..40)
        .map(|save| {
            changed(
                "a.md",
                &format!("10:0{}:{:02}", save / 20, (save % 20) * 2 + 1),
                &format!("start\ntyped {save}\n"),
            )
        })
        .collect();
    let memory = apply(journal.path(), "a", saves).unwrap();
    assert_eq!(memory.matches("## a").count(), 1, "{memory}");
    assert!(memory.contains("typed 39"), "{memory}");
    assert!(!memory.contains("typed 0\n"), "{memory}");
}

#[test]
fn a_rename_is_one_entry_and_the_next_edit_is_a_change_not_a_first_sight() {
    let journal = tempfile::tempdir().unwrap();
    apply(
        journal.path(),
        "a",
        vec![changed("inbox/Alex.md", "09:00:10", "Alex likes rivers.\n")],
    );
    let memory = apply(
        journal.path(),
        "a",
        vec![renamed(
            "inbox/Alex.md",
            "people/Alex Rivera.md",
            "09:10:00",
            "Alex likes rivers.\n",
        )],
    )
    .unwrap();
    assert_eq!(
        memory,
        "## Alex Rivera\nSource: people/Alex Rivera.md\nRenamed from inbox/Alex.md\n"
    );
    let edit = apply(
        journal.path(),
        "a",
        vec![changed(
            "people/Alex Rivera.md",
            "09:30:00",
            "Alex likes rivers.\nAnd long walks.\n",
        )],
    )
    .unwrap();
    assert!(edit.contains("Edited"), "{edit}");
    assert!(edit.contains("And long walks."), "{edit}");
    assert!(!edit.contains("likes rivers"), "{edit}");
}

#[test]
fn renaming_a_linked_note_folds_the_link_rewrites_into_it() {
    let journal = tempfile::tempdir().unwrap();
    let mut seed = vec![changed("Alex.md", "09:00:00", "Alex.\n")];
    let linking = (0..5)
        .map(|index| format!("n{index}.md"))
        .collect::<Vec<_>>();
    for name in &linking {
        seed.push(changed(name, "09:00:00", "See [[Alex]] today.\n"));
    }
    apply(journal.path(), "a", seed);

    // Same upload.
    let mut upload = vec![renamed("Alex.md", "Alex Rivera.md", "10:00:00", "Alex.\n")];
    for name in &linking {
        upload.push(changed(name, "10:00:02", "See [[Alex Rivera]] today.\n"));
    }
    let memory = apply(journal.path(), "a", upload).unwrap();
    assert_eq!(memory.matches("## ").count(), 1, "{memory}");
    assert!(memory.contains("Renamed from Alex.md"), "{memory}");

    // A rewrite arriving in a later window folds too.
    apply(
        journal.path(),
        "a",
        vec![changed("late.md", "10:05:00", "See [[Alex Rivera]].\n")],
    );
    apply(
        journal.path(),
        "a",
        vec![renamed(
            "Alex Rivera.md",
            "Alex R.md",
            "10:10:00",
            "Alex.\n",
        )],
    );
    assert_eq!(
        apply(
            journal.path(),
            "a",
            vec![changed("late.md", "10:15:30", "See [[Alex R]].\n")],
        ),
        None
    );
}

#[test]
fn a_delete_is_one_entry_and_the_note_is_forgotten() {
    let journal = tempfile::tempdir().unwrap();
    apply(
        journal.path(),
        "a",
        vec![changed("a.md", "09:00:00", "text\n")],
    );
    let memory = apply(journal.path(), "a", vec![removed("a.md", "09:10:00")]).unwrap();
    assert_eq!(memory, "## a\nSource: a.md\nDeleted\n");
    assert!(!note_file(journal.path(), "Notes", "a.md").exists());
    assert_eq!(
        apply(journal.path(), "a", vec![removed("a.md", "09:20:00")]),
        None
    );
}

#[test]
fn a_second_device_reporting_what_the_journal_has_changes_nothing() {
    let journal = tempfile::tempdir().unwrap();
    apply(
        journal.path(),
        "a",
        vec![
            changed("a.md", "09:00:00", "text\n"),
            renamed("a.md", "b.md", "09:01:00", "text\n"),
        ],
    );
    assert_eq!(
        apply(
            journal.path(),
            "b",
            vec![changed("b.md", "09:05:00", "text\n")]
        ),
        None
    );
    assert_eq!(
        apply(
            journal.path(),
            "b",
            vec![renamed("a.md", "b.md", "09:06:00", "text\n")]
        ),
        None
    );
}

#[test]
fn a_change_older_than_the_journal_holds_is_stale() {
    let journal = tempfile::tempdir().unwrap();
    apply(
        journal.path(),
        "a",
        vec![changed("a.md", "10:00:00", "newer\n")],
    );
    assert_eq!(
        apply(
            journal.path(),
            "a",
            vec![changed("a.md", "09:00:00", "older\n")]
        ),
        None
    );
}

#[test]
fn nothing_is_committed_until_the_memory_is_durable() {
    let journal = tempfile::tempdir().unwrap();
    let upload = vec![changed("a.md", "09:00:00", "text\n")];
    let prepared = prepare(journal.path(), upload.clone()).unwrap();
    let first = prepared.transcript.clone().unwrap();
    drop(prepared);
    assert!(!note_file(journal.path(), "Notes", "a.md").exists());
    // A crash here recomputes the same bytes, so the resend is a duplicate.
    let retried = prepare(journal.path(), upload).unwrap();
    assert_eq!(retried.transcript.unwrap(), first);
}

#[test]
fn the_vault_list_leaves_out_the_asking_device() {
    let journal = tempfile::tempdir().unwrap();
    apply(
        journal.path(),
        "a",
        vec![changed("a.md", "09:00:00", "text\n")],
    );
    let held = read_vault(journal.path(), "Notes").unwrap();
    assert!(held.devices.contains_key("a"));
    assert!(listed_vaults(journal.path(), "a").unwrap().is_empty());
    let listed = listed_vaults(journal.path(), "b").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["vault"], "Notes");
    assert_eq!(listed[0]["last_synced_at"], "2026-11-08T09:00:00-07:00");
}

#[test]
fn a_link_edit_long_after_a_rename_is_the_owners_own() {
    let journal = tempfile::tempdir().unwrap();
    apply(
        journal.path(),
        "a",
        vec![
            changed("A.md", "01:00:00", "a\n"),
            changed("n.md", "01:00:00", "See [[A]].\n"),
        ],
    );
    apply(
        journal.path(),
        "a",
        vec![renamed("A.md", "B.md", "01:10:00", "a\n")],
    );
    // Two days on, the rename is too far away to explain this edit.
    let mut late = changed("n.md", "23:00:00", "See [[B]].\n");
    late.changed_at += chrono::Duration::days(2);
    let memory = apply(journal.path(), "a", vec![late]).unwrap();
    assert!(memory.contains("Edited"), "{memory}");
}

#[test]
fn a_resend_rewrites_no_note_state() {
    let journal = tempfile::tempdir().unwrap();
    let upload = vec![changed("a.md", "09:00:00", "text\n")];
    apply(journal.path(), "a", upload.clone());
    let prepared = prepare(journal.path(), upload).unwrap();
    assert!(prepared.notes.is_empty());
}

#[test]
fn the_vault_list_names_a_device_by_the_label_it_paired_with() {
    let journal = tempfile::tempdir().unwrap();
    apply(
        journal.path(),
        "sha256:other",
        vec![changed("a.md", "09:00:00", "text\n")],
    );
    let path = journal.path().join("link/authorized_clients.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::json!([{"fingerprint":"sha256:other","device_label":"studio-rig",
            "paired_at":"2026-04-19T00:00:00Z","instance_id":"i"}])
        .to_string(),
    )
    .unwrap();
    let listed = listed_vaults(journal.path(), "sha256:me").unwrap();
    assert_eq!(listed[0]["device"], "studio-rig");
}
