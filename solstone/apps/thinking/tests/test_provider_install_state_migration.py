# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import importlib
import json
from types import SimpleNamespace

from solstone.think.providers import artifact_proof
from solstone.think.providers.artifact_proof import (
    ReadinessOutcome,
    artifact_manifest_path,
    build_manifest,
    proof_cache_path,
    prove_manifest,
    write_manifest,
)
from solstone.think.providers.install_lease import acquire_install_lease
from solstone.think.providers.install_state import (
    migrate_legacy_provider_artifact_truth,
    read_install_status,
)


def _write_config(journal, bundled_local):
    path = journal / "config" / "journal.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps({"providers": {"bundled": {"local": bundled_local}}}, indent=2)
        + "\n",
        encoding="utf-8",
    )


def _read_config(journal):
    return json.loads((journal / "config" / "journal.json").read_text("utf-8"))


def _readiness(status: str, reason_code: str = "ready") -> ReadinessOutcome:
    return ReadinessOutcome(
        provider="local",
        status=status,  # type: ignore[arg-type]
        reason_code=reason_code,
        target={},
        install={
            "install_state": "idle",
            "install_error": None,
            "error_code": None,
            "attempt_id": None,
            "progress_bytes_received": None,
            "progress_bytes_total": None,
            "last_transition_at": None,
            "last_progress_at": None,
        },
        host={"gpu_available": True, "gpu_probe_ok": True, "ram_sufficient": True},
        artifacts={
            "binary_installed": status == "ready",
            "model_installed": status == "ready",
        },
        proof={
            "binary": {
                "status": status,
                "reason_code": reason_code,
                "cache_hit": False,
            },
            "model": {"status": status, "reason_code": reason_code, "cache_hit": False},
        },
    )


def _ready_readiness_from_manifest(manifest_path, pin_identity) -> ReadinessOutcome:
    result = prove_manifest(
        manifest_path,
        provider="local",
        pin_identity=pin_identity,
    )
    return ReadinessOutcome(
        provider="local",
        status=result.status,  # type: ignore[arg-type]
        reason_code=result.reason_code,
        target={},
        install={
            "install_state": "idle",
            "install_error": None,
            "error_code": None,
            "attempt_id": None,
            "progress_bytes_received": None,
            "progress_bytes_total": None,
            "last_transition_at": None,
            "last_progress_at": None,
        },
        host={"gpu_available": True, "gpu_probe_ok": True, "ram_sufficient": True},
        artifacts={
            "binary_installed": result.ready,
            "model_installed": result.ready,
        },
        proof={
            "binary": {
                "status": result.status,
                "reason_code": result.reason_code,
                "cache_hit": result.cache_hit,
            },
            "model": {
                "status": result.status,
                "reason_code": result.reason_code,
                "cache_hit": result.cache_hit,
            },
        },
    )


def test_migration_task_declares_retry_and_startup_blocking():
    module = importlib.import_module(
        "solstone.apps.thinking.maint.001_migrate_provider_install_state"
    )

    assert module.MAINT_RETRY_ON_NEXT_START is True
    assert module.MAINT_BLOCKS_SUPERVISOR_START is True


def test_ready_legacy_state_publishes_status_and_cleans_config(tmp_path, monkeypatch):
    legacy = {
        "install_state": "installed",
        "vulkan_device_index": 1,
        "binary_artifact": "old",
        "binary_sha256": "old",
        "binary_path": "old",
    }
    _write_config(tmp_path, legacy)
    from solstone.think.providers import local_install

    monkeypatch.setattr(
        local_install, "inspect_readiness", lambda _model: _readiness("ready")
    )
    monkeypatch.setattr(
        local_install,
        "target_fingerprint",
        lambda _model: {"provider": "local", "unit": "test"},
    )

    result = migrate_legacy_provider_artifact_truth(journal_path=tmp_path)

    assert result["actions"][0]["status"] == "ready"
    config = _read_config(tmp_path)
    assert config["providers"]["local"]["vulkan_device_index"] == 1
    assert config["providers"]["bundled"]["local"] == {}
    status = read_install_status(name="local", journal_path=tmp_path)
    assert status["install_state"] == "installed"


def test_fedora_shape_no_manifest_exits_zero_without_cleanup(tmp_path, monkeypatch):
    _write_config(tmp_path, {"install_state": "installed"})
    from solstone.think.providers import local_cuda, local_install

    monkeypatch.setattr(
        local_install,
        "inspect_readiness",
        lambda _model: _readiness("missing-or-mismatched", "manifest_missing"),
    )
    monkeypatch.setattr(
        local_install,
        "target_fingerprint",
        lambda _model: {"provider": "local", "unit": "test"},
    )
    monkeypatch.setattr(
        local_cuda,
        "resolve_local_backend",
        lambda _pin: SimpleNamespace(backend="vulkan", reason="test"),
    )

    result = migrate_legacy_provider_artifact_truth(journal_path=tmp_path)

    action = result["actions"][0]
    assert action["status"] == "missing-or-mismatched"
    assert action["cleanup"] is False
    assert "reinstall will rebuild the proof" in action["message"]
    assert (
        _read_config(tmp_path)["providers"]["bundled"]["local"]["install_state"]
        == "installed"
    )
    assert not (tmp_path / "health" / "providers" / "local.json").exists()


def test_cache_write_failure_does_not_block_migration_promotion(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_config(tmp_path, {"install_state": "installed"})
    artifact_root = tmp_path / "artifact"
    artifact = artifact_root / "bin" / "thing"
    artifact.parent.mkdir(parents=True)
    payload = b"trusted"
    artifact.write_bytes(payload)
    artifact.chmod(0o755)
    pin_identity = {"pin": "pin"}
    manifest_path = artifact_manifest_path(artifact_root)
    write_manifest(
        manifest_path,
        build_manifest(
            provider="local",
            unit="runtime-vulkan",
            target_fingerprint_sha256="target",
            source={"pin_identity": pin_identity},
            inventory=[
                {
                    "relative_path": "bin/thing",
                    "role": "runtime_binary",
                    "size": len(payload),
                    "sha256": hashlib.sha256(payload).hexdigest(),
                }
            ],
        ),
    )
    from solstone.think.providers import local_cuda, local_install

    calls = iter(
        [
            _readiness("missing-or-mismatched", "manifest_missing"),
            _ready_readiness_from_manifest(manifest_path, pin_identity),
        ]
    )
    monkeypatch.setattr(local_install, "inspect_readiness", lambda _model: next(calls))
    monkeypatch.setattr(
        local_install,
        "target_fingerprint",
        lambda _model: {"provider": "local", "unit": "test"},
    )
    monkeypatch.setattr(
        local_cuda,
        "resolve_local_backend",
        lambda _pin: SimpleNamespace(backend="vulkan", reason="test"),
    )
    monkeypatch.setattr(
        "solstone.think.providers.install_state._verify_legacy_local_llama",
        lambda _legacy: None,
    )
    monkeypatch.setattr(local_install, "write_vulkan_manifest", lambda **_kwargs: None)
    monkeypatch.setattr(local_install, "write_model_manifest", lambda **_kwargs: None)

    def fail_cache_replace(path, *_args, **_kwargs):
        if path == proof_cache_path("local", journal_path=tmp_path):
            raise PermissionError("cache read-only")
        raise AssertionError(f"unexpected replace path: {path}")

    monkeypatch.setattr(artifact_proof, "atomic_replace", fail_cache_replace)

    result = migrate_legacy_provider_artifact_truth(journal_path=tmp_path)

    assert result["actions"][0]["status"] == "ready"
    assert result["actions"][0]["action"] == "promoted"
    assert read_install_status(name="local", journal_path=tmp_path)[
        "install_state"
    ] == ("installed")


def test_proof_unavailable_is_non_destructive(tmp_path, monkeypatch):
    _write_config(tmp_path, {"install_state": "installed", "binary_path": "old"})
    from solstone.think.providers import local_install

    monkeypatch.setattr(
        local_install,
        "inspect_readiness",
        lambda _model: _readiness("proof-unavailable", "manifest_io_error"),
    )
    monkeypatch.setattr(
        local_install,
        "target_fingerprint",
        lambda _model: {"provider": "local", "unit": "test"},
    )

    result = migrate_legacy_provider_artifact_truth(journal_path=tmp_path)

    assert result["actions"][0]["status"] == "proof-unavailable"
    assert (
        _read_config(tmp_path)["providers"]["bundled"]["local"]["binary_path"] == "old"
    )
    assert not (tmp_path / "health" / "providers" / "local.json").exists()


def test_busy_lease_defers_to_next_start(tmp_path):
    _write_config(tmp_path, {"install_state": "installed"})
    lease = acquire_install_lease("local", journal_path=tmp_path)
    assert lease is not None

    try:
        result = migrate_legacy_provider_artifact_truth(journal_path=tmp_path)
    finally:
        lease.release()

    assert result["actions"][0]["status"] == "busy"
    assert result["actions"][0]["reason_code"] == "install_busy"
    assert (
        _read_config(tmp_path)["providers"]["bundled"]["local"]["install_state"]
        == "installed"
    )
