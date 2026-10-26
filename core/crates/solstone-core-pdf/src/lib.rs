// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The native implementation of the frozen `sol-pdf/1` subprocess contract.

use std::collections::BTreeSet;
use std::ffi::{CString, c_char, c_int, c_ulong, c_void};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
#[cfg(windows)]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// Byte bound shared with the subprocess stdout reader. The typed payload is
/// counted as compact JSON while it is built, including escapes and structure.
/// This is not a page, metadata, or bitmap budget. A checked UTF-16 conversion
/// buffer may be larger than this bound when the encoded text still fits.
pub const RESPONSE_BYTE_BOUND: usize = 8 * 1024 * 1024;

fn pdfium_gate() -> &'static Mutex<()> {
    static GATE: Mutex<()> = Mutex::new(());
    &GATE
}

fn pdfium_gate_waiters() -> &'static AtomicUsize {
    static WAITERS: AtomicUsize = AtomicUsize::new(0);
    &WAITERS
}

fn with_pdfium_serialization<T>(body: impl FnOnce() -> T) -> T {
    pdfium_gate_waiters().fetch_add(1, Ordering::AcqRel);
    let _guard = pdfium_gate()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pdfium_gate_waiters().fetch_sub(1, Ordering::AcqRel);
    body()
}

/// Whether the next native entry or page installation may start.
///
/// A caller can record cancellation inside `admit` and still return true for
/// that observation. The second read is the decision. A native call that has
/// already been entered is not asked again, and this does not abandon its
/// installation.
fn admission_holds(admit: &dyn Fn() -> bool) -> bool {
    admit() && admit()
}

impl Pdfium {
    fn load() -> Result<Self, String> {
        let library_path = pdfium_library_path()?;
        Self::load_at(&library_path)
    }

    fn load_at(library_path: &Path) -> Result<Self, String> {
        let library = open_pdfium_library(library_path)?;
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

#[derive(Clone, Serialize)]
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

#[derive(Clone, Serialize)]
struct RenderPayload {
    dpi: i32,
    dir: String,
}

#[derive(Clone, Serialize)]
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
    with_pdfium_serialization(|| {
        let pdfium = Pdfium::load().map_err(ContractError::internal)?;
        pdfium.initialize();
        let result = extract_document(
            &pdfium,
            &arguments,
            &|| true,
            &mut PayloadAccount::unlimited(),
        );
        pdfium.destroy();
        match result {
            Ok(payload) => Ok(payload),
            Err(ExtractFailure::Contract(error)) => Err(error),
            Err(ExtractFailure::Cancelled) => Err(ContractError::internal("pdfium call cancelled")),
            Err(ExtractFailure::ResponseTooLarge) => {
                Err(ContractError::internal("response exceeds 8 MiB"))
            }
        }
    })
}

struct PayloadAccount {
    used: usize,
    enforced: bool,
}

impl PayloadAccount {
    fn unlimited() -> Self {
        Self {
            used: 0,
            enforced: false,
        }
    }

    fn bounded() -> Self {
        Self {
            used: 0,
            enforced: true,
        }
    }

    fn reserve(&mut self, bytes: usize) -> Result<(), ()> {
        if !self.enforced {
            return Ok(());
        }
        let next = self.used.checked_add(bytes).ok_or(())?;
        if next > RESPONSE_BYTE_BOUND {
            return Err(());
        }
        self.used = next;
        Ok(())
    }

    /// Charge the compact document with an empty page list. Later pages and
    /// warnings add only their own encoded bytes. Metadata, the engine, the
    /// file hash, and a requested render directory are already in this shell,
    /// so page admission cannot fill the bound and then overflow on that tail.
    fn begin_document(&mut self, shell: &SuccessPayload) -> Result<(), ExtractFailure> {
        if !self.enforced {
            return Ok(());
        }
        debug_assert!(shell.pages.is_empty());
        debug_assert!(shell.warnings.is_empty());
        let len = json_len(shell)?;
        if len > RESPONSE_BYTE_BOUND {
            return Err(ExtractFailure::ResponseTooLarge);
        }
        self.used = len;
        Ok(())
    }

    fn admit_page(
        &mut self,
        page: &PageEntry,
        new_warnings: &[String],
        admitted_pages: usize,
        warning_count: usize,
    ) -> Result<(), ExtractFailure> {
        if !self.enforced {
            return Ok(());
        }
        let extra = encoded_growth(page, new_warnings, admitted_pages, warning_count)?;
        self.reserve(extra)
            .map_err(|()| ExtractFailure::ResponseTooLarge)
    }

    fn rewrite_fits(
        &self,
        before: &PageEntry,
        after: &PageEntry,
        new_warnings: &[String],
        warning_count: usize,
    ) -> Result<bool, ExtractFailure> {
        if !self.enforced {
            return Ok(true);
        }
        Ok(self.rewritten_len(before, after, new_warnings, warning_count)? <= RESPONSE_BYTE_BOUND)
    }

    fn rewrite(
        &mut self,
        before: &PageEntry,
        after: &PageEntry,
        new_warnings: &[String],
        warning_count: usize,
    ) -> Result<(), ExtractFailure> {
        if !self.enforced {
            return Ok(());
        }
        let next = self.rewritten_len(before, after, new_warnings, warning_count)?;
        if next > RESPONSE_BYTE_BOUND {
            return Err(ExtractFailure::ResponseTooLarge);
        }
        self.used = next;
        Ok(())
    }

    fn rewritten_len(
        &self,
        before: &PageEntry,
        after: &PageEntry,
        new_warnings: &[String],
        warning_count: usize,
    ) -> Result<usize, ExtractFailure> {
        let before_len = json_len(before)?;
        let after_len = json_len(after)?;
        let mut next = if after_len >= before_len {
            self.used
                .checked_add(after_len - before_len)
                .ok_or(ExtractFailure::ResponseTooLarge)?
        } else {
            self.used - (before_len - after_len)
        };
        next = next
            .checked_add(warning_growth(new_warnings, warning_count)?)
            .ok_or(ExtractFailure::ResponseTooLarge)?;
        Ok(next)
    }
}

#[derive(Debug)]
enum ExtractFailure {
    Contract(ContractError),
    Cancelled,
    ResponseTooLarge,
}

fn json_len<T: Serialize>(value: &T) -> Result<usize, ExtractFailure> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|error| ExtractFailure::Contract(ContractError::internal(error.to_string())))
}

fn encoded_growth(
    page: &PageEntry,
    new_warnings: &[String],
    admitted_pages: usize,
    warning_count: usize,
) -> Result<usize, ExtractFailure> {
    let mut extra = json_len(page)?;
    if admitted_pages > 0 {
        extra = extra
            .checked_add(1)
            .ok_or(ExtractFailure::ResponseTooLarge)?;
    }
    extra
        .checked_add(warning_growth(new_warnings, warning_count)?)
        .ok_or(ExtractFailure::ResponseTooLarge)
}

fn warning_growth(warnings: &[String], warning_count: usize) -> Result<usize, ExtractFailure> {
    let mut extra = 0usize;
    for (index, warning) in warnings.iter().enumerate() {
        extra = extra
            .checked_add(json_len(warning)?)
            .ok_or(ExtractFailure::ResponseTooLarge)?;
        if warning_count + index > 0 {
            extra = extra
                .checked_add(1)
                .ok_or(ExtractFailure::ResponseTooLarge)?;
        }
    }
    Ok(extra)
}

/// Cells for a PDFium UTF-16 buffer, including the trailing NUL.
///
/// `max_text_bytes` is the most text this page can still contribute, counted as
/// one byte per UTF-16 code unit. The buffer may be larger than
/// [`RESPONSE_BYTE_BOUND`] when that encoded text still fits. Refuses before
/// allocation on overflow and when `count` cannot fit in `max_text_bytes`.
fn utf16_buffer_cells(count: i32, max_text_bytes: usize) -> Result<usize, ()> {
    let count = usize::try_from(count).map_err(|_| ())?;
    if count > max_text_bytes {
        return Err(());
    }
    let cells = count.checked_add(1).ok_or(())?;
    if cells.checked_mul(std::mem::size_of::<u16>()).is_none() {
        return Err(());
    }
    Ok(cells)
}

/// Remaining stored-text bytes for one page.
///
/// When the payload is bounded and this page stores text, the empty-text page
/// shell is already charged, so ASCII of this length still fits the encoded
/// response. Unlimited extraction and Inspect keep checked conversion only:
/// overflow still refuses the buffer, and the response bound is not applied
/// as a page-text cap. A printed subprocess response keeps the encoded bound
/// in its stdout reader.
fn text_byte_limit(
    account: &PayloadAccount,
    admitted_pages: usize,
    include_text: bool,
    index: usize,
    width_pt: f64,
    height_pt: f64,
) -> usize {
    if !account.enforced || !include_text {
        return usize::MAX;
    }
    let provisional = PageEntry {
        index: index + 1,
        chars: 0,
        width_pt,
        height_pt,
        image_area_fraction: 0.0,
        rendered: None,
        error: None,
        text: Some(String::new()),
    };
    let Ok(shell) = json_len(&provisional) else {
        return 0;
    };
    let comma = usize::from(admitted_pages > 0);
    let projected = account.used.saturating_add(comma).saturating_add(shell);
    RESPONSE_BYTE_BOUND.saturating_sub(projected)
}

fn extract_document(
    pdfium: &Pdfium,
    arguments: &Arguments,
    admit: &dyn Fn() -> bool,
    account: &mut PayloadAccount,
) -> Result<SuccessPayload, ExtractFailure> {
    let path = CString::new(arguments.pdf_path.to_string_lossy().as_bytes()).map_err(|_| {
        ExtractFailure::Contract(ContractError::usage("PDF path contains a NUL byte"))
    })?;
    let password = arguments
        .password
        .as_deref()
        .map(CString::new)
        .transpose()
        .map_err(|_| {
            ExtractFailure::Contract(ContractError::usage("password contains a NUL byte"))
        })?;
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
        return Err(ExtractFailure::Contract(open_error(error)));
    }
    let document_guard = DocumentGuard { pdfium, document };
    let page_count = unsafe { (pdfium.get_page_count)(document) };
    if page_count < 0 {
        return Err(ExtractFailure::Contract(ContractError::internal(
            "PDFium returned a negative page count",
        )));
    }
    let page_count = page_count as usize;
    let include_text = arguments.command == CommandKind::Extract;
    let encrypted =
        unsafe { (pdfium.get_security_handler_revision)(document_guard.document) } != -1;
    let engine = "pdfium 151.0.7920.0 (native)".to_owned();
    let sha256 = sha256_file(&arguments.pdf_path)
        .map_err(|error| ExtractFailure::Contract(ContractError::internal(error.to_string())))?;
    let metadata = document_metadata(pdfium, document_guard.document);
    let render = arguments.render.requested.then(|| {
        let directory = arguments
            .render
            .render_dir
            .as_ref()
            .expect("validated render directory");
        RenderPayload {
            dpi: arguments.render.dpi,
            dir: directory.display().to_string(),
        }
    });
    let mut payload = SuccessPayload {
        schema: SCHEMA,
        engine,
        sha256,
        page_count,
        encrypted,
        metadata,
        pages: Vec::new(),
        render,
        warnings: Vec::new(),
    };
    account.begin_document(&payload)?;
    for index in 0..page_count {
        if !admission_holds(admit) {
            return Err(ExtractFailure::Cancelled);
        }
        let (page, page_warnings) = extract_page(
            pdfium,
            document_guard.document,
            index,
            include_text,
            account,
            payload.pages.len(),
        )?;
        account.admit_page(
            &page,
            &page_warnings,
            payload.pages.len(),
            payload.warnings.len(),
        )?;
        payload.pages.push(page);
        payload.warnings.extend(page_warnings);
    }
    if arguments.render.requested {
        if arguments
            .render
            .pages
            .last()
            .is_some_and(|page| *page > page_count)
        {
            return Err(ExtractFailure::Contract(ContractError::usage(
                "--render-pages contains a page beyond the document",
            )));
        }
        let dpi = arguments.render.dpi;
        render_selected_pages(
            &mut payload.pages,
            &arguments.render,
            admit,
            &mut payload.warnings,
            account,
            |index, page, output| {
                render_page_png(
                    pdfium,
                    document_guard.document,
                    index,
                    page.width_pt,
                    page.height_pt,
                    dpi,
                    output,
                )
            },
        )?;
    }
    Ok(payload)
}

fn document_metadata(pdfium: &Pdfium, document: FpdfDocument) -> Metadata {
    Metadata {
        title: metadata_text(pdfium, document, "Title"),
        author: metadata_text(pdfium, document, "Author"),
        creation_date: metadata_text(pdfium, document, "CreationDate")
            .and_then(|value| parse_pdf_date(&value)),
        mod_date: metadata_text(pdfium, document, "ModDate")
            .and_then(|value| parse_pdf_date(&value)),
        producer: metadata_text(pdfium, document, "Producer"),
    }
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
    account: &PayloadAccount,
    admitted_pages: usize,
) -> Result<(PageEntry, Vec<String>), ExtractFailure> {
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
    let mut page_warnings = Vec::new();
    match extract_page_values(
        pdfium,
        document,
        index,
        account,
        admitted_pages,
        include_text,
    ) {
        Ok((width_pt, height_pt, text, image_area_fraction)) => {
            entry.width_pt = width_pt;
            entry.height_pt = height_pt;
            entry.chars = non_whitespace_chars(&text);
            entry.image_area_fraction = image_area_fraction;
            if include_text {
                entry.text = Some(text);
            }
        }
        Err(PageValueError::TooLarge) => return Err(ExtractFailure::ResponseTooLarge),
        Err(PageValueError::Page(error)) => {
            let message = format!("page {}: page extraction failed: {error}", index + 1);
            entry.error = Some(message.clone());
            page_warnings.push(message);
        }
    }
    Ok((entry, page_warnings))
}

enum PageValueError {
    Page(String),
    TooLarge,
}

fn extract_page_values(
    pdfium: &Pdfium,
    document: FpdfDocument,
    index: usize,
    account: &PayloadAccount,
    admitted_pages: usize,
    include_text: bool,
) -> Result<(f64, f64, String, f64), PageValueError> {
    let page = unsafe { (pdfium.load_page)(document, index as c_int) };
    if page.is_null() {
        return Err(PageValueError::Page(
            "PDFium could not load page".to_owned(),
        ));
    }
    let page = PageGuard { pdfium, page };
    let width_pt = unsafe { (pdfium.get_page_width_f)(page.page) } as f64;
    let height_pt = unsafe { (pdfium.get_page_height_f)(page.page) } as f64;
    let text_page = unsafe { (pdfium.text_load_page)(page.page) };
    if text_page.is_null() {
        return Err(PageValueError::Page(
            "PDFium could not load text page".to_owned(),
        ));
    }
    let text_page = TextPageGuard { pdfium, text_page };
    let count = unsafe { (pdfium.text_count_chars)(text_page.text_page) };
    if count < 0 {
        return Err(PageValueError::Page(
            "PDFium returned a negative text character count".to_owned(),
        ));
    }
    let max_text_bytes = text_byte_limit(
        account,
        admitted_pages,
        include_text,
        index,
        width_pt,
        height_pt,
    );
    let cells = utf16_buffer_cells(count, max_text_bytes).map_err(|()| PageValueError::TooLarge)?;
    let mut utf16 = vec![0_u16; cells];
    let written =
        unsafe { (pdfium.text_get_text)(text_page.text_page, 0, count, utf16.as_mut_ptr()) };
    if written < 0 {
        return Err(PageValueError::Page(
            "PDFium failed to extract text".to_owned(),
        ));
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
    pages: &mut [PageEntry],
    request: &RenderRequest,
    admit: &dyn Fn() -> bool,
    warnings: &mut Vec<String>,
    account: &mut PayloadAccount,
    mut render_one: impl FnMut(usize, &PageEntry, &Path) -> Result<(), RenderFailure>,
) -> Result<(), ExtractFailure> {
    // After text extraction. A cancellation that won before this installation
    // creates nothing. One that wins during a page lets that page finish and
    // starts no later page. A true admission that records Stop is read again
    // before the directory and before each page starts.
    if !admission_holds(admit) {
        return Err(ExtractFailure::Cancelled);
    }
    let selected = selected_pages(pages, request);
    let directory = request
        .render_dir
        .as_ref()
        .expect("validated render directory");
    fs::create_dir_all(directory)
        .map_err(|error| ExtractFailure::Contract(ContractError::render_io(error.to_string())))?;
    for page_number in selected {
        if !admission_holds(admit) {
            return Err(ExtractFailure::Cancelled);
        }
        let page = &mut pages[page_number - 1];
        if page.error.is_some() {
            continue;
        }
        let rendered = format!("page-{page_number:04}.png");
        let output = directory.join(&rendered);
        let installed = {
            let mut next = page.clone();
            next.rendered = Some(rendered);
            next
        };
        if !account.rewrite_fits(page, &installed, &[], warnings.len())? {
            return Err(ExtractFailure::ResponseTooLarge);
        }
        match render_one(page_number - 1, page, &output) {
            Ok(()) => {
                account.rewrite(page, &installed, &[], warnings.len())?;
                *page = installed;
            }
            Err(RenderFailure::Io(error)) => {
                return Err(ExtractFailure::Contract(ContractError::render_io(
                    error.to_string(),
                )));
            }
            Err(RenderFailure::Pdfium(error)) => {
                let message = format!("page {page_number}: page render failed: {error}");
                let mut failed = page.clone();
                failed.error = Some(message.clone());
                failed.chars = 0;
                failed.image_area_fraction = 0.0;
                failed.rendered = None;
                if failed.text.is_some() {
                    failed.text = Some(String::new());
                }
                account.rewrite(
                    page,
                    &failed,
                    std::slice::from_ref(&message),
                    warnings.len(),
                )?;
                *page = failed;
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
    let min_stride = width_i32
        .checked_mul(3)
        .ok_or_else(|| RenderFailure::Pdfium("render stride exceeds PDFium limits".to_owned()))?;
    if stride < min_stride || buffer.is_null() {
        return Err(RenderFailure::Pdfium(
            "PDFium returned an invalid render bitmap".to_owned(),
        ));
    }
    let stride_len = usize::try_from(stride)
        .map_err(|_| RenderFailure::Pdfium("render stride exceeds PDFium limits".to_owned()))?;
    let height_len = usize::try_from(height)
        .map_err(|_| RenderFailure::Pdfium("render height exceeds PDFium limits".to_owned()))?;
    let source_len = stride_len.checked_mul(height_len).ok_or_else(|| {
        RenderFailure::Pdfium("render bitmap size exceeds PDFium limits".to_owned())
    })?;
    let rgb_len = usize::try_from(width)
        .ok()
        .and_then(|width_len| width_len.checked_mul(height_len))
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| {
            RenderFailure::Pdfium("render bitmap size exceeds PDFium limits".to_owned())
        })?;
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|width_len| width_len.checked_mul(3))
        .ok_or_else(|| {
            RenderFailure::Pdfium("render bitmap size exceeds PDFium limits".to_owned())
        })?;
    let source = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), source_len) };
    let mut rgb = Vec::with_capacity(rgb_len);
    for row in source.chunks_exact(stride_len).take(height_len) {
        for pixel in row[..row_bytes].chunks_exact(3) {
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
    let len = usize::try_from(bytes).ok()?;
    let mut buffer = vec![0_u8; len];
    let written = unsafe {
        (pdfium.get_meta_text)(document, tag.as_ptr(), buffer.as_mut_ptr().cast(), bytes)
    };
    let written_len = usize::try_from(written).ok()?;
    if written_len < 2 {
        return None;
    }
    let values = buffer[..written_len - 2]
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
    // Heap-allocated: a 1 MiB fixed-size array here previously lived on the
    // stack, which alone is close to (and on some call paths apparently
    // exceeds) the default 1 MiB Windows main-thread stack reserve before any
    // other frame is considered. See commit history on this file for the
    // superseded main()-side worker-thread workaround this replaces.
    let mut buffer = vec![0_u8; 1024 * 1024];
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
    pdfium_library_path_at(&executable)
}

fn pdfium_library_path_at(executable: &Path) -> Result<PathBuf, String> {
    let filename = if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    };
    executable
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "executable path has no installation prefix".to_owned())
        .map(|directory| {
            directory
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

/// Typed in-process PDF request. The caller passes the admitted PDFium
/// library path to [`execute_in_process`]; this value does not locate it.
#[derive(Clone, Debug)]
pub struct InProcessRequest {
    pub command: InProcessCommand,
    pub source: PathBuf,
    pub password: Option<String>,
    pub render_pages: BTreeSet<usize>,
    pub render_dir: Option<PathBuf>,
    pub dpi: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InProcessCommand {
    Inspect,
    Extract,
}

#[derive(Clone, Debug)]
pub struct InProcessPage {
    pub index: usize,
    pub chars: usize,
    pub width_pt: f64,
    pub height_pt: f64,
    pub image_area_fraction: f64,
    pub rendered: Option<String>,
    pub error: Option<String>,
    pub text: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InProcessDocument {
    pub schema: String,
    pub engine: String,
    pub sha256: String,
    pub page_count: usize,
    pub encrypted: bool,
    pub title: Option<String>,
    pub author: Option<String>,
    pub creation_date: Option<String>,
    pub mod_date: Option<String>,
    pub producer: Option<String>,
    pub pages: Vec<InProcessPage>,
    pub render_dpi: Option<i32>,
    pub render_dir: Option<String>,
    pub warnings: Vec<String>,
}

/// Failure of an in-process extraction. `Cancelled` is only returned after the
/// PDFium call that was already running has returned. It is not a missing
/// library, a corrupt file, a protocol failure, or a timeout.
#[derive(Debug)]
pub enum InProcessError {
    Library(String),
    Rejected {
        error: String,
        detail: Option<String>,
    },
    Cancelled,
    ResponseTooLarge,
}

/// Extract one PDF inside this process.
///
/// `library` is the admitted PDFium path. This function does not call
/// [`pdfium_library_path`], does not write stdout, and does not call
/// `setrlimit`. It does not enforce the helper's 2,048 MiB address-space
/// budget, 60 CPU seconds, or the 90-second caller timeout. The extractor
/// still has no page-count, metadata-allocation, or bitmap-byte budget. The
/// typed payload is counted as compact JSON, including escapes and structure,
/// against [`RESPONSE_BYTE_BOUND`] before each page is retained. A checked
/// UTF-16 conversion buffer may be larger than that bound when the encoded
/// text still fits.
///
/// PDFium init, every PDFium call, and destroy share one process-wide lock
/// with the subprocess helper. `admit` is read again after that lock is
/// acquired and before any library load, init, or document open. It is read
/// before each text page and before each rendered page is installed. A true
/// read that records cancellation is not enough to enter: the callback is
/// read once more before that native entry or installation. A call that is
/// already inside PDFium stays owned until it returns, and a false `admit`
/// then starts no later page or render. This function cannot kill a wedged
/// native frame or isolate a crash inside the calling process.
pub fn execute_in_process(
    library: &Path,
    request: &InProcessRequest,
    admit: &dyn Fn() -> bool,
) -> Result<InProcessDocument, InProcessError> {
    with_pdfium_serialization(|| {
        if !admission_holds(admit) {
            return Err(InProcessError::Cancelled);
        }
        let pdfium = Pdfium::load_at(library).map_err(InProcessError::Library)?;
        pdfium.initialize();
        let arguments = match in_process_arguments(request) {
            Ok(arguments) => arguments,
            Err(error) => {
                pdfium.destroy();
                return Err(error);
            }
        };
        let mut account = PayloadAccount::bounded();
        let result = extract_document(&pdfium, &arguments, admit, &mut account);
        pdfium.destroy();
        match result {
            Ok(payload) => Ok(in_process_document(payload)),
            Err(ExtractFailure::Cancelled) => Err(InProcessError::Cancelled),
            Err(ExtractFailure::ResponseTooLarge) => Err(InProcessError::ResponseTooLarge),
            Err(ExtractFailure::Contract(error)) => Err(InProcessError::Rejected {
                error: error.error.to_owned(),
                detail: error.detail,
            }),
        }
    })
}

fn in_process_arguments(request: &InProcessRequest) -> Result<Arguments, InProcessError> {
    if !request.source.is_file() {
        return Err(InProcessError::Rejected {
            error: "usage".to_owned(),
            detail: Some(format!("PDF not found: {}", request.source.display())),
        });
    }
    if request.dpi <= 0 {
        return Err(InProcessError::Rejected {
            error: "usage".to_owned(),
            detail: Some("--dpi must be positive".to_owned()),
        });
    }
    let requested = request.render_dir.is_some() || !request.render_pages.is_empty();
    if requested && request.render_dir.is_none() {
        return Err(InProcessError::Rejected {
            error: "usage".to_owned(),
            detail: Some("--render-dir is required when render selectors are used".to_owned()),
        });
    }
    Ok(Arguments {
        command: match request.command {
            InProcessCommand::Inspect => CommandKind::Inspect,
            InProcessCommand::Extract => CommandKind::Extract,
        },
        pdf_path: request.source.clone(),
        password: request.password.clone(),
        render: RenderRequest {
            requested,
            render_dir: request.render_dir.clone(),
            dpi: request.dpi,
            below_chars: None,
            above_image_fraction: None,
            pages: request.render_pages.clone(),
        },
    })
}

fn in_process_document(payload: SuccessPayload) -> InProcessDocument {
    InProcessDocument {
        schema: payload.schema.to_owned(),
        engine: payload.engine,
        sha256: payload.sha256,
        page_count: payload.page_count,
        encrypted: payload.encrypted,
        title: payload.metadata.title,
        author: payload.metadata.author,
        creation_date: payload.metadata.creation_date,
        mod_date: payload.metadata.mod_date,
        producer: payload.metadata.producer,
        pages: payload
            .pages
            .into_iter()
            .map(|page| InProcessPage {
                index: page.index,
                chars: page.chars,
                width_pt: page.width_pt,
                height_pt: page.height_pt,
                image_area_fraction: page.image_area_fraction,
                rendered: page.rendered,
                error: page.error,
                text: page.text,
            })
            .collect(),
        render_dpi: payload.render.as_ref().map(|render| render.dpi),
        render_dir: payload.render.map(|render| render.dir),
        warnings: payload.warnings,
    }
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
    #[test]
    fn utf16_buffer_allows_response_sized_ascii_and_refuses_overflow() {
        assert_eq!(super::utf16_buffer_cells(0, 0).unwrap(), 1);
        assert!(super::utf16_buffer_cells(-1, super::RESPONSE_BYTE_BOUND).is_err());
        assert!(super::utf16_buffer_cells(i32::MAX, super::RESPONSE_BYTE_BOUND).is_err());
        let half = i32::try_from(super::RESPONSE_BYTE_BOUND / 2).unwrap();
        let half_cells = super::utf16_buffer_cells(half, super::RESPONSE_BYTE_BOUND).unwrap();
        assert_eq!(half_cells, super::RESPONSE_BYTE_BOUND / 2 + 1);
        assert!(half_cells * std::mem::size_of::<u16>() > super::RESPONSE_BYTE_BOUND);
        let full = i32::try_from(super::RESPONSE_BYTE_BOUND).unwrap();
        assert_eq!(
            super::utf16_buffer_cells(full, super::RESPONSE_BYTE_BOUND).unwrap(),
            super::RESPONSE_BYTE_BOUND + 1
        );
        assert!(super::utf16_buffer_cells(full, super::RESPONSE_BYTE_BOUND - 1).is_err());
    }

    #[test]
    fn unlimited_and_inspect_keep_checked_conversion_without_a_page_text_cap() {
        let page = (0, 612.0, 792.0);
        let unlimited = super::text_byte_limit(
            &super::PayloadAccount::unlimited(),
            0,
            true,
            page.0,
            page.1,
            page.2,
        );
        let inspect = super::text_byte_limit(
            &super::PayloadAccount::bounded(),
            0,
            false,
            page.0,
            page.1,
            page.2,
        );
        assert_eq!(unlimited, usize::MAX);
        assert_eq!(inspect, usize::MAX);
        let included = super::text_byte_limit(
            &super::PayloadAccount::bounded(),
            0,
            true,
            page.0,
            page.1,
            page.2,
        );
        assert!(included < super::RESPONSE_BYTE_BOUND);
        let count = i32::try_from(super::RESPONSE_BYTE_BOUND).unwrap();
        assert!(super::utf16_buffer_cells(count, unlimited).is_ok());
        assert!(super::utf16_buffer_cells(count, inspect).is_ok());
        assert!(super::utf16_buffer_cells(count, included).is_err());
        assert!(super::utf16_buffer_cells(-1, unlimited).is_err());
    }

    fn empty_shell() -> super::SuccessPayload {
        super::SuccessPayload {
            schema: super::SCHEMA,
            engine: "pdfium 151.0.7920.0 (native)".to_owned(),
            sha256: "ab".repeat(32),
            page_count: 1,
            encrypted: false,
            metadata: super::Metadata {
                title: None,
                author: None,
                creation_date: None,
                mod_date: None,
                producer: None,
            },
            pages: Vec::new(),
            render: None,
            warnings: Vec::new(),
        }
    }

    fn page_with_text(index: usize, text: Option<String>) -> super::PageEntry {
        super::PageEntry {
            index,
            chars: text
                .as_deref()
                .map(super::non_whitespace_chars)
                .unwrap_or(0),
            width_pt: 612.0,
            height_pt: 792.0,
            image_area_fraction: 0.0,
            rendered: None,
            error: None,
            text,
        }
    }

    #[test]
    fn empty_pages_stop_on_the_encoded_bound_without_growing() {
        let mut account = super::PayloadAccount::bounded();
        account.begin_document(&empty_shell()).unwrap();
        let mut pages = Vec::new();
        loop {
            let page = page_with_text(pages.len() + 1, Some(String::new()));
            let before = account.used;
            if account.admit_page(&page, &[], pages.len(), 0).is_err() {
                assert_eq!(account.used, before);
                assert!(!pages.is_empty());
                break;
            }
            pages.push(page);
        }
        let mut fitted = empty_shell();
        fitted.pages = pages;
        let encoded = serde_json::to_vec(&fitted).unwrap();
        assert!(encoded.len() <= super::RESPONSE_BYTE_BOUND);
        assert_eq!(encoded.len(), account.used);
        let mut overflow = fitted.clone();
        overflow.pages.push(page_with_text(
            overflow.pages.len() + 1,
            Some(String::new()),
        ));
        assert!(serde_json::to_vec(&overflow).unwrap().len() > super::RESPONSE_BYTE_BOUND);
    }

    fn ascii_page_of_len(index: usize, target: usize) -> super::PageEntry {
        let empty_len = serde_json::to_vec(&page_with_text(index, Some(String::new())))
            .unwrap()
            .len();
        assert!(target >= empty_len);
        let mut text_len = target - empty_len;
        for _ in 0..8 {
            let page = page_with_text(index, Some("a".repeat(text_len)));
            let actual = serde_json::to_vec(&page).unwrap().len();
            if actual == target {
                return page;
            }
            if actual > target {
                text_len -= actual - target;
            } else {
                text_len += target - actual;
            }
        }
        panic!("could not fit a page to {target} bytes");
    }

    #[test]
    fn escaped_text_is_refused_when_only_the_raw_length_would_fit() {
        let quotes = "\"".repeat(32);
        let quoted = page_with_text(2, Some(quotes));
        let plain = page_with_text(2, Some("a".repeat(32)));
        let quoted_len = serde_json::to_vec(&quoted).unwrap().len();
        let plain_len = serde_json::to_vec(&plain).unwrap().len();
        assert!(quoted_len > plain_len);

        let mut account = super::PayloadAccount::bounded();
        account.begin_document(&empty_shell()).unwrap();
        let plain_growth = 1 + plain_len;
        let filler = ascii_page_of_len(1, super::RESPONSE_BYTE_BOUND - account.used - plain_growth);
        account.admit_page(&filler, &[], 0, 0).unwrap();
        let parked = account.used;
        assert_eq!(parked, super::RESPONSE_BYTE_BOUND - plain_growth);
        assert!(account.admit_page(&quoted, &[], 1, 0).is_err());
        assert_eq!(account.used, parked);
        account.admit_page(&plain, &[], 1, 0).unwrap();
        assert_eq!(account.used, super::RESPONSE_BYTE_BOUND);
    }

    #[test]
    fn encoded_payload_fits_the_bound_and_refuses_one_more_byte() {
        let mut account = super::PayloadAccount::bounded();
        let shell = empty_shell();
        account.begin_document(&shell).unwrap();
        let fitted = ascii_page_of_len(1, super::RESPONSE_BYTE_BOUND - account.used);
        account.admit_page(&fitted, &[], 0, 0).unwrap();
        assert_eq!(account.used, super::RESPONSE_BYTE_BOUND);
        let mut payload = shell.clone();
        payload.pages = vec![fitted.clone()];
        assert_eq!(
            serde_json::to_vec(&payload).unwrap().len(),
            super::RESPONSE_BYTE_BOUND
        );

        let mut longer = fitted;
        longer.text.as_mut().unwrap().push('a');
        longer.chars += 1;
        assert!(
            serde_json::to_vec(&longer).unwrap().len()
                > serde_json::to_vec(&payload.pages[0]).unwrap().len()
        );
        let mut over = super::PayloadAccount::bounded();
        over.begin_document(&shell).unwrap();
        let before = over.used;
        assert!(over.admit_page(&longer, &[], 0, 0).is_err());
        assert_eq!(over.used, before);
    }

    #[test]
    fn accounted_length_matches_serde() {
        let mut shell = empty_shell();
        shell.metadata.title = Some("a \"title\"\n".to_owned());
        shell.metadata.author = Some("author\\name".to_owned());
        shell.render = Some(super::RenderPayload {
            dpi: 144,
            dir: "renders/line\nquote\"".to_owned(),
        });
        shell.page_count = 2;
        let mut account = super::PayloadAccount::bounded();
        account.begin_document(&shell).unwrap();
        let first = page_with_text(1, Some("plain".to_owned()));
        let mut second = page_with_text(2, Some("say \"hi\"\u{0001}".to_owned()));
        account.admit_page(&first, &[], 0, 0).unwrap();
        let warning = "page 2: page extraction failed: bad".to_owned();
        account
            .admit_page(&second, std::slice::from_ref(&warning), 1, 0)
            .unwrap();
        let installed = {
            let mut next = second.clone();
            next.rendered = Some("page-0002.png".to_owned());
            next
        };
        account.rewrite(&second, &installed, &[], 1).unwrap();
        second = installed;
        shell.pages = vec![first, second];
        shell.warnings = vec![warning];
        assert_eq!(serde_json::to_vec(&shell).unwrap().len(), account.used);
    }

    #[test]
    fn payload_account_rejects_the_next_byte_without_growing_past_the_bound() {
        let mut account = super::PayloadAccount::bounded();
        assert!(account.reserve(super::RESPONSE_BYTE_BOUND).is_ok());
        assert!(account.reserve(1).is_err());
        assert_eq!(account.used, super::RESPONSE_BYTE_BOUND);
        let mut open = super::PayloadAccount::unlimited();
        assert!(open.reserve(super::RESPONSE_BYTE_BOUND + 1).is_ok());
    }

    fn pdfium_gate_test_hold() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn pdfium_init_calls_and_destroy_share_one_gate() {
        let _hold = pdfium_gate_test_hold();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            super::with_pdfium_serialization(|| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
        });
        entered_rx.recv().unwrap();
        assert!(super::pdfium_gate().try_lock().is_err());
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(super::pdfium_gate().try_lock().is_ok());
    }

    #[test]
    fn installed_library_path_has_platform_name_and_no_parent_traversal() {
        let prefix = std::env::temp_dir().join("pdf-prefix");
        let library = super::pdfium_library_path_at(&prefix.join("bin/pdf-helper")).unwrap();
        let name = if cfg!(windows) {
            "pdfium.dll"
        } else if cfg!(target_os = "macos") {
            "libpdfium.dylib"
        } else {
            "libpdfium.so"
        };
        assert_eq!(library, prefix.join("lib/solstone-core-pdf").join(name));
        assert!(
            !library
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        );
    }

    use super::*;

    fn two_render_pages() -> Vec<PageEntry> {
        vec![
            PageEntry {
                index: 1,
                chars: 1,
                width_pt: 72.0,
                height_pt: 72.0,
                image_area_fraction: 0.0,
                rendered: None,
                error: None,
                text: None,
            },
            PageEntry {
                index: 2,
                chars: 1,
                width_pt: 72.0,
                height_pt: 72.0,
                image_area_fraction: 0.0,
                rendered: None,
                error: None,
                text: None,
            },
        ]
    }

    fn render_request(dir: &Path) -> RenderRequest {
        RenderRequest {
            requested: true,
            render_dir: Some(dir.to_path_buf()),
            dpi: 72,
            below_chars: None,
            above_image_fraction: None,
            pages: BTreeSet::from([1, 2]),
        }
    }

    #[test]
    fn cancelled_while_queued_does_not_enter_pdfium() {
        let _hold = pdfium_gate_test_hold();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            with_pdfium_serialization(|| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
        });
        entered_rx.recv().unwrap();
        assert!(pdfium_gate().try_lock().is_err());
        let admit = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let admit_worker = std::sync::Arc::clone(&admit);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = execute_in_process(
                Path::new("missing-pdfium"),
                &InProcessRequest {
                    command: InProcessCommand::Extract,
                    source: PathBuf::from("missing.pdf"),
                    password: None,
                    render_pages: BTreeSet::new(),
                    render_dir: None,
                    dpi: 72,
                },
                &|| admit_worker.load(std::sync::atomic::Ordering::Acquire),
            );
            done_tx.send(result).unwrap();
        });
        while pdfium_gate_waiters().load(std::sync::atomic::Ordering::Acquire) == 0 {
            assert!(
                !worker.is_finished(),
                "worker left the gate before it was observable as queued"
            );
            std::hint::spin_loop();
        }
        admit.store(false, std::sync::atomic::Ordering::Release);
        assert_eq!(
            done_rx.try_recv().unwrap_err(),
            std::sync::mpsc::TryRecvError::Empty
        );
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        let result = done_rx.recv().unwrap();
        worker.join().unwrap();
        assert!(matches!(result, Err(InProcessError::Cancelled)));
    }

    #[test]
    fn cancelled_after_text_before_render_installs_nothing() {
        let dir = std::env::temp_dir().join("solstone-pdf-render-before");
        let _ = fs::remove_dir_all(&dir);
        let mut pages = two_render_pages();
        let mut warnings = Vec::new();
        let mut calls = 0;
        let mut account = PayloadAccount::unlimited();
        let result = render_selected_pages(
            &mut pages,
            &render_request(&dir),
            &|| false,
            &mut warnings,
            &mut account,
            |_, _, _| {
                calls += 1;
                Ok(())
            },
        );
        assert!(matches!(result, Err(ExtractFailure::Cancelled)));
        assert_eq!(calls, 0);
        assert!(pages.iter().all(|page| page.rendered.is_none()));
        assert!(!dir.exists());
    }

    #[test]
    fn one_true_admission_sets_stop_and_installs_no_page() {
        let dir = std::env::temp_dir().join("solstone-pdf-render-one-admission");
        let _ = fs::remove_dir_all(&dir);
        let stop = std::sync::atomic::AtomicBool::new(false);
        let mut pages = two_render_pages();
        let mut warnings = Vec::new();
        let mut calls = 0;
        let mut account = PayloadAccount::unlimited();
        let result = render_selected_pages(
            &mut pages,
            &render_request(&dir),
            &|| !stop.swap(true, std::sync::atomic::Ordering::AcqRel),
            &mut warnings,
            &mut account,
            |_, _, output| {
                calls += 1;
                fs::write(output, b"png").expect("write render");
                Ok(())
            },
        );
        assert!(matches!(result, Err(ExtractFailure::Cancelled)));
        assert_eq!(calls, 0);
        assert!(pages.iter().all(|page| page.rendered.is_none()));
        assert!(!dir.join("page-0001.png").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn page_admission_that_records_stop_and_returns_true_installs_nothing() {
        let dir = std::env::temp_dir().join("solstone-pdf-render-stale-true");
        let _ = fs::remove_dir_all(&dir);
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let mut pages = two_render_pages();
        let mut warnings = Vec::new();
        let mut calls = 0;
        let mut account = PayloadAccount::unlimited();
        let result = render_selected_pages(
            &mut pages,
            &render_request(&dir),
            &|| {
                // The directory admission is two reads. The next read is the
                // page admission and returns true while Stop is recorded; only
                // the read after that refuses the installation.
                !matches!(reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst), 3..)
            },
            &mut warnings,
            &mut account,
            |_, _, output| {
                calls += 1;
                fs::write(output, b"png").expect("write render");
                Ok(())
            },
        );
        assert!(matches!(result, Err(ExtractFailure::Cancelled)));
        assert_eq!(calls, 0);
        assert!(pages.iter().all(|page| page.rendered.is_none()));
        assert!(!dir.join("page-0001.png").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelled_during_render_finishes_that_page_and_skips_the_next() {
        let dir = std::env::temp_dir().join("solstone-pdf-render-during");
        let _ = fs::remove_dir_all(&dir);
        let allow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let allow_render = std::sync::Arc::clone(&allow);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let request = render_request(&dir);
        let worker = std::thread::spawn(move || {
            let mut pages = two_render_pages();
            let mut warnings = Vec::new();
            let mut calls = 0;
            let mut account = PayloadAccount::unlimited();
            let result = render_selected_pages(
                &mut pages,
                &request,
                &|| allow_render.load(std::sync::atomic::Ordering::Acquire),
                &mut warnings,
                &mut account,
                |index, _, _| {
                    calls += 1;
                    if index == 0 {
                        entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                    }
                    Ok(())
                },
            );
            (result, calls, pages)
        });
        entered_rx.recv().unwrap();
        allow.store(false, std::sync::atomic::Ordering::Release);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || done_tx.send(worker.join().unwrap()).unwrap());
        assert!(matches!(
            done_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
        let (result, calls, pages) = done_rx.recv().unwrap();
        assert!(matches!(result, Err(ExtractFailure::Cancelled)));
        assert_eq!(calls, 1);
        assert!(pages[0].rendered.is_some());
        assert!(pages[1].rendered.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_hashing_fits_a_one_mib_stack() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pdf_corpus/text.pdf");
        let expected = format!(
            "{:x}",
            Sha256::digest(fs::read(&path).expect("read PDF fixture"))
        );
        let actual = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || sha256_file(&path).expect("hash PDF fixture"))
            .expect("spawn worker with Windows-sized stack")
            .join()
            .expect("hashing worker finished");
        assert_eq!(actual, expected);
    }

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
