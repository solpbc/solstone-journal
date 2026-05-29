# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Relay-form pair-link encoding for spl connectivity posture."""

from __future__ import annotations

import base64
import hashlib
import hmac
import uuid

from solstone.apps.link.copy import PAIR_LINK_HOST, PAIR_LINK_PATH
from solstone.apps.link.crockford32 import encode as crockford_encode
from solstone.think.link.ca import RELAY_NONCE_BYTES

RELAY_VERSION = 0x03
CA_FP_TAG_SPKI_SHA256 = 0x01
TOTP_STEP_SECONDS = 30
TOTP_DIGITS = 6


def compute_current_totp(secret_b32: str, now: int) -> int:
    padded = secret_b32 + "=" * ((8 - len(secret_b32) % 8) % 8)
    secret = base64.b32decode(padded)
    counter = now // TOTP_STEP_SECONDS
    message = counter.to_bytes(8, "big")
    mac = hmac.new(secret, message, hashlib.sha1).digest()
    offset = mac[19] & 0x0F
    value = (
        ((mac[offset] & 0x7F) << 24)
        | (mac[offset + 1] << 16)
        | (mac[offset + 2] << 8)
        | mac[offset + 3]
    )
    return value % (10**TOTP_DIGITS)


def encode_relay_pair_link(
    instance_id: str,
    totp: int,
    nonce: str,
    ca_fp_spki: str,
    *,
    relay_origin: str | None,
) -> str:
    if not 0 <= totp <= 999999:
        raise ValueError("totp must be in range 0..999999")

    instance_bytes = uuid.UUID(instance_id).bytes
    nonce_bytes = bytes.fromhex(nonce)
    if len(nonce_bytes) != RELAY_NONCE_BYTES:
        raise ValueError(f"nonce must be {RELAY_NONCE_BYTES} bytes")

    ca_fp_bytes = bytes.fromhex(ca_fp_spki)
    if len(ca_fp_bytes) < 16:
        raise ValueError("ca_fp_spki must contain at least 16 bytes")

    if relay_origin is None:
        relay_origin_bytes = b"\x00"
    else:
        origin_bytes = relay_origin.encode("utf-8")
        if not origin_bytes:
            raise ValueError("relay_origin must not be empty")
        if len(origin_bytes) > 255:
            raise ValueError("relay_origin must be 255 bytes or fewer")
        relay_origin_bytes = bytes([len(origin_bytes)]) + origin_bytes

    blob = (
        bytes([RELAY_VERSION])
        + instance_bytes
        + totp.to_bytes(3, "big")
        + nonce_bytes
        + bytes([CA_FP_TAG_SPKI_SHA256])
        + ca_fp_bytes[:16]
        + relay_origin_bytes
    )
    return f"https://{PAIR_LINK_HOST}{PAIR_LINK_PATH}#{crockford_encode(blob)}"
