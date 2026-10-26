// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retained archive configuration and one operation's nonrenewable budget.

use crate::HostedBinding;
use serde_json::{Map, Value};
use solstone_core_journal_io::JournalRoot;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const ARCHIVE_BACKUP_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;
pub const ARCHIVE_LS_TIMEOUT_SECONDS: u64 = 30 * 60;

pub trait Clock {
    fn now_unix(&self) -> i64;
    fn iso_week(&self) -> u8;
}

/// A shared refusal signal. Cancellation is irreversible and grants no authority.
///
/// `task` is observed only by this request's child. [`Self::new_child`] does not
/// copy it, so a queue deadline that fails the child leaves the parent lifetime
/// and every sibling child uncancelled.
#[derive(Clone, Default)]
pub struct ArchiveCancellation {
    cancelled: Arc<AtomicBool>,
    ancestors: Arc<[Arc<AtomicBool>]>,
    task: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
}

impl ArchiveCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe an enclosing lifetime without letting this operation cancel it.
    pub fn new_child(parent: &Self) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            ancestors: parent
                .ancestors
                .iter()
                .cloned()
                .chain(std::iter::once(Arc::clone(&parent.cancelled)))
                .collect(),
            task: None,
        }
    }

    /// Watch one admitted task checkpoint from this child only.
    pub fn with_task(mut self, task: Arc<dyn Fn() -> Result<(), String> + Send + Sync>) -> Self {
        self.task = Some(task);
        self
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        if self.cancelled.load(Ordering::Acquire)
            || self
                .ancestors
                .iter()
                .any(|flag| flag.load(Ordering::Acquire))
            || archive_work_stopping()
        {
            return true;
        }
        if let Some(task) = &self.task
            && task().is_err()
        {
            self.cancelled.store(true, Ordering::Release);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::ArchiveCancellation;

    #[test]
    fn lifetime_stop_reaches_children_without_operation_cancel_stopping_siblings() {
        let lifetime = ArchiveCancellation::new();
        let first = ArchiveCancellation::new_child(&lifetime);
        let sibling = ArchiveCancellation::new_child(&lifetime);
        let nested = ArchiveCancellation::new_child(&first);
        first.cancel();
        assert!(first.is_cancelled());
        assert!(nested.is_cancelled());
        assert!(!lifetime.is_cancelled());
        assert!(!sibling.is_cancelled());
        lifetime.cancel();
        assert!(sibling.is_cancelled());
        assert!(!ArchiveCancellation::new().is_cancelled());
    }
}

static ARCHIVE_WORK_STOPPING: AtomicBool = AtomicBool::new(false);
static LIVE_ARCHIVE_CHILDREN: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// The process is stopping: every archive cancellation now reads as cancelled,
/// so running archive children are killed and reaped instead of outliving it.
/// Only an atomic store, so a signal handler may call it.
pub fn stop_archive_work() {
    ARCHIVE_WORK_STOPPING.store(true, Ordering::Release);
}

pub fn archive_work_stopping() -> bool {
    ARCHIVE_WORK_STOPPING.load(Ordering::Acquire)
}

/// Wait until every archive child this process started has been reaped.
/// Returns false if some remain when `timeout` ends.
pub fn wait_for_archive_children(timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while LIVE_ARCHIVE_CHILDREN.load(Ordering::Acquire) != 0 {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    true
}

/// Held from spawn until the child is reaped; see [`wait_for_archive_children`].
/// Only the unix child runner spawns archive children.
#[cfg(unix)]
pub(crate) struct LiveArchiveChild(());
#[cfg(unix)]
impl LiveArchiveChild {
    pub(crate) fn enter() -> Self {
        LIVE_ARCHIVE_CHILDREN.fetch_add(1, Ordering::AcqRel);
        Self(())
    }
}
#[cfg(unix)]
impl Drop for LiveArchiveChild {
    fn drop(&mut self) {
        LIVE_ARCHIVE_CHILDREN.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    ConfigurationUnavailable,
    ConfigurationChanged,
    JournalIdentityChanged,
    DeadlineExceeded,
    ClockChanged,
    CredentialsExpired,
    Cancelled,
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ConfigurationUnavailable => "archive configuration could not be read",
            Self::ConfigurationChanged => "archive configuration changed",
            Self::JournalIdentityChanged => "your journal could not be confirmed",
            Self::DeadlineExceeded => "the archive operation deadline was reached",
            Self::ClockChanged => "the archive clock moved backward",
            Self::CredentialsExpired => "archive credentials expired",
            Self::Cancelled => "the archive operation was cancelled",
        })
    }
}
impl std::error::Error for AdmissionError {}

#[derive(PartialEq)]
pub struct ArchiveConfiguration {
    settings: Map<String, Value>,
    binding: Option<HostedBinding>,
    offload_floor: Option<Value>,
    offload_enabled: Option<Value>,
    offload_budget: Option<Value>,
}

const MAX_ARCHIVE_CONFIGURATION_BYTES: usize = 1024 * 1024;

impl ArchiveConfiguration {
    pub fn offload_enabled(&self) -> bool {
        self.offload_enabled == Some(Value::Bool(true))
    }

    pub fn offload_budget_bytes(&self) -> Result<Option<u64>, AdmissionError> {
        match self.offload_budget.as_ref() {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_u64()
                .map(Some)
                .ok_or(AdmissionError::ConfigurationUnavailable),
        }
    }

    /// Required space from this operation's captured positive reserve and bounds.
    /// The caller must still query the retained target filesystem's capacity.
    pub fn offload_required_bytes(
        &self,
        max_payload: u64,
        metadata: u64,
    ) -> Result<u64, AdmissionError> {
        let floor = self
            .offload_floor
            .as_ref()
            .and_then(Value::as_u64)
            .filter(|floor| *floor > 0)
            .ok_or(AdmissionError::ConfigurationUnavailable)?;
        max_payload
            .checked_add(floor)
            .and_then(|bytes| bytes.checked_add(metadata))
            .ok_or(AdmissionError::ConfigurationUnavailable)
    }

    pub fn enabled(&self) -> bool {
        self.settings.get("enabled") == Some(&Value::Bool(true))
    }

    pub fn mode(&self) -> Option<&str> {
        self.settings.get("mode").and_then(Value::as_str)
    }

    pub fn keys(&self) -> Result<Option<crate::BackupKeys>, AdmissionError> {
        match (
            self.settings.get("daily_key"),
            self.settings.get("recovery_key"),
        ) {
            (Some(Value::Null) | None, _) | (_, Some(Value::Null) | None) => Ok(None),
            (Some(Value::String(daily_key)), Some(Value::String(recovery_key))) => {
                crate::format_recovery_key_display(recovery_key)
                    .map_err(|_| AdmissionError::ConfigurationUnavailable)?;
                Ok(Some(crate::BackupKeys {
                    daily_key: daily_key.clone(),
                    recovery_key: recovery_key.clone(),
                }))
            }
            _ => Err(AdmissionError::ConfigurationUnavailable),
        }
    }

    pub fn destination(&self) -> Option<crate::Destination> {
        let destination = self.settings.get("destination")?.as_object()?;
        let credentials = match destination.get("credentials") {
            None => Map::new(),
            Some(Value::Object(credentials)) => credentials.clone(),
            Some(_) => return None,
        };
        Some(crate::Destination {
            repository: destination.get("repository")?.as_str()?.to_owned(),
            backend: destination.get("backend")?.as_str()?.to_owned(),
            credentials,
        })
    }

    pub fn binding(&self) -> Option<&HostedBinding> {
        self.binding.as_ref()
    }

    pub fn revalidate(&self, root: &JournalRoot) -> Result<(), AdmissionError> {
        if Self::capture(root)? != *self {
            return Err(AdmissionError::ConfigurationChanged);
        }
        Ok(())
    }

    pub fn capture(root: &JournalRoot) -> Result<Self, AdmissionError> {
        root.revalidate_canonical_binding()
            .map_err(|_| AdmissionError::JournalIdentityChanged)?;
        let raw = Self::read_object(root, Path::new("config/journal.json"))?;
        let offload = raw.get("backup").and_then(|backup| backup.get("offload"));
        let offload_field = |key| offload.and_then(|offload| offload.get(key)).cloned();
        let offload_floor = offload_field("floor_bytes");
        let offload_enabled = offload_field("enabled");
        let offload_budget = offload_field("budget_bytes");
        let config = crate::merge_backup_config(&raw);
        let binding = if config.get("mode") == Some(&Value::String("operated".into())) {
            let raw = Self::read_object(root, Path::new("backup/hosted/binding.json"))?;
            let nonblank = |key| {
                raw.get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_owned)
                    .ok_or(AdmissionError::ConfigurationUnavailable)
            };
            Some(HostedBinding {
                broker_endpoint: nonblank("broker_endpoint")?,
                account_id: nonblank("account_id")?,
                instance_id: nonblank("instance_id")?,
                bucket: nonblank("bucket")?,
                prefix: nonblank("prefix")?,
                broker_token: nonblank("broker_token")?,
            })
        } else {
            None
        };
        root.revalidate_canonical_binding()
            .map_err(|_| AdmissionError::JournalIdentityChanged)?;
        // Status, schedules and accounting are not transport identity.
        let settings = [
            "enabled",
            "mode",
            "destination",
            "daily_key",
            "recovery_key",
        ]
        .into_iter()
        .filter_map(|key| {
            config
                .get(key)
                .cloned()
                .map(|value| (key.to_owned(), value))
        })
        .collect();
        Ok(Self {
            settings,
            binding,
            offload_floor,
            offload_enabled,
            offload_budget,
        })
    }

    fn read_object(
        root: &JournalRoot,
        relative: &Path,
    ) -> Result<Map<String, Value>, AdmissionError> {
        let observed = solstone_core_journal_io::read_relative_file_bounded(
            root,
            relative,
            MAX_ARCHIVE_CONFIGURATION_BYTES,
        )
        .map_err(|_| AdmissionError::ConfigurationUnavailable)?
        .ok_or(AdmissionError::ConfigurationUnavailable)?;
        serde_json::from_slice(&observed.bytes)
            .map_err(|_| AdmissionError::ConfigurationUnavailable)
    }
}

pub struct ArchiveBudget {
    deadline: i64,
    last_now: std::cell::Cell<Option<i64>>,
    credential_expiry: Option<i64>,
    cancellation: Option<ArchiveCancellation>,
}

impl ArchiveBudget {
    /// A later cap can shorten credentials, never extend their lifetime.
    pub fn cap_credential_expiry(&mut self, expiry: i64) {
        self.credential_expiry = Some(self.credential_expiry.map_or(expiry, |old| old.min(expiry)));
    }

    pub fn new(clock: &dyn Clock) -> Result<Self, AdmissionError> {
        Self::start(clock, None)
    }

    pub fn new_cancellable(
        clock: &dyn Clock,
        cancellation: ArchiveCancellation,
    ) -> Result<Self, AdmissionError> {
        Self::start(clock, Some(cancellation))
    }

    pub fn cancellation(&self) -> Option<&ArchiveCancellation> {
        self.cancellation.as_ref()
    }

    #[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
    pub(crate) fn deadline(&self) -> i64 {
        self.deadline
    }

    fn start(
        clock: &dyn Clock,
        cancellation: Option<ArchiveCancellation>,
    ) -> Result<Self, AdmissionError> {
        let now = clock.now_unix();
        let deadline = now
            .checked_add(ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64)
            .ok_or(AdmissionError::DeadlineExceeded)?;
        Ok(Self {
            deadline,
            last_now: std::cell::Cell::new(Some(now)),
            credential_expiry: None,
            cancellation,
        })
    }

    pub fn remaining(&self, clock: &dyn Clock) -> Result<Duration, AdmissionError> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(ArchiveCancellation::is_cancelled)
        {
            return Err(AdmissionError::Cancelled);
        }
        let now = clock.now_unix();
        let last_now = self.last_now.get().ok_or(AdmissionError::ClockChanged)?;
        if now < last_now {
            self.last_now.set(None);
            return Err(AdmissionError::ClockChanged);
        }
        self.last_now.set(Some(now));
        let deadline = self
            .deadline
            .checked_sub(now)
            .filter(|remaining| *remaining > 0)
            .ok_or(AdmissionError::DeadlineExceeded)?;
        let remaining = match self.credential_expiry {
            Some(expiry) => deadline.min(
                expiry
                    .checked_sub(now)
                    .filter(|remaining| *remaining > 0)
                    .ok_or(AdmissionError::CredentialsExpired)?,
            ),
            None => deadline,
        };
        Ok(Duration::from_secs(remaining as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    struct TestClock(Cell<i64>);
    impl Clock for TestClock {
        fn now_unix(&self) -> i64 {
            self.0.get()
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    #[test]
    fn offload_space_requires_positive_floor_and_checked_sum() {
        let configuration = |floor| ArchiveConfiguration {
            settings: Map::new(),
            binding: None,
            offload_floor: floor,
            offload_enabled: None,
            offload_budget: None,
        };
        for floor in [
            None,
            Some(Value::Null),
            Some(serde_json::json!(0)),
            Some(serde_json::json!(-1)),
            Some(serde_json::json!(1.5)),
            Some(serde_json::json!(true)),
            Some(serde_json::json!("7")),
            Some(serde_json::json!([])),
            Some(serde_json::json!({})),
        ] {
            assert_eq!(
                configuration(floor).offload_required_bytes(10, 3),
                Err(AdmissionError::ConfigurationUnavailable)
            );
        }
        let positive = configuration(Some(serde_json::json!(7)));
        assert_eq!(positive.offload_required_bytes(10, 3), Ok(20));
        assert_eq!(
            positive.offload_required_bytes(u64::MAX - 7, 0),
            Ok(u64::MAX)
        );
        assert_eq!(
            positive.offload_required_bytes(u64::MAX - 6, 0),
            Err(AdmissionError::ConfigurationUnavailable)
        );
        assert_eq!(
            positive.offload_required_bytes(u64::MAX - 7, 1),
            Err(AdmissionError::ConfigurationUnavailable)
        );
    }

    #[test]
    fn offload_planning_getters_preserve_unset_and_refuse_malformed_budget() {
        let configuration = |enabled, budget| ArchiveConfiguration {
            settings: Map::new(),
            binding: None,
            offload_floor: None,
            offload_enabled: enabled,
            offload_budget: budget,
        };
        for (enabled, expected) in [
            (None, false),
            (Some(Value::Null), false),
            (Some(serde_json::json!(false)), false),
            (Some(serde_json::json!(true)), true),
            (Some(serde_json::json!(1)), false),
            (Some(serde_json::json!("true")), false),
            (Some(serde_json::json!([])), false),
            (Some(serde_json::json!({})), false),
        ] {
            assert_eq!(configuration(enabled, None).offload_enabled(), expected);
        }
        for (budget, expected) in [
            (None, Ok(None)),
            (Some(Value::Null), Ok(None)),
            (Some(serde_json::json!(0)), Ok(Some(0))),
            (Some(serde_json::json!(7)), Ok(Some(7))),
            (Some(serde_json::json!(u64::MAX)), Ok(Some(u64::MAX))),
            (
                Some(serde_json::json!(-1)),
                Err(AdmissionError::ConfigurationUnavailable),
            ),
            (
                Some(serde_json::json!(1.5)),
                Err(AdmissionError::ConfigurationUnavailable),
            ),
            (
                Some(serde_json::json!(true)),
                Err(AdmissionError::ConfigurationUnavailable),
            ),
            (
                Some(serde_json::json!("7")),
                Err(AdmissionError::ConfigurationUnavailable),
            ),
            (
                Some(serde_json::json!([])),
                Err(AdmissionError::ConfigurationUnavailable),
            ),
            (
                Some(serde_json::json!({})),
                Err(AdmissionError::ConfigurationUnavailable),
            ),
        ] {
            assert_eq!(configuration(None, budget).offload_budget_bytes(), expected);
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn offload_floor_capture_preserves_ordinary_admission_and_refuses_drift() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("config")).unwrap();
        let path = directory.path().join("config/journal.json");
        let write = |config: &Value| {
            std::fs::write(&path, serde_json::to_vec(config).unwrap()).unwrap();
        };
        write(&serde_json::json!({}));
        let root = JournalRoot::open(directory.path()).unwrap();
        for config in [
            serde_json::json!({}),
            serde_json::json!({"backup":{"offload":{"floor_bytes":null}}}),
            serde_json::json!({"backup":{"offload":{"floor_bytes":"7"}}}),
            serde_json::json!({"backup":{"offload":{"floor_bytes":0}}}),
            serde_json::json!({"backup":{"offload":{"enabled":"true","budget_bytes":"7"}}}),
        ] {
            write(&config);
            let captured = ArchiveConfiguration::capture(&root).unwrap();
            captured.revalidate(&root).unwrap();
            assert_eq!(
                captured.offload_required_bytes(10, 3),
                Err(AdmissionError::ConfigurationUnavailable)
            );
            if config["backup"]["offload"]["budget_bytes"].is_string() {
                assert_eq!(
                    captured.offload_budget_bytes(),
                    Err(AdmissionError::ConfigurationUnavailable)
                );
            }
        }
        let mut config = serde_json::json!({"backup":{"offload":{
            "floor_bytes":7,"enabled":true,"budget_bytes":100
        }}});
        write(&config);
        let captured = ArchiveConfiguration::capture(&root).unwrap();
        assert_eq!(captured.offload_required_bytes(10, 3), Ok(20));
        assert!(captured.offload_enabled());
        assert_eq!(captured.offload_budget_bytes(), Ok(Some(100)));
        config["backup"]["last_backup"] = serde_json::json!({"status":"ok"});
        write(&config);
        captured.revalidate(&root).unwrap();
        for (key, replacement) in [
            ("enabled", serde_json::json!(false)),
            ("budget_bytes", serde_json::json!(101)),
        ] {
            let mut changed = config.clone();
            changed["backup"]["offload"][key] = replacement;
            write(&changed);
            assert_eq!(
                captured.revalidate(&root),
                Err(AdmissionError::ConfigurationChanged)
            );
        }
        config["backup"]["offload"]["floor_bytes"] = serde_json::json!(8);
        write(&config);
        assert_eq!(
            captured.revalidate(&root),
            Err(AdmissionError::ConfigurationChanged)
        );
        for (original_offload, changed_offload) in [
            (serde_json::json!({}), serde_json::json!({"enabled":null})),
            (serde_json::json!({}), serde_json::json!({"enabled":false})),
            (
                serde_json::json!({}),
                serde_json::json!({"budget_bytes":null}),
            ),
            (
                serde_json::json!({"budget_bytes":"7"}),
                serde_json::json!({"budget_bytes":false}),
            ),
        ] {
            write(&serde_json::json!({"backup":{"offload":original_offload}}));
            let captured = ArchiveConfiguration::capture(&root).unwrap();
            captured.revalidate(&root).unwrap();
            write(&serde_json::json!({"backup":{"offload":changed_offload}}));
            let changed = ArchiveConfiguration::capture(&root).unwrap();
            changed.revalidate(&root).unwrap();
            assert_eq!(captured.offload_enabled(), changed.offload_enabled());
            assert_eq!(
                captured.offload_budget_bytes(),
                changed.offload_budget_bytes()
            );
            assert_eq!(
                captured.revalidate(&root),
                Err(AdmissionError::ConfigurationChanged)
            );
        }
        config["backup"]["offload"]["floor_bytes"] = Value::Null;
        write(&config);
        assert_eq!(
            captured.revalidate(&root),
            Err(AdmissionError::ConfigurationChanged)
        );
    }

    #[test]
    fn cancellation_and_original_lifetime_survive_worker_budget_transfer() {
        fn worker_entry(budget: ArchiveBudget, now: i64) -> ArchiveBudget {
            let local_clock = TestClock(Cell::new(now));
            assert_eq!(
                budget.remaining(&local_clock).unwrap(),
                Duration::from_secs(1)
            );
            budget
        }
        let admission_clock = TestClock(Cell::new(100));
        let cancellation = ArchiveCancellation::new();
        let mut budget =
            ArchiveBudget::new_cancellable(&admission_clock, cancellation.clone()).unwrap();
        budget.cap_credential_expiry(100 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64);
        budget.cap_credential_expiry(100 + 2 * ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64);
        let near_deadline = 100 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64 - 1;
        let transferred = worker_entry(budget, near_deadline);
        let worker_clock = TestClock(Cell::new(near_deadline + 1));
        assert_eq!(
            transferred.remaining(&worker_clock),
            Err(AdmissionError::DeadlineExceeded)
        );
        cancellation.cancel();
        assert!(transferred.cancellation().unwrap().is_cancelled());
        assert!(!ArchiveCancellation::new().is_cancelled());
        assert_eq!(
            transferred.remaining(&worker_clock),
            Err(AdmissionError::Cancelled)
        );
        let moved_again = transferred;
        assert_eq!(
            moved_again.remaining(&worker_clock),
            Err(AdmissionError::Cancelled)
        );
    }
    #[test]
    fn moved_budget_keeps_deadline_and_sticky_clock_refusal() {
        let clock = TestClock(Cell::new(100));
        let original = ArchiveBudget::new(&clock).unwrap();
        clock.0.set(100 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64 - 1);
        assert_eq!(original.remaining(&clock).unwrap(), Duration::from_secs(1));
        let transferred = original;
        clock.0.set(100 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64);
        assert_eq!(
            transferred.remaining(&clock),
            Err(AdmissionError::DeadlineExceeded)
        );
        let poisoned = ArchiveBudget::new(&clock).unwrap();
        clock.0.set(clock.0.get() - 1);
        assert_eq!(
            poisoned.remaining(&clock),
            Err(AdmissionError::ClockChanged)
        );
        let transferred = poisoned;
        clock.0.set(clock.0.get() + 2);
        assert_eq!(
            transferred.remaining(&clock),
            Err(AdmissionError::ClockChanged)
        );
    }
    #[test]
    fn expiry_cap_cannot_renew_a_session() {
        let clock = TestClock(Cell::new(10));
        let mut budget = ArchiveBudget::new(&clock).unwrap();
        budget.cap_credential_expiry(20);
        budget.cap_credential_expiry(30);
        assert_eq!(budget.remaining(&clock).unwrap(), Duration::from_secs(10));
        clock.0.set(20);
        assert_eq!(
            budget.remaining(&clock),
            Err(AdmissionError::CredentialsExpired)
        );
    }

    #[test]
    fn captured_transport_ignores_status_but_refuses_configuration_change() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("config")).unwrap();
        let path = directory.path().join("config/journal.json");
        let original = serde_json::json!({"backup": {
            "enabled": true, "mode": "byo", "daily_key": "key", "recovery_key": "0".repeat(64),
            "destination": {"backend":"s3", "repository":"s3:https://example.invalid/bucket", "credentials": {"access_key_id":"access", "secret_access_key":"secret"}}
        }});
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        let root = JournalRoot::open(directory.path()).unwrap();
        let captured = ArchiveConfiguration::capture(&root).unwrap();
        let mut changed = original;
        changed["backup"]["last_backup"] = serde_json::json!({"status":"ok"});
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        captured.revalidate(&root).unwrap();
        changed["backup"]["enabled"] = Value::Bool(false);
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_eq!(
            captured.revalidate(&root),
            Err(AdmissionError::ConfigurationChanged)
        );
        std::fs::write(&path, vec![b' '; MAX_ARCHIVE_CONFIGURATION_BYTES + 1]).unwrap();
        assert_eq!(
            captured.revalidate(&root),
            Err(AdmissionError::ConfigurationUnavailable)
        );
    }

    #[cfg(unix)]
    #[test]
    fn bound_configuration_refuses_symlink_and_replaced_root() {
        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().join("journal");
        std::fs::create_dir_all(journal.join("config")).unwrap();
        let outside = directory.path().join("outside.json");
        std::fs::write(&outside, b"{}").unwrap();
        std::os::unix::fs::symlink(&outside, journal.join("config/journal.json")).unwrap();
        let root = JournalRoot::open(&journal).unwrap();
        assert!(matches!(
            ArchiveConfiguration::capture(&root),
            Err(AdmissionError::ConfigurationUnavailable)
        ));
        std::fs::remove_file(journal.join("config/journal.json")).unwrap();
        std::fs::write(journal.join("config/journal.json"), b"{}").unwrap();
        let captured = ArchiveConfiguration::capture(&root).unwrap();
        std::fs::rename(&journal, directory.path().join("retained")).unwrap();
        std::fs::create_dir(&journal).unwrap();
        assert_eq!(
            captured.revalidate(&root),
            Err(AdmissionError::JournalIdentityChanged)
        );
    }
}
