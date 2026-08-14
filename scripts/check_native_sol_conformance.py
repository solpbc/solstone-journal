#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Four-way native-sol conformance check."""

from __future__ import annotations

import ast
import inspect
import sys
import tomllib
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import Any

from flask import Flask

import solstone.convey.reasons as reasons
from solstone.apps.activities.routes import activities_bp
from solstone.apps.awareness.routes import awareness_bp
from solstone.apps.curation.routes import curation_bp
from solstone.apps.entities.routes import entities_bp
from solstone.apps.network.routes import network_bp
from solstone.apps.news.routes import news_bp
from solstone.apps.settings.routes import settings_bp
from solstone.apps.sol.routes import sol_bp
from solstone.apps.support.routes import support_bp
from solstone.apps.thinking.routes import thinking_bp
from solstone.convey.chat import chat_bp
from solstone.convey.contract.assemble import build_document, rule_to_openapi_path
from solstone.convey.health import bp as health_bp
from solstone.convey.ledger import bp as ledger_bp
from solstone.convey.profile import bp as profile_bp
from solstone.convey.profile import profiles_bp
from solstone.convey.reasons import Reason
from solstone.convey.root import bp as root_bp

try:
    from scripts.build_native_sol_inventory import (
        AuthorityEntry,
        discover,
        is_private_app_authority,
    )
except ModuleNotFoundError:  # pragma: no cover - direct script execution path.
    from build_native_sol_inventory import (  # type: ignore[no-redef]
        AuthorityEntry,
        discover,
        is_private_app_authority,
    )

REPO_ROOT = Path(__file__).resolve().parents[1]
RUST_CONVEY_OPERATION_PREFIXES = ("body.", "import.", "speakers.", "transcripts.")
REASON_CODES_BY_NAME = {
    name: value.code
    for name, value in vars(reasons).items()
    if isinstance(value, Reason)
}


@dataclass(frozen=True)
class ContractOperation:
    operation_id: str
    method: str
    route: str
    reason_codes: frozenset[str]


@dataclass(frozen=True)
class RawAuthorityEntry:
    authority: Path
    raw: dict[str, Any]


def check_conformance(
    *,
    root: Path = REPO_ROOT,
    document: dict[str, Any] | None = None,
    authorities: list[AuthorityEntry] | None = None,
    route_map: dict[tuple[str, str], Callable[..., Any]] | None = None,
) -> list[str]:
    root = root.resolve()
    document = document if document is not None else build_document()
    authorities = authorities if authorities is not None else discover(root)
    route_map = route_map if route_map is not None else collect_flask_routes()

    errors: list[str] = []
    raw_authority_by_operation = load_raw_authority_entries(root)
    contract_by_operation = collect_contract_operations(document)

    if not authorities:
        errors.append("native sol conformance discovered zero authorities")
    if not route_map:
        errors.append("native sol conformance discovered zero Flask routes")
    if not contract_by_operation:
        errors.append("native sol conformance discovered zero OpenAPI operations")
    if not any(authority.entry_type == "top-level-import" for authority in authorities):
        errors.append("native sol conformance missing top-level import authority")
    if not any(authority.entry_type == "top-level-link" for authority in authorities):
        errors.append("native sol conformance missing top-level link authority")
    if not any(authority.entry_type == "top-level-status" for authority in authorities):
        errors.append("native sol conformance missing top-level status authority")

    for authority in sorted(authorities, key=lambda entry: entry.operation_id):
        if authority.operation_id.startswith(RUST_CONVEY_OPERATION_PREFIXES):
            continue
        raw_authority = raw_authority_by_operation.get(authority.operation_id)
        if authority.entry_type == "http":
            errors.extend(check_http_entry(authority, contract_by_operation, route_map))
        elif authority.entry_type in {"moved-stub", "local"}:
            errors.extend(check_non_http_entry(authority, contract_by_operation))
        elif authority.entry_type == "top-level-chat":
            errors.extend(
                check_top_level_backing_contracts(
                    authority,
                    raw_authority,
                    contract_by_operation,
                    route_map,
                    "top-level-chat",
                    "chat",
                )
            )
        elif authority.entry_type == "top-level-import":
            errors.extend(
                check_top_level_backing_contracts(
                    authority,
                    raw_authority,
                    contract_by_operation,
                    route_map,
                    "top-level-import",
                    "import",
                )
            )
        elif authority.entry_type == "top-level-status":
            errors.extend(
                check_top_level_backing_contracts(
                    authority,
                    raw_authority,
                    contract_by_operation,
                    route_map,
                    "top-level-status",
                    "status",
                )
            )
        elif authority.entry_type == "top-level-link":
            errors.extend(check_non_http_entry(authority, contract_by_operation))
        else:
            errors.append(
                f"{authority.operation_id}: unsupported entry_type "
                f"{authority.entry_type!r}"
            )
    return errors


def check_http_entry(
    authority: AuthorityEntry,
    contract_by_operation: dict[str, ContractOperation],
    route_map: dict[tuple[str, str], Callable[..., Any]],
) -> list[str]:
    operation_id = authority.operation_id
    errors: list[str] = []
    if authority.method is None or authority.route is None:
        errors.append(f"{operation_id}: HTTP authority must declare method and route")
        return errors
    route_key = (authority.method, authority.route)
    view = route_map.get(route_key)
    if view is None:
        errors.append(
            f"{operation_id}: no Flask route for {authority.method} {authority.route}"
        )

    if authority.contract_operation_id is None:
        return errors

    contract = contract_by_operation.get(authority.contract_operation_id)
    if contract is None:
        errors.append(
            f"{operation_id}: no contract operation {authority.contract_operation_id}"
        )
        return errors
    if contract.method != authority.method:
        errors.append(
            f"{operation_id}: contract method {contract.method!r} "
            f"!= authority {authority.method!r}"
        )
    if contract.route != authority.route:
        errors.append(
            f"{operation_id}: contract route {contract.route!r} "
            f"!= authority {authority.route!r}"
        )
    unknown_reasons = sorted(contract.reason_codes - set(REASON_CODES_BY_NAME.values()))
    if unknown_reasons:
        errors.append(
            f"{operation_id}: contract declares unknown reason codes {unknown_reasons!r}"
        )

    if view is not None:
        route_reason_codes = route_error_reason_codes(view)
        if route_reason_codes != contract.reason_codes:
            errors.append(
                f"{operation_id}: route reason codes {sorted(route_reason_codes)!r} "
                f"!= contract {sorted(contract.reason_codes)!r}"
            )
    return errors


def check_non_http_entry(
    authority: AuthorityEntry,
    contract_by_operation: dict[str, ContractOperation],
) -> list[str]:
    errors: list[str] = []
    operation_id = authority.operation_id
    if any(
        value is not None
        for value in (
            authority.method,
            authority.route,
            authority.contract_operation_id,
        )
    ):
        errors.append(
            f"{operation_id}: non-HTTP authority must not declare HTTP fields"
        )
    if operation_id in contract_by_operation:
        errors.append(f"{operation_id}: non-HTTP authority must not have a contract")
    return errors


def check_top_level_backing_contracts(
    authority: AuthorityEntry,
    raw_authority: RawAuthorityEntry | None,
    contract_by_operation: dict[str, ContractOperation],
    route_map: dict[tuple[str, str], Callable[..., Any]],
    entry_type: str,
    label: str,
) -> list[str]:
    errors = check_non_http_entry(authority, contract_by_operation)
    operation_id = authority.operation_id
    authority_ids = (
        raw_authority.raw.get("backing_contract_operation_ids")
        if raw_authority is not None
        else None
    )
    if not isinstance(authority_ids, list) or not authority_ids:
        errors.append(f"{operation_id}: {entry_type} must declare backing contracts")
        return errors
    for backing_id in authority_ids:
        if not isinstance(backing_id, str) or not backing_id:
            errors.append(f"{operation_id}: backing contract id must be a string")
            continue
        contract = contract_by_operation.get(backing_id)
        if contract is None:
            errors.append(
                f"{operation_id}: missing {label} backing contract {backing_id}"
            )
            continue
        if (contract.method, contract.route) not in route_map:
            errors.append(
                f"{operation_id}: missing {label} backing route "
                f"{contract.method} {contract.route}"
            )
    return errors


def collect_contract_operations(
    document: dict[str, Any],
) -> dict[str, ContractOperation]:
    output: dict[str, ContractOperation] = {}
    for route, methods in document["paths"].items():
        for raw_method, operation in methods.items():
            operation_id = operation["operationId"]
            reason_codes = {
                reason_code
                for response in operation.get("responses", {}).values()
                for reason_code in response.get("x-reason-codes", [])
            }
            output[operation_id] = ContractOperation(
                operation_id=operation_id,
                method=raw_method.upper(),
                route=route,
                reason_codes=frozenset(reason_codes),
            )
    return output


def register_native_blueprints(app: Flask) -> None:
    """Register blueprints needed by currently ported native authorities."""

    for blueprint in (
        activities_bp,
        support_bp,
        health_bp,
        chat_bp,
        root_bp,
        curation_bp,
        entities_bp,
        sol_bp,
        awareness_bp,
        ledger_bp,
        profile_bp,
        profiles_bp,
        network_bp,
        settings_bp,
        thinking_bp,
        news_bp,
    ):
        app.register_blueprint(blueprint)


def collect_flask_routes() -> dict[tuple[str, str], Callable[..., Any]]:
    app = Flask(__name__)
    register_native_blueprints(app)
    routes: dict[tuple[str, str], Callable[..., Any]] = {}
    for rule in app.url_map.iter_rules():
        view = app.view_functions[rule.endpoint]
        route = rule_to_openapi_path(rule.rule)
        for method in sorted(rule.methods or ()):
            if method in {"HEAD", "OPTIONS"}:
                continue
            routes[(method, route)] = view
    return routes


def route_error_reason_codes(view: Callable[..., Any]) -> frozenset[str]:
    source = inspect.getsourcefile(view)
    if source is None:
        return frozenset()
    functions = module_functions(Path(source))
    discovered = collect_function_reason_codes(view.__name__, functions, set())
    return frozenset(discovered | annotated_route_reason_codes(view))


def annotated_route_reason_codes(view: Callable[..., Any]) -> set[str]:
    module = inspect.getmodule(view)
    annotations = getattr(module, "NATIVE_SOL_ROUTE_REASON_CODES", None)
    if not isinstance(annotations, dict):
        return set()
    values = annotations.get(view.__name__, set())
    if not isinstance(values, (set, frozenset, list, tuple)):
        return set()
    return {str(value) for value in values}


@lru_cache(maxsize=None)
def module_functions(source: Path) -> dict[str, ast.FunctionDef]:
    tree = ast.parse(source.read_text(encoding="utf-8"), filename=str(source))
    return {node.name: node for node in tree.body if isinstance(node, ast.FunctionDef)}


def collect_function_reason_codes(
    function_name: str,
    functions: dict[str, ast.FunctionDef],
    seen: set[str],
) -> set[str]:
    if function_name in seen:
        return set()
    seen.add(function_name)
    function = functions.get(function_name)
    if function is None:
        return set()
    reason_codes: set[str] = set()
    for node in ast.walk(function):
        if not isinstance(node, ast.Call):
            continue
        name = call_name(node.func)
        if name == "error_response" and node.args:
            reason_code = reason_code_from_ast(node.args[0])
            if reason_code is not None:
                reason_codes.add(reason_code)
        elif (
            isinstance(node.func, ast.Name)
            and node.func.id in functions
            and node.func.id != function_name
        ):
            reason_codes.update(
                collect_function_reason_codes(node.func.id, functions, seen)
            )
    return reason_codes


def call_name(node: ast.expr) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    return None


def reason_code_from_ast(node: ast.expr) -> str | None:
    if isinstance(node, ast.Name):
        return REASON_CODES_BY_NAME.get(node.id)
    if isinstance(node, ast.Attribute):
        return REASON_CODES_BY_NAME.get(node.attr)
    return None


def load_raw_authority_entries(root: Path) -> dict[str, RawAuthorityEntry]:
    output: dict[str, RawAuthorityEntry] = {}
    authority_paths = sorted(
        set((root / "solstone").glob("**/native/authority.toml"))
        | set((root / "solstone").glob("**/native/**/authority.toml"))
    )
    for authority in authority_paths:
        if is_private_app_authority(authority, root):
            continue
        data = tomllib.loads(authority.read_text(encoding="utf-8"))
        for raw in data.get("entries", []):
            if not isinstance(raw, dict):
                continue
            operation_id = raw.get("operation_id")
            if isinstance(operation_id, str):
                output[operation_id] = RawAuthorityEntry(authority=authority, raw=raw)
    return output


def format_errors(errors: Iterable[str]) -> str:
    return "\n".join(f"- {error}" for error in errors)


def main() -> int:
    errors = check_conformance()
    if errors:
        print("native sol conformance failed:", file=sys.stderr)
        print(format_errors(errors), file=sys.stderr)
        return 1
    print("native sol conformance ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
