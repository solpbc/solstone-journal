#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Check native-sol migrated contracts against their Flask routes."""

from __future__ import annotations

import sys
from typing import Any

from flask import Flask

from solstone.convey.contract.assemble import build_document, rule_to_openapi_path

try:
    from scripts.build_native_sol_inventory import REPO_ROOT, AuthorityEntry, discover
    from scripts.check_native_sol_conformance import register_native_blueprints
except ModuleNotFoundError:  # pragma: no cover - direct script execution path.
    from build_native_sol_inventory import (  # type: ignore[no-redef]
        REPO_ROOT,
        AuthorityEntry,
        discover,
    )
    from check_native_sol_conformance import (  # type: ignore[no-redef]
        register_native_blueprints,
    )


def _route_app() -> Flask:
    app = Flask(__name__)
    register_native_blueprints(app)
    return app


def _flask_routes(app: Flask) -> set[tuple[str, str]]:
    routes: set[tuple[str, str]] = set()
    for rule in app.url_map.iter_rules():
        for method in sorted(rule.methods or ()):
            if method in {"HEAD", "OPTIONS"}:
                continue
            routes.add((method, rule_to_openapi_path(rule.rule)))
    return routes


def _contract_operations(
    document: dict[str, Any],
) -> tuple[dict[tuple[str, str], str], list[str]]:
    operations: dict[tuple[str, str], str] = {}
    duplicate_errors: list[str] = []
    seen_operation_ids: dict[str, tuple[str, str]] = {}
    for path, path_item in document["paths"].items():
        for raw_method, operation in path_item.items():
            method = raw_method.upper()
            operation_id = str(operation.get("operationId", ""))
            key = (method, path)
            operations[key] = operation_id
            if operation_id in seen_operation_ids:
                duplicate_errors.append(
                    "duplicate contract operation_id "
                    f"{operation_id}: {seen_operation_ids[operation_id]} and {key}"
                )
            seen_operation_ids[operation_id] = key
    return operations, duplicate_errors


def _expected_routes(
    authorities: list[AuthorityEntry] | None = None,
) -> tuple[dict[tuple[str, str], str | None], list[str]]:
    expected: dict[tuple[str, str], str | None] = {}
    errors: list[str] = []
    for entry in authorities if authorities is not None else discover(REPO_ROOT):
        if entry.surface != "sol-call" or entry.entry_type != "http":
            continue
        if (
            entry.method is None
            or entry.route is None
        ):
            errors.append(
                f"{entry.operation_id}: HTTP authority is missing method or route"
            )
            continue
        key = (entry.method, entry.route)
        existing = expected.get(key)
        if key in expected and existing != entry.contract_operation_id:
            errors.append(
                f"{entry.operation_id}: duplicate native route {entry.method} "
                f"{entry.route} already bound to {existing}"
            )
            continue
        expected[key] = entry.contract_operation_id
    return expected, errors


def main() -> int:
    expected, errors = _expected_routes()
    expected_operation_ids = {
        operation_id for operation_id in expected.values() if operation_id is not None
    }
    flask_routes = _flask_routes(_route_app())
    contract_routes, contract_errors = _contract_operations(build_document())
    errors.extend(contract_errors)

    for key, operation_id in sorted(expected.items()):
        if key not in flask_routes:
            errors.append(f"missing Flask route for {key[0]} {key[1]}")
        if operation_id is None:
            continue
        actual_operation_id = contract_routes.get(key)
        if actual_operation_id is None:
            errors.append(
                f"missing contract operation for {key[0]} {key[1]} "
                f"(expected {operation_id})"
            )
        elif actual_operation_id != operation_id:
            errors.append(
                f"contract operation mismatch for {key[0]} {key[1]}: "
                f"expected {operation_id}, found {actual_operation_id}"
            )

    for key, operation_id in sorted(contract_routes.items()):
        if operation_id not in expected_operation_ids:
            continue
        expected_key = next(
            expected_key
            for expected_key, expected_id in expected.items()
            if expected_id == operation_id
        )
        if key != expected_key:
            errors.append(
                f"migrated contract operation {operation_id} is bound to "
                f"{key[0]} {key[1]}, expected {expected_key[0]} {expected_key[1]}"
            )

    for key, operation_id in sorted(expected.items()):
        if operation_id is None:
            continue
        if key in flask_routes and key not in contract_routes:
            errors.append(
                f"migrated Flask route has no contract: {key[0]} {key[1]} "
                f"(expected {operation_id})"
            )

    if errors:
        print("native sol contract-route coverage failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print("native sol contract-route coverage ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
