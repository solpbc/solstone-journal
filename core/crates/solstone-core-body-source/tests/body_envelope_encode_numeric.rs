// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyDay, BodyDigest, BodyEnvelope, BodyRawRetention, BodySourceFamily, BodySourceHash,
    BundleId, EnvelopeLedger, EnvelopeShard, encode_body_envelope,
};

#[test]
fn u64_max_aggregate_uses_exact_unsigned_decimal_spellings() {
    assert_numeric_encoding(u64::MAX, "18446744073709551615");
}

#[test]
fn adjacent_u64_aggregate_uses_exact_unsigned_decimal_spellings() {
    assert_numeric_encoding(u64::MAX - 1, "18446744073709551614");
}

fn assert_numeric_encoding(value: u64, expected: &str) {
    let encoded = String::from_utf8(encode_body_envelope(&envelope(value)).unwrap()).unwrap();
    for field in ["row_count", "bytes", "rows", "events"] {
        assert!(encoded.contains(&format!("\"{field}\":{expected}")));
    }
    assert!(!encoded.contains(&format!("\"row_count\":-{expected}")));
    assert!(!encoded.contains(&format!("\"row_count\":{expected}.")));
    assert!(!encoded.contains(&format!("\"row_count\":{expected}e")));
    assert!(!encoded.contains(&format!("\"row_count\":{expected}E")));
}

fn envelope(value: u64) -> BodyEnvelope {
    let bundle = BundleId::from_bytes(b"body-00000000000000000000000000").unwrap();
    let digest = BodyDigest::from_bytes(
        b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .unwrap();
    let day = BodyDay::from_bytes(b"20260102").unwrap();
    BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::OuraApi,
        BodySourceHash::from_bytes_for_family(
            b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            &BodySourceFamily::OuraApi,
        )
        .unwrap(),
        BodyRawRetention::RetainParsed,
        value,
        vec![day.clone()],
        vec![EnvelopeShard::new(&bundle, 0, day.month(), value, value, digest.clone()).unwrap()],
        EnvelopeLedger::new(&bundle, value, value, digest).unwrap(),
        None,
    )
    .unwrap()
}
