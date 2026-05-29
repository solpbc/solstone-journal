# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re

from solstone.think.services import cli

CANONICAL_TOKENS = {
    "consent_link_expired",
    "consent_timeout",
    "portal_unreachable",
    "tls_verification_failed",
    "nonce_invalid",
    "unexpected_payload",
    "write_failed",
    "already_enabled",
    "manual_key_present",
    "rate_limited",
    "already_disabled",
    "spl_already_enabled",
    "spl_already_disabled",
    "relay_unreachable",
    "journal_not_initialized",
    "unknown_service",
}


def test_error_messages_use_canonical_token_set() -> None:
    assert set(cli.ERROR_MESSAGES) == CANONICAL_TOKENS


def test_error_envelope_is_short_token_first_line(capsys) -> None:
    for token in sorted(CANONICAL_TOKENS):
        cli._print_error(token)

    lines = capsys.readouterr().err.splitlines()
    assert len(lines) == len(CANONICAL_TOKENS)
    for line in lines:
        assert re.match(r"^[a-z_]+:\s", line)
        token = line.split(":", 1)[0]
        assert token in CANONICAL_TOKENS
