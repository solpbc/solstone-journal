// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The premises `GENERATE.md`'s precedence rule rests on.
//!
//! The contract publishes both `retryable` and `blocking` on every refusal, and
//! states that `blocking` governs — `retryable` is read only when `blocking` is
//! false. That rule exists because the two fields fire together on every
//! blocking refusal, so a consumer reading either one first produces a
//! defensible-looking answer and nothing reveals the disagreement.
//!
//! ⚠ The rule is prose. These are the facts underneath it, and without them
//! nothing notices when the taxonomy moves out from under the document.
//!
//! ⛔ This target is deliberately NOT differential-gated: it reads the fixture
//! and executes no interpreter.

use serde_json::Value;
use solstone_core_generate::contract;

fn reason_codes() -> &'static Vec<Value> {
    contract()["reason_codes"]
        .as_array()
        .expect("the contract carries reason_codes")
}

fn code(name: &str) -> &'static Value {
    reason_codes()
        .iter()
        .find(|entry| entry["code"] == name)
        .unwrap_or_else(|| panic!("the contract no longer carries the reason code {name}"))
}

/// 🔴 The four codes where the ordering is *observable*.
///
/// On a persistent blocking refusal both orderings end in the hold once the
/// budget is spent, so an exit-code assertion passes either way. Only a
/// transient blocking code separates them: read `retryable` first and the
/// consumer keeps calling through a provider the boundary already declared
/// unusable — the inverse of what `blocking` instructs.
///
/// ⛔ If one of these stops being blocking, the document's worked example is
/// wrong and the precedence rule loses the case that motivates it.
#[test]
fn the_codes_that_expose_the_ordering_are_still_blocking_and_retryable() {
    for name in [
        "attestation_stale",
        "local_model_loading",
        "provider_quota_exceeded",
        "install_busy",
    ] {
        let entry = code(name);
        assert_eq!(
            entry["blocking"], true,
            "{name} must be blocking — it is one of the transient codes the precedence rule turns on"
        );
        assert_eq!(
            entry["retryable"], true,
            "{name} must be retryable — if it were not, this code would stop exposing the ordering"
        );
    }
}

/// The ambiguity the rule resolves is real, and it is not an accident of one code.
///
/// ⚠ Asserted as "more than one", not as an exact count: the taxonomy is
/// expected to grow, and a blocking code that is genuinely non-retryable would
/// be a legitimate addition. What must not silently become true is that *no*
/// code carries both — at which point the rule is describing a case the
/// contract no longer has.
#[test]
fn blocking_and_retryable_still_fire_together() {
    let both = reason_codes()
        .iter()
        .filter(|entry| entry["blocking"] == true && entry["retryable"] == true)
        .count();
    assert!(
        both > 1,
        "only {both} reason codes carry both blocking and retryable; the precedence rule in \
         GENERATE.md exists to resolve that overlap and is describing a contract that no \
         longer matches"
    );
}

/// 🔴 The preserving default, on the path a consumer reaches when it has never
/// heard of the code it was handed.
///
/// ⛔ Both fields, asserted separately. `blocking: true` alone would let a
/// consumer that reads `retryable` first burn its re-entry bound against an
/// unusable provider, and `retryable: false` alone would let one that reads
/// `blocking` first record a failed attempt instead of holding the material.
#[test]
fn an_unknown_reason_code_resolves_to_the_preserving_direction() {
    let unknown = &contract()["unknown_member"];
    assert_eq!(unknown["blocking"], true, "an unknown code must block");
    assert_eq!(
        unknown["retryable"], false,
        "an unknown code must not be retried"
    );
    assert_eq!(unknown["refusal_reason"], "unknown");
}

/// The attestation family, classified by this contract rather than by the live
/// Python taxonomy — which omits all three, so its blocking predicate answers
/// `false` for exactly the case that must hold the owner's material.
#[test]
fn the_attestation_family_is_blocking_by_this_contracts_own_classification() {
    for name in [
        "attestation_not_yet_verified",
        "attestation_failed",
        "attestation_stale",
    ] {
        let entry = code(name);
        assert_eq!(
            entry["blocking"], true,
            "{name} must be blocking: an unverifiable confidential environment holds the \
             owner's material rather than recording a failed attempt"
        );
        assert_eq!(
            entry["overrides_live_taxonomy"], true,
            "{name} is absent from the live taxonomy, so the contract must declare that it \
             is overriding it — otherwise a regenerated fixture could quietly inherit the \
             taxonomy's `false`"
        );
    }
}
