# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Host fit checks for provider artifact downloads."""

from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal, cast

from solstone.think.providers.memory import (
    MLX_AVAILABLE_FLOOR_BYTES,
    MemoryVerdict,
    assess_memory,
    free_bytes,
    gb_label,
)

FitSeverity = Literal["ok", "warning", "blocked", "unknown"]

# The rf-detr.cpp binary has no pinned size; the 60 MiB GGUF dominates the gate.
_RFDETR_ENGINE_BINARY_DISK_BYTES = 1024 * 1024


@dataclass(frozen=True)
class FitCheck:
    name: str
    severity: FitSeverity
    detail: str
    required_bytes: int | None = None
    available_bytes: int | None = None


@dataclass(frozen=True)
class FitReport:
    artifact: str
    checks: tuple[FitCheck, ...]

    @property
    def overall(self) -> FitSeverity:
        severities = {check.severity for check in self.checks}
        if "blocked" in severities:
            return "blocked"
        if "warning" in severities or "unknown" in severities:
            return "warning"
        return "ok"


def render_fit_report(report: FitReport) -> str:
    lines = [f"{report.artifact} fit check: {report.overall}"]
    lines.extend(
        f"[{check.severity}] {check.name}: {check.detail}" for check in report.checks
    )
    return "\n".join(lines)


def build_local_fit_report(model_id: str) -> FitReport:
    from solstone.think.providers import local_cuda, local_install, local_vulkan
    from solstone.think.providers.local import LOCAL_MODEL_SPECS, normalize_model_id

    selected_model = normalize_model_id(model_id)
    spec = LOCAL_MODEL_SPECS[selected_model]
    checks: list[FitCheck] = [_local_platform_check()]
    checks.append(
        _ram_check(
            "ram",
            spec.min_ram_bytes,
            block_below_floor=False,
            artifact_label=selected_model,
        )
    )

    probe = None
    choice = None
    devices: list[Any] = []
    brain_lane_active = True
    if sys.platform.startswith("linux"):
        probe = local_cuda.probe_nvidia_gpu()
        cuda_pin = local_install.cuda_server_pin()
        choice = local_cuda.resolve_local_backend(cuda_pin)
        unknown_server = "llama-server tarball"
        devices = local_vulkan.detect_gpus()
        try:
            from solstone.think.models import is_local_provider_needed
            from solstone.think.providers.local_endpoint import resolve_local_endpoint

            brain_lane_active = (
                is_local_provider_needed() and resolve_local_endpoint().is_bundled
            )
        except Exception:
            brain_lane_active = True
    else:
        unknown_server = "llama-server tarball"

    known_artifacts = [("GGUF model", spec.size_bytes)]
    if spec.mmproj_size_bytes is not None:
        known_artifacts.append(("mmproj", spec.mmproj_size_bytes))
    if (
        sys.platform.startswith("linux")
        and choice is not None
        and choice.backend == "cuda"
    ):
        cuda_artifact_pin = local_install.cuda_artifact_pin_for_current_platform(
            cuda_pin
        )
        if cuda_artifact_pin is not None:
            known_artifacts.append(
                ("CUDA llama-server tarball", cuda_artifact_pin.size_bytes)
            )
            unknown_artifacts: tuple[str, ...] = ()
        else:
            unknown_artifacts = (unknown_server,)
    else:
        unknown_artifacts = (unknown_server,)
    checks.append(
        _disk_check(
            "disk",
            local_install.cache_root(),
            tuple(known_artifacts),
            unknown_artifacts,
        )
    )

    if sys.platform.startswith("linux") and probe is not None and choice is not None:
        checks.append(
            _local_gpu_check(
                probe,
                choice,
                devices,
                local_vulkan,
                brain_lane_active=brain_lane_active,
                override_index=local_install.gpu_device_override(),
            )
        )

    return FitReport(artifact="local provider artifacts", checks=tuple(checks))


def build_parakeet_fit_report(
    journal_path: str | Path | None = None,
) -> FitReport:
    from solstone.think.providers import parakeet_install

    checks = (
        _parakeet_platform_check(),
        _disk_check(
            "disk",
            parakeet_install.cache_root(journal_path),
            (("parakeet GGUF model", parakeet_install.PARAKEET_MODEL_SPEC.size_bytes),),
            ("parakeet CPU server tarball", "parakeet Vulkan server tarball"),
        ),
    )
    return FitReport(artifact="parakeet.cpp artifacts", checks=checks)


def build_rfdetr_fit_report(
    journal_path: str | Path | None = None,
) -> FitReport:
    from solstone.think.providers import rfdetr_install

    checks = (
        _rfdetr_platform_check(),
        _disk_check(
            "disk",
            rfdetr_install.cache_root(journal_path),
            (
                (
                    "rf-detr GGUF model",
                    rfdetr_install.RFDETR_SPEC.model.size_bytes,
                ),
                ("rf-detr CLI binary", _RFDETR_ENGINE_BINARY_DISK_BYTES),
            ),
            (),
        ),
    )
    return FitReport(artifact="rf-detr.cpp artifacts", checks=checks)


def build_coreml_parakeet_fit_report(
    os_name: str,
    arch: str,
    cache_dir: Path,
) -> FitReport:
    checks = (
        _coreml_platform_check(os_name, arch),
        _disk_check(
            "disk",
            cache_dir,
            (),
            ("CoreML parakeet model cache",),
        ),
    )
    return FitReport(artifact="CoreML parakeet artifacts", checks=checks)


def build_mlx_fit_report(model_id: str) -> FitReport:
    from huggingface_hub import constants

    from solstone.think.providers import mlx_install

    spec = mlx_install.resolve_model_spec(model_id)
    platform_supported = mlx_install.is_mlx_platform_supported()
    checks: list[FitCheck] = [_mlx_platform_check(platform_supported)]
    checks.append(_mlx_package_check(platform_supported))
    checks.append(
        _ram_check(
            "ram",
            MLX_AVAILABLE_FLOOR_BYTES,
            block_below_floor=True,
            artifact_label=spec.name,
        )
    )
    checks.append(
        _disk_check(
            "disk",
            Path(constants.HF_HUB_CACHE),
            ((f"{spec.name} snapshot", spec.size_bytes),),
            (),
        )
    )
    return FitReport(artifact=f"MLX model {spec.name}", checks=tuple(checks))


def _local_platform_check() -> FitCheck:
    from solstone.think.providers import local_install
    from solstone.think.providers.local import LocalProviderError

    try:
        artifact_key = local_install.llama_server_artifact_key()
        local_install.pin_for_current_platform()
    except LocalProviderError as exc:
        return FitCheck("platform", "blocked", str(exc))
    return FitCheck(
        "platform",
        "ok",
        f"pinned llama-server artifact is available for {artifact_key}",
    )


def _parakeet_platform_check() -> FitCheck:
    from solstone.think.providers import parakeet_install
    from solstone.think.providers.parakeet_install import ParakeetProviderError

    try:
        artifact_key = parakeet_install.parakeet_server_artifact_key()
    except ParakeetProviderError as exc:
        return FitCheck("platform", "blocked", str(exc))
    return FitCheck(
        "platform",
        "ok",
        f"pinned parakeet.cpp artifacts are available for {artifact_key}",
    )


def _rfdetr_platform_check() -> FitCheck:
    from solstone.think.providers import rfdetr_install

    if rfdetr_install._rfdetr_platform_supported():
        return FitCheck(
            "platform",
            "ok",
            "pinned rf-detr.cpp artifacts are available for x86_64-linux",
        )
    os_name, arch = rfdetr_install._platform_info()
    return FitCheck(
        "platform",
        "blocked",
        f"rf-detr.cpp requires x86_64 Linux, got {os_name}/{arch}",
    )


def _coreml_platform_check(os_name: str, arch: str) -> FitCheck:
    if os_name == "darwin" and arch == "arm64":
        return FitCheck("platform", "ok", "CoreML parakeet supports darwin/arm64")
    return FitCheck(
        "platform",
        "blocked",
        f"CoreML parakeet requires darwin/arm64, got {os_name}/{arch}",
    )


def _mlx_platform_check(platform_supported: bool) -> FitCheck:
    if platform_supported:
        return FitCheck("platform", "ok", "Apple Silicon macOS is available")
    return FitCheck("platform", "blocked", "requires Apple Silicon macOS")


def _mlx_package_check(platform_supported: bool) -> FitCheck:
    from solstone.think.providers import mlx_install

    if not platform_supported:
        return FitCheck(
            "package",
            "unknown",
            "mlx-vlm package was not checked because the platform is unsupported",
        )
    ok, reason = mlx_install.check_platform_and_package()
    if ok:
        return FitCheck("package", "ok", "mlx-vlm package is importable")
    return FitCheck("package", "blocked", reason or "mlx-vlm package is unavailable")


def _ram_check(
    name: str,
    required_bytes: int,
    *,
    block_below_floor: bool,
    artifact_label: str,
) -> FitCheck:
    verdict = assess_memory(required_bytes, block_below_floor=block_below_floor)
    severity = cast(FitSeverity, verdict.severity)
    detail = _ram_detail(verdict, artifact_label)
    return FitCheck(
        name,
        severity,
        detail,
        required_bytes=verdict.required_bytes,
        available_bytes=verdict.available_bytes,
    )


def _ram_detail(verdict: MemoryVerdict, artifact_label: str) -> str:
    required = gb_label(verdict.required_bytes)
    if verdict.available_bytes is None:
        return f"available memory could not be verified for {artifact_label}"
    available = gb_label(verdict.available_bytes)
    if verdict.severity == "ok":
        return (
            f"{available} GB available memory meets the "
            f"{required} GB requirement for {artifact_label}"
        )
    return (
        f"insufficient RAM for {artifact_label} "
        f"(need {required} GB available, have {available} GB available)"
    )


def _disk_check(
    name: str,
    cache_path: Path,
    known_artifacts: tuple[tuple[str, int], ...],
    unknown_artifacts: tuple[str, ...],
) -> FitCheck:
    required_bytes = sum(size for _artifact, size in known_artifacts)
    unknown_detail = _unknown_artifact_detail(unknown_artifacts)
    try:
        available_bytes = free_bytes(cache_path)
    except OSError as exc:
        detail = f"available disk space could not be verified at {cache_path}: {exc}"
        if unknown_detail:
            detail = f"{detail}; {unknown_detail}"
        return FitCheck(
            name,
            "unknown",
            detail,
            required_bytes=required_bytes,
            available_bytes=None,
        )

    if available_bytes < required_bytes:
        detail = (
            "insufficient disk space for known downloads "
            f"(need {gb_label(required_bytes)} GB, "
            f"have {gb_label(available_bytes)} GB free)"
        )
        if known_artifacts:
            detail = f"{detail}: {_artifact_names(known_artifacts)}"
        if unknown_detail:
            detail = f"{detail}; {unknown_detail}"
        return FitCheck(
            name,
            "blocked",
            detail,
            required_bytes=required_bytes,
            available_bytes=available_bytes,
        )

    if unknown_artifacts:
        detail = (
            f"{gb_label(available_bytes)} GB free; known downloads need "
            f"{gb_label(required_bytes)} GB; {unknown_detail}"
        )
        return FitCheck(
            name,
            "warning",
            detail,
            required_bytes=required_bytes,
            available_bytes=available_bytes,
        )

    return FitCheck(
        name,
        "ok",
        (
            f"{gb_label(available_bytes)} GB free for "
            f"{gb_label(required_bytes)} GB known downloads"
        ),
        required_bytes=required_bytes,
        available_bytes=available_bytes,
    )


def _artifact_names(artifacts: tuple[tuple[str, int], ...]) -> str:
    return ", ".join(name for name, _size in artifacts)


def _unknown_artifact_detail(artifacts: tuple[str, ...]) -> str:
    if not artifacts:
        return ""
    return "unknown download size for " + ", ".join(artifacts)


def _local_gpu_check(
    probe: Any,
    choice: Any,
    devices: list[Any],
    local_vulkan: Any,
    *,
    brain_lane_active: bool,
    override_index: int | None = None,
) -> FitCheck:
    from solstone.think.providers import local_cuda
    from solstone.think.providers.parakeet_placement import cpu_placement_suffix

    backend = getattr(choice, "backend")
    reason = getattr(choice, "reason")
    memory_source = getattr(probe, "memory_source")
    selected = local_vulkan.select_device(devices, override_index=override_index)
    placement_suffix = cpu_placement_suffix(
        devices=devices,
        selected=selected,
        local_vulkan=local_vulkan,
        unified_memory=memory_source == local_cuda.MEMORY_SOURCE_SYSTEM_AVAILABLE,
        brain_lane_active=brain_lane_active,
    )
    if backend == "cuda":
        if not getattr(probe, "detected"):
            return FitCheck(
                "gpu",
                "unknown",
                f"NVIDIA GPU probe unavailable; resolved backend is {backend}: {reason}",
            )
        if memory_source == local_cuda.MEMORY_SOURCE_UNAVAILABLE:
            return FitCheck(
                "gpu",
                "unknown",
                f"resolved backend is {backend}: {reason}; GPU memory is unknown",
            )
        detail = f"CUDA backend selected: {reason}"
        if memory_source == local_cuda.MEMORY_SOURCE_SYSTEM_AVAILABLE:
            detail = f"{detail}; GPU tiering memory uses system MemAvailable"
        detail = f"{detail}{placement_suffix}"
        return FitCheck("gpu", "ok", detail)

    probe_ok = local_vulkan.gpu_probe_ok()
    if not probe_ok:
        return FitCheck(
            "gpu",
            "unknown",
            f"Vulkan GPU probe did not complete; resolved backend is {backend}: {reason}",
        )
    if selected is None:
        return FitCheck(
            "gpu",
            "warning",
            f"no hardware Vulkan GPU selected; resolved backend is {backend}: {reason}",
        )
    return FitCheck(
        "gpu",
        "ok",
        (
            f"Vulkan GPU selected: {selected.name}; resolved backend is {backend}: "
            f"{reason}{placement_suffix}"
        ),
    )


__all__ = [
    "FitCheck",
    "FitReport",
    "FitSeverity",
    "build_coreml_parakeet_fit_report",
    "build_local_fit_report",
    "build_mlx_fit_report",
    "build_parakeet_fit_report",
    "build_rfdetr_fit_report",
    "render_fit_report",
]
