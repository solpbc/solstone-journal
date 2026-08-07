#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Check native-sol parity coverage against current HTTP authorities."""

from __future__ import annotations

import json
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any

try:
    from scripts.build_native_sol_inventory import (
        FINAL_HTTP_TOTAL,
        FINAL_STUB_COUNTS,
        FINAL_TOP_LEVEL_CHAT_TOTAL,
        FINAL_TOP_LEVEL_IMPORT_TOTAL,
        FINAL_TOP_LEVEL_LINK_TOTAL,
        FINAL_TOP_LEVEL_STATUS_TOTAL,
        REPO_ROOT,
        discover,
    )
except ModuleNotFoundError:  # pragma: no cover - direct script execution path.
    from build_native_sol_inventory import (  # type: ignore[no-redef]
        FINAL_HTTP_TOTAL,
        FINAL_STUB_COUNTS,
        FINAL_TOP_LEVEL_CHAT_TOTAL,
        FINAL_TOP_LEVEL_IMPORT_TOTAL,
        FINAL_TOP_LEVEL_LINK_TOTAL,
        FINAL_TOP_LEVEL_STATUS_TOTAL,
        REPO_ROOT,
        discover,
    )

SCHEMA = "native-sol-applicability-v1"
APPLICABILITY = REPO_ROOT / "core/fixtures/native-sol/applicability.json"
PARITY_DIR = REPO_ROOT / "core/fixtures/native-sol/parity"
RUST_MANIFEST = REPO_ROOT / "core/Cargo.toml"


def main() -> int:
    errors = check_coverage()
    if errors:
        print("native sol coverage failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("native sol coverage ok")
    return 0


def check_coverage(root: Path = REPO_ROOT) -> list[str]:
    entries = discover(root)
    required = {
        entry.operation_id
        for entry in entries
        if entry.surface == "sol-call" and entry.entry_type == "http"
    }
    required_stubs = {
        entry.operation_id
        for entry in entries
        if entry.surface == "sol-call" and entry.entry_type in {"moved-stub", "local"}
    }
    required_top_level_chat = {
        entry.operation_id
        for entry in entries
        if entry.surface == "sol-chat" and entry.entry_type == "top-level-chat"
    }
    required_top_level_import = {
        entry.operation_id
        for entry in entries
        if entry.surface == "sol-import" and entry.entry_type == "top-level-import"
    }
    required_top_level_status = {
        entry.operation_id
        for entry in entries
        if entry.surface == "sol-status" and entry.entry_type == "top-level-status"
    }
    required_top_level_link = {
        entry.operation_id
        for entry in entries
        if entry.surface == "sol-link" and entry.entry_type == "top-level-link"
    }
    required_dispatch = (
        required
        | required_stubs
        | required_top_level_chat
        | required_top_level_import
        | required_top_level_link
        | required_top_level_status
    )
    vectors = load_vectors(PARITY_DIR)
    resolved = resolve_vectors(PARITY_DIR, vectors)
    applicability, applicability_errors = load_applicability(APPLICABILITY)

    errors: list[str] = []
    errors.extend(applicability_errors)
    if len(required) != FINAL_HTTP_TOTAL:
        errors.append(
            f"current HTTP authority count {len(required)} != {FINAL_HTTP_TOTAL}"
        )
    stub_counts = Counter(
        entry.entry_type
        for entry in entries
        if entry.surface == "sol-call" and entry.entry_type in FINAL_STUB_COUNTS
    )
    for entry_type, expected in sorted(FINAL_STUB_COUNTS.items()):
        if stub_counts[entry_type] != expected:
            errors.append(
                f"current {entry_type} authority count {stub_counts[entry_type]} "
                f"!= {expected}"
            )
    if len(required_top_level_chat) != FINAL_TOP_LEVEL_CHAT_TOTAL:
        errors.append(
            f"current top-level chat authority count {len(required_top_level_chat)} "
            f"!= {FINAL_TOP_LEVEL_CHAT_TOTAL}"
        )
    if not applicability_errors:
        entries = applicability["entries"]
        keys = set(entries)
        http_count = applicability.get("http_count")
        if http_count != FINAL_HTTP_TOTAL:
            errors.append(
                f"applicability http_count {http_count!r} != {FINAL_HTTP_TOTAL}"
            )
        if http_count != len(entries):
            errors.append(
                f"applicability http_count {http_count!r} != entries {len(entries)}"
            )
        if http_count != len(required):
            errors.append(
                f"applicability http_count {http_count!r} != current HTTP "
                f"authority count {len(required)}"
            )
        errors.extend(compare_sets("applicability keys", required, keys))
        top_level_entries = applicability.get("top_level_entries", {})
        if not isinstance(top_level_entries, dict):
            errors.append("applicability top_level_entries must be an object")
            top_level_entries = {}
        import_keys = {
            operation_id
            for operation_id in top_level_entries
            if operation_id in required_top_level_import
        }
        errors.extend(
            compare_sets(
                "top-level import applicability keys",
                required_top_level_import,
                import_keys,
            )
        )
        status_keys = {
            operation_id
            for operation_id in top_level_entries
            if operation_id in required_top_level_status
        }
        errors.extend(
            compare_sets(
                "top-level status applicability keys",
                required_top_level_status,
                status_keys,
            )
        )

    if len(required_top_level_import) != FINAL_TOP_LEVEL_IMPORT_TOTAL:
        errors.append(
            f"current top-level import authority count {len(required_top_level_import)} "
            f"!= {FINAL_TOP_LEVEL_IMPORT_TOTAL}"
        )
    if len(required_top_level_status) != FINAL_TOP_LEVEL_STATUS_TOTAL:
        errors.append(
            f"current top-level status authority count {len(required_top_level_status)} "
            f"!= {FINAL_TOP_LEVEL_STATUS_TOTAL}"
        )
    if len(required_top_level_link) != FINAL_TOP_LEVEL_LINK_TOTAL:
        errors.append(
            f"current top-level link authority count {len(required_top_level_link)} "
            f"!= {FINAL_TOP_LEVEL_LINK_TOTAL}"
        )
    if not required_dispatch:
        errors.append("native dispatch authority set is empty")
    resolved_operations = {
        str(item["operation_id"])
        for item in resolved.values()
        if item.get("operation_id") is not None
    }
    missing_dispatch = sorted(required_dispatch - resolved_operations)
    if missing_dispatch:
        errors.append(
            "production aggregate dispatch missing required operations "
            f"{missing_dispatch!r}"
        )

    buckets = collect_buckets(vectors, resolved, required, {"http"}, errors)
    for bucket_name in ("request_binding", "success", "failure"):
        errors.extend(compare_sets(bucket_name, required, buckets[bucket_name]))
    import_buckets = collect_buckets(
        vectors,
        resolved,
        required_top_level_import,
        {"top-level-import"},
        errors,
    )
    for bucket_name in ("request_binding", "success", "failure"):
        errors.extend(
            compare_sets(
                f"top-level import {bucket_name}",
                required_top_level_import,
                import_buckets[bucket_name],
            )
        )
    status_buckets = collect_buckets(
        vectors,
        resolved,
        required_top_level_status,
        {"top-level-status"},
        errors,
    )
    for bucket_name in ("request_binding", "success", "failure"):
        errors.extend(
            compare_sets(
                f"top-level status {bucket_name}",
                required_top_level_status,
                status_buckets[bucket_name],
            )
        )
    link_buckets = collect_buckets(
        vectors,
        resolved,
        required_top_level_link,
        {"top-level-link"},
        errors,
    )
    for bucket_name in ("success", "failure"):
        errors.extend(
            compare_sets(
                f"top-level link {bucket_name}",
                required_top_level_link,
                link_buckets[bucket_name],
            )
        )

    if not applicability_errors:
        errors.extend(
            check_applicability_requirements(
                applicability["entries"], vectors, resolved, buckets
            )
        )
        errors.extend(
            check_top_level_cases(
                applicability.get("top_level_entries", {}),
                vectors,
                resolved,
            )
        )
    return errors


def load_applicability(path: Path) -> tuple[dict[str, Any], list[str]]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}, [f"{path.relative_to(REPO_ROOT)} is missing"]
    except json.JSONDecodeError as error:
        return {}, [f"{path.relative_to(REPO_ROOT)} is malformed JSON: {error}"]
    errors: list[str] = []
    if payload.get("schema") != SCHEMA:
        errors.append(f"applicability schema must be {SCHEMA!r}")
    entries = payload.get("entries")
    if not isinstance(entries, dict):
        errors.append("applicability entries must be an object")
    else:
        for operation_id, entry in entries.items():
            if not isinstance(operation_id, str) or not operation_id:
                errors.append("applicability operation ids must be non-empty strings")
            if not isinstance(entry, dict):
                errors.append(f"{operation_id}: applicability entry must be an object")
                continue
            for key in (
                "path",
                "group",
                "pagination",
                "mutation",
                "upload",
                "env_default",
                "confirmation",
                "consent",
                "dry_run",
                "multi_request",
            ):
                if key not in entry:
                    errors.append(f"{operation_id}: missing applicability field {key}")
    top_level_entries = payload.get("top_level_entries", {})
    if top_level_entries is not None and not isinstance(top_level_entries, dict):
        errors.append("applicability top_level_entries must be an object")
    return payload, errors


def load_vectors(directory: Path) -> dict[str, dict[str, Any]]:
    vectors: dict[str, dict[str, Any]] = {}
    for path in sorted(directory.glob("*.jsonl")):
        for line_number, line in enumerate(
            path.read_text(encoding="utf-8").splitlines(), 1
        ):
            if not line.strip():
                continue
            vector = json.loads(line)
            vector_id = str(vector.get("id") or "")
            if not vector_id:
                raise ValueError(
                    f"{path.relative_to(REPO_ROOT)}:{line_number}: missing id"
                )
            if vector_id in vectors:
                raise ValueError(
                    f"{path.relative_to(REPO_ROOT)}:{line_number}: duplicate id {vector_id}"
                )
            vector["_fixture"] = str(path.relative_to(REPO_ROOT))
            vectors[vector_id] = vector
    return vectors


def resolve_vectors(
    directory: Path, vectors: dict[str, dict[str, Any]]
) -> dict[str, dict[str, Any]]:
    paths = sorted(directory.glob("*.jsonl"))
    completed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(RUST_MANIFEST),
            "-p",
            "solstone-core-sol-client-cli",
            "--bin",
            "resolve_parity_leaves",
            "--",
            *(str(path) for path in paths),
        ],
        check=False,
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            "production parity leaf resolver failed:\n" + completed.stderr
        )
    resolved: dict[str, dict[str, Any]] = {}
    for line in completed.stdout.splitlines():
        if not line.strip():
            continue
        item = json.loads(line)
        vector_id = str(item["id"])
        if vector_id not in vectors:
            raise RuntimeError(f"resolver returned unknown vector id {vector_id}")
        resolved[vector_id] = item
    missing = sorted(set(vectors) - set(resolved))
    if missing:
        raise RuntimeError(f"resolver omitted vectors {missing!r}")
    return resolved


def collect_buckets(
    vectors: dict[str, dict[str, Any]],
    resolved: dict[str, dict[str, Any]],
    required: set[str],
    entry_types: set[str],
    errors: list[str],
) -> dict[str, set[str]]:
    buckets: dict[str, set[str]] = {
        "request_binding": set(),
        "success": set(),
        "failure": set(),
    }
    for vector_id, vector in vectors.items():
        operation_id = resolved[vector_id].get("operation_id")
        entry_type = resolved[vector_id].get("entry_type")
        if vector.get("surface", "sol-call") == "sol-call" and operation_id is None:
            errors.append(
                f"{vector_id}: argv did not resolve through production dispatch"
            )
            continue
        if entry_type not in entry_types or operation_id not in required:
            continue
        expected = vector.get("expected") or {}
        requests = expected.get("requests") if isinstance(expected, dict) else None
        if pins_request_shape(requests):
            buckets["request_binding"].add(operation_id)
        if is_failure_vector(vector, expected):
            buckets["failure"].add(operation_id)
        elif is_success_vector(
            expected, vector.get("transport", {}).get("requests", [])
        ):
            buckets["success"].add(operation_id)
    return buckets


def pins_request_shape(requests: Any) -> bool:
    if not isinstance(requests, list):
        return False
    return any(
        isinstance(request, dict)
        and isinstance(request.get("method"), str)
        and isinstance(request.get("path"), str)
        for request in requests
    )


def is_failure_vector(vector: dict[str, Any], expected: Any) -> bool:
    if isinstance(expected, dict) and expected.get("exit") != 0:
        return True
    for request in vector.get("transport", {}).get("requests", []):
        if not isinstance(request, dict):
            continue
        if "fault" in request:
            return True
        status = (
            request.get("response", {}).get("status")
            if isinstance(request.get("response"), dict)
            else None
        )
        if status is not None and not (200 <= int(status) <= 299):
            return True
    return False


def is_success_vector(expected: Any, transport_requests: Any) -> bool:
    if not isinstance(expected, dict) or expected.get("exit") != 0:
        return False
    if not isinstance(transport_requests, list):
        return False
    for request in transport_requests:
        if not isinstance(request, dict) or "fault" in request:
            return False
        response = request.get("response")
        if not isinstance(response, dict):
            return False
        status = int(response.get("status", 200))
        if not (200 <= status <= 299):
            return False
    return True


def compare_sets(label: str, required: set[str], actual: set[str]) -> list[str]:
    errors: list[str] = []
    missing = sorted(required - actual)
    extra = sorted(actual - required)
    if missing:
        errors.append(f"{label} missing {missing!r}")
    if extra:
        errors.append(f"{label} extra {extra!r}")
    return errors


def check_top_level_cases(
    entries: Any,
    vectors: dict[str, dict[str, Any]],
    resolved: dict[str, dict[str, Any]],
) -> list[str]:
    if not isinstance(entries, dict):
        return []
    errors: list[str] = []
    for operation_id, entry in sorted(entries.items()):
        if not isinstance(entry, dict):
            errors.append(
                f"{operation_id}: top-level applicability entry must be an object"
            )
            continue
        case_ids = entry.get("case_ids", {})
        if not isinstance(case_ids, dict):
            errors.append(f"{operation_id}: top-level case_ids must be an object")
            continue
        for case_name, ids in sorted(case_ids.items()):
            if not isinstance(ids, list) or not ids:
                errors.append(
                    f"{operation_id}: top-level case {case_name} must be non-empty"
                )
                continue
            for vector_id in ids:
                if vector_id not in vectors:
                    errors.append(
                        f"{operation_id}: top-level case {case_name} unknown vector {vector_id!r}"
                    )
                    continue
                mapped = resolved[vector_id].get("operation_id")
                if mapped != operation_id:
                    errors.append(
                        f"{operation_id}: top-level case {case_name} vector "
                        f"{vector_id!r} maps to {mapped!r}"
                    )
    return errors


def check_applicability_requirements(
    entries: dict[str, dict[str, Any]],
    vectors: dict[str, dict[str, Any]],
    resolved: dict[str, dict[str, Any]],
    buckets: dict[str, set[str]],
) -> list[str]:
    errors: list[str] = []
    for operation_id, entry in sorted(entries.items()):
        for dimension in (
            "pagination",
            "upload",
            "env_default",
            "dry_run",
            "multi_request",
        ):
            payload = entry.get(dimension)
            if not isinstance(payload, dict) or not payload.get("enabled"):
                continue
            errors.extend(
                check_named_cases(operation_id, dimension, payload, vectors, resolved)
            )
        if bool(entry.get("upload", {}).get("enabled")):
            upload = entry["upload"]
            operation_vectors = mapped_vectors(vectors, resolved, operation_id)
            if not any(
                "UPLOAD" in expected_request_methods(vector)
                for vector in operation_vectors
            ):
                errors.append(f"{operation_id}: upload=true but no UPLOAD vector")
            errors.extend(
                check_required_case_names(
                    operation_id,
                    "upload",
                    upload,
                    ["missing_file", "unreadable_file", "rejection_before_mutation"],
                )
            )
            if upload.get("file_count") == "multi" and not any(
                has_later_upload_failure(vector) for vector in operation_vectors
            ):
                errors.append(
                    f"{operation_id}: upload multi has no later-file-failure vector"
                )
        if bool(entry.get("multi_request", {}).get("enabled")):
            multi_request = entry["multi_request"]
            operation_vectors = mapped_vectors(vectors, resolved, operation_id)
            if not any(
                expected_request_count(vector) > 1 for vector in operation_vectors
            ):
                errors.append(
                    f"{operation_id}: multi_request=true but no multi-request vector"
                )
            if multi_request_can_exceed_one(
                multi_request.get("request_count")
            ) and not any(
                has_later_boundary_failure(vector) for vector in operation_vectors
            ):
                errors.append(
                    f"{operation_id}: multi_request has no later-boundary-failure vector"
                )
        if bool(entry.get("env_default", {}).get("enabled")):
            env_default = entry["env_default"]
            mode = env_default.get("mode", "required")
            if mode in {"valid_absent", "valid-absent"}:
                required_cases = ["explicit", "valid_absent"]
            elif mode == "required":
                required_cases = ["explicit", "absent"]
            elif mode in {"required_with_invalid", "required-with-invalid"}:
                required_cases = ["explicit", "absent", "invalid"]
            else:
                errors.append(
                    f"{operation_id}: env_default.mode {mode!r} is unsupported"
                )
                required_cases = []
            errors.extend(
                check_required_case_names(
                    operation_id, "env_default", env_default, required_cases
                )
            )
        if bool(entry.get("dry_run", {}).get("enabled")):
            if operation_id not in buckets["success"]:
                errors.append(f"{operation_id}: dry_run=true but no success coverage")
            if operation_id not in buckets["failure"]:
                errors.append(f"{operation_id}: dry_run=true but no failure coverage")
    return errors


def check_required_case_names(
    operation_id: str,
    dimension: str,
    payload: dict[str, Any],
    required_cases: list[str],
) -> list[str]:
    case_ids = payload.get("case_ids", {})
    if not isinstance(case_ids, dict):
        return []
    errors = []
    for case_name in required_cases:
        ids = case_ids.get(case_name)
        if not isinstance(ids, list) or not ids:
            errors.append(
                f"{operation_id}: {dimension} missing required case {case_name}"
            )
    return errors


def check_named_cases(
    operation_id: str,
    dimension: str,
    payload: dict[str, Any],
    vectors: dict[str, dict[str, Any]],
    resolved: dict[str, dict[str, Any]],
) -> list[str]:
    errors: list[str] = []
    case_ids = payload.get("case_ids", {})
    if not isinstance(case_ids, dict):
        errors.append(f"{operation_id}: {dimension}.case_ids must be an object")
        return errors
    for case_name, ids in sorted(case_ids.items()):
        if not isinstance(ids, list) or not ids:
            errors.append(
                f"{operation_id}: {dimension}.case_ids.{case_name} must be non-empty"
            )
            continue
        for vector_id in ids:
            if vector_id not in vectors:
                errors.append(
                    f"{operation_id}: {dimension}.case_ids.{case_name} unknown "
                    f"vector {vector_id!r}"
                )
                continue
            mapped = resolved[vector_id].get("operation_id")
            if mapped != operation_id:
                errors.append(
                    f"{operation_id}: {dimension}.case_ids.{case_name} vector "
                    f"{vector_id!r} maps to {mapped!r}"
                )
    return errors


def mapped_vectors(
    vectors: dict[str, dict[str, Any]],
    resolved: dict[str, dict[str, Any]],
    operation_id: str,
) -> list[dict[str, Any]]:
    return [
        vector
        for vector_id, vector in vectors.items()
        if resolved[vector_id].get("operation_id") == operation_id
    ]


def expected_request_methods(vector: dict[str, Any]) -> set[str]:
    requests = vector.get("expected", {}).get("requests", [])
    methods: set[str] = set()
    for request in requests if isinstance(requests, list) else []:
        if isinstance(request, dict) and isinstance(request.get("method"), str):
            methods.add(request["method"])
    return methods


def expected_request_count(vector: dict[str, Any]) -> int:
    requests = vector.get("expected", {}).get("requests", [])
    return len(requests) if isinstance(requests, list) else 0


def multi_request_can_exceed_one(value: Any) -> bool:
    if isinstance(value, int):
        return value > 1
    if not isinstance(value, str):
        return False
    if value == "single":
        return False
    if ".." not in value:
        try:
            return int(value) > 1
        except ValueError:
            return True
    _minimum, maximum = value.split("..", 1)
    if maximum == "n":
        return True
    try:
        return int(maximum) > 1
    except ValueError:
        return True


def has_later_boundary_failure(vector: dict[str, Any]) -> bool:
    requests = transport_requests(vector)
    for index, request in enumerate(requests):
        if index == 0 or not request_failed(request):
            continue
        if all(request_succeeded(previous) for previous in requests[:index]):
            return True
    return False


def has_later_upload_failure(vector: dict[str, Any]) -> bool:
    requests = transport_requests(vector)
    upload_indices = [
        index
        for index, request in enumerate(requests)
        if isinstance(request, dict) and request.get("method") == "UPLOAD"
    ]
    for upload_position, request_index in enumerate(upload_indices):
        if upload_position == 0 or not request_failed(requests[request_index]):
            continue
        prior_uploads = (requests[index] for index in upload_indices[:upload_position])
        if all(request_succeeded(request) for request in prior_uploads):
            return True
    return False


def transport_requests(vector: dict[str, Any]) -> list[dict[str, Any]]:
    requests = vector.get("transport", {}).get("requests", [])
    return [request for request in requests if isinstance(request, dict)]


def request_failed(request: dict[str, Any]) -> bool:
    if "fault" in request:
        return True
    status = response_status(request)
    return status is not None and not (200 <= status <= 299)


def request_succeeded(request: dict[str, Any]) -> bool:
    if "fault" in request:
        return False
    status = response_status(request)
    return status is not None and 200 <= status <= 299


def response_status(request: dict[str, Any]) -> int | None:
    response = request.get("response")
    if not isinstance(response, dict):
        return None
    return int(response.get("status", 200))


if __name__ == "__main__":
    raise SystemExit(main())
