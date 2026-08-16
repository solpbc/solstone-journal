# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
from __future__ import annotations

import json
from pathlib import Path

import scripts.build_native_sol_inventory as inventory


def write_authority(root: Path, app: str, operation_id: str) -> None:
    native = root / "solstone" / "apps" / app / "native"
    native.mkdir(parents=True)
    command = (
        root
        / "core"
        / "crates"
        / "solstone-core-sol-client"
        / "native"
        / "apps"
        / app
        / "command.rs"
    )
    command.parent.mkdir(parents=True, exist_ok=True)
    command.write_text(
        "// SPDX-License-Identifier: AGPL-3.0-only\n"
        "// Copyright (c) 2026 sol pbc\n\n"
        "pub fn fixture_handler() {}\n"
    )
    (native / "authority.toml").write_text(
        'schema = "native-sol-authority-v1"\n'
        'source = "command.rs"\n\n'
        "[[entries]]\n"
        f'path = ["{app}", "ping"]\n'
        'kind = "command"\n'
        'help = "Synthetic native sol inventory fixture."\n'
        "params = []\n"
        f'operation_id = "{operation_id}"\n'
        'entry_type = "local"\n'
        'handler = "fixture_handler"\n'
    )


def test_inventory_discovery_uses_real_adjacency_without_central_list(
    tmp_path: Path,
) -> None:
    write_authority(tmp_path, "fakeapp", "fakeapp.ping")
    write_authority(tmp_path, "_private", "private.ping")

    entries = inventory.discover(tmp_path)

    assert [entry.path for entry in entries] == [("fakeapp", "ping")]
    rendered = inventory.render(
        entries,
        tmp_path
        / "core"
        / "crates"
        / "solstone-core-sol-client"
        / "src"
        / "generated"
        / "inventory.rs",
    )
    assert 'path: &["fakeapp", "ping"]' in rendered
    assert "fakeapp.ping" in rendered
    assert "_private" not in rendered


def test_resident_entries_generate_resident_handlers(tmp_path: Path) -> None:
    native = tmp_path / "solstone" / "think" / "native" / "link"
    native.mkdir(parents=True)
    command = (
        tmp_path
        / "core"
        / "crates"
        / "solstone-core-sol-client"
        / "native"
        / "think"
        / "link"
        / "command.rs"
    )
    command.parent.mkdir(parents=True, exist_ok=True)
    command.write_text(
        "// SPDX-License-Identifier: AGPL-3.0-only\n"
        "// Copyright (c) 2026 sol pbc\n\n"
        "pub fn link_join() {}\n"
        "pub fn link_serve() {}\n"
    )
    (native / "authority.toml").write_text(
        'schema = "native-sol-authority-v1"\n'
        'source = "command.rs"\n\n'
        "[[entries]]\n"
        'surface = "sol-link"\n'
        'path = ["link", "join"]\n'
        'kind = "top-level"\n'
        'help = "join"\n'
        "params = []\n"
        'operation_id = "link.join"\n'
        'entry_type = "top-level-link"\n'
        'handler = "link_join"\n\n'
        "[[entries]]\n"
        'surface = "sol-link"\n'
        'path = ["link", "serve"]\n'
        'kind = "top-level"\n'
        'help = "serve"\n'
        "params = []\n"
        'operation_id = "link.serve"\n'
        'entry_type = "top-level-link"\n'
        'handler = "link_serve"\n'
        "resident = true\n"
    )

    entries = inventory.discover(tmp_path)
    rendered = inventory.render(
        entries,
        tmp_path
        / "core"
        / "crates"
        / "solstone-core-sol-client"
        / "src"
        / "generated"
        / "inventory.rs",
    )

    assert [entry.resident for entry in entries] == [False, True]
    assert "use crate::resident::ResidentHandler;" in rendered
    assert "pub const HANDLERS: &[Handler] = &[" in rendered
    assert "pub const RESIDENT_HANDLERS: &[ResidentHandler] = &[" in rendered
    assert "solstone_think_native_link_command_rs::link_join," in rendered
    assert "solstone_think_native_link_command_rs::link_serve," in rendered
    assert "resident: false," in rendered
    assert "resident: true," in rendered


def entry(
    tmp_path: Path,
    *,
    path: tuple[str, ...],
    operation_id: str,
    entry_type: str,
    surface: str = "sol-call",
    kind: str = "command",
    authority_name: str = "authority.toml",
    method: str | None = None,
    route: str | None = None,
    contract_operation_id: str | None = None,
    resident: bool = False,
) -> inventory.AuthorityEntry:
    return inventory.AuthorityEntry(
        authority=tmp_path / authority_name,
        authority_path=authority_name,
        source=tmp_path / "command.rs",
        module="fixture",
        surface=surface,
        path=path,
        kind=kind,
        help=f"Fixture {'.'.join(path)}.",
        params=[],
        operation_id=operation_id,
        entry_type=entry_type,
        method=method,
        route=route,
        contract_operation_id=contract_operation_id,
        handler="fixture_handler",
        resident=resident,
    )


def write_oracle(tmp_path: Path, paths: list[tuple[str, ...]]) -> Path:
    oracle = tmp_path / "oracle.json"
    oracle.write_text(
        json.dumps(
            {
                "entries": [
                    {
                        "path": list(path),
                        "kind": "command",
                        "help": f"Fixture {'.'.join(path)}.",
                        "params": [],
                    }
                    for path in paths
                ]
            }
        )
    )
    return oracle


def test_complete_partition_accepts_synthetic_final_shape(tmp_path: Path) -> None:
    oracle = write_oracle(
        tmp_path,
        [
            ("body", "status"),
            ("identity",),
            ("navigate",),
            ("link", "observer-pause"),
            ("journal", "search"),
        ],
    )
    entries = [
        entry(
            tmp_path,
            path=("body", "status"),
            operation_id="body.status",
            entry_type="http",
            method="GET",
            route="/app/body/api/status",
            contract_operation_id="body.status",
        ),
        entry(
            tmp_path,
            path=("identity",),
            operation_id="moved.identity",
            entry_type="moved-stub",
        ),
        entry(
            tmp_path,
            path=("navigate",),
            operation_id="moved.navigate",
            entry_type="moved-stub",
        ),
        entry(
            tmp_path,
            path=("link", "observer-pause"),
            operation_id="link.observer_pause",
            entry_type="local",
        ),
    ]

    errors = inventory.check_complete_partition(
        entries,
        oracle,
        expected_oracle_total=5,
        expected_http_total=1,
        expected_journal_total=1,
        expected_stub_counts={"moved-stub": 2, "local": 1},
        expected_http_group_counts={"body": 1},
    )

    assert errors == []


def test_complete_partition_rejects_non_journal_uncovered_paths(tmp_path: Path) -> None:
    oracle = write_oracle(
        tmp_path,
        [
            ("body", "status"),
            ("entities", "list"),
            ("journal", "search"),
        ],
    )
    entries = [
        entry(
            tmp_path,
            path=("body", "status"),
            operation_id="body.status",
            entry_type="http",
            method="GET",
            route="/app/body/api/status",
            contract_operation_id="body.status",
        )
    ]

    errors = inventory.check_complete_partition(
        entries,
        oracle,
        expected_oracle_total=3,
        expected_http_total=1,
        expected_journal_total=1,
        expected_stub_counts={},
        expected_http_group_counts={"body": 1},
    )

    assert any("uncovered oracle path count" in error for error in errors)
    assert any("uncovered non-journal oracle paths" in error for error in errors)


def test_same_surface_executable_path_prefixes_are_rejected(tmp_path: Path) -> None:
    entries = [
        entry(
            tmp_path,
            path=("chat",),
            operation_id="chat.root",
            entry_type="local",
            authority_name="root-authority.toml",
        ),
        entry(
            tmp_path,
            path=("chat", "start"),
            operation_id="chat.start",
            entry_type="local",
            authority_name="child-authority.toml",
        ),
    ]

    errors = inventory.check_same_surface_executable_path_prefixes(entries)

    assert errors == [
        "native sol executable path prefix conflict on surface 'sol-call': "
        f"['chat'] declared in {tmp_path / 'root-authority.toml'} "
        "is a strict prefix of ['chat', 'start'] declared in "
        f"{tmp_path / 'child-authority.toml'}"
    ]


def test_cross_surface_executable_path_prefixes_are_allowed(tmp_path: Path) -> None:
    entries = [
        entry(
            tmp_path,
            surface="sol-chat",
            path=("chat",),
            operation_id="chat.top_level",
            entry_type="top-level-chat",
            kind="top-level",
            authority_name="chat-authority.toml",
        ),
        entry(
            tmp_path,
            surface="sol-call",
            path=("chat", "start"),
            operation_id="chat.start",
            entry_type="local",
            authority_name="call-authority.toml",
        ),
    ]

    errors = inventory.check_same_surface_executable_path_prefixes(entries)

    assert errors == []


def test_moved_stub_callbacks_participate_in_prefix_rejection(tmp_path: Path) -> None:
    entries = [
        entry(
            tmp_path,
            path=("identity",),
            operation_id="moved.identity",
            entry_type="moved-stub",
            kind="callback",
            authority_name="moved-authority.toml",
        ),
        entry(
            tmp_path,
            path=("identity", "restore"),
            operation_id="identity.restore",
            entry_type="local",
            authority_name="child-authority.toml",
        ),
    ]

    errors = inventory.check_same_surface_executable_path_prefixes(entries)

    assert errors == [
        "native sol executable path prefix conflict on surface 'sol-call': "
        f"['identity'] declared in {tmp_path / 'moved-authority.toml'} "
        "is a strict prefix of ['identity', 'restore'] declared in "
        f"{tmp_path / 'child-authority.toml'}"
    ]
