// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_steward_prune::{MAX_ROW_CONTENT_BYTES, classify_prune};

fn measure(input: &[u8]) -> allocation_counter::AllocationInfo {
    allocation_counter::measure(|| {
        let result = classify_prune(input, 2_000_000_000_000);
        std::hint::black_box(result);
    })
}

#[test]
fn classifier_allocates_and_stays_within_the_adversarial_peak_budget() {
    let small = measure(b"{\"ts\":2000000000001}\nnot-json\n");
    assert!(
        small.count_total > 0,
        "the counter must observe classifier allocations"
    );

    let mut input = Vec::with_capacity(MAX_ROW_CONTENT_BYTES + 1);
    input.push(b'"');
    input.extend(std::iter::repeat_n(b'a', MAX_ROW_CONTENT_BYTES - 2));
    input.push(b'"');
    input.push(b'\n');
    let large = measure(&input);
    assert!(
        large.count_total > 0,
        "the large-row output allocation must be observed"
    );
    assert!(
        large.bytes_max <= (6 * input.len() + 64) as u64,
        "peak {} exceeds bounded input budget",
        large.bytes_max
    );
}
