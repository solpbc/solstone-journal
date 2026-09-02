// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exclusive operational-log create and canonical `oplog--` filename grammar.
//!
//! ```compile_fail,E0432
//! use solstone_core_journal_io::operational_log::OplogCreatePrimitive;
//! #[cfg(feature = "test-hooks")]
//! use solstone_core_journal_io::operational_log::__oplog_create_primitive_is_test_hooks_only;
//! ```
//!
//! ```compile_fail,E0432
//! use solstone_core_journal_io::operational_log::OplogNamespacePrimitive;
//! #[cfg(feature = "test-hooks")]
//! use solstone_core_journal_io::operational_log::__oplog_namespace_primitive_is_test_hooks_only;
//! ```
//!
//! ```compile_fail,E0603
//! let _ = solstone_core_journal_io::operational_log::sample_local_instant;
//! ```

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
};
pub use create::{
    OPLOG_CREATE_ATTEMPTS, OPLOG_FILE_ID_DRAW_BUDGET, OplogCreateError, create_oplog,
    probe_oplog_lease,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use create::{
    OplogCreatePrimitive, create_oplog_with_test_timing, run_with_oplog_create_barrier,
    run_with_oplog_create_fault, run_with_oplog_create_fault_at, run_with_oplog_file_ids,
    run_with_oplog_probe_indeterminate,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use lock::acquire_oplog_namespace_lock_with_test_timing;
pub use lock::{OplogNamespaceLock, OplogNamespaceLockError, acquire_oplog_namespace_lock};
pub use name::{
    OplogFormat, OplogIdentity, OplogName, OplogNameClassification, OplogNameError,
    classify_oplog_name, derive_day_key_and_opened_field, format_oplog_name,
};
pub use namespace::{OplogDayHealth, OplogNamespaceError, admit_day_health_directory};
#[cfg(any(test, feature = "test-hooks"))]
pub use namespace::{
    OplogNamespacePrimitive, run_with_oplog_namespace_barrier, run_with_oplog_namespace_fault,
};
pub use writer::{OplogStdioHandle, OplogWriter, OplogWriterError};

/// Sample one local instant for day-key and UTC-field derivation.
fn sample_local_instant() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}
