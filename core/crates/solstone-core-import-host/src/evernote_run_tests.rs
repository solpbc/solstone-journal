// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_import::RegistrySource;
use solstone_core_import_sources::document::{
    PdfPage, PdfPayload, PdfWorker, PdfWorkerRequest, WorkerFailure,
};
use solstone_core_import_sources::{NullDocumentModelClient, NullWireClient};
use solstone_core_removals::{Action, Page, Surface, Unit};
use solstone_core_retention::{NoIndex, Target, recording_trash::Recordings};

use super::{IndexScan, SourceSeams, run_bound, run_with_seams};
use crate::cli_argv::{ImportFacet, RegistryDispatch};

struct TextPdf;

impl PdfWorker for TextPdf {
    fn execute(&self, _request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
        Ok(PdfPayload {
            schema: "sol-pdf/1".to_owned(),
            page_count: 1,
            pages: vec![PdfPage {
                index: 1,
                chars: 64,
                text: Some(
                    "a page of text that is long enough to stand in the journal without a model"
                        .to_owned(),
                ),
                ..PdfPage::default()
            }],
            ..PdfPayload::default()
        })
    }
}

fn write_utc_zone(journal: &Path) {
    fs::create_dir_all(journal.join("config")).unwrap();
    fs::write(
        journal.join("config/journal.json"),
        br#"{"identity":{"timezone":"UTC"}}"#,
    )
    .unwrap();
}

fn dispatch(path: &Path, stamp: &str, force: bool, dry_run: bool) -> RegistryDispatch {
    RegistryDispatch {
        media: path.to_path_buf(),
        source: RegistrySource::Evernote,
        timestamp: stamp.to_owned(),
        dry_run,
        json: true,
        force,
        owner_timestamp: None,
        fallback_timestamp: None,
        facet: ImportFacet::Unattended,
        listed_moves_shown: None,
    }
}

fn note(title: &str, created: &str, updated: &str, inner: &str, resources: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <en-export>\n\
         <note><title>{title}</title><created>{created}</created><updated>{updated}</updated>\
         <content><![CDATA[<?xml version=\"1.0\"?><en-note>{inner}</en-note>]]></content>\
         {resources}</note></en-export>\n"
    )
}

fn manifests(journal: &Path) -> Vec<serde_json::Value> {
    let imports = journal.join("imports");
    let mut found = Vec::new();
    if !imports.is_dir() {
        return found;
    }
    for entry in fs::read_dir(&imports).unwrap().flatten() {
        let path = entry.path().join("manifest.json");
        if path.is_file() {
            found.push(serde_json::from_slice(&fs::read(&path).unwrap()).unwrap());
        }
    }
    found
}

#[test]
fn a_note_lands_once_and_a_later_export_of_a_deleted_note_returns() {
    let root = tempfile::tempdir().unwrap();
    write_utc_zone(root.path());
    let path = root.path().join("Inbox.enex");
    fs::write(
        &path,
        note(
            "River",
            "20200101T120000Z",
            "20200101T120000Z",
            "<div>met by the river</div>",
            "",
        ),
    )
    .unwrap();
    let (preview, id) = run_bound(dispatch(&path, "20200101_120000", false, true), root.path());
    assert_eq!(preview.exit_code, 0, "{}", preview.stderr);
    assert!(
        preview
            .stdout
            .contains("solstone.import.evernote.preview.v1"),
        "{}",
        preview.stdout
    );
    assert!(
        preview.stdout.contains("\"notebook\":\"Inbox\""),
        "{}",
        preview.stdout
    );
    assert!(id.is_none());

    let (run, import_id) = run_bound(
        dispatch(&path, "20200101_120000", false, false),
        root.path(),
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let import_id = import_id.unwrap();
    let transcript = fs::read_dir(root.path().join("chronicle/20200101/import.evernote"))
        .unwrap()
        .flatten()
        .map(|entry| fs::read_to_string(entry.path().join("note_transcript.md")).unwrap())
        .next()
        .unwrap();
    assert!(transcript.contains("met by the river"), "{transcript}");
    assert!(transcript.starts_with("## River\nSource:"), "{transcript}");
    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(root.path().join("streams/import.evernote.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["source"], "evernote");
    assert!(record["cid"].is_null());
    let finish = fs::read_to_string(
        root.path()
            .join("imports")
            .join(&import_id)
            .join("finish.txt"),
    )
    .unwrap();
    assert!(finish.contains("brought in 1 notes."), "{finish}");
    let saved = manifests(root.path());
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0]["source_type"], "evernote");

    let (again, _) = run_bound(
        dispatch(&path, "20200102_120000", false, false),
        root.path(),
    );
    assert_eq!(again.exit_code, 0, "{}", again.stderr);
    assert!(
        again.stdout.contains("this file was already imported"),
        "{}",
        again.stdout
    );

    fs::remove_dir_all(root.path().join("chronicle/20200101/import.evernote")).unwrap();
    fs::remove_dir_all(root.path().join("imports").join(&import_id)).unwrap();
    let later = root.path().join("Inbox (2).enex");
    fs::write(
        &later,
        note(
            "River",
            "20200101T120000Z",
            "20200104T120000Z",
            "<div>a later sentence</div>",
            "",
        ),
    )
    .unwrap();
    let (trashed, _) = run_bound(
        dispatch(&later, "20200104_120000", false, false),
        root.path(),
    );
    assert_eq!(trashed.exit_code, 0, "{}", trashed.stderr);
    let text = fs::read_to_string(
        root.path()
            .join("chronicle/20200104/import.evernote/120000_300/note_transcript.md"),
    )
    .unwrap();
    assert!(text.contains("a later sentence"), "{text}");
}

#[test]
fn an_image_and_a_pdf_come_in_with_the_note_and_open_no_second_import() {
    let root = tempfile::tempdir().unwrap();
    write_utc_zone(root.path());
    // STANDARD base64 and lowercase md5 of PNG and of b"%PDF-1.4 note". The host crate does not depend on those crates.
    let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAACklEQVR4nGMAAQAABQABDQottAAAAABJRU5ErkJggg==";
    let pdf_b64 = "JVBERi0xLjQgbm90ZQ==";
    let png_hash = "93ca32a536da1698ea979f183679af29";
    let pdf_hash = "d5c4275571fefdc8ef063247b9cca9eb";
    let path = root.path().join("Files.enex");
    fs::write(
        &path,
        note(
            "Files",
            "20200101T120000Z",
            "20200101T120500Z",
            &format!("<div>with files</div><en-media hash=\"{png_hash}\"/><en-media hash=\"{pdf_hash}\"/>"),
            &format!(
                "<resource><data encoding=\"base64\">{png_b64}</data><mime>image/png</mime><file-name>dot.png</file-name></resource>\
                 <resource><data encoding=\"base64\">{pdf_b64}</data><mime>application/pdf</mime><file-name>page.pdf</file-name></resource>"
            ),
        ),
    )
    .unwrap();
    let pdf_worker = TextPdf;
    let model = NullDocumentModelClient;
    let image = NullWireClient;
    let reindex = IndexScan {
        journal: root.path().to_path_buf(),
    };
    let seams = SourceSeams {
        pdf: &pdf_worker,
        model: &model,
        image: &image,
        reindex: &reindex,
    };
    let (run, _) = run_with_seams(
        dispatch(&path, "20200101_120500", false, false),
        root.path(),
        &seams,
    );
    assert_eq!(run.exit_code, 0, "{}\n{}", run.stderr, run.stdout);
    assert!(
        root.path()
            .join("chronicle/20200101/import.evernote")
            .is_dir()
    );
    assert!(root.path().join("chronicle/20200101/import.image").is_dir());
    assert!(
        root.path()
            .join("chronicle/20200101/import.document")
            .is_dir()
    );
    let saved = manifests(root.path());
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[0]["source_type"], "evernote");
    let files = saved[0]["files_created"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        files
            .iter()
            .any(|file| file.as_str().unwrap_or("").contains("import.image"))
    );
    assert!(
        files
            .iter()
            .any(|file| file.as_str().unwrap_or("").contains("import.document"))
    );
    let image_files = files_under(&root.path().join("chronicle/20200101/import.image"));
    assert!(
        image_files
            .iter()
            .any(|path| path.ends_with("original.png"))
    );
    let doc_text = files_under(&root.path().join("chronicle/20200101/import.document"))
        .into_iter()
        .filter(|path| path.ends_with("document_transcript.md"))
        .map(|path| fs::read_to_string(path).unwrap())
        .collect::<String>();
    assert!(doc_text.contains("long enough"), "{doc_text}");
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !dir.is_dir() {
        return found;
    }
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found
}

fn surface() -> Surface {
    Surface::Web {
        page: Page::Transcripts,
    }
}

fn import_note(root: &Path, stamp: &str, updated: &str, text: &str) -> String {
    let path = root.join("Inbox.enex");
    fs::write(&path, note("River", "20200101T120000Z", updated, text, "")).unwrap();
    let (run, id) = run_bound(dispatch(&path, stamp, false, false), root);
    assert_eq!(run.exit_code, 0, "{}\n{}", run.stderr, run.stdout);
    id.unwrap()
}

fn combined_notes(notes: &[String]) -> String {
    let entries: String = notes
        .iter()
        .map(|note| {
            note.split_once("<en-export>\n")
                .unwrap()
                .1
                .split_once("</en-export>")
                .unwrap()
                .0
        })
        .collect();
    format!("<en-export>{entries}</en-export>")
}

fn kept_notes(root: &Path) -> Vec<PathBuf> {
    files_under(&root.join("config/evernote-imports"))
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect()
}

fn trash_note(root: &Path, day: &str, stream: &str) -> solstone_core_trash::Item {
    Recordings::new(&NoIndex, &solstone_core_retention::made_from::NothingMade)
        .trash(
            root,
            &Target {
                day: day.into(),
                stream: stream.into(),
                dir: "120000_300".into(),
            },
            Unit::Segment {
                day: day.into(),
                stream: stream.into(),
                segment: "120000_300".into(),
            },
            Action::SegmentDelete,
            surface(),
        )
        .unwrap()
}

fn recover(root: &Path, item: &solstone_core_trash::Item) {
    solstone_core_import::trash_kinds::with_kinds(root, |kinds| {
        solstone_core_trash::attach(root, kinds, &item.id, Action::Recover, surface())
    })
    .unwrap();
}

#[test]
fn erased_evernote_import_takes_kept_text_and_same_file_returns() {
    let root = tempfile::tempdir().unwrap();
    write_utc_zone(root.path());
    let id = import_note(
        root.path(),
        "20200101_120000",
        "20200101T120000Z",
        "<div>old kept text</div>",
    );
    let kept = kept_notes(root.path());
    assert_eq!(kept.len(), 1);
    let item = solstone_core_import::import_trash::Imports::new(
        &NoIndex,
        &solstone_core_retention::made_from::NothingMade,
    )
    .trash(root.path(), &id, Action::ImportDelete, surface())
    .unwrap();
    assert!(kept.iter().all(|path| !path.exists()));
    assert!(
        item.paths
            .iter()
            .any(|path| path.starts_with("config/evernote-imports/"))
    );
    solstone_core_trash::erase(root.path(), &item.id, Action::Erase, surface()).unwrap();
    assert!(kept_notes(root.path()).is_empty());
    assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
    let new_id = import_note(
        root.path(),
        "20200102_120000",
        "20200101T120000Z",
        "<div>old kept text</div>",
    );
    assert_ne!(new_id, id);
    assert!(
        fs::read_to_string(
            root.path()
                .join("chronicle/20200101/import.evernote/120000_300/note_transcript.md")
        )
        .unwrap()
        .contains("old kept text")
    );
    assert_eq!(kept_notes(root.path()).len(), 1);
    println!(
        "whole-import demo: erase removed the kept record; the identical export brought the note back"
    );
}

#[test]
fn erased_evernote_note_takes_kept_text_and_changed_export_returns() {
    let root = tempfile::tempdir().unwrap();
    write_utc_zone(root.path());
    let live = note(
        "Hill",
        "20200103T120000Z",
        "20200103T120000Z",
        "<div>unchanged live note</div>",
        "",
    );
    let old = note(
        "River",
        "20200101T120000Z",
        "20200101T120000Z",
        "<div>old private marker</div>",
        "",
    );
    let path = root.path().join("Inbox.enex");
    fs::write(&path, combined_notes(&[old, live.clone()])).unwrap();
    let (run, id) = run_bound(
        dispatch(&path, "20200103_120000", false, false),
        root.path(),
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let id = id.unwrap();
    let kept = kept_notes(root.path());
    assert_eq!(kept.len(), 2);
    let live_path = kept
        .iter()
        .find(|path| {
            fs::read_to_string(path)
                .unwrap()
                .contains("unchanged live note")
        })
        .unwrap()
        .clone();
    let live_bytes = fs::read(&live_path).unwrap();
    let item = trash_note(root.path(), "20200101", "import.evernote");
    solstone_core_trash::erase(root.path(), &item.id, Action::Erase, surface()).unwrap();
    assert_eq!(kept_notes(root.path()), vec![live_path.clone()]);
    assert_eq!(fs::read(&live_path).unwrap(), live_bytes);
    // A single-note deletion leaves its import live, and Evernote places by fixed
    // keys: the identical file comes through and brings back only the deleted
    // note, leaving the live one byte-identical.
    let (same, claimed) = run_bound(
        dispatch(&path, "20200104_120000", false, false),
        root.path(),
    );
    assert_eq!(same.exit_code, 0, "{}", same.stderr);
    assert!(claimed.is_some(), "{}", same.stdout);
    assert!(root.path().join("imports").join(id).is_dir());
    assert_eq!(fs::read(&live_path).unwrap(), live_bytes);
    assert!(
        fs::read_to_string(
            root.path()
                .join("chronicle/20200101/import.evernote/120000_300/note_transcript.md"),
        )
        .is_ok_and(|text| text.contains("old private marker")),
        "the deleted note is back"
    );
    // Deleted again, for what follows.
    let item = trash_note(root.path(), "20200101", "import.evernote");
    solstone_core_trash::erase(root.path(), &item.id, Action::Erase, surface()).unwrap();
    let changed = note(
        "River",
        "20200101T120000Z",
        "20200102T120000Z",
        "<div>new private marker</div>",
        "",
    );
    fs::write(&path, combined_notes(&[changed, live])).unwrap();
    let (run, new_id) = run_bound(
        dispatch(&path, "20200105_120000", false, false),
        root.path(),
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join("imports")
                .join(new_id.unwrap())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["entry_count"], 1);
    let kept = kept_notes(root.path());
    assert_eq!(kept.len(), 2);
    assert_eq!(fs::read(&live_path).unwrap(), live_bytes);
    assert!(kept.iter().all(|path| {
        !fs::read_to_string(path)
            .unwrap()
            .contains("old private marker")
    }));
    let text = fs::read_to_string(
        root.path()
            .join("chronicle/20200102/import.evernote/120000_300/note_transcript.md"),
    )
    .unwrap();
    assert!(text.contains("new private marker"));
    println!(
        "single-note demo: old kept text erased; the identical file brought back only the deleted note; the live note stayed byte-identical"
    );
}

#[test]
fn shared_evernote_segment_keeps_its_record_until_the_last_import_leaves() {
    let root = tempfile::tempdir().unwrap();
    write_utc_zone(root.path());
    let first = import_note(
        root.path(),
        "20200101_120000",
        "20200101T120000Z",
        "<div>first</div>",
    );
    let second = import_note(
        root.path(),
        "20200102_120000",
        "20200101T120100Z",
        "<div>changed</div>",
    );
    let kept = kept_notes(root.path());
    assert_eq!(kept.len(), 1);
    let imports = solstone_core_import::import_trash::Imports::new(
        &NoIndex,
        &solstone_core_retention::made_from::NothingMade,
    );
    let one = imports
        .trash(root.path(), &first, Action::ImportDelete, surface())
        .unwrap();
    solstone_core_trash::erase(root.path(), &one.id, Action::Erase, surface()).unwrap();
    assert!(kept[0].exists());
    let two = imports
        .trash(root.path(), &second, Action::ImportDelete, surface())
        .unwrap();
    assert!(!kept[0].exists());
    recover(root.path(), &two);
    assert!(kept[0].exists());
    assert!(fs::read_to_string(&kept[0]).unwrap().contains("changed"));
}

#[test]
fn another_stream_at_the_same_time_keeps_evernote_text_untouched() {
    let root = tempfile::tempdir().unwrap();
    write_utc_zone(root.path());
    import_note(
        root.path(),
        "20200101_120000",
        "20200101T120000Z",
        "<div>keep me</div>",
    );
    let kept = kept_notes(root.path());
    let unrelated = root.path().join("chronicle/20200101/phone/120000_300");
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(unrelated.join("note_transcript.md"), "other source").unwrap();
    let item = trash_note(root.path(), "20200101", "phone");
    solstone_core_trash::erase(root.path(), &item.id, Action::Erase, surface()).unwrap();
    assert!(kept[0].exists());
    assert!(
        root.path()
            .join("chronicle/20200101/import.evernote/120000_300/note_transcript.md")
            .exists()
    );
}

#[test]
fn an_unreadable_evernote_record_refuses_before_anything_moves() {
    let root = tempfile::tempdir().unwrap();
    write_utc_zone(root.path());
    let id = import_note(
        root.path(),
        "20200101_120000",
        "20200101T120000Z",
        "<div>keep me</div>",
    );
    fs::write(&kept_notes(root.path())[0], b"broken json").unwrap();
    assert!(
        solstone_core_import::import_trash::Imports::new(
            &NoIndex,
            &solstone_core_retention::made_from::NothingMade
        )
        .trash(root.path(), &id, Action::ImportDelete, surface())
        .is_err()
    );
    assert!(root.path().join("imports").join(id).is_dir());
    assert!(
        root.path()
            .join("chronicle/20200101/import.evernote/120000_300/note_transcript.md")
            .exists()
    );
    assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
}

#[test]
fn recovered_collision_records_do_not_duplicate_an_unchanged_note() {
    let root = tempfile::tempdir().unwrap();
    write_utc_zone(root.path());
    let alpha = note(
        "Alpha",
        "20200101T120000Z",
        "20200101T120000Z",
        "<div>alpha text</div>",
        "",
    );
    let beta = note(
        "Beta",
        "20200101T120000Z",
        "20200102T120000Z",
        "<div>beta text</div>",
        "",
    );
    let path = root.path().join("Inbox.enex");
    let inner = |s: &str| {
        s.split_once("<en-export>\n")
            .unwrap()
            .1
            .split_once("</en-export>")
            .unwrap()
            .0
            .to_owned()
    };
    fs::write(
        &path,
        format!("<en-export>{}{}</en-export>", inner(&alpha), inner(&beta)),
    )
    .unwrap();
    let (run, _) = run_bound(
        dispatch(&path, "20200103_120000", false, false),
        root.path(),
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    assert_eq!(kept_notes(root.path()).len(), 2);
    let item = trash_note(root.path(), "20200101", "import.evernote");
    fs::write(&path, &alpha).unwrap();
    let (run, _) = run_bound(
        dispatch(&path, "20200104_120000", false, false),
        root.path(),
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    recover(root.path(), &item);
    // The recovered plain key and the re-import's disambiguated key both
    // describe Alpha; Beta is a different note with the same creation second.
    assert_eq!(kept_notes(root.path()).len(), 3);
    fs::write(&path, format!("{alpha}\n")).unwrap();
    let (run, id) = run_bound(
        dispatch(&path, "20200105_120000", false, false),
        root.path(),
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join("imports")
                .join(id.unwrap())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["entry_count"], 0);
    assert_eq!(kept_notes(root.path()).len(), 3);
    let changed = note(
        "Alpha",
        "20200101T120000Z",
        "20200106T120000Z",
        "<div>alpha changed</div>",
        "",
    );
    fs::write(&path, &changed).unwrap();
    let (run, _) = run_bound(
        dispatch(&path, "20200106_120000", false, false),
        root.path(),
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let text = fs::read_to_string(
        root.path()
            .join("chronicle/20200106/import.evernote/120000_300/note_transcript.md"),
    )
    .unwrap();
    assert!(text.contains("changed"), "{text}");
    assert!(
        !text.contains("alpha changed"),
        "the edit must contain the change, not the whole note: {text}"
    );
    assert_eq!(kept_notes(root.path()).len(), 3);
    fs::write(&path, format!("{changed}\n")).unwrap();
    let (run, id) = run_bound(
        dispatch(&path, "20200107_120000", false, false),
        root.path(),
    );
    assert_eq!(run.exit_code, 0, "{}", run.stderr);
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join("imports")
                .join(id.unwrap())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["entry_count"], 0);
}
