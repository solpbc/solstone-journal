// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Runtime bindings for the small ced.cpp C API.

use std::ffi::{CStr, CString};
use std::fmt;
use std::num::TryFromIntError;
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr::NonNull;

use libloading::Library;

const ABI_VERSION: c_int = 1;

type AbiVersion = unsafe extern "C" fn() -> c_int;
type Load = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type Free = unsafe extern "C" fn(*mut c_void);
type LastError = unsafe extern "C" fn(*const c_void) -> *const c_char;
type ClassifyPcmJson =
    unsafe extern "C" fn(*mut c_void, *const f32, c_int, c_int, c_int) -> *mut c_char;
type FreeString = unsafe extern "C" fn(*mut c_char);

/// Errors from dynamically loading or invoking ced.cpp.
#[derive(Debug)]
pub enum CedError {
    Library { detail: String },
    Symbol { name: &'static str, detail: String },
    AbiMismatch { actual: i32 },
    ModelPath { detail: String },
    ModelLoad { detail: String },
    SampleCount { detail: String },
    Classify { detail: String },
    ClassifyUtf8 { detail: String },
}

impl fmt::Display for CedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Library { detail }
            | Self::ModelPath { detail }
            | Self::ModelLoad { detail }
            | Self::SampleCount { detail }
            | Self::Classify { detail }
            | Self::ClassifyUtf8 { detail } => formatter.write_str(detail),
            Self::Symbol { name, detail } => {
                write!(formatter, "missing ced symbol {name}: {detail}")
            }
            Self::AbiMismatch { actual } => {
                write!(
                    formatter,
                    "ced C API ABI mismatch: expected {ABI_VERSION}, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for CedError {}

struct Symbols {
    abi_version: AbiVersion,
    load: Load,
    free: Free,
    last_error: LastError,
    classify_pcm_json: ClassifyPcmJson,
    free_string: FreeString,
}

/// A loaded ced.cpp shared library with ABI-checked symbols.
pub struct CedLibrary {
    _library: Library,
    symbols: Symbols,
}

impl CedLibrary {
    /// Open `path`, bind the supported ABI, and reject incompatible engine versions.
    pub fn open(path: &Path) -> Result<Self, CedError> {
        let library = open_library(path)?;
        let symbols = Symbols {
            abi_version: load_symbol(&library, b"ced_capi_abi_version\0", "ced_capi_abi_version")?,
            load: load_symbol(&library, b"ced_capi_load\0", "ced_capi_load")?,
            free: load_symbol(&library, b"ced_capi_free\0", "ced_capi_free")?,
            last_error: load_symbol(&library, b"ced_capi_last_error\0", "ced_capi_last_error")?,
            classify_pcm_json: load_symbol(
                &library,
                b"ced_capi_classify_pcm_json\0",
                "ced_capi_classify_pcm_json",
            )?,
            free_string: load_symbol(&library, b"ced_capi_free_string\0", "ced_capi_free_string")?,
        };
        let actual = unsafe { (symbols.abi_version)() };
        if actual != ABI_VERSION {
            return Err(CedError::AbiMismatch { actual });
        }
        Ok(Self {
            _library: library,
            symbols,
        })
    }

    /// Load a GGUF model into an RAII-owned ced context.
    pub fn load_model(&self, model_path: &Path) -> Result<CedContext<'_>, CedError> {
        let model = CString::new(model_path.to_string_lossy().as_bytes()).map_err(|error| {
            CedError::ModelPath {
                detail: format!("ced model path contains a NUL byte: {error}"),
            }
        })?;
        let raw = unsafe { (self.symbols.load)(model.as_ptr()) };
        let raw = NonNull::new(raw).ok_or_else(|| CedError::ModelLoad {
            detail: self
                .last_error(std::ptr::null())
                .unwrap_or_else(|| "ced_capi_load returned NULL".to_owned()),
        })?;
        Ok(CedContext { library: self, raw })
    }

    fn last_error(&self, context: *const c_void) -> Option<String> {
        let raw = unsafe { (self.symbols.last_error)(context) };
        NonNull::new(raw.cast_mut()).map(|pointer| {
            unsafe { CStr::from_ptr(pointer.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        })
    }
}

fn open_library(path: &Path) -> Result<Library, CedError> {
    #[cfg(windows)]
    {
        use solstone_core_win_dll_load::{
            LoadPolicy, load_dll, restrict_default_dll_directories,
        };

        restrict_default_dll_directories().map_err(|error| CedError::Library {
            detail: format!("could not restrict DLL search before loading CED: {error}"),
        })?;
        load_dll(LoadPolicy::ApplicationDir, path).map_err(|error| CedError::Library {
            detail: format!("could not load ced engine {}: {error}", path.display()),
        })
    }
    #[cfg(not(windows))]
    {
        // SAFETY: non-Windows callers retain the established explicit-path dynamic-load
        // contract. Windows must use `LoadLibraryExW` through the branch above.
        unsafe { Library::new(path) }.map_err(|error| CedError::Library {
            detail: format!("could not load ced engine {}: {error}", path.display()),
        })
    }
}

/// An RAII-owned ced model context.
pub struct CedContext<'library> {
    library: &'library CedLibrary,
    raw: NonNull<c_void>,
}

impl CedContext<'_> {
    /// Classify PCM and copy ced's owned JSON response into Rust memory.
    pub fn classify_pcm_json(
        &self,
        samples: &[f32],
        sample_rate: i32,
        top_k: i32,
    ) -> Result<String, CedError> {
        let n_samples = c_int::try_from(samples.len()).map_err(sample_count_error)?;
        let result = unsafe {
            (self.library.symbols.classify_pcm_json)(
                self.raw.as_ptr(),
                samples.as_ptr(),
                n_samples,
                sample_rate,
                top_k,
            )
        };
        let result = NonNull::new(result).ok_or_else(|| CedError::Classify {
            detail: self
                .library
                .last_error(self.raw.as_ptr())
                .unwrap_or_else(|| "ced_capi_classify_pcm_json returned NULL".to_owned()),
        })?;
        let owned = CedString {
            library: self.library,
            raw: result,
        };
        let text = unsafe { CStr::from_ptr(owned.raw.as_ptr()) }
            .to_str()
            .map(str::to_owned)
            .map_err(|error| CedError::ClassifyUtf8 {
                detail: format!("ced_capi_classify_pcm_json returned invalid UTF-8: {error}"),
            });
        drop(owned);
        text
    }
}

impl Drop for CedContext<'_> {
    fn drop(&mut self) {
        unsafe { (self.library.symbols.free)(self.raw.as_ptr()) };
    }
}

struct CedString<'library> {
    library: &'library CedLibrary,
    raw: NonNull<c_char>,
}

impl Drop for CedString<'_> {
    fn drop(&mut self) {
        unsafe { (self.library.symbols.free_string)(self.raw.as_ptr()) };
    }
}

fn load_symbol<T: Copy>(
    library: &Library,
    name: &'static [u8],
    display_name: &'static str,
) -> Result<T, CedError> {
    let symbol = unsafe { library.get::<T>(name) }.map_err(|error| CedError::Symbol {
        name: display_name,
        detail: error.to_string(),
    })?;
    Ok(*symbol)
}

fn sample_count_error(error: TryFromIntError) -> CedError {
    CedError::SampleCount {
        detail: format!("too many audio samples for ced C API: {error}"),
    }
}
