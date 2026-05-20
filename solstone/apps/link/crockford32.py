# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Crockford base32 encoding for link pair payloads."""

from __future__ import annotations

ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
_DECODE = {char: idx for idx, char in enumerate(ALPHABET)}
_ASCII_WHITESPACE = frozenset(" \t\n\r\v\f")


def encode(data: bytes) -> str:
    """Encode bytes as uppercase unpadded Crockford base32."""
    if not data:
        return ""

    out: list[str] = []
    buffer = 0
    bits = 0
    for byte in data:
        buffer = (buffer << 8) | byte
        bits += 8
        while bits >= 5:
            bits -= 5
            out.append(ALPHABET[(buffer >> bits) & 0x1F])

    if bits:
        out.append(ALPHABET[(buffer << (5 - bits)) & 0x1F])

    return "".join(out)


def decode(text: str) -> bytes:
    """Decode unpadded Crockford base32 text to bytes."""
    value = 0
    bits = 0
    for raw_char in text:
        char = _normalize_char(raw_char)
        if char is None:
            continue
        value = (value << 5) | _DECODE[char]
        bits += 5

    if bits == 0:
        return b""

    pad_bits = bits % 8
    if pad_bits:
        pad_mask = (1 << pad_bits) - 1 if pad_bits <= 4 else 0
        if pad_mask and value & pad_mask:
            raise ValueError("non-zero trailing pad bits")
        value >>= pad_bits

    byte_count = bits // 8
    return value.to_bytes(byte_count, "big")


def _normalize_char(char: str) -> str | None:
    if char == "-" or char in _ASCII_WHITESPACE:
        return None
    if char in {"I", "i", "L", "l"}:
        return "1"
    if char in {"O", "o"}:
        return "0"

    normalized = char.upper()
    if normalized not in _DECODE:
        raise ValueError(f"invalid Crockford base32 character: {char!r}")
    return normalized
