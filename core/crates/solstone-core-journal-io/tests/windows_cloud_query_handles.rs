// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use solstone_core_journal_io::{
    JournalRoot, WindowsAcquisitionPrimitive, run_with_windows_acquisition_trace,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

const CHECKS: u32 = 2_000;
// Far below one handle per check, and above what this process does on its own.
const ALLOWED_GROWTH: u32 = 64;

fn process_handle_count() -> u32 {
    let mut count = 0;
    // SAFETY: the pseudo-handle needs no closing and `count` is writable.
    #[allow(unsafe_code)]
    let result = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
    assert_ne!(result, 0, "read process handle count");
    count
}

#[test]
fn checking_an_ordinary_root_does_not_grow_process_handles() {
    let temporary = tempfile::tempdir().expect("temporary journal root");
    let root = JournalRoot::open(temporary.path()).expect("admit ordinary root");
    let (result, trace) = run_with_windows_acquisition_trace(|| root.revalidate());
    result.expect("first revalidation");
    assert!(
        trace
            .successful
            .contains(&WindowsAcquisitionPrimitive::CloudSyncRootInfoByHandle),
        "this control needs an NTFS fixture, where the Cloud Files query runs"
    );

    let before = process_handle_count();
    for _ in 0..CHECKS {
        root.revalidate().expect("retained root revalidates");
        drop(JournalRoot::open(temporary.path()).expect("root admits again"));
    }
    let after = process_handle_count();

    assert!(
        after <= before + ALLOWED_GROWTH,
        "{CHECKS} admissions and revalidations grew handles from {before} to {after}"
    );
}
