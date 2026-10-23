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
    if let Ok(server) = SERVING.lock()
        && let Some(handle) = server.as_ref().and_then(|server| server.handle.as_ref())
    {
        handle.cancel_owner_response(id);
    }
}
