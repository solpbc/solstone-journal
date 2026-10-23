// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use std::os::fd::FromRawFd;

/// Private in-process owner route. No address, journal path or access credential
/// can be supplied by the web document. The caller owns the response reader.
///
/// # Safety
/// `request` is readable for `len` bytes. `fd` transfers exclusive ownership of
/// a valid nonblocking pipe writer, including on refusal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_open(
    request: *const u8,
    len: usize,
    fd: i32,
) -> *mut c_char {
    let sink = (fd >= 0).then(|| unsafe { std::fs::File::from_raw_fd(fd) });
    let result = std::panic::catch_unwind(|| -> Result<Value, String> {
        let sink = sink.ok_or("owner response pipe required")?;
        if request.is_null() || len == 0 || len > 384 * 1024 {
            return Err("bounded owner request required".into());
        }
        let request: Value =
            serde_json::from_slice(unsafe { std::slice::from_raw_parts(request, len) })
                .map_err(|e| e.to_string())?;
        let id = request["id"]
            .as_u64()
            .ok_or("owner request identity required")?;
        let headers: Vec<(String, String)> =
            serde_json::from_value(request["headers"].clone()).map_err(|e| e.to_string())?;
        let receiver = {
            let serving = SERVING.lock().map_err(|_| "serving owner unavailable")?;
            let current = serving.as_ref().ok_or("journal is stopped")?;
            let generation = admit_processing(&request, &PROCESSING_GENERATION)?;
            if generation & 1 != 0 || current.serving.stop_handle().is_stopping() {
                return Err("journal is stopping".into());
            }
            current.serving.open_owner_response(
                id,
                request["method"].as_str().ok_or("method required")?,
                request["uri"].as_str().ok_or("path required")?,
                &headers,
                request["body"].as_str().unwrap_or("").as_bytes().to_vec(),
                sink,
            )?
        };
        // Route work never retains the serving mutex or lifecycle executor.
        let head = receiver
            .recv_timeout(Duration::from_secs(60))
            .map_err(|_| "owner response unavailable; mutation outcome may be unknown")??;
        Ok(json!({"status": head.status, "headers": head.headers}))
    });
    let value = match result {
        Ok(Ok(value)) => json!({"ok": true, "value": value}),
        Ok(Err(error)) => json!({"ok": false, "error": error}),
        Err(_) => {
            json!({"ok": false, "error": "owner response failed; check the result before retrying"})
        }
    };
    CString::new(value.to_string()).unwrap().into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_owner_cancel(id: u64) {
    if let Ok(serving) = SERVING.lock()
        && let Some(current) = serving.as_ref()
    {
        current.serving.cancel_owner_response(id);
    }
}

#[cfg(all(test, feature = "full-tests", unix))]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn refused_request_still_consumes_its_response_writer() {
        let mut descriptors = [-1; 2];
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        let mut reader = unsafe { std::fs::File::from_raw_fd(descriptors[0]) };
        for fd in descriptors {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
                0
            );
        }
        let result = unsafe { journal_ios_owner_open(std::ptr::null(), 0, descriptors[1]) };
        let reply: Value =
            serde_json::from_str(unsafe { CStr::from_ptr(result) }.to_str().unwrap()).unwrap();
        unsafe { journal_runtime_free(result) };
        assert_eq!(reply["ok"], false);
        // EOF proves this pipe's writer was closed. A leaked writer would
        // produce WouldBlock rather than hanging this qualification.
        assert_eq!(reader.read(&mut [0]).unwrap(), 0);
    }
}
