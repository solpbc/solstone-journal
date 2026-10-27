// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A registered Service provider called synchronously on the retained importer worker.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use journal_mobile_serving::foreground::ImportSeamFactory;
use journal_mobile_serving::import_route::{
    ConfiguredSources, SavedFolderProvider, SavedFolderRequest, SourceCopyError,
    packaged_import_factory, packaged_import_factory_with_sources,
};
use solstone_core_import::{BackendName, WorkStopGuard};

type CopyProvider =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *const c_char) -> i32;
type ReleaseProvider = unsafe extern "C" fn(*mut c_void);

struct NativeProvider {
    context: *mut c_void,
    copy: CopyProvider,
    release: ReleaseProvider,
}

// SAFETY: registration requires an immutable context that pins a local Java
// reference and attaches each calling thread independently. Calls close every
// cursor/stream before return. The last Arc exclusively releases the context.
unsafe impl Send for NativeProvider {}
// SAFETY: the same callback contract permits independent simultaneous calls.
unsafe impl Sync for NativeProvider {}

impl Drop for NativeProvider {
    fn drop(&mut self) {
        unsafe { (self.release)(self.context) }
    }
}

impl SavedFolderProvider for NativeProvider {
    fn copy(
        &self,
        request: SavedFolderRequest<'_>,
        destination: &Path,
    ) -> Result<(), SourceCopyError> {
        if request.stop.is_requested()
            || !matches!(request.backend, BackendName::Obsidian | BackendName::Audio)
        {
            return Err(SourceCopyError::Unavailable);
        }
        let identity =
            CString::new(request.source_identity).map_err(|_| SourceCopyError::Unavailable)?;
        let material =
            CString::new(request.access_material).map_err(|_| SourceCopyError::Unavailable)?;
        let destination = destination.to_str().ok_or(SourceCopyError::Unavailable)?;
        let destination = CString::new(destination).map_err(|_| SourceCopyError::Unavailable)?;
        let _installed = WorkStopGuard::install(request.stop);
        let result = unsafe {
            (self.copy)(
                self.context,
                identity.as_ptr(),
                material.as_ptr(),
                destination.as_ptr(),
            )
        };
        if request.stop.is_requested() {
            return Err(SourceCopyError::Unavailable);
        }
        match result {
            0 => Ok(()),
            1 => Err(SourceCopyError::AccessLost),
            _ => Err(SourceCopyError::Unavailable),
        }
    }
}

#[derive(Clone)]
struct Installation {
    identity: u64,
    cache: PathBuf,
    provider: Arc<NativeProvider>,
}

static INSTALLATION: Mutex<Option<Installation>> = Mutex::new(None);
static NEXT_IDENTITY: AtomicU64 = AtomicU64::new(1);

/// # Safety
/// Cache is a live UTF-8 buffer from the private Service. Context and callbacks
/// satisfy NativeProvider's thread/resource contract. Non-null context with both
/// callbacks is consumed on every return, including refusal. No root is opened.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_owner_sources_install(
    cache: *const c_char,
    context: *mut c_void,
    copy: Option<CopyProvider>,
    release: Option<ReleaseProvider>,
) -> u64 {
    let (Some(copy), Some(release)) = (copy, release) else {
        return 0;
    };
    if context.is_null() {
        return 0;
    }
    let provider = Arc::new(NativeProvider {
        context,
        copy,
        release,
    });
    std::panic::catch_unwind(move || -> Option<u64> {
        if cache.is_null() {
            return None;
        }
        let cache = unsafe { CStr::from_ptr(cache) }.to_str().ok()?;
        let cache = PathBuf::from(cache);
        if !cache.is_absolute() {
            return None;
        }
        let identity = NEXT_IDENTITY
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| old.checked_add(1))
            .ok()?;
        let mut slot = INSTALLATION.lock().ok()?;
        *slot = Some(Installation {
            identity,
            cache,
            provider,
        });
        Some(identity)
    })
    .ok()
    .flatten()
    .unwrap_or(0)
}

/// Retiring an older Service cannot remove a newer registration. Existing host
/// factories and in-flight calls retain their exact Arc until actual return.
#[unsafe(no_mangle)]
pub extern "C" fn journal_owner_sources_retire(identity: u64) {
    if let Ok(mut slot) = INSTALLATION.lock()
        && slot
            .as_ref()
            .is_some_and(|current| current.identity == identity)
    {
        slot.take();
    }
}

pub(super) fn factory(
    library: PathBuf,
) -> (Arc<ImportSeamFactory>, Option<Arc<ConfiguredSources>>) {
    let installed = INSTALLATION.lock().ok().and_then(|current| current.clone());
    let Some(installed) = installed else {
        return (packaged_import_factory(library), None);
    };
    let sources = Arc::new(ConfiguredSources::new(installed.cache, installed.provider));
    (
        packaged_import_factory_with_sources(library, Arc::clone(&sources)),
        Some(sources),
    )
}

/// Read the current native owner's public folder connection. No root, grant,
/// command arguments or provider selection is accepted from the request.
///
/// # Safety
/// Request is a readable JSON buffer for this synchronous private Service call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_owner_sources_status(
    request: *const u8,
    length: usize,
) -> *mut c_char {
    let result = std::panic::catch_unwind(|| unsafe { source_status(request, length) });
    let value = match result {
        Ok(Ok(value)) => serde_json::json!({"ok":true,"value":value}),
        _ => serde_json::json!({"ok":false,"error":"source connection is unavailable"}),
    };
    CString::new(value.to_string()).unwrap().into_raw()
}

unsafe fn source_status(request: *const u8, length: usize) -> Result<serde_json::Value, ()> {
    if request.is_null() || length == 0 || length > 32 * 1024 {
        return Err(());
    }
    let request: serde_json::Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(request, length) })
            .map_err(|_| ())?;
    let backend = match request["source"].as_str() {
        Some("obsidian") => BackendName::Obsidian,
        Some("audio") => BackendName::Audio,
        _ => return Err(()),
    };
    let (preparation, root, sources) = {
        let serving = super::SERVING.lock().map_err(|_| ())?;
        let owner = serving.as_ref().ok_or(())?;
        let generation =
            super::admitted_generation(&request, &super::THINKING_GENERATION).map_err(|_| ())?;
        if generation & 1 != 0 || owner.handle().map_err(|_| ())?.stop_handle().is_stopping() {
            return Err(());
        }
        let host = owner.host.as_ref().ok_or(())?;
        let sources = Arc::clone(owner.sources.as_ref().ok_or(())?);
        let preparation = host
            .import_preparation(solstone_core_import::WorkStop::new(), Default::default())
            .map_err(|_| ())?;
        (preparation, Arc::clone(&owner.root), sources)
    };
    preparation.checkpoint().map_err(|_| ())?;
    let guard = sources.connection_guard(backend);
    let busy = match &guard {
        Ok(_) => false,
        Err(journal_mobile_serving::import_route::SourceConnectionError::Busy) => true,
        Err(_) => return Err(()),
    };
    let connection = match solstone_core_import::read_sync_state_bound(&root, backend) {
        solstone_core_import::SyncStateRead::Absent => serde_json::Value::Null,
        solstone_core_import::SyncStateRead::Loaded(state) => solstone_core_import::public_connection(&state).map_or(serde_json::Value::Null, |public| serde_json::json!({
            "source_identity":public.source_identity,"folder_name":public.folder_name,
            "remembered":public.remembered,"access_state":public.access_state.to_string()
        })),
        solstone_core_import::SyncStateRead::Unreadable { .. } => return Err(()),
    };
    preparation.checkpoint().map_err(|_| ())?;
    Ok(serde_json::json!({"source":backend.as_str(),"connection":connection,"busy":busy}))
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "saved_sources_tests.rs"]
mod tests;
