# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Presentation-neutral service handoff outcome taxonomy."""

from __future__ import annotations

import logging
from dataclasses import dataclass

log = logging.getLogger(__name__)

APPROVED = "approved"
PENDING = "pending"
REVOKED = "revoked"
EXPIRED = "expired"
MALFORMED = "malformed"
NETWORK_ERROR = "network_error"
LOCAL_ERROR = "local_error"
NEEDS_SUBSCRIPTION = "needs_subscription"
RELAY_IDENTITY_CONFLICT = "relay_identity_conflict"
RELAY_ROTATION_UNSUPPORTED = "relay_rotation_unsupported"
RELAY_UNAVAILABLE = "relay_unavailable"
RELAY_REJECTED = "relay_rejected"

SPL_PRIVATE_LINK_ALREADY_ENABLED_DETAIL = "your private network is already on"
SPL_PRIVATE_LINK_CONSENT_LINK_PREPARE_FAILED_DETAIL = "couldn't prepare the consent link"

# Build-facing, literal-only subset for convey-shell's constrained parser.
SPL_OUTCOME_GUIDANCE = {
    "approved": None,
    "pending": "Keep the approval page open while the request finishes.",
    "revoked": "Consent was not granted. Start a new enable flow when ready.",
    "expired": "This enable link is no longer active. Start a new enable flow.",
    "malformed": "The service response was not understood. Update solstone and try again.",
    "network_error": "The service could not be reached. Check network access and try again.",
    "local_error": "Local service state could not be written. Check journal permissions and try again.",
    "needs_subscription": "private network needs an active subscription before it can turn on. your consent is saved; set one up, then enable private network again.",
    "relay_identity_conflict": "this solstone is already set up under a different identity. reach out to support to reset it, then try again.",
    "relay_rotation_unsupported": "this solstone's security key changed and can't be re-registered automatically yet. reach out to support.",
    "relay_unavailable": "the private network service isn't available right now. try again in a bit.",
    "relay_rejected": "the relay couldn't finish setting up your private network (error {code}).",
}

# Verbatim error-body `error` strings from the external relay
# (github.com/solpbc/spl, enroll.ts). Matched exactly; anything else -> RELAY_REJECTED.
RELAY_REASON_ALREADY_REGISTERED = "ca_pubkey already registered to another instance"
RELAY_REASON_CA_MISMATCH = "ca_pubkey mismatch — rotation not supported in v1"

# D guidance is status-parameterized, so it is a template formatted at build time.
_RELAY_REJECTED_GUIDANCE = (
    "the relay couldn't finish setting up your private network (error {code})."
)

CODES = frozenset(
    {
        APPROVED,
        PENDING,
        REVOKED,
        EXPIRED,
        MALFORMED,
        NETWORK_ERROR,
        LOCAL_ERROR,
        NEEDS_SUBSCRIPTION,
        RELAY_IDENTITY_CONFLICT,
        RELAY_ROTATION_UNSUPPORTED,
        RELAY_UNAVAILABLE,
        RELAY_REJECTED,
    }
)

GUIDANCE: dict[str, str | None] = {
    APPROVED: None,
    PENDING: "Keep the approval page open while the request finishes.",
    REVOKED: "Consent was not granted. Start a new enable flow when ready.",
    EXPIRED: "This enable link is no longer active. Start a new enable flow.",
    MALFORMED: (
        "The service response was not understood. Update solstone and try again."
    ),
    NETWORK_ERROR: (
        "The service could not be reached. Check network access and try again."
    ),
    LOCAL_ERROR: (
        "Local service state could not be written. "
        "Check journal permissions and try again."
    ),
    NEEDS_SUBSCRIPTION: (
        "private network needs an active subscription before it can turn on. "
        "your consent is saved; set one up, then enable private network again."
    ),
    RELAY_IDENTITY_CONFLICT: (
        "this solstone is already set up under a different identity. "
        "reach out to support to reset it, then try again."
    ),
    RELAY_ROTATION_UNSUPPORTED: (
        "this solstone's security key changed and can't be re-registered "
        "automatically yet. reach out to support."
    ),
    RELAY_UNAVAILABLE: (
        "the private network service isn't available right now. try again in a bit."
    ),
    RELAY_REJECTED: _RELAY_REJECTED_GUIDANCE,
}

TOKEN_TO_CODE: dict[str, str] = {
    "consent_link_expired": EXPIRED,
    "consent_timeout": EXPIRED,
    "nonce_invalid": MALFORMED,
    "unexpected_payload": MALFORMED,
    "portal_unreachable": NETWORK_ERROR,
    "tls_verification_failed": NETWORK_ERROR,
    "relay_unreachable": NETWORK_ERROR,
    "write_failed": LOCAL_ERROR,
    "journal_not_initialized": LOCAL_ERROR,
}

OUT_OF_DOMAIN_TOKENS = frozenset(
    {
        "already_enabled",
        "manual_key_present",
        "already_disabled",
        "spl_already_enabled",
        "spl_already_disabled",
        "unknown_service",
    }
)


@dataclass(frozen=True)
class HandoffOutcome:
    code: str
    guidance: str | None
    detail: str | None = None


def outcome_for_code(code: str, *, detail: str | None = None) -> HandoffOutcome:
    if code not in GUIDANCE:
        raise ValueError(f"unsupported handoff outcome code: {code!r}")
    return HandoffOutcome(code=code, guidance=GUIDANCE[code], detail=detail)


def relay_rejection_outcome(*, status: int, reason: str | None) -> HandoffOutcome:
    """Map a relay HTTP rejection to a cause-specific owner-facing outcome.

    The raw status + reason are carried verbatim on `detail` (operator/log facing)
    so the two distinct 409s are tellable apart from `detail` alone.
    """
    detail = f"relay rejected enroll: status={status} reason={reason}"
    if status == 409 and reason == RELAY_REASON_ALREADY_REGISTERED:
        return HandoffOutcome(
            RELAY_IDENTITY_CONFLICT, GUIDANCE[RELAY_IDENTITY_CONFLICT], detail
        )
    if status == 409 and reason == RELAY_REASON_CA_MISMATCH:
        return HandoffOutcome(
            RELAY_ROTATION_UNSUPPORTED, GUIDANCE[RELAY_ROTATION_UNSUPPORTED], detail
        )
    if status == 503:
        return HandoffOutcome(RELAY_UNAVAILABLE, GUIDANCE[RELAY_UNAVAILABLE], detail)
    return HandoffOutcome(
        RELAY_REJECTED, _RELAY_REJECTED_GUIDANCE.format(code=status), detail
    )


def outcome_from_token(token: str, *, detail: str | None = None) -> HandoffOutcome:
    if token in OUT_OF_DOMAIN_TOKENS:
        raise ValueError(f"token is not a handoff outcome: {token!r}")
    code = TOKEN_TO_CODE.get(token)
    if code is None:
        log.error("unmapped handoff outcome token: %s", token)
        code = LOCAL_ERROR
        detail = detail or token
    return outcome_for_code(code, detail=detail)
