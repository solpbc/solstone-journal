# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import shutil
import tarfile
import time
from pathlib import Path

import pytest

from solstone.think.journal_config import read_journal_config
from solstone.think.models import LOCAL_FLASH
from solstone.think.providers import local_install
from solstone.think.providers.install_state import read_install_status
from solstone.think.providers.local import LOCAL_MODEL_SPECS


def _init_journal(tmp_path, monkeypatch) -> None:
    config_dir = tmp_path / "config"
    config_dir.mkdir(parents=True)
    (config_dir / "journal.json").write_text(
        json.dumps({"providers": {}}) + "\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))


def _local_status() -> dict:
    return read_install_status(scope="bundled", name="local")


def _local_slot() -> dict:
    return read_journal_config()["providers"]["bundled"]["local"]


def _write_probe_script(tmp_path: Path, body: str) -> Path:
    script = tmp_path / "probe.sh"
    script.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    script.chmod(0o755)
    return script


def test_install_hint_literal() -> None:
    assert local_install.install_hint() == "sol call settings providers install local"


def test_install_llama_server_relocates_binary_and_libraries(tmp_path, monkeypatch):
    _init_journal(tmp_path, monkeypatch)
    pin = local_install.pin_for_current_platform()
    artifact_key = local_install.llama_server_artifact_key()
    install_dir = local_install.binary_install_dir(artifact_key, pin)
    binary_path = local_install.binary_path_for_pin(artifact_key, pin)
    inner_name = "llama-btest"
    lib_names = ["libllama.so", "libggml.so", "libfoo.dylib"]
    fixture_root = tmp_path / "fixture" / inner_name
    fixture_root.mkdir(parents=True)
    (fixture_root / pin["binary_name"]).write_bytes(b"fake llama-server")
    for lib_name in lib_names:
        (fixture_root / lib_name).write_bytes(f"fake {lib_name}".encode())
    fixture_tarball = tmp_path / pin["filename"]
    with tarfile.open(fixture_tarball, "w:gz") as archive:
        archive.add(fixture_root, arcname=inner_name)
    quarantine_calls: list[Path] = []

    def fake_download(_url, dest, **_kwargs):
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(fixture_tarball, dest)

    def record_quarantine(path):
        quarantine_calls.append(Path(path))

    monkeypatch.setattr(local_install, "_download_file", fake_download)
    monkeypatch.setattr(local_install, "_verify_sha256", lambda _path, _expected: None)
    monkeypatch.setattr(local_install, "_clear_macos_quarantine", record_quarantine)

    def assert_flat_layout() -> None:
        assert binary_path.exists()
        assert binary_path.read_bytes() == b"fake llama-server"
        for lib_name in lib_names:
            lib_path = install_dir / lib_name
            assert lib_path.exists()
            assert lib_path.read_bytes() == f"fake {lib_name}".encode()
        assert not (install_dir / inner_name).exists()
        assert (install_dir / pin["filename"]).exists()

    result = local_install.install_llama_server()

    assert result["install_state"] == "installed"
    assert_flat_layout()
    assert quarantine_calls == [install_dir]

    result = local_install.install_llama_server()

    assert result["install_state"] == "installed"
    assert_flat_layout()
    assert quarantine_calls == [install_dir, install_dir]


def test_install_llama_server_writes_canonical_sequence(tmp_path, monkeypatch):
    _init_journal(tmp_path, monkeypatch)
    pin = {
        "release_tag": "v1",
        "filename": "llama.tar.gz",
        "sha256": "abc123",
        "binary_name": "llama-server",
    }
    final_path = local_install.binary_path_for_pin("test-platform", pin)
    final_path.parent.mkdir(parents=True)
    final_path.write_text("binary", encoding="utf-8")
    observed: list[tuple[str, str, dict]] = []

    monkeypatch.setattr(
        local_install, "llama_server_artifact_key", lambda: "test-platform"
    )
    monkeypatch.setattr(local_install, "pin_for_current_platform", lambda: pin)

    def fake_download(_url, _dest, **_kwargs):
        observed.append(
            ("download", _local_status()["install_state"], dict(_local_slot()))
        )

    def fake_verify(_path, _expected):
        observed.append(
            ("verify", _local_status()["install_state"], dict(_local_slot()))
        )

    monkeypatch.setattr(local_install, "_download_file", fake_download)
    monkeypatch.setattr(local_install, "_verify_sha256", fake_verify)
    monkeypatch.setattr(
        local_install, "_safe_extract_tarball", lambda _tarball, _dest: None
    )
    monkeypatch.setattr(
        local_install, "_find_extracted_binary", lambda _dest, _name: final_path
    )
    monkeypatch.setattr(local_install, "_chmod_executable", lambda _path: None)
    monkeypatch.setattr(local_install, "_clear_macos_quarantine", lambda _path: None)

    result = local_install.install_llama_server()

    assert [entry[0] for entry in observed] == ["download", "verify"]
    assert observed[0][1] == "downloading"
    assert observed[0][2]["binary_artifact"] == "llama.tar.gz"
    assert observed[1][1] == "verifying"
    assert result["install_state"] == "installed"
    slot = _local_slot()
    assert slot["install_state"] == "installed"
    assert slot["binary_artifact"] == "llama.tar.gz"
    assert slot["binary_sha256"] == "abc123"
    assert slot["binary_path"] == str(final_path)
    assert "state" not in slot


def test_probe_binary_runnable_returns_true_for_zero_exit(tmp_path):
    script = _write_probe_script(tmp_path, "exit 0")

    assert local_install.probe_binary_runnable(script) == (True, None)


def test_probe_binary_runnable_returns_verbatim_loader_stderr(tmp_path):
    detail = "dyld: Library not loaded: @rpath/libfoo.dylib"
    script = _write_probe_script(tmp_path, f"echo '{detail}' >&2\nexit 1")

    runnable, error = local_install.probe_binary_runnable(script)

    assert runnable is False
    assert error == detail


def test_probe_binary_runnable_returns_verbatim_non_loader_stderr(tmp_path):
    detail = "plain launch failure"
    script = _write_probe_script(tmp_path, f"echo '{detail}' >&2\nexit 2")

    runnable, error = local_install.probe_binary_runnable(script)

    assert runnable is False
    assert error == detail


def test_probe_binary_runnable_uses_stdout_when_stderr_empty(tmp_path):
    detail = "stdout launch failure"
    script = _write_probe_script(tmp_path, f"echo '{detail}'\nexit 3")

    runnable, error = local_install.probe_binary_runnable(script)

    assert runnable is False
    assert error == detail


def test_probe_binary_runnable_times_out(tmp_path, monkeypatch):
    script = _write_probe_script(tmp_path, "sleep 5")
    monkeypatch.setattr(local_install, "_PROBE_TIMEOUT_SECONDS", 0.5)

    started_at = time.monotonic()
    runnable, error = local_install.probe_binary_runnable(script)

    assert time.monotonic() - started_at < 2
    assert runnable is False
    assert error is not None
    assert error.startswith("timed out")


def test_probe_binary_runnable_handles_missing_path(tmp_path):
    runnable, error = local_install.probe_binary_runnable(tmp_path / "missing")

    assert runnable is False
    assert error


def test_install_model_writes_canonical_sequence(tmp_path, monkeypatch):
    _init_journal(tmp_path, monkeypatch)
    spec = LOCAL_MODEL_SPECS[LOCAL_FLASH]
    observed: list[tuple[str, str, dict]] = []

    def fake_download(_url, _dest, **_kwargs):
        observed.append(
            ("download", _local_status()["install_state"], dict(_local_slot()))
        )

    def fake_verify(_path, _expected):
        observed.append(
            ("verify", _local_status()["install_state"], dict(_local_slot()))
        )

    monkeypatch.setattr(local_install, "_download_file", fake_download)
    monkeypatch.setattr(local_install, "_verify_sha256", fake_verify)

    result = local_install.install_model(LOCAL_FLASH)

    assert [entry[0] for entry in observed] == ["download", "verify"]
    assert observed[0][1] == "downloading"
    assert observed[0][2]["model_id"] == LOCAL_FLASH
    assert observed[1][1] == "verifying"
    assert result["install_state"] == "installed"
    slot = _local_slot()
    assert slot["install_state"] == "installed"
    assert slot["model_id"] == LOCAL_FLASH
    assert slot["model_path"] == str(local_install.model_path(spec.model_id))
    assert slot["model_sha256"] == spec.sha256
    assert "state" not in slot


def test_install_llama_server_failure_writes_canonical_failed(tmp_path, monkeypatch):
    _init_journal(tmp_path, monkeypatch)
    pin = {
        "release_tag": "v1",
        "filename": "llama.tar.gz",
        "sha256": "abc123",
        "binary_name": "llama-server",
    }
    monkeypatch.setattr(
        local_install, "llama_server_artifact_key", lambda: "test-platform"
    )
    monkeypatch.setattr(local_install, "pin_for_current_platform", lambda: pin)

    def fake_download(_url, _dest, **_kwargs):
        raise RuntimeError("network broke")

    monkeypatch.setattr(local_install, "_download_file", fake_download)

    with pytest.raises(RuntimeError, match="network broke"):
        local_install.install_llama_server()

    status = _local_status()
    assert status["install_state"] == "failed"
    assert status["install_error"] == "network broke"
    slot = _local_slot()
    assert slot["install_state"] == "failed"
    assert slot["install_error"] == "network broke"
    assert "state" not in slot
