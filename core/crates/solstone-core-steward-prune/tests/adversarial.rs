// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_steward_prune::{
    Disposition, MAX_ROW_CONTENT_BYTES, WholeNoopReason, classify_prune,
};

const FIXTURE: &str = include_str!("fixtures/adversarial.json");
const NOW: i64 = 2_000_000_000_000;
const OLD: &[u8] = b"{\"ts\":1997407999999}\n";
const FUTURE: &[u8] = b"{\"ts\":2000000000001}\n";

fn assert_noop(input: &[u8], reason: WholeNoopReason) {
    let result = classify_prune(input, NOW);
    assert_eq!(result.output, input);
    assert_eq!(result.aged, 0);
    assert_eq!(result.malformed, 0);
    assert_eq!(result.compatibility_kept, 0);
    assert_eq!(result.disposition, Disposition::WholeNoop(reason));
}

fn nested_arrays(depth: usize) -> Vec<u8> {
    let mut value = Vec::with_capacity(depth * 2 + 1);
    value.extend(std::iter::repeat_n(b'[', depth));
    value.push(b'0');
    value.extend(std::iter::repeat_n(b']', depth));
    value
}

#[test]
fn adversarial_fixture_registers_the_owned_cases() {
    let document: Value = serde_json::from_str(FIXTURE).expect("fixture is JSON");
    assert_eq!(document.get("now_ms").and_then(Value::as_i64), Some(NOW));
    assert_eq!(
        document
            .pointer("/content_cap/content_bytes")
            .and_then(Value::as_array)
            .expect("content cap twins"),
        &vec![
            Value::from(MAX_ROW_CONTENT_BYTES),
            Value::from(MAX_ROW_CONTENT_BYTES + 1)
        ]
    );
    assert_eq!(
        document.get("mixed_row_count").and_then(Value::as_u64),
        Some(101)
    );
    assert_eq!(
        document
            .pointer("/unknown_parser_difference/near_miss_tokens")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3)
    );
}

#[test]
fn one_mib_cap_is_per_content_row_and_excludes_terminators() {
    for terminator in [b"\n".as_slice(), b"".as_slice()] {
        let mut exact = Vec::with_capacity(MAX_ROW_CONTENT_BYTES + terminator.len());
        exact.push(b'"');
        exact.extend(std::iter::repeat_n(b'a', MAX_ROW_CONTENT_BYTES - 2));
        exact.push(b'"');
        exact.extend_from_slice(terminator);
        let result = classify_prune(&exact, NOW);
        assert_eq!(result.output, exact);
        assert_eq!(result.disposition, Disposition::NoChange);

        let mut over = OLD.to_vec();
        over.push(b'"');
        over.extend(std::iter::repeat_n(b'a', MAX_ROW_CONTENT_BYTES - 1));
        over.push(b'"');
        over.extend_from_slice(terminator);
        assert_noop(&over, WholeNoopReason::UnknownParserDifference);
    }
}

#[test]
fn extended_tokens_are_scoped_to_the_final_direct_ts_member() {
    assert_noop(
        b"{\"ts\":1997407999999}\n{\"ts\":2000000000001,\"unrelated\":Infinity}\n",
        WholeNoopReason::UnknownParserDifference,
    );

    for token in ["NaNx", "Infinityx", "-Infinityx"] {
        let input = format!("{{\"ts\":2000000000001,\"unrelated\":{token}}}\n");
        let result = classify_prune(input.as_bytes(), NOW);
        assert_eq!(result.output, b"");
        assert_eq!(result.malformed, 1, "{token}");
        assert_eq!(
            result.disposition,
            Disposition::Rewrite { dropped: 1 },
            "{token}"
        );
    }
    for token in [
        b"NaN".as_slice(),
        b"Infinity".as_slice(),
        b"-Infinity".as_slice(),
    ] {
        let result = classify_prune(token, NOW);
        assert_eq!(result.output, token);
        assert_eq!(result.disposition, Disposition::NoChange);
    }

    assert_noop(
        b"{\"nested\":{\"ts\":Infinity}}\n",
        WholeNoopReason::UnknownParserDifference,
    );

    let future_last = classify_prune(b"{\"ts\":1997407999999,\"ts\":2000000000001}\n", NOW);
    assert_eq!(future_last.disposition, Disposition::NoChange);
    let stale_last = classify_prune(b"{\"ts\":2000000000001,\"ts\":1997407999999}\n", NOW);
    assert_eq!(stale_last.output, b"");
    assert_eq!(stale_last.disposition, Disposition::Rewrite { dropped: 1 });
}

#[test]
fn four_proven_json_decode_error_shapes_drop_only_their_rows() {
    for malformed in [
        b"not-json\n".as_slice(),
        b"{\"ts\":}\n",
        b"[1,]\n",
        b"{} trailing\n",
    ] {
        let mut input = FUTURE.to_vec();
        input.extend_from_slice(malformed);
        let result = classify_prune(&input, NOW);
        assert_eq!(result.output, FUTURE, "{:?}", malformed);
        assert_eq!((result.aged, result.malformed), (0, 1));
        assert_eq!(result.disposition, Disposition::Rewrite { dropped: 1 });
    }
}

#[test]
fn blank_control_separators_are_preserved_as_python_blank_rows() {
    let blanks = "\u{001c}\n\u{001d}\r\u{001e}\r\n\u{001f}\n";
    let input = format!("{blanks}{{\"ts\":1997407999999}}\n");
    let result = classify_prune(input.as_bytes(), NOW);
    assert_eq!(result.output, blanks.as_bytes());
    assert_eq!((result.aged, result.malformed), (1, 0));
    assert_eq!(result.disposition, Disposition::Rewrite { dropped: 1 });
}

#[test]
fn long_runs_and_101_mixed_rows_are_iterative_and_accounted() {
    let escaped = format!(
        "{{\"note\":\"{}\",\"ts\":2000000000001}}\n",
        "\\u0061".repeat(20_000)
    );
    let mut nested = br#"{"ts":"#.to_vec();
    // The outer object contributes one open container, so 9,999 arrays reach
    // the inclusive compatibility ceiling of 10,000 rather than recursion.
    nested.extend(nested_arrays(9_999));
    nested.extend_from_slice(b"}\n");
    let mut input = escaped.into_bytes();
    input.extend(nested);
    let result = classify_prune(&input, NOW);
    assert_eq!(result.compatibility_kept, 1);
    assert_eq!(result.disposition, Disposition::NoChange);

    let mut mixed = Vec::new();
    for index in 0..101 {
        if index % 3 == 0 {
            mixed.extend_from_slice(OLD);
        } else if index % 3 == 1 {
            mixed.extend_from_slice(b"not-json\n");
        } else {
            mixed.extend_from_slice(FUTURE);
        }
    }
    let result = classify_prune(&mixed, NOW);
    assert_eq!((result.aged, result.malformed), (34, 34));
    assert_eq!(result.disposition, Disposition::Rewrite { dropped: 68 });
    assert_eq!(result.output, FUTURE.repeat(33));
}

#[test]
fn quoted_string_digit_limit_is_row_local_at_the_4300_boundary() {
    let at_limit = format!("{{\"ts\":\"-{}\"}}\n", "1".repeat(4_300));
    let result = classify_prune(at_limit.as_bytes(), NOW);
    assert_eq!(result.output, b"");
    assert_eq!((result.aged, result.malformed), (1, 0));
    assert_eq!(result.disposition, Disposition::Rewrite { dropped: 1 });

    let over_limit = format!("{{\"ts\":\"-{}\"}}\n", "1".repeat(4_301));
    let result = classify_prune(over_limit.as_bytes(), NOW);
    assert_eq!(result.output, over_limit.as_bytes());
    assert_eq!(result.disposition, Disposition::NoChange);
}

#[test]
fn digit_limit_precedes_later_malformed_tokens_but_not_earlier_ones() {
    let digits = "1".repeat(4_301);
    let digit_first = format!("{{\"ts\":{digits},\"extra\":xyz}}\n");
    assert_noop(digit_first.as_bytes(), WholeNoopReason::IntegerDigitLimit);

    let malformed_first = format!("{{\"extra\":xyz,\"ts\":{digits}}}\n");
    let result = classify_prune(malformed_first.as_bytes(), NOW);
    assert_eq!(result.output, b"");
    assert_eq!((result.aged, result.malformed), (0, 1));
    assert_eq!(result.disposition, Disposition::Rewrite { dropped: 1 });
}

#[test]
fn digit_limit_prefix_precedes_an_attached_malformed_suffix() {
    let over_limit = format!("{{\"ts\":{}x}}\n", "1".repeat(4_301));
    assert_noop(over_limit.as_bytes(), WholeNoopReason::IntegerDigitLimit);

    let at_limit = format!("{{\"ts\":{}x}}\n", "1".repeat(4_300));
    let result = classify_prune(at_limit.as_bytes(), NOW);
    assert_eq!(result.output, b"");
    assert_eq!((result.aged, result.malformed), (0, 1));
    assert_eq!(result.disposition, Disposition::Rewrite { dropped: 1 });
}

#[test]
fn recursion_limit_precedes_later_malformed_tokens() {
    let mut input = br#"{"ts":"#.to_vec();
    input.extend(std::iter::repeat_n(b'[', 10_000));
    input.extend_from_slice(b"0,xyz");
    assert_noop(input.as_slice(), WholeNoopReason::RecursionLimit);
}

#[test]
fn same_row_precedence_is_independent_of_member_order() {
    for row in [
        format!("{{\"ts\":Infinity,\"other\":{}}}\n", "1".repeat(4301)),
        format!("{{\"other\":{},\"ts\":Infinity}}\n", "1".repeat(4301)),
    ] {
        assert_noop(row.as_bytes(), WholeNoopReason::IntegerDigitLimit);
    }
    for row in [
        format!(
            "{{\"ts\":Infinity,\"other\":{}}}\n",
            String::from_utf8(nested_arrays(10_000)).expect("ASCII")
        ),
        format!(
            "{{\"other\":{},\"ts\":Infinity}}\n",
            String::from_utf8(nested_arrays(10_000)).expect("ASCII")
        ),
    ] {
        assert_noop(row.as_bytes(), WholeNoopReason::NumericOverflow);
    }
    for row in [
        format!(
            "{{\"ts\":1,\"other\":Infinity,\"deep\":{}}}\n",
            String::from_utf8(nested_arrays(10_000)).expect("ASCII")
        ),
        format!(
            "{{\"deep\":{},\"other\":Infinity,\"ts\":1}}\n",
            String::from_utf8(nested_arrays(10_000)).expect("ASCII")
        ),
    ] {
        assert_noop(row.as_bytes(), WholeNoopReason::RecursionLimit);
    }
}
