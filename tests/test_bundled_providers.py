# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import errno
import json
import logging
import os
import subprocess
import sys
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.apps.settings.install_copy import (
    INSTALL_FAILED_NO_PROGRESS,
    INSTALL_FAILED_UV_MISSING,
)
from solstone.think.providers import bundled
from tests.bundled_provider_fixtures import (
    BUNDLED_STATES,
    BundledCase,
    bundled_provider_config,
)


@pytest.fixture(autouse=True)
def reset_bundled_locks():
    bundled._LOCKS.clear()
    bundled._INSTALL_THREADS.clear()
    bundled._INSTALL_PROCESSES.clear()
    bundled._OBSERVED_PHASES.clear()
    yield
    bundled._LOCKS.clear()
    bundled._INSTALL_THREADS.clear()
    bundled._INSTALL_PROCESSES.clear()
    bundled._OBSERVED_PHASES.clear()


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


def _canonical_valid_case() -> BundledCase:
    return BundledCase("installed", "valid", False, True, False)


def _stale_installing_config(provider: str = "anthropic") -> dict:
    config = bundled_provider_config(
        provider, BundledCase("installing", "key-needed", False, False, False)
    )
    record = config["providers"]["bundled"][provider]
    record["last_transition_at"] = "2000-01-01T00:00:00+00:00"
    record["last_progress_at"] = "2000-01-01T00:00:00+00:00"
    return config


def _freshen_in_flight_config(config: dict, provider: str) -> dict:
    record = config["providers"]["bundled"][provider]
    if record["install_state"] in bundled.IN_FLIGHT_STATES:
        timestamp = bundled._now_iso()
        record["last_transition_at"] = timestamp
        record["last_progress_at"] = timestamp
    return config


class _LiveThread:
    def is_alive(self) -> bool:
        return True


class _FakeProc:
    def __init__(
        self,
        returncode: int = 0,
        *,
        wait_raises: bool = False,
    ) -> None:
        self.returncode = returncode
        self.wait_raises = wait_raises
        self.kill_calls = 0
        self.wait_calls = 0

    def wait(self, timeout=None):
        del timeout
        self.wait_calls += 1
        if self.wait_raises:
            self.wait_raises = False
            raise subprocess.TimeoutExpired("uv", bundled._UV_INSTALL_TIMEOUT_SECONDS)
        return self.returncode

    def kill(self) -> None:
        self.kill_calls += 1


@pytest.mark.parametrize("provider", ["anthropic", "openai"])
@pytest.mark.parametrize("case", BUNDLED_STATES)
def test_fixture_states_compose_contract(journal_config, provider, case):
    journal_config(
        _freshen_in_flight_config(bundled_provider_config(provider, case), provider)
    )

    contract = bundled.get_provider_state(provider)

    assert contract["name"] == provider
    assert contract["install_state"] == case.install_state
    assert contract["key_status"] == case.key_status
    assert contract["disabled"] is case.disabled
    assert "state" not in contract
    assert "stuck_enabling" not in contract
    assert "actions" in contract
    assert "issues" in contract


@pytest.mark.parametrize(
    (
        "legacy_state",
        "expected_install_state",
        "expected_cloud_key_status",
        "expected_runtime_key_status",
        "expected_disabled",
    ),
    [
        ("not-enabled", "idle", "key-needed", "not-applicable", False),
        ("enabling", "idle", "key-needed", "not-applicable", False),
        ("installed-no-key", "installed", "key-needed", "not-applicable", False),
        ("key-validating", "installed", "validating", "not-applicable", False),
        ("valid", "installed", "valid", "not-applicable", False),
        ("invalid-key", "installed", "invalid", "not-applicable", False),
        ("install-failed", "failed", "key-needed", "not-applicable", False),
        ("disabled", "idle", "key-needed", "not-applicable", True),
    ],
)
@pytest.mark.parametrize("provider", ["anthropic", "openhands"])
def test_legacy_records_migrate_on_read(
    journal_config,
    provider,
    legacy_state,
    expected_install_state,
    expected_cloud_key_status,
    expected_runtime_key_status,
    expected_disabled,
):
    record = {
        "state": legacy_state,
        "last_transition_at": "2026-05-20T00:00:00+00:00",
        "install_error": "network: timeout"
        if legacy_state == "install-failed"
        else None,
    }
    env = {}
    key_validation = {}
    if provider == "openhands":
        record["sdk_specs"] = ["openhands-sdk==1.23.*"]
        record["runtime"] = "python"
        if legacy_state in {
            "installed-no-key",
            "key-validating",
            "valid",
            "invalid-key",
        }:
            record["binary_path"] = "/tmp/solstone-test/openhands/sdk/__init__.py"
    else:
        record["sdk_spec"] = bundled.PINS[provider]["sdk_spec"]
        if legacy_state in {
            "installed-no-key",
            "key-validating",
            "valid",
            "invalid-key",
        }:
            record["binary_path"] = f"/tmp/solstone-test/{provider}"
        if legacy_state in {"key-validating", "valid", "invalid-key"}:
            env["ANTHROPIC_API_KEY"] = "test-key"
        if legacy_state == "valid":
            key_validation[provider] = {"valid": True}
        elif legacy_state == "invalid-key":
            key_validation[provider] = {"valid": False, "error": "bad key"}
    config_path = journal_config(
        {
            "env": env,
            "providers": {
                "auth": {"anthropic": "api_key"},
                "key_validation": key_validation,
                "bundled": {provider: record},
            },
        }
    )

    state = bundled.get_provider_state(provider)

    expected_key_status = (
        expected_runtime_key_status
        if provider == "openhands"
        else expected_cloud_key_status
    )
    assert state["install_state"] == expected_install_state
    assert state["key_status"] == expected_key_status
    assert state["disabled"] is expected_disabled
    assert "state" not in state
    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    persisted_record = persisted["providers"]["bundled"][provider]
    assert "state" not in persisted_record
    assert persisted_record["install_state"] == expected_install_state
    assert persisted_record["key_state"] == expected_key_status
    assert persisted_record["disabled"] is expected_disabled


def test_mixed_record_drops_legacy_state_and_keeps_canonical(
    journal_config,
    caplog,
):
    config_path = journal_config(
        {
            "providers": {
                "bundled": {
                    "anthropic": {
                        "state": "valid",
                        "install_state": "failed",
                        "last_transition_at": "2026-05-20T00:00:00+00:00",
                        "last_progress_at": None,
                        "install_error": "boom",
                        "key_state": "invalid",
                        "disabled": False,
                        "sdk_spec": bundled.PINS["anthropic"]["sdk_spec"],
                    }
                }
            }
        }
    )

    with caplog.at_level(logging.WARNING):
        state = bundled.get_provider_state("anthropic")

    assert state["install_state"] == "failed"
    assert state["key_status"] == "invalid"
    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    assert "state" not in persisted["providers"]["bundled"]["anthropic"]
    assert any(
        "Dropping legacy state='valid'" in record.message for record in caplog.records
    )


@pytest.mark.parametrize(
    ("record_updates", "env", "key_validation", "install_state", "key_status"),
    [
        ({}, {}, {}, "idle", "key-needed"),
        (
            {"binary_path": "/tmp/solstone-test/anthropic"},
            {"ANTHROPIC_API_KEY": "test-key"},
            {"anthropic": {"valid": True}},
            "installed",
            "valid",
        ),
        (
            {"install_error": "network: timeout"},
            {},
            {"anthropic": {"valid": True}},
            "failed",
            "key-needed",
        ),
    ],
)
def test_disabled_legacy_records_recover_underlying_state(
    journal_config,
    record_updates,
    env,
    key_validation,
    install_state,
    key_status,
):
    record = {
        "state": "disabled",
        "last_transition_at": "2026-05-20T00:00:00+00:00",
        "install_error": None,
        "sdk_spec": bundled.PINS["anthropic"]["sdk_spec"],
        **record_updates,
    }
    config_path = journal_config(
        {
            "env": env,
            "providers": {
                "auth": {"anthropic": "api_key"},
                "key_validation": key_validation,
                "bundled": {"anthropic": record},
            },
        }
    )

    state = bundled.get_provider_state("anthropic")

    assert state["install_state"] == install_state
    assert state["key_status"] == key_status
    assert state["disabled"] is True
    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    persisted_record = persisted["providers"]["bundled"]["anthropic"]
    assert "state" not in persisted_record
    assert persisted_record["install_state"] == install_state
    assert persisted_record["key_state"] == key_status
    if install_state == "failed":
        assert persisted_record["install_error"] == "network: timeout"


def test_install_provider_persists_installing_and_rejects_reentry(
    journal_config,
    monkeypatch,
):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("idle", "key-needed", False, False, False)
        )
    )
    release = threading.Event()
    monkeypatch.setattr(bundled, "_install_thread", lambda _name: release.wait(1))

    thread = None
    try:
        state = bundled.install_provider("anthropic")
        thread = bundled._INSTALL_THREADS.get("anthropic")

        assert state["install_state"] == "installing"
        assert thread is not None
        assert thread.is_alive()
        with pytest.raises(bundled.CogitateProviderInstallInFlight):
            bundled.install_provider("anthropic")
    finally:
        release.set()
        if thread is not None:
            thread.join(timeout=1)


def test_install_provider_installed_is_noop(journal_config):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("installed", "key-needed", False, True, False)
        )
    )
    state = bundled.install_provider("anthropic")

    assert state["install_state"] == "installed"
    assert state["key_status"] == "key-needed"
    assert bundled._INSTALL_THREADS == {}


def test_install_provider_retries_failed(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "openai", BundledCase("failed", "key-needed", False, False, True)
        )
    )
    release = threading.Event()
    monkeypatch.setattr(bundled, "_install_thread", lambda _name: release.wait(1))

    thread = None
    try:
        state = bundled.install_provider("openai")
        thread = bundled._INSTALL_THREADS.get("openai")

        assert state["install_state"] == "installing"
        assert state["install_error"] is None
        assert thread is not None
        assert thread.is_alive()
    finally:
        release.set()
        if thread is not None:
            thread.join(timeout=1)


def test_install_provider_wait_blocks_until_installed(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("idle", "key-needed", False, False, False)
        )
    )
    monkeypatch.setattr(bundled, "_run_uv_pip_install", lambda *args, **kwargs: None)
    monkeypatch.setattr(
        bundled,
        "_resolve_anthropic_binary_via_subprocess",
        lambda: Path("/tmp/claude"),
    )

    state = bundled.install_provider("anthropic", wait=True)

    assert state["install_state"] == "installed"


def test_install_provider_wait_blocks_until_failed(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("idle", "key-needed", False, False, False)
        )
    )

    def fail(*_args, **_kwargs):
        raise bundled.CogitateProviderInstallFailed("network error")

    monkeypatch.setattr(bundled, "_run_uv_pip_install", fail)

    state = bundled.install_provider("anthropic", wait=True)

    assert state["install_state"] == "failed"
    assert "network error" in state["install_error"]


def test_install_thread_success_transitions_to_installed(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("installing", "key-needed", False, False, False)
        )
    )
    monkeypatch.setattr(bundled, "_run_uv_pip_install", lambda name, specs: None)
    monkeypatch.setattr(
        bundled,
        "_resolve_anthropic_binary_via_subprocess",
        lambda: Path("/tmp/claude"),
    )

    bundled._install_thread("anthropic")

    state = bundled.get_provider_state("anthropic")
    assert state["install_state"] == "installed"
    assert state["key_status"] == "key-needed"
    assert state["binary_path"] == "/tmp/claude"


def test_install_thread_failure_transitions_to_failed(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("installing", "key-needed", False, False, False)
        )
    )

    def fail(_name, _specs):
        raise bundled.CogitateProviderInstallFailed("network: timeout")

    monkeypatch.setattr(bundled, "_run_uv_pip_install", fail)

    bundled._install_thread("anthropic")

    state = bundled.get_provider_state("anthropic")
    assert state["install_state"] == "failed"
    assert state["install_error"] == "network: timeout"


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
    journal_config(
        {
            "providers": {
                "bundled": {
                    "openhands": {
                        "install_state": "installed",
                        "last_transition_at": "2026-05-20T00:00:00+00:00",
                        "last_progress_at": None,
                        "install_error": None,
                        "key_state": "not-applicable",
                        "disabled": False,
                        "sdk_specs": ["openhands-sdk==1.23.*"],
                        "runtime": "python",
                        "binary_path": str(sdk_path),
                    }
                }
            }
        }
    )

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

    assert state["install_state"] == "installed"
    assert state["key_status"] == "not-applicable"
    assert state["sdk_specs"] == ["openhands-sdk==1.23.*"]
    assert state["binary_path"] == str(sdk_path)
    assert state["key_configured"] is True
    assert state["issues"] == []


def test_validate_key_thread_persists_result(journal_config, monkeypatch):
    config = bundled_provider_config(
        "openai", BundledCase("installed", "key-needed", False, True, False)
    )
    config["env"]["OPENAI_API_KEY"] = "test-key"
    journal_config(config)
    monkeypatch.setattr(
        bundled,
        "_validate_provider_key",
        lambda name: {"valid": True},
    )

    bundled._validate_thread("openai")

    state = bundled.get_provider_state("openai")
    assert state["key_status"] == "valid"
    assert state["key_validation"]["valid"] is True
    assert "timestamp" in state["key_validation"]


def test_validate_key_thread_persists_human_error(journal_config, monkeypatch):
    config = bundled_provider_config(
        "openai", BundledCase("installed", "validating", False, True, False)
    )
    config["env"]["OPENAI_API_KEY"] = "test-key"
    journal_config(config)

    def fail(_name):
        raise RuntimeError("provider rejected key")

    monkeypatch.setattr(bundled, "_validate_provider_key", fail)

    bundled._validate_thread("openai")

    state = bundled.get_provider_state("openai")
    assert state["key_status"] == "invalid"
    assert state["key_validation"]["error"] == "provider rejected key"
    assert state["issues"] == ["provider rejected key"]


def test_validate_key_returns_validating_immediately(journal_config, monkeypatch):
    config = bundled_provider_config(
        "anthropic", BundledCase("installed", "key-needed", False, True, False)
    )
    config["env"]["ANTHROPIC_API_KEY"] = "test-key"
    journal_config(config)
    started = []
    monkeypatch.setattr(
        bundled,
        "_start_thread",
        lambda target, args: started.append((target, args)),
    )

    state = bundled.validate_key("anthropic")

    assert state["key_status"] == "validating"
    assert len(started) == 1


def test_validate_key_not_enabled_requires_install(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("idle", "key-needed", False, False, False)
        )
    )
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


def test_uninstall_during_install_resets_to_idle(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("installing", "key-needed", False, False, False)
        )
    )
    monkeypatch.setattr(bundled, "_run_uv_pip_uninstall", lambda sdk_spec: None)

    state = bundled.uninstall_provider("anthropic")

    assert state["install_state"] == "idle"
    assert state["key_status"] == "key-needed"


def test_uninstall_not_enabled_is_noop(journal_config, monkeypatch):
    config_path = journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("idle", "key-needed", False, False, False)
        )
    )
    calls = []
    monkeypatch.setattr(
        bundled,
        "_run_uv_pip_uninstall",
        lambda sdk_spec: calls.append(sdk_spec),
    )

    state = bundled.uninstall_provider("anthropic")

    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    assert state["install_state"] == "idle"
    assert calls == []
    assert persisted["providers"]["bundled"]["anthropic"]["install_state"] == "idle"


def test_uninstall_preserves_keys_auth_and_env(journal_config, monkeypatch):
    config = bundled_provider_config("anthropic", _canonical_valid_case())
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
    assert state["install_state"] == "idle"
    assert state["key_status"] == "key-needed"
    assert persisted["env"]["ANTHROPIC_API_KEY"] == "test-key"
    assert persisted["providers"]["auth"]["anthropic"] == "api_key"
    assert persisted["providers"]["key_validation"]["anthropic"]["valid"] is True


def test_disable_provider_preserves_install_and_key_status(journal_config):
    journal_config(bundled_provider_config("anthropic", _canonical_valid_case()))

    state = bundled.disable_provider("anthropic")

    assert state["install_state"] == "installed"
    assert state["key_status"] == "valid"
    assert state["disabled"] is True


def test_enable_provider_preserves_install_and_key_status(journal_config):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("installed", "valid", True, True, False)
        )
    )

    state = bundled.enable_provider("anthropic")

    assert state["install_state"] == "installed"
    assert state["key_status"] == "valid"
    assert state["disabled"] is False


def test_uninstall_openai_reclaims_vendor_tree(journal_config, monkeypatch, tmp_path):
    site_packages = tmp_path / "site-packages"
    package_dir = _build_openai_codex_sdk_tree(site_packages)
    _patch_purelib(monkeypatch, site_packages)
    monkeypatch.setattr(bundled, "_run_uv_pip_uninstall", lambda sdk_spec: None)
    journal_config(bundled_provider_config("openai", _canonical_valid_case()))

    state = bundled.uninstall_provider("openai")

    assert state["install_state"] == "idle"
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
    journal_config(bundled_provider_config("openai", _canonical_valid_case()))

    with caplog.at_level(logging.WARNING):
        state = bundled.uninstall_provider("openai")

    assert state["install_state"] == "idle"
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
    journal_config(bundled_provider_config("openai", _canonical_valid_case()))

    with caplog.at_level(logging.WARNING):
        state = bundled.uninstall_provider("openai")

    assert state["install_state"] == "idle"
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
    journal_config(bundled_provider_config("openai", _canonical_valid_case()))

    with caplog.at_level(logging.WARNING):
        state = bundled.uninstall_provider("openai")

    assert state["install_state"] == "idle"
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
    journal_config(bundled_provider_config("anthropic", _canonical_valid_case()))

    state = bundled.uninstall_provider("anthropic")

    assert state["install_state"] == "idle"
    assert calls == []


def test_resolve_uv_command_prefers_shutil_which(monkeypatch):
    monkeypatch.setattr(bundled.shutil, "which", lambda name: f"/some/abs/{name}")

    assert bundled._resolve_uv_command() == ["/some/abs/uv"]


def test_resolve_uv_command_uses_home_local_candidate(tmp_path, monkeypatch):
    monkeypatch.setattr(bundled.shutil, "which", lambda _name: None)
    monkeypatch.setenv("HOME", str(tmp_path))
    uv = tmp_path / ".local" / "bin" / "uv"
    uv.parent.mkdir(parents=True)
    uv.write_text("#!/bin/sh\n", encoding="utf-8")
    uv.chmod(0o755)

    assert bundled._resolve_uv_command() == [str(uv.resolve())]


def test_resolve_uv_command_uses_importlib_fallback(monkeypatch):
    monkeypatch.setattr(bundled.shutil, "which", lambda _name: None)
    monkeypatch.setattr(bundled.Path, "is_file", lambda _self: False)
    monkeypatch.setattr(bundled.os, "access", lambda *_args: False)
    monkeypatch.setattr(
        bundled.importlib.util,
        "find_spec",
        lambda name: SimpleNamespace(name=name) if name == "uv" else None,
    )

    assert bundled._resolve_uv_command() == [sys.executable, "-m", "uv"]


def test_resolve_uv_command_raises_when_missing(monkeypatch):
    monkeypatch.setattr(bundled.shutil, "which", lambda _name: None)
    monkeypatch.setattr(bundled.Path, "is_file", lambda _self: False)
    monkeypatch.setattr(bundled.os, "access", lambda *_args: False)
    monkeypatch.setattr(bundled.importlib.util, "find_spec", lambda _name: None)

    with pytest.raises(bundled.CogitateProviderInstallFailed) as exc_info:
        bundled._resolve_uv_command()

    assert str(exc_info.value) == INSTALL_FAILED_UV_MISSING


@pytest.mark.parametrize(
    ("line", "phase"),
    [
        ("Resolved 5 packages in 165ms", "resolving"),
        ("  Resolving dependencies...", "resolving"),
        ("Prepared 5 packages in 51ms", "downloading"),
        ("Downloaded cpython-3.11.14-linux-x86_64-gnu", "downloading"),
        ("  Preparing packages...", "downloading"),
        ("Installed 5 packages in 2ms", "installing"),
        ("  Installing wheels...", "installing"),
        ("Using Python 3.11.14 environment at: /tmp/venv", None),
        (" + certifi==2026.5.20", None),
    ],
)
def test_phase_from_uv_line(line, phase):
    assert bundled._phase_from_uv_line(line) == phase


def test_clean_uv_line_strips_ansi_for_phase_matching():
    cleaned = bundled._clean_uv_line("\x1b[2K⠋ Resolving dependencies...\r")

    assert cleaned == "⠋ Resolving dependencies..."
    assert bundled._phase_from_uv_line(cleaned) == "resolving"


def test_advance_phase_allows_first_observed_phase_from_installing(journal_config):
    journal_config(
        _freshen_in_flight_config(
            bundled_provider_config(
                "anthropic",
                BundledCase("installing", "key-needed", False, False, False),
            ),
            "anthropic",
        )
    )

    bundled._advance_phase("anthropic", "resolving")

    state = bundled.get_provider_state("anthropic")
    assert state["install_state"] == "resolving"
    assert bundled._OBSERVED_PHASES["anthropic"] == "resolving"


def test_advance_phase_advances_and_bumps_on_lower_rank(journal_config):
    config = bundled_provider_config(
        "anthropic", BundledCase("installing", "key-needed", False, False, False)
    )
    record = config["providers"]["bundled"]["anthropic"]
    record["last_transition_at"] = "2000-01-01T00:00:00+00:00"
    record["last_progress_at"] = "2000-01-01T00:00:00+00:00"
    journal_config(config)

    bundled._advance_phase("anthropic", "resolving")
    bundled._advance_phase("anthropic", "downloading")
    after_download = bundled.read_install_status(scope="bundled", name="anthropic")
    bundled._advance_phase("anthropic", "resolving")
    after_backtrack = bundled.read_install_status(scope="bundled", name="anthropic")

    assert after_download["install_state"] == "downloading"
    assert after_backtrack["install_state"] == "downloading"
    assert after_backtrack["last_progress_at"] != after_download["last_progress_at"]
    assert bundled._OBSERVED_PHASES["anthropic"] == "downloading"


def test_advance_phase_does_not_move_observed_installing_back(journal_config):
    journal_config(
        _freshen_in_flight_config(
            bundled_provider_config(
                "anthropic",
                BundledCase("installing", "key-needed", False, False, False),
            ),
            "anthropic",
        )
    )
    bundled._OBSERVED_PHASES["anthropic"] = "installing"
    before = bundled.read_install_status(scope="bundled", name="anthropic")

    bundled._advance_phase("anthropic", "resolving")

    after = bundled.read_install_status(scope="bundled", name="anthropic")
    assert after["install_state"] == "installing"
    assert after["last_progress_at"] != before["last_progress_at"]
    assert bundled._OBSERVED_PHASES["anthropic"] == "installing"


@pytest.mark.parametrize("install_state", ["failed", "installed"])
def test_advance_phase_noops_for_terminal_state(journal_config, install_state):
    journal_config(
        bundled_provider_config(
            "anthropic",
            BundledCase(
                install_state,
                "key-needed",
                False,
                install_state == "installed",
                install_state == "failed",
            ),
        )
    )
    before = bundled.read_install_status(scope="bundled", name="anthropic")

    bundled._advance_phase("anthropic", "resolving")

    assert bundled.read_install_status(scope="bundled", name="anthropic") == before
    assert "anthropic" not in bundled._OBSERVED_PHASES


def test_get_provider_state_fails_stalled_install_without_live_thread(journal_config):
    journal_config(_stale_installing_config())

    state = bundled.get_provider_state("anthropic")

    assert state["install_state"] == "failed"
    assert state["install_error"] == INSTALL_FAILED_NO_PROGRESS


def test_get_provider_state_keeps_stalled_install_with_live_thread(journal_config):
    journal_config(_stale_installing_config())
    bundled._INSTALL_THREADS["anthropic"] = _LiveThread()

    state = bundled.get_provider_state("anthropic")

    assert state["install_state"] == "installing"
    assert state["install_error"] is None


def test_lazy_stall_kills_registered_process_once(journal_config):
    journal_config(_stale_installing_config())
    proc = _FakeProc()
    bundled._INSTALL_PROCESSES["anthropic"] = proc

    def read_state():
        return bundled.get_provider_state("anthropic")

    threads = [threading.Thread(target=read_state) for _ in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=1)

    state = bundled.get_provider_state("anthropic")
    assert state["install_state"] == "failed"
    assert proc.kill_calls == 1
    assert "anthropic" not in bundled._INSTALL_PROCESSES


def test_install_provider_registers_and_cleans_up_thread(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("idle", "key-needed", False, False, False)
        )
    )
    release = threading.Event()

    def wait_install(_name, _specs):
        release.wait(1)

    monkeypatch.setattr(bundled, "_run_uv_pip_install", wait_install)
    monkeypatch.setattr(
        bundled,
        "_resolve_anthropic_binary_via_subprocess",
        lambda: Path("/tmp/claude"),
    )

    state = bundled.install_provider("anthropic")
    thread = bundled._INSTALL_THREADS.get("anthropic")
    try:
        assert state["install_state"] == "installing"
        assert thread is not None
        assert thread.is_alive()
    finally:
        release.set()
        if thread is not None:
            thread.join(timeout=1)

    assert "anthropic" not in bundled._INSTALL_THREADS


def test_uv_install_propagates_resolver_failure(monkeypatch):
    monkeypatch.setattr(
        bundled,
        "_resolve_uv_command",
        lambda: (_ for _ in ()).throw(
            bundled.CogitateProviderInstallFailed(INSTALL_FAILED_UV_MISSING)
        ),
    )

    with pytest.raises(bundled.CogitateProviderInstallFailed) as exc_info:
        bundled._run_uv_pip_install("anthropic", ["claude-agent-sdk==0.2.82"])

    assert str(exc_info.value) == INSTALL_FAILED_UV_MISSING


def test_uv_install_popen_oserror_is_uv_missing(monkeypatch):
    monkeypatch.setattr(bundled, "_resolve_uv_command", lambda: ["/missing/uv"])

    def fail_popen(*_args, **_kwargs):
        raise OSError(errno.ENOENT, "missing")

    monkeypatch.setattr(bundled.subprocess, "Popen", fail_popen)

    with pytest.raises(bundled.CogitateProviderInstallFailed) as exc_info:
        bundled._run_uv_pip_install("anthropic", ["claude-agent-sdk==0.2.82"])

    assert str(exc_info.value) == INSTALL_FAILED_UV_MISSING


def test_uv_install_exit_126_is_uv_missing(monkeypatch):
    monkeypatch.setattr(bundled, "_resolve_uv_command", lambda: ["/fake/uv"])

    def fake_popen(*_args, **kwargs):
        os.close(kwargs["stdout"])
        return _FakeProc(returncode=126)

    monkeypatch.setattr(bundled.subprocess, "Popen", fake_popen)

    with pytest.raises(bundled.CogitateProviderInstallFailed) as exc_info:
        bundled._run_uv_pip_install("anthropic", ["claude-agent-sdk==0.2.82"])

    assert str(exc_info.value) == INSTALL_FAILED_UV_MISSING


def test_uv_install_nonzero_uses_tail_for_categorization(monkeypatch):
    monkeypatch.setattr(bundled, "_resolve_uv_command", lambda: ["/fake/uv"])

    def fake_popen(*_args, **kwargs):
        os.write(kwargs["stdout"], b"ResolutionImpossible\n")
        os.close(kwargs["stdout"])
        return _FakeProc(returncode=1)

    monkeypatch.setattr(bundled.subprocess, "Popen", fake_popen)

    with pytest.raises(bundled.CogitateProviderInstallFailed) as exc_info:
        bundled._run_uv_pip_install("anthropic", ["claude-agent-sdk==0.2.82"])

    assert str(exc_info.value) == "dependency conflict: claude-agent-sdk"


def test_uv_install_timeout_kills_and_categorizes(monkeypatch):
    monkeypatch.setattr(bundled, "_resolve_uv_command", lambda: ["/fake/uv"])
    proc = _FakeProc(returncode=1, wait_raises=True)

    def fake_popen(*_args, **kwargs):
        os.close(kwargs["stdout"])
        return proc

    monkeypatch.setattr(bundled.subprocess, "Popen", fake_popen)

    with pytest.raises(bundled.CogitateProviderInstallFailed) as exc_info:
        bundled._run_uv_pip_install("anthropic", ["claude-agent-sdk==0.2.82"])

    assert proc.kill_calls == 1
    assert str(exc_info.value).startswith("network:")


def test_uv_install_drain_observes_phase_lines(journal_config, monkeypatch):
    journal_config(
        _freshen_in_flight_config(
            bundled_provider_config(
                "anthropic",
                BundledCase("installing", "key-needed", False, False, False),
            ),
            "anthropic",
        )
    )
    monkeypatch.setattr(bundled, "_resolve_uv_command", lambda: ["/fake/uv"])

    def fake_popen(*_args, **kwargs):
        writer_fd = os.dup(kwargs["stdout"])

        def write_lines():
            os.write(writer_fd, b"Resolved 1 package in 1ms\n")
            os.write(writer_fd, b"Preparing packages... (0/1)\n")
            os.write(writer_fd, b"Installing wheels...\n")
            os.close(writer_fd)

        writer = threading.Thread(target=write_lines)
        writer.start()
        proc = _FakeProc(returncode=0)
        proc.wait = lambda timeout=None: (writer.join(timeout), proc.returncode)[1]
        return proc

    monkeypatch.setattr(bundled.subprocess, "Popen", fake_popen)

    bundled._run_uv_pip_install("anthropic", ["claude-agent-sdk==0.2.82"])

    status = bundled.read_install_status(scope="bundled", name="anthropic")
    assert status["install_state"] == "installing"
    assert bundled._OBSERVED_PHASES["anthropic"] == "installing"


def test_uv_pip_uninstall_uses_resolved_uv_and_longer_timeout(monkeypatch):
    calls = []
    monkeypatch.setattr(bundled, "_resolve_uv_command", lambda: ["/resolved/uv"])

    def fake_run(*args, **kwargs):
        calls.append((args, kwargs))
        return subprocess.CompletedProcess(args[0], 0, stdout="", stderr="")

    monkeypatch.setattr(bundled.subprocess, "run", fake_run)

    bundled._run_uv_pip_uninstall(["openhands-sdk==1.23.*", "litellm==1.0"])

    command = calls[0][0][0]
    assert command == [
        "/resolved/uv",
        "pip",
        "uninstall",
        "--python",
        sys.executable,
        "openhands-sdk",
        "litellm",
    ]
    assert calls[0][1]["timeout"] == 300


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
