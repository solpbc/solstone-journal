// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The native implementation of the frozen `sol-pdf/1` subprocess contract.

use std::collections::BTreeSet;
use std::ffi::{CString, c_char, c_int, c_ulong, c_void};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::OnceLock;

use image::{ImageBuffer, Rgb};
use libloading::Library;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SCHEMA: &str = "sol-pdf/1";
const EXIT_OK: i32 = 0;
const EXIT_INTERNAL: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_ENCRYPTED: i32 = 3;
const EXIT_CORRUPT: i32 = 4;
const EXIT_RENDER_IO: i32 = 5;

#[cfg(unix)]
const ENV_RLIMIT_AS_MB: &str = "SOLSTONE_PDF_WORKER_RLIMIT_AS_MB";
#[cfg(unix)]
const ENV_RLIMIT_CPU_SECONDS: &str = "SOLSTONE_PDF_WORKER_RLIMIT_CPU_SECONDS";
#[cfg(unix)]
const DEFAULT_RLIMIT_AS_MB: u64 = 2048;
#[cfg(unix)]
const DEFAULT_RLIMIT_CPU_SECONDS: u64 = 60;

const FPDF_ERR_FILE: c_ulong = 2;
const FPDF_ERR_FORMAT: c_ulong = 3;
const FPDF_ERR_PASSWORD: c_ulong = 4;
const FPDF_ERR_SECURITY: c_ulong = 5;
const FPDF_PAGEOBJ_IMAGE: c_int = 3;
const FPDF_BITMAP_BGR: c_int = 2;

type FpdfDocument = *mut c_void;
type FpdfPage = *mut c_void;
type FpdfTextPage = *mut c_void;
type FpdfPageObject = *mut c_void;
type FpdfBitmap = *mut c_void;

type InitLibrary = unsafe extern "C" fn();
type DestroyLibrary = unsafe extern "C" fn();
type LoadDocument = unsafe extern "C" fn(*const c_char, *const c_char) -> FpdfDocument;
type GetLastError = unsafe extern "C" fn() -> c_ulong;
type CloseDocument = unsafe extern "C" fn(FpdfDocument);
type GetPageCount = unsafe extern "C" fn(FpdfDocument) -> c_int;
type GetMetaText =
    unsafe extern "C" fn(FpdfDocument, *const c_char, *mut c_void, c_ulong) -> c_ulong;
type GetSecurityHandlerRevision = unsafe extern "C" fn(FpdfDocument) -> c_int;
type LoadPage = unsafe extern "C" fn(FpdfDocument, c_int) -> FpdfPage;
type ClosePage = unsafe extern "C" fn(FpdfPage);
type GetPageWidthF = unsafe extern "C" fn(FpdfPage) -> f32;
type GetPageHeightF = unsafe extern "C" fn(FpdfPage) -> f32;
type TextLoadPage = unsafe extern "C" fn(FpdfPage) -> FpdfTextPage;
type TextClosePage = unsafe extern "C" fn(FpdfTextPage);
type TextCountChars = unsafe extern "C" fn(FpdfTextPage) -> c_int;
type TextGetText = unsafe extern "C" fn(FpdfTextPage, c_int, c_int, *mut u16) -> c_int;
type PageCountObjects = unsafe extern "C" fn(FpdfPage) -> c_int;
type PageGetObject = unsafe extern "C" fn(FpdfPage, c_int) -> FpdfPageObject;
type PageObjGetType = unsafe extern "C" fn(FpdfPageObject) -> c_int;
type PageObjGetBounds =
    unsafe extern "C" fn(FpdfPageObject, *mut f32, *mut f32, *mut f32, *mut f32) -> c_int;
type BitmapCreateEx = unsafe extern "C" fn(c_int, c_int, c_int, *mut c_void, c_int) -> FpdfBitmap;
type BitmapFillRect = unsafe extern "C" fn(FpdfBitmap, c_int, c_int, c_int, c_int, u32) -> c_int;
type RenderPageBitmap =
    unsafe extern "C" fn(FpdfBitmap, FpdfPage, c_int, c_int, c_int, c_int, c_int, c_int);
type BitmapGetBuffer = unsafe extern "C" fn(FpdfBitmap) -> *mut c_void;
type BitmapGetStride = unsafe extern "C" fn(FpdfBitmap) -> c_int;
type BitmapDestroy = unsafe extern "C" fn(FpdfBitmap);

struct Pdfium {
    _library: Library,
    init_library: InitLibrary,
    destroy_library: DestroyLibrary,
    load_document: LoadDocument,
    get_last_error: GetLastError,
    close_document: CloseDocument,
    get_page_count: GetPageCount,
    get_meta_text: GetMetaText,
    get_security_handler_revision: GetSecurityHandlerRevision,
    load_page: LoadPage,
    close_page: ClosePage,
    get_page_width_f: GetPageWidthF,
    get_page_height_f: GetPageHeightF,
    text_load_page: TextLoadPage,
    text_close_page: TextClosePage,
    text_count_chars: TextCountChars,
    text_get_text: TextGetText,
    page_count_objects: PageCountObjects,
    page_get_object: PageGetObject,
    page_obj_get_type: PageObjGetType,
    page_obj_get_bounds: PageObjGetBounds,
    bitmap_create_ex: BitmapCreateEx,
    bitmap_fill_rect: BitmapFillRect,
    render_page_bitmap: RenderPageBitmap,
    bitmap_get_buffer: BitmapGetBuffer,
    bitmap_get_stride: BitmapGetStride,
    bitmap_destroy: BitmapDestroy,
}

impl Pdfium {
    fn load() -> Result<Self, String> {
        let library_path = pdfium_library_path()?;
        let library = open_pdfium_library(&library_path)?;
        macro_rules! symbol {
            ($name:literal, $kind:ty) => {{
                let symbol = unsafe { library.get::<$kind>(concat!($name, "\0").as_bytes()) }
                    .map_err(|error| format!("resolve PDFium {}: {error}", $name))?;
                *symbol
            }};
        }
        Ok(Self {
            init_library: symbol!("FPDF_InitLibrary", InitLibrary),
            destroy_library: symbol!("FPDF_DestroyLibrary", DestroyLibrary),
            load_document: symbol!("FPDF_LoadDocument", LoadDocument),
            get_last_error: symbol!("FPDF_GetLastError", GetLastError),
            close_document: symbol!("FPDF_CloseDocument", CloseDocument),
            get_page_count: symbol!("FPDF_GetPageCount", GetPageCount),
            get_meta_text: symbol!("FPDF_GetMetaText", GetMetaText),
            get_security_handler_revision: symbol!(
                "FPDF_GetSecurityHandlerRevision",
                GetSecurityHandlerRevision
            ),
            load_page: symbol!("FPDF_LoadPage", LoadPage),
            close_page: symbol!("FPDF_ClosePage", ClosePage),
            get_page_width_f: symbol!("FPDF_GetPageWidthF", GetPageWidthF),
            get_page_height_f: symbol!("FPDF_GetPageHeightF", GetPageHeightF),
            text_load_page: symbol!("FPDFText_LoadPage", TextLoadPage),
            text_close_page: symbol!("FPDFText_ClosePage", TextClosePage),
            text_count_chars: symbol!("FPDFText_CountChars", TextCountChars),
            text_get_text: symbol!("FPDFText_GetText", TextGetText),
            page_count_objects: symbol!("FPDFPage_CountObjects", PageCountObjects),
            page_get_object: symbol!("FPDFPage_GetObject", PageGetObject),
            page_obj_get_type: symbol!("FPDFPageObj_GetType", PageObjGetType),
            page_obj_get_bounds: symbol!("FPDFPageObj_GetBounds", PageObjGetBounds),
            bitmap_create_ex: symbol!("FPDFBitmap_CreateEx", BitmapCreateEx),
            bitmap_fill_rect: symbol!("FPDFBitmap_FillRect", BitmapFillRect),
            render_page_bitmap: symbol!("FPDF_RenderPageBitmap", RenderPageBitmap),
            bitmap_get_buffer: symbol!("FPDFBitmap_GetBuffer", BitmapGetBuffer),
            bitmap_get_stride: symbol!("FPDFBitmap_GetStride", BitmapGetStride),
            bitmap_destroy: symbol!("FPDFBitmap_Destroy", BitmapDestroy),
            _library: library,
        })
    }

    fn initialize(&self) {
        unsafe { (self.init_library)() };
    }

    fn destroy(&self) {
        unsafe { (self.destroy_library)() };
    }
}

fn open_pdfium_library(path: &Path) -> Result<Library, String> {
    #[cfg(windows)]
    {
        restrict_default_dll_directories()?;
        if !path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::CurDir | std::path::Component::ParentDir
                )
            })
        {
            return Err(format!(
                "load PDFium {}: expected an absolute path without traversal",
                path.display()
            ));
        }
        // SAFETY: the bounded package owner has verified this exact private
        // payload member before supplying the child environment. The flags
        // search only the DLL's directory and System32 for dependencies.
        let library =
            unsafe { libloading::os::windows::Library::load_with_flags(path, 0x0000_0900) }
                .map_err(|error| format!("load PDFium {}: {error}", path.display()))?;
        Ok(Library::from(library))
    }

    #[cfg(not(windows))]
    {
        // SAFETY: Unix callers retain the established explicit-path dynamic-load contract.
        unsafe { Library::new(path) }
            .map_err(|error| format!("load PDFium {}: {error}", path.display()))
    }
}

#[cfg(windows)]
fn restrict_default_dll_directories() -> Result<(), String> {
    static RESTRICTED: OnceLock<Result<(), String>> = OnceLock::new();
    match RESTRICTED.get_or_init(|| {
        // Application-directory and System32 are the only default DLL
        // search locations. The actual PDFium load adds its private
        // library directory explicitly through LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR.
        const APPLICATION_DIR_AND_SYSTEM32: u32 = 0x0000_0200 | 0x0000_0800;
        // SAFETY: this documented process-wide restriction only changes
        // the default DLL search locations for later loads.
        let ok = unsafe { SetDefaultDllDirectories(APPLICATION_DIR_AND_SYSTEM32) };
        if ok == 0 {
            return Err(format!(
                "restrict DLL search before loading PDFium: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(())
    }) {
        Ok(()) => Ok(()),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetDefaultDllDirectories(directory_flags: u32) -> i32;
}

#[derive(Debug)]
struct ContractError {
    exit_code: i32,
    error: &'static str,
    detail: Option<String>,
}

impl ContractError {
    fn usage(detail: impl Into<String>) -> Self {
        Self {
            exit_code: EXIT_USAGE,
            error: "usage",
            detail: Some(detail.into()),
        }
    }

    fn corrupt(detail: impl Into<String>) -> Self {
        Self {
            exit_code: EXIT_CORRUPT,
            error: "corrupt",
            detail: Some(detail.into()),
        }
    }

    fn encrypted() -> Self {
        Self {
            exit_code: EXIT_ENCRYPTED,
            error: "encrypted",
            detail: None,
        }
    }

    fn render_io(detail: impl Into<String>) -> Self {
        Self {
            exit_code: EXIT_RENDER_IO,
            error: "render-io",
            detail: Some(detail.into()),
        }
    }

    fn internal(detail: impl Into<String>) -> Self {
        Self {
            exit_code: EXIT_INTERNAL,
            error: "internal",
            detail: Some(detail.into()),
        }
    }

    fn payload(&self) -> Value {
        let mut payload = serde_json::Map::from_iter([
            ("schema".to_owned(), json!(SCHEMA)),
            ("error".to_owned(), json!(self.error)),
        ]);
        if let Some(detail) = &self.detail {
            payload.insert("detail".to_owned(), json!(detail));
        }
        Value::Object(payload)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Inspect,
    Extract,
}

#[derive(Debug)]
struct Arguments {
    command: CommandKind,
    pdf_path: PathBuf,
    password: Option<String>,
    render: RenderRequest,
}

#[derive(Debug, Clone)]
struct RenderRequest {
    requested: bool,
    render_dir: Option<PathBuf>,
    dpi: i32,
    below_chars: Option<i64>,
    above_image_fraction: Option<f64>,
    pages: BTreeSet<usize>,
}

impl RenderRequest {
    fn none() -> Self {
        Self {
            requested: false,
            render_dir: None,
            dpi: 150,
            below_chars: None,
            above_image_fraction: None,
            pages: BTreeSet::new(),
        }
    }
}

#[derive(Serialize)]
struct Metadata {
    title: Option<String>,
    author: Option<String>,
    creation_date: Option<String>,
    mod_date: Option<String>,
    producer: Option<String>,
}

#[derive(Serialize, Clone)]
struct PageEntry {
    index: usize,
    chars: usize,
    width_pt: f64,
    height_pt: f64,
    image_area_fraction: f64,
    rendered: Option<String>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Serialize)]
struct RenderPayload {
    dpi: i32,
    dir: String,
}

#[derive(Serialize)]
struct SuccessPayload {
    schema: &'static str,
    engine: String,
    sha256: String,
    page_count: usize,
    encrypted: bool,
    metadata: Metadata,
    pages: Vec<PageEntry>,
    render: Option<RenderPayload>,
    warnings: Vec<String>,
}

/// Runs the process-facing command after applying the reference worker limits.
pub fn entrypoint(args: impl IntoIterator<Item = std::ffi::OsString>) -> i32 {
    if let Err(error) = apply_rlimits_from_env() {
        return write_payload_and_exit(ContractError::internal(error).payload(), EXIT_INTERNAL);
    }
    let args = args.into_iter().collect::<Vec<_>>();
    if args.len() == 1 && args[0] == "--version" {
        println!("solstone-core-pdf {}", env!("CARGO_PKG_VERSION"));
        return EXIT_OK;
    }
    match parse_arguments(&args).and_then(run) {
        Ok(payload) => write_payload_and_exit(
            serde_json::to_value(payload).expect("serialize success"),
            EXIT_OK,
        ),
        Err(error) => write_payload_and_exit(error.payload(), error.exit_code),
    }
}

fn write_payload_and_exit(payload: Value, exit_code: i32) -> i32 {
    let serialized = match serde_json::to_vec(&payload) {
        Ok(serialized) => serialized,
        Err(_) => return EXIT_INTERNAL,
    };
    let stdout = io::stdout();
    let mut output = stdout.lock();
    if output
        .write_all(&serialized)
        .and_then(|()| output.write_all(b"\n"))
        .and_then(|()| output.flush())
        .is_err()
    {
        return EXIT_INTERNAL;
    }
    exit_code
}

fn parse_arguments(args: &[std::ffi::OsString]) -> Result<Arguments, ContractError> {
    let args = args
        .iter()
        .map(|arg| {
            arg.to_str()
                .map(str::to_owned)
                .ok_or_else(|| ContractError::usage("arguments must be valid Unicode"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let command = match args.first().map(String::as_str) {
        Some("inspect") => CommandKind::Inspect,
        Some("extract") => CommandKind::Extract,
        Some(value) => return Err(ContractError::usage(format!("unknown command: {value}"))),
        None => {
            return Err(ContractError::usage(
                "the following arguments are required: command, pdf_path",
            ));
        }
    };
    let mut pdf_path = None;
    let mut password = None;
    let mut render = RenderRequest::none();
    let mut position = 1;
    while position < args.len() {
        let argument = &args[position];
        if !argument.starts_with("--") {
            if pdf_path.replace(PathBuf::from(argument)).is_some() {
                return Err(ContractError::usage(
                    "only one pdf_path argument is allowed",
                ));
            }
            position += 1;
            continue;
        }
        let (flag, value) = if let Some((flag, value)) = argument.split_once('=') {
            (flag, value)
        } else {
            let value = args.get(position + 1).ok_or_else(|| {
                ContractError::usage(format!("argument {argument}: expected one argument"))
            })?;
            position += 1;
            (argument.as_str(), value.as_str())
        };
        match flag {
            "--password" => password = Some(value.to_owned()),
            "--render-below-chars" if command == CommandKind::Extract => {
                let parsed = value
                    .parse::<i64>()
                    .map_err(|_| ContractError::usage(format!("invalid int value: '{value}'")))?;
                if parsed < 0 {
                    return Err(ContractError::usage(
                        "--render-below-chars must be non-negative",
                    ));
                }
                render.below_chars = Some(parsed);
                render.requested = true;
            }
            "--render-above-image-fraction" if command == CommandKind::Extract => {
                let parsed = value
                    .parse::<f64>()
                    .map_err(|_| ContractError::usage(format!("invalid float value: '{value}'")))?;
                if parsed < 0.0 {
                    return Err(ContractError::usage(
                        "--render-above-image-fraction must be non-negative",
                    ));
                }
                render.above_image_fraction = Some(parsed);
                render.requested = true;
            }
            "--render-pages" if command == CommandKind::Extract => {
                render.pages = parse_render_pages(value)?;
                render.requested = true;
            }
            "--render-dir" if command == CommandKind::Extract => {
                render.render_dir = Some(PathBuf::from(value))
            }
            "--dpi" if command == CommandKind::Extract => {
                let parsed = value
                    .parse::<i32>()
                    .map_err(|_| ContractError::usage(format!("invalid int value: '{value}'")))?;
                if parsed <= 0 {
                    return Err(ContractError::usage("--dpi must be positive"));
                }
                render.dpi = parsed;
            }
            _ => {
                return Err(ContractError::usage(format!(
                    "unrecognized arguments: {flag} {value}"
                )));
            }
        }
        position += 1;
    }
    if render.requested && render.render_dir.is_none() {
        return Err(ContractError::usage(
            "--render-dir is required when render selectors are used",
        ));
    }
    let pdf_path = pdf_path
        .ok_or_else(|| ContractError::usage("the following arguments are required: pdf_path"))?;
    if !pdf_path.is_file() {
        return Err(ContractError::usage(format!(
            "PDF not found: {}",
            pdf_path.display()
        )));
    }
    if let Some(dir) = &render.render_dir {
        render.render_dir = Some(dir.canonicalize().unwrap_or_else(|_| absolute_path(dir)));
    }
    Ok(Arguments {
        command,
        pdf_path,
        password,
        render,
    })
}

fn parse_render_pages(raw: &str) -> Result<BTreeSet<usize>, ContractError> {
    if raw.trim().is_empty() {
        return Err(ContractError::usage(
            "--render-pages must contain at least one page",
        ));
    }
    let mut pages = BTreeSet::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(ContractError::usage(
                "--render-pages contains an empty page number",
            ));
        }
        let page = part
            .parse::<usize>()
            .map_err(|_| ContractError::usage(format!("invalid page number: {part}")))?;
        if page == 0 {
            return Err(ContractError::usage(
                "--render-pages uses 1-based positive page numbers",
            ));
        }
        pages.insert(page);
    }
    Ok(pages)
}

fn run(arguments: Arguments) -> Result<SuccessPayload, ContractError> {
    let pdfium = Pdfium::load().map_err(ContractError::internal)?;
    pdfium.initialize();
    let result = extract_document(&pdfium, &arguments);
    pdfium.destroy();
    result
}

fn extract_document(
    pdfium: &Pdfium,
    arguments: &Arguments,
) -> Result<SuccessPayload, ContractError> {
    let path = CString::new(arguments.pdf_path.to_string_lossy().as_bytes())
        .map_err(|_| ContractError::usage("PDF path contains a NUL byte"))?;
    let password = arguments
        .password
        .as_deref()
        .map(CString::new)
        .transpose()
        .map_err(|_| ContractError::usage("password contains a NUL byte"))?;
    let document = unsafe {
        (pdfium.load_document)(
            path.as_ptr(),
            password
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
        )
    };
    if document.is_null() {
        let error = unsafe { (pdfium.get_last_error)() };
        return Err(open_error(error));
    }
    let document_guard = DocumentGuard { pdfium, document };
    let page_count = unsafe { (pdfium.get_page_count)(document) };
    if page_count < 0 {
        return Err(ContractError::internal(
            "PDFium returned a negative page count",
        ));
    }
    let page_count = page_count as usize;
    let include_text = arguments.command == CommandKind::Extract;
    let mut warnings = Vec::new();
    let mut pages = (0..page_count)
        .map(|index| {
            extract_page(
                pdfium,
                document_guard.document,
                index,
                include_text,
                &mut warnings,
            )
        })
        .collect::<Vec<_>>();
    let render = if arguments.render.requested {
        if arguments
            .render
            .pages
            .last()
            .is_some_and(|page| *page > page_count)
        {
            return Err(ContractError::usage(
                "--render-pages contains a page beyond the document",
            ));
        }
        let directory = arguments
            .render
            .render_dir
            .as_ref()
            .expect("validated render directory");
        fs::create_dir_all(directory)
            .map_err(|error| ContractError::render_io(error.to_string()))?;
        render_selected_pages(
            pdfium,
            document_guard.document,
            &mut pages,
            &arguments.render,
            &mut warnings,
        )?;
        Some(RenderPayload {
            dpi: arguments.render.dpi,
            dir: directory.display().to_string(),
        })
    } else {
        None
    };
    let metadata = Metadata {
        title: metadata_text(pdfium, document_guard.document, "Title"),
        author: metadata_text(pdfium, document_guard.document, "Author"),
        creation_date: metadata_text(pdfium, document_guard.document, "CreationDate")
            .and_then(|value| parse_pdf_date(&value)),
        mod_date: metadata_text(pdfium, document_guard.document, "ModDate")
            .and_then(|value| parse_pdf_date(&value)),
        producer: metadata_text(pdfium, document_guard.document, "Producer"),
    };
    Ok(SuccessPayload {
        schema: SCHEMA,
        engine: "pdfium 151.0.7920.0 (native)".to_owned(),
        sha256: sha256_file(&arguments.pdf_path)
            .map_err(|error| ContractError::internal(error.to_string()))?,
        page_count,
        encrypted: unsafe { (pdfium.get_security_handler_revision)(document_guard.document) } != -1,
        metadata,
        pages,
        render,
        warnings,
    })
}

struct DocumentGuard<'a> {
    pdfium: &'a Pdfium,
    document: FpdfDocument,
}
impl Drop for DocumentGuard<'_> {
    fn drop(&mut self) {
        unsafe { (self.pdfium.close_document)(self.document) };
    }
}

struct PageGuard<'a> {
    pdfium: &'a Pdfium,
    page: FpdfPage,
}
impl Drop for PageGuard<'_> {
    fn drop(&mut self) {
        unsafe { (self.pdfium.close_page)(self.page) };
    }
}

struct TextPageGuard<'a> {
    pdfium: &'a Pdfium,
    text_page: FpdfTextPage,
}
impl Drop for TextPageGuard<'_> {
    fn drop(&mut self) {
        unsafe { (self.pdfium.text_close_page)(self.text_page) };
    }
}

struct BitmapGuard<'a> {
    pdfium: &'a Pdfium,
    bitmap: FpdfBitmap,
}
impl Drop for BitmapGuard<'_> {
    fn drop(&mut self) {
        unsafe { (self.pdfium.bitmap_destroy)(self.bitmap) };
    }
}

fn extract_page(
    pdfium: &Pdfium,
    document: FpdfDocument,
    index: usize,
    include_text: bool,
    warnings: &mut Vec<String>,
) -> PageEntry {
    let mut entry = PageEntry {
        index: index + 1,
        chars: 0,
        width_pt: 0.0,
        height_pt: 0.0,
        image_area_fraction: 0.0,
        rendered: None,
        error: None,
        text: include_text.then(String::new),
    };
    match extract_page_values(pdfium, document, index) {
        Ok((width_pt, height_pt, text, image_area_fraction)) => {
            entry.width_pt = width_pt;
            entry.height_pt = height_pt;
            entry.chars = non_whitespace_chars(&text);
            entry.image_area_fraction = image_area_fraction;
            if include_text {
                entry.text = Some(text);
            }
        }
        Err(error) => {
            let message = format!("page {}: page extraction failed: {error}", index + 1);
            entry.error = Some(message.clone());
            warnings.push(message);
        }
    }
    entry
}

fn extract_page_values(
    pdfium: &Pdfium,
    document: FpdfDocument,
    index: usize,
) -> Result<(f64, f64, String, f64), String> {
    let page = unsafe { (pdfium.load_page)(document, index as c_int) };
    if page.is_null() {
        return Err("PDFium could not load page".to_owned());
    }
    let page = PageGuard { pdfium, page };
    let width_pt = unsafe { (pdfium.get_page_width_f)(page.page) } as f64;
    let height_pt = unsafe { (pdfium.get_page_height_f)(page.page) } as f64;
    let text_page = unsafe { (pdfium.text_load_page)(page.page) };
    if text_page.is_null() {
        return Err("PDFium could not load text page".to_owned());
    }
    let text_page = TextPageGuard { pdfium, text_page };
    let count = unsafe { (pdfium.text_count_chars)(text_page.text_page) };
    if count < 0 {
        return Err("PDFium returned a negative text character count".to_owned());
    }
    let mut utf16 = vec![0_u16; count as usize + 1];
    let written =
        unsafe { (pdfium.text_get_text)(text_page.text_page, 0, count, utf16.as_mut_ptr()) };
    if written < 0 {
        return Err("PDFium failed to extract text".to_owned());
    }
    let written = (written as usize).min(utf16.len());
    if written > 0 && utf16[written - 1] == 0 {
        utf16.truncate(written - 1);
    } else {
        utf16.truncate(written);
    }
    let text = String::from_utf16_lossy(&utf16);
    let image_area_fraction = image_area_fraction(pdfium, page.page, width_pt, height_pt);
    Ok((width_pt, height_pt, text, image_area_fraction))
}

fn image_area_fraction(pdfium: &Pdfium, page: FpdfPage, width_pt: f64, height_pt: f64) -> f64 {
    let page_area = width_pt * height_pt;
    if page_area <= 0.0 {
        return 0.0;
    }
    let count = unsafe { (pdfium.page_count_objects)(page) }.max(0);
    let mut image_area = 0.0_f64;
    for index in 0..count {
        let object = unsafe { (pdfium.page_get_object)(page, index) };
        if object.is_null() || unsafe { (pdfium.page_obj_get_type)(object) } != FPDF_PAGEOBJ_IMAGE {
            continue;
        }
        let (mut left, mut bottom, mut right, mut top) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
        if unsafe {
            (pdfium.page_obj_get_bounds)(object, &mut left, &mut bottom, &mut right, &mut top)
        } != 0
        {
            image_area += f64::from((right - left).max(0.0)) * f64::from((top - bottom).max(0.0));
        }
    }
    (image_area / page_area).clamp(0.0, 1.0)
}

fn render_selected_pages(
    pdfium: &Pdfium,
    document: FpdfDocument,
    pages: &mut [PageEntry],
    request: &RenderRequest,
    warnings: &mut Vec<String>,
) -> Result<(), ContractError> {
    let selected = selected_pages(pages, request);
    let directory = request
        .render_dir
        .as_ref()
        .expect("validated render directory");
    for page_number in selected {
        let page = &mut pages[page_number - 1];
        if page.error.is_some() {
            continue;
        }
        let rendered = format!("page-{page_number:04}.png");
        let output = directory.join(&rendered);
        match render_page_png(
            pdfium,
            document,
            page_number - 1,
            page.width_pt,
            page.height_pt,
            request.dpi,
            &output,
        ) {
            Ok(()) => page.rendered = Some(rendered),
            Err(RenderFailure::Io(error)) => {
                return Err(ContractError::render_io(error.to_string()));
            }
            Err(RenderFailure::Pdfium(error)) => {
                let message = format!("page {page_number}: page render failed: {error}");
                page.error = Some(message.clone());
                page.chars = 0;
                page.image_area_fraction = 0.0;
                page.rendered = None;
                if page.text.is_some() {
                    page.text = Some(String::new());
                }
                warnings.push(message);
            }
        }
    }
    Ok(())
}

fn render_page_png(
    pdfium: &Pdfium,
    document: FpdfDocument,
    index: usize,
    width_pt: f64,
    height_pt: f64,
    dpi: i32,
    output: &Path,
) -> Result<(), RenderFailure> {
    let width = pixel_dimension(width_pt, dpi);
    let height = pixel_dimension(height_pt, dpi);
    let width_i32 = i32::try_from(width)
        .map_err(|_| RenderFailure::Pdfium("render width exceeds PDFium limits".to_owned()))?;
    let height_i32 = i32::try_from(height)
        .map_err(|_| RenderFailure::Pdfium("render height exceeds PDFium limits".to_owned()))?;
    let page = unsafe { (pdfium.load_page)(document, index as c_int) };
    if page.is_null() {
        return Err(RenderFailure::Pdfium(
            "PDFium could not load page for rendering".to_owned(),
        ));
    }
    let page = PageGuard { pdfium, page };
    let bitmap = unsafe {
        (pdfium.bitmap_create_ex)(
            width_i32,
            height_i32,
            FPDF_BITMAP_BGR,
            std::ptr::null_mut(),
            0,
        )
    };
    if bitmap.is_null() {
        return Err(RenderFailure::Pdfium(
            "PDFium could not allocate render bitmap".to_owned(),
        ));
    }
    let bitmap = BitmapGuard { pdfium, bitmap };
    if unsafe { (pdfium.bitmap_fill_rect)(bitmap.bitmap, 0, 0, width_i32, height_i32, 0xffff_ffff) }
        == 0
    {
        return Err(RenderFailure::Pdfium(
            "PDFium could not initialize render bitmap".to_owned(),
        ));
    }
    unsafe {
        (pdfium.render_page_bitmap)(bitmap.bitmap, page.page, 0, 0, width_i32, height_i32, 0, 0)
    };
    let stride = unsafe { (pdfium.bitmap_get_stride)(bitmap.bitmap) };
    let buffer = unsafe { (pdfium.bitmap_get_buffer)(bitmap.bitmap) };
    if stride < width_i32 * 3 || buffer.is_null() {
        return Err(RenderFailure::Pdfium(
            "PDFium returned an invalid render bitmap".to_owned(),
        ));
    }
    let source = unsafe {
        std::slice::from_raw_parts(buffer.cast::<u8>(), stride as usize * height as usize)
    };
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for row in source.chunks_exact(stride as usize).take(height as usize) {
        for pixel in row[..width as usize * 3].chunks_exact(3) {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }
    let image = ImageBuffer::<Rgb<u8>, _>::from_raw(width, height, rgb)
        .ok_or_else(|| RenderFailure::Pdfium("invalid render pixel buffer".to_owned()))?;
    image.save(output).map_err(|error| match error {
        image::ImageError::IoError(error) => RenderFailure::Io(error),
        error => RenderFailure::Pdfium(error.to_string()),
    })
}

enum RenderFailure {
    Io(io::Error),
    Pdfium(String),
}

fn metadata_text(pdfium: &Pdfium, document: FpdfDocument, tag: &str) -> Option<String> {
    let tag = CString::new(tag).expect("metadata tags are static");
    let bytes = unsafe { (pdfium.get_meta_text)(document, tag.as_ptr(), std::ptr::null_mut(), 0) };
    if bytes < 2 {
        return None;
    }
    let mut buffer = vec![0_u8; bytes as usize];
    let written = unsafe {
        (pdfium.get_meta_text)(document, tag.as_ptr(), buffer.as_mut_ptr().cast(), bytes)
    };
    if written < 2 {
        return None;
    }
    let values = buffer[..written as usize - 2]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let value = String::from_utf16_lossy(&values);
    (!value.is_empty()).then_some(value)
}

fn selected_pages(pages: &[PageEntry], request: &RenderRequest) -> BTreeSet<usize> {
    let mut selected = request.pages.clone();
    if let Some(threshold) = request.below_chars {
        selected.extend(
            pages
                .iter()
                .filter(|page| page.error.is_none() && page.chars < threshold as usize)
                .map(|page| page.index),
        );
    }
    if let Some(threshold) = request.above_image_fraction {
        selected.extend(
            pages
                .iter()
                .filter(|page| page.error.is_none() && page.image_area_fraction >= threshold)
                .map(|page| page.index),
        );
    }
    selected
}

fn pixel_dimension(points: f64, dpi: i32) -> u32 {
    (points * f64::from(dpi) / 72.0).round_ties_even() as u32
}

fn non_whitespace_chars(text: &str) -> usize {
    text.chars()
        .filter(|character| {
            !character.is_whitespace() && !matches!(character, '\u{001c}'..='\u{001f}')
        })
        .count()
}

fn parse_pdf_date(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    if bytes.len() < 16 || &bytes[..2] != b"D:" || !bytes[2..16].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let number = |start| {
        std::str::from_utf8(&bytes[start..start + 2])
            .ok()?
            .parse::<u32>()
            .ok()
    };
    let year = std::str::from_utf8(&bytes[2..6])
        .ok()?
        .parse::<i32>()
        .ok()?;
    let (month, day, hour, minute, second) = (
        number(6)?,
        number(8)?,
        number(10)?,
        number(12)?,
        number(14)?,
    );
    if !valid_datetime(year, month, day, hour, minute, second) {
        return None;
    }
    let prefix = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}");
    match &bytes[16..] {
        b"Z" => Some(format!("{prefix}Z")),
        [] => None,
        suffix
            if suffix
                .first()
                .is_some_and(|value| *value == b'+' || *value == b'-') =>
        {
            let sign = suffix[0] as char;
            let mut position = 1;
            let read_two_digits = |position: &mut usize| {
                let digits = suffix.get(*position..*position + 2)?;
                if !digits.iter().all(u8::is_ascii_digit) {
                    return None;
                }
                *position += 2;
                std::str::from_utf8(digits).ok()?.parse::<u32>().ok()
            };
            let timezone_hour = read_two_digits(&mut position)?;
            if suffix.get(position) == Some(&b'\'') {
                position += 1;
            }
            let timezone_minute = read_two_digits(&mut position)?;
            if suffix.get(position) == Some(&b'\'') {
                position += 1;
            }
            if position != suffix.len() {
                return None;
            }
            (timezone_hour <= 23 && timezone_minute <= 59)
                .then(|| format!("{prefix}{sign}{timezone_hour:02}:{timezone_minute:02}"))
        }
        _ => None,
    }
}

fn valid_datetime(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> bool {
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return false;
    }
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

fn sha256_file(path: &Path) -> io::Result<String> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Resolve PDFium only from its platform's approved location.
pub fn pdfium_library_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("SOLSTONE_CORE_PDF_LIBRARY") {
        return Ok(PathBuf::from(path));
    }
    let executable =
        std::env::current_exe().map_err(|error| format!("resolve executable path: {error}"))?;
    let filename = if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    };
    executable
        .parent()
        .ok_or_else(|| "executable path has no parent".to_owned())
        .map(|directory| {
            directory
                .join("..")
                .join("lib")
                .join("solstone-core-pdf")
                .join(filename)
        })
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .expect("current directory")
            .join(path)
    }
}

#[cfg(unix)]
fn apply_rlimits_from_env() -> Result<(), String> {
    let address_space = positive_env_int(ENV_RLIMIT_AS_MB, DEFAULT_RLIMIT_AS_MB)?;
    let cpu = positive_env_int(ENV_RLIMIT_CPU_SECONDS, DEFAULT_RLIMIT_CPU_SECONDS)?;
    if let Some(value) = address_space {
        let budget = value.saturating_mul(1024 * 1024);
        set_limit(libc::RLIMIT_AS, address_space_limit(budget)?)?;
    }
    if let Some(value) = cpu {
        set_limit(libc::RLIMIT_CPU, value)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_rlimits_from_env() -> Result<(), String> {
    // Windows receives the equivalent CPU and committed-memory limits from the
    // parent-owned Job before the worker's first instruction.
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn address_space_limit(budget: u64) -> Result<u64, String> {
    Ok(budget)
}

#[cfg(all(unix, target_os = "macos"))]
#[allow(
    deprecated,
    reason = "libc's wrapper is deprecated in favor of a new crate, not a different Darwin API"
)]
fn current_task() -> libc::mach_port_t {
    unsafe { libc::mach_task_self() }
}

#[cfg(all(unix, target_os = "macos"))]
fn address_space_limit(budget: u64) -> Result<u64, String> {
    let mut info = std::mem::MaybeUninit::<libc::mach_task_basic_info>::zeroed();
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    let status = unsafe {
        libc::task_info(
            current_task(),
            libc::MACH_TASK_BASIC_INFO,
            info.as_mut_ptr().cast::<libc::integer_t>(),
            &mut count,
        )
    };
    if status != libc::KERN_SUCCESS {
        return Err(format!("task_info(MACH_TASK_BASIC_INFO): {status}"));
    }
    if count < libc::MACH_TASK_BASIC_INFO_COUNT {
        return Err(format!(
            "task_info(MACH_TASK_BASIC_INFO): short result ({count})"
        ));
    }
    let info = unsafe { info.assume_init() };
    let baseline = info.virtual_size;
    baseline
        .checked_add(budget)
        .ok_or_else(|| "Darwin address-space limit overflow".to_owned())
}

#[cfg(unix)]
fn positive_env_int(name: &str, default: u64) -> Result<Option<u64>, String> {
    let value = match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|error| format!("invalid {name}: {error}"))?,
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => return Err(format!("read {name}: {error}")),
    };
    Ok((value > 0).then_some(value))
}

/// `getrlimit`/`setrlimit` take a different resource type per platform: Linux
/// declares `__rlimit_resource_t` (u32), Darwin declares plain `c_int`. Naming
/// the Linux type unconditionally is what kept this crate -- and therefore the
/// whole workspace build -- from compiling on macOS at all.
#[cfg(all(unix, target_os = "linux"))]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(all(unix, not(target_os = "linux")))]
type RlimitResource = libc::c_int;

#[cfg(unix)]
fn set_limit(kind: RlimitResource, limit: u64) -> Result<(), String> {
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(kind, &mut current) } != 0 {
        return Err(format!("getrlimit({kind}): {}", io::Error::last_os_error()));
    }
    let hard = current.rlim_max;
    let limit = if hard == libc::RLIM_INFINITY {
        limit as libc::rlim_t
    } else {
        (limit as libc::rlim_t).min(hard)
    };
    let requested = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    if unsafe { libc::setrlimit(kind, &requested) } != 0 {
        return Err(format!("setrlimit({kind}): {}", io::Error::last_os_error()));
    }
    Ok(())
}

fn open_error(error: c_ulong) -> ContractError {
    match error {
        FPDF_ERR_PASSWORD | FPDF_ERR_SECURITY => ContractError::encrypted(),
        FPDF_ERR_FILE => ContractError::corrupt("Failed to load document (PDFium: File error)."),
        FPDF_ERR_FORMAT => {
            ContractError::corrupt("Failed to load document (PDFium: Data format error).")
        }
        _ => ContractError::internal(format!("PDFium failed to open document (error {error})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_matches_python_isspace_exception() {
        assert_eq!(
            non_whitespace_chars(" a\t\u{001c}\u{001d}\u{001e}\u{001f}b\u{00a0}"),
            2
        );
    }

    #[test]
    fn dimensions_round_ties_to_even() {
        assert_eq!(pixel_dimension(612.0, 150), 1275);
        assert_eq!(pixel_dimension(792.0, 150), 1650);
        assert_eq!(pixel_dimension(0.24, 150), 0);
        assert_eq!(pixel_dimension(0.72, 150), 2);
    }

    #[test]
    fn pdf_dates_follow_the_frozen_grammar() {
        assert_eq!(
            parse_pdf_date("D:20260304110200-07'00'"),
            Some("2026-03-04T11:02:00-07:00".to_owned())
        );
        assert_eq!(
            parse_pdf_date("D:20260304110200+0230"),
            Some("2026-03-04T11:02:00+02:30".to_owned())
        );
        assert_eq!(
            parse_pdf_date("D:20260304110200Z"),
            Some("2026-03-04T11:02:00Z".to_owned())
        );
        assert_eq!(parse_pdf_date("D:20260304110200"), None);
        assert_eq!(parse_pdf_date("D:20260304110200+99'00'"), None);
        assert_eq!(parse_pdf_date("D:20260230010203Z"), None);
        assert_eq!(parse_pdf_date("D:20260304110200+'0700"), None);
    }

    #[test]
    fn render_selectors_are_a_union() {
        let pages = vec![
            PageEntry {
                index: 1,
                chars: 100,
                width_pt: 0.0,
                height_pt: 0.0,
                image_area_fraction: 0.0,
                rendered: None,
                error: None,
                text: None,
            },
            PageEntry {
                index: 2,
                chars: 0,
                width_pt: 0.0,
                height_pt: 0.0,
                image_area_fraction: 0.0,
                rendered: None,
                error: None,
                text: None,
            },
            PageEntry {
                index: 3,
                chars: 80,
                width_pt: 0.0,
                height_pt: 0.0,
                image_area_fraction: 0.5,
                rendered: None,
                error: None,
                text: None,
            },
        ];
        let request = RenderRequest {
            requested: true,
            render_dir: None,
            dpi: 150,
            below_chars: Some(1),
            above_image_fraction: Some(0.3),
            pages: BTreeSet::from([1]),
        };
        assert_eq!(selected_pages(&pages, &request), BTreeSet::from([1, 2, 3]));
    }

    #[test]
    fn pdfium_open_error_classes_match_the_contract() {
        assert!(matches!(
            open_error(FPDF_ERR_PASSWORD),
            ContractError {
                exit_code: EXIT_ENCRYPTED,
                ..
            }
        ));
        assert!(matches!(
            open_error(FPDF_ERR_SECURITY),
            ContractError {
                exit_code: EXIT_ENCRYPTED,
                ..
            }
        ));
        assert!(matches!(
            open_error(FPDF_ERR_FILE),
            ContractError {
                exit_code: EXIT_CORRUPT,
                ..
            }
        ));
        assert!(matches!(
            open_error(FPDF_ERR_FORMAT),
            ContractError {
                exit_code: EXIT_CORRUPT,
                ..
            }
        ));
    }
}
