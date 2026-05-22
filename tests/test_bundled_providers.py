# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
import subprocess
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think.providers import bundled
from tests.bundled_provider_fixtures import (
    BUNDLED_STATES,
    bundled_provider_config,
)


@pytest.fixture(autouse=True)
def reset_bundled_locks():
    bundled._LOCKS.clear()


@pytest.fixture
def journal_config(tmp_path, monkeypatch):
    def _write(config: dict) -> Path:
        config_path = tmp_path / "config" / "journal.json"
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        return config_path

    return _write


def _patch_purelib(monkeypatch, site_packages: Path) -> None:
    monkeypatch.setattr(
        bundled.sysconfig,
        "get_paths",
        lambda: {"purelib": str(site_packages)},
    )


def _build_openai_codex_sdk_tree(site_packages: Path) -> Path:
    package_dir = site_packages / "openai_codex_sdk"
    binary = package_dir / "vendor" / "x86_64-unknown-linux-musl" / "codex" / "codex"
    binary.parent.mkdir(parents=True, exist_ok=True)
    binary.touch()
    (package_dir / "__init__.py").write_text("", encoding="utf-8")
    (package_dir / "_install.py").write_text("", encoding="utf-8")
    return package_dir


@pytest.mark.parametrize("provider", ["anthropic", "openai"])
@pytest.mark.parametrize("state", BUNDLED_STATES)
def test_fixture_states_compose_contract(journal_config, provider, state):
    journal_config(bundled_provider_config(provider, state))

    contract = bundled.get_provider_state(provider)

    assert contract["name"] == provider
    assert contract["state"] == state
    assert "actions" in contract
    assert "issues" in contract


def test_install_provider_persists_enabling_and_starts_one_thread(
    journal_config,
    monkeypatch,
):
    journal_config(bundled_provider_config("anthropic", "not-enabled"))
    started = []
    monkeypatch.setattr(
        bundled,
        "_start_thread",
        lambda target, args: started.append((target, args)),
    )

    state = bundled.install_provider("anthropic")

    assert state["state"] == "enabling"
    assert len(started) == 1

    second = bundled.install_provider("anthropic")

    assert second["state"] == "enabling"
    assert len(started) == 1


def test_install_provider_installed_no_key_is_noop(journal_config, monkeypatch):
    journal_config(bundled_provider_config("anthropic", "installed-no-key"))
    started = []
    monkeypatch.setattr(
        bundled,
        "_start_thread",
        lambda target, args: started.append((target, args)),
    )

    state = bundled.install_provider("anthropic")

    assert state["state"] == "installed-no-key"
    assert started == []


def test_install_provider_retries_install_failed(journal_config, monkeypatch):
    journal_config(bundled_provider_config("openai", "install-failed"))
    started = []
    monkeypatch.setattr(
        bundled,
        "_start_thread",
        lambda target, args: started.append((target, args)),
    )

    state = bundled.install_provider("openai")

    assert state["state"] == "enabling"
    assert state["install_error"] is None
    assert len(started) == 1


def test_stuck_enabling_allows_install_retry(journal_config, monkeypatch):
    config = bundled_provider_config("openai", "enabling")
    old = datetime.now(timezone.utc) - timedelta(
        seconds=bundled.STUCK_ENABLING_SECONDS + 60
    )
    config["providers"]["bundled"]["openai"]["last_transition_at"] = old.isoformat()
    journal_config(config)
    started = []
    monkeypatch.setattr(
        bundled,
        "_start_thread",
        lambda target, args: started.append((target, args)),
    )

    before = bundled.get_provider_state("openai")
    after = bundled.install_provider("openai")

    assert before["stuck_enabling"] is True
    assert after["state"] == "enabling"
    assert after["stuck_enabling"] is False
    assert len(started) == 1


def test_install_thread_success_transitions_to_valid(journal_config, monkeypatch):
    config = bundled_provider_config("anthropic", "enabling")
    config["env"]["ANTHROPIC_API_KEY"] = "test-key"
    config["providers"]["key_validation"]["anthropic"] = {"valid": True}
    journal_config(config)
    monkeypatch.setattr(bundled, "_run_uv_pip_install", lambda sdk_spec: None)
    monkeypatch.setattr(
        bundled,
        "_resolve_anthropic_binary_via_subprocess",
        lambda: Path("/tmp/claude"),
    )

    bundled._install_thread("anthropic")

    state = bundled.get_provider_state("anthropic")
    assert state["state"] == "valid"
    assert state["binary_path"] == "/tmp/claude"


def test_openhands_runtime_state_uses_importable_modules(
    journal_config,
    monkeypatch,
    tmp_path,
):
    sdk_path = tmp_path / "openhands" / "sdk" / "__init__.py"
    litellm_path = tmp_path / "litellm" / "__init__.py"
    sdk_path.parent.mkdir(parents=True)
    litellm_path.parent.mkdir(parents=True)
    sdk_path.write_text("", encoding="utf-8")
    litellm_path.write_text("", encoding="utf-8")
    journal_config({"providers": {"bundled": {}}})

    def fake_find_spec(module_name: str):
        paths = {
            "openhands.sdk": sdk_path,
            "litellm": litellm_path,
        }
        path = paths.get(module_name)
        if path is None:
            return None
        return SimpleNamespace(origin=str(path), submodule_search_locations=None)

    monkeypatch.setattr(bundled.importlib.util, "find_spec", fake_find_spec)

    state = bundled.get_provider_state("openhands")

    assert state["state"] == "valid"
    assert state["sdk_specs"] == ["openhands-sdk==1.23.*"]
    assert state["binary_path"] == str(sdk_path)
    assert state["key_configured"] is True
    assert state["issues"] == []


def test_validate_key_thread_persists_result(journal_config, monkeypatch):
    config = bundled_provider_config("openai", "installed-no-key")
    config["env"]["OPENAI_API_KEY"] = "test-key"
    journal_config(config)
    monkeypatch.setattr(
        bundled,
        "_validate_provider_key",
        lambda name: {"valid": True},
    )

    bundled._validate_thread("openai")

    state = bundled.get_provider_state("openai")
    assert state["state"] == "valid"
    assert state["key_validation"]["valid"] is True
    assert "timestamp" in state["key_validation"]


def test_validate_key_thread_persists_human_error(journal_config, monkeypatch):
    config = bundled_provider_config("openai", "key-validating")
    config["env"]["OPENAI_API_KEY"] = "test-key"
    journal_config(config)

    def fail(_name):
        raise RuntimeError("provider rejected key")

    monkeypatch.setattr(bundled, "_validate_provider_key", fail)

    bundled._validate_thread("openai")

    state = bundled.get_provider_state("openai")
    assert state["state"] == "invalid-key"
    assert state["key_validation"]["error"] == "provider rejected key"
    assert "provider rejected key" in state["issues"]


def test_validate_key_returns_key_validating_immediately(journal_config, monkeypatch):
    config = bundled_provider_config("anthropic", "installed-no-key")
    config["env"]["ANTHROPIC_API_KEY"] = "test-key"
    journal_config(config)
    started = []
    monkeypatch.setattr(
        bundled,
        "_start_thread",
        lambda target, args: started.append((target, args)),
    )

    state = bundled.validate_key("anthropic")

    assert state["state"] == "key-validating"
    assert len(started) == 1


def test_validate_key_not_enabled_requires_install(journal_config, monkeypatch):
    journal_config(bundled_provider_config("anthropic", "not-enabled"))
    started = []
    monkeypatch.setattr(
        bundled,
        "_start_thread",
        lambda target, args: started.append((target, args)),
    )

    with pytest.raises(bundled.CogitateProviderNotInstalled) as exc_info:
        bundled.validate_key("anthropic")

    assert "sol call settings providers install anthropic" in str(exc_info.value)
    assert started == []


def test_uninstall_during_install_raises(journal_config):
    config = bundled_provider_config("anthropic", "enabling")
    config["providers"]["bundled"]["anthropic"]["last_transition_at"] = datetime.now(
        timezone.utc
    ).isoformat()
    journal_config(config)

    with pytest.raises(bundled.CogitateProviderInstallInFlight):
        bundled.uninstall_provider("anthropic")


def test_uninstall_not_enabled_is_noop(journal_config, monkeypatch):
    config_path = journal_config(bundled_provider_config("anthropic", "not-enabled"))
    calls = []
    monkeypatch.setattr(
        bundled,
        "_run_uv_pip_uninstall",
        lambda sdk_spec: calls.append(sdk_spec),
    )

    state = bundled.uninstall_provider("anthropic")

    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    assert state["state"] == "not-enabled"
    assert calls == []
    assert persisted["providers"]["bundled"]["anthropic"]["state"] == "not-enabled"


def test_uninstall_preserves_keys_auth_and_env(journal_config, monkeypatch):
    config = bundled_provider_config("anthropic", "valid")
    config["env"]["ANTHROPIC_API_KEY"] = "test-key"
    config["providers"]["auth"]["anthropic"] = "api_key"
    config["providers"]["key_validation"]["anthropic"] = {
        "valid": True,
        "timestamp": "2026-05-20T00:00:00+00:00",
    }
    config_path = journal_config(config)
    monkeypatch.setattr(bundled, "_run_uv_pip_uninstall", lambda sdk_spec: None)

    state = bundled.uninstall_provider("anthropic")

    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    assert state["state"] == "not-enabled"
    assert persisted["env"]["ANTHROPIC_API_KEY"] == "test-key"
    assert persisted["providers"]["auth"]["anthropic"] == "api_key"
    assert persisted["providers"]["key_validation"]["anthropic"]["valid"] is True


def test_uninstall_openai_reclaims_vendor_tree(journal_config, monkeypatch, tmp_path):
    site_packages = tmp_path / "site-packages"
    package_dir = _build_openai_codex_sdk_tree(site_packages)
    _patch_purelib(monkeypatch, site_packages)
    monkeypatch.setattr(bundled, "_run_uv_pip_uninstall", lambda sdk_spec: None)
    journal_config(bundled_provider_config("openai", "valid"))

    state = bundled.uninstall_provider("openai")

    assert state["state"] == "not-enabled"
    assert not package_dir.exists()
    assert site_packages.is_dir()


def test_uninstall_openai_skips_when_outside_site_packages(
    journal_config,
    monkeypatch,
    tmp_path,
    caplog,
):
    site_packages = tmp_path / "site-packages"
    site_packages.mkdir()
    elsewhere = tmp_path / "elsewhere"
    package_dir = _build_openai_codex_sdk_tree(elsewhere)
    (site_packages / "openai_codex_sdk").symlink_to(
        package_dir,
        target_is_directory=True,
    )
    _patch_purelib(monkeypatch, site_packages)
    monkeypatch.setattr(bundled, "_run_uv_pip_uninstall", lambda sdk_spec: None)
    journal_config(bundled_provider_config("openai", "valid"))

    with caplog.at_level(logging.WARNING):
        state = bundled.uninstall_provider("openai")

    assert state["state"] == "not-enabled"
    assert package_dir.exists()
    assert any("outside site-packages" in record.message for record in caplog.records)


def test_uninstall_openai_rmtree_failure_is_non_fatal(
    journal_config,
    monkeypatch,
    tmp_path,
    caplog,
):
    site_packages = tmp_path / "site-packages"
    package_dir = _build_openai_codex_sdk_tree(site_packages)
    _patch_purelib(monkeypatch, site_packages)
    monkeypatch.setattr(bundled, "_run_uv_pip_uninstall", lambda sdk_spec: None)

    def fail_rmtree(_path):
        raise OSError("boom")

    monkeypatch.setattr(bundled.shutil, "rmtree", fail_rmtree)
    journal_config(bundled_provider_config("openai", "valid"))

    with caplog.at_level(logging.WARNING):
        state = bundled.uninstall_provider("openai")

    assert state["state"] == "not-enabled"
    assert package_dir.exists()
    assert any("boom" in record.message for record in caplog.records)


def test_uninstall_openai_missing_target_is_silent_noop(
    journal_config,
    monkeypatch,
    tmp_path,
    caplog,
):
    site_packages = tmp_path / "site-packages"
    site_packages.mkdir()
    _patch_purelib(monkeypatch, site_packages)
    monkeypatch.setattr(bundled, "_run_uv_pip_uninstall", lambda sdk_spec: None)
    journal_config(bundled_provider_config("openai", "valid"))

    with caplog.at_level(logging.WARNING):
        state = bundled.uninstall_provider("openai")

    assert state["state"] == "not-enabled"
    assert not any(record.levelno >= logging.WARNING for record in caplog.records)


def test_uninstall_anthropic_does_not_invoke_openai_cleanup(
    journal_config,
    monkeypatch,
):
    calls = []
    monkeypatch.setattr(bundled, "_run_uv_pip_uninstall", lambda sdk_spec: None)
    monkeypatch.setattr(
        bundled,
        "_remove_openai_post_install_artifacts",
        lambda: calls.append(True),
    )
    journal_config(bundled_provider_config("anthropic", "valid"))

    state = bundled.uninstall_provider("anthropic")

    assert state["state"] == "not-enabled"
    assert calls == []


def test_resolve_bundled_binary_success(journal_config):
    journal_config(bundled_provider_config("openai", "valid"))

    assert bundled.resolve_bundled_binary("openai") == Path("/tmp/solstone-test/openai")


def test_resolve_bundled_binary_missing_has_install_hint(journal_config):
    journal_config(bundled_provider_config("anthropic", "not-enabled"))

    with pytest.raises(bundled.CogitateProviderNotInstalled) as exc_info:
        bundled.resolve_bundled_binary("anthropic")

    assert "sol call settings providers install anthropic" in str(exc_info.value)


def test_uv_install_error_categorization(monkeypatch):
    monkeypatch.setattr(
        bundled.subprocess,
        "run",
        lambda *args, **kwargs: subprocess.CompletedProcess(
            args[0],
            1,
            stdout="",
            stderr="timed out connecting to pypi",
        ),
    )

    with pytest.raises(bundled.CogitateProviderInstallFailed) as exc_info:
        bundled._run_uv_pip_install("claude-agent-sdk==0.2.82")

    assert str(exc_info.value).startswith("network:")


def test_uv_install_accepts_multiple_specs(monkeypatch):
    calls = []

    def fake_run(*args, **kwargs):
        calls.append((args, kwargs))
        return subprocess.CompletedProcess(args[0], 0, stdout="", stderr="")

    monkeypatch.setattr(bundled.subprocess, "run", fake_run)

    bundled._run_uv_pip_install(["openhands-sdk==1.23.*", "litellm"])

    command = calls[0][0][0]
    assert command[-2:] == ["openhands-sdk==1.23.*", "litellm"]


@pytest.mark.parametrize(
    ("stderr", "expected"),
    [
        ("network connection failed", "codex binary download: network"),
        ("sha256 mismatch", "codex binary download: sha256 mismatch"),
        (
            "unsupported platform triple",
            "codex binary download: unsupported platform triple",
        ),
        ("archive missing", "codex binary download: other: archive missing"),
    ],
)
def test_codex_install_error_categorization(monkeypatch, stderr, expected):
    def fake_run(*args, **kwargs):
        return subprocess.CompletedProcess(
            args[0],
            1,
            stdout="",
            stderr=stderr,
        )

    monkeypatch.setattr(bundled.subprocess, "run", fake_run)

    with pytest.raises(bundled.CogitateProviderInstallFailed) as exc_info:
        bundled._run_codex_install("rust-v0.131.0", "", "")

    assert str(exc_info.value) == expected


def test_unsupported_provider_raises():
    with pytest.raises(bundled.UnsupportedBundledProvider):
        bundled.get_provider_state("google")
