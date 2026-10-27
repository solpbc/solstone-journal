// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private Service connection changes. Permissions and state retire in one frame.

use std::ffi::{CString, c_char, c_void};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, TryLockError};

use journal_mobile_serving::import_route::{FolderConnectionChange, SourceConnectionError};
use serde_json::{Value, json};
use solstone_core_import::{
    BackendName, PublicConnection, SyncStateRead, WorkStop, WorkStopGuard,
    public_connection_status, read_sync_state_bound,
};

// Android persisted URI grants are app-wide. Serialize only their configuration
// transitions; original-root custody still comes solely from ImportPreparation.
static CHANGES: Mutex<()> = Mutex::new(());
type Permission = unsafe extern "C" fn(*mut c_void, i32, *const c_char) -> i32;

struct Grant<'a> {
    context: *mut c_void,
    callback: Permission,
    material: &'a str,
    retain: bool,
    retired: bool,
}
impl Grant<'_> {
    fn call(&self, action: i32, material: &str) -> i32 {
        let Ok(material) = CString::new(material) else {
            return -1;
        };
        unsafe { (self.callback)(self.context, action, material.as_ptr()) }
    }
    fn rollback(&mut self) -> bool {
        if self.retain || self.retired {
            return true;
        }
        self.retired = self.call(1, self.material) == 0;
        self.retired
    }
}
impl Drop for Grant<'_> {
    fn drop(&mut self) {
        self.rollback();
    }
}

fn public(value: &PublicConnection) -> Value {
    json!({"source_identity":value.source_identity,"folder_name":value.folder_name,
        "remembered":value.remembered,"access_state":value.access_state.to_string()})
}

fn existing(
    root: &solstone_core_journal_io::JournalRoot,
    backend: BackendName,
) -> Result<(Option<PublicConnection>, Option<String>), ()> {
    match read_sync_state_bound(root, backend) {
        SyncStateRead::Absent => Ok((None, None)),
        SyncStateRead::Loaded(state) => Ok((
            public_connection_status(&state).map_err(|_| ())?,
            state
                .root()
                .get("access_material")
                .and_then(Value::as_str)
                .map(str::to_owned),
        )),
        SyncStateRead::Unreadable { .. } => Err(()),
    }
}

/// # Safety
/// The request is private Service JSON. Grant material and display name come
/// from its actual picker result, never a document-supplied URI. Callback/context
/// stay valid on this original calling worker until return and must not detach
/// permission operations or retain pointers. Callback phases match the private
/// SourcePermissionTransition: acquire, rollback new, release old, inspect.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_owner_source_change(
    request: *const u8,
    length: usize,
    context: *mut c_void,
    permission: Option<Permission>,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        change(request, length, context, permission)
    }));
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(())) => {
            json!({"ok":false,"state":"refused","reason_code":"source_configuration_unavailable"})
        }
        Err(_) => {
            json!({"ok":false,"state":"outcome_unknown","reason_code":"source_configuration_unconfirmed"})
        }
    };
    CString::new(value.to_string()).unwrap().into_raw()
}

unsafe fn change(
    request: *const u8,
    length: usize,
    context: *mut c_void,
    permission: Option<Permission>,
) -> Result<Value, ()> {
    if request.is_null() || context.is_null() || length == 0 || length > 32 * 1024 {
        return Err(());
    }
    let request: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(request, length) })
            .map_err(|_| ())?;
    let callback = permission.ok_or(())?;
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
    let material = if action == "forget" {
        ""
    } else {
        request["access_material"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(())?
    };
    let name = if action == "forget" {
        ""
    } else {
        request["folder_name"].as_str().ok_or(())?
    };
    let page = request["page_authority"].as_u64().ok_or(())?;
    let (preparation, root, sources) = {
        let serving = crate::SERVING.lock().map_err(|_| ())?;
        let owner = serving.as_ref().ok_or(())?;
        let generation =
            crate::admitted_generation(&request, &crate::THINKING_GENERATION).map_err(|_| ())?;
        if generation & 1 != 0
            || owner.handle().map_err(|_| ())?.stop_handle().is_stopping()
            || !crate::owner_folder::page_live(page, generation)
        {
            return Err(());
        }
        let stop =
            WorkStop::linked_with_checkpoint(Arc::new(AtomicBool::new(false)), None, move || {
                !crate::owner_folder::page_live(page, generation)
            });
        let preparation = owner
            .host
            .as_ref()
            .ok_or(())?
            .import_preparation(stop, Default::default())
            .map_err(|_| ())?;
        (
            preparation,
            Arc::clone(&owner.root),
            Arc::clone(owner.sources.as_ref().ok_or(())?),
        )
    };
    preparation.checkpoint().map_err(|_| ())?;
    let _change = match CHANGES.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => return Ok(json!({"ok":false,"state":"busy"})),
        Err(TryLockError::Poisoned(_)) => return Err(()),
    };
    let mut guard = match sources.connection_guard(backend) {
        Ok(guard) => guard,
        Err(SourceConnectionError::Busy) => return Ok(json!({"ok":false,"state":"busy"})),
        Err(_) => return Err(()),
    };
    let (current, old_material) = existing(&root, backend)?;
    if (action == "begin" && current.is_some())
        || (action != "begin"
            && expected.is_none_or(|id| {
                current
                    .as_ref()
                    .is_none_or(|current| current.source_identity != id)
            }))
    {
        return Err(());
    }
    preparation.checkpoint().map_err(|_| ())?;
    let _installed = WorkStopGuard::install(preparation.stop());
    let mut grant = Grant {
        context,
        callback,
        material,
        retain: false,
        retired: false,
    };
    if action != "forget" && !matches!(grant.call(0, material), 0 | 1) {
        return Ok(if grant.rollback() {
            json!({"ok":false,"state":"refused"})
        } else {
            json!({"ok":false,"state":"outcome_unknown","permissions":"unknown"})
        });
    }
    let change = match action {
        "begin" => FolderConnectionChange::Begin {
            folder_name: name.into(),
            access_material: material.into(),
        },
        "reauthorize" => FolderConnectionChange::Reauthorize {
            expected_identity: expected.ok_or(())?.into(),
            folder_name: name.into(),
            access_material: material.into(),
        },
        "replace" => FolderConnectionChange::Replace {
            expected_identity: expected.ok_or(())?.into(),
            folder_name: name.into(),
            access_material: material.into(),
        },
        "forget" => FolderConnectionChange::Forget {
            expected_identity: expected.ok_or(())?.into(),
        },
        _ => unreachable!(),
    };
    // A panic or an uncertain publication may already have committed state.
    // Retain both OS grants until real state can be read, never revoke the new
    // connection merely because its write acknowledgement was lost.
    grant.retain = true;
    let connection = match guard.update(&root, preparation.stop(), change) {
        Ok(connection) => connection,
        Err(SourceConnectionError::Refused | SourceConnectionError::Busy) => {
            grant.retain = false;
            return Ok(if grant.rollback() {
                json!({"ok":false,"state":"refused"})
            } else {
                json!({"ok":false,"state":"outcome_unknown","permissions":"unknown"})
            });
        }
        Err(SourceConnectionError::Unconfirmed) => {
            return Ok(
                json!({"ok":false,"state":"outcome_unknown","configuration":"unknown","permissions":"retained"}),
            );
        }
    };
    let mut permission_state = "unchanged";
    if let Some(old) = old_material.filter(|old| old != material) {
        let other = if backend == BackendName::Obsidian {
            BackendName::Audio
        } else {
            BackendName::Obsidian
        };
        permission_state = match existing(&root, other) {
            Ok((_, Some(other))) if other == old => "retained_for_another_source",
            Ok(_) => {
                if grant.call(2, &old) == 0 {
                    "released"
                } else {
                    "unknown"
                }
            }
            Err(_) => "unknown",
        };
    }
    // Original preparation, source guard and app grant serialization remain
    // held through the real permission-return/cleanup, even after client loss.
    Ok(json!({"ok":permission_state != "unknown",
        "state":if permission_state == "unknown" {"outcome_unknown"} else {"updated"},
        "configuration":"updated","connection":public(&connection),"permissions":permission_state}))
}
