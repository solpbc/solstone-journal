// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    os::unix::ffi::OsStrExt,
    path::Path,
    ptr::NonNull,
};

unsafe extern "C" {
    fn parakeet_capi_abi_version() -> c_int;
    fn parakeet_capi_load(path: *const c_char) -> *mut c_void;
    fn parakeet_capi_load_error() -> *const c_char;
    fn parakeet_capi_free(ctx: *mut c_void);
    fn parakeet_capi_set_concurrency(ctx: *mut c_void, backends: c_int, threads: c_int) -> c_int;
    fn parakeet_capi_transcribe_pcm_batch_json(
        ctx: *mut c_void,
        samples: *const f32,
        lengths: *const c_int,
        clips: c_int,
        sample_rate: c_int,
        decoder: c_int,
    ) -> *mut c_char;
    fn parakeet_capi_last_error(ctx: *mut c_void) -> *const c_char;
    fn parakeet_capi_free_string(value: *mut c_char);
}

// NonNull deliberately keeps this owner !Send and !Sync: construction, use, and
// destruction stay on one scanner thread. No native pointer crosses the API.
pub(super) struct Context(NonNull<c_void>);

impl Context {
    pub(super) fn load(path: &Path) -> Result<Self, String> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: ABI version is a no-argument probe. path is a live NUL-terminated
        // string. load returns a unique owner or NULL; errors are thread-local.
        unsafe {
            if parakeet_capi_abi_version() != 10 {
                return Err("Parakeet C API ABI differs from pinned ABI 10".into());
            }
            let pointer = NonNull::new(parakeet_capi_load(path.as_ptr())).ok_or_else(|| {
                CStr::from_ptr(parakeet_capi_load_error())
                    .to_string_lossy()
                    .into_owned()
            })?;
            let context = Self(pointer);
            if parakeet_capi_set_concurrency(context.0.as_ptr(), 1, 4) != 1 {
                return Err(context.error());
            }
            Ok(context)
        }
    }

    fn error(&self) -> String {
        // SAFETY: self owns a live context; the API returns a borrowed error string.
        unsafe {
            CStr::from_ptr(parakeet_capi_last_error(self.0.as_ptr()))
                .to_string_lossy()
                .into_owned()
        }
    }

    pub(super) fn transcribe(
        &self,
        samples: &[f32],
        native_call: &dyn Fn(bool),
    ) -> Result<serde_json::Value, String> {
        let length = c_int::try_from(samples.len()).map_err(|e| e.to_string())?;
        // SAFETY: one clip's declared length equals the live slice length. The
        // context is uniquely owned on this thread. The result is freed exactly
        // once after copying, including when JSON decoding fails.
        unsafe {
            let result = crate::observe_native_call(native_call, || {
                parakeet_capi_transcribe_pcm_batch_json(
                    self.0.as_ptr(),
                    samples.as_ptr(),
                    &length,
                    1,
                    16_000,
                    0,
                )
            });
            if result.is_null() {
                return Err(self.error());
            }
            let json = CStr::from_ptr(result).to_bytes().to_vec();
            parakeet_capi_free_string(result);
            serde_json::from_slice(&json).map_err(|e| format!("Parakeet native JSON: {e}"))
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: this is the sole owner and free is called once after last use.
        unsafe {
            parakeet_capi_free(self.0.as_ptr());
        }
    }
}
