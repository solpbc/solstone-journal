#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture the network app's read surface into a frozen native replay corpus."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from check_channel_adapter_scrub import IPV4_RE

CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_network_corpus.json"
CA_FIXTURE_ROOT = REPO_ROOT / "core" / "fixtures" / "convey_network_corpus_ca_nonproduction"
PLACEHOLDER_ROOT = "<JOURNAL_ROOT>"
PLACEHOLDER_HOST = "<HOST_DEPENDENT>"
PLACEHOLDER_TIME = "<CAPTURE_CLOCK>"
# solstone/apps/network/copy.py REACH_HOST_ADDRESS_PLACEHOLDER — static UI copy
# text shown as a form-field example, not captured host data.
_ALLOWED_IPV4_LITERALS = {"192.168.1.44"}

Probe = tuple[str, str]
PHASES = ("unestablished", "established", "corrupt")
PROBES: tuple[Probe, ...] = (
    ("GET", "/app/network/"),
    ("GET", "/app/network/api/state"),
    ("GET", "/app/network/api/status"),
    ("GET", "/app/network/api/identity"),
    ("GET", "/app/network/api/private-link"),
    ("GET", "/app/network/local-endpoints"),
)

NORMALIZED_FIELDS: dict[tuple[str, str], dict[str, str]] = {
    ("established", "/app/network/api/status"): {
        "home_candidates": "host",
        "lan_accessible": "host",
        "reachability": "host",
    },
    ("established", "/app/network/local-endpoints"): {
        "generated_at": "capture-clock",
    },
    ("corrupt", "/app/network/"): {"body": "journal-root"},
    ("corrupt", "/app/network/api/state"): {"detail": "journal-root"},
    ("corrupt", "/app/network/api/status"): {"detail": "journal-root"},
    ("corrupt", "/app/network/api/identity"): {"detail": "journal-root"},
    ("corrupt", "/app/network/api/private-link"): {"detail": "journal-root"},
    ("corrupt", "/app/network/local-endpoints"): {"body": "journal-root"},
}

STILL_DEFERRED_PATHS = (
    "/app/network/api/status",
    "/app/network/api/identity",
    "/app/network/api/private-link",
    "/app/network/local-endpoints",
)

ESTABLISHED_DEFERRED_NATIVE_RESPONSES = [
    {"phase": "established", "path": path}
    for _method, path in PROBES
    if path in STILL_DEFERRED_PATHS
]


def _git_rev() -> str:
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def _write_config(root: Path, content: str) -> None:
    target = root / "config" / "journal.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")


def _install_fixed_ca(root: Path) -> None:
    from solstone.think.journal_io import atomic_replace

    ca_root = root / "link" / "ca"
    atomic_replace(ca_root / "cert.pem", (CA_FIXTURE_ROOT / "cert.pem").read_bytes())
    atomic_replace(
        ca_root / "private.pem",
        (CA_FIXTURE_ROOT / "private.pem").read_bytes(),
        mode=0o600,
    )


def _prepare_journal(root: Path, phase: str) -> None:
    if phase == "unestablished":
        return
    if phase == "corrupt":
        _write_config(root, '{"setup": {"completed_at": 1')
        return
    _write_config(
        root,
        json.dumps(
            {"setup": {"completed_at": 1}, "link": {"posture": "direct"}},
            sort_keys=True,
        )
        + "\n",
    )
    _install_fixed_ca(root)
    from solstone.think.link.establish import create_link_state

    create_link_state(default_label="Network Corpus")


def _snapshot_tree(root: Path) -> dict[str, dict[str, int | str]]:
    manifest: dict[str, dict[str, int | str]] = {}
    for path in sorted(root.rglob("*")):
        if path.is_file():
            body = path.read_bytes()
            manifest[str(path.relative_to(root))] = {
                "byte_length": len(body),
                "digest": hashlib.sha256(body).hexdigest(),
            }
    return manifest


def _set_path(value: Any, path: str, replacement: str, root: str) -> None:
    if path == "body":
        raise ValueError("body normalization is only valid for raw bodies")
    current = value
    fields = path.split(".")
    for field in fields[:-1]:
        if not isinstance(current, dict):
            raise TypeError(f"normalization parent is not an object: {path}")
        current = current[field]
    if not isinstance(current, dict):
        raise TypeError(f"normalization object is not an object: {path}")
    leaf = fields[-1]
    if replacement == "journal-root":
        original = current[leaf]
        if not isinstance(original, str):
            raise TypeError(f"journal-root normalization is not text: {path}")
        current[leaf] = original.replace(root, PLACEHOLDER_ROOT)
        return
    current[leaf] = replacement


def _normalize_json(value: Any, fields: dict[str, str], root: Path) -> Any:
    normalized = json.loads(json.dumps(value))
    for path, kind in fields.items():
        if kind == "host":
            _set_path(normalized, path, PLACEHOLDER_HOST, str(root))
        elif kind == "capture-clock":
            _set_path(normalized, path, PLACEHOLDER_TIME, str(root))
        elif kind == "journal-root":
            _set_path(normalized, path, kind, str(root))
        else:
            raise ValueError(f"unknown normalization kind: {kind}")
    return normalized


def _normalize_raw(body: bytes, fields: dict[str, str], root: Path) -> bytes:
    if not fields:
        return body
    if fields != {"body": "journal-root"}:
        raise ValueError(f"unsupported raw normalization: {fields}")
    return body.replace(str(root).encode("utf-8"), PLACEHOLDER_ROOT.encode("utf-8"))


def _assert_body_stable(client: Any, path: str) -> None:
    first = client.get(path, follow_redirects=False)
    second = client.get(path, follow_redirects=False)
    if first.get_data() != second.get_data():
        raise AssertionError(f"response body is not stable for {path}")


def _record(client: Any, phase: str, probe: Probe, root: Path) -> dict[str, Any]:
    method, path = probe
    if path in {"/app/network/", "/app/network/api/state"}:
        _assert_body_stable(client, path)
    response = client.open(path, method=method, follow_redirects=False)
    body = response.get_data()
    fields = NORMALIZED_FIELDS.get((phase, path), {})
    case: dict[str, Any] = {
        "method": method,
        "path": path,
        "request_headers": {},
        "status": response.status_code,
        "content_type": response.headers.get("Content-Type", ""),
        "normalized_fields": fields,
    }
    location = response.headers.get("Location")
    if location is not None:
        case["location"] = location
    if "json" in case["content_type"]:
        case["body"] = _normalize_json(response.get_json(), fields, root)
    else:
        normalized = _normalize_raw(body, fields, root)
        case["body"] = {
            "digest": hashlib.sha256(normalized).hexdigest(),
            "byte_length": len(normalized),
        }
    return case


def _assert_fixture_scrub_clean(rendered: str) -> None:
    literals = sorted(
        {
            match.group(0)
            for match in IPV4_RE.finditer(rendered)
            if match.group(0) not in _ALLOWED_IPV4_LITERALS
        }
    )
    if literals:
        raise RuntimeError(
            "network corpus contains disallowed IPv4 literals: " + ", ".join(literals)
        )


def _assert_status_content_pinned(phases: dict[str, list[dict[str, Any]]]) -> None:
    case = next(
        case
        for case in phases["established"]
        if case["path"] == "/app/network/api/status"
    )
    body = case["body"]
    fields = case["normalized_fields"]
    if body.get("posture") != "direct":
        raise AssertionError(f"expected literal posture=direct, got {body.get('posture')!r}")
    if "posture" in fields:
        raise AssertionError("posture must not be normalized away")
    if not body.get("relay_state"):
        raise AssertionError("expected a named relay_state value")
    if "relay_state" in fields:
        raise AssertionError("relay_state must not be normalized away")


def build_corpus() -> dict[str, Any]:
    from solstone.convey import create_app

    os.environ.pop("SOL_LINK_RELAY_URL", None)
    os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
    phases: dict[str, list[dict[str, Any]]] = {}
    for phase in PHASES:
        with tempfile.TemporaryDirectory(prefix=f"convey-network-{phase}-") as tmp:
            root = Path(tmp)
            os.environ["SOLSTONE_JOURNAL"] = str(root)
            _prepare_journal(root, phase)
            app = create_app(str(root))
            client = app.test_client()
            before = _snapshot_tree(root) if phase == "established" else None
            phases[phase] = [_record(client, phase, probe, root) for probe in PROBES]
            if before is not None:
                after = _snapshot_tree(root)
                if after != before:
                    raise AssertionError("established network read sweep modified the journal")
    _assert_status_content_pinned(phases)
    corpus = {
        "schema": "solstone-convey-network-corpus-v1",
        "generator": "scripts/convey_network_corpus.py",
        "rev": _git_rev(),
        "placeholders": {
            "journal_root": PLACEHOLDER_ROOT,
            "host": PLACEHOLDER_HOST,
            "capture_clock": PLACEHOLDER_TIME,
        },
        "capture_host": {"platform": sys.platform, "machine": platform.machine()},
        "native_deviations": [],
        "established_deferred_native_responses": ESTABLISHED_DEFERRED_NATIVE_RESPONSES,
        "phases": phases,
    }
    _assert_fixture_scrub_clean(json.dumps(corpus, indent=2, sort_keys=True) + "\n")
    return corpus


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="exit non-zero if the corpus would change"
    )
    args = parser.parse_args()
    rendered = json.dumps(build_corpus(), indent=2, sort_keys=True) + "\n"
    if args.check:
        if not CORPUS_PATH.exists():
            print(f"network corpus is stale: {CORPUS_PATH}", file=sys.stderr)
            return 1
        recorded = json.loads(CORPUS_PATH.read_text(encoding="utf-8"))
        generated = json.loads(rendered)
        recorded.pop("rev", None)
        generated.pop("rev", None)
        if recorded != generated:
            print(f"network corpus is stale: {CORPUS_PATH}", file=sys.stderr)
            return 1
        print(f"network corpus is current: {CORPUS_PATH}")
        return 0
    CORPUS_PATH.write_text(rendered, encoding="utf-8")
    print(f"wrote {CORPUS_PATH} ({len(PHASES) * len(PROBES)} cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
