// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pinned minisign verification for the Journal release key.
//!
//! A release build resolves the compiled product pin and reads no environment.
//! `SOLSTONE_JOURNAL_MINISIGN_PIN` is consulted only when the `test-fixture-pin`
//! feature is enabled.

use std::fmt;
#[cfg(feature = "test-fixture-pin")]
use std::fs;
use std::io::Cursor;
use std::path::Path;
#[cfg(feature = "test-fixture-pin")]
use std::sync::OnceLock;

use minisign::{PublicKey, PublicKeyBox, SignatureBox};

pub const PRODUCT_PIN: &str =
    include_str!("../../../../packaging/keys/solstone-journal-release.pub");

#[cfg(feature = "test-fixture-pin")]
const PIN_OVERRIDE_ENV: &str = "SOLSTONE_JOURNAL_MINISIGN_PIN";
#[cfg(feature = "test-fixture-pin")]
static FIXTURE_PIN: OnceLock<String> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedSignatureRefusal {
    UnparseableSignature,
    SignaturePinMismatch,
}

impl PinnedSignatureRefusal {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnparseableSignature => "unparseable-signature",
            Self::SignaturePinMismatch => "signature-pin-mismatch",
        }
    }
}

#[derive(Debug)]
pub struct PinnedSignatureError {
    refusal: PinnedSignatureRefusal,
    detail: String,
}

impl PinnedSignatureError {
    fn new(refusal: PinnedSignatureRefusal, detail: impl Into<String>) -> Self {
        Self {
            refusal,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub const fn refusal(&self) -> PinnedSignatureRefusal {
        self.refusal
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for PinnedSignatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}\n  {}", self.refusal.as_str(), self.detail)
    }
}

impl std::error::Error for PinnedSignatureError {}

/// Verify arbitrary bytes against the pinned Journal release key.
pub fn verify_pinned_signature(
    signed_bytes: &[u8],
    signature_path: &Path,
    signature_bytes: &[u8],
) -> Result<(), PinnedSignatureError> {
    let signature_text = std::str::from_utf8(signature_bytes).map_err(|error| {
        PinnedSignatureError::new(
            PinnedSignatureRefusal::UnparseableSignature,
            format!("{}: {error}", signature_path.display()),
        )
    })?;
    let signature_box = SignatureBox::from_string(signature_text).map_err(|error| {
        PinnedSignatureError::new(
            PinnedSignatureRefusal::UnparseableSignature,
            format!("{}: {error}", signature_path.display()),
        )
    })?;
    let pin = resolve_pin()?;
    minisign::verify(
        &pin,
        &signature_box,
        Cursor::new(signed_bytes),
        true,
        false,
        false,
    )
    .map_err(|error| {
        PinnedSignatureError::new(
            PinnedSignatureRefusal::SignaturePinMismatch,
            format!("{}: {error}", signature_path.display()),
        )
    })
}

fn parse_pin(text: &str) -> Result<PublicKey, PinnedSignatureError> {
    let boxed = PublicKeyBox::from_string(text).map_err(|error| {
        PinnedSignatureError::new(
            PinnedSignatureRefusal::SignaturePinMismatch,
            format!("could not parse public pin: {error}"),
        )
    })?;
    PublicKey::from_box(boxed).map_err(|error| {
        PinnedSignatureError::new(
            PinnedSignatureRefusal::SignaturePinMismatch,
            format!("could not parse public pin: {error}"),
        )
    })
}

/// Environment entries a signed-package helper needs so that it resolves the
/// same manifest pin its parent just resolved.
///
/// Empty on a production build, where the pin is compiled in and
/// [`resolve_pin`] reads no environment at all.
///
/// It is not empty on a `test-fixture-pin` build, and that difference is
/// load-bearing. A helper that re-verifies the signed payload for itself
/// runs under a bounded helper that clears the environment down to
/// `SystemRoot`. Without this, the parent resolves the fixture pin and admits
/// a test-signed package while its own child resolves the product pin and
/// refuses the same bytes.
#[must_use]
pub fn signed_package_pin_environment() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    #[cfg(feature = "test-fixture-pin")]
    {
        pin_environment_from(std::env::var_os(PIN_OVERRIDE_ENV))
    }
    #[cfg(not(feature = "test-fixture-pin"))]
    {
        Vec::new()
    }
}

#[cfg(feature = "test-fixture-pin")]
fn pin_environment_from(
    value: Option<std::ffi::OsString>,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    value
        .filter(|path| !path.is_empty())
        .map(|path| vec![(std::ffi::OsString::from(PIN_OVERRIDE_ENV), path)])
        .unwrap_or_default()
}

#[cfg(not(feature = "test-fixture-pin"))]
pub fn resolve_pin() -> Result<PublicKey, PinnedSignatureError> {
    parse_pin(PRODUCT_PIN)
}

#[cfg(feature = "test-fixture-pin")]
pub fn resolve_pin() -> Result<PublicKey, PinnedSignatureError> {
    let text = match std::env::var(PIN_OVERRIDE_ENV) {
        Ok(path) if !path.is_empty() => fs::read_to_string(&path).map_err(|error| {
            PinnedSignatureError::new(
                PinnedSignatureRefusal::SignaturePinMismatch,
                format!("could not read {PIN_OVERRIDE_ENV} {path}: {error}"),
            )
        })?,
        _ => FIXTURE_PIN
            .get()
            .cloned()
            .unwrap_or_else(|| PRODUCT_PIN.to_owned()),
    };
    parse_pin(&text)
}

#[cfg(feature = "test-fixture-pin")]
pub fn install_test_fixture_pin(path: &Path) -> Result<(), PinnedSignatureError> {
    let text = fs::read_to_string(path).map_err(|error| {
        PinnedSignatureError::new(
            PinnedSignatureRefusal::SignaturePinMismatch,
            format!("could not read fixture pin {}: {error}", path.display()),
        )
    })?;
    match FIXTURE_PIN.get() {
        Some(existing) if existing != &text => Err(PinnedSignatureError::new(
            PinnedSignatureRefusal::SignaturePinMismatch,
            "fixture pin was already installed with different bytes",
        )),
        Some(_) => Ok(()),
        None => {
            let _ = FIXTURE_PIN.set(text);
            Ok(())
        }
    }
}

#[cfg(all(test, not(feature = "test-fixture-pin")))]
mod tests {
    use super::{PRODUCT_PIN, parse_pin, resolve_pin};

    #[test]
    fn default_resolve_pin_returns_the_compiled_product_pin() {
        let resolved = resolve_pin().expect("compiled pin parses");
        let expected = parse_pin(PRODUCT_PIN).expect("product pin parses");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn a_product_build_hands_a_signed_package_helper_no_pin_environment() {
        assert!(super::signed_package_pin_environment().is_empty());
    }
}

#[cfg(all(test, feature = "test-fixture-pin"))]
mod fixture_pin_environment_tests {
    use std::ffi::OsString;

    use super::{PIN_OVERRIDE_ENV, pin_environment_from};

    #[test]
    fn a_fixture_build_passes_the_override_down_to_the_helper() {
        assert_eq!(
            pin_environment_from(Some(OsString::from("C:/pins/checkpoint.pub"))),
            vec![(
                OsString::from(PIN_OVERRIDE_ENV),
                OsString::from("C:/pins/checkpoint.pub"),
            )]
        );
    }

    #[test]
    fn an_unset_or_empty_override_adds_nothing() {
        assert!(pin_environment_from(None).is_empty());
        assert!(pin_environment_from(Some(OsString::new())).is_empty());
    }
}
