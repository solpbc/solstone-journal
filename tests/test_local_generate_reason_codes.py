# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import pytest

from solstone.think import generate_wire
from solstone.think.providers import local


def test_native_generate_failure_codes_match_wire_contract():
    local_contract = local._local_generate_contract()
    wire_contract = generate_wire._generate_contract()
    expected = {entry["code"]: entry for entry in wire_contract["reason_codes"]}
    cases = [*local_contract["reason_codes"], None]
    for local_code in cases:
        with pytest.raises(Exception) as raised:
            local._raise_native_generate_failure(
                {"outcome": "failure", "reason_code": local_code, "detail": "test"}
            )
        refusal = generate_wire._v2_refusal(raised.value, "request", "local")
        public_code = local_contract["reason_codes"].get(local_code)
        classification = expected.get(public_code, wire_contract["unknown_member"])
        assert refusal["reason_code"] == public_code
        assert refusal["retryable"] is classification["retryable"]
        assert refusal["blocking"] is classification["blocking"]
