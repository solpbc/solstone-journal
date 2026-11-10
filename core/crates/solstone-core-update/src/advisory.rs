// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The signed security advisory: which versions are missing a security fix.
//!
//! One small document, signed with the platform release key, names the
//! version of the journal on each desktop OS and of each desktop solstone app
//! that carries the latest security fix. A journal fetches it with a bare GET,
//! verifies it, and compares its own version and each paired app's against it.
//! A floor only ever causes a notice: nothing is refused because of it.
//!
//! An advisory the journal cannot confirm (unreachable, unsigned, expired,
//! older than one it already accepted) flags nothing.

use std::collections::BTreeMap;
use std::path::Path;

pub use minisign::PublicKey;
use serde::{Deserialize, Serialize};

use crate::fetch::{UPDATE_ORIGIN, download_bytes};

const ADVISORY_HOST: &str = "updates.solstone.app";
pub const ADVISORY_PATH: &str = "/security/advisory.json";
pub const SIGNATURE_PATH: &str = "/security/advisory.json.minisig";
const ADVISORY_MAX_BYTES: u64 = 64 * 1024;
const SIGNATURE_MAX_BYTES: u64 = 16 * 1024;
const MAX_DIGITS: usize = 20;

/// The only schema this journal reads. A newer schema is unconfirmable here,
/// which flags nothing.
pub const SCHEMA_VERSION: u64 = 1;

/// The signed trusted comment that makes these bytes an advisory, so no other
/// document signed with the same key is read as one.
#[must_use]
pub fn trusted_comment(sequence: u64) -> String {
    format!("solstone security advisory {sequence}")
}

/// One floor: an install below `fixed_in` is missing a security fix that
/// `notes` describes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Floor {
    pub fixed_in: String,
    pub notes: String,
}

/// A verified advisory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Advisory {
    pub schema_version: u64,
    pub sequence: u64,
    pub issued_unix: u64,
    pub expires_unix: u64,
    /// Journal floors by OS (`linux`, `macos`, `windows`).
    #[serde(default)]
    pub journal: BTreeMap<String, Floor>,
    /// App floors by the app id a paired device reports.
    #[serde(default)]
    pub clients: BTreeMap<String, Floor>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvisoryRefusal {
    FetchFailed,
    SignatureInvalid,
    SchemaInvalid,
    Expired,
    /// A lower sequence than an advisory this journal already accepted.
    Replayed,
    PinInvalid,
}

impl AdvisoryRefusal {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::FetchFailed => "fetch-failed",
            Self::SignatureInvalid => "signature-invalid",
            Self::SchemaInvalid => "schema-invalid",
            Self::Expired => "expired",
            Self::Replayed => "replayed",
            Self::PinInvalid => "pin-invalid",
        }
    }
}

/// Verify `document` against `signature` and `key` at `now_unix`, refusing a
/// sequence below `accepted_sequence`.
pub fn verify(
    document: &[u8],
    signature: &[u8],
    key: &PublicKey,
    now_unix: u64,
    accepted_sequence: Option<u64>,
) -> Result<Advisory, AdvisoryRefusal> {
    solstone_core_installed_payload::verify_prehashed_signature_with(
        key,
        document,
        Path::new("advisory.json.minisig"),
        signature,
    )
    .map_err(|_| AdvisoryRefusal::SignatureInvalid)?;
    let advisory = parse(document)?;
    // The trusted comment is covered by the signature: it binds the purpose
    // and the sequence, so another signed document is never an advisory.
    let comment = std::str::from_utf8(signature)
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("trusted comment: "))
        })
        .ok_or(AdvisoryRefusal::SignatureInvalid)?;
    if comment != trusted_comment(advisory.sequence) {
        return Err(AdvisoryRefusal::SignatureInvalid);
    }
    if now_unix >= advisory.expires_unix {
        return Err(AdvisoryRefusal::Expired);
    }
    if accepted_sequence.is_some_and(|accepted| advisory.sequence < accepted) {
        return Err(AdvisoryRefusal::Replayed);
    }
    Ok(advisory)
}

fn parse(document: &[u8]) -> Result<Advisory, AdvisoryRefusal> {
    let advisory: Advisory =
        serde_json::from_slice(document).map_err(|_| AdvisoryRefusal::SchemaInvalid)?;
    let floors_valid = advisory
        .journal
        .values()
        .chain(advisory.clients.values())
        .all(|floor| canonical_version(&floor.fixed_in) && floor.notes.starts_with("https://"));
    if advisory.schema_version != SCHEMA_VERSION
        || advisory.expires_unix <= advisory.issued_unix
        || !floors_valid
    {
        return Err(AdvisoryRefusal::SchemaInvalid);
    }
    Ok(advisory)
}

/// Fetch the advisory and its signature with a bare GET each: no identifier,
/// no version and nothing about the owner is sent.
pub fn fetch() -> Result<(Vec<u8>, Vec<u8>), AdvisoryRefusal> {
    let get = |path: &str, max: u64| {
        download_bytes(&format!("{UPDATE_ORIGIN}{path}"), ADVISORY_HOST, max)
            .map_err(|_| AdvisoryRefusal::FetchFailed)
    };
    Ok((
        get(ADVISORY_PATH, ADVISORY_MAX_BYTES)?,
        get(SIGNATURE_PATH, SIGNATURE_MAX_BYTES)?,
    ))
}

/// The compiled platform release key, which signs the advisory.
pub fn platform_key() -> Result<PublicKey, AdvisoryRefusal> {
    solstone_core_installed_payload::platform_pin().map_err(|_| AdvisoryRefusal::PinInvalid)
}

/// Whether `installed` is below `fixed_in`. An installed version this cannot
/// read is not flagged. A device's version may carry a build after the
/// version, as in `2.0.64 (130)`; only the leading version counts.
#[must_use]
pub fn is_below(installed: &str, fixed_in: &str) -> bool {
    match (leading_version(installed), leading_version(fixed_in)) {
        (Some(installed), Some(fixed_in)) => installed < fixed_in,
        _ => false,
    }
}

/// The `X.Y.Z` a reported version starts with, as in `2.0.64` for
/// `2.0.64 (130)`.
#[must_use]
pub fn version_of(text: &str) -> Option<&str> {
    let text = text.trim();
    let text = text.strip_prefix('v').unwrap_or(text);
    let end = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let core = &text[..end];
    canonical_version(core).then_some(core)
}

fn leading_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version_of(text)?
        .split('.')
        .map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

fn canonical_version(candidate: &str) -> bool {
    let parts: Vec<&str> = candidate.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= MAX_DIGITS
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (*part == "0" || !part.starts_with('0'))
        })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use minisign::KeyPair;
    use serde_json::json;

    use super::*;

    const NOW: u64 = 2_000_000_000;

    fn document(sequence: u64, expires_unix: u64) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "sequence": sequence,
            "issued_unix": NOW - 100,
            "expires_unix": expires_unix,
            "journal": {"linux": {"fixed_in": "2.0.66", "notes": "https://solstone.app/notes"}},
            "clients": {"app.solstone.windows": {"fixed_in": "2.0.45", "notes": "https://solstone.app/w"}},
            "added_later": {"ios": "ignored by this journal"},
        }))
        .unwrap()
    }

    fn sign(keys: &KeyPair, bytes: &[u8], comment: &str) -> Vec<u8> {
        minisign::sign(
            Some(&keys.pk),
            &keys.sk,
            Cursor::new(bytes),
            Some(comment),
            Some("signature from the platform key"),
        )
        .unwrap()
        .to_string()
        .into_bytes()
    }

    fn keys() -> KeyPair {
        KeyPair::generate_unencrypted_keypair().unwrap()
    }

    #[test]
    fn a_signed_current_advisory_verifies_and_reads_its_floors() {
        let keys = keys();
        let bytes = document(7, NOW + 100);
        let signature = sign(&keys, &bytes, &trusted_comment(7));
        let advisory = verify(&bytes, &signature, &keys.pk, NOW, Some(7)).unwrap();
        assert_eq!(advisory.sequence, 7);
        assert_eq!(advisory.journal["linux"].fixed_in, "2.0.66");
        assert_eq!(advisory.clients["app.solstone.windows"].fixed_in, "2.0.45");
    }

    #[test]
    fn an_unsigned_expired_replayed_or_repurposed_advisory_is_refused() {
        let keys = keys();
        let bytes = document(7, NOW + 100);
        let good = sign(&keys, &bytes, &trusted_comment(7));
        let other = self::keys();
        let refused = |signature: &[u8], now: u64, accepted: Option<u64>| {
            verify(&bytes, signature, &keys.pk, now, accepted).unwrap_err()
        };
        use AdvisoryRefusal::{Expired, Replayed, SignatureInvalid};
        assert_eq!(
            refused(&sign(&other, &bytes, &trusted_comment(7)), NOW, None),
            SignatureInvalid
        );
        assert_eq!(refused(b"not a signature", NOW, None), SignatureInvalid);
        // The release catalogue's signature, or any other, is not an advisory's.
        let catalogue = sign(&keys, &bytes, "solstone platform release manifest");
        assert_eq!(refused(&catalogue, NOW, None), SignatureInvalid);
        assert_eq!(
            refused(&sign(&keys, &bytes, &trusted_comment(8)), NOW, None),
            SignatureInvalid
        );
        assert_eq!(refused(&good, NOW + 100, None), Expired);
        assert_eq!(refused(&good, NOW, Some(8)), Replayed);
        let mut tampered = bytes;
        let at = tampered.windows(6).position(|w| w == b"2.0.66").unwrap();
        tampered[at + 5] = b'5';
        assert_eq!(
            verify(&tampered, &good, &keys.pk, NOW, None),
            Err(AdvisoryRefusal::SignatureInvalid)
        );
    }

    #[test]
    fn a_malformed_floor_or_another_schema_is_refused() {
        let keys = keys();
        for bad in [
            json!({"schema_version": 2, "sequence": 1, "issued_unix": 1, "expires_unix": NOW + 1}),
            json!({"schema_version": 1, "sequence": 1, "issued_unix": 1, "expires_unix": NOW + 1,
                   "journal": {"linux": {"fixed_in": "2.0", "notes": "https://x"}}}),
            json!({"schema_version": 1, "sequence": 1, "issued_unix": 1, "expires_unix": NOW + 1,
                   "clients": {"a": {"fixed_in": "2.0.1", "notes": "http://x"}}}),
        ] {
            let bytes = serde_json::to_vec(&bad).unwrap();
            let signature = sign(&keys, &bytes, &trusted_comment(1));
            assert_eq!(
                verify(&bytes, &signature, &keys.pk, NOW, None),
                Err(AdvisoryRefusal::SchemaInvalid)
            );
        }
    }

    #[test]
    fn a_version_below_its_floor_is_flagged_and_an_unreadable_one_is_not() {
        assert!(is_below("2.0.65", "2.0.66"));
        assert!(is_below("2.0.9", "2.0.10"));
        assert!(is_below("2.0.63 (144)", "2.0.64"));
        assert!(is_below("v1.9.99", "2.0.0"));
        assert!(!is_below("2.0.66", "2.0.66"));
        assert!(!is_below("2.0.64 (145)", "2.0.64"));
        assert!(!is_below("2.1.0", "2.0.66"));
        assert!(!is_below("unknown", "2.0.66"));
        assert!(!is_below("", "2.0.66"));
        assert!(!is_below("2.0", "2.0.66"));
    }
}
