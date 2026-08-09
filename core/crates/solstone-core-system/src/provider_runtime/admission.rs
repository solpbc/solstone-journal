// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Parakeet STT admission latch.
//!
//! A faithful port of `supervisor.py`'s `_parakeet_stt_admission_latch`: the
//! gate that decides whether the Parakeet provider is even wanted before any
//! install/launch machinery runs, memoized so a truth-observation tick that
//! sees no relevant change does not re-run `resolve_stt_backend_choice` or
//! re-read host RAM every cycle.
//!
//! Host facts (platform, machine, RAM headroom, configured backend) are
//! caller-supplied parameters, never read here -- this module stays a pure
//! decision the same way [`crate::stt_backend_choice`] does. The one piece of
//! process state Python threads through as a module global,
//! `_parakeet_admission_retry_epoch`, is likewise a caller-supplied input to
//! [`parakeet_stt_admission_latch`] rather than an ambient read: the process-wide
//! counter itself lives in [`admission_retry_epoch`]/[`bump_admission_retry_epoch`]
//! below, for the retry-token consumption call site (wired in a later change)
//! to read and bump. Keeping the counter out of the decision function's own
//! body is what makes the memoization logic testable without cross-test
//! interference from a shared process-wide static.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use solstone_core_brain::{CanonicalInput, canonical_json, fingerprint_sha256};

use super::model::ProviderName;
use super::seams::RuntimeStoreError;
use super::store::read_current_detail;
use crate::stt_backend_choice::{STT_SURFACE, resolve_stt_backend_choice};
use std::path::Path;

static PARAKEET_ADMISSION_RETRY_EPOCH: AtomicU64 = AtomicU64::new(0);

/// The current process-wide Parakeet admission retry epoch.
pub fn admission_retry_epoch() -> u64 {
    PARAKEET_ADMISSION_RETRY_EPOCH.load(Ordering::SeqCst)
}

/// Advances the process-wide Parakeet admission retry epoch by one,
/// invalidating any latch memoized against the prior epoch. Mirrors Python's
/// `_parakeet_admission_retry_epoch += 1`, called only on Parakeet retry-token
/// consumption -- the counter is process-wide (one instance, not namespaced
/// per provider), not per-call.
pub fn bump_admission_retry_epoch() {
    PARAKEET_ADMISSION_RETRY_EPOCH.fetch_add(1, Ordering::SeqCst);
}

/// Host and config facts the admission decision reads. Mirrors
/// `_parakeet_stt_admission_input`'s returned dict field-for-field; the
/// caller is responsible for gathering these (real platform/machine/RAM
/// reads do not belong inside a pure decision function).
#[derive(Debug, Clone, PartialEq)]
pub struct ParakeetAdmissionInput {
    pub platform: String,
    pub machine: String,
    pub backend: Option<String>,
    pub local_backend: Option<String>,
    pub floor_bytes: Option<u64>,
    pub confidential_lane_active: bool,
    pub confidential_audio_enabled: bool,
}

impl ParakeetAdmissionInput {
    fn canonical(&self) -> CanonicalInput {
        CanonicalInput::Object(vec![
            (
                "platform".to_owned(),
                CanonicalInput::Json(Value::String(self.platform.clone())),
            ),
            (
                "machine".to_owned(),
                CanonicalInput::Json(Value::String(self.machine.clone())),
            ),
            (
                "backend".to_owned(),
                CanonicalInput::Json(optional_string(&self.backend)),
            ),
            (
                "local_backend".to_owned(),
                CanonicalInput::Json(optional_string(&self.local_backend)),
            ),
            (
                "floor_bytes".to_owned(),
                CanonicalInput::Json(optional_u64(self.floor_bytes)),
            ),
            (
                "confidential_lane_active".to_owned(),
                CanonicalInput::Json(Value::Bool(self.confidential_lane_active)),
            ),
            (
                "confidential_audio_enabled".to_owned(),
                CanonicalInput::Json(Value::Bool(self.confidential_audio_enabled)),
            ),
        ])
    }
}

fn optional_string(value: &Option<String>) -> Value {
    match value {
        Some(text) => Value::String(text.clone()),
        None => Value::Null,
    }
}

fn optional_u64(value: Option<u64>) -> Value {
    match value {
        Some(number) => Value::Number(number.into()),
        None => Value::Null,
    }
}

/// The memoized admission verdict. Mirrors the dict `_parakeet_stt_admission_latch`
/// returns and persists into the runtime record's `detail.stt_admission_latch`.
#[derive(Debug, Clone, PartialEq)]
pub struct ParakeetAdmissionLatch {
    pub input_json: String,
    pub input_sha256: String,
    pub retry_epoch: u64,
    pub choice: String,
    pub desired: bool,
    pub blocked: bool,
    pub reason_code: &'static str,
}

impl ParakeetAdmissionLatch {
    pub fn to_json(&self) -> Value {
        json!({
            "input_json": self.input_json,
            "input_sha256": self.input_sha256,
            "retry_epoch": self.retry_epoch,
            "choice": self.choice,
            "desired": self.desired,
            "blocked": self.blocked,
            "reason_code": self.reason_code,
        })
    }

    fn matches_memo(&self, candidate_sha: &str, retry_epoch: u64) -> bool {
        self.input_sha256 == candidate_sha && self.retry_epoch == retry_epoch
    }

    fn from_persisted(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self {
            input_json: object.get("input_json")?.as_str()?.to_owned(),
            input_sha256: object.get("input_sha256")?.as_str()?.to_owned(),
            retry_epoch: object.get("retry_epoch")?.as_u64()?,
            choice: object.get("choice")?.as_str()?.to_owned(),
            desired: object.get("desired")?.as_bool()?,
            blocked: object.get("blocked")?.as_bool()?,
            reason_code: reason_code_str(object.get("reason_code")?.as_str()?)?,
        })
    }
}

fn reason_code_str(value: &str) -> Option<&'static str> {
    match value {
        "host-admission-blocked" => Some("host-admission-blocked"),
        "confidential-backend-selected" => Some("confidential-backend-selected"),
        "provider-not-needed" => Some("provider-not-needed"),
        _ => None,
    }
}

/// A caller-supplied read of available host RAM. `available_bytes()` is
/// skipped entirely when the choice does not need it (matching Python,
/// which never calls `read_available_bytes()` on those paths). Any
/// `Fn() -> Option<u64>` closure implements this, so a caller with the
/// value already in hand can pass e.g. `&|| Some(bytes)`.
pub trait AvailableBytesReader {
    fn available_bytes(&self) -> Option<u64>;
}

impl<F: Fn() -> Option<u64>> AvailableBytesReader for F {
    fn available_bytes(&self) -> Option<u64> {
        self()
    }
}

/// Computes (memoized) whether Parakeet is desired and/or admission-blocked.
///
/// Fails closed: a malformed or unreadable durable record propagates
/// [`RuntimeStoreError`] rather than falling back to a fresh recompute --
/// mirrors Python re-raising `RuntimeHealthMalformedError`/
/// `RuntimeHealthUnavailableError` instead of swallowing them. A genuinely
/// absent record is not an error (see [`read_current_detail`]) and simply
/// yields no memo to match, so the latch recomputes.
pub fn parakeet_stt_admission_latch(
    journal_path: &Path,
    input: &ParakeetAdmissionInput,
    available_bytes: &dyn AvailableBytesReader,
) -> Result<ParakeetAdmissionLatch, RuntimeStoreError> {
    let input_json = canonical_json(&input.canonical()).map_err(|_| RuntimeStoreError::Corrupt)?;
    let input_sha256 = fingerprint_sha256(&input_json);
    let retry_epoch = admission_retry_epoch();

    let detail = read_current_detail(journal_path, ProviderName::Parakeet)?;
    if let Some(existing) = detail
        .get("stt_admission_latch")
        .and_then(ParakeetAdmissionLatch::from_persisted)
        && existing.matches_memo(&input_sha256, retry_epoch)
    {
        return Ok(existing);
    }

    let skip_available_bytes = matches!(
        input.backend.as_deref(),
        Some("parakeet") | Some("parakeet-cpp") | Some("confidential")
    ) || input.confidential_lane_active;
    let available = if skip_available_bytes {
        None
    } else {
        available_bytes.available_bytes()
    };

    let choice = resolve_stt_backend_choice(
        input.backend.as_deref(),
        available,
        input.floor_bytes,
        input.local_backend.as_deref(),
        input.confidential_lane_active,
        input.confidential_audio_enabled,
    );
    let desired = choice == "parakeet" || choice == "parakeet-cpp";
    let blocked = choice == STT_SURFACE
        && input.backend.is_none()
        && !input.confidential_lane_active
        && matches!(
            input.local_backend.as_deref(),
            Some("parakeet") | Some("parakeet-cpp")
        )
        && input.floor_bytes.is_some();
    let reason_code = if blocked {
        "host-admission-blocked"
    } else if choice == "confidential" {
        "confidential-backend-selected"
    } else {
        "provider-not-needed"
    };

    Ok(ParakeetAdmissionLatch {
        input_json,
        input_sha256,
        retry_epoch,
        choice,
        desired,
        blocked,
        reason_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering as TestOrdering;
    use std::sync::{Mutex, OnceLock};

    // The retry epoch is a process-wide static; serialize tests that touch it
    // so parallel `cargo test` runs cannot see each other's bumps.
    fn epoch_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempJournal(PathBuf);

    impl TempJournal {
        // Deliberately does not pre-create the directory: `read_current_detail`
        // treats a wholly nonexistent journal path the same as a missing
        // record file (NotFound -> synthetic empty detail), and
        // `FileRuntimeStore::publish_state` creates its own directory tree on
        // first write. A path that is merely unique and unclaimed is enough.
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-admission-latch-test-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, TestOrdering::Relaxed)
            ));
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn baseline_input() -> ParakeetAdmissionInput {
        ParakeetAdmissionInput {
            platform: "linux".to_owned(),
            machine: "x86_64".to_owned(),
            backend: None,
            local_backend: Some("parakeet".to_owned()),
            floor_bytes: Some(4_000_000_000),
            confidential_lane_active: false,
            confidential_audio_enabled: false,
        }
    }

    fn no_bytes() -> Option<u64> {
        None
    }

    fn write_detail(journal: &Path, provider: ProviderName, detail: Value) {
        use super::super::model::ProviderRuntimeState;
        use super::super::seams::RuntimeStore;
        use super::super::store::{FileRuntimeStore, LocalRuntimeShared, SystemRuntimeClock};
        use std::sync::Arc;

        let shared = Arc::new(LocalRuntimeShared::default());
        let clock = Arc::new(SystemRuntimeClock::default());
        let mut store = FileRuntimeStore::new(journal.to_path_buf(), provider, shared, clock);
        let mut state = ProviderRuntimeState::new(provider);
        state.latest_detail = Some(detail);
        store.publish_state(&state).expect("publish seeded detail");
    }

    #[test]
    fn a_fresh_journal_recomputes_and_finds_ample_ram_undesired() {
        let journal = TempJournal::new();
        let input = ParakeetAdmissionInput {
            backend: None,
            local_backend: Some("surface".to_owned()),
            floor_bytes: Some(1),
            ..baseline_input()
        };
        let latch = parakeet_stt_admission_latch(journal.path(), &input, &no_bytes)
            .expect("fresh journal is not an error");
        assert!(!latch.desired);
        assert!(!latch.blocked);
        assert_eq!(latch.reason_code, "provider-not-needed");
    }

    #[test]
    fn explicit_parakeet_backend_is_desired_and_never_reads_ram() {
        let journal = TempJournal::new();
        let input = ParakeetAdmissionInput {
            backend: Some("parakeet".to_owned()),
            ..baseline_input()
        };
        let panics_if_called: fn() -> Option<u64> = || panic!("must not read RAM");
        let latch = parakeet_stt_admission_latch(journal.path(), &input, &panics_if_called)
            .expect("no store error");
        assert!(latch.desired);
        assert!(!latch.blocked);
    }

    #[test]
    fn insufficient_ram_with_implicit_parakeet_backend_blocks() {
        let journal = TempJournal::new();
        let input = baseline_input();
        let scarce = || Some(1_u64);
        let latch =
            parakeet_stt_admission_latch(journal.path(), &input, &scarce).expect("no store error");
        assert_eq!(latch.choice, STT_SURFACE);
        assert!(!latch.desired);
        assert!(latch.blocked);
        assert_eq!(latch.reason_code, "host-admission-blocked");
    }

    #[test]
    fn a_matching_memo_is_returned_without_recomputing_choice() {
        let journal = TempJournal::new();
        let input = ParakeetAdmissionInput {
            backend: Some("parakeet".to_owned()),
            ..baseline_input()
        };
        let panics_if_called: fn() -> Option<u64> = || panic!("must not read RAM");
        let first = parakeet_stt_admission_latch(journal.path(), &input, &panics_if_called)
            .expect("first computes fresh");
        write_detail(
            journal.path(),
            ProviderName::Parakeet,
            json!({"stt_admission_latch": first.to_json()}),
        );
        // A second read with a RAM reader that panics if invoked proves the
        // memo short-circuited before any branch that would consult it.
        let second = parakeet_stt_admission_latch(journal.path(), &input, &panics_if_called)
            .expect("memoized read");
        assert_eq!(second, first);
    }

    #[test]
    fn a_changed_input_sha_invalidates_the_memo() {
        let journal = TempJournal::new();
        let input = ParakeetAdmissionInput {
            backend: Some("parakeet".to_owned()),
            ..baseline_input()
        };
        let first = parakeet_stt_admission_latch(journal.path(), &input, &no_bytes)
            .expect("first computes fresh");
        write_detail(
            journal.path(),
            ProviderName::Parakeet,
            json!({"stt_admission_latch": first.to_json()}),
        );
        let changed = ParakeetAdmissionInput {
            floor_bytes: Some(999),
            ..input
        };
        let second = parakeet_stt_admission_latch(journal.path(), &changed, &no_bytes)
            .expect("second computes fresh");
        assert_ne!(second.input_sha256, first.input_sha256);
    }

    #[test]
    fn a_bumped_retry_epoch_invalidates_the_memo_even_with_identical_input() {
        let _guard = epoch_test_lock().lock().expect("epoch lock");
        let before = PARAKEET_ADMISSION_RETRY_EPOCH.load(TestOrdering::SeqCst);
        let journal = TempJournal::new();
        let input = ParakeetAdmissionInput {
            backend: Some("parakeet".to_owned()),
            ..baseline_input()
        };
        let first = parakeet_stt_admission_latch(journal.path(), &input, &no_bytes)
            .expect("first computes fresh");
        write_detail(
            journal.path(),
            ProviderName::Parakeet,
            json!({"stt_admission_latch": first.to_json()}),
        );
        bump_admission_retry_epoch();
        let second = parakeet_stt_admission_latch(journal.path(), &input, &no_bytes)
            .expect("second computes fresh");
        assert_ne!(second.retry_epoch, first.retry_epoch);
        PARAKEET_ADMISSION_RETRY_EPOCH.store(before, TestOrdering::SeqCst);
    }

    #[test]
    fn a_memo_missing_a_well_typed_desired_flag_is_not_reused() {
        let journal = TempJournal::new();
        let input = ParakeetAdmissionInput {
            backend: Some("parakeet".to_owned()),
            ..baseline_input()
        };
        let input_json =
            canonical_json(&input.canonical()).expect("canonical json for malformed memo");
        let input_sha256 = fingerprint_sha256(&input_json);
        write_detail(
            journal.path(),
            ProviderName::Parakeet,
            json!({"stt_admission_latch": {
                "input_json": input_json,
                "input_sha256": input_sha256,
                "retry_epoch": admission_retry_epoch(),
                "choice": "parakeet",
                "desired": "not-a-bool",
                "blocked": false,
                "reason_code": "provider-not-needed",
            }}),
        );
        let latch = parakeet_stt_admission_latch(journal.path(), &input, &no_bytes)
            .expect("recompute despite malformed memo shape");
        assert!(latch.desired);
    }

    // Fail-closed propagation of a Corrupt/Unavailable durable record is
    // tested at its source in store.rs
    // (`read_current_detail_fails_closed_on_corrupt_and_unavailable_records`);
    // `parakeet_stt_admission_latch` only forwards `read_current_detail`'s
    // `Result` via `?`, so re-proving both error shapes here would duplicate
    // that coverage rather than test anything this module adds.

    #[test]
    fn admission_retry_epoch_bump_is_process_wide_not_per_provider() {
        let _guard = epoch_test_lock().lock().expect("epoch lock");
        let before = admission_retry_epoch();
        bump_admission_retry_epoch();
        assert_eq!(admission_retry_epoch(), before + 1);
        PARAKEET_ADMISSION_RETRY_EPOCH.store(before, TestOrdering::SeqCst);
    }
}
