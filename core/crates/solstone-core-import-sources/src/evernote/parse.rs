// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A streaming read of an Evernote export.
//!
//! The event reader reports a DOCTYPE and does not open its SYSTEM id. Named
//! entities other than the five predefined XML ones are dropped, not fetched.
//! One note is held at a time. A resource larger than 25 MB is counted and
//! not kept.

use std::io::BufRead;

use base64::Engine;
use chrono::{DateTime, NaiveDateTime, Utc};
use md5::Context;
use quick_xml::Reader;
use quick_xml::events::Event;
use sha2::{Digest, Sha256};

use super::copy::{ENCRYPTED_LINE, MISSING_ATTACHMENT, OVERSIZED_ATTACHMENT, UNMATCHED_ATTACHMENT};

/// Evernote's cap on one resource's decoded body.
pub const MAX_RESOURCE_BYTES: usize = 25 * 1024 * 1024;

/// This route's upload ceiling. The app-wide body limit stays where it is.
pub const MAX_EXPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

const IMAGE_MIMES: &[&str] = &[
    "image/gif",
    "image/jpeg",
    "image/jpg",
    "image/png",
    "image/webp",
];

/// What a file turned out to be.
#[derive(Debug)]
pub enum ReadExport {
    NotExport,
    Export(ParsedExport),
}

#[derive(Debug)]
pub struct ParsedExport {
    pub notes: Vec<ParsedNote>,
    pub ended_early: bool,
}

#[derive(Clone, Debug)]
pub struct ParsedNote {
    pub title: String,
    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    /// The note as text: title is separate. This is what a later export is compared to.
    pub rendered: String,
    /// SHA-256 of the ENML content, lowercase hex. Collision keys use it.
    pub content_hash: String,
    pub encrypted: bool,
    pub images: Vec<HeldBytes>,
    pub pdfs: Vec<HeldBytes>,
    /// Images, documents, and attachments named or left out of the note.
    pub attachment_count: usize,
}

#[derive(Clone, Debug)]
pub struct HeldBytes {
    pub file_name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// The memory time: `updated`, or `created` when `updated` is absent.
pub fn memory_time(note: &ParsedNote) -> Option<DateTime<Utc>> {
    note.updated.or(note.created)
}

/// The identity time: `created`, or `updated` when `created` is absent.
pub fn identity_time(note: &ParsedNote) -> Option<DateTime<Utc>> {
    note.created.or(note.updated)
}

/// Whether the leading bytes name an Evernote export.
#[must_use]
pub fn looks_like_export(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8192)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    text.contains("<!doctype en-export") || text.contains("<en-export")
}

/// Read an export from any buffered reader.
pub fn read_export(reader: impl BufRead) -> ReadExport {
    let mut xml = Reader::from_reader(reader);
    xml.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut saw_export = false;
    let mut stack: Vec<Frame> = Vec::new();
    let mut note = NoteBuild::default();
    let mut notes = Vec::new();
    let mut in_note = false;
    let mut export_closed = false;

    loop {
        let event = match xml.read_event_into(&mut buf) {
            Ok(event) => event,
            Err(_) => {
                if !saw_export && notes.is_empty() {
                    return ReadExport::NotExport;
                }
                return ReadExport::Export(ParsedExport {
                    notes,
                    ended_early: true,
                });
            }
        };
        match event {
            Event::Start(start) => {
                let name = local_name(start.name().as_ref());
                if name == "en-export" {
                    saw_export = true;
                } else if name == "note" && !in_note {
                    in_note = true;
                    note = NoteBuild::default();
                    stack.clear();
                    stack.push(Frame::Ignore);
                } else if in_note {
                    let frame = classify(&name, &stack);
                    if matches!(frame, Frame::Resource) {
                        note.resources.push(ResourceBuild::default());
                    }
                    if matches!(frame, Frame::Task) {
                        note.tasks.push(TaskBuild::default());
                    }
                    stack.push(frame);
                }
            }
            Event::Empty(start) => {
                let name = local_name(start.name().as_ref());
                if name == "en-export" {
                    saw_export = true;
                }
            }
            Event::Text(text) => append_text(&mut note, stack.last(), &text_of(text.as_ref())),
            Event::CData(text) => append_text(&mut note, stack.last(), &text_of_raw(text.as_ref())),
            Event::End(end) => {
                let name = local_name(end.name().as_ref());
                if name == "en-export" {
                    export_closed = true;
                }
                if in_note {
                    stack.pop();
                    if name == "resource" {
                        if let Some(resource) = note.resources.last_mut() {
                            if resource.md5.is_empty() && !resource.b64.is_empty() {
                                decode_resource(resource);
                            }
                            resource.b64.clear();
                        }
                    }
                    if name == "note" {
                        seal_resources(&mut note);
                        notes.push(finish_note(note));
                        note = NoteBuild::default();
                        in_note = false;
                        stack.clear();
                    }
                }
            }
            Event::Eof => break,
            Event::DocType(_)
            | Event::Comment(_)
            | Event::Decl(_)
            | Event::PI(_)
            | Event::GeneralRef(_) => {}
        }
        buf.clear();
    }
    if !saw_export {
        return ReadExport::NotExport;
    }
    ReadExport::Export(ParsedExport {
        notes,
        ended_early: !export_closed,
    })
}

fn finish_note(note: NoteBuild) -> ParsedNote {
    let content_hash = hex_sha256(note.content.as_bytes());
    let enml = render_enml(&note.content);
    let mut rendered = String::new();
    if !enml.body.trim().is_empty() {
        rendered.push_str(enml.body.trim());
    }
    if !note.tags.is_empty() {
        push_line(&mut rendered, &format!("tags: {}", note.tags.join(", ")));
    }
    if !note.author.trim().is_empty() {
        push_line(&mut rendered, &format!("author: {}", note.author.trim()));
    }
    if !note.source_url.trim().is_empty() {
        push_line(&mut rendered, &format!("url: {}", note.source_url.trim()));
    }
    for task in &note.tasks {
        let mark = if task.status.eq_ignore_ascii_case("completed") {
            "- [x] "
        } else {
            "- [ ] "
        };
        let mut line = format!("{mark}{}", task.title.trim());
        if let Some(due) = task.due.as_deref().and_then(display_due) {
            line.push_str(", due ");
            line.push_str(&due);
        }
        push_line(&mut rendered, &line);
    }
    let (images, pdfs, attachment_lines) = resolve_resources(&note.resources, &enml.media);
    let attachment_count = images.len() + pdfs.len() + attachment_lines.len();
    for line in &attachment_lines {
        push_line(&mut rendered, line);
    }
    ParsedNote {
        title: note.title.trim().to_owned(),
        created: parse_stamp(&note.created),
        updated: parse_stamp(&note.updated),
        rendered,
        content_hash,
        encrypted: enml.encrypted,
        images,
        pdfs,
        attachment_count,
    }
}

fn push_line(body: &mut String, line: &str) {
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    if !body.is_empty() {
        body.push('\n');
    }
    body.push_str(line);
}

fn resolve_resources(
    resources: &[ResourceBuild],
    media: &[MediaRef],
) -> (Vec<HeldBytes>, Vec<HeldBytes>, Vec<String>) {
    let mut images = Vec::new();
    let mut pdfs = Vec::new();
    let mut lines = Vec::new();
    let mut seen_image = std::collections::BTreeSet::new();
    let mut seen_pdf = std::collections::BTreeSet::new();
    let mut referenced = std::collections::BTreeSet::new();
    for item in media {
        let hash = item.hash.to_ascii_lowercase();
        if let Some(resource) = resources
            .iter()
            .find(|resource| resource.md5 == hash && !resource.too_large)
        {
            referenced.insert(hash.clone());
            match mime_kind(&resource.mime) {
                MimeKind::Image => {
                    if seen_image.insert(hash) {
                        if let Some(bytes) = &resource.bytes {
                            images.push(HeldBytes {
                                file_name: resource.file_name.clone().unwrap_or_else(|| {
                                    format!("image.{}", image_ext(&resource.mime))
                                }),
                                mime: resource.mime.clone(),
                                bytes: bytes.clone(),
                            });
                        }
                    }
                }
                MimeKind::Pdf => {
                    if seen_pdf.insert(hash) {
                        if let Some(bytes) = &resource.bytes {
                            pdfs.push(HeldBytes {
                                file_name: resource
                                    .file_name
                                    .clone()
                                    .unwrap_or_else(|| "note.pdf".to_owned()),
                                mime: resource.mime.clone(),
                                bytes: bytes.clone(),
                            });
                        }
                    }
                }
                MimeKind::Other => {
                    let name = resource
                        .file_name
                        .clone()
                        .unwrap_or_else(|| resource.mime.clone());
                    lines.push(name);
                }
            }
            continue;
        }
        if resources.iter().any(|resource| resource.too_large) {
            lines.push(OVERSIZED_ATTACHMENT.to_owned());
        } else if resources.iter().any(|resource| !resource.md5.is_empty()) {
            lines.push(UNMATCHED_ATTACHMENT.to_owned());
        } else {
            lines.push(MISSING_ATTACHMENT.to_owned());
        }
    }
    for resource in resources {
        if resource.too_large || referenced.contains(&resource.md5) {
            continue;
        }
        if matches!(mime_kind(&resource.mime), MimeKind::Other) {
            let name = resource
                .file_name
                .clone()
                .unwrap_or_else(|| resource.mime.clone());
            if !name.is_empty() {
                lines.push(name);
            }
        }
    }
    (images, pdfs, lines)
}

fn image_ext(mime: &str) -> &'static str {
    match mime {
        "image/gif" => "gif",
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "jpg",
    }
}

#[derive(Clone, Copy)]
enum MimeKind {
    Image,
    Pdf,
    Other,
}

fn mime_kind(mime: &str) -> MimeKind {
    let mime = mime.trim().to_ascii_lowercase();
    if mime == "application/pdf" {
        MimeKind::Pdf
    } else if IMAGE_MIMES.contains(&mime.as_str()) {
        MimeKind::Image
    } else {
        MimeKind::Other
    }
}

struct Enml {
    body: String,
    media: Vec<MediaRef>,
    encrypted: bool,
}

struct MediaRef {
    hash: String,
}

fn render_enml(content: &str) -> Enml {
    let mut xml = Reader::from_str(content);
    xml.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut out = String::new();
    let mut media = Vec::new();
    let mut encrypted = false;
    let mut skip: u32 = 0;
    let mut pending_check: Option<bool> = None;
    let mut link: Option<String> = None;
    let mut link_text = String::new();
    let mut table: Option<Table> = None;
    loop {
        let event = match xml.read_event_into(&mut buf) {
            Ok(event) => event,
            Err(_) => break,
        };
        match event {
            Event::Start(start) => {
                let name = local_name(start.name().as_ref());
                if skip > 0 {
                    skip += 1;
                    buf.clear();
                    continue;
                }
                if name == "script"
                    || name == "style"
                    || (name == "div" && style_has_en_chs(&start))
                    || name == "en-crypt"
                {
                    if name == "en-crypt" {
                        encrypted = true;
                        push_inline(&mut out, ENCRYPTED_LINE);
                        out.push('\n');
                    }
                    skip = 1;
                } else if name == "a" {
                    link = attr(&start, "href");
                    link_text.clear();
                } else if name == "table" {
                    table = Some(Table::default());
                } else if name == "tr" {
                    if let Some(table) = table.as_mut() {
                        table.rows.push(Vec::new());
                    }
                } else if name == "en-todo" {
                    pending_check =
                        Some(attr(&start, "checked").is_some_and(|value| value == "true"));
                } else if matches!(name.as_str(), "td" | "th") {
                    if let Some(table) = table.as_mut() {
                        if let Some(row) = table.rows.last_mut() {
                            row.push(String::new());
                        }
                    }
                } else if matches!(name.as_str(), "br") {
                    out.push('\n');
                }
            }
            Event::Empty(start) => {
                if skip > 0 {
                    buf.clear();
                    continue;
                }
                let name = local_name(start.name().as_ref());
                if name == "en-todo" {
                    pending_check =
                        Some(attr(&start, "checked").is_some_and(|value| value == "true"));
                } else if name == "en-media" {
                    if let Some(hash) = attr(&start, "hash") {
                        media.push(MediaRef { hash });
                    } else {
                        media.push(MediaRef {
                            hash: String::new(),
                        });
                    }
                } else if name == "en-crypt" {
                    encrypted = true;
                    push_inline(&mut out, ENCRYPTED_LINE);
                    out.push('\n');
                } else if name == "br" {
                    out.push('\n');
                }
            }
            Event::Text(text) => {
                take_enml_text(
                    &mut out,
                    &mut link,
                    &mut link_text,
                    &mut table,
                    &mut pending_check,
                    skip,
                    &text_of(text.as_ref()),
                );
            }
            Event::CData(text) => {
                take_enml_text(
                    &mut out,
                    &mut link,
                    &mut link_text,
                    &mut table,
                    &mut pending_check,
                    skip,
                    &text_of_raw(text.as_ref()),
                );
            }
            Event::End(end) => {
                if skip > 0 {
                    skip -= 1;
                    buf.clear();
                    continue;
                }
                let name = local_name(end.name().as_ref());
                if name == "a" {
                    let href = link.take().unwrap_or_default();
                    let text = link_text.trim().to_owned();
                    link_text.clear();
                    if text.is_empty() {
                        push_inline(&mut out, &href);
                    } else if text == href {
                        push_inline(&mut out, &text);
                    } else {
                        push_inline(&mut out, &format!("{text} ({href})"));
                    }
                } else if matches!(
                    name.as_str(),
                    "div" | "p" | "h1" | "h2" | "h3" | "li" | "tr"
                ) && table.is_none()
                {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                } else if name == "table" {
                    if let Some(table) = table.take() {
                        out.push_str(&table.render());
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Enml {
        body: out,
        media,
        encrypted,
    }
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
}

impl Table {
    fn render(&self) -> String {
        if self.rows.is_empty() {
            return String::new();
        }
        let width = self.rows.iter().map(Vec::len).max().unwrap_or(0);
        if width == 0 {
            return String::new();
        }
        let mut out = String::new();
        for (index, row) in self.rows.iter().enumerate() {
            out.push('|');
            for cell in 0..width {
                out.push(' ');
                out.push_str(row.get(cell).map(String::as_str).unwrap_or("").trim());
                out.push_str(" |");
            }
            out.push('\n');
            if index == 0 {
                out.push('|');
                for _ in 0..width {
                    out.push_str(" --- |");
                }
                out.push('\n');
            }
        }
        out
    }
}

fn take_enml_text(
    out: &mut String,
    link: &mut Option<String>,
    link_text: &mut String,
    table: &mut Option<Table>,
    pending_check: &mut Option<bool>,
    skip: u32,
    text: &str,
) {
    if skip > 0 {
        return;
    }
    if link.is_some() {
        link_text.push_str(text);
    } else if let Some(table) = table.as_mut() {
        if let Some(row) = table.rows.last_mut() {
            if row.is_empty() {
                row.push(String::new());
            }
            if let Some(cell) = row.last_mut() {
                cell.push_str(text);
            }
        }
    } else if let Some(checked) = pending_check.take() {
        let mark = if checked { "- [x] " } else { "- [ ] " };
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(mark);
        out.push_str(text.trim());
        out.push('\n');
    } else {
        push_inline(out, text);
    }
}

fn push_inline(out: &mut String, text: &str) {
    out.push_str(text);
}

fn style_has_en_chs(start: &quick_xml::events::BytesStart<'_>) -> bool {
    attr(start, "style").is_some_and(|style| style.contains("--en-chs"))
}

fn attr(start: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    start
        .attributes()
        .filter_map(|attr| attr.ok())
        .find_map(|attr| {
            let key = local_name(attr.key.as_ref());
            (key == name).then(|| text_of(attr.value.as_ref()))
        })
}

#[derive(Default)]
struct NoteBuild {
    title: String,
    content: String,
    created: String,
    updated: String,
    tags: Vec<String>,
    author: String,
    source_url: String,
    tasks: Vec<TaskBuild>,
    resources: Vec<ResourceBuild>,
}

#[derive(Default)]
struct TaskBuild {
    title: String,
    status: String,
    due: Option<String>,
}

#[derive(Default)]
struct ResourceBuild {
    mime: String,
    file_name: Option<String>,
    bytes: Option<Vec<u8>>,
    md5: String,
    too_large: bool,
    b64: String,
}

enum Frame {
    Title,
    Content,
    Created,
    Updated,
    Tag,
    Author,
    SourceUrl,
    TaskTitle,
    TaskStatus,
    TaskDue,
    Data,
    Mime,
    FileName,
    Resource,
    Task,
    Ignore,
}

fn append_text(note: &mut NoteBuild, frame: Option<&Frame>, text: &str) {
    let Some(frame) = frame else {
        return;
    };
    match frame {
        Frame::Title => note.title.push_str(text),
        Frame::Content => note.content.push_str(text),
        Frame::Created => note.created.push_str(text),
        Frame::Updated => note.updated.push_str(text),
        Frame::Tag => {
            let tag = text.trim();
            if !tag.is_empty() {
                note.tags.push(tag.to_owned());
            }
        }
        Frame::Author => note.author.push_str(text),
        Frame::SourceUrl => note.source_url.push_str(text),
        Frame::TaskTitle => {
            if let Some(task) = note.tasks.last_mut() {
                task.title.push_str(text);
            }
        }
        Frame::TaskStatus => {
            if let Some(task) = note.tasks.last_mut() {
                task.status.push_str(text.trim());
            }
        }
        Frame::TaskDue => {
            if let Some(task) = note.tasks.last_mut() {
                task.due
                    .get_or_insert_with(String::new)
                    .push_str(text.trim());
            }
        }
        Frame::Data => {
            if let Some(resource) = note.resources.last_mut() {
                resource.b64.push_str(text);
            }
        }
        Frame::Mime => {
            if let Some(resource) = note.resources.last_mut() {
                resource.mime.push_str(text.trim());
            }
        }
        Frame::FileName => {
            if let Some(resource) = note.resources.last_mut() {
                let name = resource.file_name.get_or_insert_with(String::new);
                name.push_str(text.trim());
            }
        }
        Frame::Resource | Frame::Task | Frame::Ignore => {}
    }
}

fn retarget_title(stack: &[Frame]) -> bool {
    stack.iter().any(|frame| matches!(frame, Frame::Task))
}

/// Decode a resource's base64 once its element ends.
fn seal_resources(note: &mut NoteBuild) {
    for resource in &mut note.resources {
        if resource.md5.is_empty() && !resource.b64.is_empty() {
            decode_resource(resource);
        }
        resource.b64.clear();
    }
}

fn decode_resource(resource: &mut ResourceBuild) {
    let cleaned: String = resource
        .b64
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let estimated = cleaned.len().saturating_mul(3) / 4;
    if estimated > MAX_RESOURCE_BYTES {
        resource.too_large = true;
        resource.bytes = None;
        resource.md5.clear();
        return;
    }
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(cleaned.as_bytes()) else {
        resource.md5.clear();
        return;
    };
    if bytes.len() > MAX_RESOURCE_BYTES {
        resource.too_large = true;
        resource.bytes = None;
        resource.md5.clear();
        return;
    }
    let mut hasher = Context::new();
    hasher.consume(&bytes);
    resource.md5 = format!("{:x}", hasher.finalize());
    resource.bytes =
        matches!(mime_kind(&resource.mime), MimeKind::Image | MimeKind::Pdf).then_some(bytes);
}

fn local_name(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    text.rsplit(':')
        .next()
        .unwrap_or(&text)
        .to_ascii_lowercase()
}

/// Predefined XML entities only. An unknown reference is removed, not resolved.
fn text_of(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    match quick_xml::escape::unescape(&text) {
        Ok(decoded) => decoded.into_owned(),
        Err(_) => decode_predefined(&text),
    }
}

fn text_of_raw(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

fn decode_predefined(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        let Some(end) = rest.find(';') else {
            break;
        };
        let name = &rest[..end];
        rest = &rest[end + 1..];
        match name {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "apos" => out.push('\''),
            "quot" => out.push('"'),
            _ => {}
        }
    }
    out.push_str(rest);
    out
}

fn parse_stamp(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let naive = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%SZ").ok()?;
    Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
}

fn display_due(raw: &str) -> Option<String> {
    let stamp = parse_stamp(raw)?;
    Some(stamp.format("%Y-%m-%d").to_string())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// The notebook name a file name proposes.
#[must_use]
pub fn proposed_notebook(filename: &str) -> String {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let lower = base.to_ascii_lowercase();
    let stem = if lower.ends_with(".enex") {
        &base[..base.len() - ".enex".len()]
    } else {
        base
    };
    strip_split_suffix(stem.trim()).trim().to_owned()
}

fn strip_split_suffix(stem: &str) -> &str {
    if let Some(open) = stem.rfind(" (")
        && stem.ends_with(')')
    {
        let inner = &stem[open + 2..stem.len() - 1];
        if !inner.is_empty() && inner.bytes().all(|byte| byte.is_ascii_digit()) {
            return stem[..open].trim_end();
        }
    }
    if let Some(index) = stem.rfind(" part ") {
        let rest = &stem[index + " part ".len()..];
        if !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()) {
            return stem[..index].trim_end();
        }
    }
    stem
}

fn classify(name: &str, stack: &[Frame]) -> Frame {
    if name == "title" && retarget_title(stack) {
        return Frame::TaskTitle;
    }
    // `frame_for` does not have the stack. Duplicate the match.
    match name {
        "title" => Frame::Title,
        "content" => Frame::Content,
        "created" => Frame::Created,
        "updated" => Frame::Updated,
        "tag" => Frame::Tag,
        "author" => Frame::Author,
        "source-url" => Frame::SourceUrl,
        "task" => Frame::Task,
        "taskstatus" => Frame::TaskStatus,
        "duedate" => Frame::TaskDue,
        "data" => Frame::Data,
        "mime" => Frame::Mime,
        "file-name" => Frame::FileName,
        "resource" => Frame::Resource,
        _ => Frame::Ignore,
    }
}
