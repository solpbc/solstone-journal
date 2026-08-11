#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Behavioural oracle for ``solstone.think.check``.

The native ``solstone-core check`` implementation is compared against this
captured Python behaviour.  Each case records one input vector in the shape the
native ``CheckInputs`` will deserialize, then drives the running Python command
through the existing test harness helpers.  It is deliberately a capture, not a
hand-written restatement of the readiness rules.

⚠ Regenerating requires a runnable reference tree.  This is a frozen record:
once Python no longer owns the command, the committed fixture remains the
contract the native implementation must satisfy.

Determinism: this corpus reads no clock.  It redacts exactly two host-dependent
values: package ``version`` becomes ``<version>`` and ``platform.python``
becomes ``<python-version>``.  The GPU-unknown case raises
``RuntimeError('controlled GPU failure')`` and the disk-unknown case raises
``OSError('controlled disk failure')``; their messages are controlled input and
are recorded literally, not redacted.
"""

from __future__ import annotations

import contextlib
import io
import json
from types import SimpleNamespace
from typing import Any

import pytest

from solstone.think import check
from solstone.think import utils as think_utils
from solstone.think.providers import fit_report, local_cuda, local_vulkan, memory
from tests.test_check import (
    _nvidia_probe,
    _patch_disk,
    _patch_linux_ok,
    _patch_memory,
    _patch_platform,
    _undetected_probe,
    _vulkan_device,
)

GB = 1024**3
FIXTURE_VERSION = 1
JOURNAL_PATH = "/journal"
GPU_FAILURE = "controlled GPU failure"
DISK_FAILURE = "controlled disk failure"
PLACEMENT_LINE = (
    "; sol thinks on your GPU; transcription runs on your CPU on this machine"
)


def _platform(os_name: str = "Linux", *, arch: str = "x86_64") -> dict[str, str]:
    return {"os": os_name, "os_version": "6.8.0", "arch": arch}


def _device(
    *,
    name: str = "Vulkan GPU",
    vram_mib: int = 24576,
    device_type: int = local_vulkan.VK_TYPE_DISCRETE,
) -> dict[str, int | str]:
    return {
        "index": 0,
        "name": name,
        "device_type": device_type,
        "vram_mib": vram_mib,
    }


def _undetected_nvidia() -> dict[str, Any]:
    return {
        "detected": False,
        "vram_mib": None,
        "tiering_memory_mib": None,
        "memory_source": "unavailable",
    }


def _nvidia(
    *,
    vram_mib: int | None,
    tiering_memory_mib: int | None = None,
    memory_source: str = "nvidia_vram",
) -> dict[str, Any]:
    return {
        "detected": True,
        "vram_mib": vram_mib,
        "tiering_memory_mib": (
            vram_mib if tiering_memory_mib is None else tiering_memory_mib
        ),
        "memory_source": memory_source,
    }


def _linux_inputs(
    *,
    nvidia: dict[str, Any] | None = None,
    devices: list[dict[str, Any]] | None = None,
    probe_ok: bool = True,
    total_bytes: int | None = 64 * GB,
    available_bytes: int | None = 64 * GB,
    disk: dict[str, Any] | None = None,
    inaccessible: bool = False,
    gpu_evaluation_error: str | None = None,
) -> dict[str, Any]:
    return {
        "platform": _platform(),
        "memory": {
            "total_bytes": total_bytes,
            "available_bytes": available_bytes,
        },
        "disk": disk or {"kind": "ok", "free_bytes": 500 * GB},
        "journal_path": JOURNAL_PATH,
        "nvidia": nvidia or _undetected_nvidia(),
        "vulkan": {
            "probe_ok": probe_ok,
            "devices": devices if devices is not None else [_device()],
        },
        "render_nodes_present_but_inaccessible": inaccessible,
        "gpu_evaluation_error": gpu_evaluation_error,
        "version": "<version>",
    }


def _mac_inputs(
    *,
    total_bytes: int | None,
    available_bytes: int | None,
) -> dict[str, Any]:
    return {
        "platform": {"os": "Darwin", "os_version": "14.5", "arch": "arm64"},
        "memory": {
            "total_bytes": total_bytes,
            "available_bytes": available_bytes,
        },
        "disk": {"kind": "ok", "free_bytes": 500 * GB},
        "journal_path": JOURNAL_PATH,
        "nvidia": _undetected_nvidia(),
        "vulkan": {"probe_ok": True, "devices": []},
        "render_nodes_present_but_inaccessible": False,
        "gpu_evaluation_error": None,
        "version": "<version>",
    }


def _unsupported_inputs(os_name: str, arch: str) -> dict[str, Any]:
    return {
        "platform": {"os": os_name, "os_version": "6.8.0", "arch": arch},
        "memory": {"total_bytes": 64 * GB, "available_bytes": 64 * GB},
        "disk": {"kind": "ok", "free_bytes": 500 * GB},
        "journal_path": JOURNAL_PATH,
        "nvidia": _undetected_nvidia(),
        "vulkan": {"probe_ok": True, "devices": []},
        "render_nodes_present_but_inaccessible": False,
        "gpu_evaluation_error": None,
        "version": "<version>",
    }


CASES: tuple[tuple[str, dict[str, Any]], ...] = (
    (
        "linux_render_node_inaccessible",
        _linux_inputs(devices=[], probe_ok=False, inaccessible=True),
    ),
    ("linux_vulkan_probe_not_ok", _linux_inputs(devices=[], probe_ok=False)),
    ("linux_vulkan_no_device_selected", _linux_inputs(devices=[])),
    (
        "linux_vulkan_vram_below_floor",
        _linux_inputs(devices=[_device(name="Small Vulkan GPU", vram_mib=5632)]),
    ),
    ("linux_vulkan_ok", _linux_inputs(devices=[_device()])),
    (
        "linux_nvidia_memory_unknown",
        _linux_inputs(nvidia=_nvidia(vram_mib=None)),
    ),
    (
        "linux_nvidia_vram_below_floor",
        _linux_inputs(nvidia=_nvidia(vram_mib=5632)),
    ),
    (
        "linux_nvidia_ok_cpu_placement",
        _linux_inputs(
            nvidia=_nvidia(vram_mib=6144),
            devices=[_device(vram_mib=6144)],
        ),
    ),
    (
        "linux_nvidia_unified_memory_ok",
        _linux_inputs(
            nvidia=_nvidia(
                vram_mib=None,
                tiering_memory_mib=6144,
                memory_source="system_available",
            ),
            devices=[_device(vram_mib=6144)],
        ),
    ),
    (
        "linux_gpu_outer_exception",
        _linux_inputs(gpu_evaluation_error=GPU_FAILURE),
    ),
    ("mac_memory_unknown", _mac_inputs(total_bytes=None, available_bytes=None)),
    ("mac_memory_below_floor", _mac_inputs(total_bytes=8 * GB, available_bytes=8 * GB)),
    (
        "mac_memory_available_below_mlx_floor",
        _mac_inputs(total_bytes=16 * GB, available_bytes=10 * GB),
    ),
    ("mac_memory_ok", _mac_inputs(total_bytes=24 * GB, available_bytes=20 * GB)),
    (
        "linux_ram_unknown",
        _linux_inputs(total_bytes=None, available_bytes=None),
    ),
    (
        "linux_ram_below_8_gib",
        _linux_inputs(total_bytes=5632 * 1024**2, available_bytes=5632 * 1024**2),
    ),
    (
        "disk_error",
        _linux_inputs(disk={"kind": "error", "message": DISK_FAILURE}),
    ),
    (
        "disk_below_20_gib",
        _linux_inputs(disk={"kind": "ok", "free_bytes": 10 * GB}),
    ),
    ("unsupported_windows", _unsupported_inputs("Windows", "AMD64")),
    ("unsupported_intel_macos", _unsupported_inputs("Darwin", "x86_64")),
    ("unsupported_platform_fallback", _unsupported_inputs("FreeBSD", "x86_64")),
    (
        "blocked_gpu_wins_over_ram_warning",
        _linux_inputs(devices=[], total_bytes=5632 * 1024**2, available_bytes=5632 * 1024**2),
    ),
)


def _vulkan_devices(inputs: dict[str, Any]) -> list[local_vulkan.VulkanDevice]:
    return [
        _vulkan_device(
            index=device["index"],
            name=device["name"],
            device_type=device["device_type"],
            vram_mib=device["vram_mib"],
        )
        for device in inputs["vulkan"]["devices"]
    ]


def _probe(inputs: dict[str, Any]) -> local_cuda.NvidiaProbe:
    nvidia = inputs["nvidia"]
    if not nvidia["detected"]:
        return _undetected_probe()
    source = nvidia["memory_source"]
    source_map = {
        "nvidia_vram": local_cuda.MEMORY_SOURCE_NVIDIA_VRAM,
        "system_available": local_cuda.MEMORY_SOURCE_SYSTEM_AVAILABLE,
        "unavailable": local_cuda.MEMORY_SOURCE_UNAVAILABLE,
    }
    return _nvidia_probe(
        vram_mib=nvidia["vram_mib"],
        tiering_memory_mib=nvidia["tiering_memory_mib"],
        memory_source=source_map[source],
    )


def _configure(monkeypatch: pytest.MonkeyPatch, inputs: dict[str, Any]) -> None:
    platform = inputs["platform"]
    memory_inputs = inputs["memory"]
    disk = inputs["disk"]
    is_linux = platform["os"] == "Linux"

    if is_linux:
        _patch_linux_ok(monkeypatch)
    _patch_platform(
        monkeypatch,
        os_name=platform["os"],
        arch=platform["arch"],
        release=platform["os_version"],
        mac_version=platform["os_version"] if platform["os"] == "Darwin" else "",
    )
    _patch_memory(
        monkeypatch,
        total=memory_inputs["total_bytes"],
        available=memory_inputs["available_bytes"],
    )
    _patch_disk(monkeypatch)
    monkeypatch.setattr(think_utils, "get_journal_info", lambda: (JOURNAL_PATH, "test"))
    if disk["kind"] == "error":
        message = disk["message"]

        def fail_disk(_path: object) -> int:
            raise OSError(message)

        monkeypatch.setattr(memory, "free_bytes", fail_disk)
    else:
        monkeypatch.setattr(memory, "free_bytes", lambda _path: disk["free_bytes"])

    if not is_linux:
        return

    devices = _vulkan_devices(inputs)
    if inputs["gpu_evaluation_error"] is not None:
        message = inputs["gpu_evaluation_error"]

        def fail_gpu() -> list[local_vulkan.VulkanDevice]:
            raise RuntimeError(message)

        monkeypatch.setattr(local_vulkan, "detect_gpus", fail_gpu)
    else:
        monkeypatch.setattr(local_vulkan, "detect_gpus", lambda: devices)
    monkeypatch.setattr(local_vulkan, "gpu_probe_ok", lambda: inputs["vulkan"]["probe_ok"])
    monkeypatch.setattr(local_cuda, "probe_nvidia_gpu", lambda: _probe(inputs))
    monkeypatch.setattr(
        check,
        "_render_nodes_present_but_inaccessible",
        lambda: inputs["render_nodes_present_but_inaccessible"],
    )


def _capture_main(argv: list[str] | None = None) -> tuple[str, int]:
    stdout = io.StringIO()
    with contextlib.redirect_stdout(stdout):
        exit_code = check.main([] if argv is None else argv)
    return stdout.getvalue(), exit_code


def _redact_payload(payload: dict[str, Any]) -> dict[str, Any]:
    payload["version"] = "<version>"
    payload["platform"]["python"] = "<python-version>"
    return payload


def _capture_case(name: str, inputs: dict[str, Any]) -> dict[str, Any]:
    with pytest.MonkeyPatch.context() as monkeypatch:
        _configure(monkeypatch, inputs)
        human_stdout, human_exit = _capture_main()
        json_stdout, json_exit = _capture_main(["--json"])
        if human_exit != json_exit:
            raise AssertionError(f"{name}: human/json exit mismatch")
        payload = _redact_payload(json.loads(json_stdout))
        result: dict[str, Any] = {
            "name": name,
            "inputs": inputs,
            "human_stdout": human_stdout,
            "json_payload": payload,
            "exit_code": human_exit,
        }
        if name == "linux_nvidia_ok_cpu_placement":
            gpu = next(item for item in payload["checks"] if item["name"] == "gpu")
            if PLACEMENT_LINE not in gpu["detail"]:
                raise AssertionError(f"{name}: expected CPU placement suffix")
        if name == "linux_gpu_outer_exception":
            gpu = next(item for item in payload["checks"] if item["name"] == "gpu")
            if str(RuntimeError(GPU_FAILURE)) != GPU_FAILURE or GPU_FAILURE not in gpu["detail"]:
                raise AssertionError(f"{name}: controlled GPU error was not literal")
        if name == "disk_error":
            disk = next(item for item in payload["checks"] if item["name"] == "disk")
            if str(OSError(DISK_FAILURE)) != DISK_FAILURE or DISK_FAILURE not in disk["detail"]:
                raise AssertionError(f"{name}: controlled disk error was not literal")
        if name == "linux_vulkan_no_device_selected":
            fit = fit_report._local_gpu_check(
                _undetected_probe(),
                SimpleNamespace(backend="vulkan", reason="test vector"),
                [],
                local_vulkan,
                brain_lane_active=True,
            )
            result["fit_report_severity"] = fit.severity
        return result


def build_check_fixture() -> dict[str, Any]:
    """Capture all readiness rows through the Python implementation."""
    cases = [_capture_case(name, inputs) for name, inputs in CASES]
    return {
        "fixture": "solstone-check-corpus",
        "fixture_version": FIXTURE_VERSION,
        "generated_by": "make core-fixtures",
        "cases": cases,
    }
