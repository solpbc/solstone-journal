// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

use crate::manifest_binding::BODY_BUNDLE_REF_VALUE;
use crate::{
    BodyManifestBinding, BundleId, ManifestBindingError, ManifestKeySignal, decode_body_manifest,
    inspect_body_manifest_signal,
};

const NATIVE_PREFIX: &[u8] = b"body-";
const STAGING_PREFIX: &[u8] = b".body-staging-";
const LEDGER_SIDECAR_NAME: &str = "body-ledger.jsonl";
const MANIFEST_FILE_NAME: &str = "manifest.json";

/// Raw directory facts used to classify a body-import bundle.
#[derive(Clone, Copy, Debug)]
pub struct DirectoryObservation<'a> {
    pub name: &'a [u8],
    pub envelope_present: bool,
    pub ledger_present: bool,
    pub manifest: Option<&'a [u8]>,
}

/// The closed classification of an observed body-import directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleClass {
    StagingExcluded,
    NativeCandidate,
    LegacyCandidate,
}

/// A checked native body-import authority.
///
/// Its private field prevents direct construction outside this crate:
///
/// ```compile_fail,E0451
/// use solstone_core_body_source::NativeAuthority;
///
/// let _ = NativeAuthority { binding: todo!() };
/// ```
///
/// There is no `From<BodyManifestBinding>` conversion into an authority:
///
/// ```compile_fail,E0277
/// use solstone_core_body_source::{BodyManifestBinding, NativeAuthority};
///
/// fn assert_from<T: From<BodyManifestBinding>>() {}
/// assert_from::<NativeAuthority>();
/// ```
///
/// There is no `Default` impl for an authority:
///
/// ```compile_fail,E0277
/// use solstone_core_body_source::NativeAuthority;
///
/// fn assert_default<T: Default>() {}
/// assert_default::<NativeAuthority>();
/// ```
///
/// There is no `Deserialize` impl for an authority:
///
/// ```compile_fail,E0277
/// use solstone_core_body_source::NativeAuthority;
///
/// serde_json::from_str::<NativeAuthority>("null");
/// ```
pub struct NativeAuthority {
    binding: BodyManifestBinding,
}

impl NativeAuthority {
    /// Returns the validated bundle identifier.
    pub fn id(&self) -> &BundleId {
        self.binding.import_id()
    }

    /// Returns the checked manifest binding that establishes this authority.
    pub fn binding(&self) -> &BodyManifestBinding {
        &self.binding
    }
}

/// A bounded native body-authority failure.
#[derive(Clone, PartialEq, Eq)]
pub enum AuthorityError {
    NotNativeCandidate,
    InvalidDirectory,
    MissingEnvelope,
    MissingLedger,
    MissingManifest,
    InvalidManifest(ManifestBindingError),
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotNativeCandidate => write!(formatter, "body-authority: not a native candidate"),
            Self::InvalidDirectory => {
                write!(formatter, "body-authority: invalid directory <invalid>")
            }
            Self::MissingEnvelope => {
                write!(formatter, "body-authority: missing {BODY_BUNDLE_REF_VALUE}")
            }
            Self::MissingLedger => {
                write!(formatter, "body-authority: missing {LEDGER_SIDECAR_NAME}")
            }
            Self::MissingManifest => {
                write!(formatter, "body-authority: missing {MANIFEST_FILE_NAME}")
            }
            Self::InvalidManifest(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl fmt::Debug for AuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for AuthorityError {}

/// Classifies raw directory observations without validating directory spelling.
pub fn classify_bundle_directory(observation: DirectoryObservation<'_>) -> BundleClass {
    if observation.name.starts_with(STAGING_PREFIX) {
        return BundleClass::StagingExcluded;
    }

    let manifest_signals_native = match observation.manifest {
        None => false,
        Some(bytes) => matches!(
            inspect_body_manifest_signal(Some(bytes)),
            ManifestKeySignal::BodyKeyPresent { .. } | ManifestKeySignal::Unreadable
        ),
    };
    let has_native_signal = observation.name.starts_with(NATIVE_PREFIX)
        || observation.envelope_present
        || observation.ledger_present
        || manifest_signals_native;
    if has_native_signal {
        BundleClass::NativeCandidate
    } else {
        BundleClass::LegacyCandidate
    }
}

/// Validates a native candidate into its checked authority.
pub fn authorize_native_bundle(
    observation: DirectoryObservation<'_>,
) -> Result<NativeAuthority, AuthorityError> {
    if classify_bundle_directory(observation) != BundleClass::NativeCandidate {
        return Err(AuthorityError::NotNativeCandidate);
    }
    let bundle_id =
        BundleId::from_bytes(observation.name).map_err(|_| AuthorityError::InvalidDirectory)?;
    if !observation.envelope_present {
        return Err(AuthorityError::MissingEnvelope);
    }
    if !observation.ledger_present {
        return Err(AuthorityError::MissingLedger);
    }
    let manifest_bytes = observation
        .manifest
        .ok_or(AuthorityError::MissingManifest)?;
    let binding = decode_body_manifest(manifest_bytes, &bundle_id)
        .map_err(AuthorityError::InvalidManifest)?;
    Ok(NativeAuthority { binding })
}
