// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Receiving facts for explicit offload requests, never execution authority.
//! Immutable root-level leaves share the retained root's durability boundary.

use super::*;
use axum::body::Bytes;
use axum::extract::Path;
use serde::Serialize;

const PREFIX: &str = "owner-offload-request-";

pub(super) enum Request {
    Submit(String),
    Status(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Submit {
    request_id: String,
}

pub(super) fn parse(body: &Bytes) -> Result<Request, ()> {
    let request: Submit = serde_json::from_slice(body).map_err(|_| ())?;
    if !valid_id(&request.request_id) {
        return Err(());
    }
    Ok(Request::Submit(request.request_id))
}

pub(super) async fn status(
    State(sender): State<OwnerIngress>,
    Path((routine, request_id)): Path<(String, String)>,
) -> Response {
    if routine != "backup:offload" || !valid_id(&request_id) {
        return invalid();
    }
    submit_operation(sender, Operation::Offload(Request::Status(request_id))).await
}

pub(super) fn invalid() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"offload_request_invalid"})),
    )
        .into_response()
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

fn reference(id: &str) -> String {
    format!("{PREFIX}{id}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Received,
    Accepted,
    Result,
}

impl Phase {
    fn suffix(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Accepted => "accepted",
            Self::Result => "result",
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u8,
    request_id: String,
    reference: String,
    phase: Phase,
    exit_code: Option<i32>,
}

impl Receipt {
    fn new(id: &str, phase: Phase, exit_code: Option<i32>) -> Self {
        Self {
            version: 1,
            request_id: id.into(),
            reference: reference(id),
            phase,
            exit_code,
        }
    }
    fn name(&self) -> String {
        format!(
            ".owner-offload-{}.{}.json",
            self.request_id,
            self.phase.suffix()
        )
    }
}

#[cfg(unix)]
fn read(root: &JournalRoot, id: &str, phase: Phase) -> Result<Option<Receipt>, ()> {
    use solstone_core_journal_io::flat_directory::{FlatDirectory, read_observed_file_bounded};
    root.revalidate_canonical_binding().map_err(|_| ())?;
    let directory = FlatDirectory::from_root(root).map_err(|_| ())?;
    let expected = Receipt::new(id, phase, None);
    let observed =
        read_observed_file_bounded(&directory, std::ffi::OsStr::new(&expected.name()), 1024)
            .map_err(|_| ())?;
    let receipt = observed
        .map(|observed| {
            let receipt: Receipt = serde_json::from_slice(&observed.bytes).map_err(|_| ())?;
            if receipt.version != 1
                || receipt.request_id != id
                || receipt.reference != reference(id)
                || receipt.phase != phase
                || (phase == Phase::Result) != receipt.exit_code.is_some()
            {
                return Err(());
            }
            Ok(receipt)
        })
        .transpose()?;
    root.revalidate_canonical_binding().map_err(|_| ())?;
    Ok(receipt)
}

#[cfg(not(unix))]
fn read(_root: &JournalRoot, _id: &str, _phase: Phase) -> Result<Option<Receipt>, ()> {
    Err(())
}

#[cfg(unix)]
fn publish(root: &JournalRoot, receipt: &Receipt) -> Result<(), ()> {
    use solstone_core_journal_io::atomic::write_bytes_exclusive_bound_detailed_checked;
    use solstone_core_journal_io::flat_directory::FlatDirectory;
    let checkpoint = || {
        root.revalidate_canonical_binding()
            .map_err(|error| error.to_string())
    };
    checkpoint().map_err(|_| ())?;
    let directory = FlatDirectory::from_root(root).map_err(|_| ())?;
    let bytes = serde_json::to_vec(receipt).map_err(|_| ())?;
    let write = || {
        write_bytes_exclusive_bound_detailed_checked(
            &directory,
            std::ffi::OsStr::new(&receipt.name()),
            &bytes,
            0o600,
            &checkpoint,
        )
    };
    #[cfg(all(test, feature = "full-tests"))]
    let publication = match PUBLICATION_FAULT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .filter(|fault| fault.root == root.canonical_path() && receipt.phase == Phase::Received)
    {
        Some(fault) => {
            let (result, fired) =
                solstone_core_journal_io::atomic::run_with_bound_publication_fault(
                    fault.primitive,
                    1,
                    5,
                    write,
                );
            fault.fired.store(fired, Ordering::Release);
            result
        }
        None => write(),
    };
    #[cfg(not(all(test, feature = "full-tests")))]
    let publication = write();
    let publication = publication.map_err(|_| ())?;
    if !publication.is_fully_confirmed() {
        return Err(());
    }
    checkpoint().map_err(|_| ())
}

#[cfg(not(unix))]
fn publish(_root: &JournalRoot, _receipt: &Receipt) -> Result<(), ()> {
    Err(())
}

fn unavailable() -> (StatusCode, Value) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error":"offload_receiving_unavailable","reason_code":"receiving_record_unavailable"}),
    )
}

fn observed(root: &JournalRoot, id: &str) -> Result<Value, ()> {
    let received = read(root, id, Phase::Received)?;
    let accepted = read(root, id, Phase::Accepted)?;
    let result = read(root, id, Phase::Result)?;
    if received.is_none() && (accepted.is_some() || result.is_some()) {
        return Err(());
    }
    // A completion can win the race with acceptance publication, or survive
    // its failure. Only its fully parsed actual exit code is a terminal fact.
    let (state, admitted, terminal, exit_code) = if let Some(result) = result {
        ("finished", Some(true), true, result.exit_code)
    } else if accepted.is_some() {
        let state = crate::foreground::owner_offload_state(root, &reference(id))
            .unwrap_or(None)
            .unwrap_or("outcome_unknown");
        (state, Some(true), false, None)
    } else if received.is_some() {
        ("admission_unknown", None, false, None)
    } else {
        ("unknown", Some(false), false, None)
    };
    root.revalidate_canonical_binding().map_err(|_| ())?;
    Ok(
        json!({"request_id":id,"reference":reference(id),"state":state,
        "accepted":admitted,"terminal":terminal,"exit_code":exit_code}),
    )
}

pub(super) fn execute(
    root: &Arc<JournalRoot>,
    request: Request,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> (StatusCode, Value) {
    let (id, submit) = match request {
        Request::Submit(id) => (id, true),
        Request::Status(id) => (id, false),
    };
    if !valid_id(&id) {
        return (
            StatusCode::BAD_REQUEST,
            json!({"error":"offload_request_invalid"}),
        );
    }
    if cancelled() || root.revalidate_canonical_binding().is_err() {
        return unavailable();
    }
    let before = match observed(root, &id) {
        Ok(value) => value,
        Err(()) => return unavailable(),
    };
    if !submit {
        return (StatusCode::OK, before);
    }
    if before["state"] != "unknown" {
        let mut value = before;
        value["reused"] = json!(true);
        return (StatusCode::ACCEPTED, value);
    }
    // No queue authority exists until this no-replace receipt and its root
    // directory are durably confirmed. Any uncertain publication is retained.
    if publish(root, &Receipt::new(&id, Phase::Received, None)).is_err() {
        return unavailable();
    }
    if cancelled() {
        return unavailable();
    }
    if crate::foreground::submit_owner_offload(root, reference(&id)).is_err() {
        return unavailable();
    }
    #[cfg(all(test, feature = "full-tests"))]
    if after_submit(root, &id).is_err() {
        return unavailable();
    }
    // HTTP loss/Stop after queue admission must not erase that fact. This
    // publication revalidates the original root but does not redispatch work.
    if publish(root, &Receipt::new(&id, Phase::Accepted, None)).is_err() {
        return unavailable();
    }
    match observed(root, &id) {
        Ok(mut value) => {
            value["reused"] = json!(false);
            (StatusCode::ACCEPTED, value)
        }
        Err(()) => unavailable(),
    }
}

/// The existing queue fans Stopped out to every coalesced reference. Retain
/// these completion facts even during Stop; never reopen a pathname as owner.
pub(crate) fn record_completion(
    root: &JournalRoot,
    event: &solstone_core_system::queue::TaskQueueEvent,
) {
    use solstone_core_system::{partition::canonical_journal_command, queue::TaskQueueEvent};
    let TaskQueueEvent::Stopped {
        reference,
        command,
        exit_code,
        ..
    } = event
    else {
        return;
    };
    if *command != canonical_journal_command(["maintenance", "run", "backup:offload"]) {
        return;
    }
    let Some(id) = reference.strip_prefix(PREFIX).filter(|id| valid_id(id)) else {
        return;
    };
    let result = (|| {
        if read(root, id, Phase::Received)?.is_none() {
            return Err(());
        }
        let expected = Receipt::new(id, Phase::Result, Some(*exit_code));
        if let Some(existing) = read(root, id, Phase::Result)? {
            return if existing == expected {
                Ok(())
            } else {
                Err(())
            };
        }
        publish(root, &expected)
    })();
    if result.is_err() {
        log::warn!("offload receiving completion record unavailable; outcome remains unknown");
    }
    #[cfg(all(test, feature = "full-tests"))]
    if result.is_ok() {
        let hook = AFTER_RESULT
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(hook) = hook {
            hook(root, id);
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
type AfterSubmit = Arc<dyn Fn(&JournalRoot, &str) -> Result<(), ()> + Send + Sync>;
#[cfg(all(test, feature = "full-tests"))]
pub(crate) static AFTER_SUBMIT: std::sync::Mutex<Option<AfterSubmit>> = std::sync::Mutex::new(None);
#[cfg(all(test, feature = "full-tests"))]
type AfterResult = Arc<dyn Fn(&JournalRoot, &str) + Send + Sync>;
#[cfg(all(test, feature = "full-tests"))]
pub(crate) static AFTER_RESULT: std::sync::Mutex<Option<AfterResult>> = std::sync::Mutex::new(None);
#[cfg(all(test, feature = "full-tests", unix))]
#[derive(Clone)]
pub(crate) struct PublicationFault {
    pub root: PathBuf,
    pub primitive: solstone_core_journal_io::atomic::BoundPublicationPrimitive,
    pub fired: Arc<AtomicBool>,
}
#[cfg(all(test, feature = "full-tests", unix))]
pub(crate) static PUBLICATION_FAULT: std::sync::Mutex<Option<PublicationFault>> =
    std::sync::Mutex::new(None);
#[cfg(all(test, feature = "full-tests"))]
fn after_submit(root: &JournalRoot, id: &str) -> Result<(), ()> {
    let hook = AFTER_SUBMIT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    hook.map_or(Ok(()), |hook| hook(root, id))
}

#[cfg(all(test, feature = "full-tests", unix))]
mod tests {
    use super::*;

    #[test]
    fn offload_receiving_replacement_root_receives_no_receipt_or_submission() {
        let parent = tempfile::tempdir().unwrap();
        let name = parent.path().join("journal");
        std::fs::create_dir(&name).unwrap();
        let root = Arc::new(JournalRoot::open(&name).unwrap());
        std::fs::rename(&name, parent.path().join("retained")).unwrap();
        std::fs::create_dir(&name).unwrap();
        assert!(publish(&root, &Receipt::new("root-replaced", Phase::Received, None)).is_err());
        assert_eq!(
            execute(&root, Request::Submit("root-replaced".into()), &|| false).0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(std::fs::read_dir(&name).unwrap().count(), 0);
        assert_eq!(
            std::fs::read_dir(parent.path().join("retained"))
                .unwrap()
                .count(),
            0
        );
    }
}
