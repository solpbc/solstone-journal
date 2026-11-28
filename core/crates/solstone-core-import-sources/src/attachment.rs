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

use crate::document::{self, PdfCommand, PdfRenderOptions, PdfWorker, WorkerFailure};
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
    #[cfg(feature = "local-attachment-ocr")]
    let mut local = None;
    #[cfg(feature = "local-attachment-ocr")]
    let mut ocr = |png: &[u8]| {
        if local.is_none() {
            local = Some(
                solstone_core_describe::installed_ocr_worker()
                    .ok()
                    .flatten(),
            );
        }
        let worker = local
            .as_mut()
            .and_then(Option::as_mut)
            .ok_or(Failure::Unavailable)?;
        solstone_core_describe::read_still_ocr_text(worker.as_mut(), png)
            .map_err(|_| Failure::Unavailable)
    };
    #[cfg(not(feature = "local-attachment-ocr"))]
    let mut ocr = |_: &[u8]| Err(Failure::Unavailable);
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
}
