#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""MLX provider for local Apple Silicon generation."""

from __future__ import annotations

import asyncio
import importlib
import logging
import platform
from dataclasses import dataclass
from typing import Any, Callable

import psutil

from solstone.think.models import QWEN_35_9B
from solstone.think.utils import get_config

from .shared import GenerateResult

GEMMA4_26B_A4B_4BIT = "gemma-4-26b-a4b-it-mlx-4bit"
_GEMMA4_SOFT_TOKENS = 1120
_GEMMA4_MIN_POSITION_EMBEDDING_SIZE = 10240


@dataclass(frozen=True)
class MLXModelSpec:
    name: str
    repo: str
    revision: str
    min_ram_bytes: int
    post_load: Callable[[Any, Any], None] | None = None


def _gemma4_post_load(model: Any, processor: Any) -> None:
    try:
        position_embedding_size = model.config.vision_config.position_embedding_size
    except AttributeError as exc:
        raise RuntimeError(
            f"{GEMMA4_26B_A4B_4BIT} requires "
            "model.config.vision_config.position_embedding_size >= "
            f"{_GEMMA4_MIN_POSITION_EMBEDDING_SIZE}; actual missing"
        ) from exc

    if position_embedding_size < _GEMMA4_MIN_POSITION_EMBEDDING_SIZE:
        raise RuntimeError(
            f"{GEMMA4_26B_A4B_4BIT} requires "
            "model.config.vision_config.position_embedding_size >= "
            f"{_GEMMA4_MIN_POSITION_EMBEDDING_SIZE}; "
            f"actual {position_embedding_size}"
        )

    pool_k = model.vision_tower.pooling_kernel_size
    max_patches = _GEMMA4_SOFT_TOKENS * pool_k * pool_k

    # The resize ratio must match max_soft_tokens=1120; otherwise gemma4 regresses
    # to the smaller visual budget and loses screenshot faithfulness.
    processor.image_processor.max_soft_tokens = _GEMMA4_SOFT_TOKENS
    if hasattr(processor.image_processor, "image_seq_length"):
        processor.image_processor.image_seq_length = _GEMMA4_SOFT_TOKENS
    if hasattr(processor, "image_seq_length"):
        processor.image_seq_length = _GEMMA4_SOFT_TOKENS
    model.vision_tower.max_patches = max_patches
    model.vision_tower.default_output_length = _GEMMA4_SOFT_TOKENS
    pooler = getattr(model.vision_tower, "pooler", None)
    if pooler is not None and hasattr(pooler, "default_output_length"):
        pooler.default_output_length = _GEMMA4_SOFT_TOKENS

    if processor.image_processor.max_soft_tokens != _GEMMA4_SOFT_TOKENS:
        raise RuntimeError(
            f"{GEMMA4_26B_A4B_4BIT} monkey-patch did not take: "
            "processor.image_processor.max_soft_tokens"
        )
    if model.vision_tower.max_patches != max_patches:
        raise RuntimeError(
            f"{GEMMA4_26B_A4B_4BIT} monkey-patch did not take: "
            "model.vision_tower.max_patches"
        )
    if model.vision_tower.default_output_length != _GEMMA4_SOFT_TOKENS:
        raise RuntimeError(
            f"{GEMMA4_26B_A4B_4BIT} monkey-patch did not take: "
            "model.vision_tower.default_output_length"
        )


_MLX_MODEL_REGISTRY: dict[str, MLXModelSpec] = {
    QWEN_35_9B: MLXModelSpec(
        name=QWEN_35_9B,
        repo="mlx-community/Qwen3.5-9B-MLX-8bit",
        revision="84f7c2deea248d8df56240f88102def51c7ed5d6",
        min_ram_bytes=16 * 1024**3,
        post_load=None,
    ),
    GEMMA4_26B_A4B_4BIT: MLXModelSpec(
        name=GEMMA4_26B_A4B_4BIT,
        repo="mlx-community/gemma-4-26b-a4b-it-4bit",
        revision="efbeee6e582ebfd06abc9d65e90839c4b5d2116b",
        min_ram_bytes=24 * 1024**3,
        post_load=_gemma4_post_load,
    ),
}

logger = logging.getLogger(__name__)

_module_level_cache: dict[str, tuple[Any, Any, Any]] = {}


class ModelSnapshotMissingError(RuntimeError):
    """Raised when the pinned MLX model snapshot is not available locally."""

    def __str__(self) -> str:
        text = super().__str__()
        if "model snapshot not present" in text:
            return text
        return f"model snapshot not present: {text}"


def _platform_unsupported_reason() -> str | None:
    if platform.system() != "Darwin":
        return "not running on macOS"
    if platform.machine() != "arm64":
        return "not running on Apple Silicon"
    return None


def _check_platform_and_package() -> tuple[bool, str]:
    platform_reason = _platform_unsupported_reason()
    if platform_reason is not None:
        return False, platform_reason

    try:
        importlib.import_module("mlx_vlm")
    except ImportError:
        return False, "mlx-vlm package not installed"

    return True, ""


def is_mlx_platform_supported() -> bool:
    """True when the host is Apple Silicon macOS. Does not import mlx_vlm."""
    return _platform_unsupported_reason() is None


def is_mlx_available() -> tuple[bool, str]:
    ok, reason = _check_platform_and_package()
    if not ok:
        return ok, reason
    spec = _MLX_MODEL_REGISTRY[QWEN_35_9B]
    total_ram = psutil.virtual_memory().total
    if total_ram < spec.min_ram_bytes:
        return False, f"insufficient RAM (need 16 GB, have {total_ram // 1024**3} GB)"
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


def _snapshot_missing_error(spec: MLXModelSpec) -> ModelSnapshotMissingError:
    return ModelSnapshotMissingError(
        f"model snapshot not present at {spec.repo}@{spec.revision}"
    )


def _resolve_default_model() -> str:
    try:
        active_model = get_config()["providers"]["mlx"]["active_model"]
    except (KeyError, TypeError):
        return QWEN_35_9B
    except Exception as exc:
        logger.warning("Failed to resolve MLX active model: %s", exc)
        return QWEN_35_9B

    if active_model in _MLX_MODEL_REGISTRY:
        return active_model
    if isinstance(active_model, str):
        logger.warning(
            "Unknown MLX active model %r in journal config; falling back to %s",
            active_model,
            QWEN_35_9B,
        )
    return QWEN_35_9B


def _is_snapshot_missing_oserror(exc: OSError) -> bool:
    message = str(exc).lower()
    return any(
        marker in message
        for marker in (
            "not cached",
            "no cached",
            "could not find the requested files in the disk cache",
            "cannot find the requested files in the disk cache",
        )
    )


def _load_model(model_name: str) -> tuple[Any, Any, Any]:
    if model_name in _module_level_cache:
        return _module_level_cache[model_name]
    spec = _MLX_MODEL_REGISTRY.get(model_name)
    if spec is None:
        raise ValueError(
            f"unknown MLX model: {model_name!r}; known: {sorted(_MLX_MODEL_REGISTRY)}"
        )

    import mlx_vlm
    from huggingface_hub.errors import LocalEntryNotFoundError

    try:
        model, processor = mlx_vlm.load(spec.repo, revision=spec.revision)
    except LocalEntryNotFoundError as exc:
        raise _snapshot_missing_error(spec) from exc
    except OSError as exc:
        if _is_snapshot_missing_oserror(exc):
            raise _snapshot_missing_error(spec) from exc
        raise

    if spec.post_load is not None:
        spec.post_load(model, processor)

    config = model.config
    loaded = (model, processor, config)
    _module_level_cache[model_name] = loaded
    return loaded


def _split_contents(contents: str | list[Any]) -> tuple[str, list[Any]]:
    from PIL import Image

    text_parts: list[str] = []
    images: list[Any] = []

    def visit(value: Any) -> None:
        if isinstance(value, Image.Image):
            images.append(value)
        elif isinstance(value, str):
            text = value.strip()
            if text:
                text_parts.append(text)
        elif isinstance(value, dict) and "content" in value:
            visit(value["content"])
        elif isinstance(value, (list, tuple)):
            for item in value:
                visit(item)
        elif value is not None:
            text = str(value).strip()
            if text:
                text_parts.append(text)

    visit(contents)
    return "\n\n".join(text_parts), images


def _build_messages(
    system_instruction: str | None, text_prompt: str
) -> list[dict[str, str]]:
    messages: list[dict[str, str]] = []
    if system_instruction:
        messages.append({"role": "system", "content": system_instruction})
    messages.append({"role": "user", "content": text_prompt})
    return messages


def _get_tokenizer(processor: Any) -> Any:
    return processor.tokenizer if hasattr(processor, "tokenizer") else processor


def _build_logits_processors(processor: Any, json_schema: dict | None) -> list[Any]:
    if json_schema is None:
        return []
    structured = importlib.import_module("mlx_vlm.structured")
    logits_processor = structured.build_json_schema_logits_processor(
        _get_tokenizer(processor),
        json_schema,
    )
    return [logits_processor]


def _normalize_finish_reason(raw: Any) -> str | None:
    if raw is None:
        return None
    reason = str(raw).lower()
    if reason in {"stop", "eos", "end", "finished"}:
        return "stop"
    if reason in {"max_tokens", "length", "max_length"}:
        return "max_tokens"
    return reason


def _extract_finish_reason(result: Any, max_output_tokens: int) -> str | None:
    for attr in ("finish_reason", "stop_reason", "done_reason"):
        if hasattr(result, attr):
            normalized = _normalize_finish_reason(getattr(result, attr))
            if normalized:
                return normalized

    generation_tokens = getattr(result, "generation_tokens", None)
    if isinstance(generation_tokens, int) and generation_tokens >= max_output_tokens:
        return "max_tokens"

    # mlx-vlm GenerationResult does not expose a stop reason when it ends normally.
    return None


def _extract_usage(result: Any) -> dict[str, int] | None:
    input_tokens = getattr(result, "prompt_tokens", None)
    output_tokens = getattr(result, "generation_tokens", None)
    total_tokens = getattr(result, "total_tokens", None)
    if not isinstance(input_tokens, int) or not isinstance(output_tokens, int):
        return None
    if not isinstance(total_tokens, int):
        total_tokens = input_tokens + output_tokens
    return {
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens,
    }


def run_generate(
    contents: str | list[Any],
    model: str | None = None,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    thinking_budget: int | None = None,
    json_schema: dict | None = None,
    timeout_s: float | None = None,
    **kwargs: Any,
) -> GenerateResult:
    if model is None:
        model = _resolve_default_model()
    mlx_model, processor, config = _load_model(model)
    text_prompt, images = _split_contents(contents)
    messages = _build_messages(system_instruction, text_prompt)

    import mlx_vlm

    templated_prompt = mlx_vlm.apply_chat_template(
        processor,
        config,
        prompt=messages,
        num_images=len(images),
        enable_thinking=False,
        add_generation_prompt=True,
    )
    logits_processors = _build_logits_processors(processor, json_schema)
    generate_kwargs: dict[str, Any] = {}
    if logits_processors:
        generate_kwargs["logits_processors"] = logits_processors

    result = mlx_vlm.generate(
        mlx_model,
        processor,
        prompt=templated_prompt,
        image=images if images else None,
        max_tokens=max_output_tokens,
        temperature=temperature,
        **generate_kwargs,
    )

    return GenerateResult(
        text=result.text,
        usage=_extract_usage(result),
        finish_reason=_extract_finish_reason(result, max_output_tokens),
        thinking=None,
    )


async def run_agenerate(
    contents: str | list[Any],
    model: str | None = None,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: str | None = None,
    json_output: bool = False,
    thinking_budget: int | None = None,
    json_schema: dict | None = None,
    timeout_s: float | None = None,
    **kwargs: Any,
) -> GenerateResult:
    if model is None:
        model = _resolve_default_model()
    # mlx-vlm generation is synchronous, so async callers run it in a worker thread.
    return await asyncio.to_thread(
        run_generate,
        contents=contents,
        model=model,
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        system_instruction=system_instruction,
        json_output=json_output,
        thinking_budget=thinking_budget,
        json_schema=json_schema,
        timeout_s=timeout_s,
        **kwargs,
    )


async def run_cogitate(
    config: dict[str, Any],
    on_event: Callable[[dict], None] | None = None,
) -> str:
    raise RuntimeError(
        "MLX provider does not support cogitate in v1 — it is vision/generate-only. "
        "Configure a cloud provider for cogitate agents."
    )


def list_models(provider: str) -> list[str]:
    del provider
    return list(_MLX_MODEL_REGISTRY.keys())


def validate_key(provider: str, api_key: str) -> dict:
    del provider, api_key
    return {"valid": True}


__all__ = [
    "GEMMA4_26B_A4B_4BIT",
    "MLXModelSpec",
    "ModelSnapshotMissingError",
    "QWEN_35_9B",
    "is_mlx_available",
    "is_mlx_available_for_model",
    "is_mlx_platform_supported",
    "list_models",
    "run_agenerate",
    "run_cogitate",
    "run_generate",
    "validate_key",
]
