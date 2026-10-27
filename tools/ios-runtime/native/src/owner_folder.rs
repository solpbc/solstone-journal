// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A synchronous OS folder callback under the original iOS owner preparation.

use super::*;
use journal_mobile_serving::foreground::{FolderCopyError, FolderCopyResult};
use solstone_core_import::{FolderAdmitError, WorkStop, installed_stop};
use std::ffi::c_void;

#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_owner_folder_copy_continues() -> bool {
    installed_stop().is_some_and(|stop| !stop.is_requested())
}

/// # Safety
/// Cache and request remain readable for this synchronous private native call.
/// Context and callback remain on the original retained worker until return.
/// The callback closes all provider reads and security scopes before returning;
/// it cannot detach reads, retain pointers or reuse the destination afterward.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_folder_copy(
    cache: *const c_char,
    request: *const u8,
    length: usize,
    context: *mut c_void,
    copy: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> bool>,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        copy_folder(cache, request, length, context, copy)
    }));
    let reply = match result {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => {
            json!({"ok":false,"state":"refused","error":FolderAdmitError::Denied.to_string()})
        }
        Err(_) => {
            json!({"ok":false,"state":"outcome_unknown","error":solstone_core_import::IMPORT_UNCONFIRMED_REASON})
        }
    };
    CString::new(reply.to_string()).unwrap().into_raw()
}

unsafe fn copy_folder(
    cache: *const c_char,
    request: *const u8,
    length: usize,
    context: *mut c_void,
    copy: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> bool>,
) -> Result<Value, ()> {
    if cache.is_null() || request.is_null() || length == 0 || length > 32 * 1024 {
        return Err(());
    }
    let cache = Path::new(unsafe { CStr::from_ptr(cache) }.to_str().map_err(|_| ())?);
    if !cache.is_absolute() {
        return Err(());
    }
    let request: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(request, length) })
            .map_err(|_| ())?;
    let copy = copy.ok_or(())?;
    let source = request["source"].as_str().ok_or(())?;
    if !matches!(source, "obsidian" | "gemini" | "document" | "audio_archive") {
        return Err(());
    }
    let reference = request["client_item_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or(())?;
    let display = request["folder_name"].as_str().ok_or(())?;
    let authority = request["authority"]
        .as_u64()
        .filter(|id| *id != 0)
        .ok_or(())?;
    if !owner_response::owner_authority_live(authority) {
        return Err(());
    }
    let preparation = {
        let serving = SERVING.lock().map_err(|_| ())?;
        let current = serving.as_ref().ok_or(())?;
        let generation = admit_processing(&request, &PROCESSING_GENERATION).map_err(|_| ())?;
        if generation & 1 != 0
            || current.serving.stop_handle().is_stopping()
            || !owner_response::owner_authority_live(authority)
        {
            return Err(());
        }
        let stop =
            WorkStop::linked_with_checkpoint(Arc::new(AtomicBool::new(false)), None, move || {
                !owner_response::owner_authority_live(authority)
            });
        current
            .processing
            .as_ref()
            .ok_or(())?
            .import_preparation(stop, Default::default())
            .map_err(|_| ())?
    };
    // No provider, copy or configuration read holds the lifecycle mutex.
    let timestamp = preparation.folder_timestamp().map_err(|_| ())?;
    let result = preparation.with_folder_copy(
        cache,
        reference,
        &timestamp,
        |destination| {
            let destination = CString::new(
                destination
                    .to_str()
                    .ok_or_else(|| FolderAdmitError::Denied.to_string())?,
            )
            .map_err(|_| FolderAdmitError::Denied.to_string())?;
            if unsafe { copy(context, destination.as_ptr()) } {
                Ok(())
            } else {
                Err(FolderAdmitError::Denied.to_string())
            }
        },
        |root, folder| {
            let config = solstone_core_journal_config::read_journal_config_bound(root)
                .map_err(|_| FolderAdmitError::Denied.to_string())?;
            let zone = config
                .config
                .as_ref()
                .and_then(|config| config.get("identity"))
                .and_then(|identity| identity.get("timezone"))
                .and_then(Value::as_str)
                .and_then(solstone_core_journal_config::parse_zone)
                .unwrap_or_else(solstone_core_journal_config::host_zone);
            solstone_core_import_web::register_admitted_folder(
                root, folder, source, display, reference, &timestamp, zone,
            )
            .map_err(|error| error.to_string())
        },
    );
    Ok(match result {
        Ok(FolderCopyResult::Registered { folder, value }) => {
            json!({"ok":true,"state":"saved","import_id":folder.import_id.as_str(),"folder_name":display,"value":value})
        }
        Ok(FolderCopyResult::RegistrationFailed { folder, reason }) => {
            json!({"ok":false,"state":"registration_failed","import_id":folder.import_id.as_str(),"folder_name":display,"custody":"saved","error":reason})
        }
        Err(FolderCopyError::AdmissionUnconfirmed { reference, reason }) => {
            json!({"ok":false,"state":"outcome_unknown","reference":reference,"error":reason})
        }
        Err(FolderCopyError::Refused(reason)) => {
            json!({"ok":false,"state":"refused","error":reason})
        }
    })
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "owner_folder_tests.rs"]
pub(crate) mod tests;
