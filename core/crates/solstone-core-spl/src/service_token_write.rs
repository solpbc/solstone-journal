// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Service-token publication through the caller's original journal capability.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use solstone_core_journal_io::{JournalRoot, media::MediaDirectory};

/// A refusal before the token rename. It does not describe an earlier posture write.
pub struct ServiceTokenWriteRefusal(Box<dyn Error + Send + Sync>);

impl ServiceTokenWriteRefusal {
    pub(crate) fn from_error(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

impl fmt::Debug for ServiceTokenWriteRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ServiceTokenWriteRefusal")
    }
}

impl fmt::Display for ServiceTokenWriteRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("service token publication was refused")
    }
}

impl Error for ServiceTokenWriteRefusal {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.0)
    }
}

/// Actual native publication and independent caller confirmation. Formatting
/// deliberately excludes the native observation, which contains token bytes.
pub struct ServiceTokenPublication {
    #[cfg(unix)]
    native: solstone_core_journal_io::BoundAtomicOutcome,
    #[cfg(windows)]
    native: solstone_core_journal_io::DetailedAtomicOutcome,
    pub confirmation_refusal: Option<String>,
}

impl ServiceTokenPublication {
    pub fn confirmed(&self) -> bool {
        #[cfg(unix)]
        let native_confirmed = matches!(
            self.native,
            solstone_core_journal_io::BoundAtomicOutcome::Published { .. }
        );
        #[cfg(windows)]
        let native_confirmed = matches!(
            self.native,
            solstone_core_journal_io::DetailedAtomicOutcome::Published
        );
        #[cfg(not(any(unix, windows)))]
        let native_confirmed = false;
        native_confirmed && self.confirmation_refusal.is_none()
    }

    #[cfg(unix)]
    pub fn native(&self) -> &solstone_core_journal_io::BoundAtomicOutcome {
        &self.native
    }

    #[cfg(windows)]
    pub fn native(&self) -> &solstone_core_journal_io::DetailedAtomicOutcome {
        &self.native
    }
}

impl fmt::Debug for ServiceTokenPublication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(unix)]
        let native = match &self.native {
            solstone_core_journal_io::BoundAtomicOutcome::Published { .. } => "published",
            solstone_core_journal_io::BoundAtomicOutcome::PublishedDurabilityUncertain {
                ..
            } => "published_durability_uncertain",
            solstone_core_journal_io::BoundAtomicOutcome::PublishedObservationUncertain {
                ..
            } => "published_observation_uncertain",
        };
        #[cfg(windows)]
        let native = match &self.native {
            solstone_core_journal_io::DetailedAtomicOutcome::Published => "published",
            solstone_core_journal_io::DetailedAtomicOutcome::PublishedDurabilityUncertain {
                ..
            } => "published_durability_uncertain",
            solstone_core_journal_io::DetailedAtomicOutcome::PublishedParentPathRaced {
                ..
            } => "published_parent_path_raced",
            solstone_core_journal_io::DetailedAtomicOutcome::PublishedParentPathUnverified {
                ..
            } => "published_parent_path_unverified",
        };
        #[cfg(not(any(unix, windows)))]
        let native = "unsupported";
        formatter
            .debug_struct("ServiceTokenPublication")
            .field("native", &native)
            .field("confirmed", &self.confirmed())
            .field("confirmation_refused", &self.confirmation_refusal.is_some())
            .finish_non_exhaustive()
    }
}

/// Caller holds the existing service-configuration mutex. No pathname is
/// opened for creation/publication and no refusal falls back to a path writer.
pub(crate) fn save_service_token_at_root_unlocked(
    root: &Arc<JournalRoot>,
    token: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<ServiceTokenPublication, ServiceTokenWriteRefusal> {
    #[cfg(any(unix, windows))]
    {
        let check_root = || {
            checkpoint()?;
            root.revalidate_canonical_binding()
                .map_err(|e| e.to_string())
        };
        check_root().map_err(|e| ServiceTokenWriteRefusal::from_error(io::Error::other(e)))?;
        #[cfg(unix)]
        let link = solstone_core_journal_io::create_or_open_flat_directory_bound(
            &**root,
            OsStr::new("link"),
            0o700,
            root.canonical_path(),
        );
        #[cfg(windows)]
        let link = solstone_core_journal_io::create_or_open_windows_flat_directory_bound(
            &**root,
            OsStr::new("link"),
            root.canonical_path(),
        );
        let link = link.map_err(ServiceTokenWriteRefusal::from_error)?;
        check_root().map_err(|e| ServiceTokenWriteRefusal::from_error(io::Error::other(e)))?;
        #[cfg(unix)]
        let parent = solstone_core_journal_io::create_or_open_flat_directory_bound(
            &link,
            OsStr::new("tokens"),
            0o700,
            &root.canonical_path().join("link"),
        );
        #[cfg(windows)]
        let parent = solstone_core_journal_io::create_or_open_windows_flat_directory_bound(
            &link,
            OsStr::new("tokens"),
            &root.canonical_path().join("link"),
        );
        let parent = parent.map_err(ServiceTokenWriteRefusal::from_error)?;
        let directory = MediaDirectory::open(Arc::clone(root), Path::new("link/tokens"))
            .map_err(ServiceTokenWriteRefusal::from_error)?;
        #[cfg(unix)]
        let parent_identity = parent.identity();
        #[cfg(windows)]
        let parent_identity = parent.object_identity();
        if parent_identity != directory.object_identity() {
            return Err(ServiceTokenWriteRefusal::from_error(io::Error::other(
                "retained token parent changed",
            )));
        }
        let check = || {
            check_root()?;
            directory.revalidate().map_err(|e| e.to_string())
        };
        let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({"service_token":token}))
            .map_err(ServiceTokenWriteRefusal::from_error)?;
        bytes.push(b'\n');
        #[cfg(unix)]
        let native = solstone_core_journal_io::atomic_replace_bound_checked(
            &parent,
            OsStr::new("account.json"),
            &bytes,
            0o600,
            &check,
        )
        .map_err(ServiceTokenWriteRefusal::from_error)?;
        #[cfg(windows)]
        let native = {
            let options = solstone_core_journal_io::LockOptions::default();
            let lock = solstone_core_journal_io::acquire_existing_parent_lock_bound_checked(
                &parent,
                OsStr::new("account.json.lock"),
                options.timeout,
                options.poll_interval,
                &check,
            )
            .map_err(ServiceTokenWriteRefusal::from_error)?;
            solstone_core_journal_io::atomic_replace_detailed_bound_checked(
                &parent,
                &lock,
                OsStr::new("account.json"),
                &bytes,
                0o600,
                &check,
            )
            .map_err(ServiceTokenWriteRefusal::from_error)?
        };
        // The rename landed. A changed owner/parent or cancellation now means
        // unconfirmed publication, never a no-write refusal.
        let confirmation_refusal = (|| {
            check()?;
            let observed = directory
                .read_file_bounded(OsStr::new("account.json"), bytes.len())
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "published token is missing".to_owned())?;
            if !observed.complete || observed.bytes != bytes {
                return Err("published token observation changed".to_owned());
            }
            check()
        })()
        .err();
        Ok(ServiceTokenPublication {
            native,
            confirmation_refusal,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (root, token, checkpoint);
        Err(ServiceTokenWriteRefusal::from_error(io::Error::new(
            io::ErrorKind::Unsupported,
            "retained token publication is unsupported",
        )))
    }
}
