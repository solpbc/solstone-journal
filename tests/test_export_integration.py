# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from argparse import Namespace
from pathlib import Path

import pytest

from solstone.observe.export import ExportResult, main


def test_error_resilience(monkeypatch, capsys):
    calls: list[str] = []

    monkeypatch.setattr(
        "solstone.observe.export.setup_cli",
        lambda parser: Namespace(
            to="host-a",
            key=None,
            only=None,
            dry_run=False,
            day=None,
        ),
    )
    monkeypatch.setattr(
        "solstone.observe.export._parse_day_spec", lambda day, root: ["20260413"]
    )
    monkeypatch.setattr(
        "solstone.observe.export._query_manifest", lambda session, base_url, key: {}
    )
    monkeypatch.setattr(
        "solstone.observe.export.resolve_peer",
        lambda label: Namespace(dir=Path("peer-dir"), instance_id="test-key-123456"),
    )
    monkeypatch.setattr(
        "solstone.observe.export.load_client_identity",
        lambda _peer_dir: object(),
    )
    monkeypatch.setattr("solstone.observe.export.relay_url", lambda: "https://relay")

    class _DummyTunnelClient:
        def __init__(self, *_args, **_kwargs):
            pass

        def __enter__(self):
            return object()

        def __exit__(self, *_args):
            return None

    monkeypatch.setattr("solstone.observe.export.TunnelClient", _DummyTunnelClient)
    monkeypatch.setattr(
        "solstone.observe.export.export_segments",
        lambda base_url, key, days, dry_run, session=None: (
            calls.append("segments") or ExportResult(area="segments", sent=1)
        ),
    )
    monkeypatch.setattr(
        "solstone.observe.export.export_imports",
        lambda base_url, key, dry_run, session=None: (
            calls.append("imports") or ExportResult(area="imports", sent=1)
        ),
    )

    def _explode(*args, **kwargs):
        calls.append("entities")
        raise RuntimeError("boom")

    monkeypatch.setattr("solstone.observe.export.export_entities", _explode)
    monkeypatch.setattr(
        "solstone.observe.export.export_facets",
        lambda base_url, key, dry_run, session=None: (
            calls.append("facets") or ExportResult(area="facets", sent=1)
        ),
    )
    monkeypatch.setattr(
        "solstone.observe.export.export_config",
        lambda base_url, key, dry_run, session=None: (
            calls.append("config") or ExportResult(area="config", staged=1)
        ),
    )

    with pytest.raises(SystemExit, match="1"):
        main()

    output = capsys.readouterr().out
    assert calls == ["segments", "imports", "entities", "facets", "config"]
    assert "Warning: entity export failed" in output
    assert "entities: FAILED" in output
    assert "segments: 1 sent" in output
    assert "facets: 1 sent" in output
