# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Fixture setup for isolated journal test cases."""

from __future__ import annotations

import json
from pathlib import Path


def write_established_config(journal_dir: Path) -> None:
    """Write minimal established journal config so session gate admits requests."""
    config_dir = journal_dir / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    config_payload = {
        "setup": {"completed_at": 1700000000000},
        "identity": {"name": "Harness Owner", "timezone": "UTC"},
    }
    (config_dir / "journal.json").write_text(
        json.dumps(config_payload, indent=2), encoding="utf-8"
    )


def write_prior_mlx_status(journal_dir: Path) -> None:
    """Write prior-format MLX status record matching tests.rs begin fixture."""
    providers_dir = journal_dir / "health" / "providers"
    providers_dir.mkdir(parents=True, exist_ok=True)
    status_payload = {
        "schema_version": 1,
        "provider": "local",
        "revision": 1,
        "install_state": "downloading",
        "attempt_id": "018e4f1a2b3c4d5e0000000000000001",
        "target_fingerprint_json": '{"provider":"local","runtime":"mlx","model_pin":{"model_id":"qwen3.5:9b"}}',
        "target_fingerprint_sha256": "legacy-mlx",
        "started_at": "2026-01-01T00:00:00.000000Z",
        "last_transition_at": "2026-01-01T00:00:00.000000Z",
        "last_progress_at": None,
        "completed_at": None,
        "progress_bytes_received": 1048576,
        "progress_bytes_total": 5000000000,
        "install_error": None,
        "error_code": None,
        "owner": None,
    }
    (providers_dir / "local.json").write_text(
        json.dumps(status_payload, indent=2), encoding="utf-8"
    )


def setup_case_journal(case_dir: Path, case_name: str) -> Path:
    """Prepare an isolated journal directory for the named case."""
    journal_dir = case_dir / "journal"
    journal_dir.mkdir(parents=True, exist_ok=True)
    write_established_config(journal_dir)
    if case_name == "prior_mlx":
        write_prior_mlx_status(journal_dir)
    return journal_dir
