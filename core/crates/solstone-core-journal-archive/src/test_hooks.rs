// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use crate::encode::{
    TestFault, TestSourceAction, encode_injected_operation_fired, install_encode_control,
    reset_encode_control,
};
use crate::source::{DescendantBarrier, trace_descendants};

pub use crate::encode::{TestBoundary, TestFaultKind, TestSinkOperation};
pub use crate::source::DescendantPrimitive;
pub use solstone_core_journal_io::{AcquisitionPrimitive, run_with_acquisition_fault};

/// Truncation to install before a source-member read during a controlled encode.
#[doc(hidden)]
pub struct EncodeTruncateBeforeRead {
    pub member: String,
    pub copied: u64,
    pub path: PathBuf,
    pub length: u64,
}

/// Run `op` with a descendant barrier whose callback is supplied by the caller.
/// Returns the op result and whether the barrier fired.
#[doc(hidden)]
pub fn run_with_descendant_barrier<T>(
    primitive: DescendantPrimitive,
    member: Option<&str>,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    let barrier = DescendantBarrier {
        primitive,
        member: member.map(str::to_owned),
        ordinal,
        callback: Box::new(callback),
    };
    let (result, outcome) = trace_descendants(None, Some(barrier), op);
    (result, outcome.barrier_fired)
}

/// Run `op` under an encode sink/source control. Returns the op result and
/// whether the injected sink operation fired.
#[doc(hidden)]
pub fn run_with_encode_control<T>(
    boundary: TestBoundary,
    operation: TestSinkOperation,
    ordinal: usize,
    kind: TestFaultKind,
    truncate: Option<EncodeTruncateBeforeRead>,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    let fault = TestFault {
        boundary,
        operation,
        ordinal,
        kind,
    };
    let action = truncate.map(|truncate| TestSourceAction::TruncateBeforeRead {
        member: truncate.member,
        copied: truncate.copied,
        path: truncate.path,
        length: truncate.length,
    });
    install_encode_control(Some(fault), action, None);
    let result = op();
    let fired = encode_injected_operation_fired();
    reset_encode_control();
    (result, fired)
}
