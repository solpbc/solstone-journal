// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyDigest, BodyEnvelope, BodyLedgerValidator, BundleId, LedgerEventError, ValidatedBodyLedger,
};

type BundleAccessor = fn(&ValidatedBodyLedger) -> &BundleId;
type BytesAccessor = fn(&ValidatedBodyLedger) -> u64;
type EventsAccessor = fn(&ValidatedBodyLedger) -> u64;
type DigestAccessor = fn(&ValidatedBodyLedger) -> &BodyDigest;

#[test]
fn public_validator_and_receipt_shapes_type_check() {
    fn validator_api(envelope: &BodyEnvelope) -> Result<ValidatedBodyLedger, LedgerEventError> {
        let mut validator = BodyLedgerValidator::new(envelope);
        let push_result: Result<(), LedgerEventError> = validator.push(b"");
        push_result?;
        validator.finish()
    }

    let _: fn(&BodyEnvelope) -> Result<ValidatedBodyLedger, LedgerEventError> = validator_api;
    let _: BundleAccessor = ValidatedBodyLedger::bundle_id;
    let _: BytesAccessor = ValidatedBodyLedger::bytes;
    let _: EventsAccessor = ValidatedBodyLedger::events;
    let _: DigestAccessor = ValidatedBodyLedger::sha256;
}
