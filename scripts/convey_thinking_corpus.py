#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the thinking app surface, against configured journals.

The thinking surface is where an owner chooses how sol thinks: a model on this
computer, confidential processing operated by sol pbc, or their own engine. Its
payload is therefore almost entirely a *projection of journal config plus the
durable brain record* -- which means an empty journal proves close to nothing,
and a single phase proves less than that. The reference is driven over five
journal states, because the same route returns materially different owner-facing
words in each:

  * ``unestablished``   -- no config at all; the session gate's first-run branch
  * ``corrupt``         -- config exists and cannot be parsed; the gate's THIRD
                           outcome, which a port written ``unwrap_or(false)``
                           collapses into the first and thereby tells an owner
                           with a real journal that they never set one up
  * ``none``            -- established, no thinking engine chosen
  * ``bundled_local``   -- the bundled on-device model selected
  * ``byo_cloud``       -- a cloud key saved, a model remembered, validation cached
  * ``byo_endpoint``    -- the owner's own OpenAI-compatible endpoint configured
  * ``confidential``    -- confidential processing provisioned; the ONLY phase that
                           reaches the attestation view's verified branch

🔴 The durable brain record in each configured phase is written by **the
reference's own writer**, not hand-assembled here. A hand-written record is
rejected as ``brain_record_invalid`` -- the record carries a fingerprint keyed to
the journal -- and a corpus seeded that way silently pins the *invalid-record*
branch of every route while looking fully populated. That was the first version
of this file, and it is the shape of every "green because something was broken"
assertion in this tree.

⚠ This corpus has a clock. Regenerating it requires a runnable reference tree and
the conversion deletes that tree. It is a frozen record, not a live comparison.

⛔ Every journal is built in a temporary directory by this generator. No value
here is read from any real journal, and nothing here may be pointed at one.

⛔ **Probes never leave the machine.** ``/api/keys/check``, ``/api/validate-keys``,
``/api/validate-model`` and ``/api/confidential/*`` reach a provider or spawn a
process on their success paths, so only their *refusal* paths are probed -- which
is the half a port actually gets wrong, and the half that is reproducible.

Determinism: ``TZ`` is pinned to UTC before any solstone import.

⛔ **The brain record's instants and the ages derived from them are NOT pinned by
this corpus, and an earlier draft of this docstring claimed they were.** Seeding
on a pinned clock was tried and does not work: the record's ``updated_at`` is
stamped by the native writer in a *separate process*, which no in-process clock
patch reaches, so a frozen reader sees a record from its own future and the
reference bakes ``brain_record_stale`` into the record itself -- every phase then
pins the stale branch while looking fully populated. The seed therefore runs on
the real clock and seven instants are normalized to ``<CAPTURE_CLOCK>``.
🔴 **So duration arithmetic is ungraded here and needs its own unit test.** Saying
otherwise is the exact failure this corpus exists to prevent, one layer up.

🔴 Normalization is a PATH ALLOWLIST, never a shape test, and every normalized
field is named per case in ``normalized_fields``. Host-dependent fields (physical
memory, platform support) are normalized by path and the capture host is recorded
in the corpus metadata, so a reader can tell a portable pin from a local one.

Usage:
    python scripts/convey_thinking_corpus.py            # write the corpus
    python scripts/convey_thinking_corpus.py --check    # fail if it would change
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

# Pinned before any solstone import so module-level datetime work sees it too.
os.environ["TZ"] = "UTC"
if hasattr(time, "tzset"):
    time.tzset()

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_thinking_corpus.json"

# A fixed instant so `setup.completed_at` is reproducible across regenerations.
# 2026-01-01T00:00:00Z -- before any journal this corpus will ever describe.
PINNED_COMPLETED_AT = 1767225600
# The seeded brain record's observed instant. Deliberately NOT normalized: the
# recorded `age_text` is then a real assertion about the reference's own
# duration arithmetic rather than a placeholder that matches anything.
PINNED_BRAIN_OBSERVED_AT = "2026-01-01T00:00:00+00:00"
PINNED_KEY_VALIDATED_AT = "2026-01-01T00:00:00+00:00"
# The seeded evidence TTL. Fixed so `expires_at` is pinned rather than
# derived from the capture instant.
PINNED_BRAIN_EXPIRES_AT = "2026-01-02T00:00:00+00:00"
# The canonical bundled local model id, read from the reference so the
# corpus cannot drift from the pin it is describing.
LOCAL_MODEL_ID = "local/qwen3.5-4b"
# The confidential phase's endpoint identity. Not a real host: `.invalid` is
# reserved by RFC 2606 and can never resolve, so no probe can leave this machine.
CONFIDENTIAL_ENDPOINT_URL = "https://spp.example.invalid/v1"
CONFIDENTIAL_SERVED_MODEL = "served-model"
CONFIDENTIAL_CREDENTIAL = "corpus-confidential-credential"


def _sha256_hex(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()

PLACEHOLDER_ROOT = "<JOURNAL_ROOT>"
PLACEHOLDER_VERSION = "<VERSION>"
PLACEHOLDER_HOST = "<HOST_DEPENDENT>"
PLACEHOLDER_TIME = "<CAPTURE_CLOCK>"
VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+")

# 🔴 Every entry is a FIELD PATH, never a value shape. Two classes live here and
# they are not the same claim:
#   * `<VERSION>`      -- reproducible on any host, just not across releases
#   * `<HOST_DEPENDENT>` -- a property of the machine that captured this corpus.
#     A port matching these has proved nothing about them; a port matching
#     everything else has proved everything else.
#
# ⛔ This set is deliberately NARROW. `binary_present`, `model_present` and
# `provider_status.local.configured` read install artifacts under the *journal*
# (`solstone-core install inspect local --journal <root>`), so on a temporary
# journal they are reproducible `false` -- normalizing them would erase four of
# the ten fields this route exists to serve. Only genuine properties of the
# machine are listed: CPU architecture / OS (`platform_supported`,
# `local_backend`), physical memory, and the two values derived from them.
HOST_DEPENDENT_FIELDS: dict[str, set[str]] = {
    "/app/thinking/api/local/availability": {
        "platform_supported",
        "total_memory_gb",
        "available_memory_gb",
        "warning",
        # `available` and `reason` are `platform_supported`-gated: on an
        # unsupported host they short-circuit before the artifact checks.
        "available",
        "reason",
    },
    "/app/thinking/api/providers": {"local_backend"},
}
VERSION_FIELDS: dict[str, set[str]] = {}

# 🔴 The consequence of seeding on the real clock: the record's instants and the
# ages derived from them cannot be reproduced. They are normalized BY PATH and
# named, and nothing else is -- `state`, `headline`, `reason_code`, `action`,
# `identity`, every component `status`, the spp readiness block and the
# attestation `state` all stay pinned, which is the whole state machine.
# ⛔ Do not widen this to `brain.components.*` wholesale: that would take
# `status` and `reason_text` with it.
_BRAIN_TIME_FIELDS = {
    "brain.evidence.observed_at",
    "brain.evidence.age_seconds",
    "brain.evidence.age_text",
    "brain.components.generate.observed_at",
    "brain.components.cogitate.observed_at",
    "active_lane.confidential_attestation.observed_at",
    "active_lane.confidential_attestation.expires_at",
}
TIME_DEPENDENT_FIELDS: dict[str, set[str]] = {
    "/app/thinking/api/providers": set(_BRAIN_TIME_FIELDS),
    "/app/thinking/api/state": {f"providers.{name}" for name in _BRAIN_TIME_FIELDS},
}

# (method, path, json body or None, why this probe is in the corpus)
Probe = tuple[str, str, "dict[str, Any] | None", str]

PROBES: list[Probe] = [
    # ---- the shell half -------------------------------------------------
    ("GET", "/app/thinking/", None, "the app index: must be the shell, byte-identical"),
    # ⚠ Flask redirects a missing trailing slash; axum does not, and a port that
    # inherits the router default answers the unconverted-app refusal instead. A
    # sibling lane shipped three silent narrowings of exactly this kind, one of
    # them a trailing slash the reference accepted.
    ("GET", "/app/thinking", None, "🔴 NO trailing slash -- the reference's own redirect behaviour"),
    ("GET", "/app/thinking/workspace", None, "the app fragment bytes the shell injects"),
    ("GET", "/app/thinking/static/thinking.js", None, "per-app static; the whole client"),
    # ---- the read surface ------------------------------------------------
    ("GET", "/app/thinking/api/state", None, "the boot payload: providers + keys + ALL owner copy"),
    ("GET", "/app/thinking/api/providers", None, "the surface's centre of gravity"),
    ("GET", "/app/thinking/api/providers?local_model=local/qwen3.5-4b", None, "explicit accepted local model"),
    ("GET", "/app/thinking/api/providers?local_model=nope", None, "🔴 unknown local model REFUSES; it does not degrade"),
    ("GET", "/app/thinking/api/providers?local_model=", None, "empty local_model falls back to the default"),
    ("GET", "/app/thinking/api/keys", None, "key presence + cached validation, never the key"),
    ("GET", "/app/thinking/api/validate-keys", None, "GET is the non-persisting validation read"),
    ("GET", "/app/thinking/api/providers/local/status", None, "local readiness, the local row alone"),
    ("GET", "/app/thinking/api/local/availability", None, "host fit for the bundled model"),
    ("GET", "/app/thinking/api/local/availability?model=nope", None, "unknown model refusal on the availability route"),
    ("GET", "/app/thinking/api/local/bootstrap/status", None, "install state machine, read-only"),
    ("GET", "/app/thinking/api/local/bootstrap/status?model=nope", None, "unknown model refusal on the status route"),
    ("GET", "/app/thinking/api/local/models", None, "selectable local models for this backend"),
    ("GET", "/app/thinking/api/local/runtime", None, "local runtime recovery view"),
    ("GET", "/app/thinking/api/generators", None, "⚠ NO browser or CLI caller reaches this -- recorded so a deletion is a decision"),
    # ---- refusal paths of the write surface ------------------------------
    # None of these leaves the machine. Each is a branch a port silently widens.
    ("PUT", "/app/thinking/api/keys", None, "missing body is missing_request_body, not a 500"),
    ("PUT", "/app/thinking/api/keys", {"env_var": "NOPE_API_KEY", "value": "x"}, "unmanaged env var names the allowlist"),
    ("PUT", "/app/thinking/api/keys", {"env_var": "OPENAI_API_KEY", "value": 7}, "non-string value is invalid_request_value"),
    ("POST", "/app/thinking/api/keys/check", None, "missing body"),
    ("POST", "/app/thinking/api/keys/check", {"env_var": "OPENAI_API_KEY", "value": ""}, "empty candidate refuses before any network call"),
    ("POST", "/app/thinking/api/keys/check", {"env_var": "bogus", "value": "x"}, "unmanaged env var"),
    ("POST", "/app/thinking/api/validate-model", None, "missing body"),
    ("POST", "/app/thinking/api/validate-model", {"provider": "local", "model": "m"}, "🔴 local is NOT a cloud BYO provider here"),
    ("POST", "/app/thinking/api/validate-model", {"provider": "openai"}, "missing model"),
    ("POST", "/app/thinking/api/validate-model", {"provider": "openai", "model": "  "}, "blank model"),
    ("POST", "/app/thinking/api/providers", None, "missing body"),
    ("POST", "/app/thinking/api/providers", {"provider": "openai"}, "lane is required"),
    ("POST", "/app/thinking/api/providers", {"lane": "nope"}, "unknown lane names the closed set"),
    ("POST", "/app/thinking/api/providers", {"lane": "byo", "provider": "nope"}, "unknown BYO provider"),
    ("POST", "/app/thinking/api/providers", {"lane": "byo"}, "BYO with no provider"),
    ("POST", "/app/thinking/api/providers", {"lane": "byo", "provider": "openai", "bogus": 1}, "unknown field is refused, not ignored"),
    ("POST", "/app/thinking/api/providers", {"lane": "local", "model": "m"}, "model is cloud-BYO only"),
    ("POST", "/app/thinking/api/providers", {"lane": "confidential"}, "confidential must go through the enable flow"),
    ("POST", "/app/thinking/api/providers", {"lane": "byo", "provider": "google", "model": "m", "google_model_resolution_targets": ["nope"]}, "unknown alias target"),
    ("POST", "/app/thinking/api/providers", {"lane": "byo", "provider": "openai", "model": "m", "google_model_resolution_targets": ["confidential_prior"]}, "alias targets are Google-only"),
    ("POST", "/app/thinking/api/local/endpoint", None, "missing body"),
    ("POST", "/app/thinking/api/local/endpoint", {"served_model_id": "m"}, "endpoint_url is required"),
    ("POST", "/app/thinking/api/local/endpoint", {"endpoint_url": "ftp://x/v1", "served_model_id": "m"}, "scheme allowlist"),
    ("POST", "/app/thinking/api/local/endpoint", {"endpoint_url": "http:///v1", "served_model_id": "m"}, "a URL with no host"),
    ("POST", "/app/thinking/api/local/endpoint", {"endpoint_url": "http://h/v1"}, "served_model_id is required"),
    ("POST", "/app/thinking/api/local/endpoint", {"endpoint_url": "http://h/v1", "served_model_id": "m", "credential": 7}, "non-string credential"),
    ("POST", "/app/thinking/api/local/runtime/retry", None, "missing body"),
    ("POST", "/app/thinking/api/local/runtime/retry", {"health_revision": 1}, "🔴 the retry body must be EXACTLY the recovery state"),
    ("POST", "/app/thinking/api/local/runtime/retry", {"health_revision": True, "retry_revision": 0, "desired_fingerprint_sha256": "x"}, "⚠ bool is not an int here, and Python would say it is"),
    ("POST", "/app/thinking/api/local/runtime/retry", {"health_revision": -1, "retry_revision": 0, "desired_fingerprint_sha256": "x"}, "negative revision"),
    ("PUT", "/app/thinking/api/generators", None, "missing body"),
    ("PUT", "/app/thinking/api/generators", {"k": {"disabled": "yes"}}, "disabled must be boolean"),
    ("PUT", "/app/thinking/api/generators", {"k": {"extract": 1}}, "extract must be boolean"),
]


def _normalize(
    value: Any,
    found: dict[str, str],
    host_fields: set[str],
    version_fields: set[str],
    time_fields: set[str],
    path: str = "",
) -> Any:
    """Replace allowlisted scalars, recording each replacement and its reason.

    ⛔ A field absent from both allowlists is returned verbatim however volatile
    it looks. Widening either set is a decision, not a convenience.
    """
    if isinstance(value, dict):
        return {
            key: _normalize(
                item,
                found,
                host_fields,
                version_fields,
                time_fields,
                f"{path}.{key}" if path else key,
            )
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [
            _normalize(item, found, host_fields, version_fields, time_fields, f"{path}[]")
            for item in value
        ]
    if path in host_fields:
        found[path] = "host"
        return PLACEHOLDER_HOST
    if path in time_fields:
        found[path] = "capture-clock"
        return PLACEHOLDER_TIME
    if path in version_fields and isinstance(value, str) and VERSION_PATTERN.match(value):
        found[path] = "version"
        return PLACEHOLDER_VERSION
    return value


def _seed_brain_record(root: Path, *, ok: bool = True) -> None:
    """Write a durable brain record THROUGH THE REFERENCE'S OWN WRITER.

    ⛔ Never hand-assemble this file. The record is validated against a
    fingerprint keyed to the journal, so a hand-written one inspects as
    ``brain_record_invalid`` and every route then reports the *unknown* branch
    while the corpus looks fully populated.

    The instants are pinned to `PINNED_BRAIN_OBSERVED_AT`, so `age_seconds` and
    `age_text` in the recorded payloads are the reference's own duration
    arithmetic over a fixed interval -- asserted, not normalized away.
    """
    from datetime import datetime, timedelta, timezone

    from solstone.think.providers.brain_state import (
        begin_brain_refresh,
        finish_brain_refresh,
    )

    # 🔴 The REAL clock, deliberately. Freezing it does not work here and the
    # reason is worth keeping: the record's `updated_at` is stamped by the native
    # writer in a SEPARATE PROCESS, which no in-process clock patch reaches, so a
    # frozen reader sees a record from its own future and the reference bakes
    # `brain_record_stale` into the record itself. Every phase would then pin the
    # stale branch while looking fully populated -- the same failure as a
    # hand-written record, one layer down. Measured, not assumed.
    observed = datetime.now(timezone.utc)
    expires = observed + timedelta(days=1)

    def component(status: str, reason_code: str | None = None) -> dict[str, Any]:
        body: dict[str, Any] = {
            "status": status,
            "observed_at": observed.isoformat(),
            "expires_at": expires.isoformat(),
        }
        if reason_code is not None:
            body["reason_code"] = reason_code
        return body

    permit = begin_brain_refresh(observed, journal_path=root)
    if permit is None:
        raise RuntimeError(
            "the reference refused to begin a brain refresh; the corpus cannot "
            "seed a valid record and must not record the invalid-record branch "
            "as if it were the configured one"
        )
    good = component("ok")
    outcome = {
        "configuration": good,
        "lane_prerequisites": dict(good),
        "generate": dict(good) if ok else component("failed", "provider_key_invalid"),
        "cogitate": dict(good) if ok else component("failed", "provider_key_invalid"),
    }
    finish_brain_refresh(permit, outcome, observed, journal_path=root)


def _write_config(root: Path, config: dict[str, Any] | str) -> None:
    (root / "config").mkdir(parents=True, exist_ok=True)
    target = root / "config" / "journal.json"
    if isinstance(config, str):
        target.write_text(config)
        return
    target.write_text(json.dumps(config, indent=2, sort_keys=True) + "\n")


def _established(**providers: Any) -> dict[str, Any]:
    body: dict[str, Any] = {"setup": {"completed_at": PINNED_COMPLETED_AT}}
    if providers:
        body.update(providers)
    return body


def _build_journal(root: Path, phase: str) -> None:
    if phase == "unestablished":
        return
    if phase == "corrupt":
        _write_config(root, '{"setup": {"completed_at": 17672256')
        return
    if phase == "none":
        # ⛔ No brain record: "no engine chosen" has nothing to have probed, and
        # seeding one here would pin a state the reference cannot reach.
        _write_config(root, _established())
        return
    if phase == "bundled_local":
        _write_config(
            root,
            _established(providers={"active": {"provider": "local", "model": LOCAL_MODEL_ID}}),
        )
        _seed_brain_record(root)
        return
    if phase == "byo_cloud":
        _write_config(
            root,
            _established(
                env={"OPENAI_API_KEY": "sk-corpus-not-a-real-key"},
                providers={
                    "active": {"provider": "openai", "model": "gpt-5"},
                    "byo_models": {"openai": "gpt-5"},
                    "key_validation": {
                        "openai": {"valid": True, "timestamp": PINNED_KEY_VALIDATED_AT}
                    },
                },
            ),
        )
        _seed_brain_record(root)
        return
    if phase == "byo_endpoint":
        _write_config(
            root,
            _established(
                providers={
                    "active": {"provider": "local", "model": "served-model"},
                    # ⚠ Port 1 on loopback, deliberately: the reference PROBES a
                    # BYO endpoint to decide `local_endpoint_unreachable`, so the
                    # captured value depends on what answers. A plausible port
                    # (8000) makes the corpus depend on what happens to be
                    # running on the capture host; port 1 is refused instantly,
                    # needs no DNS, and never leaves the machine.
                    "local": {
                        "endpoint_url": "http://127.0.0.1:1/v1",
                        "served_model_id": "served-model",
                        "credential": "corpus-credential",
                    },
                },
            ),
        )
        _seed_brain_record(root)
        return
    if phase == "confidential_inactive":
        # Confidential is PROVISIONED but is not the active lane, which is the
        # only way to reach the attestation view's `inactive` branch. The active
        # provider is a cloud one, so `derive_active_brain_lane` answers
        # `byo-cloud` while `spp_configured` stays true.
        _write_config(
            root,
            _established(
                env={"OPENAI_API_KEY": "sk-corpus-not-a-real-key"},
                providers={
                    "active": {"provider": "openai", "model": "gpt-5"},
                    "local": {
                        "endpoint_url": CONFIDENTIAL_ENDPOINT_URL,
                        "served_model_id": CONFIDENTIAL_SERVED_MODEL,
                        "credential": CONFIDENTIAL_CREDENTIAL,
                    },
                },
                services={
                    "confidential": {
                        "endpoint_url": CONFIDENTIAL_ENDPOINT_URL,
                        "served_model_id": CONFIDENTIAL_SERVED_MODEL,
                        "credential_fingerprint_sha256": _sha256_hex(
                            CONFIDENTIAL_CREDENTIAL
                        ),
                        "prior_active": {"provider": "openai", "model": "gpt-5"},
                    }
                },
            ),
        )
        _seed_brain_record(root)
        return
    if phase == "confidential":
        # The only phase that reaches the attestation view's non-`off` branches.
        # `services.confidential` + `providers.local.credential` is what
        # `is_confidential_enabled` and `confidential_provenance_block` read.
        _write_config(
            root,
            _established(
                providers={
                    "active": {"provider": "local", "model": "served-model"},
                    "local": {
                        "endpoint_url": CONFIDENTIAL_ENDPOINT_URL,
                        "served_model_id": CONFIDENTIAL_SERVED_MODEL,
                        "credential": CONFIDENTIAL_CREDENTIAL,
                    },
                },
                services={
                    # 🔴 The lane resolves to `spp` only when the provenance
                    # block MATCHES the live endpoint: same normalized URL, same
                    # served model, and the sha256 of the same credential.
                    # A block that merely exists resolves the lane to `None`,
                    # which the reference reports as `configuration_invalid` --
                    # so a corpus seeded with a decorative block records the
                    # broken branch of the surface it exists to pin.
                    "confidential": {
                        "endpoint_url": CONFIDENTIAL_ENDPOINT_URL,
                        "served_model_id": CONFIDENTIAL_SERVED_MODEL,
                        "credential_fingerprint_sha256": _sha256_hex(
                            CONFIDENTIAL_CREDENTIAL
                        ),
                        "prior_active": {"provider": "openai", "model": "gpt-5"},
                    }
                },
            ),
        )
        _seed_brain_record(root)
        return
    raise ValueError(f"unknown phase: {phase}")


def _record(client: Any, probe: Probe, root: Path) -> dict[str, Any]:
    method, path, body, why = probe
    kwargs: dict[str, Any] = {"method": method}
    if body is not None:
        kwargs["json"] = body
    response = client.open(path, **kwargs)
    raw = response.get_data()
    normalized_body = raw.replace(str(root).encode(), PLACEHOLDER_ROOT.encode())
    content_type = response.headers.get("Content-Type", "")
    probe_key = path.split("?")[0]

    case: dict[str, Any] = {
        "method": method,
        "path": path,
        "why": why,
        "status": response.status_code,
        "content_type": content_type,
        "body_bytes": len(normalized_body),
        "body_sha256": hashlib.sha256(normalized_body).hexdigest(),
        "body_sha256_basis": "raw-body",
    }
    if body is not None:
        case["request_json"] = body
    if normalized_body != raw:
        case["body_normalized"] = [PLACEHOLDER_ROOT]
    location = response.headers.get("Location")
    if location:
        case["location"] = location

    if "json" in content_type:
        found: dict[str, str] = {}
        case["json"] = _normalize(
            json.loads(normalized_body),
            found,
            HOST_DEPENDENT_FIELDS.get(probe_key, set()),
            VERSION_FIELDS.get(probe_key, set()),
            TIME_DEPENDENT_FIELDS.get(probe_key, set()),
        )
        case["normalized_fields"] = {name: found[name] for name in sorted(found)}
        # 🔴 A raw-body hash is not reproducible for a case carrying a normalized
        # field. Hash what the corpus actually asserts.
        if found:
            case["body_sha256"] = hashlib.sha256(
                json.dumps(case["json"], sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest()
            case["body_sha256_basis"] = "normalized-json"
    elif response.status_code >= 400:
        case["body_text"] = normalized_body.decode("utf-8", errors="replace")
    return case


PHASES = (
    "unestablished",
    "corrupt",
    "none",
    "bundled_local",
    "byo_cloud",
    "byo_endpoint",
    "confidential_inactive",
    "confidential",
)


def build_corpus() -> dict[str, Any]:
    from solstone.convey import create_app

    cases: dict[str, list[dict[str, Any]]] = {}
    for phase in PHASES:
        with tempfile.TemporaryDirectory(prefix=f"convey-thinking-{phase}-") as tmp:
            root = Path(tmp)
            _build_journal(root, phase)
            os.environ["SOLSTONE_JOURNAL"] = str(root)
            os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
            app = create_app(str(root))
            client = app.test_client()
            cases[phase] = [_record(client, probe, root) for probe in PROBES]

    return {
        "schema": "solstone-convey-thinking-corpus-v1",
        "generator": "scripts/convey_thinking_corpus.py",
        "tz": "UTC",
        "pinned_completed_at": PINNED_COMPLETED_AT,
        "pinned_brain_observed_at": PINNED_BRAIN_OBSERVED_AT,
        "placeholders": {
            "journal_root": PLACEHOLDER_ROOT,
            "version": PLACEHOLDER_VERSION,
            "host_dependent": PLACEHOLDER_HOST,
            "capture_clock": PLACEHOLDER_TIME,
        },
        # A field normalized as `host` was captured on THIS machine. Recording
        # the machine is what lets a later reader tell "the corpus does not pin
        # this" from "the corpus pins this and the port disagrees".
        "capture_host": {
            "platform": sys.platform,
            "machine": platform.machine(),
        },
        # 🔴 Where the NATIVE surface deliberately differs. A checker reads this
        # and honours it; anything NOT listed here is a defect. ⛔ Declaring a
        # deviation only in prose means every independent check re-finds it.
        "native_deviations": [
            {
                "path": "/app/thinking/api/generators",
                "method": "PUT",
                "when": "the request carries no body",
                "reference": "500 settings_operation_failed -- the handler calls "
                "request.get_json() without silent=True, werkzeug raises "
                "UnsupportedMediaType, and the route's blanket except turns an "
                "unhandled exception into a generic failure",
                "native": "400 missing_request_body, the same typed refusal every "
                "other write route on this surface already answers",
                "why": "a raise is not a refusal. Every sibling route on this "
                "surface answers missing_request_body for the identical input, "
                "and the reference's own generator comments approve of that "
                "answer; reproducing the 500 would be preserving a defect, not "
                "fidelity. ⚠ This is the ONLY declared deviation -- anything "
                "else that differs from a recorded case is a defect.",
            }
        ],
        "phases": cases,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="exit non-zero if the corpus would change"
    )
    args = parser.parse_args()

    rendered = json.dumps(build_corpus(), indent=2, sort_keys=True) + "\n"

    if args.check:
        if not CORPUS_PATH.exists():
            print(f"missing corpus: {CORPUS_PATH}", file=sys.stderr)
            return 1
        if CORPUS_PATH.read_text() != rendered:
            print(
                f"thinking corpus is stale: {CORPUS_PATH}\n"
                "regenerate with: python scripts/convey_thinking_corpus.py",
                file=sys.stderr,
            )
            return 1
        print(f"thinking corpus is current: {CORPUS_PATH}")
        return 0

    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text(rendered)
    print(
        f"wrote {CORPUS_PATH} "
        f"({len(PROBES)} probes x {len(PHASES)} phases = {len(PROBES) * len(PHASES)} cases)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
