# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import ipaddress
import secrets

import pytest

from solstone.apps.link.crockford32 import decode, encode


def test_encode_matches_pair_link_reference_vector() -> None:
    payload = (
        bytes([2, 1])
        + ipaddress.IPv4Address("192.0.2.42").packed
        + (7070).to_bytes(2, "big")
        + bytes.fromhex("A1B2C3D4E5F60718")
        + bytes.fromhex("DEADBEEFCAFEBABE0123456789ABCDEF")
    )

    assert len(payload) == 32
    assert encode(payload) == "080W000258DSX8DJRFAEBXG733FAVFQFSBZBNFG14D2PF2DBSQQG"


def test_round_trip_random_payloads() -> None:
    for length in range(65):
        for _ in range(2):
            payload = secrets.token_bytes(length)
            assert decode(encode(payload)) == payload


def test_decode_tolerates_crockford_aliases_case_and_separators() -> None:
    assert decode("0o0i0l") == decode("000110")
    assert decode("abcd- efgh") == decode("ABCDEFGH")
    assert decode("ABCD\tEFGH\n") == decode("ABCDEFGH")


@pytest.mark.parametrize("text", ["?", "U"])
def test_decode_rejects_non_alphabet_characters(text: str) -> None:
    with pytest.raises(ValueError, match="invalid Crockford"):
        decode(text)


def test_decode_rejects_non_zero_trailing_pad_bits() -> None:
    with pytest.raises(ValueError, match="trailing pad bits"):
        decode("0" * 51 + "1")
