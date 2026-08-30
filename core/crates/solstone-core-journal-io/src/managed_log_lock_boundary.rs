// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Platform-neutral lock-key policy for managed operational-log aliases.

use std::ffi::OsStr;

use crate::errors::ExistingParentLockError;
use crate::locking::LockOptions;

/// Injected boundary for lock-key tests; it keeps policy separate from OS exclusion.
#[allow(
    dead_code,
    reason = "the Windows managed-log substrate is intentionally inactive"
)]
pub(crate) trait ManagedLogAliasLockBoundary {
    type Guard;

    fn acquire(
        &self,
        lock_name: &OsStr,
        options: LockOptions,
    ) -> Result<Self::Guard, ExistingParentLockError>;
}

#[allow(
    dead_code,
    reason = "the Windows managed-log substrate is intentionally inactive"
)]
pub(crate) fn acquire_with_boundary<B: ManagedLogAliasLockBoundary>(
    boundary: &B,
    lock_name: &OsStr,
    options: LockOptions,
) -> Result<B::Guard, ExistingParentLockError> {
    boundary.acquire(lock_name, options)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::time::Duration;

    use super::*;
    use crate::errors::LockTimeout;
    use crate::managed_log_names::{ManagedLogAliasRole, alias_lock_name};

    #[derive(Default)]
    struct BoundaryDouble {
        held: RefCell<BTreeSet<OsString>>,
        attempts: RefCell<Vec<OsString>>,
    }

    impl ManagedLogAliasLockBoundary for BoundaryDouble {
        type Guard = OsString;

        fn acquire(
            &self,
            lock_name: &OsStr,
            _options: LockOptions,
        ) -> Result<Self::Guard, ExistingParentLockError> {
            self.attempts.borrow_mut().push(lock_name.to_os_string());
            if !self.held.borrow_mut().insert(lock_name.to_os_string()) {
                return Err(ExistingParentLockError::Timeout(LockTimeout {
                    path: lock_name.into(),
                    timeout: Duration::ZERO,
                }));
            }
            Ok(lock_name.to_os_string())
        }
    }

    #[test]
    fn injected_boundary_proves_key_selection_exclusion_and_independent_progress() {
        let boundary = BoundaryDouble::default();
        let options = LockOptions::default();
        let first = alias_lock_name(ManagedLogAliasRole::Root, "alpha");
        let second = alias_lock_name(ManagedLogAliasRole::Root, "beta");
        assert_eq!(
            acquire_with_boundary(&boundary, &first, options).unwrap(),
            first
        );
        assert!(acquire_with_boundary(&boundary, &first, options).is_err());
        assert_eq!(
            acquire_with_boundary(&boundary, &second, options).unwrap(),
            second
        );
        assert_eq!(
            &*boundary.attempts.borrow(),
            &[first.clone(), first, second]
        );
    }
}
