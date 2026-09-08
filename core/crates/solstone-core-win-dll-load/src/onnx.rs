// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Safe ONNX ABI preflight inside the existing Windows FFI boundary.

use std::ffi::{CStr, c_char, c_void};
use std::path::Path;

use crate::{LoadPolicy, load_dll, restrict_default_dll_directories};

// The stable two-entry OrtApiBase from onnxruntime_c_api.h. The full OrtApi
// table stays opaque here; ort owns its versioned bindings and invocation.
#[repr(C)]
struct OrtApiBase {
    get_api: unsafe extern "system" fn(u32) -> *const c_void,
    get_version_string: unsafe extern "system" fn() -> *const c_char,
}

pub struct LoadedOnnxRuntime {
    // Keep DllMain state and the ABI behind the copied version alive.
    _library: libloading::Library,
    version: String,
    api_address: usize,
}

impl LoadedOnnxRuntime {
    /// Compare the binding held by ort with this exact loaded module/API.
    pub fn matches_api(&self, api: *const c_void) -> bool {
        self.api_address == api.addr()
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Load an already admitted product DLL and require the requested ORT ABI.
/// Package signature/member verification remains the caller's responsibility,
/// as with load_dll. This performs no alternate file discovery.
pub fn load_onnx_runtime(path: &Path, api_level: u32) -> Result<LoadedOnnxRuntime, String> {
    restrict_default_dll_directories().map_err(|error| error.to_string())?;
    // Runtime dependencies are the app-local CRT in bin plus Windows system
    // DLLs. No cwd, PATH, or user DLL directory participates in resolution.
    let library = load_dll(LoadPolicy::ApplicationDir, path).map_err(|error| error.to_string())?;
    // SAFETY: the caller admitted the ONNX Runtime DLL. Its documented export
    // has this ABI; the library remains live until after every pointer use.
    let getter: libloading::Symbol<unsafe extern "system" fn() -> *const OrtApiBase> =
        unsafe { library.get(b"OrtGetApiBase\0") }
            .map_err(|error| format!("ONNX Runtime has no OrtGetApiBase export: {error}"))?;
    // SAFETY: invokes the documented export from the retained library.
    let base = unsafe { getter() };
    if base.is_null() {
        return Err("ONNX Runtime returned a null API base".to_owned());
    }
    // SAFETY: OrtGetApiBase returned its stable table; GetApi returns null for
    // unsupported versions. We never dereference the opaque OrtApi pointer.
    let api = unsafe { ((*base).get_api)(api_level) };
    if api.is_null() {
        return Err(format!("ONNX Runtime does not provide API {api_level}"));
    }
    // SAFETY: the version is a runtime-owned, NUL-terminated UTF-8 string.
    let version = unsafe { ((*base).get_version_string)() };
    if version.is_null() {
        return Err("ONNX Runtime returned a null version string".to_owned());
    }
    // SAFETY: checked non-null; the library owning the string is retained.
    let version = unsafe { CStr::from_ptr(version) }
        .to_str()
        .map_err(|error| format!("ONNX Runtime version is not UTF-8: {error}"))?
        .to_owned();
    Ok(LoadedOnnxRuntime {
        _library: library,
        version,
        api_address: api.addr(),
    })
}
