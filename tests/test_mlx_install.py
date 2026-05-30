# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import importlib
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest
from huggingface_hub import RepoFile

from solstone.think.journal_config import read_journal_config
from solstone.think.models import GEMMA4_26B_A4B_4BIT, QWEN_35_9B
from solstone.think.providers import mlx_install
from solstone.think.providers.install_state import read_install_status


def _init_journal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    config_dir = tmp_path / "config"
    config_dir.mkdir(parents=True)
    (config_dir / "journal.json").write_text(
        json.dumps({"providers": {}}) + "\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(mlx_install.constants, "HF_HUB_CACHE", str(tmp_path / "hf"))


def _local_status() -> dict:
    return read_install_status(scope="bundled", name="local")


def _local_slot() -> dict:
    return read_journal_config()["providers"]["bundled"]["local"]


def _allow_install(monkeypatch: pytest.MonkeyPatch, *, ram_gb: int = 64) -> None:
    monkeypatch.setattr(mlx_install, "_check_platform_and_package", lambda: (True, ""))
    monkeypatch.setattr(
        mlx_install.psutil,
        "virtual_memory",
        lambda: SimpleNamespace(total=ram_gb * 1024**3),
    )


def _write_snapshot(
    spec: mlx_install.MLXModelSpec,
    *,
    include_gemma_config: bool = False,
    weight_bytes: bytes = b"weights",
) -> tuple[Path, str]:
    snapshot_dir = mlx_install.snapshot_dir_for_spec(spec)
    snapshot_dir.mkdir(parents=True)
    weight_name = "model-00001-of-00001.safetensors"
    (snapshot_dir / weight_name).write_bytes(weight_bytes)
    (snapshot_dir / "model.safetensors.index.json").write_text(
        json.dumps({"weight_map": {"layer.weight": weight_name}}) + "\n",
        encoding="utf-8",
    )
    (snapshot_dir / "tokenizer.json").write_text("{}\n", encoding="utf-8")
    if include_gemma_config:
        (snapshot_dir / "config.json").write_text(
            json.dumps(
                {
                    "vision_config": {
                        "default_output_length": 280,
                        "pooling_kernel_size": 3,
                        "position_embedding_size": 10240,
                    },
                    "vision_soft_tokens_per_image": 280,
                }
            )
            + "\n",
            encoding="utf-8",
        )
        (snapshot_dir / "processor_config.json").write_text(
            json.dumps(
                {
                    "image_processor": {
                        "image_processor_type": "Gemma4ImageProcessor",
                        "max_soft_tokens": 280,
                        "image_seq_length": 280,
                        "pooling_kernel_size": 3,
                    },
                    "image_seq_length": 280,
                }
            )
            + "\n",
            encoding="utf-8",
        )
    return snapshot_dir, hashlib.sha256(weight_bytes).hexdigest()


def test_module_import_is_mlx_vlm_free(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setitem(sys.modules, "mlx_vlm", None)

    reloaded = importlib.reload(mlx_install)

    assert not hasattr(reloaded, "mlx_vlm")


def test_default_model_and_registry_contents() -> None:
    assert mlx_install.resolve_model_spec().name == QWEN_35_9B
    assert set(mlx_install._MLX_MODEL_REGISTRY) == {
        QWEN_35_9B,
        GEMMA4_26B_A4B_4BIT,
    }
    assert (
        mlx_install._MLX_MODEL_REGISTRY[GEMMA4_26B_A4B_4BIT].repo
        == "mlx-community/gemma-4-26b-a4b-it-4bit"
    )


def test_install_local_mlx_writes_canonical_sequence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _init_journal(tmp_path, monkeypatch)
    _allow_install(monkeypatch)
    spec = mlx_install._MLX_MODEL_REGISTRY[GEMMA4_26B_A4B_4BIT]
    snapshot_dir, _sha = _write_snapshot(spec, include_gemma_config=True)
    variant_dir = mlx_install.variant_dir_for_snapshot(snapshot_dir)
    observed: list[tuple[str, str, dict]] = []

    def fake_available(_spec):
        observed.append(("resolve", _local_status()["install_state"], {}))
        return True, ""

    def fake_snapshot_download(*, repo_id, revision):
        assert repo_id == spec.repo
        assert revision == spec.revision
        observed.append(("download", _local_status()["install_state"], {}))
        return str(snapshot_dir)

    def fake_verify(_spec, _snapshot_dir):
        observed.append(("verify", _local_status()["install_state"], {}))

    def fake_create(_snapshot_dir):
        observed.append(("install", _local_status()["install_state"], {}))
        variant_dir.mkdir(parents=True, exist_ok=True)
        return variant_dir

    monkeypatch.setattr(mlx_install, "is_mlx_available_for_model", fake_available)
    monkeypatch.setattr(
        mlx_install.huggingface_hub, "snapshot_download", fake_snapshot_download
    )
    monkeypatch.setattr(mlx_install, "validate_snapshot_sha256", fake_verify)
    monkeypatch.setattr(mlx_install, "create_gemma4_variant", fake_create)

    result = mlx_install.install_local_mlx(GEMMA4_26B_A4B_4BIT)

    assert [entry[0] for entry in observed] == [
        "resolve",
        "download",
        "verify",
        "install",
    ]
    assert [entry[1] for entry in observed] == [
        "resolving",
        "downloading",
        "verifying",
        "installing",
    ]
    assert result["install_state"] == "installed"
    slot = _local_slot()
    assert slot["install_state"] == "installed"
    assert slot["mlx_model_id"] == GEMMA4_26B_A4B_4BIT
    assert slot["mlx_revision"] == spec.revision
    assert slot["mlx_snapshot_dir"] == str(snapshot_dir)
    assert slot["mlx_variant_dir"] == str(variant_dir)
    assert "mlx" not in read_journal_config()["providers"]


def test_unsupported_platform_fails(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    _init_journal(tmp_path, monkeypatch)
    monkeypatch.setattr(mlx_install.platform, "system", lambda: "Linux")
    monkeypatch.setattr(mlx_install.platform, "machine", lambda: "x86_64")

    with pytest.raises(mlx_install.MLXInstallUnavailableError, match="macOS"):
        mlx_install.install_local_mlx()

    assert _local_status()["install_state"] == "failed"
    assert "macOS" in _local_status()["install_error"]


def test_insufficient_ram_fails(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    _init_journal(tmp_path, monkeypatch)
    _allow_install(monkeypatch, ram_gb=1)

    with pytest.raises(
        mlx_install.MLXInstallUnavailableError, match="insufficient RAM"
    ):
        mlx_install.install_local_mlx()

    assert _local_status()["install_state"] == "failed"
    assert "insufficient RAM" in _local_status()["install_error"]


def test_download_failure_transitions_failed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _init_journal(tmp_path, monkeypatch)
    _allow_install(monkeypatch)

    def fail_download(**_kwargs):
        raise RuntimeError("download broke")

    monkeypatch.setattr(mlx_install.huggingface_hub, "snapshot_download", fail_download)

    with pytest.raises(RuntimeError, match="download broke"):
        mlx_install.install_local_mlx()

    assert _local_status()["install_state"] == "failed"
    assert _local_status()["install_error"] == "download broke"


def test_verify_failure_transitions_failed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _init_journal(tmp_path, monkeypatch)
    _allow_install(monkeypatch)
    spec = mlx_install._MLX_MODEL_REGISTRY[QWEN_35_9B]
    snapshot_dir = mlx_install.snapshot_dir_for_spec(spec)

    def fake_snapshot_download(**_kwargs):
        _write_snapshot(spec)
        return str(snapshot_dir)

    def fail_verify(_spec, _snapshot_dir):
        raise mlx_install.MLXVerificationError("verify broke")

    monkeypatch.setattr(
        mlx_install.huggingface_hub, "snapshot_download", fake_snapshot_download
    )
    monkeypatch.setattr(mlx_install, "validate_snapshot_sha256", fail_verify)

    with pytest.raises(mlx_install.MLXVerificationError, match="verify broke"):
        mlx_install.install_local_mlx()

    assert _local_status()["install_state"] == "failed"
    assert _local_status()["install_error"] == "verify broke"


def test_installing_failure_transitions_failed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _init_journal(tmp_path, monkeypatch)
    _allow_install(monkeypatch)
    spec = mlx_install._MLX_MODEL_REGISTRY[GEMMA4_26B_A4B_4BIT]
    snapshot_dir, _sha = _write_snapshot(spec)

    monkeypatch.setattr(
        mlx_install.huggingface_hub,
        "snapshot_download",
        lambda **_kwargs: str(snapshot_dir),
    )
    monkeypatch.setattr(mlx_install, "validate_snapshot_sha256", lambda *_args: None)

    with pytest.raises(FileNotFoundError):
        mlx_install.install_local_mlx(GEMMA4_26B_A4B_4BIT)

    assert _local_status()["install_state"] == "failed"
    assert "config.json" in _local_status()["install_error"]


def test_idempotent_rerun_skips_network(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _init_journal(tmp_path, monkeypatch)
    _allow_install(monkeypatch)
    spec = mlx_install._MLX_MODEL_REGISTRY[QWEN_35_9B]
    snapshot_dir = mlx_install.snapshot_dir_for_spec(spec)
    calls = {"download": 0, "verify": 0}

    def fake_snapshot_download(**_kwargs):
        calls["download"] += 1
        _write_snapshot(spec)
        return str(snapshot_dir)

    def fake_verify(_spec, _snapshot_dir):
        calls["verify"] += 1

    monkeypatch.setattr(
        mlx_install.huggingface_hub, "snapshot_download", fake_snapshot_download
    )
    monkeypatch.setattr(mlx_install, "validate_snapshot_sha256", fake_verify)

    assert mlx_install.install_local_mlx()["install_state"] == "installed"
    assert calls == {"download": 1, "verify": 1}

    assert mlx_install.install_local_mlx()["install_state"] == "installed"
    assert calls == {"download": 1, "verify": 1}
    assert _local_slot()["mlx_model_id"] == QWEN_35_9B


def test_validate_snapshot_sha256_uses_lfs_metadata(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _init_journal(tmp_path, monkeypatch)
    spec = mlx_install._MLX_MODEL_REGISTRY[QWEN_35_9B]
    snapshot_dir, sha = _write_snapshot(spec, weight_bytes=b"abc")
    calls: list[dict[str, object]] = []

    class FakeApi:
        def list_repo_tree(self, **kwargs):
            calls.append(kwargs)
            return [
                RepoFile(
                    path="model-00001-of-00001.safetensors",
                    size=3,
                    oid="oid",
                    lfs={"size": 3, "oid": sha, "pointerSize": 123},
                )
            ]

    monkeypatch.setattr(mlx_install.huggingface_hub, "HfApi", lambda: FakeApi())

    mlx_install.validate_snapshot_sha256(spec, snapshot_dir)

    assert calls == [
        {
            "repo_id": spec.repo,
            "revision": spec.revision,
            "repo_type": "model",
            "recursive": True,
        }
    ]


def test_create_gemma4_variant_rewrites_json_and_symlinks_rest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _init_journal(tmp_path, monkeypatch)
    spec = mlx_install._MLX_MODEL_REGISTRY[GEMMA4_26B_A4B_4BIT]
    snapshot_dir, _sha = _write_snapshot(
        spec,
        include_gemma_config=True,
        weight_bytes=b"gemma weights",
    )
    (snapshot_dir / "generation_config.json").write_text("{}\n", encoding="utf-8")

    variant_dir = mlx_install.create_gemma4_variant(snapshot_dir)

    config = json.loads((variant_dir / "config.json").read_text(encoding="utf-8"))
    processor_config = json.loads(
        (variant_dir / "processor_config.json").read_text(encoding="utf-8")
    )
    budget = mlx_install.MLX_SOFT_TOKEN_BUDGET
    assert config["vision_config"]["default_output_length"] == budget
    assert processor_config["image_processor"]["max_soft_tokens"] == budget
    assert processor_config["image_processor"]["image_seq_length"] == budget
    assert config["vision_config"]["default_output_length"] != 280
    assert processor_config["image_processor"]["max_soft_tokens"] != 280
    assert processor_config["image_processor"]["image_seq_length"] != 280

    symlinked = {
        "model.safetensors.index.json",
        "model-00001-of-00001.safetensors",
        "tokenizer.json",
        "generation_config.json",
    }
    for rel_path in symlinked:
        target = variant_dir / rel_path
        assert target.is_symlink()
        assert target.resolve() == (snapshot_dir / rel_path).resolve()

    real_files = sorted(
        path.relative_to(variant_dir).as_posix()
        for path in variant_dir.rglob("*")
        if path.is_file() and not path.is_symlink()
    )
    assert real_files == ["config.json", "processor_config.json"]
    assert sum((variant_dir / path).stat().st_size for path in real_files) < 64 * 1024
