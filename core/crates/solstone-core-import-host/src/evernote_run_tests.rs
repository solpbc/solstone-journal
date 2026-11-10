use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_import::RegistrySource;
use solstone_core_import_sources::document::{
    PdfPage, PdfPayload, PdfWorker, PdfWorkerRequest, WorkerFailure,
};
use solstone_core_import_sources::{NullDocumentModelClient, NullWireClient};

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
fn a_note_lands_once_and_a_later_export_of_a_trashed_note_stays_out() {
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
    assert!(
        trashed
            .stdout
            .contains("some of these notes are in the trash"),
        "{}",
        trashed.stdout
    );
    assert!(!root.path().join("chronicle/20200104").exists());
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
