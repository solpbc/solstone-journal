// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;

use nix::errno::Errno;

/// Protocol checkpoints reserved for same-crate owner-bootstrap tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OwnerBootstrapPrimitive {
    CommittedIdentityLoad,
    RootRevalidateBeforeEndpoint,
    EffectiveUid,
    DirectoryNoFollowProbe,
    DirectoryCreate,
    DirectoryOpen,
    DirectoryFchmod,
    DirectoryFstat,
    RootRevalidateAndFsync,
    DirectoryBindingCheckBeforeLock,
    LockNoFollowProbe,
    LockCreate,
    LockOpen,
    LockFchmod,
    LockFstat,
    LockAcquire,
    KeyPrecheckStat,
    KeyOpen,
    KeyFstat,
    KeyRead,
    KeyFinalRestat,
    KeyDecode,
    KeyGenerate,
    KeyPublish,
    FinalKeyOpen,
    FinalKeyRestat,
    FinalKeyContentCompare,
    FinalKeyFsync,
    FinalDirectoryFsync,
    DirectoryBindingCheckBeforeSuccess,
}

struct Fault {
    primitive: OwnerBootstrapPrimitive,
    ordinal: usize,
    error: Errno,
}

struct Barrier {
    primitive: OwnerBootstrapPrimitive,
    ordinal: usize,
    callback: Box<dyn FnOnce()>,
}

struct TraceState {
    attempted: Vec<OwnerBootstrapPrimitive>,
    faults: Vec<Fault>,
    faults_consumed: usize,
    barriers: Vec<Barrier>,
    barriers_fired: usize,
}

thread_local! {
    static TRACE: std::cell::RefCell<Option<TraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Run `op` with one injected errno at an owner-bootstrap checkpoint.
#[allow(dead_code)]
pub(crate) fn run_with_owner_fault<T>(
    primitive: OwnerBootstrapPrimitive,
    ordinal: usize,
    raw_errno: i32,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    install_trace(TraceState {
        attempted: Vec::new(),
        faults: vec![Fault {
            primitive,
            ordinal,
            error: Errno::from_raw(raw_errno),
        }],
        faults_consumed: 0,
        barriers: Vec::new(),
        barriers_fired: 0,
    });
    let result = op();
    let trace = take_trace();
    (result, trace.faults_consumed == 1)
}

/// Run `op` with two injected errnos at owner-bootstrap checkpoints.
#[allow(dead_code)]
pub(crate) fn run_with_two_owner_faults<T>(
    first_primitive: OwnerBootstrapPrimitive,
    first_ordinal: usize,
    first_raw_errno: i32,
    second_primitive: OwnerBootstrapPrimitive,
    second_ordinal: usize,
    second_raw_errno: i32,
    op: impl FnOnce() -> T,
) -> (T, usize) {
    install_trace(TraceState {
        attempted: Vec::new(),
        faults: vec![
            Fault {
                primitive: first_primitive,
                ordinal: first_ordinal,
                error: Errno::from_raw(first_raw_errno),
            },
            Fault {
                primitive: second_primitive,
                ordinal: second_ordinal,
                error: Errno::from_raw(second_raw_errno),
            },
        ],
        faults_consumed: 0,
        barriers: Vec::new(),
        barriers_fired: 0,
    });
    let result = op();
    let trace = take_trace();
    (result, trace.faults_consumed)
}

/// Run `op` with one deterministic owner-bootstrap barrier callback.
#[allow(dead_code)]
pub(crate) fn run_with_owner_barrier<T>(
    primitive: OwnerBootstrapPrimitive,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    install_trace(TraceState {
        attempted: Vec::new(),
        faults: Vec::new(),
        faults_consumed: 0,
        barriers: vec![Barrier {
            primitive,
            ordinal,
            callback: Box::new(callback),
        }],
        barriers_fired: 0,
    });
    let result = op();
    let trace = take_trace();
    (result, trace.barriers_fired == 1)
}

/// Run `op` with two deterministic owner-bootstrap barrier callbacks.
#[allow(dead_code)]
pub(crate) fn run_with_two_owner_barriers<T>(
    first_primitive: OwnerBootstrapPrimitive,
    first_ordinal: usize,
    first_callback: impl FnOnce() + 'static,
    second_primitive: OwnerBootstrapPrimitive,
    second_ordinal: usize,
    second_callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, usize) {
    install_trace(TraceState {
        attempted: Vec::new(),
        faults: Vec::new(),
        faults_consumed: 0,
        barriers: vec![
            Barrier {
                primitive: first_primitive,
                ordinal: first_ordinal,
                callback: Box::new(first_callback),
            },
            Barrier {
                primitive: second_primitive,
                ordinal: second_ordinal,
                callback: Box::new(second_callback),
            },
        ],
        barriers_fired: 0,
    });
    let result = op();
    let trace = take_trace();
    (result, trace.barriers_fired)
}

pub(crate) fn checkpoint(primitive: OwnerBootstrapPrimitive) -> io::Result<()> {
    let (fault, barrier) = TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return (None, None);
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|candidate| **candidate == primitive)
            .count();
        if let Some(index) = state
            .faults
            .iter()
            .position(|fault| fault.primitive == primitive && fault.ordinal == ordinal)
        {
            let fault = state.faults.remove(index);
            state.faults_consumed += 1;
            return (Some(fault.error), None);
        }
        let barrier = state
            .barriers
            .iter()
            .position(|barrier| barrier.primitive == primitive && barrier.ordinal == ordinal)
            .map(|index| {
                state.barriers_fired += 1;
                state.barriers.remove(index).callback
            });
        (None, barrier)
    });
    if let Some(error) = fault {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    if let Some(callback) = barrier {
        callback();
    }
    Ok(())
}

fn install_trace(state: TraceState) {
    TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "owner-bootstrap trace is already active"
        );
        *trace.borrow_mut() = Some(state);
    });
}

fn take_trace() -> TraceState {
    TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("owner-bootstrap trace remains active")
    })
}
