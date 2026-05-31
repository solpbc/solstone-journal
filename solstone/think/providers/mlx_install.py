# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""MLX local backend install helpers."""

from __future__ import annotations

import hashlib
import importlib
import json
import platform
import shutil
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import huggingface_hub
import psutil
from huggingface_hub import constants
from huggingface_hub.file_download import repo_folder_name

from solstone.think.journal_config import read_journal_config, write_journal_config
from solstone.think.models import GEMMA4_26B_A4B_4BIT, QWEN_35_9B
from solstone.think.providers.install_state import (
    InstallStatus,
    read_install_status,
    transition_state,
    write_install_status,
)

MLX_SOFT_TOKEN_BUDGET = 1120
_GEMMA4_MIN_POSITION_EMBEDDING_SIZE = 10240
_LOCAL_NAME = "local"
_HASH_CHUNK_SIZE = 1024 * 1024
_REWRITTEN_VARIANT_FILES = frozenset({"config.json", "processor_config.json"})
_MLX_METADATA_KEYS = frozenset(
    {
        "mlx_model_id",
        "mlx_revision",
        "mlx_snapshot_dir",
        "mlx_variant_dir",
    }
)


@dataclass(frozen=True)
class MLXModelSpec:
    name: str
    repo: str
    revision: str
    min_ram_bytes: int


_MLX_MODEL_REGISTRY: dict[str, MLXModelSpec] = {
    QWEN_35_9B: MLXModelSpec(
        name=QWEN_35_9B,
        repo="mlx-community/Qwen3.5-9B-MLX-8bit",
        revision="84f7c2deea248d8df56240f88102def51c7ed5d6",
        min_ram_bytes=16 * 1024**3,
    ),
    GEMMA4_26B_A4B_4BIT: MLXModelSpec(
        name=GEMMA4_26B_A4B_4BIT,
        repo="mlx-community/gemma-4-26b-a4b-it-4bit",
        revision="efbeee6e582ebfd06abc9d65e90839c4b5d2116b",
        min_ram_bytes=24 * 1024**3,
    ),
}


class MLXInstallUnavailableError(RuntimeError):
    """Raised when the host cannot install or run the requested MLX model."""


class MLXVerificationError(RuntimeError):
    """Raised when a downloaded MLX snapshot fails sha256 verification."""


def _read_status() -> InstallStatus:
    return read_install_status(scope="bundled", name=_LOCAL_NAME)


def _write_status(status: InstallStatus) -> InstallStatus:
    write_install_status(status, scope="bundled")
    return status


def _platform_unsupported_reason() -> str | None:
    if platform.system() != "Darwin":
        return "not running on macOS"
    if platform.machine() != "arm64":
        return "not running on Apple Silicon"
    return None


def is_mlx_platform_supported() -> bool:
    """True when the host is Apple Silicon macOS. Does not import mlx_vlm."""
    return _platform_unsupported_reason() is None


def _check_platform_and_package() -> tuple[bool, str]:
    platform_reason = _platform_unsupported_reason()
    if platform_reason is not None:
        return False, platform_reason

    try:
        importlib.import_module("mlx_vlm")
    except ImportError:
        return False, "mlx-vlm package not installed"

    return True, ""


def is_mlx_available_for_model(spec: MLXModelSpec) -> tuple[bool, str]:
    ok, reason = _check_platform_and_package()
    if not ok:
        return ok, reason
    total_ram = psutil.virtual_memory().total
    if total_ram < spec.min_ram_bytes:
        return False, (
            f"insufficient RAM for {spec.name} "
            f"(need {spec.min_ram_bytes // 1024**3} GB, "
            f"have {total_ram // 1024**3} GB)"
        )
    return True, ""


def resolve_model_spec(model_id: str | None = None) -> MLXModelSpec:
    selected = model_id or QWEN_35_9B
    spec = _MLX_MODEL_REGISTRY.get(selected)
    if spec is None:
        raise ValueError(
            f"unknown MLX model: {selected!r}; known: {sorted(_MLX_MODEL_REGISTRY)}"
        )
    return spec


def snapshot_dir_for_spec(spec: MLXModelSpec) -> Path:
    repo_folder = repo_folder_name(repo_id=spec.repo, repo_type="model")
    return Path(constants.HF_HUB_CACHE) / repo_folder / "snapshots" / spec.revision


def variant_dir_for_snapshot(snapshot_dir: Path) -> Path:
    return (
        snapshot_dir.parent
        / f"{snapshot_dir.name}-solstone-budget{MLX_SOFT_TOKEN_BUDGET}"
    )


def _safetensors_paths(snapshot_dir: Path) -> list[str]:
    index_path = snapshot_dir / "model.safetensors.index.json"
    data = json.loads(index_path.read_text(encoding="utf-8"))
    weight_map = data.get("weight_map")
    if not isinstance(weight_map, dict):
        raise ValueError("model.safetensors.index.json missing weight_map")
    paths = sorted({str(path) for path in weight_map.values() if str(path)})
    if not paths:
        raise ValueError("model.safetensors.index.json has no safetensors paths")
    return paths


def _snapshot_present(snapshot_dir: Path) -> bool:
    index_path = snapshot_dir / "model.safetensors.index.json"
    if not snapshot_dir.is_dir() or not index_path.is_file():
        return False
    try:
        for rel_path in _safetensors_paths(snapshot_dir):
            file_path = snapshot_dir / rel_path
            if not file_path.is_file() or file_path.stat().st_size <= 0:
                return False
    except (OSError, ValueError, json.JSONDecodeError):
        return False
    return True


def _remote_safetensors_metadata(
    spec: MLXModelSpec, paths: list[str]
) -> dict[str, tuple[str, int]]:
    wanted = set(paths)
    found: dict[str, tuple[str, int]] = {}
    api = huggingface_hub.HfApi()
    for entry in api.list_repo_tree(
        repo_id=spec.repo,
        revision=spec.revision,
        repo_type="model",
        recursive=True,
    ):
        if not isinstance(entry, huggingface_hub.RepoFile) or entry.path not in wanted:
            continue
        if entry.lfs is None:
            raise MLXVerificationError(f"missing LFS sha256 for {entry.path}")
        found[entry.path] = (entry.lfs.sha256, int(entry.lfs.size))
    missing = sorted(wanted - set(found))
    if missing:
        raise MLXVerificationError(f"missing published sha256 for {missing[0]}")
    return found


def validate_snapshot_sha256(spec: MLXModelSpec, snapshot_dir: Path) -> None:
    safetensors_paths = _safetensors_paths(snapshot_dir)
    metadata = _remote_safetensors_metadata(spec, safetensors_paths)

    for rel_path in safetensors_paths:
        expected_sha, _expected_size = metadata[rel_path]
        file_path = snapshot_dir / rel_path
        digest = hashlib.sha256()
        with file_path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(_HASH_CHUNK_SIZE), b""):
                digest.update(chunk)
        actual_sha = digest.hexdigest()
        if actual_sha != expected_sha:
            raise MLXVerificationError(f"sha256 mismatch for {rel_path}")


def _read_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def _write_json(path: Path, data: dict[str, Any]) -> None:
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _validate_gemma4_position_embedding(config: dict[str, Any]) -> None:
    vision_config = config.get("vision_config")
    if not isinstance(vision_config, dict):
        raise ValueError("config.json missing vision_config")
    position_embedding_size = vision_config.get("position_embedding_size")
    if position_embedding_size is None:
        raise ValueError("config.json missing vision_config.position_embedding_size")
    if position_embedding_size < _GEMMA4_MIN_POSITION_EMBEDDING_SIZE:
        raise ValueError(
            "config.json vision_config.position_embedding_size must be >= "
            f"{_GEMMA4_MIN_POSITION_EMBEDDING_SIZE}; actual {position_embedding_size}"
        )


def _rewrite_config(source: Path, target: Path) -> None:
    data = _read_json(source)
    _validate_gemma4_position_embedding(data)
    vision_config = data["vision_config"]
    vision_config["default_output_length"] = MLX_SOFT_TOKEN_BUDGET
    _write_json(target, data)


def _rewrite_processor_config(source: Path, target: Path) -> None:
    data = _read_json(source)
    image_processor = data.get("image_processor")
    if not isinstance(image_processor, dict):
        raise ValueError("processor_config.json missing image_processor")
    image_processor["max_soft_tokens"] = MLX_SOFT_TOKEN_BUDGET
    image_processor["image_seq_length"] = MLX_SOFT_TOKEN_BUDGET
    if "image_seq_length" in data:
        data["image_seq_length"] = MLX_SOFT_TOKEN_BUDGET
    _write_json(target, data)


def _gemma4_variant_valid(variant_dir: Path) -> bool:
    try:
        config = _read_json(variant_dir / "config.json")
        processor_config = _read_json(variant_dir / "processor_config.json")
        _validate_gemma4_position_embedding(config)
        image_processor = processor_config["image_processor"]
        return (
            config["vision_config"]["default_output_length"] == MLX_SOFT_TOKEN_BUDGET
            and image_processor["max_soft_tokens"] == MLX_SOFT_TOKEN_BUDGET
            and image_processor["image_seq_length"] == MLX_SOFT_TOKEN_BUDGET
            and processor_config["image_seq_length"] == MLX_SOFT_TOKEN_BUDGET
        )
    except (KeyError, TypeError, ValueError, OSError, json.JSONDecodeError):
        return False


def _symlink_snapshot_entry(source: Path, target: Path) -> None:
    import os

    relative_source = os.path.relpath(source, target.parent)
    target.symlink_to(relative_source)


def _remove_path(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()


def create_gemma4_variant(snapshot_dir: Path) -> Path:
    variant_dir = variant_dir_for_snapshot(snapshot_dir)
    if variant_dir.exists() and _gemma4_variant_valid(variant_dir):
        return variant_dir

    config_source = snapshot_dir / "config.json"
    processor_source = snapshot_dir / "processor_config.json"
    if not config_source.is_file():
        raise FileNotFoundError(config_source)
    if not processor_source.is_file():
        raise FileNotFoundError(processor_source)

    tmp_dir = variant_dir.parent / f".{variant_dir.name}.{uuid.uuid4().hex}.tmp"
    if tmp_dir.exists():
        shutil.rmtree(tmp_dir)
    tmp_dir.mkdir(parents=True)
    try:
        for source in snapshot_dir.rglob("*"):
            rel_path = source.relative_to(snapshot_dir)
            target = tmp_dir / rel_path
            if source.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue

            target.parent.mkdir(parents=True, exist_ok=True)
            rel_name = rel_path.as_posix()
            if rel_name in _REWRITTEN_VARIANT_FILES:
                if rel_name == "config.json":
                    _rewrite_config(source, target)
                else:
                    _rewrite_processor_config(source, target)
            else:
                _symlink_snapshot_entry(source, target)

        if not _gemma4_variant_valid(tmp_dir):
            raise ValueError("generated Gemma4 variant failed validation")
        if variant_dir.exists():
            _remove_path(variant_dir)
        tmp_dir.replace(variant_dir)
    except Exception:
        shutil.rmtree(tmp_dir, ignore_errors=True)
        raise
    return variant_dir


def _artifact_presence(spec: MLXModelSpec) -> dict[str, Any]:
    snapshot_dir = snapshot_dir_for_spec(spec)
    snapshot_installed = _snapshot_present(snapshot_dir)
    variant_dir = variant_dir_for_snapshot(snapshot_dir)
    variant_installed = True
    runtime_dir = snapshot_dir
    if spec.name == GEMMA4_26B_A4B_4BIT:
        variant_installed = _gemma4_variant_valid(variant_dir)
        runtime_dir = variant_dir
    model_installed = snapshot_installed and variant_installed
    return {
        "model_installed": model_installed,
        "snapshot_installed": snapshot_installed,
        "variant_installed": variant_installed,
        "snapshot_dir": snapshot_dir,
        "variant_dir": variant_dir if spec.name == GEMMA4_26B_A4B_4BIT else None,
        "runtime_dir": runtime_dir,
    }


def _write_mlx_metadata(
    spec: MLXModelSpec,
    *,
    snapshot_dir: Path,
    variant_dir: Path | None,
) -> None:
    config = read_journal_config()
    slot = (
        config.setdefault("providers", {})
        .setdefault("bundled", {})
        .setdefault(_LOCAL_NAME, {})
    )
    for key in _MLX_METADATA_KEYS:
        slot.pop(key, None)
    slot["mlx_model_id"] = spec.name
    slot["mlx_revision"] = spec.revision
    slot["mlx_snapshot_dir"] = str(snapshot_dir)
    if variant_dir is not None:
        slot["mlx_variant_dir"] = str(variant_dir)
    write_journal_config(config)


def inspect_readiness(model_id: str | None = None) -> dict[str, Any]:
    config = read_journal_config()
    record = config.get("providers", {}).get("bundled", {}).get(_LOCAL_NAME, {})
    if not isinstance(record, dict):
        record = {}
    selected_model = model_id or record.get("mlx_model_id") or QWEN_35_9B
    spec = resolve_model_spec(str(selected_model))
    status = _read_status()
    presence = _artifact_presence(spec)
    total_ram = psutil.virtual_memory().total
    return {
        "install_state": status["install_state"],
        "model_installed": presence["model_installed"],
        "snapshot_installed": presence["snapshot_installed"],
        "variant_installed": presence["variant_installed"],
        "ram_sufficient": total_ram >= spec.min_ram_bytes,
        "platform_supported": is_mlx_platform_supported(),
        "package_available": _check_platform_and_package()[0],
        "model_id": spec.name,
        "snapshot_dir": str(presence["snapshot_dir"]),
        "variant_dir": (
            str(presence["variant_dir"])
            if presence["variant_dir"] is not None
            else None
        ),
        "runtime_dir": str(presence["runtime_dir"]),
        "install_error": status["install_error"],
    }


def install_local_mlx(model_id: str = QWEN_35_9B) -> InstallStatus:
    try:
        _write_status(transition_state(_read_status(), new_state="resolving"))
        spec = resolve_model_spec(model_id)
        ok, reason = is_mlx_available_for_model(spec)
        if not ok:
            raise MLXInstallUnavailableError(reason)

        presence = _artifact_presence(spec)
        if presence["model_installed"]:
            _write_mlx_metadata(
                spec,
                snapshot_dir=presence["snapshot_dir"],
                variant_dir=presence["variant_dir"],
            )
            return _write_status(
                transition_state(_read_status(), new_state="installed")
            )

        _write_status(transition_state(_read_status(), new_state="downloading"))
        snapshot_dir = Path(
            huggingface_hub.snapshot_download(
                repo_id=spec.repo,
                revision=spec.revision,
            )
        )

        _write_status(transition_state(_read_status(), new_state="verifying"))
        validate_snapshot_sha256(spec, snapshot_dir)

        _write_status(transition_state(_read_status(), new_state="installing"))
        variant_dir = None
        if spec.name == GEMMA4_26B_A4B_4BIT:
            variant_dir = create_gemma4_variant(snapshot_dir)

        _write_mlx_metadata(spec, snapshot_dir=snapshot_dir, variant_dir=variant_dir)
        return _write_status(transition_state(_read_status(), new_state="installed"))
    except Exception as exc:
        _write_status(
            transition_state(_read_status(), new_state="failed", error=str(exc))
        )
        raise


__all__ = [
    "GEMMA4_26B_A4B_4BIT",
    "MLXInstallUnavailableError",
    "MLXModelSpec",
    "MLXVerificationError",
    "MLX_SOFT_TOKEN_BUDGET",
    "QWEN_35_9B",
    "_GEMMA4_MIN_POSITION_EMBEDDING_SIZE",
    "_MLX_MODEL_REGISTRY",
    "create_gemma4_variant",
    "inspect_readiness",
    "install_local_mlx",
    "is_mlx_available_for_model",
    "is_mlx_platform_supported",
    "resolve_model_spec",
    "snapshot_dir_for_spec",
    "validate_snapshot_sha256",
    "variant_dir_for_snapshot",
]
