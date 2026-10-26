// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use std::os::fd::FromRawFd;

/// Private Binder endpoint. Ownership of a nonblocking write descriptor transfers
/// at entry, including every error path. The Java caller retains only its reader.
///
/// # Safety
/// `root` is a live NUL-terminated UTF-8 string; `request` is readable for `len`.
/// `fd` is an exclusively owned, valid nonblocking pipe write descriptor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_owner_open(
    root: *const c_char,
    request: *const u8,
    len: usize,
    fd: i32,
) -> *mut c_char {
    let sink = if fd >= 0 {
        Some(unsafe { fs::File::from_raw_fd(fd) })
    } else {
        None
    };
    let result = std::panic::catch_unwind(|| -> Result<Value, String> {
        let sink = sink.ok_or("owner response pipe required")?;
        if root.is_null() || request.is_null() || len == 0 || len > 384 * 1024 {
            return Err("bounded owner request required".into());
        }
        let root = Path::new(
            unsafe { CStr::from_ptr(root) }
                .to_str()
                .map_err(|e| e.to_string())?,
        );
        let request: Value =
            serde_json::from_slice(unsafe { std::slice::from_raw_parts(request, len) })
                .map_err(|e| e.to_string())?;
        let id = request["id"]
            .as_u64()
            .ok_or("owner request identity required")?;
        let headers: Vec<(String, String)> =
            serde_json::from_value(request["headers"].clone()).map_err(|e| e.to_string())?;
        let receiver = {
            let server = SERVING.lock().map_err(|_| "serving owner unavailable")?;
            let current = server.as_ref().ok_or("server stopped")?;
            let handle = current.handle()?;
            let generation = admitted_generation(&request, &THINKING_GENERATION)?;
            if generation & 1 != 0 {
                return Err("server stopped".into());
            }
            if handle.stop_handle().is_stopping() {
                return Err("server stop pending".into());
            }
            current
                .root
                .revalidate_canonical_binding()
                .map_err(|e| e.to_string())?;
            if fs::canonicalize(root).map_err(|e| e.to_string())? != current.root.canonical_path() {
                return Err("owner journal does not match the serving journal".into());
            }
            handle.open_owner_response(
                id,
                request["method"].as_str().ok_or("method required")?,
                request["uri"].as_str().ok_or("uri required")?,
                &headers,
                request["body"].as_str().unwrap_or("").as_bytes().to_vec(),
                sink,
            )?
        };
        // No serving lock or lifecycle executor is retained during route work.
        let head = receiver
            .recv_timeout(std::time::Duration::from_secs(60))
            .map_err(|_| {
                "owner response unavailable; mutation outcome may be unknown".to_owned()
            })??;
        Ok(json!({"status":head.status,"headers":head.headers}))
    });
    let value = match result {
        Ok(Ok(value)) => json!({"ok":true,"value":value}),
        Ok(Err(error)) => json!({"ok":false,"error":error}),
        Err(_) => {
            json!({"ok":false,"error":"owner transport failed; mutation outcome may be unknown"})
        }
    };
    CString::new(value.to_string()).unwrap().into_raw()
}

/// Cancel only the current runtime's named private response, never an owner action.
#[unsafe(no_mangle)]
pub extern "C" fn journal_owner_cancel(id: u64) {
    if let Ok(mut slots) = upload_slots().lock() {
        slots.remove(&id);
    }
    if let Ok(server) = SERVING.lock()
        && let Some(handle) = server.as_ref().and_then(|server| server.handle.as_ref())
    {
        handle.cancel_owner_response(id);
    }
}

/// Stream a request body from `body_fd`. Returns when the upload is admitted.
/// Both descriptors are owned here on every path, including refusal.
///
/// # Safety
/// `root` is a live NUL-terminated UTF-8 string. `request` is readable for `len`.
/// `response_fd` and `body_fd` are exclusively owned. `response_fd` is nonblocking.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_owner_open_stream(
    root: *const c_char,
    request: *const u8,
    len: usize,
    response_fd: i32,
    body_fd: i32,
) -> *mut c_char {
    let sink = take_fd(response_fd);
    let body = take_fd(body_fd);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        open_stream(root, request, len, sink, body)
    }));
    owned_reply(result)
}

/// Nonblocking head poll. Pending means the transfer is still running.
#[unsafe(no_mangle)]
pub extern "C" fn journal_owner_poll(id: u64) -> *mut c_char {
    let result = std::panic::catch_unwind(|| poll_upload(id));
    owned_reply(result)
}

/// Delete abandoned bridge copies. `live_json` is a JSON array of numeric ids.
///
/// # Safety
/// Each pointer is either null or a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_owner_reclaim(
    cache: *const c_char,
    journal: *const c_char,
    live_json: *const c_char,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reclaim_bridge(cache, journal, live_json)
    }));
    owned_reply(result)
}

type UploadSlot =
    std::sync::mpsc::Receiver<Result<journal_mobile_serving::OwnerResponseHead, String>>;
type UploadSlots = std::sync::Mutex<std::collections::HashMap<u64, UploadSlot>>;

fn upload_slots() -> &'static UploadSlots {
    static SLOTS: std::sync::OnceLock<UploadSlots> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn take_fd(fd: i32) -> Result<fs::File, String> {
    if fd < 0 {
        return Err("owner upload pipe required".into());
    }
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

fn open_stream(
    root: *const c_char,
    request: *const u8,
    len: usize,
    sink: Result<fs::File, String>,
    body: Result<fs::File, String>,
) -> Result<Value, String> {
    let sink = sink?;
    let body = body?;
    if root.is_null() || request.is_null() || len == 0 || len > 384 * 1024 {
        return Err("bounded owner request required".into());
    }
    let root = Path::new(
        unsafe { CStr::from_ptr(root) }
            .to_str()
            .map_err(|error| error.to_string())?,
    );
    let request: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(request, len) })
            .map_err(|error| error.to_string())?;
    let id = request["id"]
        .as_u64()
        .ok_or("owner request identity required")?;
    let headers: Vec<(String, String)> =
        serde_json::from_value(request["headers"].clone()).map_err(|error| error.to_string())?;
    let receiver = {
        let server = SERVING.lock().map_err(|_| "serving owner unavailable")?;
        let current = server.as_ref().ok_or("server stopped")?;
        let handle = current.handle()?;
        let generation = admitted_generation(&request, &THINKING_GENERATION)?;
        if generation & 1 != 0 {
            return Err("server stopped".into());
        }
        if handle.stop_handle().is_stopping() {
            return Err("server stop pending".into());
        }
        current
            .root
            .revalidate_canonical_binding()
            .map_err(|error| error.to_string())?;
        if fs::canonicalize(root).map_err(|error| error.to_string())?
            != current.root.canonical_path()
        {
            return Err("owner journal does not match the serving journal".into());
        }
        handle.open_owner_upload(
            id,
            request["method"].as_str().ok_or("method required")?,
            request["uri"].as_str().ok_or("uri required")?,
            &headers,
            body,
            sink,
        )?
    };
    upload_slots()
        .lock()
        .map_err(|_| "serving owner unavailable")?
        .insert(id, receiver);
    Ok(json!({"streaming": true}))
}

fn poll_upload(id: u64) -> Result<Value, String> {
    let mut slots = upload_slots()
        .lock()
        .map_err(|_| "serving owner unavailable")?;
    let outcome = {
        let Some(receiver) = slots.get(&id) else {
            return Err(
                "journal response unavailable; check the result before trying again.".into(),
            );
        };
        receiver.try_recv()
    };
    match outcome {
        Ok(Ok(head)) => {
            slots.remove(&id);
            Ok(json!({"status": head.status, "headers": head.headers}))
        }
        Ok(Err(error)) => {
            slots.remove(&id);
            Err(error)
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => Ok(json!({"pending": true})),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            slots.remove(&id);
            Err("journal response unavailable; check the result before trying again.".into())
        }
    }
}

fn reclaim_bridge(
    cache: *const c_char,
    journal: *const c_char,
    live_json: *const c_char,
) -> Result<Value, String> {
    if cache.is_null() || journal.is_null() {
        return Err("owner bridge paths required".into());
    }
    let cache = Path::new(
        unsafe { CStr::from_ptr(cache) }
            .to_str()
            .map_err(|error| error.to_string())?,
    );
    let journal = Path::new(
        unsafe { CStr::from_ptr(journal) }
            .to_str()
            .map_err(|error| error.to_string())?,
    );
    let mut live = extra_live(live_json)?;
    if let Ok(slots) = upload_slots().lock() {
        live.extend(slots.keys().copied());
    }
    journal_mobile_serving::owner_bridge::reclaim(cache, journal, &live)?;
    Ok(json!({"reclaimed": true}))
}

fn extra_live(live_json: *const c_char) -> Result<Vec<u64>, String> {
    if live_json.is_null() {
        return Ok(Vec::new());
    }
    let text = unsafe { CStr::from_ptr(live_json) }
        .to_str()
        .map_err(|error| error.to_string())?;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let list = value.as_array().ok_or("owner bridge live list required")?;
    list.iter()
        .map(|item| {
            item.as_u64()
                .ok_or_else(|| "owner bridge live list required".to_owned())
        })
        .collect()
}

fn owned_reply(
    result: Result<Result<Value, String>, Box<dyn std::any::Any + Send>>,
) -> *mut c_char {
    let value = match result {
        Ok(Ok(value)) => json!({"ok": true, "value": value}),
        Ok(Err(error)) => json!({"ok": false, "error": error}),
        Err(_) => {
            json!({"ok": false, "error": "journal response unavailable; check the result before trying again."})
        }
    };
    CString::new(value.to_string())
        .unwrap_or_else(|_| {
            CString::new(r#"{"ok":false,"error":"journal response unavailable; check the result before trying again."}"#).unwrap()
        })
        .into_raw()
}
