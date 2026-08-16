# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from scripts import check_native_sol_conformance as conformance
from scripts.build_native_sol_inventory import AuthorityEntry


def test_native_sol_conformance_rejects_empty_authority_scan() -> None:
    errors = conformance.check_conformance(
        authorities=[],
        route_map={},
        document={"paths": {}},
    )

    assert "native sol conformance discovered zero authorities" in errors
    assert "native sol conformance discovered zero Flask routes" in errors
    assert "native sol conformance discovered zero OpenAPI operations" in errors
    assert "native sol conformance missing top-level import authority" in errors


def test_native_sol_conformance_self_test_detects_authority_route_mismatch() -> None:
    authorities = [
        AuthorityEntry(
            authority=conformance.REPO_ROOT
            / "solstone/apps/activities/native/authority.toml",
            authority_path="solstone/apps/activities/native/authority.toml",
            source=conformance.REPO_ROOT
            / "core/crates/solstone-core-sol-client/native/apps/activities/command.rs",
            module="solstone_apps_activities_native_command_rs",
            surface="sol-call",
            path=("activities", "list"),
            kind="command",
            help="List activity records for one day or an inclusive day range.",
            params=[],
            operation_id="activities.list",
            entry_type="http",
            method="GET",
            route="/app/activities/api/day/{day}/wrong",
            contract_operation_id="activities.list",
            handler="list",
            resident=False,
        )
    ]

    errors = conformance.check_conformance(authorities=authorities)

    assert any("activities.list" in error and "route" in error for error in errors)


def test_route_reason_annotations_are_union_with_ast_scan() -> None:
    from solstone.apps.network.routes import pair_start

    assert "pl_revoked" in conformance.route_error_reason_codes(pair_start)
    assert "pairing_request_invalid" in conformance.route_error_reason_codes(pair_start)
