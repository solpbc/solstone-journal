// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native macOS process birth observation.

#[cfg(any(target_os = "macos", test))]
use super::{ExecutionState, InspectResult, ProcessBirth, ProcessInstance};

/// The stable subset copied from `proc_bsdinfo`; keeping this independent of
/// libc makes the birth extractor hermetic on non-macOS test hosts.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MacosBsdInfo {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: i32,
    pub status: u32,
    pub start_tvsec: u64,
    pub start_tvusec: u64,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacosProcError {
    // Constructed only by the macOS syscall wrapper; Linux parser tests never
    // exercise that platform-specific path.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Unavailable,
    InvalidBirth,
}

/// Convert Darwin's `(seconds, microseconds)` birth tuple into the opaque
/// identity used everywhere else. Invalid native data fails closed.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn process_birth_from_macos_bsd_info(
    info: MacosBsdInfo,
) -> Result<ProcessBirth, MacosProcError> {
    if info.start_tvusec >= 1_000_000 {
        return Err(MacosProcError::InvalidBirth);
    }
    let seconds = i64::try_from(info.start_tvsec).map_err(|_| MacosProcError::InvalidBirth)?;
    let micros = seconds
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(i64::try_from(info.start_tvusec).ok()?))
        .ok_or(MacosProcError::InvalidBirth)?;
    Ok(ProcessBirth::macos(micros))
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn inspect_from_macos_bsd_info(info: MacosBsdInfo) -> InspectResult {
    let Ok(birth) = process_birth_from_macos_bsd_info(info) else {
        return InspectResult::Unverifiable;
    };
    // Darwin's private proc headers define SZOMB as 5 and SSTOP as 4. Keep
    // those values here as data, not a dependency on macOS-only constants in
    // the Linux-testable extractor.
    if info.status == 5 {
        return InspectResult::Absent;
    }
    let execution = if info.status == 4 {
        ExecutionState::Stopped
    } else {
        ExecutionState::Running
    };
    InspectResult::Present {
        instance: ProcessInstance {
            pid: info.pid,
            birth,
        },
        execution,
        ppid: Some(info.ppid),
        pgid: Some(info.pgid),
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(crate) fn read_bsd_info(pid: u32) -> Result<MacosBsdInfo, MacosProcError> {
    let pid = i32::try_from(pid).map_err(|_| MacosProcError::Unavailable)?;
    // SAFETY: `raw` is a properly sized writable proc_bsdinfo buffer, and the
    // requested flavor is PROC_PIDTBSDINFO. `proc_pidinfo` writes no more than
    // the supplied size; the returned byte count is checked before reading it.
    let mut raw: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let expected = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
        .map_err(|_| MacosProcError::Unavailable)?;
    let wrote = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(raw).cast(),
            expected,
        )
    };
    if wrote != expected {
        return Err(MacosProcError::Unavailable);
    }
    Ok(MacosBsdInfo {
        pid: u32::try_from(raw.pbi_pid).map_err(|_| MacosProcError::Unavailable)?,
        ppid: u32::try_from(raw.pbi_ppid).map_err(|_| MacosProcError::Unavailable)?,
        pgid: i32::try_from(raw.pbi_pgid).map_err(|_| MacosProcError::Unavailable)?,
        status: u32::try_from(raw.pbi_status).map_err(|_| MacosProcError::Unavailable)?,
        start_tvsec: raw.pbi_start_tvsec,
        start_tvusec: raw.pbi_start_tvusec,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(micros: u64) -> MacosBsdInfo {
        MacosBsdInfo {
            pid: 42,
            ppid: 1,
            pgid: 42,
            status: 2,
            start_tvsec: 100,
            start_tvusec: micros,
        }
    }

    #[test]
    fn ac38_macos_birth_identity_retains_microseconds() {
        let first = process_birth_from_macos_bsd_info(info(1)).expect("birth");
        let second = process_birth_from_macos_bsd_info(info(2)).expect("birth");
        assert_ne!(first, second);
        assert_eq!(first.epoch_seconds(), 100.000_001);
    }

    #[test]
    fn ac39_macos_birth_rejects_malformed_or_overflow_values() {
        assert_eq!(
            process_birth_from_macos_bsd_info(info(1_000_000)),
            Err(MacosProcError::InvalidBirth)
        );
        let mut overflow = info(0);
        overflow.start_tvsec = u64::MAX;
        assert_eq!(
            process_birth_from_macos_bsd_info(overflow),
            Err(MacosProcError::InvalidBirth)
        );
        let mut unavailable = info(0);
        unavailable.start_tvusec = 1_000_000;
        assert_eq!(
            inspect_from_macos_bsd_info(unavailable),
            InspectResult::Unverifiable
        );
    }
}
