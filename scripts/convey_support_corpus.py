#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Freeze the support reference surface against a loopback stub portal.

The support web surface is being rebuilt natively. Every acceptance criterion the
rebuild could write for itself would restate what the new code does, not what the
reference actually served. This module drives the reference Flask blueprint over
an authored probe table and records what came back.

🔴 **Driving this app is not a read.** Probing one of its routes once generated an
RSA-4096 keypair, fetched and SIGNED a live terms document, and POSTed a signup to
a production host — from a route that reads. The blueprint registers its
acknowledgement drain on **both** ``before_request`` and ``after_request``, so with
a non-empty pending-acknowledgement ledger *every* request here can reach the
portal, including ones that refuse. ⛔ The unit of analysis is the blueprint, not
the route.

So this generator establishes that egress is impossible rather than reasoning
about which routes reach out:

  * ``forbid_non_loopback_egress()`` runs before the reference is imported, and
    every phase asserts both that the guard fires on a real outbound call and
    that its blocked-destination log is otherwise **empty**. ⚠ Byte-identical
    output is not proof on its own: a route can attempt an outbound call, swallow
    the exception, and answer unchanged.
  * ``SOLSTONE_SUPPORT_URL`` — the reference's own published seam, which takes
    precedence over journal config and the default host — points at a stub this
    generator runs on ``127.0.0.1``.
  * ✅ Additionally intended to run inside a network namespace with only loopback
    (``bwrap --unshare-net --dev-bind / /``), so the in-process guard is a
    detector rather than the only barrier.

**The stub does not verify DPoP or the access token.** Verifying would make it a
second implementation of the thing under test. It records what it received; the
request log is the *only* oracle for the drain, because both drain layers swallow
every exception.

**What a green replay of this corpus is not evidence about** is stated inside the
fixture, computed from the recorded cases rather than written down.

Determinism: ``TZ`` is pinned to UTC before any reference import. Values the
reference reads from the host rather than from the seeded journal are **authored**
where the reference publishes a seam (the portal URL, the agent handle) and
**normalized by explicit field path** where it does not (the package version, the
source revision, the kernel string). ⛔ Normalization here is a path allowlist,
never a shape test: an oracle that erases the values it exists to pin reports
green for a port that got them wrong.

Usage:
    python scripts/convey_support_corpus.py            # write the corpus
    python scripts/convey_support_corpus.py --check    # fail if it would change
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

from corpus_scrub import (  # noqa: E402
    assert_egress_guard_can_see,
    assert_guard_can_see,
    assert_no_egress_attempted,
    assert_publishable,
    forbid_non_loopback_egress,
)

forbid_non_loopback_egress()
os.environ["TZ"] = "UTC"
if hasattr(time, "tzset"):
    time.tzset()

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_support_corpus.json"
CAPTURE_ROOT = Path("/var/tmp/solstone-convey-support-corpus")

PHASES = ("unestablished", "corrupt", "established", "disabled", "unregistered")

HEADER_ALLOWLIST = ("Content-Type", "Location")
HOST_HEADER = "127.0.0.1"

# Authored, not observed. `PortalClient.handle` otherwise derives from
# socket.gethostname(), which is a fact about the capture machine and would reach
# a public fixture through /api/register's response body.
AUTHORED_HOSTNAME = "corpus-host"
AUTHORED_HANDLE = "solstone-corpus-host"
# A fixed instant so `setup.completed_at` is reproducible across regenerations.
# 2026-01-01T00:00:00Z, before any journal this corpus will ever describe.
PINNED_COMPLETED_AT = 1767225600
# A caller-owned parent action id in the reference's required `sact1_` shape.
ACTION_KEY = "sact1_corpusfixedparentaction"
SEEDED_TICKET_ID = 7

PLACEHOLDER_ROOT = "<JOURNAL_ROOT>"
PLACEHOLDER_PORTAL = "<STUB_PORTAL>"


# --------------------------------------------------------------------------
# The probe table, authored here so the count is not self-certifying.
# --------------------------------------------------------------------------
#
# `app.url_map` reports exactly 22 addressable rules under /app/support at the
# captured revision: 18 decorated (9 GET, 9 POST — /api/tickets carries two
# decorators on one path), plus `/` and `/workspace` and `/background` injected by
# the app registry and `/static/<path:filename>` from the blueprint's static
# folder. ⛔ A probe table keyed to 21 has dropped a rule.

_JSON = {"Content-Type": "application/json"}


def _probe(name, method, path, *, kind="probe", body=None, key=True, files=None):
    """One authored case. `key` sends the Idempotency-Key header when true."""
    return {
        "name": name,
        "method": method,
        "path": path,
        "kind": kind,
        "body": body,
        "key": key,
        "files": files,
    }


# The 22 addressable endpoints, one canonical probe each, plus the routing-404
# case — which is discriminating in every phase because Flask's session gate is a
# `before_app_request` and a request that matches no rule never reaches it.
BASE_PROBES = (
    _probe("page_index", "GET", "/app/support/"),
    _probe("page_workspace", "GET", "/app/support/workspace"),
    _probe("page_background", "GET", "/app/support/background"),
    _probe("static_support_js", "GET", "/app/support/static/support.js"),
    _probe("api_config", "GET", "/app/support/api/config"),
    _probe("api_tickets_list", "GET", "/app/support/api/tickets"),
    _probe("api_tickets_list_status", "GET", "/app/support/api/tickets?status=open"),
    _probe("api_ticket_get", "GET", f"/app/support/api/tickets/{SEEDED_TICKET_ID}"),
    _probe("api_tickets_closed", "GET", "/app/support/api/tickets/closed"),
    _probe("api_articles", "GET", "/app/support/api/articles?q=backup"),
    _probe("api_article", "GET", "/app/support/api/articles/getting-started"),
    _probe("api_announcements", "GET", "/app/support/api/announcements"),
    _probe("api_diagnostics", "GET", "/app/support/api/diagnostics"),
    _probe("api_badge_count", "GET", "/app/support/api/badge-count"),
    _probe("routing_404_non_integer_ticket", "GET", "/app/support/api/tickets/notanint"),
    _probe(
        "api_draft",
        "POST",
        "/app/support/api/draft",
        kind="probe_repeat",
        body={"verb": "create", "payload": {"subject": "s", "description": "d"}},
        key=False,
    ),
    _probe("api_register", "POST", "/app/support/api/register", kind="probe_repeat", key=False),
    _probe(
        "api_ticket_create",
        "POST",
        "/app/support/api/tickets",
        kind="probe_repeat",
        body={
            "subject": "corpus subject",
            "description": "corpus description",
            "auto_context": False,
        },
    ),
    _probe(
        "api_ticket_reply",
        "POST",
        f"/app/support/api/tickets/{SEEDED_TICKET_ID}/reply",
        kind="probe_repeat",
        body={"content": "corpus reply"},
    ),
    _probe(
        "api_ticket_attachment",
        "POST",
        f"/app/support/api/tickets/{SEEDED_TICKET_ID}/attachments",
        kind="probe_repeat",
        files={"file": ("corpus.txt", b"corpus attachment bytes", "text/plain")},
        body={"index": "0"},
    ),
    _probe(
        "api_ticket_close",
        "POST",
        f"/app/support/api/tickets/{SEEDED_TICKET_ID}/close",
        kind="probe_repeat",
        body={},
    ),
    _probe(
        "api_resolution_confirm",
        "POST",
        f"/app/support/api/tickets/{SEEDED_TICKET_ID}/resolution/confirm",
        kind="probe_repeat",
        body={},
    ),
    _probe(
        "api_resolution_still_need_help",
        "POST",
        f"/app/support/api/tickets/{SEEDED_TICKET_ID}/resolution/still-need-help",
        kind="probe_repeat",
        body={},
    ),
    _probe(
        "api_feedback",
        "POST",
        "/app/support/api/feedback",
        kind="probe_repeat",
        body={"body": "corpus feedback"},
    ),
)

# 🔴 SEVEN of the nine writes require an Idempotency-Key, not nine. `api/draft`
# and `api/register` never reach `_action_id_or_error` and succeed without one, so
# a criterion saying "every mutation refuses a missing key" is unsatisfiable for
# two of them and tells an implementer to add a requirement the reference lacks.
NO_KEY_PROBES = tuple(
    _probe(f"{p['name']}_no_key", p["method"], p["path"], body=p["body"], key=False, files=p["files"])
    for p in BASE_PROBES
    if p["method"] == "POST" and p["key"]
)

VALIDATION_PROBES = (
    _probe(
        "api_draft_unknown_verb",
        "POST",
        "/app/support/api/draft",
        body={"verb": "not-a-verb", "payload": {}},
        key=False,
    ),
    _probe("api_draft_missing_payload", "POST", "/app/support/api/draft", body={"verb": "create"}, key=False),
    _probe(
        "api_ticket_create_missing_subject",
        "POST",
        "/app/support/api/tickets",
        body={"description": "d"},
    ),
    _probe(
        "api_feedback_missing_body",
        "POST",
        "/app/support/api/feedback",
        body={},
    ),
    _probe(
        "api_ticket_attachment_bad_suffix",
        "POST",
        f"/app/support/api/tickets/{SEEDED_TICKET_ID}/attachments",
        files={"file": ("corpus.exe", b"nope", "application/octet-stream")},
        body={"index": "0"},
    ),
    # `_record_path` rejects an action id that does not match ^sact1_[A-Za-z0-9_-]+$
    # before any stub contact. This is the deterministic local path to the
    # support_portal_failed wire code. ⛔ Not "a busy ledger", which needs a held
    # lock and is not deterministic.
    _probe(
        "api_ticket_create_malformed_key",
        "POST",
        "/app/support/api/tickets",
        body={"subject": "s", "description": "d", "auto_context": False},
        key="not-a-valid-action-id",
    ),
)

# 🔴 The drain reaches the portal on a request that REFUSES, and only the stub's
# request log can see it — both drain layers swallow every exception. Each of
# these seeds one completed, unacknowledged ledger record first, then issues a
# request the reference refuses locally.
#
# ⚠ In the `disabled` phase the seeding create is itself refused, so the seed runs
# with support temporarily enabled — which is also the only honest version of the
# scenario: an owner disables support while acknowledgements are still pending.
DRAIN_PROBES = (
    _probe("drain_on_missing_key_refusal", "POST", "/app/support/api/tickets", kind="drain", body={"subject": "s", "description": "d"}, key=False),
    _probe("drain_on_page_request", "GET", "/app/support/workspace", kind="drain"),
)
DISABLED_DRAIN_PROBES = (
    _probe("drain_on_feature_unavailable_refusal", "GET", "/app/support/api/badge-count", kind="drain"),
    _probe("drain_on_local_read", "GET", "/app/support/api/config", kind="drain"),
)


def probes_for(phase: str) -> tuple[dict[str, Any], ...]:
    if phase == "established":
        return BASE_PROBES + NO_KEY_PROBES + VALIDATION_PROBES + DRAIN_PROBES
    if phase == "disabled":
        return BASE_PROBES + DISABLED_DRAIN_PROBES
    return BASE_PROBES


# ⛔ Assert these, and assert the total. A number the implementation chooses is
# not a bound.
EXPECTED_PER_PHASE = {
    "unestablished": 24,
    "corrupt": 24,
    "established": 39,
    "disabled": 26,
    "unregistered": 24,
}
EXPECTED_TOTAL = 137


# --------------------------------------------------------------------------
# Normalization — a PATH ALLOWLIST, never a shape test.
# --------------------------------------------------------------------------
#
# Each entry is a dotted path within a recorded case. `*` matches one list index
# or one map key. A field absent from this table is recorded verbatim however
# volatile it looks; widening it is a decision, not a convenience.
NORMALIZED_PATHS = {
    # The installed package version. Rolls on every release.
    "response.body.version": "<VERSION>",
    # `git rev-parse --short HEAD` in the package dir. In a checkout this freezes
    # the capturing commit; in a wheel install it is genuinely None. Neither is a
    # contract, and the first is a fact about this machine.
    "response.body.revision": "<REVISION>",
    # The capture host's kernel string, architecture and interpreter version. The
    # SHAPE is the contract (four keys); the values are host identity, and
    # `_host_identifiers()` does not cover platform.release().
    "response.body.platform.release": "<PLATFORM_RELEASE>",
    "response.body.platform.machine": "<PLATFORM_MACHINE>",
    "response.body.platform.python": "<PLATFORM_PYTHON>",
    # Stamped by collect_recent_errors from a naive local now(), and the seed must
    # be inside the 168-hour window for the collector to return anything at all.
    "response.body.recent_errors.*.time": "<CAPTURE_TIME>",
    # uuid4 and a millisecond wall clock, minted per call by /api/draft.
    "response.body.draft_id": "<DRAFT_ID>",
    "repeat_response.body.draft_id": "<DRAFT_ID>",
}
NORMALIZED_PATHS.update(
    {key.replace("response.", "repeat_response.", 1): value for key, value in NORMALIZED_PATHS.items()}
)


def _matches(path: str, pattern: str) -> bool:
    parts, expected = path.split("."), pattern.split(".")
    return len(parts) == len(expected) and all(a == b or b == "*" for a, b in zip(parts, expected))


def _normalize(value: Any, *, path: str, root: str, portal: str) -> tuple[Any, list[str]]:
    for pattern, placeholder in NORMALIZED_PATHS.items():
        if _matches(path, pattern):
            return placeholder, [path]
    if isinstance(value, str):
        hits: list[str] = []
        out = value
        if root and root in out:
            out = out.replace(root, PLACEHOLDER_ROOT)
            hits.append(f"{path}#journal_root")
        if portal and portal in out:
            out = out.replace(portal, PLACEHOLDER_PORTAL)
            hits.append(f"{path}#portal_url")
        return out, hits
    if isinstance(value, dict):
        output: dict[str, Any] = {}
        hits = []
        for key in sorted(value):
            item, item_hits = _normalize(
                value[key], path=f"{path}.{key}", root=root, portal=portal
            )
            output[key] = item
            hits.extend(item_hits)
        return output, hits
    if isinstance(value, list):
        items = []
        hits = []
        for entry in value:
            item, item_hits = _normalize(entry, path=f"{path}.*", root=root, portal=portal)
            items.append(item)
            hits.extend(item_hits)
        return items, hits
    return value, []


# --------------------------------------------------------------------------
# The stub portal.
# --------------------------------------------------------------------------

STUB_TOS = "Corpus terms of service. Authored for this fixture; not a real agreement.\n"
STUB_ACCESS_TOKEN = "corpus.stub.access.token"


def _stub_app():
    """A minimal welcome-mat portal on loopback, with a request log.

    ⛔ It does not verify DPoP or the access token. Verifying would make the stub a
    second implementation of the thing under test, and the reference's own tests
    already cover the wire it produces.
    """
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

    log: list[dict[str, Any]] = []

    open_ticket = {
        "ticket_id": SEEDED_TICKET_ID,
        "status": "open",
        "subject": "a seeded open ticket",
        "created_at": "2026-02-01T00:00:00Z",
        "updated_at": "2026-02-02T00:00:00Z",
        "body": "a field an active ticket keeps",
    }
    closed_ticket = {
        "ticket_id": 8,
        "status": "closed",
        "closed_at": "2026-02-03T00:00:00Z",
        "close_scheduled_at": "2026-02-10T00:00:00Z",
        "reason_code": "resolved",
        # ⛔ Present so a port that returns what it received fails the tombstone
        # projection instead of passing it.
        "subject": "a field a tombstone must drop",
        "thread": [{"body": "a message a tombstone must drop"}],
    }

    routes: dict[tuple[str, str], tuple[int, Any]] = {
        ("GET", "/tos"): (200, STUB_TOS),
        ("POST", "/api/signup"): (200, {"access_token": STUB_ACCESS_TOKEN, "handle": AUTHORED_HANDLE}),
        ("POST", "/api/idempotency/ack"): (200, {"acknowledged": True}),
        ("GET", "/api/tickets"): (200, [open_ticket, closed_ticket]),
        ("GET", f"/api/tickets/{SEEDED_TICKET_ID}"): (200, open_ticket),
        ("GET", "/api/tickets/closed"): (200, {"tickets": [closed_ticket], "next_cursor": "corpus-cursor"}),
        ("GET", "/api/articles"): (200, [{"slug": "getting-started", "title": "Getting started"}]),
        ("GET", "/api/articles/getting-started"): (200, {"slug": "getting-started", "body": "an article"}),
        ("GET", "/api/announcements"): (200, [{"id": 1, "title": "an announcement"}]),
        ("POST", "/api/tickets"): (201, {"ticket_id": 101, "status": "open", "subject": "corpus subject"}),
        ("POST", f"/api/tickets/{SEEDED_TICKET_ID}/messages"): (201, {"message_id": 202, "status": "open"}),
        ("POST", f"/api/tickets/{SEEDED_TICKET_ID}/attachments"): (
            201,
            {"attachment_id": 303, "status": "open", "filename": "corpus.txt"},
        ),
        ("POST", f"/api/tickets/{SEEDED_TICKET_ID}/close"): (200, closed_ticket),
        ("POST", f"/api/tickets/{SEEDED_TICKET_ID}/resolution/confirm"): (200, closed_ticket),
        ("POST", f"/api/tickets/{SEEDED_TICKET_ID}/resolution/still-need-help"): (200, open_ticket),
    }

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):  # noqa: A003 - silence the default stderr log
            return

        def _dispatch(self, method: str) -> None:
            path = self.path.split("?", 1)[0]
            length = int(self.headers.get("Content-Length") or 0)
            if length:
                self.rfile.read(length)
            log.append(
                {
                    "method": method,
                    "path": path,
                    "had_idempotency_key": "Idempotency-Key" in self.headers,
                    "had_authorization": "Authorization" in self.headers,
                    "had_dpop": "DPoP" in self.headers,
                }
            )
            status, payload = routes.get((method, path), (404, {"error": "not_found"}))
            if isinstance(payload, str):
                body = payload.encode("utf-8")
                content_type = "text/plain; charset=utf-8"
            else:
                body = json.dumps(payload).encode("utf-8")
                content_type = "application/json"
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):  # noqa: N802 - BaseHTTPRequestHandler contract
            self._dispatch("GET")

        def do_POST(self):  # noqa: N802 - BaseHTTPRequestHandler contract
            self._dispatch("POST")

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    return server, log


# --------------------------------------------------------------------------
# Phase seeding.
# --------------------------------------------------------------------------

PORTAL_STORAGE = ("apps", "support", "portal")


def _seed_journal(root: Path, phase: str, capture_now: datetime) -> None:
    (root / "config").mkdir(parents=True, exist_ok=True)
    journal_config = root / "config" / "journal.json"
    if phase == "unestablished":
        return
    if phase == "corrupt":
        journal_config.write_text('{"setup": {"completed_at": 17672256')
        return
    journal_config.write_text(
        json.dumps({"setup": {"completed_at": PINNED_COMPLETED_AT}}, indent=2) + "\n"
    )
    # `is_enabled()` and `_get_portal_url_from_settings()` read config/config.json,
    # a DIFFERENT file from the gate's journal.json. Writing support.enabled into
    # journal.json captures a fully-populated ENABLED phase, green, with no signal.
    app_config = {
        "support": {"enabled": phase != "disabled"},
        # An authored, obviously-invalid literal under a secret-shaped key, so the
        # capture proves `_strip_secrets` fired rather than assuming it.
        "provider": {"api_key": "corpus-not-a-real-key-authored-for-this-fixture"},
        "observe": {"enabled": True},
    }
    (root / "config" / "config.json").write_text(json.dumps(app_config, indent=2) + "\n")

    health = root / "health"
    health.mkdir(parents=True, exist_ok=True)
    # collect_services: one pid that is alive by construction, one that cannot parse.
    (health / "observer.pid").write_text(f"{os.getpid()}\n")
    (health / "cortex.pid").write_text("not-a-pid\n")
    # collect_recent_errors filters on a 168-hour window anchored to a naive local
    # now(), so a fixed-date seed yields [] — indistinguishable from "worked,
    # nothing there" — and vacates the only collector redaction can be seen on.
    stamp = (capture_now - timedelta(hours=1)).isoformat(timespec="seconds")
    stale = (capture_now - timedelta(hours=200)).isoformat(timespec="seconds")
    (health / "supervisor.log").write_text(
        # The redaction cases: an assignment-form secret, and a POSIX path.
        f"{stamp} ERROR provider rejected the call: api_key=corpus-authored-secret-value\n"
        f"{stamp} ERROR reading /corpus/authored/path/file.txt failed\n"
        # No parseable leading timestamp, so this line inherits the previous
        # parsed one and is marked approximate — and the whole line is redacted
        # rather than just the message. It carries the traceback header, whose
        # "suppression" is only a rename: any frames around it survive.
        "ERROR Traceback (most recent call last):\n"
        # Outside the 168-hour window: parsed, then dropped.
        f"{stale} ERROR this one is older than the recency window\n"
        # ⚠ The collector tests `\"ERROR\" in line` — a substring test over the whole
        # line, not a level field — so a line that merely MENTIONS the word is
        # collected. This is reference behaviour, and the seed makes it visible.
        f"{stamp} INFO the operator asked about an ERROR yesterday\n"
        # No ERROR token anywhere: never collected.
        f"{stamp} INFO an ordinary informational line\n"
    )


def _reset_portal_storage(root: Path, phase: str, registered_state: dict[str, bytes] | None) -> None:
    """Restore the phase's declared artifact state before each case.

    ⛔ A delete is not a restore. `established` and `disabled` declare a REGISTERED
    client with an EMPTY ledger, so the keypair, token and cached terms are put
    back and only the ledger is cleared. `unregistered` declares no portal state at
    all. Without this, the first case in a phase to reach `ensure_registered()`
    registers and every later one does not — which makes the probe ORDER
    load-bearing and the corpus a function of an undocumented list.
    """
    storage = root.joinpath(*PORTAL_STORAGE)
    if storage.exists():
        shutil.rmtree(storage)
    if registered_state is None:
        return
    storage.mkdir(parents=True, exist_ok=True)
    for name, payload in registered_state.items():
        target = storage / name
        target.write_bytes(payload)
        target.chmod(0o600)


def _seed_pending_acknowledgement(root: Path) -> None:
    """Leave exactly one completed, unacknowledged ledger record.

    Written through the reference's own ledger writer rather than forged as JSON,
    so the record's shape, its HMAC fingerprints and its permissions are whatever
    the reference actually produces.
    """
    from solstone.apps.support import operations

    storage = root.joinpath(*PORTAL_STORAGE)
    record = operations.begin_operation(
        ACTION_KEY,
        "close",
        {"ticket_id": SEEDED_TICKET_ID},
        principal="anonymous",
        storage_dir=storage,
    )
    record = operations.mark_in_progress(record, storage_dir=storage)
    operations.mark_completed(record, remote_operation_id="909", storage_dir=storage)
    pending = operations.list_pending_acknowledgements(storage_dir=storage)
    if len(pending) != 1:
        raise AssertionError(
            f"drain seed did not leave exactly one pending acknowledgement: {len(pending)}"
        )


def _capture_registered_state(root: Path, portal_url: str) -> dict[str, bytes]:
    """Register once, as a per-phase precondition, and snapshot the artifacts."""
    from solstone.apps.support.portal import PortalClient

    storage = root.joinpath(*PORTAL_STORAGE)
    if storage.exists():
        shutil.rmtree(storage)
    client = PortalClient(portal_url=portal_url, storage_dir=storage)
    client.register()
    return {path.name: path.read_bytes() for path in sorted(storage.iterdir()) if path.is_file()}


# --------------------------------------------------------------------------
# Recording.
# --------------------------------------------------------------------------


def _issue(client: Any, probe: dict[str, Any]) -> Any:
    headers = {"Host": HOST_HEADER}
    if probe["method"] == "POST" and probe["key"]:
        headers["Idempotency-Key"] = (
            probe["key"] if isinstance(probe["key"], str) else ACTION_KEY
        )
    kwargs: dict[str, Any] = {"headers": headers}
    if probe["files"]:
        data = dict(probe["body"] or {})
        for field, (filename, payload, _content_type) in probe["files"].items():
            import io

            data[field] = (io.BytesIO(payload), filename)
        kwargs["data"] = data
        kwargs["content_type"] = "multipart/form-data"
    elif probe["body"] is not None:
        kwargs["json"] = probe["body"]
    elif probe["method"] == "POST":
        kwargs["json"] = {}
    return client.open(probe["path"], method=probe["method"], **kwargs)


# A non-JSON body over this size is recorded as a digest plus a bounded prefix.
# The three page assets are 1 KB–40 KB of HTML and JavaScript, captured once per
# phase; recording them verbatim five times is 200 KB of fixture that asserts
# nothing a digest does not. ⚠ Below the bound — the redirect body, the
# corrupt-config text — the exact bytes ARE the contract, so they stay verbatim.
INLINE_TEXT_LIMIT = 2000


def _response_record(response: Any, root: Path, portal_url: str) -> tuple[dict[str, Any], list[str]]:
    import hashlib

    content_type = response.headers.get("Content-Type", "")
    headers = {h: response.headers[h] for h in HEADER_ALLOWLIST if h in response.headers}
    record: dict[str, Any] = {"status": response.status_code, "headers": headers}
    if content_type.startswith("application/json"):
        body: Any = response.get_json()
        normalized, hits = _normalize(body, path="response.body", root=str(root), portal=portal_url)
        record["body"] = normalized
        return record, hits
    raw = response.get_data()
    text = raw.decode("utf-8", "replace")
    normalized_text, hits = _normalize(text, path="response.text", root=str(root), portal=portal_url)
    record["text_sha256"] = hashlib.sha256(normalized_text.encode("utf-8")).hexdigest()
    record["text_bytes"] = len(normalized_text.encode("utf-8"))
    if record["text_bytes"] <= INLINE_TEXT_LIMIT:
        record["text"] = normalized_text
    else:
        record["text_prefix"] = normalized_text[:200]
    return record, hits


def _record_case(
    client: Any,
    probe: dict[str, Any],
    *,
    root: Path,
    portal_url: str,
    log: list[dict[str, Any]],
    reset,
) -> dict[str, Any]:
    reset()
    mark = len(log)
    if probe["kind"] == "drain":
        # Leave one completed, unacknowledged ledger record, then issue the target
        # request. Only the stub's log can show the drain: both hook layers swallow.
        #
        # ⚠ Driving a successful mutation through a ROUTE does not work as a seed,
        # and finding that out is worth recording: that request's own
        # `after_request` drain acknowledges the record it just created, so the
        # ledger is empty again before the next request arrives. A pending
        # acknowledgement therefore survives only when the acknowledgement itself
        # did not land. The seed writes the record through the reference's own
        # ledger writer instead of forging JSON.
        _seed_pending_acknowledgement(root)
        mark = len(log)
    response = _issue(client, probe)
    record, hits = _response_record(response, root, portal_url)
    case: dict[str, Any] = {
        "name": probe["name"],
        "method": probe["method"],
        "path": probe["path"],
        "sends_idempotency_key": bool(probe["key"]) if probe["method"] == "POST" else False,
        "response": record,
    }
    if probe["kind"] == "probe_repeat":
        repeat = _issue(client, probe)
        repeat_record, repeat_hits = _response_record(repeat, root, portal_url)
        # Rewrite the repeat's normalization hits onto their own prefix.
        case["repeat_response"] = repeat_record
        hits = hits + [h.replace("response.", "repeat_response.", 1) for h in repeat_hits]
    case["portal_requests"] = log[mark:]
    case["normalized"] = sorted(set(hits))
    return case


def _run_phase_worker(phase: str, root: Path) -> list[dict[str, Any]]:
    if "solstone.convey" in sys.modules:
        raise AssertionError("solstone.convey imported before the egress guard")

    capture_now = datetime.now()
    server, log = _stub_app()
    portal_url = f"http://127.0.0.1:{server.server_address[1]}"
    import threading

    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        os.environ["SOLSTONE_SUPPORT_URL"] = portal_url
        os.environ["SOLSTONE_JOURNAL"] = str(root)
        os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
        _seed_journal(root, phase, capture_now)

        from solstone.convey import create_app
        from solstone.think.utils import get_journal

        if phase != "unestablished" and get_journal() != str(root):
            raise AssertionError("SOLSTONE_JOURNAL did not resolve to the phase root")

        with patch("socket.gethostname", return_value=AUTHORED_HOSTNAME):
            registered_state = None
            if phase in {"established", "disabled"}:
                registered_state = _capture_registered_state(root, portal_url)
                if "keypair.pem" not in registered_state or "token.json" not in registered_state:
                    raise AssertionError(
                        "phase precondition did not register: "
                        f"{sorted(registered_state)}"
                    )
            app = create_app(str(root))
            app.config.update(TESTING=True)
            client = app.test_client()

            def reset() -> None:
                _reset_portal_storage(root, phase, registered_state)

            cases = [
                _record_case(
                    client,
                    probe,
                    root=root,
                    portal_url=portal_url,
                    log=log,
                    reset=reset,
                )
                for probe in probes_for(phase)
            ]
    finally:
        server.shutdown()
        server.server_close()

    assert_egress_guard_can_see(f"convey-support child {phase}")
    assert_no_egress_attempted(
        f"convey-support child {phase}", ignore=("example.invalid", "198.51.100.7")
    )
    return cases


# --------------------------------------------------------------------------
# Orchestration, validation and publication.
# --------------------------------------------------------------------------


def _reset_capture_root() -> None:
    home = Path.home().resolve()
    root = CAPTURE_ROOT.resolve()
    if root == home or home.is_relative_to(root) or root.is_relative_to(home):
        raise RuntimeError("capture root must be an absolute path outside $HOME")
    if CAPTURE_ROOT.exists():
        unexpected = {entry.name for entry in CAPTURE_ROOT.iterdir()} - set(PHASES)
        if unexpected:
            raise RuntimeError(f"capture root contains unexpected entries: {sorted(unexpected)}")
        shutil.rmtree(CAPTURE_ROOT)
    CAPTURE_ROOT.mkdir(parents=True)
    for phase in PHASES:
        (CAPTURE_ROOT / phase).mkdir()


def _collect_phases() -> dict[str, list[dict[str, Any]]]:
    _reset_capture_root()
    captured: dict[str, list[dict[str, Any]]] = {}
    for phase in PHASES:
        command = [
            sys.executable,
            os.path.abspath(__file__),
            "--_phase-worker",
            phase,
            "--_root",
            str(CAPTURE_ROOT / phase),
        ]
        result = subprocess.run(command, capture_output=True, text=True)
        if result.returncode != 0:
            detail = result.stderr.strip() or result.stdout.strip() or "no child output"
            raise RuntimeError(
                f"convey-support phase {phase} failed; no fixture written: {detail}"
            )
        captured[phase] = json.loads(result.stdout)
    return captured


def _validate_counts(captured: dict[str, list[dict[str, Any]]]) -> None:
    actual = {phase: len(cases) for phase, cases in captured.items()}
    if actual != EXPECTED_PER_PHASE:
        raise AssertionError(f"case count mismatch: {actual} != {EXPECTED_PER_PHASE}")
    total = sum(actual.values())
    if total != EXPECTED_TOTAL:
        raise AssertionError(f"case total mismatch: {total} != {EXPECTED_TOTAL}")
    for phase, cases in captured.items():
        names = [case["name"] for case in cases]
        if len(set(names)) != len(names):
            raise AssertionError(f"duplicate case names in {phase}")


#: The literal `reason_code` `collect_brain_health`'s bare `except Exception`
#: returns. `build_brain_snapshot` never produces it, so seeing it means the
#: reference SWALLOWED an exception and handed back a plausible envelope, and a
#: port that hardcodes the fallback would match the oracle exactly.
#: ⚠ The sibling guard in the system corpus reads body["brain"]["reason_code"].
#: Here it is one level deeper and under a different key — copy-pasting that
#: accessor finds nothing and reports clean.
_BRAIN_FALLBACK_REASON = "brain_record_unavailable"


def _reject_swallowed_reference_failures(captured: dict[str, list[dict[str, Any]]]) -> None:
    offenders = []
    for phase, cases in captured.items():
        for case in cases:
            for key in ("response", "repeat_response"):
                body = (case.get(key) or {}).get("body")
                if not isinstance(body, dict):
                    continue
                snapshot = (body.get("brain_health") or {}).get("snapshot")
                if isinstance(snapshot, dict) and snapshot.get("reason_code") == _BRAIN_FALLBACK_REASON:
                    offenders.append(f"{phase}/{case['name']}")
    if offenders:
        raise RuntimeError(
            "convey-support refuses to freeze a swallowed reference failure: "
            f"{len(offenders)} case(s) carry brain_health.snapshot.reason_code "
            f"{_BRAIN_FALLBACK_REASON!r}, which only collect_brain_health's "
            "`except Exception` produces. Fix the environment rather than "
            f"recording its fallback. First offenders: {offenders[:5]}"
        )


def _assert_redaction_was_observed(captured: dict[str, list[dict[str, Any]]]) -> None:
    """Prove the redactors FIRED, rather than assuming they had nothing to do.

    🔴 A corpus captured from a journal with no secrets in it matches a port with
    the redactor DELETED, exactly. The seed plants two authored literals — one
    under a secret-shaped config key, one inside a log line — so the capture can
    assert both that the literal is gone and that the redaction marker is present.
    ⛔ This is a capture of the redactor's OUTPUT, not a proof of its coverage.
    That proof is a derivation test over injected inputs in the implementation.
    """
    seeded_config_secret = "corpus-not-a-real-key-authored-for-this-fixture"
    seeded_log_secret = "corpus-authored-secret-value"
    saw_config_redaction = False
    saw_message_redaction = False
    for cases in captured.values():
        for case in cases:
            body = (case.get("response") or {}).get("body")
            if not isinstance(body, dict) or "config" not in body:
                continue
            rendered = json.dumps(body)
            if seeded_config_secret in rendered or seeded_log_secret in rendered:
                raise RuntimeError(
                    "convey-support captured a seeded secret literal verbatim: the "
                    "reference did not redact it, or the seed reached a collector "
                    "that does not redact"
                )
            provider = (body.get("config") or {}).get("provider")
            if isinstance(provider, dict) and provider.get("api_key") == "***":
                saw_config_redaction = True
            for entry in body.get("recent_errors") or []:
                if isinstance(entry, dict) and "<secret>" in (entry.get("message") or ""):
                    saw_message_redaction = True
    if not saw_config_redaction:
        raise RuntimeError(
            "convey-support never observed _strip_secrets replacing the seeded "
            "config key — the seed did not reach collect_config, so this corpus "
            "says nothing about redaction"
        )
    if not saw_message_redaction:
        raise RuntimeError(
            "convey-support never observed _bounded_redacted_text replacing the "
            "seeded log secret — collect_recent_errors returned nothing, most "
            "likely because the seeded stamp fell outside the 168-hour window"
        )


_MUTATING_METHODS = {"POST", "PUT", "PATCH", "DELETE"}


def _coverage_limits(captured: dict[str, list[dict[str, Any]]]) -> dict[str, Any]:
    """State what this corpus does NOT cover, computed from the recorded cases.

    🔴 The number that matters is not how many write-method cases a corpus carries.
    It is how many of them **succeeded** — a refusal grades the refusal envelope
    and the session gate, never mutation semantics. "N write cases across M routes"
    is a true aggregate that reads as thorough and is not a claim about the thing
    anyone cares about.

    Computed rather than written down, so it cannot drift from the fixture it
    describes: a hand-written zero stays right until someone adds a probe, and is
    then a false negative inside the artifact everyone trusts.
    """
    cases = [c for phase_cases in captured.values() for c in phase_cases]
    mutating = [c for c in cases if c["method"] in _MUTATING_METHODS]

    def succeeded(case: dict[str, Any], key: str) -> bool:
        status = ((case.get(key) or {}).get("status")) or 0
        return 200 <= status < 300

    first_success = [c for c in mutating if succeeded(c, "response")]
    repeat_success = [c for c in mutating if "repeat_response" in c and succeeded(c, "repeat_response")]
    routes: dict[str, list[str]] = {}
    for phase, phase_cases in captured.items():
        for case in phase_cases:
            if case["method"] in _MUTATING_METHODS and succeeded(case, "response"):
                routes.setdefault(f"{case['method']} {case['path']}", []).append(phase)
    return {
        "mutation_census": {
            "total_cases": len(cases),
            "mutating_method_cases": len(mutating),
            "cases_that_actually_mutated": len(first_success),
            "cases_whose_repeat_also_succeeded": len(repeat_success),
            "routes_with_a_successful_mutation": routes,
        },
        "what_a_green_replay_is_not_evidence_about": (
            "Every response body a portal-backed route returns here is the STUB's, "
            "not a production portal's: this corpus pins what the reference DID "
            "with a response, never what a real portal sends. It says nothing "
            "about the local operation ledger's state machine, nothing about the "
            "diagnostics redactor's coverage, and nothing about the twelve-arm "
            "outcome matrix, all of which are invisible to a replay and are proven "
            "by derivation tests over injected inputs in the implementation."
        ),
        "named_hazards_a_replay_cannot_see": [
            "PortalClient._principal() calls _ensure_keypair(), so a read-shaped "
            "name generates and persists an RSA-4096 keypair on every mutation. A "
            "port that regenerates instead of reusing orphans the owner's "
            "registered identity and returns success.",
            "register() ALWAYS writes tos.txt and token.json; only "
            "ensure_registered() is guarded on is_registered. A replay cannot tell "
            "the two apart because both produce the same body.",
            "operations._load_or_create_fingerprint_key fills only when absent and "
            "refuses on unsafe permissions. Regenerating the key invalidates every "
            "stored canonical_fingerprint, turning in-flight operations into "
            "idempotency conflicts.",
            "The 409 operation_in_progress arm of the dispatch matrix touches the "
            "ledger not at all, leaving a live ~60s lease; the other retryable arms "
            "release it. All four leave state == in_progress, so any assertion "
            "keyed to the recorded STATE cannot separate them.",
            "The redaction captured here is the redactor's OUTPUT on one authored "
            "seed. Four of the seven diagnostics collectors redact nothing at all, "
            "which is the reference's behaviour and not an oversight.",
            "collect_version and collect_revision are normalized, so this corpus "
            "pins their PRESENCE and their key, never their derivation.",
        ],
        "already_green_before_the_port": (
            "The unestablished and corrupt phases are properties of the shared "
            "session gate and the shared API error handler, not of this app. In the "
            "native shell the app catch-all already answers both, so those cases go "
            "green with zero routes registered. Only the established, disabled and "
            "unregistered phases discriminate."
        ),
    }


def _absolute_paths(rendered: str) -> set[str]:
    candidates = set(re.findall(r"(?<![A-Za-z0-9_])/(?:[A-Za-z0-9_.-]+/?)+", rendered))
    return {
        candidate.rstrip("/")
        for candidate in candidates
        if candidate.startswith(("/home/", "/Users/", "/var/", "/tmp/", "/private/", "/opt/"))
    }


def assert_independent_scrub(rendered: str, *, label: str) -> None:
    """Reject host-shaped values the shared publication guard does not cover."""
    findings: list[str] = []
    hostname_label = socket.gethostname().split(".", 1)[0].lower()
    username = getpass.getuser()
    home = os.environ.get("HOME", "")
    lowered = rendered.lower()
    if hostname_label and len(hostname_label) > 2 and hostname_label in lowered:
        findings.append("hostname first label")
    if username and username in rendered:
        findings.append("username")
    if home and home in rendered:
        findings.append("HOME")
    outside = sorted(_absolute_paths(rendered))
    if outside:
        findings.append("absolute host paths: " + ", ".join(outside))
    if findings:
        raise RuntimeError(f"{label}: independent scrub found " + "; ".join(findings))


def build_corpus() -> str:
    captured = _collect_phases()
    _validate_counts(captured)
    _reject_swallowed_reference_failures(captured)
    _assert_redaction_was_observed(captured)
    fixture = {
        "schema": "solstone-convey-support-corpus-v1",
        "generator": "scripts/convey_support_corpus.py",
        "tz": "UTC",
        "header_allowlist": list(HEADER_ALLOWLIST),
        "pinned": {
            "completed_at": PINNED_COMPLETED_AT,
            "handle": AUTHORED_HANDLE,
            "hostname": AUTHORED_HOSTNAME,
            "parent_action_id": ACTION_KEY,
            "seeded_ticket_id": SEEDED_TICKET_ID,
            "stub_access_token": STUB_ACCESS_TOKEN,
            "stub_tos": STUB_TOS,
        },
        "placeholders": {
            "journal_root": PLACEHOLDER_ROOT,
            "portal_url": PLACEHOLDER_PORTAL,
        },
        "expected_case_counts": EXPECTED_PER_PHASE,
        "phases": captured,
        "coverage_limits": _coverage_limits(captured),
    }
    rendered = json.dumps(fixture, indent=2, sort_keys=True) + "\n"
    assert_guard_can_see("convey-support fixture")
    assert_publishable(rendered, label=CORPUS_PATH.name)
    assert_independent_scrub(rendered, label=CORPUS_PATH.name)
    assert_egress_guard_can_see("convey-support orchestrator")
    assert_no_egress_attempted(
        "convey-support orchestrator", ignore=("example.invalid", "198.51.100.7")
    )
    return rendered


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail when the committed fixture differs")
    parser.add_argument("--_phase-worker")
    parser.add_argument("--_root")
    args = parser.parse_args()

    if args._phase_worker:
        if not args._root:
            parser.error("--_phase-worker requires --_root")
        cases = _run_phase_worker(args._phase_worker, Path(args._root))
        sys.stdout.write(json.dumps(cases, sort_keys=True) + "\n")
        return 0

    rendered = build_corpus()
    if args.check:
        if not CORPUS_PATH.exists() or CORPUS_PATH.read_text(encoding="utf-8") != rendered:
            print(f"convey support corpus is stale: {CORPUS_PATH}", file=sys.stderr)
            print("regenerate with: python scripts/convey_support_corpus.py", file=sys.stderr)
            return 1
        print(f"convey support corpus is current: {CORPUS_PATH}")
        return 0
    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text(rendered, encoding="utf-8")
    print(f"wrote {CORPUS_PATH} ({EXPECTED_TOTAL} cases across {len(PHASES)} phases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
