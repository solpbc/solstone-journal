// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Handoff of journal setup into the installed Solstone app on Windows.
//!
//! When journal setup completes, the owner can continue in the Solstone desktop
//! app on this PC. Communication happens through an on-disk mailbox and lock
//! protocol defined in the setup handoff contract.

use std::fs::{self, File, TryLockError};
use std::io::Read;
use std::path::{Path, PathBuf};

use jsonschema::Draft;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_journal_io::{DetailedAtomicOutcome, atomic_replace_detailed};

use crate::setup_flow::{ContinueOutcome, admit_mark_render};

const CONTRACT_JSON: &str = include_str!("../contracts/windows-setup-handoff.json");

#[derive(Debug, Clone, Deserialize)]
pub struct HandoffContract {
    pub argv: ArgvContract,
    pub capability_document: CapabilityDocumentContract,
    pub desktop: DesktopContract,
    pub fields: FieldsContract,
    pub files: FilesContract,
    pub identity: IdentityContract,
    pub limits: LimitsContract,
    pub resource: ResourceContract,
    pub schemas: SchemasContract,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArgvContract {
    pub cli_flag: String,
    pub control_verb: String,
    pub installed_executable: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CapabilityDocumentContract {
    pub capabilities: Vec<String>,
    pub schema: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DesktopContract {
    pub canonical_prefix: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FieldsContract {
    pub mailbox: FieldGroupContract,
    pub receipt: FieldGroupContract,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FieldGroupContract {
    pub unknown_fields: String,
    pub required: Vec<FieldDefContract>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FieldDefContract {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub r#const: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilesContract {
    pub lock: String,
    pub mailbox: String,
    pub receipt: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IdentityContract {
    pub method: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LimitsContract {
    pub mailbox_max_bytes: usize,
    pub receipt_max_bytes: usize,
    pub capability_resource_max_bytes: usize,
    pub lock_acquire_ms: u64,
    // Part of the pinned contract; in this crate only the contract test reads it.
    #[cfg(test)]
    pub unpaired_draft_max_age_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResourceContract {
    pub name: String,
    #[serde(rename = "type")]
    pub resource_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SchemasContract {
    pub mailbox: String,
    pub receipt: String,
}

pub fn resource_type_id(resource_type: &str) -> u32 {
    match resource_type {
        "RT_RCDATA" => 10,
        other => panic!("resource type must be RT_RCDATA, got: {other}"),
    }
}

pub fn get_contract() -> HandoffContract {
    let contract: HandoffContract =
        serde_json::from_str(CONTRACT_JSON).expect("valid embedded handoff contract");
    assert_eq!(
        resource_type_id(&contract.resource.resource_type),
        10,
        "resource type must be RT_RCDATA"
    );
    assert_eq!(
        contract.identity.method, "GET",
        "identity method must be GET"
    );
    assert!(
        !contract.argv.control_verb.is_empty(),
        "control_verb must not be empty"
    );
    contract
}

pub fn build_json_schema(group: &FieldGroupContract, schema_const: &str) -> Result<Value, String> {
    if group.unknown_fields != "forbidden" {
        return Err(format!(
            "unsupported unknown_fields: {}",
            group.unknown_fields
        ));
    }
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for field in &group.required {
        required.push(Value::String(field.name.clone()));
        let mut prop = serde_json::Map::new();
        match field.field_type.as_str() {
            "string" => {
                prop.insert("type".to_string(), json!("string"));
                if field.name == "schema" {
                    prop.insert("const".to_string(), json!(schema_const));
                } else if let Some(ref c) = field.r#const {
                    prop.insert("const".to_string(), c.clone());
                } else {
                    prop.insert("minLength".to_string(), json!(1));
                }
            }
            "uuid" => {
                prop.insert("type".to_string(), json!("string"));
                prop.insert(
                    "pattern".to_string(),
                    json!("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"),
                );
            }
            "integer" => {
                prop.insert("type".to_string(), json!("integer"));
                if let Some(ref c) = field.r#const {
                    prop.insert("const".to_string(), c.clone());
                }
            }
            "sha256-hex" => {
                prop.insert("type".to_string(), json!("string"));
                prop.insert("pattern".to_string(), json!("^[0-9a-f]{64}$"));
            }
            "boolean" => {
                prop.insert("type".to_string(), json!("boolean"));
                if let Some(ref c) = field.r#const {
                    prop.insert("const".to_string(), c.clone());
                }
            }
            "unix-milliseconds" => {
                prop.insert("type".to_string(), json!("integer"));
                prop.insert("minimum".to_string(), json!(0));
            }
            "https-pair-link" => {
                prop.insert("type".to_string(), json!("string"));
                prop.insert("minLength".to_string(), json!(1));
            }
            other => return Err(format!("unknown type tag: {other}")),
        }
        properties.insert(field.name.clone(), Value::Object(prop));
    }

    let mut schema = serde_json::Map::new();
    schema.insert("type".to_string(), json!("object"));
    schema.insert("properties".to_string(), Value::Object(properties));
    schema.insert("required".to_string(), Value::Array(required));
    schema.insert("additionalProperties".to_string(), json!(false));
    Ok(Value::Object(schema))
}

pub fn compile_validator(schema_val: &Value) -> Result<jsonschema::Validator, String> {
    jsonschema::options()
        .with_draft(Draft::Draft202012)
        .build(schema_val)
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingContinuation {
    pub handoff_id: String,
}

pub fn is_valid_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let b = s.as_bytes();
    if b[8] != b'-' || b[13] != b'-' || b[18] != b'-' || b[23] != b'-' {
        return false;
    }
    for (i, &byte) in b.iter().enumerate() {
        if i == 8 || i == 13 || i == 18 || i == 23 {
            continue;
        }
        if !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase() {
            return false;
        }
    }
    true
}

pub fn generate_uuid() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("getrandom available");
    // Version 4: bits 12..15 of time_hi_and_version set to 0100
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    // Variant RFC 4122: bits 6..7 of clock_seq_hi_and_reserved set to 10
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

pub fn read_pending(state_dir: &Path) -> Result<Option<PendingContinuation>, String> {
    let path = state_dir.join("setup-continuation.json");
    let bytes = match read_bounded(&path, 1024) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let pending: PendingContinuation = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if !is_valid_uuid(&pending.handoff_id) {
        return Err("invalid handoff_id".to_string());
    }
    Ok(Some(pending))
}

pub fn write_pending(state_dir: &Path, handoff_id: &str) -> Result<(), String> {
    if !is_valid_uuid(handoff_id) {
        return Err("invalid handoff_id".to_string());
    }
    fs::create_dir_all(state_dir).map_err(|e| e.to_string())?;
    let target = state_dir.join("setup-continuation.json");
    let content = serde_json::to_string(&PendingContinuation {
        handoff_id: handoff_id.to_string(),
    })
    .map_err(|e| e.to_string())?;
    publish_record(&target, content.as_bytes())?;
    Ok(())
}

fn publish_record(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let outcome =
        atomic_replace_detailed(target, bytes, 0o600).map_err(|error| error.to_string())?;
    require_confirmed_publication(outcome)
}

fn require_confirmed_publication(outcome: DetailedAtomicOutcome) -> Result<(), String> {
    match outcome {
        DetailedAtomicOutcome::Published => Ok(()),
        _ => Err("handoff publication could not be confirmed".to_owned()),
    }
}

fn read_bounded(path: &Path, max: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "handoff record is too large",
        ));
    }
    Ok(bytes)
}

pub fn delete_pending(state_dir: &Path) -> Result<(), String> {
    let path = state_dir.join("setup-continuation.json");
    if path.exists() {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct HandoffPaths {
    pub local_appdata: PathBuf,
    pub state_dir: PathBuf,
}

impl HandoffPaths {
    pub fn solstone_dir(&self) -> PathBuf {
        self.local_appdata.join("Solstone")
    }
}

pub fn resolve_installed_executable(template: &str, root: &Path) -> Result<PathBuf, String> {
    if !template.starts_with("%LOCALAPPDATA%\\") && !template.starts_with("%LOCALAPPDATA%/") {
        return Err("installed_executable must start with %LOCALAPPDATA%".to_string());
    }
    let rest = &template["%LOCALAPPDATA%".len()..];
    let trimmed = rest.trim_start_matches('\\').trim_start_matches('/');
    let mut path = root.to_path_buf();
    for segment in trimmed.split(['\\', '/']) {
        if !segment.is_empty() {
            path.push(segment);
        }
    }
    Ok(path)
}

pub trait WaitClock {
    fn now_ms(&self) -> u64;
    fn monotonic_ms(&self) -> u64;
    fn sleep_ms(&mut self, ms: u64);
}

pub struct WallClock;

impl WaitClock for WallClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn monotonic_ms(&self) -> u64 {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn sleep_ms(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

#[derive(Debug)]
pub enum HandoffError {
    Busy,
    Io(String),
}

impl std::fmt::Display for HandoffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => write!(f, "handoff lock is busy"),
            Self::Io(msg) => write!(f, "handoff io error: {msg}"),
        }
    }
}

impl std::error::Error for HandoffError {}

#[derive(Debug)]
pub struct HandoffLock {
    file: File,
}

impl Drop for HandoffLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl HandoffLock {
    pub fn acquire<C: WaitClock>(
        lock_path: &Path,
        max_wait_ms: u64,
        clock: &mut C,
    ) -> Result<Self, HandoffError> {
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|e| HandoffError::Io(e.to_string()))?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|e| HandoffError::Io(e.to_string()))?;

        let start = clock.monotonic_ms();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) => {
                    let elapsed = clock.monotonic_ms().saturating_sub(start);
                    if elapsed >= max_wait_ms {
                        return Err(HandoffError::Busy);
                    }
                    let remaining = max_wait_ms.saturating_sub(elapsed);
                    let step = 50.min(remaining);
                    if step == 0 {
                        return Err(HandoffError::Busy);
                    }
                    clock.sleep_ms(step);
                }
                Err(TryLockError::Error(e)) => return Err(HandoffError::Io(e.to_string())),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityResult {
    Supported,
    Absent,
    Malformed,
    Oversized,
    Unknown,
    NoResource,
}

pub fn admit_capability_document(
    bytes: &[u8],
    contract_doc: &CapabilityDocumentContract,
    max_bytes: usize,
) -> CapabilityResult {
    if bytes.len() > max_bytes {
        return CapabilityResult::Oversized;
    }
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return CapabilityResult::Malformed,
    };
    match serde_json::to_value(contract_doc) {
        Ok(expected) if value == expected => CapabilityResult::Supported,
        _ => CapabilityResult::Unknown,
    }
}

pub trait HandoffEndpoints {
    fn is_current(&self) -> bool;
    fn get(&self, path: &str) -> Result<Value, String>;
    fn post(&self, path: &str, body: &Value) -> Result<Value, String>;
}

#[cfg(windows)]
impl HandoffEndpoints for crate::convey::Convey {
    fn is_current(&self) -> bool {
        crate::journal::status()
            .ok()
            .and_then(|status| status.port)
            .is_some_and(|port| self.base() == format!("http://127.0.0.1:{port}"))
    }
    fn get(&self, path: &str) -> Result<Value, String> {
        self.get(path).map_err(|r| r.message)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.post(path, Some(body)).map_err(|r| r.message)
    }
}

pub struct HandoffCollaborators<
    'a,
    E: HandoffEndpoints,
    P: FnMut(&Path) -> Result<G, CapabilityResult>,
    L: FnMut(&G, &Path, &str) -> Result<(), ()>,
    C: WaitClock,
    F: FnMut() -> String,
    S: Fn() -> bool,
    G,
> {
    pub paths: HandoffPaths,
    pub endpoints: &'a E,
    pub prepare: P,
    pub launch: L,
    pub still_current: S,
    pub clock: C,
    pub new_id: F,
}

pub fn continue_in_solstone<E, P, L, C, F, S, G>(
    mut collaborators: HandoffCollaborators<'_, E, P, L, C, F, S, G>,
) -> ContinueOutcome
where
    E: HandoffEndpoints,
    P: FnMut(&Path) -> Result<G, CapabilityResult>,
    L: FnMut(&G, &Path, &str) -> Result<(), ()>,
    C: WaitClock,
    F: FnMut() -> String,
    S: Fn() -> bool,
{
    let contract = get_contract();
    let solstone_dir = collaborators.paths.solstone_dir();

    // 1. check still_current
    if !(collaborators.still_current)() {
        return ContinueOutcome::Superseded;
    }

    // 2. lock and advance pending ID
    let lock_path = solstone_dir.join(&contract.files.lock);
    let handoff_id = {
        let _lock = match HandoffLock::acquire(
            &lock_path,
            contract.limits.lock_acquire_ms,
            &mut collaborators.clock,
        ) {
            Ok(l) => l,
            Err(_) => return ContinueOutcome::Failed,
        };
        let id = (collaborators.new_id)();
        if write_pending(&collaborators.paths.state_dir, &id).is_err() {
            return ContinueOutcome::Failed;
        }
        id
    };

    // 3. check still_current
    if !(collaborators.still_current)() {
        return ContinueOutcome::Superseded;
    }

    // 4. fetch identity
    let identity_val = match collaborators.endpoints.get(&contract.identity.path) {
        Ok(v) => v,
        Err(_) => return ContinueOutcome::Failed,
    };
    if identity_val.get("committed").and_then(Value::as_bool) != Some(true) {
        return ContinueOutcome::Failed;
    }
    let instance_id = match identity_val.get("instance_id").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return ContinueOutcome::Failed,
    };
    let mark = match identity_val.get("mark") {
        Some(m) if !m.is_null() => m,
        _ => return ContinueOutcome::Failed,
    };
    if admit_mark_render(mark).is_err() {
        return ContinueOutcome::Failed;
    }

    // 5. check still_current
    if !(collaborators.still_current)() {
        return ContinueOutcome::Superseded;
    }

    // 6. check receiver capability
    let exe_path = match resolve_installed_executable(
        &contract.argv.installed_executable,
        &collaborators.paths.local_appdata,
    ) {
        Ok(p) => p,
        Err(_) => return ContinueOutcome::Failed,
    };
    let prepared = match (collaborators.prepare)(&exe_path) {
        Ok(prepared) => prepared,
        Err(_) => return ContinueOutcome::Unsupported,
    };

    // 7. record request start timestamp and POST /app/network/pair-start
    let request_start = collaborators.clock.now_ms();
    let pair_resp = match collaborators
        .endpoints
        .post("/app/network/pair-start", &json!({ "same_machine": true }))
    {
        Ok(v) => v,
        Err(_) => return ContinueOutcome::Failed,
    };

    // A service restart or journal rebind during mint must not combine the
    // previous instance with a new journal's link and CA expectation.
    match collaborators.endpoints.get(&contract.identity.path) {
        Ok(current)
            if current.get("committed").and_then(Value::as_bool) == Some(true)
                && current.get("instance_id").and_then(Value::as_str)
                    == Some(instance_id.as_str())
                && current.get("mark") == Some(mark) => {}
        _ => return ContinueOutcome::Failed,
    }

    if !collaborators.endpoints.is_current() {
        return ContinueOutcome::Failed;
    }

    // 8. validate pair-start response (reads pair_link)
    let pairing_link = match pair_resp.get("pair_link").and_then(Value::as_str) {
        Some(s) if s.starts_with(&contract.desktop.canonical_prefix) => s.to_string(),
        _ => return ContinueOutcome::Failed,
    };
    let parsed_link = match spl_core::pairlink::parse(&pairing_link) {
        Ok(spl_core::pairlink::ParsedPairLink::Direct(d)) => d,
        _ => return ContinueOutcome::Failed,
    };
    if parsed_link.candidates.is_empty()
        || !parsed_link.candidates.iter().all(|c| c.host == "127.0.0.1")
    {
        return ContinueOutcome::Failed;
    }

    let ca_fp = match pair_resp.get("ca_fingerprint").and_then(Value::as_str) {
        Some(s)
            if s.len() == 64
                && s.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) =>
        {
            s.to_string()
        }
        _ => return ContinueOutcome::Failed,
    };
    let prefix_hex: String = parsed_link
        .ca_fp_prefix
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if prefix_hex.len() != 32 || ca_fp[..32] != prefix_hex {
        return ContinueOutcome::Failed;
    }

    let expires_in = match pair_resp.get("expires_in").and_then(Value::as_u64) {
        Some(n) if n >= 1 => n,
        _ => return ContinueOutcome::Failed,
    };
    let expires_ms = match expires_in.checked_mul(1000) {
        Some(ms) => ms,
        None => return ContinueOutcome::Failed,
    };
    let deadline = match request_start.checked_add(expires_ms) {
        Some(d) => d,
        None => return ContinueOutcome::Failed,
    };
    if collaborators.clock.now_ms() >= deadline {
        return ContinueOutcome::Failed;
    }

    // 9. check still_current
    if !(collaborators.still_current)() {
        return ContinueOutcome::Superseded;
    }

    // 10. under lock verify still_current, pending ID, and atomically replace mailbox
    {
        let _lock = match HandoffLock::acquire(
            &lock_path,
            contract.limits.lock_acquire_ms,
            &mut collaborators.clock,
        ) {
            Ok(l) => l,
            Err(_) => return ContinueOutcome::Failed,
        };

        if !(collaborators.still_current)() {
            return ContinueOutcome::Superseded;
        }

        match read_pending(&collaborators.paths.state_dir) {
            Ok(Some(current_pending)) if current_pending.handoff_id == handoff_id => {}
            _ => return ContinueOutcome::Superseded,
        }

        let step_const = match contract
            .fields
            .mailbox
            .required
            .iter()
            .find(|f| f.name == "step")
            .and_then(|f| f.r#const.as_ref())
            .and_then(Value::as_u64)
        {
            Some(c) => c,
            None => return ContinueOutcome::Failed,
        };

        let mailbox_val = json!({
            "schema": contract.schemas.mailbox,
            "handoff_id": handoff_id,
            "step": step_const,
            "expected_instance_id": instance_id,
            "expected_ca_fingerprint": ca_fp,
            "pairing_link": pairing_link,
            "expires_at_unix_ms": deadline,
        });

        let mailbox_schema =
            match build_json_schema(&contract.fields.mailbox, &contract.schemas.mailbox) {
                Ok(s) => s,
                Err(_) => return ContinueOutcome::Failed,
            };
        let validator = match compile_validator(&mailbox_schema) {
            Ok(v) => v,
            Err(_) => return ContinueOutcome::Failed,
        };
        if !validator.is_valid(&mailbox_val) {
            return ContinueOutcome::Failed;
        }

        let mailbox_bytes = match serde_json::to_vec(&mailbox_val) {
            Ok(b) if b.len() <= contract.limits.mailbox_max_bytes => b,
            _ => return ContinueOutcome::Failed,
        };

        let target_mailbox = solstone_dir.join(&contract.files.mailbox);
        if collaborators.clock.now_ms() >= deadline
            || publish_record(&target_mailbox, &mailbox_bytes).is_err()
        {
            return ContinueOutcome::Failed;
        }
    }

    // 11. check still_current
    if !(collaborators.still_current)() {
        return ContinueOutcome::Superseded;
    }

    // 12. launch installed executable with argv.cli_flag
    if collaborators.clock.now_ms() >= deadline {
        return ContinueOutcome::Failed;
    }
    match (collaborators.launch)(&prepared, &exe_path, &contract.argv.cli_flag) {
        Ok(()) => ContinueOutcome::Published,
        Err(()) => ContinueOutcome::Failed,
    }
}

pub fn observe_receipt<C: WaitClock>(paths: &HandoffPaths, clock: &mut C) -> Result<bool, String> {
    let contract = get_contract();
    let solstone_dir = paths.solstone_dir();
    let lock_path = solstone_dir.join(&contract.files.lock);

    let _lock = HandoffLock::acquire(&lock_path, contract.limits.lock_acquire_ms, clock)
        .map_err(|e| e.to_string())?;

    let pending = match read_pending(&paths.state_dir)? {
        Some(p) => p,
        None => return Ok(false),
    };

    let receipt_path = solstone_dir.join(&contract.files.receipt);
    if !receipt_path.exists() {
        return Ok(true);
    }

    let receipt_bytes = match read_bounded(&receipt_path, contract.limits.receipt_max_bytes) {
        Ok(b) => b,
        Err(_) => return Ok(true),
    };

    if receipt_bytes.len() > contract.limits.receipt_max_bytes {
        return Ok(true);
    }

    let receipt_val: Value = match serde_json::from_slice(&receipt_bytes) {
        Ok(v) => v,
        Err(_) => return Ok(true),
    };

    let receipt_schema =
        match build_json_schema(&contract.fields.receipt, &contract.schemas.receipt) {
            Ok(s) => s,
            Err(_) => return Ok(true),
        };
    let validator = match compile_validator(&receipt_schema) {
        Ok(v) => v,
        Err(_) => return Ok(true),
    };
    if !validator.is_valid(&receipt_val) {
        return Ok(true);
    }

    let receipt_id = match receipt_val.get("handoff_id").and_then(Value::as_str) {
        Some(id) => id,
        None => return Ok(true),
    };

    if receipt_id == pending.handoff_id {
        delete_pending(&paths.state_dir)?;
        Ok(false)
    } else {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    #[test]
    fn contract_provenance_and_mailbox_receipt_rules() {
        let contract_bytes = include_bytes!("../contracts/windows-setup-handoff.json");
        let hash = format!("{:x}", Sha256::digest(contract_bytes));
        let prov_str = include_str!("../contracts/windows-setup-handoff.provenance.json");
        let prov_val: Value = serde_json::from_str(prov_str).unwrap();
        assert_eq!(hash, prov_val["sha256"].as_str().unwrap());

        let raw_json: Value = serde_json::from_slice(contract_bytes).unwrap();
        let loaded = get_contract();

        assert_eq!(
            loaded.limits.mailbox_max_bytes,
            raw_json["limits"]["mailbox_max_bytes"].as_u64().unwrap() as usize
        );
        assert_eq!(
            loaded.limits.receipt_max_bytes,
            raw_json["limits"]["receipt_max_bytes"].as_u64().unwrap() as usize
        );
        assert_eq!(
            loaded.limits.capability_resource_max_bytes,
            raw_json["limits"]["capability_resource_max_bytes"]
                .as_u64()
                .unwrap() as usize
        );
        assert_eq!(
            loaded.limits.lock_acquire_ms,
            raw_json["limits"]["lock_acquire_ms"].as_u64().unwrap()
        );
        assert_eq!(
            loaded.limits.unpaired_draft_max_age_secs,
            raw_json["limits"]["unpaired_draft_max_age_secs"]
                .as_u64()
                .unwrap()
        );
        assert_eq!(loaded.schemas.mailbox, raw_json["schemas"]["mailbox"]);
        assert_eq!(loaded.schemas.receipt, raw_json["schemas"]["receipt"]);
        assert_eq!(loaded.argv.cli_flag, raw_json["argv"]["cli_flag"]);
        assert_eq!(loaded.resource.name, raw_json["resource"]["name"]);
        assert_eq!(loaded.files.lock, raw_json["files"]["lock"]);
        assert_eq!(loaded.files.mailbox, raw_json["files"]["mailbox"]);
        assert_eq!(loaded.files.receipt, raw_json["files"]["receipt"]);
        assert_eq!(loaded.identity.path, raw_json["identity"]["path"]);

        // Step const
        let step_field = loaded
            .fields
            .mailbox
            .required
            .iter()
            .find(|f| f.name == "step")
            .unwrap();
        assert_eq!(step_field.r#const, Some(json!(5)));

        // Mailbox validation rules
        let mb_schema = build_json_schema(&loaded.fields.mailbox, &loaded.schemas.mailbox).unwrap();
        let mb_val = compile_validator(&mb_schema).unwrap();

        let valid_mb = json!({
            "schema": "solstone.windows-setup-handoff.v1",
            "handoff_id": "01234567-89ab-4cde-8f01-23456789abcd",
            "step": 5,
            "expected_instance_id": "inst-1",
            "expected_ca_fingerprint": "a".repeat(64),
            "pairing_link": "https://go.solstone.app/p#mock",
            "expires_at_unix_ms": 1234567890u64,
        });
        assert!(mb_val.is_valid(&valid_mb));

        let mut extra_field_mb = valid_mb.clone();
        extra_field_mb["extra"] = json!("forbidden");
        assert!(!mb_val.is_valid(&extra_field_mb));

        let mut wrong_schema_mb = valid_mb.clone();
        wrong_schema_mb["schema"] = json!("wrong.schema");
        assert!(!mb_val.is_valid(&wrong_schema_mb));

        // Receipt validation rules
        let rc_schema = build_json_schema(&loaded.fields.receipt, &loaded.schemas.receipt).unwrap();
        let rc_val = compile_validator(&rc_schema).unwrap();

        let valid_rc = json!({
            "schema": "solstone.windows-setup-receipt.v1",
            "handoff_id": "01234567-89ab-4cde-8f01-23456789abcd",
            "accepted": true,
        });
        assert!(rc_val.is_valid(&valid_rc));

        let mut extra_field_rc = valid_rc.clone();
        extra_field_rc["extra"] = json!("forbidden");
        assert!(!rc_val.is_valid(&extra_field_rc));

        let mut rejected_rc = valid_rc.clone();
        rejected_rc["accepted"] = json!(false);
        assert!(!rc_val.is_valid(&rejected_rc));

        let mut wrong_schema_rc = valid_rc.clone();
        wrong_schema_rc["schema"] = json!("wrong.receipt.schema");
        assert!(!rc_val.is_valid(&wrong_schema_rc));

        // Byte limit assertions
        let large_mb = vec![b'a'; loaded.limits.mailbox_max_bytes + 1];
        assert!(large_mb.len() > loaded.limits.mailbox_max_bytes);
        let large_rc = vec![b'a'; loaded.limits.receipt_max_bytes + 1];
        assert!(large_rc.len() > loaded.limits.receipt_max_bytes);
    }

    #[test]
    fn capability_document_is_bounded_and_exact() {
        let contract = get_contract();
        let expected = &contract.capability_document;
        let max = contract.limits.capability_resource_max_bytes;
        let valid = serde_json::to_vec(expected).unwrap();
        assert_eq!(
            admit_capability_document(&valid, expected, max),
            CapabilityResult::Supported
        );
        for malformed in [b"".as_slice(), b"MZ", b"{invalid}"] {
            assert_eq!(
                admit_capability_document(malformed, expected, max),
                CapabilityResult::Malformed
            );
        }
        assert_eq!(
            admit_capability_document(&vec![b' '; max + 1], expected, max),
            CapabilityResult::Oversized
        );
        for unknown in [
            json!({"schema":"unknown","capabilities":expected.capabilities}),
            json!({"schema":expected.schema,"capabilities":[]}),
            json!({"schema":expected.schema,"capabilities":expected.capabilities,"extra":true}),
        ] {
            assert_eq!(
                admit_capability_document(&serde_json::to_vec(&unknown).unwrap(), expected, max),
                CapabilityResult::Unknown
            );
        }
        let root = Path::new("local-root");
        assert_eq!(
            resolve_installed_executable(&contract.argv.installed_executable, root).unwrap(),
            root.join("Solstone")
                .join("current")
                .join("solstone-windows-app.exe")
        );
        assert!(resolve_installed_executable("%OTHER%\\app.exe", root).is_err());
    }

    #[derive(Clone, Default)]
    pub struct VirtualClock {
        pub now: std::sync::Arc<std::sync::atomic::AtomicU64>,
        pub sleep_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
        pub total_slept: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }

    impl VirtualClock {
        pub fn new(now: u64) -> Self {
            Self {
                now: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now)),
                sleep_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
                total_slept: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            }
        }
    }

    impl WaitClock for VirtualClock {
        fn now_ms(&self) -> u64 {
            self.now.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn monotonic_ms(&self) -> u64 {
            self.now_ms()
        }

        fn sleep_ms(&mut self, ms: u64) {
            self.now.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
            self.sleep_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.total_slept
                .fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct MockEndpoints {
        authority_current: bool,
        identity_after_mint: Option<Value>,
        identity: Result<Value, String>,
        pair_start: Result<Value, String>,
        pair_start_called: std::cell::Cell<bool>,
        on_pair_start: Option<Box<dyn Fn()>>,
    }

    impl HandoffEndpoints for MockEndpoints {
        fn is_current(&self) -> bool {
            self.authority_current
        }
        fn get(&self, _path: &str) -> Result<Value, String> {
            if self.pair_start_called.get()
                && let Some(identity) = &self.identity_after_mint
            {
                return Ok(identity.clone());
            }
            self.identity.clone()
        }

        fn post(&self, _path: &str, _body: &Value) -> Result<Value, String> {
            self.pair_start_called.set(true);
            if let Some(ref cb) = self.on_pair_start {
                cb();
            }
            self.pair_start.clone()
        }
    }

    fn sample_identity() -> Value {
        json!({
            "committed": true,
            "instance_id": "inst-test",
            "mark": {
                "icon1": {
                    "svg": "<path d=\"M0 0h10v10H0z\"/>",
                    "rot": 0,
                    "color": {
                        "name": "blue",
                        "hex": "#0000ff"
                    }
                },
                "icon2": {
                    "svg": "<circle cx=\"5\" cy=\"5\" r=\"5\"/>",
                    "rot": 45,
                    "color": {
                        "name": "red",
                        "hex": "#ff0000"
                    }
                },
                "words": ["north", "stone"]
            }
        })
    }

    fn sample_v04_link(fp_prefix: &[u8; 16]) -> String {
        let mut v04 = vec![0x04, 0x01];
        v04.extend_from_slice(&[127, 0, 0, 1]);
        v04.extend_from_slice(&52443u16.to_be_bytes());
        v04.extend_from_slice(&[0x11; 16]); // nonce
        v04.extend_from_slice(fp_prefix);
        let fragment = spl_core::crockford::encode(&v04);
        format!("https://go.solstone.app/p#{fragment}")
    }

    fn sample_v05_multi_link(fp_prefix: &[u8; 16]) -> String {
        let mut v05 = vec![0x05, 0x01, 0x02]; // count = 2
        v05.extend_from_slice(&52443u16.to_be_bytes());
        v05.extend_from_slice(&[127, 0, 0, 1]);
        v05.extend_from_slice(&[192, 168, 1, 50]);
        v05.extend_from_slice(&[0x11; 16]); // nonce
        v05.extend_from_slice(fp_prefix);
        let fragment = spl_core::crockford::encode(&v05);
        format!("https://go.solstone.app/p#{fragment}")
    }

    #[test]
    fn continue_writes_mailbox_then_launches_or_retries() {
        let contract = get_contract();
        let dir = tempdir().unwrap();
        let local_appdata = dir.path().join("local");
        let state_dir = dir.path().join("state");
        let solstone_dir = local_appdata.join("Solstone");
        fs::create_dir_all(&solstone_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();

        let paths = HandoffPaths {
            local_appdata: local_appdata.clone(),
            state_dir: state_dir.clone(),
        };

        let exe_path =
            resolve_installed_executable(&contract.argv.installed_executable, &paths.local_appdata)
                .unwrap();

        let fp_prefix = [0xa0u8; 16];
        let link = sample_v04_link(&fp_prefix);
        let full_fp = "a0".repeat(32);

        let pair_resp = json!({
            "pair_link": link,
            "ca_fingerprint": full_fp,
            "expires_in": 60,
        });

        let mut launched = None;
        let endpoints = MockEndpoints {
            authority_current: true,
            identity_after_mint: None,
            identity: Ok(sample_identity()),
            pair_start: Ok(pair_resp.clone()),
            pair_start_called: std::cell::Cell::new(false),
            on_pair_start: None,
        };

        let fixed_id = "01234567-89ab-4cde-8f01-23456789abcd".to_string();
        let fixed_id_copy = fixed_id.clone();

        let collaborators = HandoffCollaborators {
            paths: paths.clone(),
            endpoints: &endpoints,
            prepare: |_| Ok(()),
            launch: |_, p: &Path, f: &str| {
                launched = Some((p.to_path_buf(), f.to_string()));
                Ok(())
            },
            still_current: || true,
            clock: VirtualClock::new(1000),
            new_id: move || fixed_id_copy.clone(),
        };

        let outcome = continue_in_solstone(collaborators);
        assert_eq!(outcome, ContinueOutcome::Published);
        assert!(endpoints.pair_start_called.get());

        // Verify launch parameters
        let (launch_p, launch_f) = launched.unwrap();
        assert_eq!(launch_p, exe_path);
        assert_eq!(launch_f, contract.argv.cli_flag);

        // Verify mailbox content
        let mb_path = solstone_dir.join(&contract.files.mailbox);
        let mb_bytes = fs::read(&mb_path).unwrap();
        let mb_val: Value = serde_json::from_slice(&mb_bytes).unwrap();
        assert_eq!(mb_val["schema"], contract.schemas.mailbox);
        assert_eq!(mb_val["handoff_id"], fixed_id);
        let step_const = contract
            .fields
            .mailbox
            .required
            .iter()
            .find(|f| f.name == "step")
            .and_then(|f| f.r#const.as_ref())
            .and_then(Value::as_u64)
            .unwrap();
        assert_eq!(mb_val["step"], step_const);
        assert_eq!(mb_val["expected_instance_id"], "inst-test");
        assert_eq!(mb_val["expected_ca_fingerprint"], full_fp);
        assert_eq!(mb_val["pairing_link"], link);
        assert_eq!(mb_val["expires_at_unix_ms"], 1000 + 60 * 1000);

        // Pending file still exists after Published
        assert!(state_dir.join("setup-continuation.json").exists());

        // Multi candidate link with extra non-localhost candidate publishes nothing
        {
            let _ = fs::remove_file(&mb_path);
            let multi_link = sample_v05_multi_link(&fp_prefix);
            let mut launched = false;
            let ep = MockEndpoints {
                authority_current: true,
                identity_after_mint: None,
                identity: Ok(sample_identity()),
                pair_start: Ok(json!({
                    "pair_link": multi_link,
                    "ca_fingerprint": full_fp,
                    "expires_in": 60,
                })),
                pair_start_called: std::cell::Cell::new(false),
                on_pair_start: None,
            };
            let c = HandoffCollaborators {
                paths: paths.clone(),
                endpoints: &ep,
                prepare: |_| Ok(()),
                launch: |_, _, _| {
                    launched = true;
                    Ok(())
                },
                still_current: || true,
                clock: VirtualClock::new(1000),
                new_id: || fixed_id.clone(),
            };
            assert_eq!(continue_in_solstone(c), ContinueOutcome::Failed);
            assert!(!launched);
            assert!(!mb_path.exists());
        }

        // Test refusal cases where pair-start must NOT be called or mailbox/launch must not happen
        let cases: Vec<(Value, Value)> = vec![
            (
                json!({"committed": false, "instance_id": "inst", "mark": sample_identity()["mark"]}),
                pair_resp.clone(),
            ),
            (
                json!({"committed": true, "instance_id": Value::Null, "mark": sample_identity()["mark"]}),
                pair_resp.clone(),
            ),
            (
                json!({"committed": true, "instance_id": "inst", "mark": Value::Null}),
                pair_resp.clone(),
            ),
            (
                sample_identity(),
                json!({"pair_link": link, "ca_fingerprint": "b0".repeat(32), "expires_in": 60}),
            ),
            (
                sample_identity(),
                json!({"pair_link": link, "ca_fingerprint": full_fp, "expires_in": 0}),
            ),
            (
                sample_identity(),
                json!({"pair_link": link, "ca_fingerprint": full_fp, "expires_in": 1.5}),
            ),
            (
                sample_identity(),
                json!({"pair_link": link, "ca_fingerprint": full_fp, "expires_in": "60"}),
            ),
            (
                sample_identity(),
                json!({"pairing_link": link, "ca_fingerprint": full_fp, "expires_in": 60}), // only pairing_link, no pair_link
            ),
        ];

        for (id_val, p_resp) in cases {
            let _ = fs::remove_file(&mb_path);
            let mut launched = false;
            let ep = MockEndpoints {
                authority_current: true,
                identity_after_mint: None,
                identity: Ok(id_val),
                pair_start: Ok(p_resp),
                pair_start_called: std::cell::Cell::new(false),
                on_pair_start: None,
            };
            let c = HandoffCollaborators {
                paths: paths.clone(),
                endpoints: &ep,
                prepare: |_| Ok(()),
                launch: |_, _, _| {
                    launched = true;
                    Ok(())
                },
                still_current: || true,
                clock: VirtualClock::new(1000),
                new_id: || fixed_id.clone(),
            };
            assert_eq!(continue_in_solstone(c), ContinueOutcome::Failed);
            assert!(!launched);
            assert!(!mb_path.exists());
        }

        // Deadline already elapsed test
        {
            let clock_now = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1000));
            let clock_now_clone = clock_now.clone();
            let mut launched = false;
            let ep = MockEndpoints {
                authority_current: true,
                identity_after_mint: None,
                identity: Ok(sample_identity()),
                pair_start: Ok(pair_resp.clone()),
                pair_start_called: std::cell::Cell::new(false),
                on_pair_start: Some(Box::new(move || {
                    clock_now_clone.store(1000 + 70 * 1000, std::sync::atomic::Ordering::SeqCst);
                })),
            };
            let c = HandoffCollaborators {
                paths: paths.clone(),
                endpoints: &ep,
                prepare: |_| Ok(()),
                launch: |_, _, _| {
                    launched = true;
                    Ok(())
                },
                still_current: || true,
                clock: VirtualClock {
                    now: clock_now,
                    sleep_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
                    total_slept: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
                },
                new_id: || fixed_id.clone(),
            };
            assert_eq!(continue_in_solstone(c), ContinueOutcome::Failed);
            assert!(!launched);
        }

        // Lock already held test
        {
            let lock_path = solstone_dir.join(&contract.files.lock);
            let held_file = File::create(&lock_path).unwrap();
            held_file.try_lock().unwrap();

            let mut launched = false;
            let ep = MockEndpoints {
                authority_current: true,
                identity_after_mint: None,
                identity: Ok(sample_identity()),
                pair_start: Ok(pair_resp.clone()),
                pair_start_called: std::cell::Cell::new(false),
                on_pair_start: None,
            };
            let c = HandoffCollaborators {
                paths: paths.clone(),
                endpoints: &ep,
                prepare: |_| Ok(()),
                launch: |_, _, _| {
                    launched = true;
                    Ok(())
                },
                still_current: || true,
                clock: VirtualClock::default(),
                new_id: || fixed_id.clone(),
            };
            assert_eq!(continue_in_solstone(c), ContinueOutcome::Failed);
            assert!(!launched);
            drop(held_file);
        }

        // Mailbox destination replaced by a directory
        {
            let _ = fs::remove_file(&mb_path);
            fs::create_dir_all(&mb_path).unwrap();
            let mut launched = false;
            let ep = MockEndpoints {
                authority_current: true,
                identity_after_mint: None,
                identity: Ok(sample_identity()),
                pair_start: Ok(pair_resp.clone()),
                pair_start_called: std::cell::Cell::new(false),
                on_pair_start: None,
            };
            let c = HandoffCollaborators {
                paths: paths.clone(),
                endpoints: &ep,
                prepare: |_| Ok(()),
                launch: |_, _, _| {
                    launched = true;
                    Ok(())
                },
                still_current: || true,
                clock: VirtualClock::new(1000),
                new_id: || fixed_id.clone(),
            };
            assert_eq!(continue_in_solstone(c), ContinueOutcome::Failed);
            assert!(!launched);
            let _ = fs::remove_dir(&mb_path);
        }

        // still_current false before work
        {
            let mut launched = false;
            let ep = MockEndpoints {
                authority_current: true,
                identity_after_mint: None,
                identity: Ok(sample_identity()),
                pair_start: Ok(pair_resp.clone()),
                pair_start_called: std::cell::Cell::new(false),
                on_pair_start: None,
            };
            let c = HandoffCollaborators {
                paths: paths.clone(),
                endpoints: &ep,
                prepare: |_| Ok(()),
                launch: |_, _, _| {
                    launched = true;
                    Ok(())
                },
                still_current: || false,
                clock: VirtualClock::default(),
                new_id: || fixed_id.clone(),
            };
            assert_eq!(continue_in_solstone(c), ContinueOutcome::Superseded);
            assert!(!launched);
            assert!(!ep.pair_start_called.get());
        }

        // pair-start swapping the pending ID publishes nothing
        {
            let _ = fs::remove_file(&mb_path);
            let state_dir_clone = state_dir.clone();
            let ep = MockEndpoints {
                authority_current: true,
                identity_after_mint: None,
                identity: Ok(sample_identity()),
                pair_start: Ok(pair_resp.clone()),
                pair_start_called: std::cell::Cell::new(false),
                on_pair_start: Some(Box::new(move || {
                    let other_id = "99999999-9999-4999-8999-999999999999";
                    write_pending(&state_dir_clone, other_id).unwrap();
                })),
            };
            let mut launched = false;
            let c = HandoffCollaborators {
                paths: paths.clone(),
                endpoints: &ep,
                prepare: |_| Ok(()),
                launch: |_, _, _| {
                    launched = true;
                    Ok(())
                },
                still_current: || true,
                clock: VirtualClock::new(1000),
                new_id: || fixed_id.clone(),
            };
            assert_eq!(continue_in_solstone(c), ContinueOutcome::Superseded);
            assert!(!launched);
            assert!(!mb_path.exists());
        }

        // Launch Err leaves pending and returns Failed
        {
            write_pending(&state_dir, &fixed_id).unwrap();
            let ep = MockEndpoints {
                authority_current: true,
                identity_after_mint: None,
                identity: Ok(sample_identity()),
                pair_start: Ok(pair_resp.clone()),
                pair_start_called: std::cell::Cell::new(false),
                on_pair_start: None,
            };
            let c = HandoffCollaborators {
                paths: paths.clone(),
                endpoints: &ep,
                prepare: |_| Ok(()),
                launch: |_, _, _| Err(()),
                still_current: || true,
                clock: VirtualClock::new(1000),
                new_id: || fixed_id.clone(),
            };
            assert_eq!(continue_in_solstone(c), ContinueOutcome::Failed);
            assert!(state_dir.join("setup-continuation.json").exists());
        }

        // Receipt observation tests
        let rc_path = solstone_dir.join(&contract.files.receipt);

        // Missing receipt leaves pending
        let mut clk = VirtualClock::default();
        assert!(observe_receipt(&paths, &mut clk).unwrap());
        assert!(state_dir.join("setup-continuation.json").exists());

        // Receipt ID mismatch (receipt ID A leaves pending B)
        let other_id = "88888888-8888-4888-8888-888888888888";
        let rc_mismatch = json!({
            "schema": contract.schemas.receipt,
            "handoff_id": other_id,
            "accepted": true,
        });
        fs::write(&rc_path, serde_json::to_vec(&rc_mismatch).unwrap()).unwrap();
        assert!(observe_receipt(&paths, &mut clk).unwrap());
        assert!(state_dir.join("setup-continuation.json").exists());

        // Oversized receipt leaves pending
        let large_rc = vec![b' '; 5000];
        fs::write(&rc_path, &large_rc).unwrap();
        assert!(observe_receipt(&paths, &mut clk).unwrap());
        assert!(state_dir.join("setup-continuation.json").exists());

        // Extra field in receipt leaves pending
        let rc_extra = json!({
            "schema": contract.schemas.receipt,
            "handoff_id": fixed_id,
            "accepted": true,
            "extra": "bad",
        });
        fs::write(&rc_path, serde_json::to_vec(&rc_extra).unwrap()).unwrap();
        assert!(observe_receipt(&paths, &mut clk).unwrap());
        assert!(state_dir.join("setup-continuation.json").exists());

        // accepted: false leaves pending
        let rc_declined = json!({
            "schema": contract.schemas.receipt,
            "handoff_id": fixed_id,
            "accepted": false,
        });
        fs::write(&rc_path, serde_json::to_vec(&rc_declined).unwrap()).unwrap();
        assert!(observe_receipt(&paths, &mut clk).unwrap());
        assert!(state_dir.join("setup-continuation.json").exists());

        // Matching receipt deletes ONLY pending; receipt and mailbox remain intact
        let rc_matching = json!({
            "schema": contract.schemas.receipt,
            "handoff_id": fixed_id,
            "accepted": true,
        });
        fs::write(&rc_path, serde_json::to_vec(&rc_matching).unwrap()).unwrap();
        assert!(!observe_receipt(&paths, &mut clk).unwrap());
        assert!(!state_dir.join("setup-continuation.json").exists());
        assert!(rc_path.exists());
    }

    struct LockWaitClock {
        wall: u64,
        monotonic: u64,
        held: std::rc::Rc<std::cell::RefCell<Option<File>>>,
        release_on_sleep: bool,
    }

    impl WaitClock for LockWaitClock {
        fn now_ms(&self) -> u64 {
            self.wall
        }
        fn monotonic_ms(&self) -> u64 {
            self.monotonic
        }
        fn sleep_ms(&mut self, ms: u64) {
            self.monotonic += ms;
            assert!(
                self.monotonic <= 5000,
                "lock wait exceeded the monotonic budget"
            );
            if self.release_on_sleep {
                self.wall += 3000;
                self.held.borrow_mut().take();
            } else {
                self.wall = self.wall.saturating_sub(1000);
            }
        }
    }

    #[test]
    fn expiry_during_publication_lock_wait_preserves_prior_mailbox() {
        let contract = get_contract();
        let root = tempdir().unwrap();
        let paths = HandoffPaths {
            local_appdata: root.path().join("local"),
            state_dir: root.path().join("state"),
        };
        fs::create_dir_all(paths.solstone_dir()).unwrap();
        let mailbox = paths.solstone_dir().join(&contract.files.mailbox);
        fs::write(&mailbox, b"prior handoff").unwrap();
        let lock_path = paths.solstone_dir().join(&contract.files.lock);
        let held = std::rc::Rc::new(std::cell::RefCell::new(None));
        let lock_slot = held.clone();
        let endpoints = MockEndpoints {
            authority_current: true,
            identity_after_mint: None,
            identity: Ok(sample_identity()),
            pair_start: Ok(
                json!({"pair_link": sample_v04_link(&[0xa0;16]), "ca_fingerprint":"a0".repeat(32), "expires_in":1}),
            ),
            pair_start_called: std::cell::Cell::new(false),
            on_pair_start: Some(Box::new(move || {
                let file = File::options()
                    .read(true)
                    .write(true)
                    .open(&lock_path)
                    .unwrap();
                file.try_lock().unwrap();
                *lock_slot.borrow_mut() = Some(file);
            })),
        };
        let mut launched = false;
        let outcome = continue_in_solstone(HandoffCollaborators {
            paths: paths.clone(),
            endpoints: &endpoints,
            prepare: |_| Ok(()),
            launch: |_, _, _| {
                launched = true;
                Ok(())
            },
            still_current: || true,
            clock: LockWaitClock {
                wall: 1000,
                monotonic: 0,
                held,
                release_on_sleep: true,
            },
            new_id: || "01234567-89ab-4cde-8f01-23456789abcd".into(),
        });
        assert_eq!(outcome, ContinueOutcome::Failed);
        assert!(!launched);
        assert_eq!(fs::read(mailbox).unwrap(), b"prior handoff");
        assert!(read_pending(&paths.state_dir).unwrap().is_some());
    }

    #[test]
    fn unsupported_image_or_changed_authority_never_publishes_or_launches() {
        let contract = get_contract();
        for case in 0..3 {
            let root = tempdir().unwrap();
            let paths = HandoffPaths {
                local_appdata: root.path().join("local"),
                state_dir: root.path().join("state"),
            };
            let mut other = sample_identity();
            other["instance_id"] = json!("replacement-instance");
            let endpoints = MockEndpoints {
                authority_current: case != 2,
                identity_after_mint: if case == 1 { Some(other) } else { None },
                identity: Ok(sample_identity()),
                pair_start: Ok(
                    json!({"pair_link": sample_v04_link(&[0xa0;16]), "ca_fingerprint":"a0".repeat(32), "expires_in":60}),
                ),
                pair_start_called: std::cell::Cell::new(false),
                on_pair_start: None,
            };
            let mut launched = false;
            let outcome = continue_in_solstone(HandoffCollaborators {
                paths: paths.clone(),
                endpoints: &endpoints,
                prepare: |_| {
                    if case == 0 {
                        Err(CapabilityResult::NoResource)
                    } else {
                        Ok(())
                    }
                },
                launch: |_, _, _| {
                    launched = true;
                    Ok(())
                },
                still_current: || true,
                clock: VirtualClock::new(1000),
                new_id: || "01234567-89ab-4cde-8f01-23456789abcd".into(),
            });
            assert_eq!(
                outcome,
                if case == 0 {
                    ContinueOutcome::Unsupported
                } else {
                    ContinueOutcome::Failed
                }
            );
            assert!(!launched);
            assert!(!paths.solstone_dir().join(&contract.files.mailbox).exists());
            if case == 0 {
                assert!(!endpoints.pair_start_called.get());
            }
        }
    }

    #[test]
    fn prepared_image_lives_through_launch_and_expiry_is_rechecked_after_publication() {
        struct ImageGuard(std::rc::Rc<std::cell::Cell<bool>>);
        impl Drop for ImageGuard {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let contract = get_contract();
        for expire_before_launch in [false, true] {
            let root = tempdir().unwrap();
            let paths = HandoffPaths {
                local_appdata: root.path().join("local"),
                state_dir: root.path().join("state"),
            };
            let endpoints = MockEndpoints {
                authority_current: true,
                identity_after_mint: None,
                identity: Ok(sample_identity()),
                pair_start: Ok(
                    json!({"pair_link":sample_v04_link(&[0xa0;16]),"ca_fingerprint":"a0".repeat(32),"expires_in":60}),
                ),
                pair_start_called: std::cell::Cell::new(false),
                on_pair_start: None,
            };
            let alive = std::rc::Rc::new(std::cell::Cell::new(false));
            let clock = VirtualClock::new(1000);
            let now = clock.now.clone();
            let checks = std::cell::Cell::new(0);
            let mut launched = false;
            let outcome = continue_in_solstone(HandoffCollaborators {
                paths: paths.clone(),
                endpoints: &endpoints,
                prepare: |_| {
                    alive.set(true);
                    Ok(ImageGuard(alive.clone()))
                },
                launch: |guard: &ImageGuard, _, _| {
                    assert!(guard.0.get());
                    launched = true;
                    Ok(())
                },
                still_current: || {
                    checks.set(checks.get() + 1);
                    if expire_before_launch && checks.get() == 6 {
                        now.store(61_000, std::sync::atomic::Ordering::SeqCst);
                    }
                    true
                },
                clock,
                new_id: || "01234567-89ab-4cde-8f01-23456789abcd".into(),
            });
            assert_eq!(
                outcome,
                if expire_before_launch {
                    ContinueOutcome::Failed
                } else {
                    ContinueOutcome::Published
                }
            );
            assert_eq!(launched, !expire_before_launch);
            assert!(!alive.get());
            let mailbox: Value = serde_json::from_slice(
                &fs::read(paths.solstone_dir().join(&contract.files.mailbox)).unwrap(),
            )
            .unwrap();
            assert_eq!(mailbox["expires_at_unix_ms"], 61_000);
            assert!(read_pending(&paths.state_dir).unwrap().is_some());
        }
    }

    #[test]
    fn backward_wall_clock_does_not_extend_lock_budget() {
        let root = tempdir().unwrap();
        let path = root.path().join("handoff.lock");
        let file = File::create(&path).unwrap();
        file.try_lock().unwrap();
        let held = std::rc::Rc::new(std::cell::RefCell::new(Some(file)));
        let mut clock = LockWaitClock {
            wall: 100_000,
            monotonic: 0,
            held,
            release_on_sleep: false,
        };
        assert!(matches!(
            HandoffLock::acquire(&path, 5000, &mut clock),
            Err(HandoffError::Busy)
        ));
        assert_eq!(clock.monotonic, 5000);
        assert_eq!(clock.wall, 0);
    }

    #[test]
    fn pending_publication_and_bounded_read_fail_closed() {
        let root = tempdir().unwrap();
        let id = "01234567-89ab-4cde-8f01-23456789abcd";
        write_pending(root.path(), id).unwrap();
        assert_eq!(read_pending(root.path()).unwrap().unwrap().handoff_id, id);
        let path = root.path().join("setup-continuation.json");
        fs::write(&path, vec![b' '; 1025]).unwrap();
        assert!(read_pending(root.path()).is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(write_pending(root.path(), id).is_err());
        assert!(path.is_dir());
    }

    #[test]
    fn uncertain_publication_never_advances() {
        use std::io::Error;
        assert!(require_confirmed_publication(DetailedAtomicOutcome::Published).is_ok());
        for outcome in [
            DetailedAtomicOutcome::PublishedDurabilityUncertain {
                source: Error::other("sync failed"),
            },
            DetailedAtomicOutcome::PublishedParentPathRaced { sync_error: None },
            DetailedAtomicOutcome::PublishedParentPathUnverified {
                observation: Error::other("observation failed"),
                sync_error: None,
            },
        ] {
            assert!(require_confirmed_publication(outcome).is_err());
        }
    }

    #[test]
    fn handoff_lock_is_exclusive_until_drop() {
        let contract = get_contract();
        let dir = tempdir().unwrap();
        let lock_path = dir.path().join(&contract.files.lock);

        let holder_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        holder_file.try_lock().unwrap();

        let mut vclock = VirtualClock::new(0);

        // Bound is contract lock_acquire_ms
        let err = HandoffLock::acquire(&lock_path, contract.limits.lock_acquire_ms, &mut vclock)
            .unwrap_err();
        assert!(matches!(err, HandoffError::Busy));
        assert!(
            vclock.total_slept.load(std::sync::atomic::Ordering::SeqCst)
                >= contract.limits.lock_acquire_ms
        );

        // Bound 0 on held lock returns busy and virtual clock's sleep count stays 0
        let mut zero_clock = VirtualClock::new(0);
        let err0 = HandoffLock::acquire(&lock_path, 0, &mut zero_clock).unwrap_err();
        assert!(matches!(err0, HandoffError::Busy));
        assert_eq!(
            zero_clock
                .sleep_count
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        // Drop holder, acquire succeeds
        drop(holder_file);
        let mut acquire_clock = VirtualClock::new(0);
        let lock_ok = HandoffLock::acquire(
            &lock_path,
            contract.limits.lock_acquire_ms,
            &mut acquire_clock,
        );
        assert!(lock_ok.is_ok());
    }
}
