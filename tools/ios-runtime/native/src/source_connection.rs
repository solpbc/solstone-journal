// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bookmark creation and source configuration under the same original use.

use journal_mobile_serving::import_route::{FolderConnectionChange, SourceConnectionError};
use serde_json::{Value, json};
use solstone_core_import::{
    BackendName, SyncStateRead, WorkStop, WorkStopGuard, public_connection_status,
    read_sync_state_bound,
};
use std::ffi::{CString, c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

type Bookmark = unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> isize;

/// # Safety
/// Request/context/callback belong to one synchronous private picker worker.
/// Callback creates bookmark bytes from its actual picked URL, never document
/// data. It closes its security scope and all reads before return, checks the
/// installed full Stop, writes at most capacity bytes, and retains no pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_source_change(
    request: *const u8,
    length: usize,
    context: *mut c_void,
    bookmark: Option<Bookmark>,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        change(request, length, context, bookmark)
    }));
    let reply = match result {
        Ok(Ok(value)) => value,
        Ok(Err(())) => json!({"ok":false,"state":"refused"}),
        Err(_) => json!({"ok":false,"state":"outcome_unknown"}),
    };
    CString::new(reply.to_string()).unwrap().into_raw()
}

unsafe fn change(
    request: *const u8,
    length: usize,
    context: *mut c_void,
    bookmark: Option<Bookmark>,
) -> Result<Value, ()> {
    if request.is_null() || length == 0 || length > 32 * 1024 {
        return Err(());
    }
    let request: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(request, length) })
            .map_err(|_| ())?;
    let backend = match request["source"].as_str() {
        Some("obsidian") => BackendName::Obsidian,
        Some("audio") => BackendName::Audio,
        _ => return Err(()),
    };
    let action = request["action"].as_str().ok_or(())?;
    if !matches!(action, "begin" | "reauthorize" | "replace" | "forget") {
        return Err(());
    }
    let expected = request["expected_identity"].as_str();
    let authority = request["authority"]
        .as_u64()
        .filter(|value| *value != 0)
        .ok_or(())?;
    let (preparation, root, sources) = {
        let serving = super::super::SERVING.lock().map_err(|_| ())?;
        let current = serving.as_ref().ok_or(())?;
        let generation =
            super::super::admit_processing(&request, &super::super::PROCESSING_GENERATION)
                .map_err(|_| ())?;
        if generation & 1 != 0
            || current.serving.stop_handle().is_stopping()
            || !super::super::owner_response::owner_authority_live(authority)
        {
            return Err(());
        }
        let stop =
            WorkStop::linked_with_checkpoint(Arc::new(AtomicBool::new(false)), None, move || {
                !super::super::owner_response::owner_authority_live(authority)
            });
        let preparation = current
            .processing
            .as_ref()
            .ok_or(())?
            .import_preparation(stop, Default::default())
            .map_err(|_| ())?;
        let root = current
            .serving
            .backup_dependencies()
            .retained_root()
            .map_err(|_| ())?;
        (
            preparation,
            root,
            Arc::clone(current.sources.as_ref().ok_or(())?),
        )
    };
    preparation.checkpoint().map_err(|_| ())?;
    let mut guard = match sources.connection_guard(backend) {
        Ok(guard) => guard,
        Err(SourceConnectionError::Busy) => return Ok(json!({"ok":false,"state":"busy"})),
        Err(_) => return Err(()),
    };
    let current = match read_sync_state_bound(&root, backend) {
        SyncStateRead::Absent => None,
        SyncStateRead::Loaded(state) => public_connection_status(&state).map_err(|_| ())?,
        SyncStateRead::Unreadable { .. } => return Err(()),
    };
    if (action == "begin" && current.is_some())
        || (action != "begin"
            && expected.is_none_or(|id| {
                current
                    .as_ref()
                    .is_none_or(|value| value.source_identity != id)
            }))
    {
        return Err(());
    }
    let _installed = WorkStopGuard::install(preparation.stop());
    let material = if action == "forget" {
        String::new()
    } else {
        if context.is_null() {
            return Err(());
        }
        let callback = bookmark.ok_or(())?;
        let mut bytes = vec![0_u8; 64 * 1024];
        let count = unsafe { callback(context, bytes.as_mut_ptr(), bytes.len()) };
        let count = usize::try_from(count).map_err(|_| ())?;
        if count == 0 || count > bytes.len() {
            return Err(());
        }
        bytes.truncate(count);
        let material = String::from_utf8(bytes).map_err(|_| ())?;
        if !material
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'+' | b'/' | b'='))
        {
            return Err(());
        }
        material
    };
    preparation.checkpoint().map_err(|_| ())?;
    let name = request["folder_name"].as_str().unwrap_or("");
    let change = match action {
        "begin" => FolderConnectionChange::Begin {
            folder_name: name.into(),
            access_material: material,
        },
        "reauthorize" => FolderConnectionChange::Reauthorize {
            expected_identity: expected.ok_or(())?.into(),
            folder_name: name.into(),
            access_material: material,
        },
        "replace" => FolderConnectionChange::Replace {
            expected_identity: expected.ok_or(())?.into(),
            folder_name: name.into(),
            access_material: material,
        },
        "forget" => FolderConnectionChange::Forget {
            expected_identity: expected.ok_or(())?.into(),
        },
        _ => unreachable!(),
    };
    Ok(match guard.update(&root, preparation.stop(), change) {
        Ok(connection) => {
            json!({"ok":true,"state":"updated","permissions":"unchanged","connection":{"source_identity":connection.source_identity,"folder_name":connection.folder_name,"remembered":connection.remembered,"access_state":connection.access_state.to_string()}})
        }
        Err(SourceConnectionError::Unconfirmed) => {
            json!({"ok":false,"state":"outcome_unknown","configuration":"unknown"})
        }
        Err(SourceConnectionError::Refused | SourceConnectionError::Busy) => {
            json!({"ok":false,"state":"refused"})
        }
    })
}
