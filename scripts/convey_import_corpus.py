#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the import app's owner-facing surface and its ingest door.

This drives the reference Flask application over the import surface in four
journal states and records **the full response body** of every probe, so a port
is checked against what the reference *answered* and not merely against which
routes it answered.

🔴 **The import app has TWO consumers and only one of them is a browser.**
Six routes under `/app/import/journal/<key_prefix>/…` are authenticated by a
journal-source key, not by a session, and `solstone/convey/root.py:102-107`
exempts them from the session gate by Flask endpoint name. **This corpus probes
them in the unestablished phase specifically to record that they do NOT redirect
to `/init` there.** A port that puts them behind the session layer locks every
paired journal source out of the door owner material comes through — and it
passes every session-gate criterion while doing so, because passing the gate is
what breaks it.

⚠ **Two things here are deliberately NOT in the corpus, and both are clocks.**

1. `build_import_info` reads `import_dir.stat().st_ctime` into `created_at` and
   `imported_at`. `st_ctime` is inode change time; `os.utime` cannot set it, so
   it is structurally unreproducible. Both are normalized **by exact field path**
   and recorded in `normalized_fields`. ⛔ This is a path allowlist, never a
   shape rule — nothing else date-shaped is touched.
2. `resolve_import_status` has a branch that compares `now` against
   `imported_at` and turns `running` into `failed`/`timeout` past
   `IMPORT_TASK_TIMEOUT_SECONDS`. Every seeded import here resolves
   clock-independently (`success`, `failed`, `pending`). ✅ **The timeout branch
   belongs in a derivation test over injected `now`, not in a captured fixture** —
   a capture of it would pin whatever the generating host's clock happened to
   make true.

⛔ Every value here is synthetic. No probe reads a real journal, and the seeded
journal is a fresh temporary directory per phase.

Usage:
    python scripts/convey_import_corpus.py            # write the corpus
    python scripts/convey_import_corpus.py --check    # fail if it would change
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

# Pin before importing time or any Solstone module: routes read process-local time.
os.environ["TZ"] = "UTC"
import time  # noqa: E402

if hasattr(time, "tzset"):
    time.tzset()

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_import_corpus.json"
sys.path.insert(0, str(REPO_ROOT))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from corpus_scrub import (  # noqa: E402
    assert_egress_guard_can_see,
    assert_guard_can_see,
    assert_no_egress_attempted,
    assert_publishable,
    forbid_non_loopback_egress,
)

# The two destinations the guard's own positive control provokes. Anything else
# in the attempt log is a reference route reaching out.
CONTROL_DESTINATIONS = ("example.invalid", "198.51.100.7")

# 🔴 Installed BEFORE any Solstone module is imported. Driving a reference
# route is not provably read-only: one app's list endpoint was measured
# registering a real account on a production service while being probed on a
# throwaway journal. The harness makes egress impossible rather than reasoning
# about which routes reach out; loopback stays open for callosum and friends.
forbid_non_loopback_egress()
assert_egress_guard_can_see(__file__)
# ⚠ **The unit of analysis is the BLUEPRINT, not the route.** A drain registered
# on `before_request`/`after_request` runs for every request to that blueprint,
# including refusals, so auditing the routes you intend to probe does not bound
# what probing them reaches. Audited on 2026-08-13 with a positive control:
# `app:support` registers `_drain_pending_acknowledgements_{before,after}_request`
# and the query found both, while `app:import` and `app:backup` register **no**
# blueprint-scoped hooks at all. The four app-wide hooks are convey core —
# identity stamp, request id, the access gate, the loopback-origin guard.


PLACEHOLDER_ROOT = "<JOURNAL_ROOT>"
PLACEHOLDER_CTIME = "<DIR_CTIME>"

PINNED_COMPLETED_AT = 1767225600
# A journal-source key long enough for the 8-character state prefix the reference
# derives (`key[:8]`), fixed so every recorded path is reproducible.
PINNED_SOURCE_KEY = "corpusSourceKey0000000000000000000000000000"
PINNED_SOURCE_PREFIX = PINNED_SOURCE_KEY[:8]
PINNED_SOURCE_NAME = "corpus_peer"
PINNED_CREATED_AT_MS = 1767225600000

OK_IMPORT = "20260801_120000"
FAILED_IMPORT = "20260802_130000"
PENDING_IMPORT = "20260803_140000"
CONTENT_IMPORT = "20260804_150000"

# 🔴 Normalization is a PATH ALLOWLIST, never a shape test. Each entry is the
# dotted field path inside a JSON body whose value is a temp-directory inode
# timestamp and therefore cannot be reproduced by anything.
NORMALIZED_CTIME_PATHS: frozenset[str] = frozenset(
    {
        "created_at",
        "imported_at",
        "imports[].created_at",
        "imports[].imported_at",
    }
)

# (method, path, body-or-None, why this probe is in the corpus)
Probe = tuple[str, str, dict[str, Any] | None, str]

PROBES: tuple[Probe, ...] = (
    ("GET", "/app/import/", None, "the app index: the SPA shell, byte-identical"),
    ("GET", "/app/import/workspace", None, "the app fragment bytes"),
    ("GET", "/app/import/background", None, "the background fragment"),
    (
        "GET",
        "/app/import/static/import_detail.js",
        None,
        "per-app static script",
    ),
    ("GET", "/app/import/api/sources", None, "the source catalogue the picker renders"),
    (
        "GET",
        "/app/import/api/list",
        None,
        "🔴 the import history list: a LIST of enriched records, not a map",
    ),
    ("GET", "/app/import/api/guide/ics", None, "an export guide that exists"),
    (
        "GET",
        "/app/import/api/guide/nope",
        None,
        "an export guide that does not exist: 404, not an empty 200",
    ),
    (
        "GET",
        "/app/import/api/guide/BAD-Name",
        None,
        "a guide name failing the [a-z_]+ guard: refusal, not a path read",
    ),
    (
        "GET",
        f"/app/import/{OK_IMPORT}",
        None,
        "the per-import detail shell",
    ),
    (
        "GET",
        f"/app/import/api/{OK_IMPORT}",
        None,
        "detail for a successfully processed import",
    ),
    (
        "GET",
        f"/app/import/api/{FAILED_IMPORT}",
        None,
        "detail for a failed import: status and error_stage travel",
    ),
    (
        "GET",
        f"/app/import/api/{PENDING_IMPORT}",
        None,
        "detail for an unstarted import",
    ),
    (
        "GET",
        "/app/import/api/20991231_235959",
        None,
        "detail for an import that does not exist",
    ),
    (
        "GET",
        f"/app/import/api/{OK_IMPORT}/content",
        None,
        "content for an import with no manifest and none derivable: refusal, not an empty 200",
    ),
    (
        "GET",
        f"/app/import/api/{CONTENT_IMPORT}/content",
        None,
        "the content listing for an import that has a manifest",
    ),
    (
        "GET",
        f"/app/import/api/{CONTENT_IMPORT}/content?month=202608&per_page=1&page=2",
        None,
        "the same listing filtered and paged: the arithmetic, not just the shape",
    ),
    (
        "GET",
        f"/app/import/api/{CONTENT_IMPORT}/content/corpus-entry-2",
        None,
        "one content item by id",
    ),
    (
        "GET",
        f"/app/import/api/{CONTENT_IMPORT}/content/no-such-entry",
        None,
        "a content item id that is not in the manifest",
    ),
    (
        "GET",
        "/app/import/api/journal-sources/list",
        None,
        "the registered journal sources an owner can see",
    ),
    (
        "GET",
        f"/app/import/api/journal-sources/{PINNED_SOURCE_NAME}/status",
        None,
        "status for a registered source",
    ),
    (
        "GET",
        "/app/import/api/journal-sources/missing/status",
        None,
        "status for an unregistered source: refusal",
    ),
    (
        "GET",
        f"/app/import/api/journal-sources/{PINNED_SOURCE_NAME}/staged",
        None,
        "staged review items for a registered source",
    ),
    (
        "GET",
        "/app/import/api/journal-sources/missing/staged",
        None,
        "staged items for an unregistered source: refusal",
    ),
    # ---- refusals on the browser write surface -------------------------------
    (
        "POST",
        "/app/import/api/save-path",
        {},
        "register-a-path with no fields: the missing-field refusal",
    ),
    (
        "POST",
        "/app/import/api/save-path",
        {"client_item_id": "corpus-1", "path": "/nonexistent/corpus/path"},
        # 🔴 The path must not exist, and the reason is not that a missing path is
        # the interesting case. A path that DOES exist is hashed by `hash_source`
        # and recorded as a staged import against the seeded journal, so the probe
        # would be a mutation. ⛔ Do not point this at a real file.
        "register-a-path pointing at nothing: refuses before the source is read",
    ),
    (
        "POST",
        "/app/import/api/meta",
        {},
        "metadata update with no target",
    ),
    (
        "POST",
        "/app/import/api/journal-sources/create",
        {"name": ""},
        "source create with an empty name",
    ),
    (
        "POST",
        "/app/import/api/journal-sources/create",
        {"name": "../escape"},
        "🔴 source create with a traversal name: what the name guard ACTUALLY guards",
    ),
    (
        "POST",
        "/app/import/api/journal-sources/create",
        {"name": PINNED_SOURCE_NAME},
        "source create colliding with a registered name: 409, not a silent overwrite",
    ),
    (
        "POST",
        f"/app/import/api/journal-sources/{PINNED_SOURCE_NAME}/resolve-config",
        {"field": "unknown.field", "action": "skip"},
        "resolve a config field that was never staged",
    ),
    # ---- the ingest door: key-authenticated, session-gate EXEMPT --------------
    (
        "GET",
        f"/app/import/journal/{PINNED_SOURCE_PREFIX}/manifest/entities",
        None,
        "🔴 door probe with NO key: must refuse on its own terms, never 302 to /init",
    ),
    (
        "GET",
        "/app/import/journal/00000000/manifest/entities",
        None,
        "door probe for an unknown prefix with no key",
    ),
    (
        "POST",
        f"/app/import/journal/{PINNED_SOURCE_PREFIX}/ingest/entities",
        {"entities": []},
        "🔴 ingest POST with no key: the door's own refusal",
    ),
)


def _reset_registry() -> None:
    """Drop the journal-source registry singleton between phases.

    ⚠ `JournalSourceRegistry.singleton()` caches on the resolved sources
    directory. Each phase uses a fresh journal root so the directory differs and
    the cache re-instantiates — but that is a property of this harness, not a
    guarantee, so it is asserted rather than assumed.
    """
    # ⚠ `import` is a reserved word, so this package is only reachable through
    # `import_module` — the same reason every consumer in the tree spells it
    # that way. A plain `from solstone.apps.import…` is a SyntaxError.
    from importlib import import_module

    module = import_module("solstone.apps.import.journal_sources")
    assert hasattr(module, "JournalSourceRegistry"), (
        "journal_sources.JournalSourceRegistry moved; the registry reset no longer binds"
    )
    module.JournalSourceRegistry._instance = None


def _seed_import(
    root: Path,
    timestamp: str,
    *,
    import_json: dict[str, Any],
    imported_json: dict[str, Any] | None,
    payload_name: str,
    payload: bytes,
) -> None:
    import_dir = root / "imports" / timestamp
    import_dir.mkdir(parents=True, exist_ok=True)
    (import_dir / payload_name).write_bytes(payload)
    (import_dir / "import.json").write_text(
        json.dumps(import_json, indent=2, sort_keys=True) + "\n"
    )
    if imported_json is not None:
        (import_dir / "imported.json").write_text(
            json.dumps(imported_json, indent=2, sort_keys=True) + "\n"
        )


def _base_import_json(name: str, **overrides: Any) -> dict[str, Any]:
    record: dict[str, Any] = {
        "original_filename": name,
        "file_size": 42,
        "mime_type": "text/plain",
        "facet": None,
        "setting": None,
        "user_timestamp": None,
        "imported_via": "web_dashboard",
        "link_id": None,
        "observer_handle": None,
        "source": "corpus",
        "source_hash": "sha256:" + "0" * 64,
        "client_item_id": "corpus-item-1",
    }
    record.update(overrides)
    return record


def _build_journal(root: Path, phase: str) -> None:
    """Create the journal a phase's probes run against."""
    if phase == "unestablished":
        # ⚠ The door probes need a REGISTERED source even here, because the
        # question this phase answers is whether the door refuses on its own
        # terms rather than redirecting — and an unregistered prefix would refuse
        # for a second, different reason that hides the first.
        _seed_journal_sources(root)
        return
    (root / "config").mkdir(parents=True, exist_ok=True)
    target = root / "config" / "journal.json"
    if phase == "corrupt":
        target.write_text('{"setup": {"completed_at": 17672256')
        return
    target.write_text(
        json.dumps({"setup": {"completed_at": PINNED_COMPLETED_AT}}, indent=2) + "\n"
    )
    _seed_journal_sources(root)
    if phase != "populated":
        return

    _seed_import(
        root,
        OK_IMPORT,
        import_json=_base_import_json("notes.txt", client_item_id="corpus-item-1"),
        imported_json={
            "processed": True,
            "files_written": 1,
            "days": ["20260801"],
        },
        payload_name="notes.txt",
        payload=b"corpus import payload\n",
    )
    _seed_import(
        root,
        FAILED_IMPORT,
        import_json=_base_import_json(
            "broken.ics",
            client_item_id="corpus-item-2",
            mime_type="text/calendar",
        ),
        imported_json={
            "processed": False,
            "error": "calendar payload could not be parsed",
            "error_stage": "detect",
        },
        payload_name="broken.ics",
        payload=b"not really an ics\n",
    )
    _seed_import(
        root,
        PENDING_IMPORT,
        import_json=_base_import_json("waiting.md", client_item_id="corpus-item-3"),
        imported_json=None,
        payload_name="waiting.md",
        payload=b"# waiting\n",
    )
    _seed_import(
        root,
        CONTENT_IMPORT,
        import_json=_base_import_json(
            "conversations.json",
            client_item_id="corpus-item-4",
            mime_type="application/json",
        ),
        imported_json={
            "processed": True,
            "files_written": 3,
            "source_type": "chatgpt",
            "days": ["20260801", "20260802", "20260901"],
        },
        payload_name="conversations.json",
        payload=b"[]\n",
    )
    # A manifest with entries in two different months, so the month histogram
    # and the page arithmetic both have something to be wrong about.
    manifest_entries = [
        {
            "id": "corpus-entry-1",
            "date": "20260801",
            "title": "first conversation",
            "preview": "a short preview of the first entry",
            "body": "the full body of the first entry",
        },
        {
            "id": "corpus-entry-2",
            "date": "20260802",
            "title": "second conversation",
            "preview": "a short preview of the second entry",
            "body": "the full body of the second entry",
        },
        {
            "id": "corpus-entry-3",
            "date": "20260901",
            "title": "a September conversation",
            "preview": "a short preview of the third entry",
            "body": "the full body of the third entry",
        },
    ]
    (root / "imports" / CONTENT_IMPORT / "content_manifest.jsonl").write_text(
        "".join(json.dumps(entry, sort_keys=True) + "\n" for entry in manifest_entries)
    )


def _seed_journal_sources(root: Path) -> None:
    """Register one DL journal source and its per-source state directories."""
    sources_dir = root / "apps" / "import" / "journal_sources"
    sources_dir.mkdir(parents=True, exist_ok=True)
    record = {
        "key": PINNED_SOURCE_KEY,
        "name": PINNED_SOURCE_NAME,
        "created_at": PINNED_CREATED_AT_MS,
        "enabled": True,
        "revoked": False,
        "revoked_at": None,
        "stats": {
            "segments_received": 0,
            "entities_received": 0,
            "facets_received": 0,
            "imports_received": 0,
            "config_received": 0,
        },
    }
    (sources_dir / f"{PINNED_SOURCE_NAME}.json").write_text(
        json.dumps(record, indent=2, sort_keys=True) + "\n"
    )
    state_dir = root / "imports" / PINNED_SOURCE_PREFIX
    state_dir.mkdir(parents=True, exist_ok=True)
    (state_dir / "source.json").write_text("{}")
    for area in ("segments", "entities", "facets", "imports", "config"):
        (state_dir / area).mkdir(parents=True, exist_ok=True)


def _normalize(value: Any, found: set[str], path: str = "") -> Any:
    """Replace allowlisted inode timestamps, recording each one.

    ⛔ A field absent from `NORMALIZED_CTIME_PATHS` is returned verbatim however
    volatile it looks. Widening this is a decision, not a convenience.
    """
    if isinstance(value, dict):
        return {
            key: _normalize(item, found, f"{path}.{key}" if path else key)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_normalize(item, found, f"{path}[]") for item in value]
    if path in NORMALIZED_CTIME_PATHS and isinstance(value, (int, float)):
        found.add(path)
        return PLACEHOLDER_CTIME
    return value


def _record(client: Any, probe: Probe, root: Path) -> dict[str, Any]:
    method, path, body, why = probe
    if body is None:
        response = client.open(path, method=method)
    else:
        response = client.open(path, method=method, json=body)
    raw = response.get_data()
    redacted = raw.replace(str(root).encode(), PLACEHOLDER_ROOT.encode())
    content_type = response.headers.get("Content-Type", "")
    case: dict[str, Any] = {
        "method": method,
        "path": path,
        "why": why,
        "status": response.status_code,
        "content_type": content_type,
    }
    if body is not None:
        case["request_json"] = body
    location = response.headers.get("Location")
    if location:
        case["location"] = location
    if redacted != raw:
        case["body_normalized"] = [PLACEHOLDER_ROOT]

    if "json" in content_type:
        found: set[str] = set()
        # 🔴 The whole body, not a summary. A corpus that records which routes
        # answered cannot see a map served where a list was published.
        case["json"] = _normalize(json.loads(redacted), found)
        case["normalized_fields"] = sorted(found)
        case["body_sha256"] = hashlib.sha256(
            json.dumps(case["json"], sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        case["body_sha256_basis"] = "canonical-json"
        return case

    case["body_bytes"] = len(redacted)
    case["body_sha256"] = hashlib.sha256(redacted).hexdigest()
    case["body_sha256_basis"] = "raw-body"
    if response.status_code >= 400:
        case["body_text"] = redacted.decode("utf-8", errors="replace")
    return case


PHASES = ("unestablished", "corrupt", "empty", "populated")


def build_corpus() -> dict[str, Any]:
    from solstone.convey import create_app

    cases: dict[str, list[dict[str, Any]]] = {}
    for phase in PHASES:
        with tempfile.TemporaryDirectory(prefix=f"convey-import-{phase}-") as tmp:
            root = Path(tmp)
            _build_journal(root, phase)
            os.environ["SOLSTONE_JOURNAL"] = str(root)
            os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
            app = create_app(str(root))
            _reset_registry()
            client = app.test_client()
            cases[phase] = [_record(client, probe, root) for probe in PROBES]

    return {
        "schema": "solstone-convey-import-corpus-v1",
        "generator": "scripts/convey_import_corpus.py",
        "tz": "UTC",
        "pinned": {
            "completed_at": PINNED_COMPLETED_AT,
            "source_key": PINNED_SOURCE_KEY,
            "source_prefix": PINNED_SOURCE_PREFIX,
            "source_name": PINNED_SOURCE_NAME,
            "source_created_at_ms": PINNED_CREATED_AT_MS,
            "imports": {
                "success": OK_IMPORT,
                "failed": FAILED_IMPORT,
                "pending": PENDING_IMPORT,
            },
        },
        "placeholders": {
            "journal_root": PLACEHOLDER_ROOT,
            "dir_ctime": PLACEHOLDER_CTIME,
        },
        "phases": cases,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if the corpus on disk differs from a fresh capture",
    )
    args = parser.parse_args()

    corpus = build_corpus()
    rendered = json.dumps(corpus, indent=2, sort_keys=True) + "\n"

    # 🔴 These fixtures are published. Prove the guard can see a leak, then run it.
    assert_no_egress_attempted(
        f"convey {'backup' if 'backup' in __file__ else 'import'} corpus",
        ignore=CONTROL_DESTINATIONS,
    )
    assert_guard_can_see("import corpus")
    assert_publishable(rendered, label="convey import corpus")

    if args.check:
        if not CORPUS_PATH.exists():
            print(f"missing corpus: {CORPUS_PATH}", file=sys.stderr)
            return 1
        if CORPUS_PATH.read_text() != rendered:
            print(
                f"convey import corpus is stale: {CORPUS_PATH}\n"
                "regenerate with: python scripts/convey_import_corpus.py",
                file=sys.stderr,
            )
            return 1
        print(f"convey import corpus is current: {CORPUS_PATH}")
        return 0

    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text(rendered)
    total = sum(len(phase) for phase in corpus["phases"].values())
    print(f"wrote {CORPUS_PATH} ({total} cases across {len(corpus['phases'])} phases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
