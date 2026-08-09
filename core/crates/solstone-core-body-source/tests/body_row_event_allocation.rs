// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use allocation_counter::measure;
use solstone_core_body_source::{
    decode_body_envelope, decode_body_ledger_event, validate_body_row_event,
};

mod support;

use support::native_bundle_fixture;

#[test]
fn oversized_rows_have_bounded_peak_allocation() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
            .expect("fixture envelope decodes");
    let event = decode_body_ledger_event(
        case["expected_ledger_jsonl"].as_str().unwrap().as_bytes(),
        &envelope,
        1,
    )
    .expect("fixture event decodes");
    for size in [1_048_577, 4 * 1_048_576] {
        let frame = vec![b'x'; size];
        let info = measure(|| {
            let error = validate_body_row_event(&envelope, &frame, &event)
                .expect_err("oversized row refuses");
            assert_eq!(error.kind().as_str(), "input_too_large");
        });
        assert!(
            info.bytes_max <= 128 * 1024,
            "{size}-byte input peaked at {} bytes",
            info.bytes_max
        );
    }
}
