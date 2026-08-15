# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Build the observer-client OpenAPI contract bundle."""

from __future__ import annotations

import copy
import hashlib
import json
import os
import re
import stat
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from solstone.convey.contract.assemble import CALLOSUM_REGISTRY, build_document
from solstone.observe import protocol

INITIAL_BUNDLE_SEMVER = "1.0.0"
BUNDLE_SEMVER = "7.0.2"
GENERATOR_IDENTITY = "solstone.convey.contract.observer_bundle.v1"
BUNDLE_SCHEMA_IDENTITY = "solstone.observer-client-contract-bundle.schema.v1"
SCHEMA_DIALECT_URI = "https://json-schema.org/draft/2020-12/schema"

BUNDLE_REL_DIR = Path("docs/openapi/observer-client-contract")
MANIFEST_REL = BUNDLE_REL_DIR / "manifest.json"
PROJECTION_REL = BUNDLE_REL_DIR / "projection.openapi.json"
VECTORS_REL = BUNDLE_REL_DIR / "vectors.json"
FIXTURES_REL = BUNDLE_REL_DIR / "fixtures/wire-behavior.json"
CONSUMER_AUDIT_REL = BUNDLE_REL_DIR / "consumer-audit.json"
MANIFEST_NAME = "manifest.json"

OBSERVER_CLIENT_OPERATION_IDS: tuple[str, ...] = (
    "callosum.rootEvents",
    "chat.openSolChatRequest",
    "link.pair",
    "observer.callosumStream",
    "observer.ingestEvent",
    "observer.ingestSegments",
    "observer.ingestUpload",
    "observer.register",
)

CONSUMER_IDENTIFIERS = [
    "solstone-android",
    "solstone-browser",
    "solstone-linux",
    "solstone-macos",
    "solstone-swift",
    "solstone-tmux",
    "solstone-windows",
]

AUDITED_CONSUMER_REVISIONS = [
    {
        "consumer_identifier": "solstone-windows",
        "revision": "19c972c4fea775176cea6421ac8b87f3bb20ab42",
    },
    {
        "consumer_identifier": "solstone-linux",
        "revision": "1c679db1ce6f9a65db70c5aae0ca2fad677416ef",
    },
    {
        "consumer_identifier": "solstone-browser",
        "revision": "998c1095cd8f766dd188bece5ad6527444f8dfac",
    },
]

WINDOWS_LINUX_ROLLOUT_TARGETS = [
    {
        "adoption_blocker_ids": [
            "linux.sol_voice.path",
            "linux.sol_voice.linux_notify_send",
        ],
        "consumer_identifier": "solstone-linux",
    },
    {
        "adoption_blocker_ids": [],
        "consumer_identifier": "solstone-windows",
    },
]

_SEMVER_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_WINDOWS_RESERVED_BASENAMES = {
    "CON",
    "PRN",
    "AUX",
    "NUL",
    *(f"COM{index}" for index in range(1, 10)),
    *(f"LPT{index}" for index in range(1, 10)),
}

_SOURCE_INPUTS: tuple[tuple[str, Path, str], ...] = (
    (
        "bundle.projection_builder",
        Path("solstone/convey/contract/observer_bundle.py"),
        "projection_builder",
    ),
    (
        "bundle.recording",
        Path("solstone/convey/contract/observer_bundle_recording.py"),
        "fixture_vector_builder",
    ),
    (
        "openapi.assembler",
        Path("solstone/convey/contract/assemble.py"),
        "openapi_source",
    ),
    (
        "fragment.root",
        Path("solstone/convey/root_contract.py"),
        "code_adjacent_fragment",
    ),
    (
        "fragment.chat",
        Path("solstone/convey/chat_contract.py"),
        "code_adjacent_fragment",
    ),
    (
        "fragment.link",
        Path("solstone/apps/network/contract.py"),
        "code_adjacent_fragment",
    ),
    (
        "fragment.observer",
        Path("solstone/apps/observer/contract.py"),
        "code_adjacent_fragment",
    ),
    (
        "extension.root_chat_native_subset",
        Path("solstone/convey/root_contract.py"),
        "code_adjacent_extension",
    ),
    (
        "extension.observer_sse_error_frame",
        Path("solstone/apps/observer/contract.py"),
        "code_adjacent_extension",
    ),
    ("producer.root_sse", Path("solstone/convey/root.py"), "producer"),
    ("producer.chat_routes", Path("solstone/convey/chat.py"), "producer"),
    (
        "producer.chat_stream",
        Path("solstone/convey/chat_stream.py"),
        "producer",
    ),
    (
        "producer.chat_sol_initiated_copy",
        Path("solstone/convey/sol_initiated/copy.py"),
        "producer",
    ),
    (
        "producer.chat_sol_initiated_events",
        Path("solstone/convey/sol_initiated/events.py"),
        "producer",
    ),
    (
        "producer.observer_routes",
        Path("solstone/apps/observer/routes.py"),
        "producer",
    ),
    (
        "producer.observer_utils",
        Path("solstone/apps/observer/utils.py"),
        "producer",
    ),
    ("producer.protocol", Path("solstone/observe/protocol.py"), "producer"),
    ("reason_codes", Path("solstone/convey/reasons.py"), "vocabulary_source"),
)


class ObserverBundleError(RuntimeError):
    """Raised when the observer-client bundle cannot be built."""


class BundleVerificationError(ObserverBundleError):
    """Raised when a bundle directory or manifest is invalid."""


class BundleExportRefused(ObserverBundleError):
    """Raised when bundle export must refuse without mutating the target."""


class BundleCompatibilityError(ObserverBundleError):
    """Raised when bundle history compatibility cannot be accepted."""


@dataclass(frozen=True)
class BundleSnapshot:
    """Verified bundle files keyed by bundle-relative path."""

    manifest: dict[str, Any]
    files: dict[str, bytes]


def parse_semver(value: str) -> tuple[int, int, int]:
    """Parse strict MAJOR.MINOR.PATCH SemVer."""

    match = _SEMVER_RE.fullmatch(value)
    if not match:
        raise ValueError(f"invalid SemVer: {value!r}")
    return tuple(int(part) for part in match.groups())


def compare_semver(left: str, right: str) -> int:
    """Compare two strict SemVer values."""

    left_parts = parse_semver(left)
    right_parts = parse_semver(right)
    return (left_parts > right_parts) - (left_parts < right_parts)


def render_json(payload: object) -> str:
    """Return canonical generated JSON text."""

    return json.dumps(payload, indent=2, sort_keys=True) + "\n"


def build_projection_document(source: dict[str, Any] | None = None) -> dict[str, Any]:
    """Project the full OpenAPI document to observer-client operations."""

    source_document = source if source is not None else build_document()
    selected_paths: dict[str, dict[str, Any]] = {}
    found: set[str] = set()

    for path, methods in source_document.get("paths", {}).items():
        if not isinstance(methods, dict):
            continue
        for method, operation in methods.items():
            if not isinstance(operation, dict):
                continue
            operation_id = operation.get("operationId")
            if operation_id not in OBSERVER_CLIENT_OPERATION_IDS:
                continue
            selected_paths.setdefault(path, {})[method] = copy.deepcopy(operation)
            found.add(operation_id)

    missing = set(OBSERVER_CLIENT_OPERATION_IDS) - found
    if missing:
        raise ObserverBundleError(
            "observer bundle projection missing operation(s): "
            + ", ".join(sorted(missing))
        )

    component_names = component_ref_closure(selected_paths, source_document)
    source_schemas = source_document.get("components", {}).get("schemas", {})
    components = {
        "schemas": {
            name: copy.deepcopy(source_schemas[name]) for name in component_names
        }
    }

    projection = {
        "openapi": source_document["openapi"],
        "info": {
            "title": "Solstone Observer Client Contract Projection",
            "version": source_document["info"]["version"],
            "x-generated": True,
            "x-generated-by": GENERATOR_IDENTITY,
            "description": (
                "Generated OpenAPI projection for observer-owned client "
                "contract surfaces. Regenerate with make openapi."
            ),
        },
        "paths": {path: selected_paths[path] for path in sorted(selected_paths)},
        "components": components,
        "x-callosum-registry": {
            key: CALLOSUM_REGISTRY[key] for key in sorted(CALLOSUM_REGISTRY)
        },
    }
    if "x-vocabularies" in source_document:
        projection["x-vocabularies"] = copy.deepcopy(source_document["x-vocabularies"])
    validate_projection_refs(projection)
    return projection


def component_ref_closure(
    selected_paths: dict[str, Any], source_document: dict[str, Any]
) -> list[str]:
    """Return sorted component names reachable from selected paths."""

    source_schemas = source_document.get("components", {}).get("schemas", {})
    closure: set[str] = set()
    pending = list(_local_schema_refs(selected_paths))

    while pending:
        name = pending.pop()
        if name in closure:
            continue
        if name not in source_schemas:
            raise ObserverBundleError(f"projection references missing schema: {name}")
        closure.add(name)
        pending.extend(_local_schema_refs(source_schemas[name]))

    return sorted(closure)


def validate_projection_refs(projection: dict[str, Any]) -> None:
    """Raise if any projection `$ref` does not resolve inside the projection."""

    schemas = projection.get("components", {}).get("schemas", {})
    missing = sorted(set(_local_schema_refs(projection)) - set(schemas))
    if missing:
        raise ObserverBundleError(
            "projection has dangling schema reference(s): " + ", ".join(missing)
        )


def build_bundle_files(root: Path | None = None) -> dict[Path, str]:
    """Build every generated observer-client bundle file as canonical text."""

    from solstone.convey.contract.observer_bundle_recording import (
        build_fixture_and_vector_payloads,
    )

    repo_root = _repo_root(root)
    projection = build_projection_document()
    fixtures, vectors = build_fixture_and_vector_payloads(projection)
    consumer_audit = build_consumer_audit_payload()

    payload_texts = {
        PROJECTION_REL: render_json(projection),
        FIXTURES_REL: render_json(fixtures),
        VECTORS_REL: render_json(vectors),
        CONSUMER_AUDIT_REL: render_json(consumer_audit),
    }
    manifest = build_manifest_payload(repo_root, projection, payload_texts)
    return {
        MANIFEST_REL: render_json(manifest),
        **payload_texts,
    }


def stale_bundle_paths(root: Path | None = None) -> list[Path]:
    """Return generated bundle paths whose committed text is stale."""

    repo_root = _repo_root(root)
    stale: list[Path] = []
    for rel_path, expected in build_bundle_files(repo_root).items():
        path = repo_root / rel_path
        try:
            current = path.read_text(encoding="utf-8")
        except FileNotFoundError:
            current = ""
        if current != expected:
            stale.append(rel_path)
    return sorted(stale)


def build_manifest_payload(
    root: Path, projection: dict[str, Any], payload_texts: dict[Path, str]
) -> dict[str, Any]:
    """Build the deterministic manifest payload."""

    parse_semver(BUNDLE_SEMVER)
    return {
        "audited_consumer_revisions": AUDITED_CONSUMER_REVISIONS,
        "bundle_schema_identity": BUNDLE_SCHEMA_IDENTITY,
        "bundle_semver": BUNDLE_SEMVER,
        "component_closure": sorted(
            projection.get("components", {}).get("schemas", {}).keys()
        ),
        "consumer_identifiers": CONSUMER_IDENTIFIERS,
        "files": [
            {
                "path": rel_path.relative_to(BUNDLE_REL_DIR).as_posix(),
                "sha256": _sha256_text(payload_texts[rel_path]),
            }
            for rel_path in sorted(payload_texts)
        ],
        "generator_identity": GENERATOR_IDENTITY,
        "generator_inputs": _generator_inputs(root),
        "openapi_document_version": projection["info"]["version"],
        "openapi_spec_version": projection["openapi"],
        "observer_protocol_version": protocol.OBSERVER_PROTOCOL_VERSION,
        "operation_ids": list(OBSERVER_CLIENT_OPERATION_IDS),
        "projection_path": PROJECTION_REL.relative_to(BUNDLE_REL_DIR).as_posix(),
        "schema_dialect_uri": SCHEMA_DIALECT_URI,
        "supported_response_variants": [1, 2],
        "vocabularies": build_vocabulary_inventory(projection),
        "windows_linux_rollout_targets": WINDOWS_LINUX_ROLLOUT_TARGETS,
    }


def build_consumer_audit_payload() -> dict[str, Any]:
    """Return the committed frozen consumer audit data."""

    windows_rev = "19c972c4fea775176cea6421ac8b87f3bb20ab42"
    linux_rev = "1c679db1ce6f9a65db70c5aae0ca2fad677416ef"
    browser_rev = "998c1095cd8f766dd188bece5ad6527444f8dfac"
    windows_observer_files = [
        "crates/observer-pl/src/lib.rs",
        "crates/observer-pl/src/wire.rs",
    ]
    linux_chat_file = "crates/solstone-linux/src/chat_bridge.rs"
    browser_file = "extension/journal.js"

    return {
        "audited_commits": [
            {"commit": windows_rev, "consumer": "solstone-windows"},
            {"commit": linux_rev, "consumer": "solstone-linux"},
            {"commit": browser_rev, "consumer": "solstone-browser"},
        ],
        "direct_paths": [
            _audit_path(
                "solstone-browser",
                browser_rev,
                [browser_file],
                "/app/observer/register",
                "bundled",
                "Projected as observer.register for browser observer enrollment.",
            ),
            _audit_path(
                "solstone-browser",
                browser_rev,
                [browser_file],
                "/app/observer/ingest",
                "bundled",
                "Projected as observer.ingestUpload for browser upload.",
            ),
            _audit_path(
                "solstone-browser",
                browser_rev,
                [browser_file],
                "/app/observer/ingest/event",
                "bundled",
                "Projected as observer.ingestEvent for browser event relay.",
            ),
            _audit_path(
                "solstone-browser",
                browser_rev,
                [browser_file],
                "/app/observer/ingest/segments/{day}",
                "bundled",
                (
                    "Projected as observer.ingestSegments; browser reconcile "
                    "consumes the legacy v1 array variant."
                ),
            ),
            _audit_path(
                "solstone-browser",
                browser_rev,
                [browser_file],
                "/enroll/device",
                "relay_session_control_excluded_from_journal_projection",
                "Relay enrollment/session-control path, not journal-owned API.",
            ),
            _audit_path(
                "solstone-linux",
                linux_rev,
                ["crates/solstone-linux/src/upload.rs"],
                "/app/observer/register",
                "bundled",
                "Projected as observer.register for Linux observer enrollment.",
            ),
            _audit_path(
                "solstone-linux",
                linux_rev,
                ["crates/solstone-linux/src/upload.rs"],
                "/app/observer/ingest",
                "bundled",
                "Projected as observer.ingestUpload for Linux segment upload.",
            ),
            _audit_path(
                "solstone-linux",
                linux_rev,
                ["crates/solstone-linux/src/upload.rs"],
                "/app/observer/ingest/event",
                "bundled",
                "Projected as observer.ingestEvent for Linux event relay.",
            ),
            _audit_path(
                "solstone-linux",
                linux_rev,
                ["crates/solstone-linux/src/upload.rs"],
                "/app/observer/ingest/segments/{day}",
                "bundled",
                "Projected as observer.ingestSegments for Linux reconciliation.",
            ),
            _audit_path(
                "solstone-linux",
                linux_rev,
                [linux_chat_file],
                "/app/observer/callosum",
                "bundled",
                "Projected as observer.callosumStream for Linux chat bridge SSE.",
            ),
            _audit_path(
                "solstone-linux",
                linux_rev,
                [linux_chat_file],
                "/api/chat/sol_chat_request/open",
                "bundled",
                "Projected as chat.openSolChatRequest for Linux chat bridge.",
            ),
            _audit_path(
                "solstone-linux",
                linux_rev,
                [linux_chat_file],
                "/app/chat/{day}#event-{index}",
                "browser_navigation_excluded_from_api_projection",
                "Browser navigation anchor, not an HTTP API contract path.",
            ),
            _audit_path(
                "solstone-linux",
                linux_rev,
                [linux_chat_file],
                "/api/sol_voice",
                "consumer_drift_adoption_blocker",
                (
                    "Consumer uses the old settings path and reads "
                    "linux_notify_send; journal serves /app/settings/api/sol_voice "
                    "and nests the Linux boolean under system_notifications.linux."
                ),
            ),
            _audit_path(
                "solstone-windows",
                windows_rev,
                windows_observer_files,
                "/app/network/pair",
                "bundled",
                "Projected as link.pair for Windows pairing.",
            ),
            _audit_path(
                "solstone-windows",
                windows_rev,
                windows_observer_files,
                "/app/observer/register",
                "bundled",
                "Projected as observer.register for Windows observer enrollment.",
            ),
            _audit_path(
                "solstone-windows",
                windows_rev,
                windows_observer_files,
                "/app/observer/ingest",
                "bundled",
                "Projected as observer.ingestUpload for Windows segment upload.",
            ),
            _audit_path(
                "solstone-windows",
                windows_rev,
                windows_observer_files,
                "/app/observer/ingest/event",
                "bundled",
                "Projected as observer.ingestEvent for Windows event relay.",
            ),
            _audit_path(
                "solstone-windows",
                windows_rev,
                windows_observer_files,
                "/app/observer/ingest/segments/{day}",
                "bundled",
                "Projected as observer.ingestSegments for Windows reconciliation.",
            ),
            _audit_path(
                "solstone-windows",
                windows_rev,
                ["crates/pl-transport-win/src/journal_bridge.rs"],
                "/sse/events",
                "bundled",
                "Projected as callosum.rootEvents for Windows journal bridge SSE.",
            ),
        ],
        "schema": "solstone.observer-client-consumer-audit.v1",
        "searched_files": [
            _searched_file(
                "solstone-browser",
                browser_rev,
                browser_file,
            ),
            _searched_file(
                "solstone-linux",
                linux_rev,
                "crates/solstone-linux/src/upload.rs",
            ),
            _searched_file("solstone-linux", linux_rev, linux_chat_file),
            _searched_file(
                "solstone-windows",
                windows_rev,
                "crates/observer-pl/src/lib.rs",
            ),
            _searched_file(
                "solstone-windows",
                windows_rev,
                "crates/observer-pl/src/wire.rs",
            ),
            _searched_file(
                "solstone-windows",
                windows_rev,
                "crates/pl-transport-win/src/journal_bridge.rs",
            ),
        ],
        "settings_drift_findings": [
            {
                "consumer": "solstone-linux",
                "id": "linux.sol_voice.path",
                "rationale": (
                    "Linux reads /api/sol_voice, but the journal route is "
                    "/app/settings/api/sol_voice."
                ),
                "status": "adoption_blocker",
                "verified_citations": [
                    "core/crates/solstone-core-settings-web/src/lib.rs:117-121",
                    "core/crates/solstone-core-settings-web/src/router_contracts.rs:1-36",
                ],
            },
            {
                "consumer": "solstone-linux",
                "id": "linux.sol_voice.linux_notify_send",
                "rationale": (
                    "Linux reads top-level linux_notify_send, but the journal "
                    "response exposes system_notifications.linux."
                ),
                "status": "adoption_blocker",
                "verified_citations": [
                    "solstone/convey/sol_initiated/settings.py:41-42",
                    "solstone/convey/sol_initiated/settings.py:61-63",
                    "solstone/convey/sol_initiated/settings.py:211-213",
                    "core/crates/solstone-core-settings-web/src/build_contract.rs:29-37",
                ],
            },
        ],
    }


def _audit_path(
    consumer: str,
    revision: str,
    source_files: list[str],
    path: str,
    classification: str,
    rationale: str,
) -> dict[str, Any]:
    return {
        "classification": classification,
        "consumer": consumer,
        "path": path,
        "rationale": rationale,
        "revision": revision,
        "source_files": source_files,
    }


def _searched_file(consumer: str, revision: str, path: str) -> dict[str, str]:
    return {
        "consumer": consumer,
        "path": path,
        "revision": revision,
        "role": "production",
    }


def build_vocabulary_inventory(projection: dict[str, Any]) -> list[dict[str, Any]]:
    """Return reachable vocabulary classifications for the bundle manifest."""

    vocabularies: dict[str, dict[str, Any]] = {}
    for pointer, node in _walk_dict_nodes(projection):
        vocabulary = node.get("x-vocabulary")
        if isinstance(vocabulary, dict):
            record = _vocabulary_record_from_extension(vocabulary, node)
            record["source_pointer"] = _display_pointer(pointer)
            vocabularies[record["id"]] = record
        grouped = node.get("x-vocabularies")
        if isinstance(grouped, dict):
            for vocab_id, metadata in sorted(grouped.items()):
                if not isinstance(metadata, dict):
                    continue
                record = {"id": vocab_id, **copy.deepcopy(metadata)}
                record["source_pointer"] = _display_pointer(pointer)
                vocabularies[vocab_id] = record
        chat_events = node.get("x-chat-events")
        if isinstance(chat_events, dict):
            record = {
                "id": _extension_id(chat_events, "x-chat-events"),
                "native_client_interest_subset": list(chat_events.get("kinds", [])),
                "source_pointer": _display_pointer(pointer),
            }
            for key in (
                "classification",
                "description",
                "stream_exhaustive",
                "unknown_value_behavior",
            ):
                if key in chat_events:
                    record[key] = copy.deepcopy(chat_events[key])
            vocabularies[record["id"]] = record
        sse_frames = node.get("x-sse-frame-kinds")
        if isinstance(sse_frames, dict):
            record = {
                "id": _extension_id(sse_frames, "x-sse-frame-kinds"),
                **copy.deepcopy(sse_frames),
                "source_pointer": _display_pointer(pointer),
            }
            vocabularies[record["id"]] = record

    for path, methods in projection["paths"].items():
        for method, operation in methods.items():
            for status, response in operation.get("responses", {}).items():
                response_pointer = _response_pointer(path, method, str(status))
                codes = response.get("x-reason-codes")
                if isinstance(codes, list) and codes:
                    vocab_id = (
                        f"{operation['operationId']}.responses.{status}.reason_code"
                    )
                    vocabularies[vocab_id] = {
                        "classification": "closed",
                        "id": vocab_id,
                        "method": method.upper(),
                        "path": path,
                        "source_pointer": response_pointer,
                        "unknown_value_behavior": "reject",
                        "values": sorted(codes),
                    }
                frame = response.get("x-sse-error-frame")
                if isinstance(frame, dict):
                    frame_codes = frame.get("x-reason-codes", [])
                    vocab_id = (
                        f"{operation['operationId']}.responses."
                        f"{status}.sse_error.reason_code"
                    )
                    vocabularies[vocab_id] = {
                        "classification": "closed",
                        "id": vocab_id,
                        "method": method.upper(),
                        "path": path,
                        "source_pointer": _pointer_join(
                            response_pointer,
                            "x-sse-error-frame",
                        ),
                        "unknown_value_behavior": "reject",
                        "values": sorted(frame_codes),
                    }

    return [vocabularies[key] for key in sorted(vocabularies)]


def _iter_operations(
    document: dict[str, Any],
) -> Iterator[tuple[str, str, dict[str, Any]]]:
    for path, methods in document.get("paths", {}).items():
        if not isinstance(methods, dict):
            continue
        for method, operation in methods.items():
            if isinstance(operation, dict):
                yield path, method, operation


def _walk_dict_nodes(
    node: Any, pointer: str = ""
) -> Iterator[tuple[str, dict[str, Any]]]:
    if isinstance(node, dict):
        yield pointer, node
        for key, value in node.items():
            yield from _walk_dict_nodes(value, _pointer_join(pointer, str(key)))
    elif isinstance(node, list):
        for index, value in enumerate(node):
            yield from _walk_dict_nodes(value, _pointer_join(pointer, str(index)))


def _vocabulary_record_from_extension(
    extension: dict[str, Any], schema_node: dict[str, Any]
) -> dict[str, Any]:
    _extension_id(extension, "x-vocabulary")
    record = copy.deepcopy(extension)
    enum_values = schema_node.get("enum")
    if isinstance(enum_values, list):
        record["values"] = list(enum_values)
    elif "const" in schema_node:
        record["values"] = [schema_node["const"]]
    return record


def _extension_id(extension: dict[str, Any], extension_name: str) -> str:
    vocab_id = extension.get("id")
    if not isinstance(vocab_id, str) or not vocab_id:
        raise ObserverBundleError(f"{extension_name} extension missing id")
    return vocab_id


def _response_pointer(path: str, method: str, status: str) -> str:
    return _pointer_join(
        _pointer_join(
            _pointer_join(_pointer_join("/paths", path), method),
            "responses",
        ),
        status,
    )


def _pointer_join(base: str, token: str) -> str:
    escaped = token.replace("~", "~0").replace("/", "~1")
    if not base:
        return f"/{escaped}"
    return f"{base}/{escaped}"


def _display_pointer(pointer: str) -> str:
    return pointer or "/"


def _local_schema_refs(node: Any) -> Iterator[str]:
    if isinstance(node, dict):
        ref = node.get("$ref")
        if isinstance(ref, str):
            name = _schema_name_from_ref(ref)
            if name is not None:
                yield name
        for value in node.values():
            yield from _local_schema_refs(value)
    elif isinstance(node, list):
        for item in node:
            yield from _local_schema_refs(item)


def _schema_name_from_ref(ref: str) -> str | None:
    prefix = "#/components/schemas/"
    if not ref.startswith(prefix):
        return None
    return ref[len(prefix) :]


def _generator_inputs(root: Path) -> list[dict[str, Any]]:
    bundle_root = (root / BUNDLE_REL_DIR).resolve()
    records: list[dict[str, Any]] = []
    for input_id, rel_path, role in _SOURCE_INPUTS:
        path = root / rel_path
        try:
            resolved_path = path.resolve(strict=True)
        except FileNotFoundError as exc:
            raise ObserverBundleError(
                f"generator input path does not exist: {rel_path.as_posix()}"
            ) from exc
        if _is_relative_to(resolved_path, bundle_root):
            raise ObserverBundleError(
                f"generator input {input_id} points inside generated bundle: "
                f"{rel_path.as_posix()}"
            )
        records.append(
            {
                "id": input_id,
                "path": rel_path.as_posix(),
                "role": role,
                "sha256": _sha256_path(path),
            }
        )
    return sorted(records, key=lambda item: item["id"])


def _sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _sha256_path(path: Path) -> str:
    parent_fd, leaf_name = _open_parent_dir_no_follow(path)
    try:
        try:
            leaf_stat = os.stat(leaf_name, dir_fd=parent_fd, follow_symlinks=False)
        except FileNotFoundError as exc:
            raise ObserverBundleError(
                f"generator input path does not exist: {path}"
            ) from exc
        mode = leaf_stat.st_mode
        if stat.S_ISLNK(mode):
            digest = _new_tree_digest()
            _digest_record(
                digest,
                kind=b"symlink",
                rel_parts=(),
                value=_readlink_at_stable(parent_fd, leaf_name, leaf_stat),
            )
            return digest.hexdigest()
        if stat.S_ISREG(mode):
            return _sha256_regular_at_no_follow(parent_fd, leaf_name, leaf_stat)
        if not stat.S_ISDIR(mode):
            raise ObserverBundleError(f"generator input is not regular: {path}")
        # Directory inputs are supported for defensive/future generator-input trees.
        # Current declared inputs are files, but verification may inspect older or tampered manifests.
        root_fd = _open_dir_at_no_follow(parent_fd, leaf_name, leaf_stat)
    finally:
        os.close(parent_fd)
    digest = _new_tree_digest()
    try:
        _digest_directory_fd(digest, root_fd, (), os.fstat(root_fd))
    finally:
        os.close(root_fd)
    return digest.hexdigest()


def _new_tree_digest() -> Any:
    digest = hashlib.sha256()
    _digest_frame(digest, b"domain", b"solstone-observer-bundle-input-tree-v1")
    return digest


def _digest_directory_fd(
    digest: Any,
    dir_fd: int,
    rel_parts: tuple[bytes, ...],
    expected_stat: os.stat_result,
) -> None:
    before_dir = _verify_opened_stat(
        dir_fd,
        expected_stat,
        _display_rel_parts(rel_parts),
    )
    before_entries = _directory_entry_snapshot(dir_fd)
    for name, entry_stat in before_entries:
        entry_parts = (*rel_parts, name)
        mode = entry_stat.st_mode
        if stat.S_ISLNK(mode):
            _digest_record(
                digest,
                kind=b"symlink",
                rel_parts=entry_parts,
                value=_readlink_at_stable(dir_fd, name, entry_stat),
            )
            continue
        if stat.S_ISDIR(mode):
            _digest_record(digest, kind=b"dir", rel_parts=entry_parts, value=b"")
            child_fd = _open_dir_at_no_follow(dir_fd, name, entry_stat)
            try:
                _digest_directory_fd(digest, child_fd, entry_parts, os.fstat(child_fd))
            finally:
                os.close(child_fd)
            continue
        if stat.S_ISREG(mode):
            file_digest = _sha256_regular_at_no_follow(dir_fd, name, entry_stat)
            _digest_record(
                digest,
                kind=b"file",
                rel_parts=entry_parts,
                value=file_digest.encode("ascii"),
            )
            continue
        raise ObserverBundleError(
            "generator input is not regular: " + _display_rel_parts(entry_parts)
        )
    after_entries = _directory_entry_snapshot(dir_fd)
    if _entry_snapshot_identity(after_entries) != _entry_snapshot_identity(
        before_entries
    ):
        raise ObserverBundleError(
            "generator input directory entries changed while being read: "
            + _display_rel_parts(rel_parts)
        )
    if _stat_identity(os.fstat(dir_fd)) != _stat_identity(before_dir):
        raise ObserverBundleError(
            "generator input directory changed while being read: "
            + _display_rel_parts(rel_parts)
        )


def _open_parent_dir_no_follow(path: Path) -> tuple[int, bytes]:
    parts = path.parts
    if not parts:
        raise ObserverBundleError("generator input path is empty")
    if path.is_absolute():
        fd = _open_trusted_dir(Path(path.anchor))
        component_parts = parts[1:]
    else:
        fd = _open_trusted_dir(Path("."))
        component_parts = parts
    if not component_parts:
        return fd, b"."
    for component in component_parts[:-1]:
        name = os.fsencode(component)
        if name in {b"", b".", b".."}:
            os.close(fd)
            raise ObserverBundleError(f"generator input path is unsafe: {path}")
        try:
            entry_stat = os.stat(name, dir_fd=fd, follow_symlinks=False)
            if stat.S_ISLNK(entry_stat.st_mode):
                raise ObserverBundleError(
                    "generator input path component is a symlink: " + os.fsdecode(name)
                )
            child_fd = _open_dir_at_no_follow(fd, name, entry_stat)
        except Exception:
            os.close(fd)
            raise
        os.close(fd)
        fd = child_fd
    leaf_name = os.fsencode(component_parts[-1])
    if leaf_name in {b"", b".", b".."}:
        os.close(fd)
        raise ObserverBundleError(f"generator input path is unsafe: {path}")
    return fd, leaf_name


def _open_trusted_dir(path: Path) -> int:
    try:
        fd = os.open(path, _dir_open_flags())
    except OSError as exc:
        raise ObserverBundleError(
            f"trusted directory cannot be opened: {path}"
        ) from exc
    if not stat.S_ISDIR(os.fstat(fd).st_mode):
        os.close(fd)
        raise ObserverBundleError(f"trusted path is not a directory: {path}")
    return fd


def _open_dir_at_no_follow(
    parent_fd: int, name: bytes, expected_stat: os.stat_result
) -> int:
    try:
        fd = os.open(name, _dir_open_flags(), dir_fd=parent_fd)
    except OSError as exc:
        raise ObserverBundleError(
            "generator input directory cannot be opened: " + os.fsdecode(name)
        ) from exc
    try:
        _verify_opened_stat(fd, expected_stat, os.fsdecode(name))
        return fd
    except Exception:
        os.close(fd)
        raise


def _sha256_regular_at_no_follow(
    parent_fd: int, name: bytes, expected_stat: os.stat_result
) -> str:
    payload = _read_regular_at_no_follow(parent_fd, name, expected_stat)
    return hashlib.sha256(payload).hexdigest()


def _read_regular_at_no_follow(
    parent_fd: int, name: bytes, expected_stat: os.stat_result
) -> bytes:
    try:
        fd = os.open(name, _file_open_flags(), dir_fd=parent_fd)
    except OSError as exc:
        raise ObserverBundleError(
            "file cannot be opened without following symlinks: " + os.fsdecode(name)
        ) from exc
    try:
        before = _verify_opened_stat(fd, expected_stat, os.fsdecode(name))
        chunks: list[bytes] = []
        while True:
            chunk = os.read(fd, 1024 * 1024)
            if not chunk:
                break
            chunks.append(chunk)
        after = os.fstat(fd)
        if _stat_identity(after) != _stat_identity(before):
            raise ObserverBundleError(
                "file changed while being read: " + os.fsdecode(name)
            )
        return b"".join(chunks)
    finally:
        os.close(fd)


def _verify_opened_stat(
    fd: int, expected_stat: os.stat_result, label: str
) -> os.stat_result:
    opened_stat = os.fstat(fd)
    expected_is_dir = stat.S_ISDIR(expected_stat.st_mode)
    opened_matches_expected = (
        _node_identity(opened_stat) == _node_identity(expected_stat)
        if expected_is_dir
        else _stat_identity(opened_stat) == _stat_identity(expected_stat)
    )
    if not opened_matches_expected:
        raise ObserverBundleError(f"generator input changed before open: {label}")
    if not (stat.S_ISREG(opened_stat.st_mode) or stat.S_ISDIR(opened_stat.st_mode)):
        raise ObserverBundleError(f"generator input is not regular: {label}")
    return opened_stat


def _readlink_at_stable(
    parent_fd: int, name: bytes, expected_stat: os.stat_result
) -> bytes:
    before = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    if _stat_identity(before) != _stat_identity(expected_stat):
        raise ObserverBundleError(
            "symlink changed before readlink: " + os.fsdecode(name)
        )
    try:
        target = os.readlink(name, dir_fd=parent_fd)
    except OSError as exc:
        raise ObserverBundleError(
            "symlink cannot be read: " + os.fsdecode(name)
        ) from exc
    after = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    if _stat_identity(after) != _stat_identity(before):
        raise ObserverBundleError(
            "symlink changed while being read: " + os.fsdecode(name)
        )
    return os.fsencode(target)


def _directory_entry_snapshot(dir_fd: int) -> list[tuple[bytes, os.stat_result]]:
    entries: list[tuple[bytes, os.stat_result]] = []
    list_fd = _open_dir_at_no_follow(dir_fd, b".", os.fstat(dir_fd))
    try:
        raw_names = os.listdir(list_fd)
    finally:
        os.close(list_fd)
    for raw_name in raw_names:
        name = os.fsencode(raw_name)
        try:
            entry_stat = os.stat(name, dir_fd=dir_fd, follow_symlinks=False)
        except OSError as exc:
            raise ObserverBundleError(
                f"directory entry cannot be inspected: {os.fsdecode(name)}"
            ) from exc
        entries.append((name, entry_stat))
    return sorted(entries, key=lambda item: item[0])


def _entry_snapshot_identity(
    entries: list[tuple[bytes, os.stat_result]],
) -> tuple[tuple[bytes, tuple[int, int, int, int, int, int]], ...]:
    return tuple((name, _stat_identity(entry_stat)) for name, entry_stat in entries)


def _stat_identity(value: os.stat_result) -> tuple[int, int, int, int, int, int]:
    return (
        value.st_dev,
        value.st_ino,
        value.st_mode,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
    )


def _node_identity(value: os.stat_result) -> tuple[int, int, int]:
    return (value.st_dev, value.st_ino, value.st_mode)


def _dir_open_flags() -> int:
    return (
        os.O_RDONLY
        | getattr(os, "O_DIRECTORY", 0)
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
    )


def _file_open_flags() -> int:
    return (
        os.O_RDONLY
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_NONBLOCK", 0)
    )


def _digest_record(
    digest: Any,
    *,
    kind: bytes,
    rel_parts: tuple[bytes, ...],
    value: bytes,
) -> None:
    _digest_frame(digest, b"record", b"")
    _digest_frame(digest, b"kind", kind)
    _digest_frame(digest, b"path-components", len(rel_parts).to_bytes(8, "big"))
    for part in rel_parts:
        _digest_frame(digest, b"path-component", part)
    _digest_frame(digest, b"value", value)


def _digest_frame(digest: Any, label: bytes, value: bytes) -> None:
    digest.update(len(label).to_bytes(8, "big"))
    digest.update(label)
    digest.update(len(value).to_bytes(8, "big"))
    digest.update(value)


def _display_rel_parts(parts: tuple[bytes, ...]) -> str:
    if not parts:
        return "."
    return "/".join(os.fsdecode(part) for part in parts)


def _repo_root(root: Path | None) -> Path:
    if root is not None:
        return Path(root).resolve()
    return Path(__file__).resolve().parents[3]


def _is_relative_to(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
    except ValueError:
        return False
    return True


__all__ = [
    "BUNDLE_REL_DIR",
    "BUNDLE_SEMVER",
    "BundleCompatibilityError",
    "BundleExportRefused",
    "BundleSnapshot",
    "BundleVerificationError",
    "CONSUMER_AUDIT_REL",
    "FIXTURES_REL",
    "INITIAL_BUNDLE_SEMVER",
    "MANIFEST_REL",
    "OBSERVER_CLIENT_OPERATION_IDS",
    "PROJECTION_REL",
    "VECTORS_REL",
    "build_bundle_files",
    "build_projection_document",
    "compare_semver",
    "component_ref_closure",
    "parse_semver",
    "render_json",
    "stale_bundle_paths",
    "validate_projection_refs",
]
