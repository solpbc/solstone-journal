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
    if let Ok(mut slots) = upload_slots().lock() {
        slots.remove(&id);
    }
    if let Ok(serving) = SERVING.lock()
        && let Some(current) = serving.as_ref()
    {
        current.serving.cancel_owner_response(id);
    }
}

/// Stream a request body from `body_fd`. Returns when the upload is admitted.
/// Both descriptors are owned here on every path, including refusal.
///
/// # Safety
/// `request` is readable for `len`. `response_fd` and `body_fd` are exclusively owned.
/// `response_fd` is nonblocking. No journal path is accepted from the page.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_open_stream(
    request: *const u8,
    len: usize,
    response_fd: i32,
    body_fd: i32,
) -> *mut c_char {
    let sink = take_fd(response_fd);
    let body = take_fd(body_fd);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        open_stream(request, len, sink, body)
    }));
    owned_reply(result)
}

#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_owner_poll(id: u64) -> *mut c_char {
    let result = std::panic::catch_unwind(|| poll_upload(id));
    owned_reply(result)
}

/// Delete abandoned bridge copies under `cache`. The journal is the serving root.
/// `live_json` is a JSON array of numeric ids, or null.
///
/// # Safety
/// Each pointer is either null or a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_reclaim(
    cache: *const c_char,
    live_json: *const c_char,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reclaim_bridge(cache, live_json)
    }));
    owned_reply(result)
}

/// Create a bridge directory that no existing copy occupies.
/// The copy is leased to `authority` and is not created when that authority is retired.
///
/// # Safety
/// `cache` is a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_bridge_allocate(
    cache: *const c_char,
    authority: u64,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let cache = unsafe { cache_path(cache) }?;
        if !outcome_board().authority_live(authority) {
            return Err("couldn't be read".into());
        }
        let owned =
            journal_mobile_serving::owner_bridge::allocate_copy(&cache).map_err(bridge_error)?;
        if let Err(error) = outcome_board().note_folder(authority, owned.id()) {
            let _ = journal_mobile_serving::owner_bridge::release_copy(&owned);
            return Err(error.into());
        }
        Ok(json!({"id": owned.id(), "token": owned.token()}))
    }));
    owned_reply(result)
}

/// Write the receipt for a copy this token owns. An existing receipt is not replaced.
///
/// # Safety
/// Each pointer is either null or a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_bridge_commit(
    cache: *const c_char,
    authority: u64,
    id: u64,
    token: *const c_char,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let cache = unsafe { cache_path(cache) }?;
        let token = unsafe { c_text(token) }?;
        outcome_board()
            .authorize_publish(authority, id)
            .map_err(str::to_owned)?;
        let mut owned = journal_mobile_serving::owner_bridge::adopt_copy(&cache, id, &token, false);
        journal_mobile_serving::owner_bridge::commit_copy(&mut owned).map_err(bridge_error)?;
        Ok(json!({"committed": true}))
    }));
    owned_reply(result)
}

/// Delete only the directory and receipt created for this claim token.
///
/// # Safety
/// Each pointer is either null or a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_bridge_release(
    cache: *const c_char,
    authority: u64,
    id: u64,
    token: *const c_char,
    receipt_created: i32,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let cache = unsafe { cache_path(cache) }?;
        let token = unsafe { c_text(token) }?;
        // A retired document can still delete the copy it created. Another
        // document's authority cannot.
        outcome_board()
            .authorize_release(authority, id)
            .map_err(str::to_owned)?;
        let owned = journal_mobile_serving::owner_bridge::adopt_copy(
            &cache,
            id,
            &token,
            receipt_created != 0,
        );
        journal_mobile_serving::owner_bridge::release_copy(&owned).map_err(bridge_error)?;
        outcome_board().forget_folder(authority, id);
        Ok(json!({"released": true}))
    }));
    owned_reply(result)
}

/// Drop the claim after the folder path is published.
///
/// # Safety
/// Each pointer is either null or a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_bridge_disarm(
    cache: *const c_char,
    authority: u64,
    id: u64,
    token: *const c_char,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let cache = unsafe { cache_path(cache) }?;
        let token = unsafe { c_text(token) }?;
        outcome_board()
            .authorize_publish(authority, id)
            .map_err(str::to_owned)?;
        let owned = journal_mobile_serving::owner_bridge::adopt_copy(&cache, id, &token, false);
        journal_mobile_serving::owner_bridge::disarm_copy(&owned).map_err(bridge_error)?;
        outcome_board().forget_folder(authority, id);
        Ok(json!({"disarmed": true}))
    }));
    owned_reply(result)
}

/// # Safety
/// `cache` is either null or a live NUL-terminated UTF-8 string.
unsafe fn cache_path(cache: *const c_char) -> Result<PathBuf, String> {
    if cache.is_null() {
        return Err("couldn't be read".into());
    }
    Ok(PathBuf::from(
        unsafe { CStr::from_ptr(cache) }
            .to_str()
            .map_err(|_| "couldn't be read".to_owned())?,
    ))
}

/// # Safety
/// `text` is either null or a live NUL-terminated UTF-8 string.
unsafe fn c_text(text: *const c_char) -> Result<String, String> {
    if text.is_null() {
        return Err("couldn't be read".into());
    }
    Ok(unsafe { CStr::from_ptr(text) }
        .to_str()
        .map_err(|_| "couldn't be read".to_owned())?
        .to_owned())
}

fn bridge_error(error: std::io::Error) -> String {
    if error.raw_os_error() == Some(28) || error.kind() == std::io::ErrorKind::StorageFull {
        "No space".into()
    } else {
        "couldn't be read".into()
    }
}

type UploadSlot =
    std::sync::mpsc::Receiver<Result<journal_mobile_serving::OwnerResponseHead, String>>;
type UploadSlots = std::sync::Mutex<std::collections::HashMap<u64, UploadSlot>>;

fn upload_slots() -> &'static UploadSlots {
    static SLOTS: std::sync::OnceLock<UploadSlots> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn take_fd(fd: i32) -> Result<std::fs::File, String> {
    if fd < 0 {
        return Err("owner upload pipe required".into());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

fn open_stream(
    request: *const u8,
    len: usize,
    sink: Result<std::fs::File, String>,
    body: Result<std::fs::File, String>,
) -> Result<Value, String> {
    let sink = sink?;
    let body = body?;
    if request.is_null() || len == 0 || len > 384 * 1024 {
        return Err("bounded owner request required".into());
    }
    let request: Value =
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(request, len) })
            .map_err(|error| error.to_string())?;
    let id = request["id"]
        .as_u64()
        .ok_or("owner request identity required")?;
    let headers: Vec<(String, String)> =
        serde_json::from_value(request["headers"].clone()).map_err(|error| error.to_string())?;
    let receiver = {
        let serving = SERVING.lock().map_err(|_| "serving owner unavailable")?;
        let current = serving.as_ref().ok_or("journal is stopped")?;
        let generation = admit_processing(&request, &PROCESSING_GENERATION)?;
        if generation & 1 != 0 || current.serving.stop_handle().is_stopping() {
            return Err("journal is stopping".into());
        }
        current.serving.open_owner_upload(
            id,
            request["method"].as_str().ok_or("method required")?,
            request["uri"].as_str().ok_or("path required")?,
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

fn reclaim_bridge(cache: *const c_char, live_json: *const c_char) -> Result<Value, String> {
    if cache.is_null() {
        return Err("owner bridge path required".into());
    }
    let cache = std::path::Path::new(
        unsafe { CStr::from_ptr(cache) }
            .to_str()
            .map_err(|error| error.to_string())?,
    );
    let journal = {
        let serving = SERVING.lock().map_err(|_| "serving owner unavailable")?;
        let current = serving.as_ref().ok_or("journal is stopped")?;
        current.serving.canonical_journal_path().to_path_buf()
    };
    let mut live = extra_live(live_json)?;
    if let Ok(slots) = upload_slots().lock() {
        live.extend(slots.keys().copied());
    }
    journal_mobile_serving::owner_bridge::reclaim(cache, &journal, &live)?;
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

const OUTCOME_BOUND: usize = 32;
const OUTCOME_REPLY_LIMIT: usize = 12 * 1024 * 1024;

/// Page-scoped upload replies. A completed save stays until finish takes it,
/// the page cancels it, or that document's authority is retired.
///
/// Authorities are issued per document. Retiring one does not retire every
/// lower id, so a bridge that opens again can admit the same page id.
struct OutcomeBoard {
    inner: std::sync::Mutex<OutcomeState>,
}

struct OutcomeState {
    next: u64,
    retired: std::collections::HashSet<u64>,
    slots: std::collections::HashMap<(u64, String), OutcomeSlot>,
    folders: std::collections::HashMap<u64, u64>,
}

enum OutcomeSlot {
    Open,
    Ready { reply: Value },
}

#[derive(Debug)]
enum OutcomeTake {
    Ready(Value),
    Pending,
    Absent,
}

impl Default for OutcomeBoard {
    fn default() -> Self {
        Self {
            inner: std::sync::Mutex::new(OutcomeState {
                next: 1,
                retired: std::collections::HashSet::new(),
                slots: std::collections::HashMap::new(),
                folders: std::collections::HashMap::new(),
            }),
        }
    }
}

impl OutcomeBoard {
    fn lock(&self) -> std::sync::MutexGuard<'_, OutcomeState> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Next document authority. Zero is never issued, and a retired or still
    /// live id is not issued again.
    fn issue(&self) -> u64 {
        let mut state = self.lock();
        loop {
            let id = state.next;
            state.next = state.next.wrapping_add(1);
            if id == 0 {
                continue;
            }
            let used = state.retired.contains(&id)
                || state.slots.keys().any(|key| key.0 == id)
                || state.folders.values().any(|authority| *authority == id);
            if !used {
                return id;
            }
        }
    }

    fn authority_live(&self, authority: u64) -> bool {
        authority != 0 && !self.lock().retired.contains(&authority)
    }

    fn admit(&self, authority: u64, page_id: &str) -> Result<(), &'static str> {
        if authority == 0 || !page_id_ok(page_id) {
            return Err("couldn't be read");
        }
        let mut state = self.lock();
        let key = (authority, page_id.to_owned());
        if state.retired.contains(&authority)
            || state.slots.contains_key(&key)
            || state.slots.len() >= OUTCOME_BOUND
        {
            return Err("busy");
        }
        state.slots.insert(key, OutcomeSlot::Open);
        Ok(())
    }

    fn keep(&self, authority: u64, page_id: &str, reply: &str) -> Result<(), &'static str> {
        if authority == 0 || !page_id_ok(page_id) || reply.len() > OUTCOME_REPLY_LIMIT {
            return Err("couldn't be read");
        }
        let reply: Value = serde_json::from_str(reply).map_err(|_| "couldn't be read")?;
        if !reply.is_object() {
            return Err("couldn't be read");
        }
        let mut state = self.lock();
        if state.retired.contains(&authority) {
            return Err("retired");
        }
        let key = (authority, page_id.to_owned());
        match state.slots.get(&key) {
            Some(OutcomeSlot::Open) => {}
            Some(OutcomeSlot::Ready { .. }) => return Ok(()),
            None => return Err("retired"),
        }
        state.slots.insert(key, OutcomeSlot::Ready { reply });
        Ok(())
    }

    fn take(&self, authority: u64, page_id: &str) -> OutcomeTake {
        if authority == 0 || !page_id_ok(page_id) {
            return OutcomeTake::Absent;
        }
        let mut state = self.lock();
        let key = (authority, page_id.to_owned());
        match state.slots.remove(&key) {
            Some(OutcomeSlot::Ready { reply }) => OutcomeTake::Ready(reply),
            Some(open) => {
                state.slots.insert(key, open);
                OutcomeTake::Pending
            }
            None => OutcomeTake::Absent,
        }
    }

    fn cancel(&self, authority: u64, page_id: &str) {
        if authority != 0 && page_id_ok(page_id) {
            self.lock().slots.remove(&(authority, page_id.to_owned()));
        }
    }

    /// Drop this document's slots. A later authority, including a smaller id
    /// issued by another board, stays admissible on this board.
    fn retire(&self, authority: u64) {
        if authority == 0 {
            return;
        }
        let mut state = self.lock();
        state.retired.insert(authority);
        state.slots.retain(|key, _| key.0 != authority);
    }

    fn note_folder(&self, authority: u64, id: u64) -> Result<(), &'static str> {
        if authority == 0 || id == 0 {
            return Err("couldn't be read");
        }
        let mut state = self.lock();
        if state.retired.contains(&authority) {
            return Err("couldn't be read");
        }
        if state.folders.contains_key(&id) {
            return Err("busy");
        }
        state.folders.insert(id, authority);
        Ok(())
    }

    fn authorize_publish(&self, authority: u64, id: u64) -> Result<(), &'static str> {
        let state = self.lock();
        match state.folders.get(&id) {
            Some(recorded)
                if *recorded == authority
                    && authority != 0
                    && !state.retired.contains(&authority) =>
            {
                Ok(())
            }
            _ => Err("couldn't be read"),
        }
    }

    fn authorize_release(&self, authority: u64, id: u64) -> Result<(), &'static str> {
        let state = self.lock();
        match state.folders.get(&id) {
            Some(recorded) if *recorded == authority && authority != 0 => Ok(()),
            _ => Err("couldn't be read"),
        }
    }

    fn forget_folder(&self, authority: u64, id: u64) {
        let mut state = self.lock();
        if state.folders.get(&id) == Some(&authority) {
            state.folders.remove(&id);
        }
    }
}

fn page_id_ok(page_id: &str) -> bool {
    !page_id.is_empty() && page_id.len() < 64
}

fn outcome_board() -> &'static OutcomeBoard {
    static BOARD: std::sync::OnceLock<OutcomeBoard> = std::sync::OnceLock::new();
    BOARD.get_or_init(OutcomeBoard::default)
}

const OWNER_SCHEME: &str = "journal-owner";
const OWNER_HOST: &str = "local";

/// The script-message sender is the main frame of this owner document.
/// A URL carried in the message body is not this check.
fn sender_admits(main_frame: bool, protocol: &str, host: &str, port: u16) -> bool {
    main_frame && protocol == OWNER_SCHEME && host == OWNER_HOST && port == 0
}

/// Admit one script-message sender. Wrong frame, origin, or port is refused.
///
/// # Safety
/// `protocol` and `host` are null or each point at a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_sender_admit(
    main_frame: bool,
    protocol: *const c_char,
    host: *const c_char,
    port: u16,
) -> bool {
    if protocol.is_null() || host.is_null() {
        return false;
    }
    let Ok(protocol) = unsafe { CStr::from_ptr(protocol) }.to_str() else {
        return false;
    };
    let Ok(host) = unsafe { CStr::from_ptr(host) }.to_str() else {
        return false;
    };
    sender_admits(main_frame, protocol, host, port)
}

/// Issue one document authority. It stays distinct for this process.
#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_owner_authority_issue() -> u64 {
    outcome_board().issue()
}

/// Drop one document's outcome slots. Its folder lease can still be released.
#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_owner_authority_retire(authority: u64) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        outcome_board().retire(authority);
    }));
}

/// Reserve one page slot for a transfer that has not finished.
///
/// # Safety
/// `page_id` is either null or a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_outcome_admit(
    authority: u64,
    page_id: *const c_char,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let page_id = unsafe { c_text(page_id) }?;
        outcome_board()
            .admit(authority, &page_id)
            .map_err(str::to_owned)?;
        Ok(json!({"admitted": true}))
    }));
    owned_reply(result)
}

/// Retain a completed reply until finish, cancel, or this authority's retirement.
/// A second keep for the same open slot does not replace the stored reply.
///
/// # Safety
/// Each pointer is either null or a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_outcome_keep(
    authority: u64,
    page_id: *const c_char,
    reply_json: *const c_char,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let page_id = unsafe { c_text(page_id) }?;
        let reply_json = unsafe { c_text(reply_json) }?;
        outcome_board()
            .keep(authority, &page_id, &reply_json)
            .map_err(str::to_owned)?;
        Ok(json!({"kept": true}))
    }));
    owned_reply(result)
}

/// Consume a completed reply for this authority, or report that it is still open.
///
/// # Safety
/// `page_id` is either null or a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_outcome_take(
    authority: u64,
    page_id: *const c_char,
) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let page_id = unsafe { c_text(page_id) }?;
        let value = match outcome_board().take(authority, &page_id) {
            OutcomeTake::Ready(reply) => json!({"state": "ready", "reply": reply}),
            OutcomeTake::Pending => json!({"state": "pending"}),
            OutcomeTake::Absent => json!({"state": "absent"}),
        };
        Ok(value)
    }));
    owned_reply(result)
}

/// Drop one page slot for this authority. A later keep does not recreate it,
/// and the same page id on another authority stays.
///
/// # Safety
/// `page_id` is either null or a live NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_ios_owner_outcome_cancel(authority: u64, page_id: *const c_char) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Ok(page_id) = unsafe { c_text(page_id) } {
            outcome_board().cancel(authority, &page_id);
        }
    }));
}

#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_owner_next_id() -> u64 {
    journal_mobile_serving::next_owner_request_id()
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
            CString::new(
                r#"{"ok":false,"error":"journal response unavailable; check the result before trying again."}"#,
            )
            .unwrap()
        })
        .into_raw()
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

    #[test]
    fn refused_stream_closes_both_writers() {
        let mut response = [-1; 2];
        let mut body = [-1; 2];
        assert_eq!(unsafe { libc::pipe(response.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::pipe(body.as_mut_ptr()) }, 0);
        let mut response_reader = unsafe { std::fs::File::from_raw_fd(response[0]) };
        let mut body_reader = unsafe { std::fs::File::from_raw_fd(body[0]) };
        let result =
            unsafe { journal_ios_owner_open_stream(std::ptr::null(), 0, response[1], body[1]) };
        let reply: Value =
            serde_json::from_str(unsafe { CStr::from_ptr(result) }.to_str().unwrap()).unwrap();
        unsafe { journal_runtime_free(result) };
        assert_eq!(reply["ok"], false);
        assert_eq!(response_reader.read(&mut [0]).unwrap(), 0);
        assert_eq!(body_reader.read(&mut [0]).unwrap(), 0);
    }
}

#[cfg(test)]
mod outcome_controls {
    use super::*;
    use std::sync::{Arc, mpsc};
    use std::thread;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("owner-authority-{}", journal_ios_owner_next_id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn child(&self, id: u64) -> std::path::PathBuf {
            self.0.join("owner-bridge").join(id.to_string())
        }

        fn receipt(&self, id: u64) -> std::path::PathBuf {
            self.0.join("owner-bridge").join(format!("{id}.receipt"))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn reply(status: u16) -> String {
        json!({"status": status, "body": "AA=="}).to_string()
    }

    #[test]
    fn fast_completion_stays_until_finish_takes_it() {
        let board = OutcomeBoard::default();
        let authority = board.issue();
        board.admit(authority, "save").unwrap();
        assert!(matches!(
            board.take(authority, "save"),
            OutcomeTake::Pending
        ));
        board.keep(authority, "save", &reply(200)).unwrap();
        board
            .keep(authority, "save", r#"{"error":"later"}"#)
            .unwrap();
        match board.take(authority, "save") {
            OutcomeTake::Ready(value) => assert_eq!(value["status"], 200),
            other => panic!("fast completion was dropped: {other:?}"),
        }
        assert!(matches!(board.take(authority, "save"), OutcomeTake::Absent));
    }

    fn ffi_value(pointer: *mut c_char) -> Value {
        assert!(!pointer.is_null());
        let text = unsafe { CStr::from_ptr(pointer) }
            .to_str()
            .unwrap()
            .to_owned();
        unsafe { journal_runtime_free(pointer) };
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn ffi_finish_reads_a_retained_completion() {
        let page = CString::new(format!("ffi-{}", journal_ios_owner_next_id())).unwrap();
        let authority = journal_ios_owner_authority_issue();
        let admitted =
            ffi_value(unsafe { journal_ios_owner_outcome_admit(authority, page.as_ptr()) });
        assert_eq!(admitted["ok"], true);
        let pending =
            ffi_value(unsafe { journal_ios_owner_outcome_take(authority, page.as_ptr()) });
        assert_eq!(pending["value"]["state"], "pending");
        let reply = CString::new(reply(200)).unwrap();
        let kept = ffi_value(unsafe {
            journal_ios_owner_outcome_keep(authority, page.as_ptr(), reply.as_ptr())
        });
        assert_eq!(kept["ok"], true);
        let taken = ffi_value(unsafe { journal_ios_owner_outcome_take(authority, page.as_ptr()) });
        assert_eq!(taken["value"]["state"], "ready");
        assert_eq!(taken["value"]["reply"]["status"], 200);
        unsafe { journal_ios_owner_outcome_cancel(authority, page.as_ptr()) };
    }

    #[test]
    fn ffi_reopen_keeps_a_late_take_off_the_new_document() {
        let page = CString::new(format!("reopen-{}", journal_ios_owner_next_id())).unwrap();
        let first = journal_ios_owner_authority_issue();
        assert_ne!(first, 0);
        let admitted = ffi_value(unsafe { journal_ios_owner_outcome_admit(first, page.as_ptr()) });
        assert_eq!(admitted["ok"], true);
        let first_reply = CString::new(reply(200)).unwrap();
        let kept = ffi_value(unsafe {
            journal_ios_owner_outcome_keep(first, page.as_ptr(), first_reply.as_ptr())
        });
        assert_eq!(kept["ok"], true);
        journal_ios_owner_authority_retire(first);
        let second = journal_ios_owner_authority_issue();
        assert_ne!(second, first);
        assert_ne!(second, 0);
        let reopened = ffi_value(unsafe { journal_ios_owner_outcome_admit(second, page.as_ptr()) });
        assert_eq!(reopened["ok"], true);
        let second_reply = CString::new(reply(201)).unwrap();
        let kept = ffi_value(unsafe {
            journal_ios_owner_outcome_keep(second, page.as_ptr(), second_reply.as_ptr())
        });
        assert_eq!(kept["ok"], true);
        let late = ffi_value(unsafe { journal_ios_owner_outcome_take(first, page.as_ptr()) });
        assert_eq!(late["value"]["state"], "absent");
        unsafe { journal_ios_owner_outcome_cancel(first, page.as_ptr()) };
        let taken = ffi_value(unsafe { journal_ios_owner_outcome_take(second, page.as_ptr()) });
        assert_eq!(taken["value"]["state"], "ready");
        assert_eq!(taken["value"]["reply"]["status"], 201);
        let refused = ffi_value(unsafe { journal_ios_owner_outcome_admit(0, page.as_ptr()) });
        assert_eq!(refused["ok"], false);
    }

    #[test]
    fn error_before_finish_stays_until_finish_takes_it() {
        let board = OutcomeBoard::default();
        let authority = board.issue();
        board.admit(authority, "save").unwrap();
        board
            .keep(
                authority,
                "save",
                r#"{"error":"journal response unavailable; check the result before trying again"}"#,
            )
            .unwrap();
        match board.take(authority, "save") {
            OutcomeTake::Ready(value) => {
                assert_eq!(
                    value["error"],
                    "journal response unavailable; check the result before trying again"
                );
            }
            other => panic!("error before finish was dropped: {other:?}"),
        }
    }

    #[test]
    fn cancel_and_retirement_drop_only_that_outcome() {
        let board = OutcomeBoard::default();
        let authority = board.issue();
        board.admit(authority, "read").unwrap();
        board.admit(authority, "upload").unwrap();
        board.keep(authority, "upload", &reply(201)).unwrap();
        board.cancel(authority, "read");
        assert!(board.keep(authority, "read", &reply(200)).is_err());
        assert!(matches!(board.take(authority, "read"), OutcomeTake::Absent));
        match board.take(authority, "upload") {
            OutcomeTake::Ready(value) => assert_eq!(value["status"], 201),
            other => panic!("cancel crossed uploads: {other:?}"),
        }

        let board = OutcomeBoard::default();
        let old = board.issue();
        let current = board.issue();
        board.admit(old, "page").unwrap();
        board.admit(current, "page").unwrap();
        board.keep(old, "page", &reply(200)).unwrap();
        board.keep(current, "page", &reply(202)).unwrap();
        board.retire(old);
        assert!(matches!(board.take(old, "page"), OutcomeTake::Absent));
        assert!(board.admit(old, "late").is_err());
        assert!(board.keep(old, "late", &reply(200)).is_err());
        match board.take(current, "page") {
            OutcomeTake::Ready(value) => assert_eq!(value["status"], 202),
            other => panic!("authority retirement crossed a later copy: {other:?}"),
        }
    }

    #[test]
    fn close_and_reload_leave_the_new_page_reply() {
        let board = OutcomeBoard::default();
        let closed = board.issue();
        assert_ne!(closed, 0);
        board.admit(closed, "1").unwrap();
        board.keep(closed, "1", &reply(200)).unwrap();
        board.retire(closed);
        let reopened = board.issue();
        assert_ne!(reopened, closed);
        assert_ne!(reopened, 0);
        board.admit(reopened, "1").unwrap();
        board.keep(reopened, "1", &reply(201)).unwrap();
        assert!(board.keep(closed, "1", &reply(500)).is_err());
        assert!(matches!(board.take(closed, "1"), OutcomeTake::Absent));
        board.cancel(closed, "1");
        match board.take(reopened, "1") {
            OutcomeTake::Ready(value) => assert_eq!(value["status"], 201),
            other => panic!("a reloaded page lost its reply: {other:?}"),
        }
        board.retire(reopened);
        let again = board.issue();
        assert_ne!(again, 0);
        assert_ne!(again, closed);
        assert_ne!(again, reopened);
        board.admit(again, "1").unwrap();
        assert!(board.admit(0, "1").is_err());
    }

    #[test]
    fn sender_admission_rejects_wrong_frame_origin_and_stale_completion() {
        assert!(sender_admits(true, OWNER_SCHEME, OWNER_HOST, 0));
        assert!(!sender_admits(false, "journal-owner", "local", 0));
        assert!(!sender_admits(true, "https", "local", 0));
        assert!(!sender_admits(true, "journal-owner", "evil", 0));
        assert!(!sender_admits(true, "journal-owner", "local", 443));
        assert!(!sender_admits(true, "journal-owner", "", 0));
        let scheme = CString::new("journal-owner").unwrap();
        let host = CString::new("local").unwrap();
        let https = CString::new("https").unwrap();
        assert!(unsafe { journal_ios_owner_sender_admit(true, scheme.as_ptr(), host.as_ptr(), 0) });
        assert!(!unsafe {
            journal_ios_owner_sender_admit(false, scheme.as_ptr(), host.as_ptr(), 0)
        });
        assert!(!unsafe { journal_ios_owner_sender_admit(true, https.as_ptr(), host.as_ptr(), 0) });
        assert!(!unsafe {
            journal_ios_owner_sender_admit(true, std::ptr::null(), host.as_ptr(), 0)
        });
        assert!(!unsafe {
            journal_ios_owner_sender_admit(true, scheme.as_ptr(), std::ptr::null(), 0)
        });

        let board = OutcomeBoard::default();
        let old = board.issue();
        let current = board.issue();
        assert_ne!(old, 0);
        assert_ne!(current, old);
        board.admit(old, "1").unwrap();
        board.keep(old, "1", &reply(200)).unwrap();
        board.retire(old);
        board.admit(current, "1").unwrap();
        board.keep(current, "1", &reply(201)).unwrap();
        assert!(matches!(board.take(old, "1"), OutcomeTake::Absent));
        board.cancel(old, "1");
        match board.take(current, "1") {
            OutcomeTake::Ready(value) => assert_eq!(value["status"], 201),
            other => panic!("stale completion consumed the new document: {other:?}"),
        }
    }

    #[test]
    fn completed_outcomes_count_toward_the_open_bound() {
        let board = OutcomeBoard::default();
        let authority = board.issue();
        for index in 0..OUTCOME_BOUND {
            board.admit(authority, &format!("page-{index}")).unwrap();
            board
                .keep(authority, &format!("page-{index}"), &reply(200))
                .unwrap();
        }
        assert!(board.admit(authority, "overflow").is_err());
        assert!(matches!(
            board.take(authority, "page-0"),
            OutcomeTake::Ready(_)
        ));
        board.admit(authority, "overflow").unwrap();
        board.keep(authority, "overflow", &reply(204)).unwrap();
        match board.take(authority, "overflow") {
            OutcomeTake::Ready(value) => assert_eq!(value["status"], 204),
            other => panic!("a freed slot did not accept the next save: {other:?}"),
        }
    }

    #[test]
    fn concurrent_page_outcomes_do_not_cross() {
        let board = Arc::new(OutcomeBoard::default());
        let authority = board.issue();
        board.admit(authority, "left").unwrap();
        board.admit(authority, "right").unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let mut joins = Vec::new();
        for (page, status) in [("left", 200u16), ("right", 201u16)] {
            let board = Arc::clone(&board);
            let barrier = Arc::clone(&barrier);
            joins.push(thread::spawn(move || {
                barrier.wait();
                board.keep(authority, page, &reply(status)).unwrap();
            }));
        }
        for join in joins {
            join.join().unwrap();
        }
        match board.take(authority, "left") {
            OutcomeTake::Ready(value) => assert_eq!(value["status"], 200),
            other => panic!("left outcome crossed: {other:?}"),
        }
        match board.take(authority, "right") {
            OutcomeTake::Ready(value) => assert_eq!(value["status"], 201),
            other => panic!("right outcome crossed: {other:?}"),
        }
    }

    #[test]
    fn retired_folder_authority_cannot_publish_or_release_the_new_copy() {
        let scratch = Scratch::new();
        let cache = CString::new(scratch.0.to_str().unwrap()).unwrap();
        let first = journal_ios_owner_authority_issue();
        let allocated =
            ffi_value(unsafe { journal_ios_owner_bridge_allocate(cache.as_ptr(), first) });
        assert_eq!(allocated["ok"], true, "{allocated}");
        let id_a = allocated["value"]["id"].as_u64().unwrap();
        let token_a = CString::new(allocated["value"]["token"].as_str().unwrap()).unwrap();
        journal_ios_owner_authority_retire(first);
        let second = journal_ios_owner_authority_issue();
        assert_ne!(second, first);
        assert_ne!(second, 0);
        let allocated =
            ffi_value(unsafe { journal_ios_owner_bridge_allocate(cache.as_ptr(), second) });
        assert_eq!(allocated["ok"], true, "{allocated}");
        let id_b = allocated["value"]["id"].as_u64().unwrap();
        let token_b = CString::new(allocated["value"]["token"].as_str().unwrap()).unwrap();
        assert_ne!(id_a, id_b);
        let dir_a = scratch.child(id_a);
        let dir_b = scratch.child(id_b);
        std::fs::write(dir_a.join("note.txt"), b"old-bytes").unwrap();
        std::fs::write(dir_b.join("note.txt"), b"new-bytes").unwrap();
        let committed = ffi_value(unsafe {
            journal_ios_owner_bridge_commit(cache.as_ptr(), first, id_a, token_a.as_ptr())
        });
        assert_eq!(committed["ok"], false, "{committed}");
        let disarmed = ffi_value(unsafe {
            journal_ios_owner_bridge_disarm(cache.as_ptr(), first, id_a, token_a.as_ptr())
        });
        assert_eq!(disarmed["ok"], false, "{disarmed}");
        assert!(!scratch.receipt(id_a).exists());
        assert_eq!(std::fs::read(dir_a.join("note.txt")).unwrap(), b"old-bytes");
        let crossed = ffi_value(unsafe {
            journal_ios_owner_bridge_release(cache.as_ptr(), second, id_a, token_a.as_ptr(), 0)
        });
        assert_eq!(crossed["ok"], false, "{crossed}");
        assert_eq!(std::fs::read(dir_a.join("note.txt")).unwrap(), b"old-bytes");
        assert_eq!(std::fs::read(dir_b.join("note.txt")).unwrap(), b"new-bytes");
        let released = ffi_value(unsafe {
            journal_ios_owner_bridge_release(cache.as_ptr(), first, id_a, token_a.as_ptr(), 0)
        });
        assert_eq!(released["ok"], true, "{released}");
        assert!(!dir_a.exists());
        assert_eq!(std::fs::read(dir_b.join("note.txt")).unwrap(), b"new-bytes");
        journal_ios_owner_authority_retire(second);
        let committed = ffi_value(unsafe {
            journal_ios_owner_bridge_commit(cache.as_ptr(), second, id_b, token_b.as_ptr())
        });
        assert_eq!(committed["ok"], false, "{committed}");
        assert!(!scratch.receipt(id_b).exists());
        assert_eq!(std::fs::read(dir_b.join("note.txt")).unwrap(), b"new-bytes");
        let released = ffi_value(unsafe {
            journal_ios_owner_bridge_release(cache.as_ptr(), second, id_b, token_b.as_ptr(), 0)
        });
        assert_eq!(released["ok"], true, "{released}");
        assert!(!dir_b.exists());
    }

    #[test]
    fn cancelling_a_read_identity_leaves_the_upload_slot() {
        let read_id = journal_ios_owner_next_id();
        let upload_id = journal_ios_owner_next_id();
        assert_ne!(read_id, upload_id);
        let (read_tx, read_rx) = mpsc::channel();
        let (upload_tx, upload_rx) = mpsc::channel();
        {
            let mut slots = upload_slots().lock().unwrap();
            slots.insert(read_id, read_rx);
            slots.insert(upload_id, upload_rx);
        }
        journal_ios_owner_cancel(read_id);
        {
            let slots = upload_slots().lock().unwrap();
            assert!(!slots.contains_key(&read_id));
            assert!(slots.contains_key(&upload_id));
        }
        journal_ios_owner_cancel(upload_id);
        assert!(!upload_slots().lock().unwrap().contains_key(&upload_id));
        drop((read_tx, upload_tx));
    }
}
