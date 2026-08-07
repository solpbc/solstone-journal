# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CUDA GPU discovery and backend selection for the bundled local provider."""

from __future__ import annotations

import json
import logging
import re
import shutil
import subprocess
from dataclasses import dataclass
from enum import StrEnum
from typing import TYPE_CHECKING

from solstone.think import core_handshake

if TYPE_CHECKING:
    from solstone.think.providers.local_install import CudaServerPin

LOG = logging.getLogger(__name__)

_PROBE_TIMEOUT_S = 10
_ARCH_RE = re.compile(r"\bsm_\d+a?\b")
MEMORY_SOURCE_UNAVAILABLE = "unavailable"
MEMORY_SOURCE_NVIDIA_VRAM = "nvidia memory.total"
MEMORY_SOURCE_SYSTEM_AVAILABLE = "system MemAvailable (unified memory)"
# The CUDA arch set embedded in the pinned llama.cpp CUDA server image, and the
# minimum driver CUDA major version it requires. The bundled-provider CUDA pin
# reads these as its sole source of truth.
# TODO(AC10): confirm via cuobjdump --list-elf libggml-cuda.so on hardware.
CUDA_EMBEDDED_ARCH_SET: frozenset[str] = frozenset(
    {"sm_86", "sm_89", "sm_120a", "sm_121a"}
)
CUDA_MIN_DRIVER_VERSION = 13


class LocalCudaError(RuntimeError):
    """CUDA local-provider failure with a recovery reason code."""

    def __init__(self, reason_code: str, message: str) -> None:
        super().__init__(message)
        self.reason_code = reason_code


class ArtifactTrust(StrEnum):
    TRUSTED = "trusted"
    ABSENT = "absent"
    UNAVAILABLE = "unavailable"


@dataclass(frozen=True)
class NvidiaProbe:
    index: int | None
    compute_cap: str | None
    driver_cuda_version: int | None
    vram_mib: int | None
    tiering_memory_mib: int | None
    memory_source: str
    detected: bool


@dataclass(frozen=True)
class BackendChoice:
    backend: str
    reason: str


def _base_arch(arch: str) -> str:
    return arch[:-1] if arch.endswith("a") else arch


def _undetected() -> NvidiaProbe:
    return NvidiaProbe(
        index=None,
        compute_cap=None,
        driver_cuda_version=None,
        vram_mib=None,
        tiering_memory_mib=None,
        memory_source=MEMORY_SOURCE_UNAVAILABLE,
        detected=False,
    )


def has_unified_memory_name(gpu_name: str) -> bool:
    return "GB10" in gpu_name.upper()


def detect_nvidia_unified_memory() -> bool:
    if shutil.which("nvidia-smi") is None:
        return False

    try:
        completed = subprocess.run(
            ["nvidia-smi", "--query-gpu=name", "--format=csv,noheader"],
            capture_output=True,
            text=True,
            timeout=_PROBE_TIMEOUT_S,
            check=False,
        )
    except subprocess.TimeoutExpired:
        LOG.warning(
            "NVIDIA unified-memory probe timed out after %.0fs", _PROBE_TIMEOUT_S
        )
        return False
    except OSError as exc:
        LOG.warning("NVIDIA unified-memory probe could not start: %s", exc)
        return False

    if completed.returncode != 0:
        LOG.warning(
            "NVIDIA unified-memory probe exited with status %s", completed.returncode
        )
        return False

    first_row = next(
        (line.strip() for line in completed.stdout.splitlines() if line.strip()),
        None,
    )
    if first_row is None:
        LOG.warning("NVIDIA unified-memory probe returned no rows")
        return False
    return has_unified_memory_name(first_row)


def probe_nvidia_gpu(
    *,
    handshake_checker=core_handshake.check_solstone_core_handshake,
    helper_locator=core_handshake.helper_path_for_executable,
    runner=subprocess.run,
) -> NvidiaProbe:
    handshake = handshake_checker()
    if handshake.status != "ok":
        raise RuntimeError(
            "NVIDIA GPU probe requires a usable solstone-core helper: "
            f"{handshake.message or 'unknown handshake failure'}"
        )
    try:
        completed = runner(
            [str(helper_locator()), "local", "probe-nvidia"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise RuntimeError(f"solstone-core local probe-nvidia failed to launch: {exc}") from exc
    if completed.returncode != 0:
        raise RuntimeError(f"solstone-core local probe-nvidia failed: {completed.stderr}")
    payload = json.loads(completed.stdout)
    if not payload["detected"]:
        return _undetected()
    vram_mib = payload["vram_mib"]
    unified_memory_mib = payload["unified_memory_mib"]
    if vram_mib is not None:
        tiering_memory_mib = vram_mib
        memory_source = MEMORY_SOURCE_NVIDIA_VRAM
    elif unified_memory_mib is not None:
        tiering_memory_mib = unified_memory_mib
        memory_source = MEMORY_SOURCE_SYSTEM_AVAILABLE
    else:
        tiering_memory_mib = None
        memory_source = MEMORY_SOURCE_UNAVAILABLE
    return NvidiaProbe(
        index=payload["gpu_index"],
        compute_cap=payload["arch"],
        driver_cuda_version=payload["driver_cuda_major"],
        vram_mib=vram_mib,
        tiering_memory_mib=tiering_memory_mib,
        memory_source=memory_source,
        detected=True,
    )


def select_local_backend(
    probe: NvidiaProbe,
    arch_set: frozenset[str],
    cuda_version: int,
    trust: ArtifactTrust,
    *,
    persisted_installed_cuda: bool,
) -> BackendChoice:
    hardware_rejection = _hardware_backend_rejection(probe, arch_set, cuda_version)
    if hardware_rejection is not None:
        return hardware_rejection

    assert probe.compute_cap is not None
    assert probe.driver_cuda_version is not None
    cuda_reason = (
        f"compute_cap {probe.compute_cap} covered; "
        f"driver CUDA {probe.driver_cuda_version} >= {cuda_version}"
    )
    if trust == ArtifactTrust.TRUSTED or (
        trust == ArtifactTrust.UNAVAILABLE and persisted_installed_cuda
    ):
        return BackendChoice("cuda", cuda_reason)

    return BackendChoice(
        "vulkan",
        (f"{cuda_reason}; no trusted CUDA runtime artifact present"),
    )


def _hardware_backend_rejection(
    probe: NvidiaProbe,
    arch_set: frozenset[str],
    cuda_version: int,
) -> BackendChoice | None:
    if not probe.detected:
        return BackendChoice("vulkan", "no NVIDIA GPU detected")
    if probe.compute_cap is None:
        return BackendChoice("vulkan", "NVIDIA compute capability unreadable")

    base_arches = {_base_arch(arch) for arch in arch_set}
    if _base_arch(probe.compute_cap) not in base_arches:
        return BackendChoice(
            "vulkan",
            f"compute_cap {probe.compute_cap} not in CUDA image arch set",
        )

    if probe.driver_cuda_version is None:
        return BackendChoice("vulkan", "driver CUDA version unreadable")
    if probe.driver_cuda_version < cuda_version:
        return BackendChoice(
            "vulkan",
            f"driver CUDA {probe.driver_cuda_version} < required {cuda_version}",
        )
    return None


def resolve_local_backend(pin: CudaServerPin) -> BackendChoice:
    probe = probe_nvidia_gpu()
    hardware_rejection = _hardware_backend_rejection(
        probe,
        pin.embedded_arch_set,
        pin.cuda_version,
    )
    if hardware_rejection is not None:
        return hardware_rejection

    from solstone.think.providers import local_install

    trust = local_install.probe_cuda_runtime_artifact_trust(pin)
    persisted_installed_cuda = local_install.has_persisted_installed_cuda_target()
    return select_local_backend(
        probe,
        pin.embedded_arch_set,
        pin.cuda_version,
        trust,
        persisted_installed_cuda=persisted_installed_cuda,
    )


def parse_embedded_arch_set(cuobjdump_list_elf_text: str) -> frozenset[str]:
    # TODO(AC10): live cuobjdump --list-elf on the extracted libggml-cuda.so
    # runs on hardware; in-lode uses a synthetic fixture.
    return frozenset(_ARCH_RE.findall(cuobjdump_list_elf_text))


def verify_cuda_pin_arch_set(text: str, declared: frozenset[str]) -> None:
    actual = parse_embedded_arch_set(text)
    if actual != declared:
        raise LocalCudaError(
            "arch_set_mismatch",
            f"CUDA embedded arch set mismatch: declared={sorted(declared)}, actual={sorted(actual)}",
        )


__all__ = [
    "ArtifactTrust",
    "BackendChoice",
    "CUDA_EMBEDDED_ARCH_SET",
    "CUDA_MIN_DRIVER_VERSION",
    "LocalCudaError",
    "MEMORY_SOURCE_NVIDIA_VRAM",
    "MEMORY_SOURCE_SYSTEM_AVAILABLE",
    "MEMORY_SOURCE_UNAVAILABLE",
    "NvidiaProbe",
    "detect_nvidia_unified_memory",
    "has_unified_memory_name",
    "parse_embedded_arch_set",
    "probe_nvidia_gpu",
    "resolve_local_backend",
    "select_local_backend",
    "verify_cuda_pin_arch_set",
]
