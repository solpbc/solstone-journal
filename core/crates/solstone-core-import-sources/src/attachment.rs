// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Prepare a file inside a source-owned segment without installing an original,
//! creating segments, recording places/faces, or inventing a capture time.

use std::collections::BTreeSet;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use solstone_core_format::attachment::{FileType, PrepareFailure as Failure, file_type};
pub use solstone_core_format::attachment::{
    PrepareFailure, PreparedAttachment, SharedAttachmentReader,
};
use solstone_core_import::triage::{self, Kind};

use crate::document::{
    self, InstalledLocalPdfOcr, PdfCommand, PdfPageOcr, PdfRenderOptions, PdfWorker, WorkerFailure,
};
use crate::image::{self as images, WireClient};

const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = solstone_core_format::folder::MAX_NOTE_BYTES;
const MAX_SOURCE_PIXELS: u64 = 25_000_000;
const MAX_OCR_PAGES: usize = 8;
const OCR_DPI: i32 = 120;
const MAX_OCR_PIXELS: f64 = 8_000_000.0;
const MAX_RASTER_BYTES: usize = 32 * 1024 * 1024;
const MAX_PREVIEW_EDGE: u32 = 320;
const DESCRIPTION_EDGE: u32 = 512;
const OCR_EDGE: u32 = 1920;

type LocalOcr<'a> = dyn FnMut(&[u8]) -> Result<String, Failure> + 'a;

const PDF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Capture the owning journal and the bounded PDF worker's actual availability.
pub fn installed_reader(journal: std::path::PathBuf) -> SharedAttachmentReader {
    SharedAttachmentReader {
        read: std::sync::Arc::new(move |name, mime, bytes| {
            prepare_installed(&journal, name, mime, bytes)
        }),
        pdf_unavailable: document::installed_pdf_worker(PDF_TIMEOUT).is_err(),
    }
}

/// Use the same installed readers from the user-initiated and scheduled syncs.
/// A journal built without the local OCR runtime retains native PDF text.
pub fn prepare_installed(
    journal: &Path,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<PreparedAttachment, Failure> {
    let mut local = InstalledLocalPdfOcr::default();
    let mut ocr = |png: &[u8]| local.recognize_png(png).map_err(|_| Failure::Unavailable);
    prepare(
        name,
        mime,
        bytes,
        &|| document::installed_pdf_worker(PDF_TIMEOUT).map_err(|_| Failure::Unavailable),
        &images::JournalWireClient(journal),
        &mut ocr,
    )
}

/// Read one source occurrence. The supplied OCR callback is local. Only an
/// admitted photograph reaches the configured journal image description client.
pub fn prepare(
    name: &str,
    mime: &str,
    bytes: &[u8],
    pdf: &dyn Fn() -> Result<Box<dyn PdfWorker>, Failure>,
    wire: &dyn WireClient,
    ocr: &mut LocalOcr<'_>,
) -> Result<PreparedAttachment, Failure> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Failure::OverBudget);
    }
    if bytes.is_empty() {
        return Err(Failure::Unreadable);
    }
    if !solstone_core_import::work_continues() {
        return Err(Failure::Unavailable);
    }
    match file_type(name, mime).ok_or(Failure::Unsupported)? {
        FileType::Pdf => {
            if !bytes.starts_with(b"%PDF-") {
                return Err(Failure::Unreadable);
            }
            prepare_pdf(bytes, pdf()?.as_ref(), ocr)
        }
        FileType::Document => prepare_document(name, mime, bytes),
        FileType::Text => {
            if bytes.len() > MAX_TEXT_BYTES {
                return Err(Failure::OverBudget);
            }
            let text = triage::decode_text(bytes).ok_or(Failure::Unreadable)?;
            if text.trim().is_empty() || text.chars().any(|c| c.is_control() && !c.is_whitespace())
            {
                return Err(Failure::Unreadable);
            }
            let verdict = triage::classify(bytes, name, Path::new(""), None, None);
            if verdict.reasons.contains(&"email_headers")
                || verdict.reasons.contains(&"document_format")
            {
                return Err(Failure::Unsupported);
            }
            let kind = match verdict.kind {
                Kind::Notes => "notes",
                Kind::Documents => "documents",
                _ => return Err(Failure::Unsupported),
            };
            Ok(PreparedAttachment {
                kind,
                text: text.into_owned(),
                preview_jpeg: None,
                partial: false,
                thoughts: 0,
            })
        }
        kind => {
            let format = match kind {
                FileType::Jpeg => ImageFormat::Jpeg,
                FileType::Png => ImageFormat::Png,
                FileType::Gif => ImageFormat::Gif,
                FileType::WebP => ImageFormat::WebP,
                _ => unreachable!("PDF and text handled above"),
            };
            prepare_image(name, bytes, format, wire, ocr)
        }
    }
}

fn prepare_document(name: &str, mime: &str, bytes: &[u8]) -> Result<PreparedAttachment, Failure> {
    use solstone_core_doc_extract::{DocumentFormat, ExtractError, ExtractLimits, ExtractOutcome};

    let mut limits = ExtractLimits::for_current_platform();
    limits.max_input_bytes = limits.max_input_bytes.min(MAX_FILE_BYTES as u64);

    let hint = if DocumentFormat::from_filename(name).is_some() {
        Some(name)
    } else {
        synthetic_hint_for_mime(mime)
    };

    let outcome = solstone_core_doc_extract::extract_bytes(bytes, hint, &limits).map_err(
        |err| match err {
            ExtractError::LimitExceeded(_) => Failure::OverBudget,
            ExtractError::Invalid(_) => Failure::Unreadable,
            ExtractError::Internal(_) => Failure::Unavailable,
        },
    )?;

    if !solstone_core_import::work_continues() {
        return Err(Failure::Unavailable);
    }

    match outcome {
        ExtractOutcome::Extracted(doc) => {
            let raw = if doc.markdown.trim().is_empty() {
                doc.plain_text
            } else {
                doc.markdown
            };
            let (text, byte_truncated) = truncate_utf8_bytes(&raw, MAX_TEXT_BYTES);
            if text.trim().is_empty() {
                return Err(Failure::Unreadable);
            }
            Ok(PreparedAttachment {
                kind: "documents",
                text,
                preview_jpeg: None,
                partial: doc.truncated || byte_truncated,
                thoughts: 0,
            })
        }
        ExtractOutcome::Encrypted => Err(Failure::Encrypted),
        ExtractOutcome::Empty { .. } => Err(Failure::Unreadable),
        ExtractOutcome::Unsupported => Err(Failure::Unsupported),
    }
}

fn synthetic_hint_for_mime(mime: &str) -> Option<&'static str> {
    let base = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "application/msword" => Some("attachment.doc"),
        "application/vnd.ms-excel" => Some("attachment.xls"),
        "application/vnd.ms-powerpoint" => Some("attachment.ppt"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some("attachment.docx")
        }
        "application/vnd.ms-word.document.macroenabled.12" => Some("attachment.docm"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("attachment.xlsx")
        }
        "application/vnd.ms-excel.sheet.macroenabled.12" => Some("attachment.xlsm"),
        "application/vnd.ms-excel.sheet.binary.macroenabled.12" => Some("attachment.xlsb"),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            Some("attachment.pptx")
        }
        "application/vnd.ms-powerpoint.presentation.macroenabled.12" => Some("attachment.pptm"),
        "application/vnd.openxmlformats-officedocument.presentationml.slideshow" => {
            Some("attachment.ppsx")
        }
        "application/epub+zip" => Some("attachment.epub"),
        "application/vnd.oasis.opendocument.text" => Some("attachment.odt"),
        "application/vnd.oasis.opendocument.spreadsheet" => Some("attachment.ods"),
        "application/vnd.oasis.opendocument.presentation" => Some("attachment.odp"),
        "application/rtf" | "text/rtf" | "application/x-rtf" => Some("attachment.rtf"),
        "message/rfc822" => Some("attachment.eml"),
        "text/csv" | "application/csv" => Some("attachment.csv"),
        "text/tab-separated-values" => Some("attachment.tsv"),
        "text/html" => Some("attachment.html"),
        _ => None,
    }
}

fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.trim_end().to_owned(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].trim_end().to_owned(), true)
}

fn prepare_image(
    name: &str,
    bytes: &[u8],
    expected: ImageFormat,
    wire: &dyn WireClient,
    ocr: &mut LocalOcr<'_>,
) -> Result<PreparedAttachment, Failure> {
    let format = image::guess_format(bytes).map_err(|_| Failure::Unreadable)?;
    if format != expected {
        return Err(Failure::Unreadable);
    }
    let rejection = |_| Failure::Unreadable;
    let reader = || ImageReader::with_format(Cursor::new(bytes), format);
    let (w, h) = reader().into_dimensions().map_err(rejection)?;
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_SOURCE_PIXELS {
        return Err(Failure::OverBudget);
    }
    let animated = match format {
        ImageFormat::Png => image::codecs::png::PngDecoder::new(Cursor::new(bytes))
            .and_then(|d| d.is_apng())
            .map_err(rejection)?,
        ImageFormat::WebP => image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
            .map_err(rejection)?
            .has_animation(),
        ImageFormat::Gif => {
            let gif = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(rejection)?;
            let mut frames = gif.into_frames();
            frames
                .next()
                .ok_or(Failure::Unreadable)?
                .map_err(rejection)?;
            frames.next().transpose().map_err(rejection)?.is_some()
        }
        _ => false,
    };
    if animated {
        return Err(Failure::Unsupported);
    }
    let mut decoder = reader().into_decoder().map_err(rejection)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut pixels = DynamicImage::from_decoder(decoder).map_err(rejection)?;
    pixels.apply_orientation(orientation);
    let screen = solstone_core_format::screenshots::is_screen_still_file_name(name)
        || matches!(
            triage::classify(bytes, name, Path::new(""), None, None).kind,
            Kind::Screen | Kind::Screenshots
        );
    let preview_jpeg = Some(pixel_copy(&pixels, MAX_PREVIEW_EDGE, ImageFormat::Jpeg)?);
    let (text, thoughts) = if screen {
        (ocr(&pixel_copy(&pixels, OCR_EDGE, ImageFormat::Png)?)?, 0)
    } else {
        let jpeg = pixel_copy(&pixels, DESCRIPTION_EDGE, ImageFormat::Jpeg)?;
        let (outcome, completion) = images::read_vision_pixels(&jpeg, wire);
        let reading = outcome.reading().ok_or(if completion {
            Failure::DescriptionUnavailable
        } else {
            Failure::Unavailable
        })?;
        let mut text = reading.description.clone();
        if let Some(visible) = &reading.visible_text {
            text.push_str("\n\n");
            text.push_str(visible);
        }
        (text, 1)
    };
    Ok(PreparedAttachment {
        kind: if screen { "screenshots" } else { "photos" },
        text,
        preview_jpeg,
        partial: false,
        thoughts,
    })
}

fn pixel_copy(image: &DynamicImage, max: u32, format: ImageFormat) -> Result<Vec<u8>, Failure> {
    let smaller = image
        .thumbnail(image.width().min(max), image.height().min(max))
        .into_rgb8();
    let mut buffer = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(smaller)
        .write_to(&mut buffer, format)
        .map_err(|_| Failure::Unreadable)?;
    Ok(buffer.into_inner())
}

fn prepare_pdf(
    bytes: &[u8],
    worker: &dyn PdfWorker,
    ocr: &mut LocalOcr<'_>,
) -> Result<PreparedAttachment, Failure> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("solstone-attachment-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let dir = builder.tempdir().map_err(|_| Failure::Unavailable)?;
    #[cfg(windows)]
    let dir = std::sync::Arc::new(dir);
    let file = dir.path().join("input.pdf");
    // Only the private staging directory supplies paths to the worker.
    let mut staged =
        tempfile::NamedTempFile::new_in(dir.path()).map_err(|_| Failure::Unavailable)?;
    staged.write_all(bytes).map_err(|_| Failure::Unavailable)?;
    staged.persist(&file).map_err(|_| Failure::Unavailable)?;
    let extract = |render| {
        let request = document::worker_request(PdfCommand::Extract, &file, None, render);
        // A killed Windows Job can still be retiring after its timeout.
        #[cfg(windows)]
        let request = document::PdfWorkerRequest {
            resources: document::PdfWorkerResources {
                _directory: Some(dir.clone()),
            },
            ..request
        };
        worker.execute(&request)
    };
    let native = extract(None).map_err(|err| match err {
        WorkerFailure::Process {
            exit_code: Some(3), ..
        } => Failure::Encrypted,
        WorkerFailure::Process {
            exit_code: Some(4), ..
        } => Failure::Unreadable,
        _ => Failure::Unavailable,
    })?;
    if native.encrypted {
        return Err(Failure::Encrypted);
    }
    if native.page_count == 0 {
        return Err(Failure::Unreadable);
    }
    let sparse: BTreeSet<_> = native
        .pages
        .iter()
        .filter(|p| {
            p.error.is_none()
                && p.text.as_deref().unwrap_or_default().trim().is_empty()
                && p.width_pt > 0.0
                && p.height_pt > 0.0
                && p.width_pt * p.height_pt * (f64::from(OCR_DPI) / 72.0).powi(2) <= MAX_OCR_PIXELS
        })
        .take(MAX_OCR_PAGES)
        .map(|p| p.index)
        .collect();
    let rendered = !sparse.is_empty()
        && extract(Some(PdfRenderOptions {
            pages: sparse.clone(),
            render_dir: dir.path().to_owned(),
            dpi: OCR_DPI,
        }))
        .is_ok();
    let mut partial = native.pages.len() != native.page_count;
    let mut contents = String::new();
    for page in &native.pages {
        if !solstone_core_import::work_continues() {
            return Err(Failure::Unavailable);
        }
        let text = if let Some(text) = page.text.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(text.trim_end().to_string())
        } else if rendered && sparse.contains(&page.index) {
            let mut read = || -> Option<String> {
                let mut png = Vec::new();
                let file =
                    std::fs::File::open(dir.path().join(format!("page-{:04}.png", page.index)))
                        .ok()?;
                file.take(MAX_RASTER_BYTES as u64 + 1)
                    .read_to_end(&mut png)
                    .ok()?;
                if png.len() > MAX_RASTER_BYTES {
                    return None;
                }
                ocr(&png).ok()
            };
            read()
        } else {
            None
        };
        if page.error.is_some() || text.is_none() {
            partial = true;
        }
        if let Some(text) = text.filter(|s| !s.trim().is_empty()) {
            let page_text = format!("Page {}\n{}\n\n", page.index, text);
            if contents.len() + page_text.len() > MAX_TEXT_BYTES {
                partial = true;
                break;
            }
            contents.push_str(&page_text);
        }
    }
    if contents.is_empty() {
        return Err(if partial {
            Failure::Unavailable
        } else {
            Failure::Unreadable
        });
    }
    Ok(PreparedAttachment {
        kind: "documents",
        text: contents.trim_end().to_string(),
        preview_jpeg: None,
        partial,
        thoughts: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NullWireClient;
    use crate::document::{PdfPage, PdfPayload, PdfWorkerRequest};

    fn unused_pdf() -> Result<Box<dyn PdfWorker>, Failure> {
        panic!("unexpected PDF worker")
    }
    fn unused_ocr(_: &[u8]) -> Result<String, Failure> {
        panic!("unexpected OCR")
    }

    #[test]
    fn only_recognized_notes_and_documents_are_admitted() {
        let read = |name: &str, mime: &str, bytes: &[u8]| {
            prepare(
                name,
                mime,
                bytes,
                &unused_pdf,
                &NullWireClient,
                &mut unused_ocr,
            )
        };
        let file = read(
            "weekly.md",
            "application/octet-stream",
            b"Remember to bring the drawings.",
        )
        .unwrap();
        assert_eq!(file.kind, "notes");
        assert_eq!(file.thoughts, 0);
        assert_eq!(file.text, "Remember to bring the drawings.");
        assert_eq!(
            read(
                "record.txt",
                "text/plain",
                b"WEBVTT\n\n00:00:00.000 --> 00:00:10.000\n<v Kay>Signed.\n"
            )
            .unwrap_err(),
            Failure::Unsupported
        );
        assert_eq!(
            read("orders.csv", "text/plain", b"total,10").unwrap_err(),
            Failure::Unsupported
        );
        assert_eq!(
            read("x.pdf", "image/png", b"%PDF-1.3").unwrap_err(),
            Failure::Unsupported
        );
        assert_eq!(
            read("x.txt", "text/plain", b"\x7f\x01").unwrap_err(),
            Failure::Unreadable
        );
        assert_eq!(
            read("note.txt", "text/plain", b"\xff\xfeN\0o\0t\0e\0")
                .unwrap()
                .text,
            "Note"
        );
    }

    struct MixedPdf;
    impl PdfWorker for MixedPdf {
        fn execute(&self, request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
            assert_eq!(std::fs::read(&request.source).unwrap(), b"%PDF-fixture");
            if let Some(render) = &request.render {
                assert_eq!(render.pages, BTreeSet::from([2]));
                std::fs::write(render.render_dir.join("page-0002.png"), b"read the raster")
                    .unwrap();
            }
            let pages = vec![
                PdfPage {
                    index: 1,
                    chars: 7,
                    text: Some("Paid $35".to_owned()),
                    ..Default::default()
                },
                PdfPage {
                    index: 2,
                    width_pt: 600.,
                    height_pt: 800.,
                    ..Default::default()
                },
            ];
            Ok(PdfPayload {
                page_count: 2,
                pages,
                ..Default::default()
            })
        }
    }
    #[test]
    fn short_pdf_text_survives_a_missing_reader_and_only_sparse_pages_get_ocr() {
        for ready in [true, false] {
            let file = prepare(
                "invoice.pdf",
                "application/pdf",
                b"%PDF-fixture",
                &|| Ok(Box::new(MixedPdf)),
                &NullWireClient,
                &mut |png| {
                    assert_eq!(png, b"read the raster");
                    if ready {
                        Ok("Second page".to_owned())
                    } else {
                        Err(Failure::Unavailable)
                    }
                },
            )
            .unwrap();
            assert!(file.text.contains("Paid $35"));
            assert_eq!(file.text.contains("Second page"), ready);
            assert_eq!(file.partial, !ready);
            assert_eq!(file.thoughts, 0);
        }
    }

    struct TestVision(&'static str);
    impl WireClient for TestVision {
        fn execute(
            &self,
            req: &solstone_core_generate::GenerateRequest,
        ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError>
        {
            use base64::Engine;
            use solstone_core_generate::{ContentPart, GenerateResponse, GeneratedResponse};
            let image = req
                .contents
                .iter()
                .find_map(|c| match c {
                    ContentPart::Image { data, .. } => Some(data),
                    _ => None,
                })
                .unwrap();
            let jpeg = base64::engine::general_purpose::STANDARD
                .decode(image)
                .unwrap();
            let pixels = image::load_from_memory(&jpeg).unwrap();
            assert_eq!((pixels.width(), pixels.height()), (512, 384));
            assert!(!jpeg.windows(8).any(|b| b == b"GPS_TEST"));
            Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
                id: None,
                text: self.0.to_owned(),
                model: "fixture".into(),
                usage: serde_json::json!({}),
                finish_reason: "stop".into(),
                thinking: None,
                schema_validation: None,
                input_budget: None,
                request_budget: None,
                inference: None,
                hints_applied: Vec::new(),
            })))
        }
    }

    #[test]
    fn photograph_keeps_a_new_thumbnail_and_one_attributed_reading() {
        let img = DynamicImage::new_rgb8(800, 600);
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        let mut bytes = buf.into_inner();
        bytes.splice(2..2, [0xff, 0xfe, 0, 10].into_iter().chain(*b"GPS_TEST"));
        let response =
            r#"{"description":"The blue sheet","visible_text":"AGENDA","belonging_class":null}"#;
        let output = prepare(
            "site.jpg",
            "image/jpeg",
            &bytes,
            &unused_pdf,
            &TestVision(response),
            &mut unused_ocr,
        )
        .unwrap();
        assert_eq!(output.kind, "photos");
        assert_eq!(output.thoughts, 1);
        assert!(output.text.contains("The blue sheet") && output.text.contains("AGENDA"));
        let thumb = output.preview_jpeg.unwrap();
        assert!(!thumb.windows(8).any(|b| b == b"GPS_TEST"));
        assert_eq!(
            prepare(
                "site.jpg",
                "image/jpeg",
                &bytes,
                &unused_pdf,
                &TestVision(""),
                &mut unused_ocr
            )
            .unwrap_err(),
            Failure::DescriptionUnavailable
        );
        assert_eq!(
            prepare(
                "site.jpg",
                "image/jpeg",
                &bytes,
                &unused_pdf,
                &NullWireClient,
                &mut unused_ocr
            )
            .unwrap_err(),
            Failure::Unavailable
        );
    }

    #[test]
    fn screenshot_uses_only_ocr_and_keeps_small_preview() {
        let img = DynamicImage::new_rgb8(800, 600);
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        for name in [
            "Screenshot_20250407-123123.png",
            "screenshot_screen.png",
            "screenshot_cafed00d_screen.png",
        ] {
            let file = prepare(
                name,
                "image/png",
                buf.get_ref(),
                &unused_pdf,
                &NullWireClient,
                &mut |pixels| {
                    assert_eq!(image::guess_format(pixels).unwrap(), ImageFormat::Png);
                    Ok("A legible receipt".into())
                },
            )
            .unwrap();
            assert_eq!(file.kind, "screenshots");
            assert_eq!(file.text, "A legible receipt");
            assert_eq!(file.thoughts, 0);
            let thumb = image::load_from_memory(&file.preview_jpeg.unwrap()).unwrap();
            assert_eq!((thumb.width(), thumb.height()), (320, 240));
        }
    }

    fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as _;
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, data) in entries {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn minimal_docx(body_text: &str) -> Vec<u8> {
        let doc_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>{body_text}</w:t></w:r></w:p></w:body>
</w:document>"#
        );
        stored_zip(&[
            (
                "[Content_Types].xml",
                br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
            ),
            ("word/document.xml", doc_xml.as_bytes()),
        ])
    }

    fn minimal_encrypted_ooxml() -> Vec<u8> {
        // Minimal MS-CFB header with Root Entry -> EncryptionInfo + EncryptedPackage.
        const SECTOR: usize = 512;
        const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
        const FATSECT: u32 = 0xFFFF_FFFD;
        const FREESECT: u32 = 0xFFFF_FFFF;
        const NOSTREAM: u32 = 0xFFFF_FFFF;
        let mut buf = vec![0u8; SECTOR * 3];
        buf[0..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
        buf[24..26].copy_from_slice(&0x003Eu16.to_le_bytes());
        buf[26..28].copy_from_slice(&0x0003u16.to_le_bytes());
        buf[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
        buf[30..32].copy_from_slice(&9u16.to_le_bytes());
        buf[32..34].copy_from_slice(&6u16.to_le_bytes());
        buf[44..48].copy_from_slice(&1u32.to_le_bytes()); // FAT count
        buf[48..52].copy_from_slice(&1u32.to_le_bytes()); // dir first sector = 1
        buf[56..60].copy_from_slice(&4096u32.to_le_bytes());
        buf[60..64].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
        buf[68..72].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
        buf[76..80].copy_from_slice(&0u32.to_le_bytes()); // DIFAT[0] = sector 0
        for slot in 1..109 {
            let off = 76 + slot * 4;
            buf[off..off + 4].copy_from_slice(&FREESECT.to_le_bytes());
        }
        // FAT at sector 0 (offset 512)
        buf[SECTOR..SECTOR + 4].copy_from_slice(&FATSECT.to_le_bytes());
        buf[SECTOR + 4..SECTOR + 8].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
        for i in 2..128 {
            let off = SECTOR + i * 4;
            buf[off..off + 4].copy_from_slice(&FREESECT.to_le_bytes());
        }
        // Directory at sector 1 (offset 1024)
        let write_entry =
            |slot: &mut [u8], name: &str, kind: u8, left: u32, right: u32, child: u32| {
                let utf16: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                for (idx, u) in utf16.iter().enumerate() {
                    slot[idx * 2..idx * 2 + 2].copy_from_slice(&u.to_le_bytes());
                }
                slot[64..66].copy_from_slice(&((utf16.len() * 2) as u16).to_le_bytes());
                slot[66] = kind;
                slot[67] = 1;
                slot[68..72].copy_from_slice(&left.to_le_bytes());
                slot[72..76].copy_from_slice(&right.to_le_bytes());
                slot[76..80].copy_from_slice(&child.to_le_bytes());
                slot[116..120].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
            };
        let dir = 2 * SECTOR;
        write_entry(
            &mut buf[dir..dir + 128],
            "Root Entry",
            5,
            NOSTREAM,
            NOSTREAM,
            1,
        );
        write_entry(
            &mut buf[dir + 128..dir + 256],
            "EncryptionInfo",
            2,
            NOSTREAM,
            2,
            NOSTREAM,
        );
        write_entry(
            &mut buf[dir + 256..dir + 384],
            "EncryptedPackage",
            2,
            NOSTREAM,
            NOSTREAM,
            NOSTREAM,
        );
        buf
    }

    #[test]
    fn document_attachment_type_table_stays_in_sync_with_doc_extract_and_guards_spoofs() {
        // Every extension supported by solstone-core-doc-extract maps to FileType::Document.
        for ext in solstone_core_doc_extract::SUPPORTED_EXTENSIONS {
            let name = format!("file.{ext}");
            assert_eq!(
                file_type(&name, "application/octet-stream"),
                Some(FileType::Document),
                "missing attachment FileType::Document for .{ext}"
            );
            assert_eq!(
                file_type(&name, ""),
                Some(FileType::Document),
                "missing attachment FileType::Document (empty mime) for .{ext}"
            );
        }

        // Every canonical document MIME maps to FileType::Document (including unnamed
        // MIME-derived names and parameterized Content-Type headers) and has a synthetic hint.
        let mimes = [
            "application/msword",
            "application/vnd.ms-excel",
            "application/vnd.ms-powerpoint",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/vnd.ms-word.document.macroenabled.12",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/vnd.ms-excel.sheet.macroenabled.12",
            "application/vnd.ms-excel.sheet.binary.macroenabled.12",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "application/vnd.ms-powerpoint.presentation.macroenabled.12",
            "application/vnd.openxmlformats-officedocument.presentationml.slideshow",
            "application/epub+zip",
            "application/vnd.oasis.opendocument.text",
            "application/vnd.oasis.opendocument.spreadsheet",
            "application/vnd.oasis.opendocument.presentation",
            "application/rtf",
            "text/rtf",
            "application/x-rtf",
            "message/rfc822",
            "text/csv",
            "application/csv",
            "text/tab-separated-values",
            "text/html",
        ];
        for mime in mimes {
            assert_eq!(
                file_type("", mime),
                Some(FileType::Document),
                "empty name {mime}"
            );
            assert_eq!(
                file_type(mime, mime),
                Some(FileType::Document),
                "unnamed {mime}"
            );
            let with_param = format!("{mime}; charset=utf-8");
            assert_eq!(
                file_type(mime, &with_param),
                Some(FileType::Document),
                "parameterized {mime}"
            );
            let hint = synthetic_hint_for_mime(&with_param).expect(mime);
            assert!(
                solstone_core_doc_extract::DocumentFormat::from_filename(hint).is_some(),
                "hint {hint} for {mime} must resolve in solstone-core-doc-extract"
            );
        }

        // Outlook CSV MIME quirk is accepted, while spoofs and mismatches are rejected.
        assert_eq!(
            file_type("data.csv", "application/vnd.ms-excel"),
            Some(FileType::Document)
        );
        for (name, mime) in [
            ("meeting.ics", "text/html"),
            ("script.js", "text/csv"),
            ("orders.csv", "text/plain"),
            ("archive.pdf", "application/zip"),
        ] {
            assert_eq!(file_type(name, mime), None, "{name} ({mime}) must reject");
        }
    }

    #[test]
    fn pure_rust_document_attachments_prepare_without_pdf_or_model_including_unnamed_mimes_and_errors()
     {
        let read = |name: &str, mime: &str, bytes: &[u8]| {
            prepare(
                name,
                mime,
                bytes,
                &unused_pdf,
                &NullWireClient,
                &mut unused_ocr,
            )
        };

        // Named DOCX, RTF, and Outlook CSV.
        let docx = minimal_docx("Quarterly roadmap proposal.");
        let docx_out = read(
            "proposal.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            &docx,
        )
        .unwrap();
        assert_eq!(docx_out.kind, "documents");
        assert_eq!(docx_out.thoughts, 0);
        assert!(!docx_out.partial);
        assert!(docx_out.text.contains("Quarterly roadmap proposal."));

        let rtf_out = read(
            "memo.rtf",
            "application/rtf",
            br"{\rtf1\ansi\ansicpg1252 Memo \'e4\par}",
        )
        .unwrap();
        assert_eq!(rtf_out.kind, "documents");
        assert_eq!(rtf_out.thoughts, 0);
        assert!(rtf_out.text.contains("Memo ä"));

        let outlook_csv = read(
            "data.csv",
            "application/vnd.ms-excel",
            b"Item,Qty\nBolt,4\n",
        )
        .unwrap();
        assert_eq!(outlook_csv.kind, "documents");
        assert_eq!(outlook_csv.thoughts, 0);
        assert!(outlook_csv.text.contains("| Bolt | 4 |"));

        // Extensionless / MIME-derived attachment names (as produced by attachment_parts.rs).
        let eml_bytes =
            b"From: Alice <a@example.org>\r\nSubject: Forwarded\r\n\r\nAttached message body.\r\n";
        let eml_out = read("message/rfc822", "message/rfc822", eml_bytes).unwrap();
        assert_eq!(eml_out.kind, "documents");
        assert_eq!(eml_out.thoughts, 0);
        assert!(eml_out.text.contains("Attached message body."));

        let unnamed_csv = read("text/csv", "text/csv; charset=utf-8", b"A,B\n1,2\n").unwrap();
        assert_eq!(unnamed_csv.kind, "documents");
        assert!(unnamed_csv.text.contains("| 1 | 2 |"));

        let unnamed_tsv = read(
            "text/tab-separated-values",
            "text/tab-separated-values",
            b"K\tV\nx\t9\n",
        )
        .unwrap();
        assert_eq!(unnamed_tsv.kind, "documents");
        assert!(unnamed_tsv.text.contains("| x | 9 |"));

        let unnamed_html = read(
            "text/html",
            "text/html; charset=utf-8",
            b"<p>Fragment paragraph.</p>",
        )
        .unwrap();
        assert_eq!(unnamed_html.kind, "documents");
        assert!(unnamed_html.text.contains("Fragment paragraph."));

        let docx_mime = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        let unnamed_docx = read(docx_mime, docx_mime, &docx).unwrap();
        assert_eq!(unnamed_docx.kind, "documents");
        assert!(unnamed_docx.text.contains("Quarterly roadmap proposal."));
        assert_eq!(
            read(docx_mime, docx_mime, b"corrupt not a zip").unwrap_err(),
            Failure::Unreadable
        );

        // Encrypted, empty, and over-budget failure mapping.
        let encrypted = minimal_encrypted_ooxml();
        assert_eq!(
            read("locked.docx", docx_mime, &encrypted).unwrap_err(),
            Failure::Encrypted
        );
        let empty_docx = minimal_docx("");
        assert_eq!(
            read("blank.docx", docx_mime, &empty_docx).unwrap_err(),
            Failure::Unreadable
        );
        let deep_rtf = format!("{{\\rtf1{}x{}}}", "{".repeat(2000), "}".repeat(2000));
        assert_eq!(
            read("bomb.rtf", "application/rtf", deep_rtf.as_bytes()).unwrap_err(),
            Failure::OverBudget
        );

        // Document exceeding max_output_chars propagates doc.truncated -> partial == true.
        let huge_rtf = format!("{{\\rtf1\\ansi {}}}", "0123456789abcdef ".repeat(40_000));
        let truncated_out = read("huge.rtf", "application/rtf", huge_rtf.as_bytes()).unwrap();
        assert_eq!(truncated_out.kind, "documents");
        assert!(truncated_out.partial);
        assert_eq!(truncated_out.thoughts, 0);

        // Direct unit test of truncate_utf8_bytes on multi-byte UTF-8 boundaries.
        let (cut, was_cut) = truncate_utf8_bytes("héllo 😀 world", 8);
        assert!(was_cut);
        assert!(cut.len() <= 8);
        assert_eq!(cut, "héllo");
        let (full, not_cut) = truncate_utf8_bytes("héllo  ", 64);
        assert!(!not_cut);
        assert_eq!(full, "héllo");
    }
}
