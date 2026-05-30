# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Install and inspect bundled local provider artifacts.

This module is the sole writer for ``providers.bundled.local`` install state.
It performs no network access at import time.
"""

from __future__ import annotations

import hashlib
import os
import platform
import shutil
import stat
import sys
from pathlib import Path
from typing import Any, Callable

from solstone.think.journal_config import read_journal_config, write_journal_config
from solstone.think.models import LOCAL_MODEL
from solstone.think.providers.install_state import (
    IN_FLIGHT_STATES,
    InstallStatus,
    bump_progress,
    read_install_status,
    transition_state,
    write_install_status,
)
from solstone.think.providers.local import (
    LOCAL_MODEL_SPECS,
    LocalModelSpec,
    LocalProviderError,
    normalize_model_id,
)
from solstone.think.utils import get_journal

LOCAL_PROVIDER_NAME = "local"
_PROBE_TIMEOUT_SECONDS = 10
_LOCAL_METADATA_KEYS = frozenset(
    {
        "binary_artifact",
        "binary_sha256",
        "binary_path",
        "model_id",
        "model_path",
        "model_sha256",
        "mmproj_path",
        "mmproj_sha256",
    }
)

LLAMA_SERVER_PINS: dict[str, dict[str, str]] = {
    "aarch64-apple-darwin": {
        "release_tag": "b9291",
        "filename": "llama-b9291-bin-macos-arm64.tar.gz",
        "sha256": "0e985f87dd71f96a9cb9ebc3ad26f8388030342d000e7e82d4a38d14913373ff",
        "binary_name": "llama-server",
    },
    "x86_64-unknown-linux-gnu": {
        "release_tag": "b9291",
        "filename": "llama-b9291-bin-ubuntu-x64.tar.gz",
        "sha256": "8cb79eb596cc5cc15a6089ceadaa2723e3d75c1e7b37cfb9977ad1d4dc4a41eb",
        "binary_name": "llama-server",
    },
}


def llama_server_artifact_key() -> str:
    machine = platform.machine().lower()
    if machine in {"amd64", "x64"}:
        machine = "x86_64"
    elif machine == "arm64":
        machine = "aarch64"

    if sys.platform == "darwin":
        return f"{machine}-apple-darwin"
    if sys.platform.startswith("linux"):
        return f"{machine}-unknown-linux-gnu"
    return f"{machine}-{sys.platform}"


def pin_for_current_platform() -> dict[str, str]:
    key = llama_server_artifact_key()
    pin = LLAMA_SERVER_PINS.get(key)
    if not pin:
        raise LocalProviderError(
            "unsupported_platform",
            f"No pinned llama-server artifact for platform {key}",
        )
    return pin


def cache_root() -> Path:
    return Path(get_journal()) / "cache" / "providers" / LOCAL_PROVIDER_NAME


def binary_install_dir(
    artifact_key: str | None = None,
    pin: dict[str, str] | None = None,
) -> Path:
    artifact_key = artifact_key or llama_server_artifact_key()
    pin = pin or pin_for_current_platform()
    return cache_root() / "bin" / artifact_key / pin["release_tag"]


def binary_path_for_pin(
    artifact_key: str | None = None,
    pin: dict[str, str] | None = None,
) -> Path:
    pin = pin or pin_for_current_platform()
    return binary_install_dir(artifact_key, pin) / pin["binary_name"]


def model_dir(model_id: str) -> Path:
    safe_id = model_id.replace("/", "__")
    return cache_root() / "models" / safe_id


def model_path(model_id: str) -> Path:
    spec = LOCAL_MODEL_SPECS[normalize_model_id(model_id)]
    return model_dir(spec.model_id) / spec.filename


def mmproj_path(model_id: str) -> Path | None:
    spec = LOCAL_MODEL_SPECS[normalize_model_id(model_id)]
    if spec.mmproj_filename is None:
        return None
    return model_dir(spec.model_id) / spec.mmproj_filename


def install_hint() -> str:
    return "sol call settings providers install local"


def _read_local_status() -> InstallStatus:
    return read_install_status(scope="bundled", name=LOCAL_PROVIDER_NAME)


def _write_local_status(status: InstallStatus) -> InstallStatus:
    write_install_status(status, scope="bundled")
    return status


def _write_local_metadata(updates: dict[str, str]) -> None:
    unknown_keys = sorted(set(updates) - _LOCAL_METADATA_KEYS)
    if unknown_keys:
        raise ValueError(f"unknown local install metadata key: {unknown_keys[0]}")

    config = read_journal_config()
    slot = (
        config.setdefault("providers", {})
        .setdefault("bundled", {})
        .setdefault(LOCAL_PROVIDER_NAME, {})
    )
    for key, value in updates.items():
        slot[key] = value
    write_journal_config(config)


def _record_local_progress(received: int, total: int | None) -> None:
    status = _read_local_status()
    if status["install_state"] not in IN_FLIGHT_STATES:
        return
    _write_local_status(bump_progress(status, received=received, total=total))


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _verify_sha256(path: Path, expected: str) -> None:
    actual = _sha256_file(path)
    if actual != expected:
        raise LocalProviderError(
            "sha256_mismatch",
            f"sha256 mismatch for {path.name}: expected {expected}, got {actual}",
        )


def _download_file(
    url: str,
    dest: Path,
    *,
    timeout_s: float = 600.0,
    on_progress: Callable[[int, int | None], None] | None = None,
) -> None:
    import httpx

    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".tmp")
    with httpx.stream("GET", url, timeout=timeout_s, follow_redirects=True) as response:
        response.raise_for_status()
        total_header = response.headers.get("content-length")
        total = int(total_header) if total_header and total_header.isdigit() else None
        received = 0
        with tmp.open("wb") as handle:
            for chunk in response.iter_bytes():
                if chunk:
                    handle.write(chunk)
                    received += len(chunk)
                    if on_progress is not None:
                        on_progress(received, total)
    tmp.replace(dest)


def _safe_extract_tarball(tarball: Path, dest: Path) -> None:
    import tarfile

    dest.mkdir(parents=True, exist_ok=True)
    dest_resolved = dest.resolve()
    with tarfile.open(tarball, "r:*") as archive:
        for member in archive.getmembers():
            target = (dest / member.name).resolve()
            if target != dest_resolved and dest_resolved not in target.parents:
                raise LocalProviderError(
                    "archive_path_traversal",
                    f"Unsafe tar member path: {member.name}",
                )
        archive.extractall(dest)


def _find_extracted_binary(dest: Path, binary_name: str) -> Path:
    direct = dest / binary_name
    if direct.exists():
        return direct
    matches = [path for path in dest.rglob(binary_name) if path.is_file()]
    if not matches:
        raise LocalProviderError(
            "binary_missing",
            f"Extracted archive did not contain {binary_name}",
        )
    if len(matches) > 1:
        matches.sort(key=lambda path: len(path.parts))
    return matches[0]


def _chmod_executable(path: Path) -> None:
    mode = path.stat().st_mode
    path.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _clear_macos_quarantine(path: Path) -> None:
    if sys.platform != "darwin":
        return
    import subprocess

    try:
        subprocess.run(
            ["xattr", "-dr", "com.apple.quarantine", str(path)],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except OSError:
        return


def probe_binary_runnable(binary_path: str | Path) -> tuple[bool, str | None]:
    import subprocess

    try:
        completed = subprocess.run(
            [str(binary_path), "--version"],
            capture_output=True,
            text=True,
            timeout=_PROBE_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return False, f"timed out after {_PROBE_TIMEOUT_SECONDS}s"
    except Exception as exc:
        return False, str(exc)

    if completed.returncode == 0:
        return True, None

    detail = (
        (completed.stderr or "").strip()
        or (completed.stdout or "").strip()
        or f"exited with status {completed.returncode}"
    )
    return False, detail


def install_llama_server() -> dict[str, Any]:
    artifact_key = llama_server_artifact_key()
    pin = pin_for_current_platform()
    url = (
        "https://github.com/ggml-org/llama.cpp/releases/download/"
        f"{pin['release_tag']}/{pin['filename']}"
    )
    install_dir = binary_install_dir(artifact_key, pin)
    tarball = install_dir / pin["filename"]

    try:
        _write_local_status(
            transition_state(_read_local_status(), new_state="downloading")
        )
        _write_local_metadata({"binary_artifact": pin["filename"]})
        _download_file(url, tarball, on_progress=_record_local_progress)
        _write_local_status(
            transition_state(_read_local_status(), new_state="verifying")
        )
        _verify_sha256(tarball, pin["sha256"])
        if install_dir.exists():
            for child in install_dir.iterdir():
                if child != tarball:
                    if child.is_dir():
                        shutil.rmtree(child)
                    else:
                        child.unlink()
        _safe_extract_tarball(tarball, install_dir)
        extracted = _find_extracted_binary(install_dir, pin["binary_name"])
        final_path = binary_path_for_pin(artifact_key, pin)
        inner_dir = extracted.parent
        if inner_dir != install_dir:
            for item in inner_dir.iterdir():
                shutil.move(str(item), str(install_dir / item.name))
            inner_dir.rmdir()
        _chmod_executable(final_path)
        _clear_macos_quarantine(install_dir)
        _write_local_metadata(
            {
                "binary_artifact": pin["filename"],
                "binary_sha256": pin["sha256"],
                "binary_path": str(final_path),
            }
        )
        return _write_local_status(
            transition_state(_read_local_status(), new_state="installed")
        )
    except Exception as exc:
        _write_local_status(
            transition_state(_read_local_status(), new_state="failed", error=str(exc))
        )
        raise


def install_model(model_id: str = LOCAL_MODEL) -> dict[str, Any]:
    spec = LOCAL_MODEL_SPECS[normalize_model_id(model_id)]
    url = f"https://huggingface.co/{spec.repo}/resolve/{spec.revision}/{spec.filename}"
    dest = model_path(spec.model_id)
    mmproj_dest = mmproj_path(spec.model_id)

    try:
        _write_local_status(
            transition_state(_read_local_status(), new_state="downloading")
        )
        _write_local_metadata({"model_id": spec.model_id})
        _download_file(url, dest, on_progress=_record_local_progress)
        if spec.mmproj_filename and mmproj_dest is not None:
            mmproj_url = (
                f"https://huggingface.co/{spec.repo}/resolve/"
                f"{spec.revision}/{spec.mmproj_filename}"
            )
            _download_file(mmproj_url, mmproj_dest)
        _write_local_status(
            transition_state(_read_local_status(), new_state="verifying")
        )
        _verify_sha256(dest, spec.sha256)
        metadata = {
            "model_id": spec.model_id,
            "model_path": str(dest),
            "model_sha256": spec.sha256,
        }
        if spec.mmproj_sha256 and mmproj_dest is not None:
            _verify_sha256(mmproj_dest, spec.mmproj_sha256)
            metadata["mmproj_path"] = str(mmproj_dest)
            metadata["mmproj_sha256"] = spec.mmproj_sha256
        _write_local_metadata(metadata)
        return _write_local_status(
            transition_state(_read_local_status(), new_state="installed")
        )
    except Exception as exc:
        _write_local_status(
            transition_state(_read_local_status(), new_state="failed", error=str(exc))
        )
        raise


def install_local(model_id: str = LOCAL_MODEL) -> dict[str, Any]:
    install_llama_server()
    return install_model(model_id)


def _ram_sufficient(spec: LocalModelSpec) -> bool:
    try:
        import psutil

        return int(psutil.virtual_memory().total) >= spec.min_ram_bytes
    except Exception:
        return True


def inspect_readiness(model_id: str | None = None) -> dict[str, Any]:
    config = read_journal_config()
    record = config.get("providers", {}).get("bundled", {}).get(LOCAL_PROVIDER_NAME, {})
    if not isinstance(record, dict):
        record = {}
    status = _read_local_status()
    selected_model = normalize_model_id(
        model_id or record.get("model_id") or LOCAL_MODEL
    )
    spec = LOCAL_MODEL_SPECS[selected_model]
    binary_path = Path(record.get("binary_path") or binary_path_for_pin())
    gguf_path = Path(record.get("model_path") or model_path(selected_model))
    configured_mmproj = record.get("mmproj_path")
    spec_mmproj = mmproj_path(selected_model)
    resolved_mmproj = Path(configured_mmproj) if configured_mmproj else spec_mmproj
    mmproj_installed = resolved_mmproj is None or resolved_mmproj.exists()
    ram_sufficient = _ram_sufficient(spec)
    return {
        "install_state": status["install_state"],
        "binary_installed": binary_path.exists() and os.access(binary_path, os.X_OK),
        "model_installed": gguf_path.exists() and mmproj_installed,
        "gguf_installed": gguf_path.exists(),
        "mmproj_installed": mmproj_installed,
        "ram_sufficient": ram_sufficient,
        "binary_path": str(binary_path),
        "model_path": str(gguf_path),
        "mmproj_path": str(resolved_mmproj) if resolved_mmproj is not None else None,
        "model_id": selected_model,
        "install_error": status["install_error"],
    }


def ensure_artifacts_installed(model_id: str) -> tuple[Path, Path, Path | None]:
    selected_model = normalize_model_id(model_id)
    readiness = inspect_readiness(selected_model)
    if not readiness["ram_sufficient"]:
        raise LocalProviderError(
            "ram_insufficient",
            "This computer does not have enough memory for the selected local model.",
        )
    if not readiness["binary_installed"]:
        raise LocalProviderError("binary_missing", "Local runtime is not installed.")
    if not readiness["model_installed"]:
        raise LocalProviderError(
            "model_missing", "Local model files are not installed."
        )
    mmproj = readiness.get("mmproj_path")
    return (
        Path(readiness["binary_path"]),
        Path(readiness["model_path"]),
        Path(mmproj) if mmproj else None,
    )


__all__ = [
    "LLAMA_SERVER_PINS",
    "llama_server_artifact_key",
    "pin_for_current_platform",
    "binary_path_for_pin",
    "model_path",
    "mmproj_path",
    "install_llama_server",
    "install_model",
    "install_local",
    "install_hint",
    "probe_binary_runnable",
    "inspect_readiness",
    "ensure_artifacts_installed",
]
