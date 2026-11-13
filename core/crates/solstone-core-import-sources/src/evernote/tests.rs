use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, Utc};

use super::copy;
use super::{
    MAX_EXPORT_BYTES, MAX_RESOURCE_BYTES, Plan, exceeds_export_ceiling, plan, proposed_notebook,
};

const TODAY: NaiveDate = match NaiveDate::from_ymd_opt(2026, 11, 9) {
    Some(day) => day,
    None => panic!("today"),
};

fn journal() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn write_export(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    path
}

fn export(notes: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE en-export SYSTEM \"http://xml.evernote.com/pub/evernote-export4.dtd\">\n\
         <en-export export-date=\"20261109T235638Z\" application=\"Evernote\" version=\"11.41.9\">\n\
         {notes}\n\
         </en-export>\n"
    )
}

fn note(title: &str, created: &str, updated: &str, inner: &str) -> String {
    let created = optional_stamp("created", created);
    let updated = optional_stamp("updated", updated);
    format!(
        "<note><title>{title}</title>{created}{updated}\
         <content><![CDATA[<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <!DOCTYPE en-note SYSTEM \"http://xml.evernote.com/pub/enml2.dtd\">\
         <en-note>{inner}</en-note>]]></content></note>"
    )
}

fn optional_stamp(name: &str, value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        format!("<{name}>{value}</{name}>")
    }
}

fn plan_at(root: &Path, path: &Path, notebook: &str) -> Plan {
    plan(root, path, notebook, &Utc, TODAY).unwrap()
}

fn keep(root: &Path, planned: &Plan) {
    for file in &planned.files {
        let dir = root
            .join("chronicle")
            .join(&file.day)
            .join("import.evernote")
            .join(&file.segment);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("note_transcript.md"), &file.contents).unwrap();
    }
    super::commit(root, &planned.updates).unwrap();
}

fn stored_names(root: &Path, notebook: &str) -> Vec<String> {
    let dir = root
        .join("config")
        .join("evernote-imports")
        .join(super::notebook_id(notebook))
        .join("notes");
    let mut names = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

#[test]
fn golden_sample_is_one_note_dated_at_updated() {
    let root = journal();
    let path = write_export(
        root.path(),
        "sample.enex",
        include_str!("golden-sample.enex"),
    );
    let planned = plan_at(root.path(), &path, "Inbox");
    assert!(planned.refusal.is_none());
    assert_eq!(planned.files.len(), 1);
    assert_eq!(planned.entries, 1);
    let file = &planned.files[0];
    assert_eq!(file.day, "20261109");
    assert_eq!(file.segment, "235552_300");
    assert!(file.contents.contains("River walk"), "{}", file.contents);
    assert!(
        file.contents.contains("EvMarkerAlpha7"),
        "{}",
        file.contents
    );
    assert!(file.contents.contains("EvMarkerBeta3"), "{}", file.contents);
    assert!(file.contents.contains("author: sam"), "{}", file.contents);
    assert!(!file.contents.contains("--en-chs"), "{}", file.contents);
    assert!(!file.contents.contains("eyJpc0VtcHR5"), "{}", file.contents);
    assert!(!file.contents.contains("guid"), "{}", file.contents);
    assert_eq!(planned.updates[0].identity_second, "20261109T235401Z");
    assert_eq!(planned.updates[0].time, "20261109T235552Z");
    assert_eq!(planned.updates[0].notebook, "Inbox");
}

#[test]
fn missing_dates_and_a_future_date_stay_out_of_the_record() {
    let root = journal();
    let body = export(&format!(
        "{}{}{}",
        note("Dated", "", "20200101T120000Z", "<div>from created</div>"),
        note("Blank", "", "", "<div>no date</div>"),
        note(
            "Later",
            "29990101T120000Z",
            "29990101T120000Z",
            "<div>not yet</div>"
        ),
    ));
    let path = write_export(root.path(), "dates.enex", &body);
    let planned = plan_at(root.path(), &path, "Inbox");
    assert!(planned.refusal.is_none());
    assert_eq!(planned.files.len(), 1);
    assert_eq!(planned.files[0].day, "20200101");
    assert!(planned.files[0].contents.contains("from created"));
    assert_eq!(planned.updates.len(), 1);
    assert_eq!(planned.updates[0].original_title, "Dated");
    assert!(
        planned.summary.contains(&copy::no_date(1)),
        "{}",
        planned.summary
    );
    assert!(
        planned.summary.contains(&copy::dated_after_today(1)),
        "{}",
        planned.summary
    );
    assert!(!planned.summary.contains("not yet"));
}

#[test]
fn the_same_note_again_writes_nothing_and_an_edit_keeps_the_earlier_memory() {
    let root = journal();
    let first = write_export(
        root.path(),
        "first.enex",
        &export(&note(
            "River",
            "20200101T120000Z",
            "20200101T120000Z",
            "<div>met by the river</div>",
        )),
    );
    let planned = plan_at(root.path(), &first, "Inbox");
    let original = planned.files[0].contents.clone();
    let day = planned.files[0].day.clone();
    let segment = planned.files[0].segment.clone();
    keep(root.path(), &planned);

    let again = write_export(
        root.path(),
        "again.enex",
        &export(&note(
            "River",
            "20200101T120000Z",
            "20200102T120000Z",
            "<div>met by the river</div>",
        )),
    );
    let same = plan_at(root.path(), &again, "Inbox");
    assert!(same.files.is_empty(), "{}", same.summary);
    assert!(
        same.summary.contains(copy::ALREADY_IN_JOURNAL),
        "{}",
        same.summary
    );
    assert_eq!(same.updates.len(), 1);
    assert_eq!(same.updates[0].time, "20200102T120000Z");
    keep(root.path(), &same);

    let edited = write_export(
        root.path(),
        "edited.enex",
        &export(&note(
            "River",
            "20200101T120000Z",
            "20200103T150000Z",
            "<div>met by the river and stayed</div>",
        )),
    );
    let change = plan_at(root.path(), &edited, "Inbox");
    assert_eq!(change.files.len(), 1);
    assert_eq!(change.files[0].day, "20200103");
    assert_ne!(change.files[0].segment, segment);
    assert!(
        change.files[0].contents.contains("edited"),
        "{}",
        change.files[0].contents
    );
    assert!(
        change.files[0].contents.contains("stayed"),
        "{}",
        change.files[0].contents
    );
    let earlier = fs::read_to_string(
        root.path()
            .join("chronicle")
            .join(&day)
            .join("import.evernote")
            .join(&segment)
            .join("note_transcript.md"),
    )
    .unwrap();
    assert_eq!(earlier, original);
    assert!(
        change.updates[0].rendered.contains("stayed"),
        "{}",
        change.updates[0].rendered
    );
    assert_eq!(change.updates[0].day, "20200103");
}

#[test]
fn a_note_missing_from_the_next_file_stays() {
    let root = journal();
    let both = write_export(
        root.path(),
        "both.enex",
        &export(&format!(
            "{}{}",
            note(
                "One",
                "20200101T120000Z",
                "20200101T120000Z",
                "<div>one</div>"
            ),
            note(
                "Two",
                "20200201T120000Z",
                "20200201T120000Z",
                "<div>two</div>"
            ),
        )),
    );
    let planned = plan_at(root.path(), &both, "Inbox");
    assert_eq!(planned.updates.len(), 2);
    keep(root.path(), &planned);
    let names = stored_names(root.path(), "Inbox");
    let only = write_export(
        root.path(),
        "only.enex",
        &export(&note(
            "One",
            "20200101T120000Z",
            "20200101T120000Z",
            "<div>one</div>",
        )),
    );
    let next = plan_at(root.path(), &only, "Inbox");
    assert!(next.files.is_empty());
    assert_eq!(stored_names(root.path(), "Inbox"), names);
}

#[test]
fn the_same_second_keeps_two_notes_and_a_new_title_stays_on_the_key() {
    let root = journal();
    let first = write_export(
        root.path(),
        "pair.enex",
        &export(&format!(
            "{}{}",
            note(
                "Alpha",
                "20200101T120000Z",
                "20200101T120000Z",
                "<div>alpha body</div>"
            ),
            note(
                "Beta",
                "20200101T120000Z",
                "20200101T120000Z",
                "<div>beta body</div>"
            ),
        )),
    );
    let planned = plan_at(root.path(), &first, "Work");
    assert_eq!(planned.updates.len(), 2);
    assert_eq!(
        planned.files.len(),
        1,
        "notes in the same minute share a transcript"
    );
    assert!(planned.files[0].contents.contains("## Alpha"));
    assert!(planned.files[0].contents.contains("## Beta"));
    keep(root.path(), &planned);
    let names = stored_names(root.path(), "Work");
    assert_eq!(names.len(), 2);

    let renamed = write_export(
        root.path(),
        "renamed.enex",
        &export(&note(
            "Beta renamed",
            "20200101T120000Z",
            "20200101T120100Z",
            "<div>beta body</div>",
        )),
    );
    let next = plan_at(root.path(), &renamed, "Work");
    assert!(next.files.is_empty(), "{}", next.summary);
    assert_eq!(next.updates.len(), 1);
    assert_eq!(next.updates[0].last_title, "Beta renamed");
    assert_eq!(next.updates[0].original_title, "Beta");
    assert_eq!(next.updates[0].disambiguator.as_deref(), Some("Beta"));
    keep(root.path(), &next);
    assert_eq!(stored_names(root.path(), "Work"), names);

    let only_beta = write_export(
        root.path(),
        "beta.enex",
        &export(&note(
            "Beta renamed",
            "20200101T120000Z",
            "20200101T120100Z",
            "<div>beta body</div>",
        )),
    );
    let partial = plan_at(root.path(), &only_beta, "Work");
    assert!(partial.files.is_empty());
    assert!(partial.updates.is_empty());
    assert_eq!(stored_names(root.path(), "Work"), names);
}

#[test]
fn split_filenames_propose_one_name_and_the_typed_name_is_stored() {
    assert_eq!(proposed_notebook("Work.enex"), "Work");
    assert_eq!(proposed_notebook("Work (2).enex"), "Work");
    assert_eq!(proposed_notebook("Work part 3.enex"), "Work");
    assert_eq!(proposed_notebook("/tmp/Work (2).ENEX"), "Work");
    let root = journal();
    let path = write_export(
        root.path(),
        "Work (2).enex",
        &export(&note(
            "Page",
            "20200101T120000Z",
            "20200101T120000Z",
            "<div>page</div>",
        )),
    );
    let preview = super::preview_path(&path).unwrap();
    assert_eq!(preview.notebook, "Work");
    assert!(preview.ready);
    let planned = plan_at(root.path(), &path, "Field notes");
    assert_eq!(planned.notebook, "Field notes");
    assert_eq!(planned.updates[0].notebook, "Field notes");
    let stored_dir = planned.updates[0].notebook.clone();
    keep(root.path(), &planned);
    let parent = root
        .path()
        .join("config")
        .join("evernote-imports")
        .join(super::notebook_id("Field notes"));
    assert!(parent.is_dir());
    assert!(!parent.join(&stored_dir).exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = parent
            .join("notes")
            .join(stored_names(root.path(), "Field notes")[0].clone());
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn a_deleted_note_is_new_beside_a_live_note_in_a_later_export() {
    let root = journal();
    let path = write_export(
        root.path(),
        "note.enex",
        &export(&note(
            "River",
            "20200101T120000Z",
            "20200101T120000Z",
            "<div>met by the river</div>",
        )),
    );
    let planned = plan_at(root.path(), &path, "Inbox");
    keep(root.path(), &planned);
    let dir = root
        .path()
        .join("chronicle")
        .join(&planned.files[0].day)
        .join("import.evernote")
        .join(&planned.files[0].segment);
    fs::remove_dir_all(&dir).unwrap();
    let again = write_export(
        root.path(),
        "again.enex",
        &export(&format!(
            "{}{}",
            note(
                "River",
                "20200101T120000Z",
                "20200104T120000Z",
                "<div>a later sentence</div>"
            ),
            note(
                "Fresh",
                "20200301T120000Z",
                "20200301T120000Z",
                "<div>a new note</div>"
            ),
        )),
    );
    let next = plan_at(root.path(), &again, "Inbox");
    assert_eq!(next.entries, 2);
    assert!(
        next.files
            .iter()
            .any(|file| file.contents.contains("a later sentence"))
    );
    assert!(
        next.files
            .iter()
            .any(|file| file.contents.contains("a new note"))
    );
    assert!(
        next.updates
            .iter()
            .any(|note| note.original_title == "River")
    );
}

#[test]
fn attachments_follow_the_note_or_leave_a_line() {
    let png = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137,
    ];
    let pdf = b"%PDF-1.4 note";
    let wav = b"RIFF....WAVE";
    let png_hash = md5_hex(&png);
    let pdf_hash = md5_hex(pdf);
    let wav_hash = md5_hex(wav);
    let png_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png);
    let pdf_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, pdf);
    let wav_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, wav);
    let inner = format!(
        "<div>with files</div><en-media hash=\"{png_hash}\"/><en-media hash=\"{pdf_hash}\"/>\
         <en-media hash=\"{wav_hash}\"/><en-media hash=\"wrong\"/>"
    );
    let resources = format!(
        "<resource><data encoding=\"base64\">{png_b64}</data><mime>image/png</mime><file-name>dot.png</file-name></resource>\
         <resource><data encoding=\"base64\">{pdf_b64}</data><mime>application/pdf</mime><file-name>page.pdf</file-name></resource>\
         <resource><data encoding=\"base64\">{wav_b64}</data><mime>audio/wav</mime><file-name>clip.wav</file-name></resource>\
         <resource><data encoding=\"base64\">QQ==</data><mime>application/octet-stream</mime><file-name>other.bin</file-name></resource>"
    );
    let body = export(&format!(
        "<note><title>Files</title><created>20200101T120000Z</created><updated>20200101T120500Z</updated>\
         <content><![CDATA[<?xml version=\"1.0\"?><en-note>{inner}</en-note>]]></content>{resources}</note>\
         <note><title>Gone</title><created>20200101T120000Z</created><updated>20200101T120500Z</updated>\
         <content><![CDATA[<?xml version=\"1.0\"?><en-note><div>elsewhere</div><en-media hash=\"missing\"/></en-note>]]></content></note>"
    ));
    let root = journal();
    let path = write_export(root.path(), "files.enex", &body);
    let planned = plan_at(root.path(), &path, "Inbox");
    assert_eq!(planned.images.len(), 1);
    assert_eq!(planned.images[0].file_name, "dot.png");
    assert_eq!(planned.images[0].bytes, png.to_vec());
    assert_eq!(planned.images[0].owner_timestamp, "20200101_120500");
    assert_eq!(planned.pdfs.len(), 1);
    assert_eq!(planned.pdfs[0].bytes, pdf.to_vec());
    assert_eq!(planned.pdfs[0].owner_timestamp, "20200101_120500");
    let text = &planned.files[0].contents;
    assert!(text.contains("clip.wav"), "{text}");
    assert!(text.contains(copy::UNMATCHED_ATTACHMENT), "{text}");
    assert!(text.contains(copy::MISSING_ATTACHMENT), "{text}");
    assert!(!text.contains("dot.png"), "{text}");
    assert!(!text.contains("page.pdf"), "{text}");
}

#[test]
fn an_oversized_resource_leaves_the_line_and_not_the_bytes() {
    let root = journal();
    let path = root.path().join("big.enex");
    let mut file = fs::File::create(&path).unwrap();
    write!(
        file,
        "<?xml version=\"1.0\"?><en-export><note><title>Big</title>\
         <created>20200101T120000Z</created><updated>20200101T120000Z</updated>\
         <content><![CDATA[<?xml version=\"1.0\"?><en-note><div>kept</div><en-media hash=\"none\"/></en-note>]]></content>\
         <resource><data encoding=\"base64\">"
    )
    .unwrap();
    let chunk = vec![b'A'; 1024 * 1024];
    let need = MAX_RESOURCE_BYTES / 3 * 4 + 4;
    let mut left = need;
    while left > 0 {
        let take = left.min(chunk.len());
        file.write_all(&chunk[..take]).unwrap();
        left -= take;
    }
    write!(
        file,
        "</data><mime>application/octet-stream</mime><file-name>big.bin</file-name></resource></note></en-export>"
    )
    .unwrap();
    drop(file);
    let planned = plan_at(root.path(), &path, "Inbox");
    assert_eq!(planned.files.len(), 1);
    assert!(planned.files[0].contents.contains("kept"));
    assert!(
        planned.files[0]
            .contents
            .contains(copy::OVERSIZED_ATTACHMENT)
    );
    assert!(planned.images.is_empty());
    assert!(planned.pdfs.is_empty());
}

#[test]
fn encrypted_text_todos_tasks_tables_and_web_clips() {
    let inner = "\
        <div style=\"display:none;--en-chs:&quot;hidden-marker&quot;\">secret-hidden</div>\
        <script>secret-script</script><style>secret-style</style>\
        <div>Visible clip</div>\
        <en-crypt>CIPHERTEXT-SECRET</en-crypt>\
        <div><en-todo/>buy milk</div>\
        <div><en-todo checked=\"true\"/>done</div>\
        <table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>\
        <div><a href=\"https://example.com/walk\">the river</a></div>";
    let body = export(&format!(
        "<note><title>Rich</title><created>20200101T120000Z</created><updated>20200101T120000Z</updated>\
         <tag>walk</tag>\
         <note-attributes><author>sam</author><source-url>https://example.com/walk</source-url></note-attributes>\
         <content><![CDATA[<?xml version=\"1.0\"?><en-note>{inner}</en-note>]]></content>\
         <task><title>Send the letter</title><taskStatus>open</taskStatus><dueDate>20261201T000000Z</dueDate></task>\
         </note>"
    ));
    let root = journal();
    let path = write_export(root.path(), "rich.enex", &body);
    let planned = plan_at(root.path(), &path, "Inbox");
    let text = &planned.files[0].contents;
    assert!(text.contains("Visible clip"), "{text}");
    assert!(text.contains(copy::ENCRYPTED_LINE), "{text}");
    assert!(!text.contains("CIPHERTEXT-SECRET"), "{text}");
    assert!(!text.contains("secret-hidden"), "{text}");
    assert!(!text.contains("secret-script"), "{text}");
    assert!(!text.contains("hidden-marker"), "{text}");
    assert!(text.contains("- [ ] buy milk"), "{text}");
    assert!(text.contains("- [x] done"), "{text}");
    assert!(
        text.contains("- [ ] Send the letter, due 2026-12-01"),
        "{text}"
    );
    assert!(text.contains("| A | B |"), "{text}");
    assert!(
        text.contains("the river (https://example.com/walk)"),
        "{text}"
    );
    assert!(text.contains("tags: walk"), "{text}");
    assert!(text.contains("author: sam"), "{text}");
    assert!(text.contains("url: https://example.com/walk"), "{text}");
}

#[test]
fn export_1_and_3_still_import() {
    for (name, system) in [
        (
            "one.enex",
            "http://xml.evernote.com/pub/evernote-export.dtd",
        ),
        (
            "three.enex",
            "http://xml.evernote.com/pub/evernote-export3.dtd",
        ),
    ] {
        let body = format!(
            "<?xml version=\"1.0\"?><!DOCTYPE en-export SYSTEM \"{system}\"><en-export>{}</en-export>",
            note(
                "Old",
                "20200101T120000Z",
                "20200101T120000Z",
                "<div>from an older export</div>"
            )
        );
        let root = journal();
        let path = write_export(root.path(), name, &body);
        let planned = plan_at(root.path(), &path, "Inbox");
        assert!(planned.refusal.is_none(), "{name}");
        assert!(planned.files[0].contents.contains("from an older export"));
    }
}

#[test]
fn an_external_entity_does_not_enter_the_note() {
    let root = journal();
    let secret = root.path().join("secret.txt");
    fs::write(&secret, "EvXxeMarker9182").unwrap();
    let path = root.path().join("xxe.enex");
    fs::write(
        &path,
        format!(
            "<?xml version=\"1.0\"?>\n\
             <!DOCTYPE en-export [<!ENTITY boom SYSTEM \"file://{}\">]>\n\
             <en-export><note><title>Safe &boom;</title>\
             <created>20200101T120000Z</created><updated>20200101T120000Z</updated>\
             <content><![CDATA[<?xml version=\"1.0\"?><en-note><div>plain</div></en-note>]]></content>\
             </note></en-export>\n",
            secret.display()
        ),
    )
    .unwrap();
    let planned = plan_at(root.path(), &path, "Inbox");
    let rendered = planned
        .files
        .iter()
        .map(|file| file.contents.clone())
        .collect::<String>();
    assert!(!rendered.contains("EvXxeMarker9182"));
    assert!(!planned.summary.contains("EvXxeMarker9182"));
    if let Some(file) = planned.files.first() {
        assert!(file.contents.contains("plain"), "{}", file.contents);
    }
}

#[test]
fn a_truncated_export_keeps_the_notes_already_ended() {
    let root = journal();
    let body = format!(
        "<?xml version=\"1.0\"?><en-export>{}<note><title>Broken",
        note(
            "Kept",
            "20200101T120000Z",
            "20200101T120000Z",
            "<div>kept</div>"
        )
    );
    let path = write_export(root.path(), "cut.enex", &body);
    let planned = plan_at(root.path(), &path, "Inbox");
    assert!(planned.refusal.is_none());
    assert_eq!(planned.files.len(), 1);
    assert!(planned.files[0].contents.contains("kept"));
    assert!(
        planned.summary.contains("ended early"),
        "{}",
        planned.summary
    );
}

#[test]
fn a_file_that_is_not_an_export_and_an_empty_export_write_nothing() {
    let root = journal();
    let not_export = write_export(root.path(), "notes.txt", "hello");
    let refused = plan_at(root.path(), &not_export, "Inbox");
    assert_eq!(refused.refusal.as_deref(), Some(copy::NOT_AN_EXPORT));
    assert!(refused.files.is_empty());
    let empty = write_export(
        root.path(),
        "empty.enex",
        "<?xml version=\"1.0\"?><en-export></en-export>",
    );
    let none = plan_at(root.path(), &empty, "Inbox");
    assert_eq!(none.refusal.as_deref(), Some(copy::EMPTY_EXPORT));
    assert!(none.files.is_empty());
}

#[test]
fn the_export_ceiling_is_two_gigabytes() {
    assert!(!exceeds_export_ceiling(64 * 1024 * 1024 + 1));
    assert!(!exceeds_export_ceiling(MAX_EXPORT_BYTES));
    assert!(exceeds_export_ceiling(MAX_EXPORT_BYTES + 1));
}

#[test]
fn notes_six_minutes_apart_open_two_transcripts() {
    let root = journal();
    let path = write_export(
        root.path(),
        "span.enex",
        &export(&format!(
            "{}{}",
            note(
                "Early",
                "20200101T120000Z",
                "20200101T120000Z",
                "<div>early</div>"
            ),
            note(
                "Later",
                "20200101T120600Z",
                "20200101T120600Z",
                "<div>later</div>"
            ),
        )),
    );
    let planned = plan_at(root.path(), &path, "Inbox");
    assert_eq!(planned.files.len(), 2);
    assert_eq!(planned.files[0].segment, "120000_300");
    assert_eq!(planned.files[1].segment, "120600_300");
}
