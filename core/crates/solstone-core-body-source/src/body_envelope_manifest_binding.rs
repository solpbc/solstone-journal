// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};

use crate::{
    BodyDigest, BodyEnvelope, BodyManifestBinding, EnvelopeError, EnvelopeErrorCode,
    EnvelopeErrorField, decode_body_envelope,
};

/// Decodes canonical body-envelope JSONL bytes and binds them to a checked manifest.
pub fn decode_body_envelope_with_manifest(
    input: &[u8],
    binding: &BodyManifestBinding,
) -> Result<BodyEnvelope, EnvelopeError> {
    let envelope = decode_body_envelope(input)?;
    if envelope.bundle_id() != binding.import_id()
        || envelope.source_family() != binding.source_type()
        || envelope.source_hash() != binding.source_hash()
        || envelope.raw_retention() != binding.raw_retention()
        || envelope.row_count() != binding.entry_count()
        || envelope.days() != binding.days_affected()
        || &body_envelope_input_digest(input) != binding.body_bundle_sha256()
    {
        return Err(EnvelopeError::new(
            Some(envelope.bundle_id().clone()),
            EnvelopeErrorCode::ManifestMismatch,
            EnvelopeErrorField::ManifestBinding,
            None,
        ));
    }
    Ok(envelope)
}

fn body_envelope_input_digest(input: &[u8]) -> BodyDigest {
    let mut digest = Sha256::new();
    digest.update(input);
    let spelling = format!("sha256:{:x}", digest.finalize());
    BodyDigest::from_bytes(spelling.as_bytes())
        .expect("SHA-256 output is always a valid body digest")
}
