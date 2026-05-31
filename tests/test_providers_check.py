# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import argparse
import asyncio
import json
from datetime import datetime
from unittest.mock import MagicMock, patch

import pytest


def test_run_check_writes_health_file(tmp_path, monkeypatch):
    """_run_check writes provider health results to SOLSTONE_JOURNAL/health/talents.json."""
    import solstone.think.providers_cli as providers_cli

    fake_registry = {"fake": object()}
    fake_defaults = {
        "fake": {
            1: "fake-pro-model",
            2: "fake-flash-model",
            3: "fake-lite-model",
        }
    }

    monkeypatch.setattr("solstone.think.providers.PROVIDER_REGISTRY", fake_registry)
    monkeypatch.setattr("solstone.think.models.PROVIDER_DEFAULTS", fake_defaults)
    monkeypatch.setattr(providers_cli, "get_journal", lambda: str(tmp_path))
    monkeypatch.setattr(providers_cli, "_check_generate", lambda *_args: ("ok", "ok"))

    async def mock_check_cogitate(*_args):
        return "ok", "ok"

    monkeypatch.setattr(providers_cli, "_check_cogitate", mock_check_cogitate)

    args = argparse.Namespace(
        provider=None,
        interface=None,
        tier=None,
        json=False,
        timeout=1,
        targeted=False,
    )

    with pytest.raises(SystemExit) as exc_info:
        asyncio.run(providers_cli._run_check(args))

    assert exc_info.value.code == 0

    health_file = tmp_path / "health" / "talents.json"
    assert health_file.exists()

    payload = json.loads(health_file.read_text())
    assert "results" in payload
    assert "summary" in payload
    assert "checked_at" in payload
    assert datetime.fromisoformat(payload["checked_at"]).tzinfo is not None
    assert payload["summary"]["passed"] > 0
    assert payload["summary"]["skipped"] == 0


def test_run_check_partial_failure_exits_one(tmp_path, monkeypatch):
    """_run_check exits 1 when any check fails."""
    import solstone.think.providers_cli as providers_cli

    fake_registry = {"fake": object()}
    fake_defaults = {
        "fake": {
            1: "fake-pro-model",
            2: "fake-flash-model",
            3: "fake-lite-model",
        }
    }

    monkeypatch.setattr("solstone.think.providers.PROVIDER_REGISTRY", fake_registry)
    monkeypatch.setattr("solstone.think.models.PROVIDER_DEFAULTS", fake_defaults)
    monkeypatch.setattr(providers_cli, "get_journal", lambda: str(tmp_path))
    monkeypatch.setattr(providers_cli, "_check_generate", lambda *_args: ("ok", "ok"))

    async def mock_check_cogitate(*_args):
        return "fail", "FAIL: timeout"

    monkeypatch.setattr(providers_cli, "_check_cogitate", mock_check_cogitate)

    args = argparse.Namespace(
        provider=None,
        interface=None,
        tier=None,
        json=False,
        timeout=1,
        targeted=False,
    )

    with pytest.raises(SystemExit) as exc_info:
        asyncio.run(providers_cli._run_check(args))

    assert exc_info.value.code == 1

    health_file = tmp_path / "health" / "talents.json"
    payload = json.loads(health_file.read_text())
    assert payload["summary"]["passed"] == 3
    assert payload["summary"]["skipped"] == 0
    assert payload["summary"]["failed"] == 3


def test_run_check_full_provider_failure_exits_one(tmp_path, monkeypatch):
    """_run_check exits 1 when all checks for a provider fail."""
    import solstone.think.providers_cli as providers_cli

    fake_registry = {"fake": object()}
    fake_defaults = {
        "fake": {
            1: "fake-pro-model",
            2: "fake-flash-model",
            3: "fake-lite-model",
        }
    }

    monkeypatch.setattr("solstone.think.providers.PROVIDER_REGISTRY", fake_registry)
    monkeypatch.setattr("solstone.think.models.PROVIDER_DEFAULTS", fake_defaults)
    monkeypatch.setattr(providers_cli, "get_journal", lambda: str(tmp_path))
    monkeypatch.setattr(
        providers_cli, "_check_generate", lambda *_args: ("fail", "FAIL: key not set")
    )

    async def mock_check_cogitate(*_args):
        return "fail", "FAIL: key not set"

    monkeypatch.setattr(providers_cli, "_check_cogitate", mock_check_cogitate)

    args = argparse.Namespace(
        provider=None,
        interface=None,
        tier=None,
        json=False,
        timeout=1,
        targeted=False,
    )

    with pytest.raises(SystemExit) as exc_info:
        asyncio.run(providers_cli._run_check(args))

    assert exc_info.value.code == 1

    health_file = tmp_path / "health" / "talents.json"
    payload = json.loads(health_file.read_text())
    assert payload["summary"]["passed"] == 0
    assert payload["summary"]["skipped"] == 0
    assert payload["summary"]["failed"] == 6


def test_run_check_dedup_same_model(tmp_path, monkeypatch):
    """_run_check deduplicates checks when tiers resolve to the same model."""
    import solstone.think.providers_cli as providers_cli

    fake_registry = {"fake": object()}
    fake_defaults = {
        "fake": {
            1: "fake-same-model",
            2: "fake-same-model",
            3: "fake-same-model",
        }
    }

    monkeypatch.setattr("solstone.think.providers.PROVIDER_REGISTRY", fake_registry)
    monkeypatch.setattr("solstone.think.models.PROVIDER_DEFAULTS", fake_defaults)
    monkeypatch.setattr(providers_cli, "get_journal", lambda: str(tmp_path))

    gen_mock = MagicMock(return_value=("ok", "ok"))
    monkeypatch.setattr(providers_cli, "_check_generate", gen_mock)

    cog_inner = MagicMock(return_value=("ok", "ok"))

    async def mock_check_cogitate(*args):
        return cog_inner(*args)

    monkeypatch.setattr(providers_cli, "_check_cogitate", mock_check_cogitate)

    args = argparse.Namespace(
        provider=None,
        interface=None,
        tier=None,
        json=False,
        timeout=1,
        targeted=False,
    )

    with pytest.raises(SystemExit) as exc_info:
        asyncio.run(providers_cli._run_check(args))

    assert exc_info.value.code == 0
    assert gen_mock.call_count == 1
    assert cog_inner.call_count == 1

    health_file = tmp_path / "health" / "talents.json"
    assert health_file.exists()

    payload = json.loads(health_file.read_text())
    results = payload["results"]
    assert len(results) == 6
    assert payload["summary"]["total"] == 6
    assert payload["summary"]["passed"] == 6
    assert payload["summary"]["skipped"] == 0

    non_reused = [result for result in results if "reused_from" not in result]
    reused = [result for result in results if "reused_from" in result]
    assert len(non_reused) == 2
    assert len(reused) == 4
    assert all(result["reused_from"] == "pro" for result in reused)
    assert all(result["elapsed_s"] == 0.0 for result in reused)


def test_run_check_targeted_filters_to_configured_pairs(tmp_path, monkeypatch):
    """--targeted filters checks to only configured provider+tier pairs."""
    import solstone.think.providers_cli as providers_cli

    fake_registry = {"provA": object(), "provB": object(), "provC": object()}
    fake_defaults = {
        "provA": {1: "a-pro", 2: "a-flash", 3: "a-lite"},
        "provB": {1: "b-pro", 2: "b-flash", 3: "b-lite"},
        "provC": {1: "c-pro", 2: "c-flash", 3: "c-lite"},
    }
    fake_type_defaults = {
        "generate": {"provider": "provA", "tier": 2, "backup": "provB"},
        "cogitate": {"provider": "provC", "tier": 2, "backup": "provB"},
    }

    monkeypatch.setattr("solstone.think.providers.PROVIDER_REGISTRY", fake_registry)
    monkeypatch.setattr("solstone.think.models.PROVIDER_DEFAULTS", fake_defaults)
    monkeypatch.setattr("solstone.think.models.TYPE_DEFAULTS", fake_type_defaults)
    monkeypatch.setattr(providers_cli, "get_journal", lambda: str(tmp_path))
    monkeypatch.setattr(providers_cli, "_check_generate", lambda *_args: ("ok", "ok"))

    async def mock_check_cogitate(*_args):
        return "ok", "ok"

    monkeypatch.setattr(providers_cli, "_check_cogitate", mock_check_cogitate)

    # Mock get_config to return no overrides (use TYPE_DEFAULTS)
    monkeypatch.setattr("solstone.think.utils.get_config", lambda: {})

    # Mock get_backup_provider to return the backup from fake_type_defaults
    def fake_get_backup(agent_type):
        d = fake_type_defaults[agent_type]
        if d["backup"] == d["provider"]:
            return None
        return d["backup"]

    monkeypatch.setattr("solstone.think.models.get_backup_provider", fake_get_backup)

    args = argparse.Namespace(
        provider=None,
        interface=None,
        tier=None,
        json=True,
        timeout=1,
        targeted=True,
    )

    with pytest.raises(SystemExit) as exc_info:
        asyncio.run(providers_cli._run_check(args))

    assert exc_info.value.code == 0

    health_file = tmp_path / "health" / "talents.json"
    payload = json.loads(health_file.read_text())
    # Expected targeted pairs: (provA, 2), (provB, 2), (provC, 2) = 3 pairs × 2 interfaces = 6 checks
    assert payload["summary"]["total"] == 6
    checked_pairs = {(r["provider"], r["tier"]) for r in payload["results"]}
    assert checked_pairs == {("provA", "flash"), ("provB", "flash"), ("provC", "flash")}


def test_run_check_targeted_flock_dedup(tmp_path, monkeypatch):
    """--targeted exits silently when another targeted check holds the lock."""
    import fcntl

    import solstone.think.providers_cli as providers_cli

    fake_registry = {"fake": object()}
    fake_defaults = {"fake": {1: "m", 2: "m", 3: "m"}}
    fake_type_defaults = {
        "generate": {"provider": "fake", "tier": 2, "backup": "fake"},
        "cogitate": {"provider": "fake", "tier": 2, "backup": "fake"},
    }

    monkeypatch.setattr("solstone.think.providers.PROVIDER_REGISTRY", fake_registry)
    monkeypatch.setattr("solstone.think.models.PROVIDER_DEFAULTS", fake_defaults)
    monkeypatch.setattr("solstone.think.models.TYPE_DEFAULTS", fake_type_defaults)
    monkeypatch.setattr(providers_cli, "get_journal", lambda: str(tmp_path))
    monkeypatch.setattr("solstone.think.utils.get_config", lambda: {})
    monkeypatch.setattr("solstone.think.models.get_backup_provider", lambda _: None)

    # Pre-acquire the lock to simulate a concurrent check
    lock_dir = tmp_path / "health"
    lock_dir.mkdir(parents=True, exist_ok=True)
    lock_file = open(lock_dir / "recheck.lock", "w")
    fcntl.flock(lock_file, fcntl.LOCK_EX | fcntl.LOCK_NB)

    gen_mock = MagicMock(return_value=("ok", "ok"))
    monkeypatch.setattr(providers_cli, "_check_generate", gen_mock)

    args = argparse.Namespace(
        provider=None,
        interface=None,
        tier=None,
        json=False,
        timeout=1,
        targeted=True,
    )

    # Should return silently (no SystemExit, no checks run)
    asyncio.run(providers_cli._run_check(args))
    assert gen_mock.call_count == 0

    # No health file written
    assert not (tmp_path / "health" / "talents.json").exists()

    lock_file.close()


def test_check_generate_logs_token_usage(monkeypatch):
    """_check_generate logs token usage when result includes usage data."""
    import solstone.think.providers_cli as providers_cli

    fake_module = MagicMock()
    fake_module.run_generate.return_value = {
        "text": "OK",
        "usage": {"input_tokens": 5, "output_tokens": 2},
    }

    monkeypatch.setattr(
        "solstone.think.providers.get_provider_module", lambda _: fake_module
    )
    monkeypatch.setattr(
        "solstone.think.providers.PROVIDER_METADATA",
        {"fake": {"env_key": "FAKE_API_KEY"}},
    )
    monkeypatch.setattr(
        "solstone.think.models.PROVIDER_DEFAULTS", {"fake": {2: "fake-flash"}}
    )
    monkeypatch.setenv("FAKE_API_KEY", "test-key")

    log_mock = MagicMock()
    monkeypatch.setattr("solstone.think.models.log_token_usage", log_mock)

    status, msg = providers_cli._check_generate("fake", 2, 30)

    assert status == "ok"
    assert msg == "OK"
    log_mock.assert_called_once_with(
        model="fake-flash",
        usage={"input_tokens": 5, "output_tokens": 2},
        context="health.check.generate",
        type="generate",
    )


def test_cortex_start_emits_providers_check(tmp_path):
    """Cortex startup requests a providers health check via supervisor."""
    from solstone.think.cortex import CortexService

    cortex = CortexService(journal_path=str(tmp_path))
    cortex.callosum = MagicMock()
    cortex.callosum.start.return_value = None
    cortex.shutdown_requested.set()

    with patch("solstone.think.cortex.threading.Thread") as mock_thread:
        mock_thread.return_value = MagicMock()
        with patch("solstone.think.cortex.time.sleep", return_value=None):
            cortex.start()

    cortex.callosum.emit.assert_any_call(
        "supervisor", "request", cmd=["journal", "providers", "check"]
    )


def test_missing_env_key_returns_skip(monkeypatch):
    """_check_generate returns skip status when env key is not set."""
    import solstone.think.providers_cli as providers_cli

    monkeypatch.setattr(
        "solstone.think.providers.PROVIDER_METADATA",
        {"fake": {"env_key": "FAKE_API_KEY", "label": "Fake Provider"}},
    )
    monkeypatch.delenv("FAKE_API_KEY", raising=False)

    status, msg = providers_cli._check_generate("fake", 2, 30)
    assert status == "skip"
    assert "Fake Provider not configured" in msg
    assert "FAKE_API_KEY" in msg


def test_check_cogitate_cloud_configured_runs_without_install_skip(monkeypatch):
    import solstone.think.providers_cli as providers_cli

    class FakeModule:
        @staticmethod
        async def run_cogitate(*_args, **_kwargs):
            return "OK"

    monkeypatch.setenv("ANTHROPIC_API_KEY", "test-key")
    monkeypatch.setattr(
        "solstone.think.providers.get_provider_module",
        lambda _provider: FakeModule,
    )

    status, msg = asyncio.run(providers_cli._check_cogitate("anthropic", 2, 30))

    assert (status, msg) == ("ok", "OK")


def test_check_cogitate_local_missing_runtime_names_local_install_hint(monkeypatch):
    import solstone.think.providers_cli as providers_cli

    monkeypatch.setattr(
        providers_cli,
        "_provider_status",
        lambda _name: {
            "configured": True,
            "cogitate_cli_found": False,
        },
    )

    status, msg = asyncio.run(providers_cli._check_cogitate("local", 2, 30))

    assert status == "skip"
    assert "journal install-provider local" in msg


def test_all_skip_exits_zero(tmp_path, monkeypatch):
    """Exit code is 0 when all results are skipped (no fails)."""
    import solstone.think.providers_cli as providers_cli

    fake_registry = {"fake": object()}
    fake_defaults = {"fake": {1: "m1", 2: "m2", 3: "m3"}}

    monkeypatch.setattr("solstone.think.providers.PROVIDER_REGISTRY", fake_registry)
    monkeypatch.setattr("solstone.think.models.PROVIDER_DEFAULTS", fake_defaults)
    monkeypatch.setattr(providers_cli, "get_journal", lambda: str(tmp_path))
    monkeypatch.setattr(
        providers_cli, "_check_generate", lambda *_args: ("skip", "not configured")
    )

    async def mock_check_cogitate(*_args):
        return "skip", "not configured"

    monkeypatch.setattr(providers_cli, "_check_cogitate", mock_check_cogitate)

    args = argparse.Namespace(
        provider=None,
        interface=None,
        tier=None,
        json=False,
        timeout=1,
        targeted=False,
    )

    with pytest.raises(SystemExit) as exc_info:
        asyncio.run(providers_cli._run_check(args))

    assert exc_info.value.code == 0

    payload = json.loads((tmp_path / "health" / "talents.json").read_text())
    assert payload["summary"]["skipped"] == 6
    assert payload["summary"]["failed"] == 0
    assert payload["summary"]["passed"] == 0
    for result in payload["results"]:
        assert result["status"] == "skip"
        assert result["ok"] is True


def test_mix_skip_and_fail_exits_one(tmp_path, monkeypatch):
    """Exit code is 1 when there's a mix of skip and fail results."""
    import solstone.think.providers_cli as providers_cli

    fake_registry = {"fake": object()}
    fake_defaults = {"fake": {1: "m1", 2: "m2", 3: "m3"}}

    monkeypatch.setattr("solstone.think.providers.PROVIDER_REGISTRY", fake_registry)
    monkeypatch.setattr("solstone.think.models.PROVIDER_DEFAULTS", fake_defaults)
    monkeypatch.setattr(providers_cli, "get_journal", lambda: str(tmp_path))
    monkeypatch.setattr(
        providers_cli, "_check_generate", lambda *_args: ("skip", "not configured")
    )

    async def mock_check_cogitate(*_args):
        return "fail", "FAIL: broken"

    monkeypatch.setattr(providers_cli, "_check_cogitate", mock_check_cogitate)

    args = argparse.Namespace(
        provider=None,
        interface=None,
        tier=None,
        json=False,
        timeout=1,
        targeted=False,
    )

    with pytest.raises(SystemExit) as exc_info:
        asyncio.run(providers_cli._run_check(args))

    assert exc_info.value.code == 1

    payload = json.loads((tmp_path / "health" / "talents.json").read_text())
    assert payload["summary"]["skipped"] == 3
    assert payload["summary"]["failed"] == 3


def test_skipped_count_in_summary(tmp_path, monkeypatch):
    """Summary total equals passed + skipped + failed."""
    import solstone.think.providers_cli as providers_cli

    fake_registry = {"okp": object(), "skipP": object()}
    fake_defaults = {
        "okp": {1: "m1", 2: "m2", 3: "m3"},
        "skipP": {1: "s1", 2: "s2", 3: "s3"},
    }

    monkeypatch.setattr("solstone.think.providers.PROVIDER_REGISTRY", fake_registry)
    monkeypatch.setattr("solstone.think.models.PROVIDER_DEFAULTS", fake_defaults)
    monkeypatch.setattr(providers_cli, "get_journal", lambda: str(tmp_path))

    def mock_gen(provider, tier, timeout):
        if provider == "okp":
            return "ok", "OK"
        return "skip", "not configured"

    monkeypatch.setattr(providers_cli, "_check_generate", mock_gen)

    async def mock_cog(provider, tier, timeout):
        if provider == "okp":
            return "ok", "OK"
        return "skip", "not configured"

    monkeypatch.setattr(providers_cli, "_check_cogitate", mock_cog)

    args = argparse.Namespace(
        provider=None,
        interface=None,
        tier=None,
        json=True,
        timeout=1,
        targeted=False,
    )

    with pytest.raises(SystemExit) as exc_info:
        asyncio.run(providers_cli._run_check(args))

    assert exc_info.value.code == 0
    payload = json.loads((tmp_path / "health" / "talents.json").read_text())
    summary = payload["summary"]
    assert (
        summary["total"] == summary["passed"] + summary["skipped"] + summary["failed"]
    )
    assert summary["passed"] == 6
    assert summary["skipped"] == 6
    assert summary["failed"] == 0


def test_status_field_in_json_output(tmp_path, monkeypatch, capsys):
    """JSON output includes status per result and skipped in summary."""
    import solstone.think.providers_cli as providers_cli

    fake_registry = {"fake": object()}
    fake_defaults = {"fake": {1: "m1", 2: "m2", 3: "m3"}}

    monkeypatch.setattr("solstone.think.providers.PROVIDER_REGISTRY", fake_registry)
    monkeypatch.setattr("solstone.think.models.PROVIDER_DEFAULTS", fake_defaults)
    monkeypatch.setattr(providers_cli, "get_journal", lambda: str(tmp_path))
    monkeypatch.setattr(providers_cli, "_check_generate", lambda *_args: ("ok", "OK"))

    async def mock_cog(*_args):
        return "ok", "OK"

    monkeypatch.setattr(providers_cli, "_check_cogitate", mock_cog)

    args = argparse.Namespace(
        provider=None,
        interface=None,
        tier=None,
        json=True,
        timeout=1,
        targeted=False,
    )

    with pytest.raises(SystemExit):
        asyncio.run(providers_cli._run_check(args))

    captured = capsys.readouterr()
    data = json.loads(captured.out)
    for result in data["results"]:
        assert "status" in result
        assert result["status"] in ("ok", "skip", "fail")
    assert "skipped" in data["summary"]
