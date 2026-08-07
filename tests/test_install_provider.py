# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI ownership tests for native local and MLX install invocation."""

from __future__ import annotations

import sys

import pytest

from solstone.think import install_provider


class _Readiness:
    ready = False


def _local_cli(monkeypatch, *, probe_free: bool = True) -> None:
    monkeypatch.setattr(sys, "argv", ["journal install-provider", "local"])
    monkeypatch.setattr(install_provider, "require_solstone", lambda: None)
    monkeypatch.setattr(install_provider, "_is_mlx_backend", lambda: False)
    monkeypatch.setattr(install_provider.local_install, "inspect_readiness", lambda: _Readiness())
    monkeypatch.setattr(install_provider.local_install, "target_fingerprint", lambda: {"provider": "local"})
    monkeypatch.setattr(install_provider, "probe_install_lease_free", lambda _provider: probe_free)
    monkeypatch.setattr(install_provider, "_render_fit_report", lambda _report: None)
    from solstone.think.providers import fit_report
    monkeypatch.setattr(fit_report, "build_local_fit_report", lambda _model: object())


def _mlx_cli(monkeypatch, *, probe_free: bool = True) -> None:
    monkeypatch.setattr(install_provider, "_is_mlx_backend", lambda: True)
    monkeypatch.setattr(install_provider.mlx_install, "resolve_model_spec", lambda: type("Spec", (), {"name": "mlx-model"})())
    monkeypatch.setattr(install_provider.mlx_install, "inspect_readiness", lambda _model: _Readiness())
    monkeypatch.setattr(install_provider.mlx_install, "target_fingerprint", lambda _model: {"provider": "local", "runtime": "mlx"})
    monkeypatch.setattr(install_provider, "probe_install_lease_free", lambda _provider: probe_free)
    monkeypatch.setattr(install_provider, "_render_fit_report", lambda _report: None)
    from solstone.think.providers import fit_report
    monkeypatch.setattr(fit_report, "build_mlx_fit_report", lambda _model: object())


def test_linux_local_free_probe_invokes_native_wrapper(monkeypatch):
    _local_cli(monkeypatch)
    calls = []
    monkeypatch.setattr(install_provider.local_install, "install_local", lambda **kwargs: calls.append(kwargs) or {"install_state": "installed"})
    assert install_provider.main() == 0
    assert calls == [{"owner": {"entry": "install_provider"}}]


def test_linux_local_preflight_busy_same_target_observes(monkeypatch):
    _local_cli(monkeypatch, probe_free=False)
    calls = []
    monkeypatch.setattr(install_provider, "_observe_same_target", lambda provider, target: calls.append((provider, target)) or 0)
    assert install_provider.main() == 0
    assert calls and calls[0][0] == "local"


def test_linux_local_preflight_busy_different_target_keeps_observer_result(monkeypatch):
    _local_cli(monkeypatch, probe_free=False)
    monkeypatch.setattr(install_provider, "_observe_same_target", lambda *_args: 1)
    assert install_provider.main() == 1


def test_linux_local_post_probe_busy_race_observes(monkeypatch):
    _local_cli(monkeypatch)
    monkeypatch.setattr(install_provider.local_install, "install_local", lambda **_kwargs: (_ for _ in ()).throw(install_provider.local_install.LocalInstallBusyError("install_busy", "held")))
    monkeypatch.setattr(install_provider, "_observe_same_target", lambda *_args: 0)
    assert install_provider.main() == 0


def test_linux_local_native_failure_uses_existing_error_presentation(monkeypatch):
    _local_cli(monkeypatch)
    monkeypatch.setattr(install_provider.local_install, "install_local", lambda **_kwargs: (_ for _ in ()).throw(RuntimeError("native failed")))
    calls = []
    monkeypatch.setattr(install_provider, "_handle_install_failure", lambda provider, exc: calls.append((provider, str(exc))) or 1)
    assert install_provider.main() == 1
    assert calls == [("local", "native failed")]


def test_mlx_free_probe_invokes_native_wrapper(monkeypatch):
    _mlx_cli(monkeypatch)
    calls = []
    monkeypatch.setattr(install_provider.mlx_install, "install_local_mlx", lambda model, **kwargs: calls.append((model, kwargs)) or {"install_state": "installed"})
    assert install_provider._install_mlx_local() == 0
    assert calls == [("mlx-model", {"owner": {"entry": "install_provider"}})]


def test_mlx_preflight_busy_and_post_probe_race_are_distinct(monkeypatch):
    _mlx_cli(monkeypatch, probe_free=False)
    monkeypatch.setattr(install_provider, "_observe_same_target", lambda *_args: 1)
    assert install_provider._install_mlx_local() == 1
    _mlx_cli(monkeypatch, probe_free=True)
    monkeypatch.setattr(install_provider.mlx_install, "install_local_mlx", lambda *_args, **_kwargs: (_ for _ in ()).throw(install_provider.mlx_install.MLXInstallBusyError("held")))
    monkeypatch.setattr(install_provider, "_observe_same_target", lambda *_args: 0)
    assert install_provider._install_mlx_local() == 0


def test_mlx_native_failure_uses_existing_error_presentation(monkeypatch):
    _mlx_cli(monkeypatch)
    monkeypatch.setattr(install_provider.mlx_install, "install_local_mlx", lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("mlx failed")))
    monkeypatch.setattr(install_provider, "_handle_install_failure", lambda provider, exc: 1 if (provider, str(exc)) == ("local", "mlx failed") else 0)
    assert install_provider._install_mlx_local() == 1
