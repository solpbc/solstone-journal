# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import solstone.think.services.constants as constants
from solstone.think.services.cli import _mint_nonce
from solstone.think.services.constants import (
    NONCE_ALPHABET,
    NONCE_LENGTH_CHARS,
    NONCE_REGEX,
)


def test_nonce_constants_match_worker_contract() -> None:
    assert NONCE_ALPHABET == "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
    assert NONCE_LENGTH_CHARS == 52
    assert NONCE_REGEX.pattern == r"^[A-Z2-7]{52}$"


def test_minted_nonces_match_regex_and_are_high_cardinality() -> None:
    samples = [_mint_nonce() for _ in range(1000)]

    assert all(NONCE_REGEX.fullmatch(sample) for sample in samples)
    assert all(set(sample) <= set(NONCE_ALPHABET) for sample in samples)
    assert len(set(samples)) >= 990


def test_device_code_constants_are_not_defined() -> None:
    assert not any(name.startswith("DEVICE_CODE_") for name in dir(constants))
