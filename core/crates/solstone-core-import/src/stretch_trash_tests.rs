// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]

use super::*;
use serde_json::{Value, json};
use solstone_core_removals::Page;
use std::fs;
use tempfile::TempDir;

const DAY: &str = "20260801";

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// An import of `source` that kept its upload and lists `segments`.
fn import(root: &Path, id: &str, source: &str, segments: &[&str]) {
    let listed: Vec<Value> = segments
        .iter()
        .map(|path| {
            let parts: Vec<&str> = path.split('/').collect();
            json!({"day": parts[1], "stream": parts[2], "segment": parts[3],
                "outcome": {"status": "bound", "prev_day": null, "prev_segment": null, "seq": 1}})
        })
        .collect();
    write(
        root,
        &format!("imports/{id}/imported.json"),
        json!({"schema": "solstone.import.publication.v2", "status": "success",
            "source_type": source, "segments": listed,
            "indexing": {"queued": [], "declined": [], "errored": []}, "day_markers": []})
        .to_string()
        .as_bytes(),
    );
    write(
        root,
        &format!("imports/{id}/import.json"),
        json!({"import_id": id, "original_filename": "talk.m4a",
            "file_path": format!("imports/{id}/talk.m4a")})
        .to_string()
        .as_bytes(),
    );
    write(root, &format!("imports/{id}/talk.m4a"), b"the upload");
    for path in segments {
        write(root, &format!("{path}/audio.flac"), b"sound");
    }
}

fn seg(stream: &str, key: &str) -> String {
    format!("chronicle/{DAY}/{stream}/{key}")
}

/// Inside the import dated inside the stretch.
const INSIDE_ID: &str = "20260801_141000";
/// The import dated across the stretch's end.
const ACROSS_ID: &str = "20260801_152500";

/// The afternoon of acceptance 3: two devices, location, a body window and a
/// day of body totals, an import inside the stretch and one across its end, a
/// segment already in the trash, one still being processed and one arriving.
fn afternoon() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write(
        root,
        "config/journal.json",
        br#"{"setup":{"completed_at":1700000000000}}"#,
    );
    // Two devices: the laptop's audio and screen, the phone's audio and location.
    for key in ["140000_300", "140500_300", "150000_300", "152500_600"] {
        write(root, &format!("{}/audio.flac", seg("laptop", key)), b"a");
        write(root, &format!("{}/screen.webm", seg("laptop", key)), b"s");
    }
    for key in ["135800_300", "141500_300"] {
        write(root, &format!("{}/audio.m4a", seg("phone", key)), b"a");
        write(
            root,
            &format!("{}/location.jsonl", seg("phone", key)),
            b"{}\n",
        );
    }
    // Before and after: not in it.
    write(
        root,
        &format!("{}/audio.flac", seg("laptop", "120000_300")),
        b"a",
    );
    write(
        root,
        &format!("{}/audio.flac", seg("laptop", "170000_300")),
        b"a",
    );
    // A body window (Apple Health, hourly) and the day's totals.
    write(
        root,
        &format!("{}/body.jsonl", seg("import.apple_health", "140000_3600")),
        b"{}\n",
    );
    write(
        root,
        &format!("{}/body.jsonl", seg("import.apple_health", "000000_86400")),
        b"{}\n",
    );
    // An import inside, and one across the end.
    import(
        root,
        INSIDE_ID,
        "audio",
        &[&seg("import.audio", "141000_300")],
    );
    import(
        root,
        ACROSS_ID,
        "audio",
        &[
            &seg("import.audio", "152500_300"),
            &seg("import.audio", "153000_300"),
            &seg("import.audio", "160000_300"),
        ],
    );
    // An agent's memory of the afternoon: not recorded then.
    write(
        root,
        &format!("{}/memory.md", seg("agent-memory-claude", "143000_300")),
        b"m",
    );
    // Arriving and processing.
    write(
        root,
        &format!("{}/audio.flac", seg("phone", "144000_300")),
        b"a",
    );
    write(
        root,
        &format!("{}/.tmp_ingest-stage.tmp", seg("phone", "144000_300")),
        b"",
    );
    write(
        root,
        &format!("{}/audio.flac", seg("phone", "145000_300")),
        b"a",
    );
    write(
        root,
        &format!("{}/.analyzing_audio", seg("phone", "145000_300")),
        b"{}",
    );
    temp
}

fn at(value: &str) -> NaiveDateTime {
    parse_wall(value).unwrap()
}

fn web() -> Surface {
    Surface::Web {
        page: Page::Transcripts,
    }
}

fn taken(plan: &Plan) -> BTreeSet<String> {
    plan.segments.iter().map(Found::rel).collect()
}

fn trash_earlier(root: &Path, rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    let (item, _) = segment_trash::trash_segment(
        root,
        &solstone_core_retention::NoIndex,
        Unit::Segment {
            day: parts[1].into(),
            stream: parts[2].into(),
            segment: parts[3].into(),
        },
        rel.to_owned(),
        Action::SegmentDelete,
        web(),
    )
    .unwrap();
    item.id
}

#[test]
fn a_stretch_takes_every_source_overlapping_it_and_names_what_stays() {
    let temp = afternoon();
    let root = temp.path();
    let earlier = trash_earlier(root, &seg("laptop", "140500_300"));
    let plan = plan(root, at("2026-08-01T14:00"), at("2026-08-01T15:30")).unwrap();
    let taken = taken(&plan);
    let expect: BTreeSet<String> = [
        seg("laptop", "140000_300"),
        seg("laptop", "150000_300"),
        // Overlaps the end: taken whole.
        seg("laptop", "152500_600"),
        // Overlaps the start: taken whole.
        seg("phone", "135800_300"),
        seg("phone", "141500_300"),
        seg("import.apple_health", "140000_3600"),
        seg("import.audio", "141000_300"),
        seg("import.audio", "152500_300"),
        // The laptop's 3:25–3:35 widens the stretch to 3:35, so the import's
        // 3:30 is in it too: everything from the bounds shown is taken.
        seg("import.audio", "153000_300"),
    ]
    .into_iter()
    .collect();
    assert_eq!(taken, expect);
    // Whole segments counted: from the phone's 1:58 to the body hour's 3:00
    // and the laptop's 3:35.
    assert_eq!(plan.from, "2026-08-01T13:58:00");
    assert_eq!(plan.to, "2026-08-01T15:35:00");
    assert_eq!(
        plan.kinds,
        vec!["audio", "body", "imported", "location", "screen"]
    );
    let why: BTreeMap<String, Why> = plan
        .left_out
        .iter()
        .map(|left| (format!("{}/{}", left.stream, left.key), left.why))
        .collect();
    assert_eq!(why.get("phone/144000_300"), Some(&Why::Arriving));
    assert_eq!(why.get("phone/145000_300"), Some(&Why::Processing));
    assert_eq!(
        why.get("import.apple_health/000000_86400"),
        Some(&Why::WholeDay)
    );
    assert_eq!(why.len(), 3, "{why:?}");
    // The import inside goes whole; the one across the end keeps its upload.
    let imports: BTreeMap<&str, bool> = plan
        .imports
        .iter()
        .map(|import| (import.id.as_str(), import.takes))
        .collect();
    assert_eq!(imports.get(INSIDE_ID), Some(&true));
    assert_eq!(imports.get(ACROSS_ID), Some(&false));
    // The segment already in the trash stays its own item, untouched.
    assert!(solstone_core_trash::item(root, &earlier).unwrap().is_some());
}

#[test]
fn a_stretch_is_one_thing_in_the_trash_recovered_and_erased_whole() {
    let temp = afternoon();
    let root = temp.path();
    let earlier = trash_earlier(root, &seg("laptop", "140500_300"));
    let outcome = trash(
        root,
        at("2026-08-01T14:00"),
        at("2026-08-01T15:30"),
        web(),
        false,
    )
    .unwrap()
    .unwrap();
    assert!(outcome.refused.is_empty(), "{:?}", outcome.refused);
    assert_eq!(outcome.imports_taken, vec![INSIDE_ID.to_owned()]);
    let members = solstone_core_trash::stretch_members(root, &outcome.stretch.id).unwrap();
    // Eight segments alone (the across import's two among them) and one
    // import whole.
    assert_eq!(members.len(), 9);
    let erase_at: BTreeSet<&str> = members.iter().map(|item| item.erase_at.as_str()).collect();
    assert_eq!(erase_at.len(), 1, "one erase date: {erase_at:?}");
    assert!(
        members
            .iter()
            .all(|item| item.action == Action::StretchDelete)
    );
    // The inside import's upload is in the trash with it; the across import's
    // stays, and its segment in the stretch names it.
    assert!(!root.join(format!("imports/{INSIDE_ID}")).exists());
    assert!(root.join(format!("imports/{ACROSS_ID}/talk.m4a")).is_file());
    let needing = members
        .iter()
        .find(|item| item.paths == vec![seg("import.audio", "152500_300")])
        .unwrap();
    assert_eq!(needing.needs, vec![format!("imports/{ACROSS_ID}")]);
    // What stays stays.
    assert!(
        root.join(seg("phone", "144000_300"))
            .join("audio.flac")
            .is_file()
    );
    assert!(
        root.join(seg("laptop", "120000_300"))
            .join("audio.flac")
            .is_file()
    );
    assert!(
        root.join(seg("agent-memory-claude", "143000_300"))
            .join("memory.md")
            .is_file()
    );
    // Recover every member: everything comes back as it was, the earlier item
    // stays in the trash.
    for member in &members {
        crate::trash_kinds::with_kinds(root, |kinds| {
            solstone_core_trash::attach(root, kinds, &member.id, Action::Recover, web())
        })
        .unwrap();
    }
    assert!(
        solstone_core_trash::stretch_members(root, &outcome.stretch.id)
            .unwrap()
            .is_empty()
    );
    assert!(
        root.join(seg("laptop", "152500_600"))
            .join("screen.webm")
            .is_file()
    );
    assert!(root.join(format!("imports/{INSIDE_ID}/talk.m4a")).is_file());
    assert!(
        root.join(seg("import.apple_health", "140000_3600"))
            .join("body.jsonl")
            .is_file()
    );
    assert!(solstone_core_trash::item(root, &earlier).unwrap().is_some());
    // Delete it again and erase it: nothing of it is left.
    let again = trash(
        root,
        at("2026-08-01T14:00"),
        at("2026-08-01T15:30"),
        web(),
        true,
    )
    .unwrap()
    .unwrap();
    assert_eq!(again.erased, again.moved);
    assert!(
        solstone_core_trash::stretch_members(root, &again.stretch.id)
            .unwrap()
            .is_empty()
    );
    assert!(!root.join(format!("imports/{INSIDE_ID}")).exists());
    assert!(
        !root
            .join(seg("laptop", "140000_300"))
            .join("audio.flac")
            .exists()
    );
}

#[test]
fn a_stretch_across_midnight_takes_both_days() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write(
        root,
        "chronicle/20260801/laptop/235800_600/audio.flac",
        b"a",
    );
    write(
        root,
        "chronicle/20260802/laptop/000500_300/audio.flac",
        b"a",
    );
    write(
        root,
        "chronicle/20260802/laptop/010000_300/audio.flac",
        b"a",
    );
    let plan = plan(root, at("2026-08-01T23:59"), at("2026-08-02T00:30")).unwrap();
    assert_eq!(
        taken(&plan),
        [
            "chronicle/20260801/laptop/235800_600".to_owned(),
            "chronicle/20260802/laptop/000500_300".to_owned(),
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(plan.from, "2026-08-01T23:58:00");
    assert_eq!(plan.to, "2026-08-02T00:10:00");
}

#[test]
fn an_instant_is_in_a_stretch_only_when_it_falls_inside() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write(
        root,
        "chronicle/20260801/import.image/140000_0/photo.jpg",
        b"p",
    );
    write(
        root,
        "chronicle/20260801/import.image/150000_0/photo.jpg",
        b"p",
    );
    let plan = plan(root, at("2026-08-01T14:00"), at("2026-08-01T15:00")).unwrap();
    assert_eq!(
        taken(&plan),
        ["chronicle/20260801/import.image/140000_0".to_owned()]
            .into_iter()
            .collect()
    );
}

#[test]
fn a_stretch_covering_the_day_takes_its_body_totals() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write(
        root,
        "chronicle/20260801/import.apple_health/000000_86400/body.jsonl",
        b"{}\n",
    );
    let plan = plan(root, at("2026-08-01T00:00"), at("2026-08-02T00:00")).unwrap();
    assert_eq!(plan.segments.len(), 1);
    assert!(plan.left_out.is_empty());
}

#[test]
fn a_backwards_or_too_long_stretch_is_refused() {
    let temp = TempDir::new().unwrap();
    assert_eq!(
        plan(temp.path(), at("2026-08-01T15:00"), at("2026-08-01T14:00")).unwrap_err(),
        Refused::Backwards
    );
    assert_eq!(
        plan(temp.path(), at("2026-08-01T00:00"), at("2026-09-02T00:00")).unwrap_err(),
        Refused::TooLong
    );
}

#[test]
fn a_running_import_is_left_out() {
    let temp = afternoon();
    let root = temp.path();
    let _held = solstone_core_journal_io::hold_lock(
        root.join(format!("imports/{INSIDE_ID}/.attempt")),
        solstone_core_journal_io::LockOptions::default(),
    )
    .unwrap();
    let plan = plan(root, at("2026-08-01T14:00"), at("2026-08-01T15:30")).unwrap();
    assert!(
        plan.left_out
            .iter()
            .any(|left| left.why == Why::ImportRunning
                && left.stream == "import.audio"
                && left.key == "141000_300")
    );
}
