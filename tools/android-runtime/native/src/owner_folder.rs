// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Synchronous folder copying on the private JournalService worker.

use super::*;
use journal_mobile_serving::foreground::{FolderCopyError, FolderCopyResult};
use solstone_core_import::{FolderAdmitError, WorkStop, installed_stop, validate_timestamp};
use solstone_core_journal_config::work_priority::{WorkBand, WorkPriority};
use std::ffi::c_void;
use std::sync::Arc;

// This is the attached owner document's authority, never a recovery fence.
// Issuance is called only by the private service after its current host opens.
static PAGE: AtomicU64 = AtomicU64::new(1);
// Active host generations are even, including the first generation zero.
const UNBOUND_HOST: u64 = u64::MAX;
static PAGE_HOST: AtomicU64 = AtomicU64::new(UNBOUND_HOST);
static PAGE_BINDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Allocate in service-message order before queued root validation. A late
/// validation can bind only this nonce and cannot replace a newer document.
#[unsafe(no_mangle)]
pub extern "C" fn journal_owner_folder_page_allocate() -> u64 {
    let Ok(_binding) = PAGE_BINDING.lock() else {
        return 0;
    };
    let Some(next) = PAGE
        .load(Ordering::SeqCst)
        .checked_add(2)
        .map(|next| next & !1)
    else {
        return 0;
    };
    PAGE.store(next, Ordering::SeqCst);
    PAGE_HOST.store(UNBOUND_HOST, Ordering::SeqCst);
    next
}

/// # Safety
/// `root` is the live UTF-8 root of the document being attached by the service.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_owner_folder_page_issue(
    root: *const c_char,
    generation: u64,
) -> u64 {
    let page = journal_owner_folder_page_allocate();
    unsafe { journal_owner_folder_page_bind(root, generation, page) }
}

/// # Safety
/// `root` has the same buffer contract as `journal_owner_folder_page_issue`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_owner_folder_page_bind(
    root: *const c_char,
    generation: u64,
    page: u64,
) -> u64 {
    if root.is_null() || page == 0 || page & 1 != 0 {
        return 0;
    }
    let Ok(root) = (unsafe { CStr::from_ptr(root) }).to_str() else {
        return 0;
    };
    let Ok(serving) = SERVING.lock() else {
        return 0;
    };
    let Some(owner) = serving.as_ref() else {
        return 0;
    };
    let Ok(handle) = owner.handle() else { return 0 };
    if generation & 1 != 0
        || generation != THINKING_GENERATION.load(Ordering::SeqCst)
        || handle.stop_handle().is_stopping()
        || owner.root.revalidate_canonical_binding().is_err()
        || fs::canonicalize(root).ok().as_deref() != Some(owner.root.canonical_path())
    {
        return 0;
    }
    let Ok(_binding) = PAGE_BINDING.lock() else {
        return 0;
    };
    if PAGE.load(Ordering::SeqCst) != page
        || THINKING_GENERATION.load(Ordering::SeqCst) != generation
    {
        return 0;
    }
    PAGE_HOST.store(generation, Ordering::SeqCst);
    page
}

#[unsafe(no_mangle)]
pub extern "C" fn journal_owner_folder_page_retire(page: u64) {
    let Ok(_binding) = PAGE_BINDING.lock() else {
        return;
    };
    if page != 0
        && PAGE
            .compare_exchange(page, page | 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        PAGE_HOST.store(UNBOUND_HOST, Ordering::SeqCst);
    }
}

/// Called by the Java copier on this same JNI thread. No installed worker means
/// no authority. The installed stop includes document, host and original root.
#[unsafe(no_mangle)]
pub extern "C" fn journal_owner_folder_copy_continues() -> bool {
    installed_stop().is_some_and(|stop| !stop.is_requested())
}

#[unsafe(no_mangle)]
pub extern "C" fn journal_owner_folder_page_current() -> u64 {
    if !journal_owner_folder_copy_continues() {
        return 0;
    }
    let page = PAGE.load(Ordering::SeqCst);
    if page & 1 == 0 { page } else { 0 }
}

/// # Safety
/// Cache and request are live UTF-8/JSON buffers for the synchronous call.
/// The callback and context remain valid on this service worker until return.
/// It closes every provider cursor/stream before returning. It must not spawn
/// or detach reads, reuse the destination later, or retain callback pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_owner_folder_copy(
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
    let page = request["page_authority"]
        .as_u64()
        .filter(|page| *page != 0 && *page & 1 == 0)
        .ok_or(())?;
    let (preparation, timestamp) = {
        let serving = SERVING.lock().map_err(|_| ())?;
        let owner = serving.as_ref().ok_or(())?;
        let handle = owner.handle().map_err(|_| ())?;
        let generation = admitted_generation(&request, &THINKING_GENERATION).map_err(|_| ())?;
        if generation & 1 != 0
            || handle.stop_handle().is_stopping()
            || PAGE.load(Ordering::SeqCst) != page
            || PAGE_HOST.load(Ordering::SeqCst) != generation
        {
            return Err(());
        }
        let host = owner.host.as_ref().ok_or(())?;
        let generated;
        let raw_timestamp = match request.get("timestamp") {
            Some(value) => value.as_str().ok_or(())?,
            None => {
                owner.root.revalidate_canonical_binding().map_err(|_| ())?;
                let zone = solstone_core_journal_config::owner_zone(owner.root.canonical_path());
                generated = chrono::Utc::now()
                    .with_timezone(&zone)
                    .format("%Y%m%d_%H%M%S")
                    .to_string();
                &generated
            }
        };
        let timestamp = validate_timestamp(raw_timestamp).map_err(|_| ())?;
        let stop =
            WorkStop::linked_with_checkpoint(Arc::new(AtomicBool::new(false)), None, move || {
                PAGE.load(Ordering::SeqCst) != page
            });
        let priority = WorkPriority {
            band: WorkBand::Import,
            source_time: timestamp
                .as_str()
                .replace('_', "")
                .parse()
                .map_err(|_| ())?,
        };
        (
            host.import_preparation(stop, priority).map_err(|_| ())?,
            timestamp,
        )
    };
    // SERVING is released before all provider, staging, metadata and cleanup I/O.
    // No native import attempt is admitted by this copy/register operation.
    let outcome = preparation.with_folder_copy(
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
            let zone = solstone_core_journal_config::owner_zone(root.canonical_path());
            #[cfg(all(test, feature = "full-tests"))]
            tests::before_registration(root, folder);
            solstone_core_import_web::register_admitted_folder(
                root, folder, source, display, reference, &timestamp, zone,
            )
            .map_err(|error| error.to_string())
        },
    );
    Ok(match outcome {
        Ok(FolderCopyResult::Registered { folder, value }) => {
            json!({"ok":true,"state":"saved","import_id":folder.import_id.as_str(),"value":value})
        }
        Ok(FolderCopyResult::RegistrationFailed { folder, reason }) => {
            json!({"ok":false,"state":"registration_failed","import_id":folder.import_id.as_str(),"custody":"saved","error":reason})
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
mod tests;
