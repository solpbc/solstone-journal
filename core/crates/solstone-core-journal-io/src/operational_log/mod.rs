// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exclusive operational-log create and canonical `oplog--` filename grammar.

mod admission;
mod create;
mod lock;
mod name;
mod namespace;
mod writer;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use chrono::{DateTime, FixedOffset, Local};

pub use crate::lease::LeaseProbe;
pub use admission::{
    OPLOG_ADMISSION_MAX_BYTES, OplogAdmissionError, OplogAdmissionRecord, validate_oplog_admission,
    validate_oplog_admission_set,
};
pub use create::{
    OPLOG_CREATE_ATTEMPTS, OPLOG_FILE_ID_DRAW_BUDGET, OplogCreateError, OplogCreatePrimitive,
    create_oplog, probe_oplog_lease,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use create::{
    create_oplog_with_test_timing, run_with_oplog_create_barrier, run_with_oplog_create_fault,
    run_with_oplog_file_ids, run_with_oplog_probe_indeterminate, run_with_oplog_rollback_fail,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use lock::acquire_oplog_namespace_lock_with_test_timing;
pub use lock::{OplogNamespaceLock, OplogNamespaceLockError, acquire_oplog_namespace_lock};
pub use name::{
    OplogFormat, OplogIdentity, OplogName, OplogNameClassification, OplogNameError,
    classify_oplog_name, derive_day_key_and_opened_field, format_oplog_name,
};
pub use namespace::{OplogDayHealth, OplogNamespaceError, admit_day_health_directory};
pub use writer::{OplogStdioHandle, OplogWriter, OplogWriterError};

/// Sample one local instant for day-key and UTC-field derivation.
pub fn sample_local_instant() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}
