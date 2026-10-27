// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Static iOS provider callbacks retained by the original host factory.

use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use journal_mobile_serving::foreground::ImportSeamFactory;
use journal_mobile_serving::import_route::{
    ConfiguredSources, SavedFolderProvider, SavedFolderRequest, SourceCopyError,
    packaged_import_factory, packaged_import_factory_with_sources,
};
use solstone_core_import::{BackendName, WorkStopGuard};

type CopyProvider = unsafe extern "C" fn(*const c_char, *const c_char) -> i32;

struct NativeProvider {
    identity: u64,
    copy: CopyProvider,
}

impl Drop for NativeProvider {
    fn drop(&mut self) {
        journal_ios_owner_sources_retire(self.identity);
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
        let material =
            CString::new(request.access_material).map_err(|_| SourceCopyError::Unavailable)?;
        let destination = CString::new(destination.to_str().ok_or(SourceCopyError::Unavailable)?)
            .map_err(|_| SourceCopyError::Unavailable)?;
        let _installed = WorkStopGuard::install(request.stop);
        let result = unsafe { (self.copy)(material.as_ptr(), destination.as_ptr()) };
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
    copy: CopyProvider,
}

static INSTALLATION: Mutex<Option<Installation>> = Mutex::new(None);
static NEXT_IDENTITY: AtomicU64 = AtomicU64::new(1);

#[path = "source_connection.rs"]
mod connection;

/// # Safety
/// Cache is a live private UTF-8 buffer. The callback has static code lifetime,
/// accepts simultaneous calls on native retained workers, installs no detached
/// work, closes every provider read/security scope before return, and retains
/// neither borrowed strings nor the destination. No root is opened here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_sources_install(
    cache: *const c_char,
    copy: Option<CopyProvider>,
) -> u64 {
    std::panic::catch_unwind(|| -> Option<u64> {
        let copy = copy?;
        if cache.is_null() {
            return None;
        }
        let cache = PathBuf::from(unsafe { CStr::from_ptr(cache) }.to_str().ok()?);
        if !cache.is_absolute() {
            return None;
        }
        // Match the existing constructor's SERVING -> INSTALLATION lock order.
        // A duplicate/failed startup cannot replace an active owner's provider.
        let serving = super::SERVING.lock().ok()?;
        let mut slot = INSTALLATION.lock().ok()?;
        if let Some(current) = slot.as_ref()
            && current.cache == cache
            && std::ptr::fn_addr_eq(current.copy, copy)
        {
            return Some(current.identity);
        }
        if serving.is_some() {
            return None;
        }
        let identity = NEXT_IDENTITY
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| old.checked_add(1))
            .ok()?;
        *slot = Some(Installation {
            identity,
            cache,
            copy,
        });
        Some(identity)
    })
    .ok()
    .flatten()
    .unwrap_or(0)
}

/// An older provider cannot retire a newer registration. Host factories and
/// in-flight changes/copies keep their Sources Arc through actual joined work.
#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_owner_sources_retire(identity: u64) {
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
    let installed = INSTALLATION.lock().ok().and_then(|slot| slot.clone());
    let Some(installed) = installed else {
        return (packaged_import_factory(library), None);
    };
    let provider = Arc::new(NativeProvider {
        identity: installed.identity,
        copy: installed.copy,
    });
    let sources = Arc::new(ConfiguredSources::new(installed.cache, provider));
    (
        packaged_import_factory_with_sources(library, Arc::clone(&sources)),
        Some(sources),
    )
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "saved_sources_tests.rs"]
mod tests;
