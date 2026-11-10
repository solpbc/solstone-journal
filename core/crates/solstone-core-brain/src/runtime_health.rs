// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use getrandom::fill as fill_random;
use serde_json::{Map, Value};
use solstone_core_journal_io::{FileLock, JsonWriteOptions, LockOptions, hold_lock, write_json};

use crate::fixture::local_contract;

// Retains SCHEMA_VERSION from the retired Python runtime-health implementation.
// This version is distinct from brain_state.schema_version.
const RUNTIME_HEALTH_SCHEMA_VERSION: u64 = 1;
const RUNTIME_RECORD_MODE: u32 = 0o600;
const RUNTIME_PROVIDERS: &[&str] = &["local", "parakeet"];

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeRecordInspection {
    pub status: String,
    pub provider: String,
    pub record_kind: Option<String>,
    pub path: PathBuf,
    pub record: Option<Value>,
    pub reason_code: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeRetryRecord {
    pub provider: String,
    pub revision: u64,
    pub token_id: Option<String>,
    pub desired_fingerprint_sha256: Option<String>,
    pub requested_at: Option<String>,
    pub reason_code: Option<String>,
    pub owner: Option<Map<String, Value>>,
}

impl RuntimeRetryRecord {
    pub fn pending(&self) -> bool {
        self.token_id.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeRetryError {
    InvalidProvider(String),
    HealthRevisionConflict,
    RetryRevisionConflict,
    DesiredFingerprintConflict,
    PhaseNotFailed,
    RetryAlreadyRequested,
    Malformed(String),
    Unavailable(String),
    Random,
}

impl fmt::Display for RuntimeRetryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProvider(provider) => {
                write!(formatter, "unsupported runtime provider {provider}")
            }
            Self::HealthRevisionConflict => formatter.write_str("stale runtime health revision"),
            Self::RetryRevisionConflict => formatter.write_str("stale retry-token revision"),
            Self::DesiredFingerprintConflict => {
                formatter.write_str("runtime desired fingerprint changed")
            }
            Self::PhaseNotFailed => {
                formatter.write_str("runtime retry requires a terminal failure")
            }
            Self::RetryAlreadyRequested => formatter.write_str("runtime retry already requested"),
            Self::Malformed(message) | Self::Unavailable(message) => formatter.write_str(message),
            Self::Random => formatter.write_str("could not generate runtime retry token"),
        }
    }
}

impl Error for RuntimeRetryError {}

/// The provider operation lock also serializes runtime health publications.
pub fn hold_runtime_operation(
    journal: &Path,
    provider: &str,
) -> Result<FileLock, RuntimeRetryError> {
    let provider = validated_provider(provider)?;
    hold_lock(
        operation_path(journal, provider),
        LockOptions {
            mode: Some(RUNTIME_RECORD_MODE),
            ..LockOptions::default()
        },
    )
    .map_err(|error| RuntimeRetryError::Unavailable(error.to_string()))
}

pub fn runtime_retry_token_path(
    journal: &Path,
    provider: &str,
) -> Result<PathBuf, RuntimeRetryError> {
    Ok(retry_path(journal, validated_provider(provider)?))
}

/// Holds one provider operation lock from observation through publication.
/// Callers retain their own owner-request or internal retry admission policy.
pub struct RuntimeRetryTransaction {
    provider: String,
    path: PathBuf,
    _lock: FileLock,
}

impl RuntimeRetryTransaction {
    pub fn begin(journal: &Path, provider: &str) -> Result<Self, RuntimeRetryError> {
        Ok(Self {
            provider: validated_provider(provider)?.to_owned(),
            path: runtime_retry_token_path(journal, provider)?,
            _lock: hold_runtime_operation(journal, provider)?,
        })
    }

    /// Read without repairing or setting aside a corrupt record.
    pub fn read_record(&self) -> Result<RuntimeRetryRecord, RuntimeRetryError> {
        read_retry_record(&self.path, &self.provider)
    }

    /// Explicit runtime repair; inspection and owner requests never use this.
    pub fn repair_record(&self) -> Result<RuntimeRetryRecord, RuntimeRetryError> {
        use solstone_core_journal_io::durability::{
            ArtifactId, DurableRead, read_json_durable_validated,
        };
        let result = read_json_durable_validated::<Value>(
            ArtifactId::ProviderRetryToken,
            &self.path,
            |value| {
                // Runtime reads require a persisted revision; read-only legacy
                // inspection retains its missing-revision default of zero.
                required_u64(&value["revision"])?;
                retry_record_from_value(value.clone(), &self.provider).map(|_| ())
            },
        )
        .map_err(|error| RuntimeRetryError::Unavailable(error.to_string()))?;
        match result {
            DurableRead::Present(value) => {
                retry_record_from_value(value, &self.provider).map_err(RuntimeRetryError::Malformed)
            }
            DurableRead::Absent | DurableRead::SetAside(_) => {
                Ok(synthetic_retry_record(&self.provider))
            }
            DurableRead::Unreadable { .. } => Err(RuntimeRetryError::Unavailable(
                "runtime retry-token record is unreadable".to_owned(),
            )),
        }
    }

    pub fn publish(&self, record: &RuntimeRetryRecord) -> Result<(), RuntimeRetryError> {
        if record.provider != self.provider {
            return Err(RuntimeRetryError::InvalidProvider(record.provider.clone()));
        }
        write_json(
            &self.path,
            &retry_record_value(record),
            JsonWriteOptions {
                mode: Some(RUNTIME_RECORD_MODE),
                indent: Some(2),
                sort_keys: true,
            },
        )
        .map_err(|error| RuntimeRetryError::Unavailable(error.to_string()))
    }
}

/// Whether a local model download or install is under way: the install
/// record says so and its installer still holds the install lease. A record
/// left in flight by an installer that exited (killed, crashed, or stopped
/// with its journal) is not in flight. An unreadable or absent record, or a
/// lease that cannot be probed, is not in flight either.
pub fn local_install_in_flight(journal_path: &Path) -> bool {
    let Ok(bytes) = fs::read(journal_path.join("health/providers/local.json")) else {
        return false;
    };
    let recorded_in_flight = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|record| {
            record
                .get("install_state")
                .and_then(Value::as_str)
                .map(|state| {
                    matches!(
                        state,
                        "resolving" | "downloading" | "verifying" | "installing"
                    )
                })
        })
        .unwrap_or(false);
    recorded_in_flight && install_lease_held(&journal_path.join("health/providers/local.lease"))
}

/// Probe the installer's lease the way the installer's own observers do: open
/// it read-only and take a momentary shared lock, so the probe never makes a
/// starting installer or another observer read the lease as busy.
fn install_lease_held(path: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        matches!(
            nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockSharedNonblock),
            Err((_file, nix::errno::Errno::EWOULDBLOCK))
        )
    }
    #[cfg(not(unix))]
    {
        matches!(file.try_lock_shared(), Err(fs::TryLockError::WouldBlock))
    }
}

pub fn inspect_runtime_health(journal_path: &Path) -> RuntimeRecordInspection {
    inspect_runtime_health_for_provider(journal_path, "local")
}

/// Inspect a runtime retry-token record without exposing a mutation handle.
pub fn inspect_runtime_retry_token(journal_path: &Path, provider: &str) -> RuntimeRecordInspection {
    let provider = match validated_provider(provider) {
        Ok(provider) => provider,
        Err(error) => {
            return RuntimeRecordInspection {
                status: "corrupt".to_owned(),
                provider: provider.to_owned(),
                record_kind: Some("retry-token".to_owned()),
                path: journal_path.join("health/providers/runtime"),
                record: None,
                reason_code: Some("record-malformed".to_owned()),
                error: Some(error.to_string()),
            };
        }
    };
    let path = retry_path(journal_path, provider);
    match fs::read(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => RuntimeRecordInspection {
            status: "ok".to_owned(),
            provider: provider.to_owned(),
            record_kind: Some("retry-token".to_owned()),
            path,
            record: Some(retry_record_value(&synthetic_retry_record(provider))),
            reason_code: None,
            error: None,
        },
        Err(error) => RuntimeRecordInspection {
            status: "unavailable".to_owned(),
            provider: provider.to_owned(),
            record_kind: Some("retry-token".to_owned()),
            path,
            record: None,
            reason_code: Some("record-unavailable".to_owned()),
            error: Some(error.to_string()),
        },
        Ok(bytes) => match decode_retry_record(&bytes, provider) {
            Ok(record) => RuntimeRecordInspection {
                status: "ok".to_owned(),
                provider: provider.to_owned(),
                record_kind: Some("retry-token".to_owned()),
                path,
                record: Some(retry_record_value(&record)),
                reason_code: None,
                error: None,
            },
            Err(error) => RuntimeRecordInspection {
                status: "corrupt".to_owned(),
                provider: provider.to_owned(),
                record_kind: Some("retry-token".to_owned()),
                path,
                record: None,
                reason_code: Some("record-malformed".to_owned()),
                error: Some(error.to_string()),
            },
        },
    }
}

/// Request a retry token only if the caller's displayed runtime state is current.
pub fn request_runtime_retry(
    journal: &Path,
    provider: &str,
    expected_health_revision: u64,
    expected_retry_revision: u64,
    desired_fingerprint_sha256: &str,
    owner: Map<String, Value>,
) -> Result<RuntimeRetryRecord, RuntimeRetryError> {
    let provider = validated_provider(provider)?;
    let health_path = health_path(journal, provider);
    let transaction = RuntimeRetryTransaction::begin(journal, provider)?;
    let health = read_health_record(&health_path, provider)?;
    let retry = transaction.read_record()?;

    if health["revision"].as_u64() != Some(expected_health_revision) {
        return Err(RuntimeRetryError::HealthRevisionConflict);
    }
    if retry.revision != expected_retry_revision {
        return Err(RuntimeRetryError::RetryRevisionConflict);
    }
    if health["desired_fingerprint_sha256"].as_str() != Some(desired_fingerprint_sha256) {
        return Err(RuntimeRetryError::DesiredFingerprintConflict);
    }
    if health["phase"].as_str() != Some("failed") {
        return Err(RuntimeRetryError::PhaseNotFailed);
    }
    if retry.token_id.is_some()
        && retry.desired_fingerprint_sha256.as_deref() == Some(desired_fingerprint_sha256)
    {
        return Err(RuntimeRetryError::RetryAlreadyRequested);
    }

    let record = RuntimeRetryRecord {
        provider: provider.to_owned(),
        revision: retry.revision + 1,
        token_id: Some(retry_token_id()?),
        desired_fingerprint_sha256: Some(desired_fingerprint_sha256.to_owned()),
        requested_at: Some(Utc::now().to_rfc3339()),
        reason_code: Some("retry-token-requested".to_owned()),
        owner: Some(owner),
    };
    transaction.publish(&record)?;
    Ok(record)
}

fn inspect_runtime_health_for_provider(
    journal_path: &Path,
    provider: &str,
) -> RuntimeRecordInspection {
    let path = health_path(journal_path, provider);
    match fs::read(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => RuntimeRecordInspection {
            status: "ok".to_owned(),
            provider: provider.to_owned(),
            record_kind: Some("health".to_owned()),
            path,
            record: Some(synthetic_stopped_record(provider)),
            reason_code: None,
            error: None,
        },
        Err(error) => RuntimeRecordInspection {
            status: "unavailable".to_owned(),
            provider: provider.to_owned(),
            record_kind: Some("health".to_owned()),
            path,
            record: None,
            reason_code: Some("record-unavailable".to_owned()),
            error: Some(error.to_string()),
        },
        Ok(bytes) => match decode_health_record(&bytes, provider) {
            Ok(record) => RuntimeRecordInspection {
                status: "ok".to_owned(),
                provider: provider.to_owned(),
                record_kind: Some("health".to_owned()),
                path,
                record: Some(record),
                reason_code: None,
                error: None,
            },
            Err(error) => RuntimeRecordInspection {
                status: "corrupt".to_owned(),
                provider: provider.to_owned(),
                record_kind: Some("health".to_owned()),
                path,
                record: None,
                reason_code: Some("record-malformed".to_owned()),
                error: Some(error.to_string()),
            },
        },
    }
}

fn validated_provider(provider: &str) -> Result<&str, RuntimeRetryError> {
    RUNTIME_PROVIDERS
        .contains(&provider)
        .then_some(provider)
        .ok_or_else(|| RuntimeRetryError::InvalidProvider(provider.to_owned()))
}

fn runtime_directory(journal: &Path) -> PathBuf {
    journal.join("health/providers/runtime")
}

fn health_path(journal: &Path, provider: &str) -> PathBuf {
    runtime_directory(journal).join(format!("{provider}.json"))
}

fn retry_path(journal: &Path, provider: &str) -> PathBuf {
    runtime_directory(journal).join(format!("{provider}.retry-token.json"))
}

fn operation_path(journal: &Path, provider: &str) -> PathBuf {
    runtime_directory(journal).join(format!("{provider}.operation"))
}

fn read_health_record(path: &Path, provider: &str) -> Result<Value, RuntimeRetryError> {
    match fs::read(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(synthetic_stopped_record(provider))
        }
        Err(error) => Err(RuntimeRetryError::Unavailable(error.to_string())),
        Ok(bytes) => decode_health_record(&bytes, provider),
    }
}

fn decode_health_record(bytes: &[u8], provider: &str) -> Result<Value, RuntimeRetryError> {
    serde_json::from_slice::<Value>(bytes)
        .map_err(|error| RuntimeRetryError::Malformed(error.to_string()))
        .and_then(|value| {
            coerce_record_for_provider(value, provider).map_err(RuntimeRetryError::Malformed)
        })
}

fn read_retry_record(path: &Path, provider: &str) -> Result<RuntimeRetryRecord, RuntimeRetryError> {
    match fs::read(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(synthetic_retry_record(provider))
        }
        Err(error) => Err(RuntimeRetryError::Unavailable(error.to_string())),
        Ok(bytes) => decode_retry_record(&bytes, provider),
    }
}

fn decode_retry_record(
    bytes: &[u8],
    provider: &str,
) -> Result<RuntimeRetryRecord, RuntimeRetryError> {
    let value = serde_json::from_slice::<Value>(bytes)
        .map_err(|error| RuntimeRetryError::Malformed(error.to_string()))?;
    retry_record_from_value(value, provider).map_err(RuntimeRetryError::Malformed)
}

fn synthetic_retry_record(provider: &str) -> RuntimeRetryRecord {
    RuntimeRetryRecord {
        provider: provider.to_owned(),
        revision: 0,
        token_id: None,
        desired_fingerprint_sha256: None,
        requested_at: None,
        reason_code: None,
        owner: None,
    }
}

fn retry_record_from_value(value: Value, provider: &str) -> Result<RuntimeRetryRecord, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("runtime retry-token record must be an object for {provider}"))?;
    let schema_version = object
        .get("schema_version")
        .map_or(Ok(RUNTIME_HEALTH_SCHEMA_VERSION), required_u64)?;
    if schema_version != RUNTIME_HEALTH_SCHEMA_VERSION {
        return Err(format!(
            "unsupported runtime retry-token schema_version for {provider}"
        ));
    }
    if object
        .get("provider")
        .is_some_and(|value| value.as_str() != Some(provider))
    {
        return Err(format!(
            "runtime retry-token provider does not match {provider}"
        ));
    }
    let token_id = optional_string(object, "token_id")?;
    let desired_fingerprint_sha256 = optional_string(object, "desired_fingerprint_sha256")?;
    let requested_at = optional_string(object, "requested_at")?;
    let reason_code = optional_reason_code(object.get("reason_code"))?;
    let owner = optional_object(object, "owner")?;
    if token_id.is_none()
        && (desired_fingerprint_sha256.is_some()
            || requested_at.is_some()
            || reason_code.is_some()
            || owner.is_some())
    {
        return Err("cleared retry-token record cannot carry token fields".to_owned());
    }
    if token_id.is_some() && (requested_at.is_none() || reason_code.is_none()) {
        return Err("outstanding retry-token requires requested_at and reason_code".to_owned());
    }
    Ok(RuntimeRetryRecord {
        provider: provider.to_owned(),
        revision: default_nonnegative_u64(object, "revision")?,
        token_id,
        desired_fingerprint_sha256,
        requested_at,
        reason_code,
        owner,
    })
}

fn retry_record_value(record: &RuntimeRetryRecord) -> Value {
    Value::Object(Map::from_iter([
        (
            "schema_version".to_owned(),
            Value::from(RUNTIME_HEALTH_SCHEMA_VERSION),
        ),
        (
            "provider".to_owned(),
            Value::String(record.provider.clone()),
        ),
        ("revision".to_owned(), Value::from(record.revision)),
        (
            "token_id".to_owned(),
            record.token_id.clone().map_or(Value::Null, Value::String),
        ),
        (
            "desired_fingerprint_sha256".to_owned(),
            record
                .desired_fingerprint_sha256
                .clone()
                .map_or(Value::Null, Value::String),
        ),
        (
            "requested_at".to_owned(),
            record
                .requested_at
                .clone()
                .map_or(Value::Null, Value::String),
        ),
        (
            "reason_code".to_owned(),
            record
                .reason_code
                .clone()
                .map_or(Value::Null, Value::String),
        ),
        (
            "owner".to_owned(),
            record.owner.clone().map_or(Value::Null, Value::Object),
        ),
    ]))
}

fn retry_token_id() -> Result<String, RuntimeRetryError> {
    let mut bytes = [0_u8; 16];
    fill_random(&mut bytes).map_err(|_| RuntimeRetryError::Random)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
pub(crate) fn inspection_from_fixture(value: &Value) -> RuntimeRecordInspection {
    let object = value.as_object();
    RuntimeRecordInspection {
        status: object
            .and_then(|object| object.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("ok")
            .to_owned(),
        provider: "local".to_owned(),
        record_kind: object
            .and_then(|object| object.get("record_kind"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        path: PathBuf::from("fixture"),
        record: object.and_then(|object| object.get("record")).cloned(),
        reason_code: object
            .and_then(|object| object.get("reason_code"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        error: object
            .and_then(|object| object.get("error"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn coerce_record_for_provider(value: Value, provider: &str) -> Result<Value, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "runtime health record must be an object".to_owned())?;
    let schema_version = object
        .get("schema_version")
        .map_or(Ok(RUNTIME_HEALTH_SCHEMA_VERSION), required_u64)
        .map_err(|error| format!("runtime health {error}"))?;
    if schema_version != RUNTIME_HEALTH_SCHEMA_VERSION {
        return Err(format!(
            "unsupported runtime health schema_version for {provider}"
        ));
    }
    let phase = object
        .get("phase")
        .and_then(Value::as_str)
        .ok_or_else(|| "runtime health phase must be a string".to_owned())?;
    if !local_contract()
        .brain_state
        .runtime_phases
        .iter()
        .any(|candidate| candidate == phase)
    {
        return Err("runtime health phase is unknown".to_owned());
    }
    let detail = object
        .get("detail")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    if !detail.is_object() {
        return Err("runtime health detail must be an object".to_owned());
    }
    let process = optional_object(object, "process")?;
    let owner = optional_object(object, "owner")?;
    let reason_code = optional_reason_code(object.get("reason_code"))?;
    let desired_fingerprint = optional_string(object, "desired_fingerprint_sha256")?;
    let incarnation = optional_string(object, "incarnation")?;
    let updated_at = optional_string(object, "updated_at")?;
    let display_deadline_at = optional_string(object, "display_deadline_at")?;
    Ok(Value::Object(Map::from_iter([
        (
            "schema_version".to_owned(),
            Value::from(RUNTIME_HEALTH_SCHEMA_VERSION),
        ),
        ("provider".to_owned(), Value::String(provider.to_owned())),
        (
            "revision".to_owned(),
            Value::from(default_nonnegative_u64(object, "revision")?),
        ),
        ("phase".to_owned(), Value::String(phase.to_owned())),
        (
            "reason_code".to_owned(),
            reason_code.map_or(Value::Null, Value::String),
        ),
        ("detail".to_owned(), detail),
        (
            "desired_fingerprint_sha256".to_owned(),
            desired_fingerprint.map_or(Value::Null, Value::String),
        ),
        (
            "incarnation".to_owned(),
            incarnation.map_or(Value::Null, Value::String),
        ),
        (
            "generation".to_owned(),
            Value::from(default_nonnegative_u64(object, "generation")?),
        ),
        (
            "attempt".to_owned(),
            Value::from(default_nonnegative_u64(object, "attempt")?),
        ),
        (
            "process".to_owned(),
            process.map_or(Value::Null, Value::Object),
        ),
        (
            "updated_at".to_owned(),
            updated_at.map_or(Value::Null, Value::String),
        ),
        (
            "display_deadline_at".to_owned(),
            display_deadline_at.map_or(Value::Null, Value::String),
        ),
        ("owner".to_owned(), owner.map_or(Value::Null, Value::Object)),
    ])))
}

fn required_u64(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .ok_or_else(|| "value must be a nonnegative integer".to_owned())
}

fn default_nonnegative_u64(object: &Map<String, Value>, field: &str) -> Result<u64, String> {
    object
        .get(field)
        .map(required_u64)
        .unwrap_or(Ok(0))
        .map_err(|_| format!("{field} must be a nonnegative integer"))
}

fn optional_string(object: &Map<String, Value>, field: &str) -> Result<Option<String>, String> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{field} must be a string or null")),
    }
}

fn optional_object(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<Map<String, Value>>, String> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("runtime health {field} must be object/null")),
    }
}

fn optional_reason_code(value: Option<&Value>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(reason) = value.as_str() else {
        return Err("invalid runtime health reason_code".to_owned());
    };
    if local_contract()
        .brain_state
        .runtime_reason_codes
        .iter()
        .any(|candidate| candidate == reason)
    {
        Ok(Some(reason.to_owned()))
    } else {
        Err("invalid runtime health reason_code".to_owned())
    }
}

fn synthetic_stopped_record(provider: &str) -> Value {
    Value::Object(Map::from_iter([
        (
            "schema_version".to_owned(),
            Value::from(RUNTIME_HEALTH_SCHEMA_VERSION),
        ),
        ("provider".to_owned(), Value::String(provider.to_owned())),
        ("revision".to_owned(), Value::from(0)),
        ("phase".to_owned(), Value::String("stopped".to_owned())),
        ("reason_code".to_owned(), Value::Null),
        ("detail".to_owned(), Value::Object(Map::new())),
        ("desired_fingerprint_sha256".to_owned(), Value::Null),
        ("incarnation".to_owned(), Value::Null),
        ("generation".to_owned(), Value::from(0)),
        ("attempt".to_owned(), Value::from(0)),
        ("process".to_owned(), Value::Null),
        ("updated_at".to_owned(), Value::Null),
        ("display_deadline_at".to_owned(), Value::Null),
        ("owner".to_owned(), Value::Null),
    ]))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::{Map, Value, json};

    use super::{
        RuntimeRetryError, RuntimeRetryRecord, health_path, inspect_runtime_retry_token,
        local_install_in_flight, request_runtime_retry, retry_path, retry_record_value,
    };

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestJournal(PathBuf);

    impl TestJournal {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "solstone-runtime-retry-test-{}-{}-{sequence}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock after epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("test journal creates");
            Self(path)
        }

        fn write_health(&self, revision: u64, phase: &str, fingerprint: &str) {
            let path = health_path(&self.0, "local");
            fs::create_dir_all(path.parent().expect("runtime directory"))
                .expect("runtime directory");
            fs::write(
                path,
                serde_json::to_vec(&json!({
                    "schema_version": 1,
                    "provider": "local",
                    "revision": revision,
                    "phase": phase,
                    "reason_code": null,
                    "detail": {},
                    "desired_fingerprint_sha256": fingerprint,
                    "incarnation": null,
                    "generation": 0,
                    "attempt": 0,
                    "process": null,
                    "updated_at": null,
                    "display_deadline_at": null,
                    "owner": null,
                }))
                .expect("health record serializes"),
            )
            .expect("health record writes");
        }

        fn write_retry(&self, revision: u64, token_id: Option<&str>, fingerprint: Option<&str>) {
            let path = retry_path(&self.0, "local");
            fs::create_dir_all(path.parent().expect("runtime directory"))
                .expect("runtime directory");
            let outstanding = token_id.is_some();
            let record = RuntimeRetryRecord {
                provider: "local".to_owned(),
                revision,
                token_id: token_id.map(str::to_owned),
                desired_fingerprint_sha256: fingerprint.map(str::to_owned),
                requested_at: outstanding.then(|| "2026-01-01T00:00:00+00:00".to_owned()),
                reason_code: outstanding.then(|| "retry-token-requested".to_owned()),
                owner: outstanding.then(Map::new),
            };
            fs::write(
                path,
                serde_json::to_vec(&retry_record_value(&record)).expect("retry record serializes"),
            )
            .expect("retry record writes");
        }
    }

    #[test]
    fn a_local_model_download_is_in_flight_until_it_settles() {
        let journal = TestJournal::new();
        assert!(!local_install_in_flight(&journal.0));
        fs::create_dir_all(journal.0.join("health/providers")).expect("providers dir");
        let record = journal.0.join("health/providers/local.json");
        let installer = fs::File::create(journal.0.join("health/providers/local.lease"))
            .expect("install lease");
        #[cfg(unix)]
        let installer = nix::fcntl::Flock::lock(installer, nix::fcntl::FlockArg::LockExclusive)
            .expect("installer holds the lease");
        #[cfg(not(unix))]
        installer.lock().expect("installer holds the lease");
        for (state, in_flight) in [
            ("resolving", true),
            ("downloading", true),
            ("verifying", true),
            ("installing", true),
            ("installed", false),
            ("failed", false),
            ("cancelled", false),
        ] {
            fs::write(&record, json!({"install_state": state}).to_string()).expect("record");
            assert_eq!(local_install_in_flight(&journal.0), in_flight, "{state}");
        }
        fs::write(&record, b"not json").expect("record");
        assert!(!local_install_in_flight(&journal.0));
        drop(installer);
    }

    #[test]
    fn a_download_its_installer_left_behind_is_not_in_flight() {
        let journal = TestJournal::new();
        fs::create_dir_all(journal.0.join("health/providers")).expect("providers dir");
        fs::write(
            journal.0.join("health/providers/local.json"),
            json!({"install_state": "downloading"}).to_string(),
        )
        .expect("record");
        let lease = journal.0.join("health/providers/local.lease");
        assert!(!local_install_in_flight(&journal.0), "no lease file");

        let installer = fs::File::create(&lease).expect("install lease");
        #[cfg(unix)]
        let installer = nix::fcntl::Flock::lock(installer, nix::fcntl::FlockArg::LockExclusive)
            .expect("installer holds the lease");
        #[cfg(not(unix))]
        installer.lock().expect("installer holds the lease");
        assert!(local_install_in_flight(&journal.0), "installer running");

        drop(installer);
        assert!(!local_install_in_flight(&journal.0), "installer exited");
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn install_observers_share_the_lease_and_never_create_a_missing_one() {
        let journal = TestJournal::new();
        fs::create_dir_all(journal.0.join("health/providers")).unwrap();
        fs::write(
            journal.0.join("health/providers/local.json"),
            json!({"install_state": "downloading"}).to_string(),
        )
        .unwrap();
        let lease = journal.0.join("health/providers/local.lease");
        assert!(!local_install_in_flight(&journal.0));
        assert!(!lease.exists());
        fs::File::create(&lease).unwrap();
        let observer = nix::fcntl::Flock::lock(
            fs::File::open(&lease).unwrap(),
            nix::fcntl::FlockArg::LockSharedNonblock,
        )
        .unwrap();
        assert!(!local_install_in_flight(&journal.0));
        drop(observer);
        let installer = nix::fcntl::Flock::lock(
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lease)
                .unwrap(),
            nix::fcntl::FlockArg::LockExclusiveNonblock,
        )
        .unwrap();
        assert!(local_install_in_flight(&journal.0));
        drop(installer);
        assert!(!local_install_in_flight(&journal.0));
    }

    impl Drop for TestJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn request(
        journal: &TestJournal,
        health_revision: u64,
        retry_revision: u64,
        fingerprint: &str,
    ) -> Result<RuntimeRetryRecord, RuntimeRetryError> {
        request_runtime_retry(
            &journal.0,
            "local",
            health_revision,
            retry_revision,
            fingerprint,
            Map::from_iter([("source".to_owned(), Value::String("test".to_owned()))]),
        )
    }

    #[test]
    fn retry_codec_validates_provider_and_token_shape() {
        use super::{retry_record_from_value, synthetic_retry_record};

        let cleared = retry_record_value(&synthetic_retry_record("local"));
        assert_eq!(
            retry_record_from_value(cleared.clone(), "local").unwrap(),
            synthetic_retry_record("local")
        );
        let mut pending = cleared.clone();
        pending["revision"] = json!(7);
        pending["token_id"] = json!("retry-1");
        pending["requested_at"] = json!("2026-01-01T00:00:00Z");
        pending["reason_code"] = json!("retry-token-requested");
        let decoded = retry_record_from_value(pending.clone(), "local").unwrap();
        assert_eq!(retry_record_value(&decoded), pending);
        // Missing provider and schema retain the existing wire defaults.
        let mut minimal = pending.clone();
        minimal.as_object_mut().unwrap().remove("provider");
        minimal.as_object_mut().unwrap().remove("schema_version");
        assert_eq!(retry_record_from_value(minimal, "local").unwrap(), decoded);

        for (field, value) in [
            ("provider", json!("parakeet")),
            ("provider", Value::Null),
            ("provider", json!(42)),
            ("schema_version", json!(2)),
            ("schema_version", json!("1")),
            ("revision", json!(-1)),
            ("token_id", json!(42)),
            ("requested_at", Value::Null),
            ("reason_code", Value::Null),
            ("reason_code", json!("unknown")),
            ("owner", json!([])),
        ] {
            let mut invalid = pending.clone();
            invalid[field] = value;
            assert!(
                retry_record_from_value(invalid, "local").is_err(),
                "{field}"
            );
        }
        for (field, value) in [
            ("desired_fingerprint_sha256", json!("fingerprint")),
            ("requested_at", json!("2026-01-01T00:00:00Z")),
            ("reason_code", json!("retry-token-requested")),
            ("owner", json!({})),
        ] {
            let mut invalid = cleared.clone();
            invalid[field] = value;
            assert!(
                retry_record_from_value(invalid, "local").is_err(),
                "{field}"
            );
        }
        assert!(retry_record_from_value(json!([]), "local").is_err());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn corrupt_retry_inspection_and_owner_request_never_repair() {
        let journal = TestJournal::new();
        journal.write_health(7, "failed", "fingerprint");
        journal.write_retry(2, Some("retry-1"), Some("fingerprint"));
        let path = retry_path(&journal.0, "local");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["provider"] = json!("parakeet");
        let bytes = serde_json::to_vec(&value).unwrap();
        fs::write(&path, &bytes).unwrap();

        assert_eq!(
            inspect_runtime_retry_token(&journal.0, "local").status,
            "corrupt"
        );
        assert!(matches!(
            request(&journal, 7, 2, "fingerprint"),
            Err(RuntimeRetryError::Malformed(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let transaction = super::RuntimeRetryTransaction::begin(&journal.0, "local").unwrap();
        assert!(!transaction.repair_record().unwrap().pending());
        assert!(!path.exists());
    }

    #[test]
    fn request_runtime_retry_persists_a_fresh_pending_token() {
        let journal = TestJournal::new();
        journal.write_health(7, "failed", "fingerprint");

        let record = request(&journal, 7, 0, "fingerprint").expect("retry writes");

        assert_eq!(record.revision, 1);
        assert!(record.pending());
        assert_eq!(
            record.desired_fingerprint_sha256.as_deref(),
            Some("fingerprint")
        );
        assert_eq!(record.reason_code.as_deref(), Some("retry-token-requested"));
        assert_eq!(
            record.owner,
            Some(Map::from_iter([("source".to_owned(), json!("test"))]))
        );
        let inspection = inspect_runtime_retry_token(&journal.0, "local");
        assert_eq!(inspection.status, "ok");
        assert_eq!(
            inspection
                .record
                .as_ref()
                .and_then(|value| value["revision"].as_u64()),
            Some(1)
        );
        assert!(
            inspection
                .record
                .as_ref()
                .is_some_and(|value| value["token_id"].is_string())
        );
    }

    #[test]
    fn request_runtime_retry_rejects_stale_health_revision() {
        let journal = TestJournal::new();
        journal.write_health(7, "failed", "fingerprint");

        assert_eq!(
            request(&journal, 6, 0, "fingerprint"),
            Err(RuntimeRetryError::HealthRevisionConflict)
        );
    }

    #[test]
    fn request_runtime_retry_rejects_stale_retry_revision() {
        let journal = TestJournal::new();
        journal.write_health(7, "failed", "fingerprint");
        journal.write_retry(2, None, None);

        assert_eq!(
            request(&journal, 7, 1, "fingerprint"),
            Err(RuntimeRetryError::RetryRevisionConflict)
        );
    }

    #[test]
    fn request_runtime_retry_rejects_changed_desired_fingerprint() {
        let journal = TestJournal::new();
        journal.write_health(7, "failed", "fingerprint");

        assert_eq!(
            request(&journal, 7, 0, "other"),
            Err(RuntimeRetryError::DesiredFingerprintConflict)
        );
    }

    #[test]
    fn request_runtime_retry_requires_a_failed_runtime() {
        let journal = TestJournal::new();
        journal.write_health(7, "ready", "fingerprint");

        assert_eq!(
            request(&journal, 7, 0, "fingerprint"),
            Err(RuntimeRetryError::PhaseNotFailed)
        );
    }

    #[test]
    fn request_runtime_retry_rejects_an_existing_matching_token() {
        let journal = TestJournal::new();
        journal.write_health(7, "failed", "fingerprint");
        journal.write_retry(2, Some("already-requested"), Some("fingerprint"));

        assert_eq!(
            request(&journal, 7, 2, "fingerprint"),
            Err(RuntimeRetryError::RetryAlreadyRequested)
        );
    }
}
