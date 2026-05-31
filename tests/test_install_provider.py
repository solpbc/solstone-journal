# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import sys

from solstone.think import install_provider


def test_install_provider_local_prints_install_status(monkeypatch, capsys):
    calls = []

    def install_local():
        calls.append(True)
        return {"name": "local", "install_state": "installed"}

    monkeypatch.setattr(sys, "argv", ["journal install-provider", "local"])
    monkeypatch.setattr(install_provider.local_install, "install_local", install_local)

    assert install_provider.main() == 0

    assert calls == [True]
    assert json.loads(capsys.readouterr().out) == {
        "name": "local",
        "install_state": "installed",
    }


def test_install_provider_non_local_rejects_without_install(monkeypatch, capsys):
    calls = []

    def install_local():
        calls.append(True)
        return {"name": "local", "install_state": "installed"}

    monkeypatch.setattr(sys, "argv", ["journal install-provider", "anthropic"])
    monkeypatch.setattr(install_provider.local_install, "install_local", install_local)

    assert install_provider.main() == 2

    captured = capsys.readouterr()
    assert calls == []
    assert "only 'local' is supported" in captured.err
