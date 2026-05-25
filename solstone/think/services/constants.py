# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared service-enable constants from solstone.app/account/src/enable-constants.js."""

from __future__ import annotations

import re

NONCE_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"  # RFC 4648 base32, no padding
NONCE_LENGTH_CHARS = 52
NONCE_REGEX = re.compile(r"^[A-Z2-7]{52}$")
