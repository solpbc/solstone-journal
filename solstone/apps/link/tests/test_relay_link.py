# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
import uuid

import pytest

from solstone.apps.link.crockford32 import decode as crockford_decode
from solstone.apps.link.relay_link import (
    CA_FP_TAG_SPKI_SHA256,
    RELAY_VERSION,
    compute_current_totp,
    encode_relay_pair_link,
)
from solstone.apps.link.routes import _build_pair_link

INSTANCE_ID = "12345678-1234-5678-1234-567812345678"
TOTP = 123456
NONCE = "0123456789abcdef0123456789abcdef"
CA_FP_SPKI = "deadbeefcafebabe0123456789abcdef0123456789abcdef0123456789abcdef"
WELL_KNOWN_URL = (
    "https://link.solpbc.org/p#"
    "0C938NKR28T5CY0J6HB7G4HMASW03RJ004HMASW9NF6YY0938NKRKAYDXW0XXBDYXZ5"
    "FXENY04HMASW9NF6YY00"
)
CUSTOM_URL = (
    "https://link.solpbc.org/p#"
    "0C938NKR28T5CY0J6HB7G4HMASW03RJ004HMASW9NF6YY0938NKRKAYDXW0XXBDYXZ5"
    "FXENY04HMASW9NF6YY5B8EHT70WST5WQQ4SBCC5WJWSBRC5PQ0V35"
)


def _expected_relay_blob(relay_origin: str | None) -> bytes:
    prefix = (
        bytes([RELAY_VERSION])
        + uuid.UUID(INSTANCE_ID).bytes
        + TOTP.to_bytes(3, "big")
        + bytes.fromhex(NONCE)
        + bytes([CA_FP_TAG_SPKI_SHA256])
        + bytes.fromhex(CA_FP_SPKI)[:16]
    )
    if relay_origin is None:
        return prefix + bytes([0x00])
    origin_bytes = relay_origin.encode("utf-8")
    return prefix + bytes([len(origin_bytes)]) + origin_bytes


def _fragment(url: str) -> str:
    return url.rsplit("#", 1)[1]


def test_relay_pair_link_reference_vector_well_known() -> None:
    expected = _expected_relay_blob(None)

    url = encode_relay_pair_link(
        INSTANCE_ID,
        TOTP,
        NONCE,
        CA_FP_SPKI,
        relay_origin=None,
    )

    assert len(expected) == 54
    assert url == WELL_KNOWN_URL
    assert crockford_decode(_fragment(url)) == expected


def test_relay_pair_link_reference_vector_custom_origin() -> None:
    relay_origin = "https://relay.example"
    expected = _expected_relay_blob(relay_origin)

    url = encode_relay_pair_link(
        INSTANCE_ID,
        TOTP,
        NONCE,
        CA_FP_SPKI,
        relay_origin=relay_origin,
    )

    assert len(expected) == 75
    assert expected.endswith(bytes([21]) + b"https://relay.example")
    assert url == CUSTOM_URL
    assert crockford_decode(_fragment(url)) == expected


@pytest.mark.parametrize(
    ("now", "expected"),
    [
        (59, "287082"),
        (1111111109, "081804"),
        (1234567890, "005924"),
    ],
)
def test_totp_matches_rfc6238_sha1_vectors(now: int, expected: str) -> None:
    secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"

    assert f"{compute_current_totp(secret, now):06d}" == expected


@pytest.mark.parametrize(
    ("kwargs", "match"),
    [
        ({"totp": 1_000_000}, "totp"),
        ({"totp": -1}, "totp"),
        ({"nonce": "00" * 15}, "nonce"),
        ({"relay_origin": "a" * 256}, "relay_origin"),
        ({"instance_id": "not-a-uuid"}, "badly formed hexadecimal UUID string"),
    ],
)
def test_relay_pair_link_validates_inputs(
    kwargs: dict[str, object],
    match: str,
) -> None:
    params: dict[str, object] = {
        "instance_id": INSTANCE_ID,
        "totp": TOTP,
        "nonce": NONCE,
        "ca_fp_spki": CA_FP_SPKI,
        "relay_origin": None,
    }
    params.update(kwargs)

    with pytest.raises(ValueError, match=re.escape(match)):
        encode_relay_pair_link(**params)  # type: ignore[arg-type]


def test_version_byte_disambiguates_direct_and_relay_forms() -> None:
    relay_blob = crockford_decode(_fragment(WELL_KNOWN_URL))
    direct_url = _build_pair_link(
        "192.0.2.42",
        7070,
        "a1b2c3d4e5f607181122334455667788",
        "deadbeefcafebabe0123456789abcdef",
    )
    direct_blob = crockford_decode(_fragment(direct_url))

    assert len(relay_blob) == 54
    assert relay_blob[0] == 0x03
    assert not (len(relay_blob) == 40 and relay_blob[0] == 0x04)

    assert len(direct_blob) == 40
    assert direct_blob[0] == 0x04
    assert not (len(direct_blob) >= 54 and direct_blob[0] == 0x03)


def test_relay_origin_selector_defines_blob_end() -> None:
    well_known = crockford_decode(_fragment(WELL_KNOWN_URL))
    custom = crockford_decode(_fragment(CUSTOM_URL))

    assert len(well_known) == 54
    assert well_known[53] == 0x00

    assert len(custom) == 54 + 21
    assert custom[53] == 21
    assert custom[54:] == b"https://relay.example"
