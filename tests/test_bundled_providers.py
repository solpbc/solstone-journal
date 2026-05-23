# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think.providers import bundled
from tests.bundled_provider_fixtures import (
    BUNDLED_STATES,
    BundledCase,
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


def _canonical_valid_case() -> BundledCase:
    return BundledCase("installed", "valid", False, True, False)


@pytest.mark.parametrize("provider", ["anthropic", "openai"])
@pytest.mark.parametrize("case", BUNDLED_STATES)
def test_fixture_states_compose_contract(journal_config, provider, case):
    journal_config(bundled_provider_config(provider, case))

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
    started = []
    monkeypatch.setattr(
        bundled,
        "_start_thread",
        lambda target, args: started.append((target, args)),
    )

    state = bundled.install_provider("anthropic")

    assert state["install_state"] == "installing"
    assert len(started) == 1
    with pytest.raises(bundled.CogitateProviderInstallInFlight):
        bundled.install_provider("anthropic")


def test_install_provider_installed_is_noop(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("installed", "key-needed", False, True, False)
        )
    )
    started = []
    monkeypatch.setattr(
        bundled,
        "_start_thread",
        lambda target, args: started.append((target, args)),
    )

    state = bundled.install_provider("anthropic")

    assert state["install_state"] == "installed"
    assert state["key_status"] == "key-needed"
    assert started == []


def test_install_provider_retries_failed(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "openai", BundledCase("failed", "key-needed", False, False, True)
        )
    )
    started = []
    monkeypatch.setattr(
        bundled,
        "_start_thread",
        lambda target, args: started.append((target, args)),
    )

    state = bundled.install_provider("openai")

    assert state["install_state"] == "installing"
    assert state["install_error"] is None
    assert len(started) == 1


def test_install_thread_success_transitions_to_installed(journal_config, monkeypatch):
    journal_config(
        bundled_provider_config(
            "anthropic", BundledCase("installing", "key-needed", False, False, False)
        )
    )
    monkeypatch.setattr(bundled, "_run_uv_pip_install", lambda sdk_spec: None)
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

    def fail(_sdk_spec):
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
