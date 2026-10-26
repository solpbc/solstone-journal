// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One RF-DETR context for in-process depict and describe.
//!
//! The context is created on the first `detect` inside an allowed job. A missing
//! or rejected model is `Err`, never `Ok(None)`. An empty detection list is
//! `Ok(Some)` of `{image, detections: []}`.

#[cfg(feature = "rfdetr-native")]
use std::env;
#[cfg(feature = "rfdetr-native")]
use std::fs;
use std::path::Path;
#[cfg(feature = "rfdetr-native")]
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::Value;
#[cfg(feature = "rfdetr-native")]
use sha2::{Digest, Sha256};

#[cfg(feature = "rfdetr-native")]
const MODEL_SIZE: u64 = 63_439_488;
#[cfg(feature = "rfdetr-native")]
const MODEL_SHA256: &str = "d798cc448faa53209b88fc905c91beb1dd104634b95f6948cc4877540a8fd3ee";
const THRESHOLD: f32 = 0.25;
const TOP_K: u32 = 300;
const THREADS: u32 = 4;

/// Desktop and mobile detection use one threshold, thread count, and top-k.
pub(crate) fn detection_call() -> (f32, u32, u32) {
    (THRESHOLD, THREADS, TOP_K)
}

/// Shared depict `Detector` and describe screen hook. One native context per job.
pub struct RfdetrDetector {
    journal: std::path::PathBuf,
    context: Mutex<Option<Loaded>>,
}

struct Loaded {
    #[cfg(feature = "rfdetr-native")]
    handle: native::Handle,
}

impl RfdetrDetector {
    pub fn new(journal: &Path) -> Self {
        Self {
            journal: journal.to_path_buf(),
            context: Mutex::new(None),
        }
    }

    /// Raw detector JSON with `image` and `detections`. Never `Ok` of an absent run.
    pub fn detect_value(&self, png: &[u8], journal: &Path) -> Result<Value, String> {
        #[cfg(not(feature = "rfdetr-native"))]
        {
            let _ = (&self.context, &self.journal, png, journal);
            return Err("rfdetr native library is not linked".to_owned());
        }
        #[cfg(feature = "rfdetr-native")]
        {
            let (threshold, threads, top_k) = detection_call();
            let mut guard = self
                .context
                .lock()
                .map_err(|_| "rfdetr context lock poisoned".to_owned())?;
            if guard.is_none() {
                let path = model_path(journal);
                admit_model(&path)?;
                let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes())
                    .map_err(|_| "rfdetr model path is not a C string".to_owned())?;
                let mut handle = 0usize;
                let rc = unsafe { native::rfdetr_capi_load(c_path.as_ptr(), threads, &mut handle) };
                if rc != 0 || handle == 0 {
                    return Err(format!("rfdetr_capi_load failed ({rc})"));
                }
                *guard = Some(Loaded { handle });
            }
            let handle = guard.as_ref().expect("context was just loaded").handle;
            let mut json_ptr: *mut std::ffi::c_char = std::ptr::null_mut();
            let rc = unsafe {
                native::rfdetr_capi_detect_buffer(
                    handle,
                    png.as_ptr(),
                    png.len(),
                    threshold,
                    top_k,
                    &mut json_ptr,
                )
            };
            if json_ptr.is_null() {
                return Err(format!("rfdetr_capi_detect_buffer failed ({rc})"));
            }
            let text = unsafe { std::ffi::CStr::from_ptr(json_ptr) }
                .to_string_lossy()
                .into_owned();
            unsafe { native::rfdetr_capi_free_string(json_ptr) };
            if rc != 0 {
                return Err(format!("rfdetr_capi_detect_buffer failed ({rc})"));
            }
            let value: Value = serde_json::from_str(&text)
                .map_err(|error| format!("rfdetr detection json: {error}"))?;
            if value.get("status").and_then(Value::as_str) != Some("ok") {
                return Err("rfdetr detection status was not ok".to_owned());
            }
            if value.get("image").is_none() || value.get("detections").is_none() {
                return Err("rfdetr detection omitted image or detections".to_owned());
            }
            Ok(value)
        }
    }
}

impl Drop for Loaded {
    fn drop(&mut self) {
        #[cfg(feature = "rfdetr-native")]
        unsafe {
            native::rfdetr_capi_unload(self.handle);
        }
    }
}

impl solstone_core_depict::Detector for RfdetrDetector {
    fn detect(&self, full_png: &[u8]) -> Result<Option<Value>, String> {
        self.detect_value(full_png, &self.journal).map(Some)
    }
}

impl solstone_core_describe::pipeline::ScreenDetector for RfdetrDetector {
    fn detect(&self, png: &[u8], journal: &Path) -> Result<Value, String> {
        self.detect_value(png, journal)
    }
}

#[cfg(feature = "rfdetr-native")]
fn model_path(journal: &Path) -> PathBuf {
    if let Some(path) = env::var_os("SOLSTONE_RFDETR_MODEL") {
        return PathBuf::from(path);
    }
    solstone_core_local::install::rfdetr_install::model_path(journal)
}

#[cfg(feature = "rfdetr-native")]
fn admit_model(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| format!("rfdetr model: {error}"))?;
    if !metadata.is_file() || metadata.len() != MODEL_SIZE {
        return Err("rfdetr model size does not match the pinned asset".to_owned());
    }
    let bytes = fs::read(path).map_err(|error| format!("rfdetr model: {error}"))?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if digest != MODEL_SHA256 {
        return Err("rfdetr model digest does not match the pinned asset".to_owned());
    }
    Ok(())
}

#[cfg(feature = "rfdetr-native")]
#[allow(dead_code, non_camel_case_types)]
mod native {
    use std::ffi::{c_char, c_float, c_int};

    /// `uintptr_t rfdetr_handle_t` from the pinned `rfdetr_capi.h`.
    pub type Handle = usize;

    // Prototypes from the pinned header at ec73712e4c933dc36e79963cab47d702c34f02bf.
    unsafe extern "C" {
        pub fn rfdetr_capi_load(
            model_path: *const c_char,
            n_threads: u32,
            out_handle: *mut Handle,
        ) -> c_int;
        pub fn rfdetr_capi_unload(handle: Handle) -> c_int;
        pub fn rfdetr_capi_detect_path(
            handle: Handle,
            image_path: *const c_char,
            threshold: c_float,
            top_k: u32,
            out_json: *mut *mut c_char,
        ) -> c_int;
        pub fn rfdetr_capi_detect_buffer(
            handle: Handle,
            data: *const u8,
            len: usize,
            threshold: c_float,
            top_k: u32,
            out_json: *mut *mut c_char,
        ) -> c_int;
        pub fn rfdetr_capi_free_string(ptr: *mut c_char);
        pub fn rfdetr_capi_get_n_detections(handle: Handle, out_n: *mut u32) -> c_int;
        pub fn rfdetr_capi_get_detection_class_id(
            handle: Handle,
            index: u32,
            out_class_id: *mut i32,
        ) -> c_int;
        pub fn rfdetr_capi_get_detection_box(
            handle: Handle,
            index: u32,
            x1: *mut c_float,
            y1: *mut c_float,
            x2: *mut c_float,
            y2: *mut c_float,
        ) -> c_int;
        pub fn rfdetr_capi_get_detection_score(
            handle: Handle,
            index: u32,
            out_score: *mut c_float,
        ) -> c_int;
        pub fn rfdetr_capi_get_detection_class_name(
            handle: Handle,
            index: u32,
            buf: *mut c_char,
            buflen: usize,
        ) -> c_int;
        pub fn rfdetr_capi_get_detection_mask_png(
            handle: Handle,
            index: u32,
            out_png: *mut *mut u8,
            out_len: *mut usize,
        ) -> c_int;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn detection_call_matches_desktop_contract() {
        let (threshold, threads, top_k) = super::detection_call();
        assert_eq!(threshold, 0.25);
        assert_eq!(threads, 4);
        assert_eq!(top_k, 300);
    }
}
