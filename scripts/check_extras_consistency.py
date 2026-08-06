#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Lint: the thin-base / journal leaf package menu stays internally consistent.

After the package split, the root solstone distribution owns the public POSIX
`sol` / `solstone` launchers and solstone-core owns their native sibling binary.
`solstone-journal` and `solstone-journal-cuda` are leaf packages that own the
host-only `journal` and `mlx-vlm-server` console scripts and compose the root
`[journal-host]` building block with exactly one ONNX runtime.

The invariants are:

  1. Base `[project.dependencies]` is exactly the thin access partition plus
     marker-gated solstone-core pins for covered platforms and the unsupported
     platform tombstone for the complement. No heavy host dependency may leak
     into base.
  2. There is no `[all]` extra.
  3. Root `[journal]` and `[journal-cuda]` are tombstones pinned exactly to
     `solstone-journal-host==0.7.0`.
  4. `[journal-host]` stays in root, folds in the `[pdf]` building block, pins
     `solstone-journal-models==<models leaf version>`, and pins the tested
     LiteLLM runtime used by OpenHands.
  5. The CPU leaf depends on `solstone[journal-host]==<root version>`, pulls
     CPU `onnxruntime`, and does not pull `onnxruntime-gpu`.
  6. The CUDA leaf depends on `solstone[journal-host]==<root version>`, pulls
     `onnxruntime-gpu` plus the seven NVIDIA CUDA wheels, and does not pull CPU
     `onnxruntime`.
  7. The two leaves never depend on each other.
  8. Both leaves own exactly the host-only console scripts.
  9. Root `[project.scripts]` stays absent and root script-files stay exactly the
     public POSIX launchers.
 10. Each leaf has metadata-only setuptools config, a workspace source for
     `solstone`, the expected package name, and the root version.
 11. uv workspace members/sources are exactly the two journal leaves plus
     models plus core plus the speakers analyze helper; `solstone-journal-host`
     is absent.
 12. `[tool.uv].override-dependencies` contains both tombstone pins.
 13. The Makefile no longer uses root journal extra spellings.
 14. The speakers analyze helper is a workspace-only maturin bin leaf with
     staged wheel data and no Python dependency surface.
"""

import sys
import tomllib
from pathlib import Path

try:
    from scripts.check_wheel_contents import ROOT_LAUNCHER_NAMES
except ModuleNotFoundError:  # pragma: no cover - direct script execution path.
    from check_wheel_contents import ROOT_LAUNCHER_NAMES  # type: ignore[no-redef]
from solstone.think.features import FEATURES
from solstone.think.probe import (
    solstone_core_marker_pins,
    solstone_core_speakers_analyze_marker_pins,
    solstone_core_unsupported_platform_pin,
)

# The thin access partition. Adding anything here must keep the `sol` access
# commands import-clean (scripts/check_access_imports_clean.py) — keep this in
# lockstep with pyproject's [project.dependencies].
THIN_BASE = {
    "setproctitle",
    "typer",
    "requests",
    "psutil",
    "userpath>=1.9.2,<2",
}
ROOT_SCRIPT_FILES = tuple(
    f"scripts/root-launchers/{name}" for name in ROOT_LAUNCHER_NAMES
)
HOST_SCRIPTS = {
    "journal": "solstone.think.sol_cli:journal_main",
    "mlx-vlm-server": "solstone.think.providers.mlx_server:main",
    "solstone-generate-wire": "solstone.think.generate_wire:main",
}
TOMBSTONE_PIN = "solstone-journal-host==0.7.0"
LITELLM_PIN = "litellm==1.86.1"
PDF_META_EXTRA = [
    "solstone[pdf-import]",
    "solstone[pdf-export]",
]
DIST_TO_IMPORT_NAME = {
    "pillow": "PIL",
}
CPU_ONNXRUNTIME_DEPS = {
    "onnxruntime>=1.20.0,!=1.24.1",
    "onnxruntime>=1.25.0,!=1.24.1; sys_platform == 'linux' and platform_machine == 'x86_64'",
}
CUDA_ONNXRUNTIME_DEP = "onnxruntime-gpu>=1.25.0"
NVIDIA_CUDA_DEPS = {
    "nvidia-cuda-runtime-cu12",
    "nvidia-cudnn-cu12",
    "nvidia-cublas-cu12",
    "nvidia-cufft-cu12",
    "nvidia-curand-cu12",
    "nvidia-cuda-nvrtc-cu12",
    "nvidia-nvjitlink-cu12",
}
WORKSPACE_MEMBERS = [
    "packages/solstone-journal",
    "packages/solstone-journal-cuda",
    "packages/solstone-journal-models",
    "packages/solstone-core",
    "packages/solstone-core-speakers-analyze",
    "packages/solstone-core-describe",
]
WORKSPACE_SOURCES = {
    "solstone-journal",
    "solstone-journal-cuda",
    "solstone-journal-models",
    "solstone-core",
    "solstone-core-speakers-analyze",
    "solstone-core-describe",
}
SPEAKERS_ANALYZE_CACHE_KEYS = [
    {"file": "pyproject.toml"},
    {"file": "wheel-data/**"},
    {"file": "../../scripts/stage_speakers_analyze_runtime.py"},
    {"file": "../../core/Cargo.toml"},
    {"file": "../../core/Cargo.lock"},
    {"file": "../../core/crates/solstone-core-speakers/Cargo.toml"},
    {"file": "../../core/crates/solstone-core-speakers/**/*.rs"},
    {"file": "../../core/crates/solstone-core-speakers-onnx/Cargo.toml"},
    {"file": "../../core/crates/solstone-core-speakers-onnx/**/*.rs"},
    {"file": "../../core/crates/solstone-core-speakers-analyze/Cargo.toml"},
    {"file": "../../core/crates/solstone-core-speakers-analyze/build.rs"},
    {"file": "../../core/crates/solstone-core-speakers-analyze/**/*.rs"},
]
DESCRIBE_CACHE_KEYS = [
    {"file": "pyproject.toml"},
    {"file": "../../core/Cargo.toml"},
    {"file": "../../core/Cargo.lock"},
    {"file": "../../core/crates/**/Cargo.toml"},
    {"file": "../../core/crates/**/*.rs"},
    {"file": "../../core/fixtures/**"},
]


def _names(reqs: list[str]) -> set[str]:
    """Bare distribution names (drop version specifiers and markers)."""
    out = set()
    for r in reqs:
        head = r.split(";", 1)[0].strip()
        for sep in ("[", ">", "<", "=", "!", "~", " "):
            head = head.split(sep, 1)[0]
        out.add(head.strip().lower())
    return out


def _import_names(reqs: list[str]) -> set[str]:
    return {DIST_TO_IMPORT_NAME.get(name, name) for name in _names(reqs)}


def _check_models_pin(extras: dict, member_version: str | None) -> list[str]:
    """Return errors for the journal models distribution pin."""
    host = extras.get("journal-host", [])
    pins = [dep for dep in host if dep.startswith("solstone-journal-models==")]
    if len(pins) != 1:
        return [
            "[journal-host] must contain exactly one solstone-journal-models== pin; "
            f"found {len(pins)}"
        ]
    if (
        member_version is not None
        and pins[0] != f"solstone-journal-models=={member_version}"
    ):
        return [
            "[journal-host] models pin must be "
            f"solstone-journal-models=={member_version}; found {pins[0]}"
        ]
    return []


def _check_core_pins(base: list[str], root_version: str | None) -> list[str]:
    pins = sorted(dep for dep in base if dep.startswith("solstone-core=="))
    expected = sorted(solstone_core_marker_pins(root_version or ""))
    if len(pins) != len(expected):
        return [
            "base [project.dependencies] must contain exactly "
            f"{len(expected)} marker-gated solstone-core== pins; found {len(pins)}"
        ]
    if root_version is not None and pins != expected:
        return [
            "base [project.dependencies] solstone-core marker pins must be exactly "
            f"{expected}; found {pins}"
        ]
    return []


def _check_core_unsupported_pin(base: list[str], root_version: str | None) -> list[str]:
    pins = [
        dep for dep in base if dep.startswith("solstone-core-unsupported-platform==")
    ]
    expected = solstone_core_unsupported_platform_pin(root_version or "")
    if len(pins) != 1:
        return [
            "base [project.dependencies] must contain exactly one "
            "solstone-core-unsupported-platform== pin; found "
            f"{len(pins)}"
        ]
    if root_version is not None and pins[0] != expected:
        return [
            "base [project.dependencies] unsupported-platform tombstone pin must be "
            f"{expected}; found {pins[0]}"
        ]
    return []


def _check_speakers_analyze_pins(
    *, label: str, deps: list[str], root_version: str | None
) -> list[str]:
    pins = sorted(
        dep for dep in deps if dep.startswith("solstone-core-speakers-analyze==")
    )
    expected = sorted(solstone_core_speakers_analyze_marker_pins(root_version or ""))
    if len(pins) != len(expected):
        return [
            f"{label} must contain exactly {len(expected)} marker-gated "
            "solstone-core-speakers-analyze== pins; found "
            f"{len(pins)}"
        ]
    if root_version is not None and pins != expected:
        return [
            f"{label} solstone-core-speakers-analyze marker pins must be exactly "
            f"{expected}; found {pins}"
        ]
    return []


def _read_toml(path: Path, root: Path, errors: list[str]) -> dict:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        errors.append(f"missing pyproject: {path.relative_to(root)}")
    except tomllib.TOMLDecodeError as exc:
        errors.append(f"invalid TOML in {path.relative_to(root)}: {exc}")
    return {}


def _leaf_dependencies(
    *,
    label: str,
    data: dict,
    expected_name: str,
    root_version: str | None,
    errors: list[str],
) -> list[str]:
    project = data.get("project", {})
    tool = data.get("tool", {})
    setuptools = tool.get("setuptools", {})
    uv = tool.get("uv", {})
    deps = project.get("dependencies", [])

    if project.get("name") != expected_name:
        errors.append(f"{label} [project].name must be {expected_name!r}")
    if project.get("version") != root_version:
        errors.append(
            f"{label} [project].version must match root version {root_version}; "
            f"found {project.get('version')!r}"
        )
    if root_version:
        expected_pin = f"solstone[journal-host]=={root_version}"
        pins = [dep for dep in deps if dep.startswith("solstone[journal-host]==")]
        if len(pins) != 1:
            errors.append(
                f"{label} must contain exactly one solstone[journal-host]== pin; found {len(pins)}"
            )
        elif pins[0] != expected_pin:
            errors.append(f"{label} host pin must be {expected_pin}; found {pins[0]}")
    if project.get("scripts", {}) != HOST_SCRIPTS:
        errors.append(
            f"{label} [project.scripts] must be exactly {HOST_SCRIPTS}; "
            f"found {project.get('scripts', {})}"
        )
    if setuptools.get("packages") != []:
        errors.append(f"{label} [tool.setuptools].packages must be []")
    if setuptools.get("py-modules") != []:
        errors.append(f"{label} [tool.setuptools].py-modules must be []")
    if uv.get("sources", {}).get("solstone") != {"workspace": True}:
        errors.append(
            f"{label} [tool.uv.sources].solstone must be {{workspace = true}}"
        )
    return deps


def _check_core_leaf(
    *,
    data: dict,
    root_version: str | None,
    errors: list[str],
) -> None:
    project = data.get("project", {})
    build_system = data.get("build-system", {})
    tool = data.get("tool", {})
    maturin = tool.get("maturin", {})

    if project.get("name") != "solstone-core":
        errors.append("core leaf [project].name must be 'solstone-core'")
    if project.get("version") != root_version:
        errors.append(
            f"core leaf [project].version must match root version {root_version}; "
            f"found {project.get('version')!r}"
        )
    if project.get("scripts", {}) != {}:
        errors.append("core leaf must not define [project.scripts]")
    if build_system.get("build-backend") != "maturin":
        errors.append("core leaf [build-system].build-backend must be 'maturin'")
    requires = build_system.get("requires", [])
    expected_requires = ["maturin==1.14.1"]
    if requires != expected_requires:
        errors.append(
            "core leaf [build-system].requires mismatch\n"
            f"  expected: {expected_requires!r}\n"
            f"  actual: {requires!r}\n"
            "  repair command: edit packages/solstone-core/pyproject.toml to "
            'requires = ["maturin==1.14.1"]'
        )
    if maturin.get("bindings") != "bin":
        errors.append("core leaf [tool.maturin].bindings must be 'bin'")
    if maturin.get("manifest-path") != "../../core/crates/solstone-core/Cargo.toml":
        errors.append(
            "core leaf [tool.maturin].manifest-path must be "
            "'../../core/crates/solstone-core/Cargo.toml'"
        )
    if maturin.get("profile") != "release":
        errors.append("core leaf [tool.maturin].profile must be 'release'")
    if maturin.get("strip") is not True:
        errors.append("core leaf [tool.maturin].strip must be true")


def _check_speakers_analyze_leaf(
    *,
    data: dict,
    root_version: str | None,
    errors: list[str],
) -> None:
    project = data.get("project", {})
    build_system = data.get("build-system", {})
    tool = data.get("tool", {})
    maturin = tool.get("maturin", {})
    uv = tool.get("uv", {})

    if project.get("name") != "solstone-core-speakers-analyze":
        errors.append(
            "speakers analyze leaf [project].name must be "
            "'solstone-core-speakers-analyze'"
        )
    if project.get("version") != root_version:
        errors.append(
            "speakers analyze leaf [project].version must match root version "
            f"{root_version}; found {project.get('version')!r}"
        )
    if project.get("dependencies", []) != []:
        errors.append("speakers analyze leaf must not define [project].dependencies")
    if project.get("scripts", {}) != {}:
        errors.append("speakers analyze leaf must not define [project.scripts]")
    if build_system.get("build-backend") != "maturin":
        errors.append(
            "speakers analyze leaf [build-system].build-backend must be 'maturin'"
        )
    requires = build_system.get("requires", [])
    expected_requires = ["maturin==1.14.1"]
    if requires != expected_requires:
        errors.append(
            "speakers analyze leaf [build-system].requires mismatch\n"
            f"  expected: {expected_requires!r}\n"
            f"  actual: {requires!r}\n"
            "  repair command: edit "
            "packages/solstone-core-speakers-analyze/pyproject.toml to "
            'requires = ["maturin==1.14.1"]'
        )
    if maturin.get("bindings") != "bin":
        errors.append("speakers analyze leaf [tool.maturin].bindings must be 'bin'")
    if (
        maturin.get("manifest-path")
        != "../../core/crates/solstone-core-speakers-analyze/Cargo.toml"
    ):
        errors.append(
            "speakers analyze leaf [tool.maturin].manifest-path must be "
            "'../../core/crates/solstone-core-speakers-analyze/Cargo.toml'"
        )
    if maturin.get("profile") != "release":
        errors.append("speakers analyze leaf [tool.maturin].profile must be 'release'")
    if maturin.get("strip") is not True:
        errors.append("speakers analyze leaf [tool.maturin].strip must be true")
    if maturin.get("data") != "wheel-data":
        errors.append("speakers analyze leaf [tool.maturin].data must be 'wheel-data'")
    if uv.get("cache-keys") != SPEAKERS_ANALYZE_CACHE_KEYS:
        errors.append(
            "speakers analyze leaf [tool.uv].cache-keys must match the "
            "declared native build inputs"
        )


def _check_describe_leaf(
    *,
    data: dict,
    root_version: str | None,
    errors: list[str],
) -> None:
    project = data.get("project", {})
    build_system = data.get("build-system", {})
    tool = data.get("tool", {})
    maturin = tool.get("maturin", {})
    uv = tool.get("uv", {})

    if project.get("name") != "solstone-core-describe":
        errors.append(
            "describe leaf [project].name must be 'solstone-core-describe'"
        )
    if project.get("version") != root_version:
        errors.append(
            "describe leaf [project].version must match root version "
            f"{root_version}; found {project.get('version')!r}"
        )
    if project.get("dependencies", []) != []:
        errors.append("describe leaf must not define [project].dependencies")
    if project.get("scripts", {}) != {}:
        errors.append("describe leaf must not define [project.scripts]")
    if build_system.get("build-backend") != "maturin":
        errors.append("describe leaf [build-system].build-backend must be 'maturin'")
    expected_requires = ["maturin==1.14.1"]
    requires = build_system.get("requires", [])
    if requires != expected_requires:
        errors.append(
            "describe leaf [build-system].requires mismatch\n"
            f"  expected: {expected_requires!r}\n"
            f"  actual: {requires!r}\n"
            "  repair command: edit packages/solstone-core-describe/pyproject.toml "
            'to requires = ["maturin==1.14.1"]'
        )
    if maturin.get("bindings") != "bin":
        errors.append("describe leaf [tool.maturin].bindings must be 'bin'")
    if (
        maturin.get("manifest-path")
        != "../../core/crates/solstone-core-describe/Cargo.toml"
    ):
        errors.append(
            "describe leaf [tool.maturin].manifest-path must be "
            "'../../core/crates/solstone-core-describe/Cargo.toml'"
        )
    if maturin.get("profile") != "release":
        errors.append("describe leaf [tool.maturin].profile must be 'release'")
    if maturin.get("strip") is not True:
        errors.append("describe leaf [tool.maturin].strip must be true")
    if "data" in maturin:
        errors.append("describe leaf [tool.maturin] must not define data")
    if uv.get("cache-keys") != DESCRIBE_CACHE_KEYS:
        errors.append(
            "describe leaf [tool.uv].cache-keys must match the declared native "
            "build inputs"
        )


def main(root: Path | None = None) -> int:
    root = Path(root) if root is not None else Path(__file__).resolve().parent.parent
    pyproject = root / "pyproject.toml"
    cpu_pyproject = root / "packages" / "solstone-journal" / "pyproject.toml"
    cuda_pyproject = root / "packages" / "solstone-journal-cuda" / "pyproject.toml"
    models_pyproject = root / "packages" / "solstone-journal-models" / "pyproject.toml"
    core_pyproject = root / "packages" / "solstone-core" / "pyproject.toml"
    speakers_analyze_pyproject = (
        root / "packages" / "solstone-core-speakers-analyze" / "pyproject.toml"
    )
    describe_pyproject = root / "packages" / "solstone-core-describe" / "pyproject.toml"
    makefile = root / "Makefile"
    errors: list[str] = []

    data = _read_toml(pyproject, root, errors)
    cpu_data = _read_toml(cpu_pyproject, root, errors)
    cuda_data = _read_toml(cuda_pyproject, root, errors)
    models_data = _read_toml(models_pyproject, root, errors)
    core_data = _read_toml(core_pyproject, root, errors)
    speakers_analyze_data = _read_toml(speakers_analyze_pyproject, root, errors)
    describe_data = _read_toml(describe_pyproject, root, errors)

    project = data.get("project", {})
    root_version = project.get("version")
    base = project.get("dependencies", [])
    extras = project.get("optional-dependencies", {})
    root_tool = data.get("tool", {})
    root_uv = root_tool.get("uv", {})
    models_version = models_data.get("project", {}).get("version")

    if not isinstance(root_version, str) or not root_version:
        errors.append("root [project].version must be a non-empty string")
        root_version = None
    if not isinstance(models_version, str) or not models_version:
        errors.append(
            "models [project].version must be a non-empty string "
            f"in {models_pyproject.relative_to(root)}"
        )
        models_version = None

    expected_base = (
        THIN_BASE
        | set(solstone_core_marker_pins(root_version or ""))
        | {solstone_core_unsupported_platform_pin(root_version or "")}
    )

    # 1. Base stays exactly the thin access partition plus native-core
    # platform split.
    if set(base) != expected_base:
        missing = sorted(expected_base - set(base))
        unexpected = sorted(set(base) - expected_base)
        errors.append("base [project.dependencies] drifted from the thin partition")
        if unexpected:
            errors.append(
                f"  unexpected in base (move to [journal-host]?): {unexpected}"
            )
        if missing:
            errors.append(f"  missing from base: {missing}")

    # 2. [all] is retired.
    if "all" in extras:
        errors.append("[all] extra must be removed")

    for name in (
        "pdf-import",
        "pdf-export",
        "pdf",
        "journal",
        "journal-cuda",
        "journal-host",
    ):
        if name not in extras:
            errors.append(f"missing required extra: [{name}]")

    for name in ("pdf-import", "pdf-export"):
        if name in extras and name in FEATURES:
            feature_modules = set(FEATURES[name].pip_modules)
            extra_modules = _import_names(extras[name])
            if extra_modules != feature_modules:
                errors.append(
                    f"[{name}] package set must match features.py pip_modules "
                    f"{sorted(feature_modules)}; found {sorted(extra_modules)}"
                )

    if extras.get("pdf") != PDF_META_EXTRA:
        errors.append(f"[pdf] must be exactly {PDF_META_EXTRA!r}")

    # 3. Root user-facing journal extras are tombstones.
    for name in ("journal", "journal-cuda"):
        if extras.get(name) != [TOMBSTONE_PIN]:
            errors.append(f"[{name}] must be exactly [{TOMBSTONE_PIN!r}]")

    # 4. journal-host folds pdf and pins models plus the tested OpenHands
    # runtime. OpenHands leaves LiteLLM broad, so an unconstrained fresh install
    # can drift beyond the version exercised by this repository's lockfile.
    if "journal-host" in extras:
        host = extras["journal-host"]
        if "solstone[pdf]" not in host:
            errors.append("[journal-host] must fold in solstone[pdf]")
        errors.extend(_check_models_pin(extras, models_version))
        errors.extend(_check_core_pins(base, root_version))
        errors.extend(_check_core_unsupported_pin(base, root_version))
        host_core_pins = [
            dep
            for dep in host
            if dep.startswith(
                ("solstone-core==", "solstone-core-unsupported-platform==")
            )
        ]
        if host_core_pins:
            errors.append(
                "[journal-host] must not contain native-core platform pins; "
                f"found {host_core_pins}"
            )
        litellm_requirements = [
            dep
            for dep in host
            if dep.split(";", 1)[0].strip().lower().startswith("litellm")
        ]
        if litellm_requirements != [LITELLM_PIN]:
            errors.append(
                f"[journal-host] must contain exactly {LITELLM_PIN!r}; "
                f"found {litellm_requirements}"
            )

    if "scripts" in project:
        errors.append(
            f"root [project.scripts] must be absent; found {project['scripts']}"
        )
    root_script_files = tuple(
        data.get("tool", {}).get("setuptools", {}).get("script-files", ())
    )
    if root_script_files != ROOT_SCRIPT_FILES:
        errors.append(
            "root [tool.setuptools] script-files must be exactly "
            f"{ROOT_SCRIPT_FILES}; found {root_script_files}"
        )

    cpu_deps = _leaf_dependencies(
        label="CPU leaf",
        data=cpu_data,
        expected_name="solstone-journal",
        root_version=root_version,
        errors=errors,
    )
    cuda_deps = _leaf_dependencies(
        label="CUDA leaf",
        data=cuda_data,
        expected_name="solstone-journal-cuda",
        root_version=root_version,
        errors=errors,
    )
    _check_core_leaf(data=core_data, root_version=root_version, errors=errors)
    _check_speakers_analyze_leaf(
        data=speakers_analyze_data,
        root_version=root_version,
        errors=errors,
    )
    _check_describe_leaf(
        data=describe_data,
        root_version=root_version,
        errors=errors,
    )
    errors.extend(
        _check_speakers_analyze_pins(
            label="CPU leaf", deps=cpu_deps, root_version=root_version
        )
    )
    errors.extend(
        _check_speakers_analyze_pins(
            label="CUDA leaf", deps=cuda_deps, root_version=root_version
        )
    )

    # 5. CPU leaf runtime split.
    missing_cpu_runtime = sorted(CPU_ONNXRUNTIME_DEPS - set(cpu_deps))
    if missing_cpu_runtime:
        errors.append(f"CPU leaf missing CPU onnxruntime deps: {missing_cpu_runtime}")
    if "onnxruntime-gpu" in _names(cpu_deps):
        errors.append("CPU leaf must NOT pull onnxruntime-gpu")

    # 6. CUDA leaf runtime split.
    if CUDA_ONNXRUNTIME_DEP not in cuda_deps:
        errors.append(f"CUDA leaf must pull {CUDA_ONNXRUNTIME_DEP}")
    missing_nvidia = sorted(NVIDIA_CUDA_DEPS - set(cuda_deps))
    if missing_nvidia:
        errors.append(f"CUDA leaf missing NVIDIA CUDA deps: {missing_nvidia}")
    if "onnxruntime" in _names(cuda_deps):
        errors.append("CUDA leaf must NOT pull CPU onnxruntime")

    # 7. Leaves do not depend on each other.
    if "solstone-journal-cuda" in _names(cpu_deps):
        errors.append("CPU leaf must not depend on solstone-journal-cuda")
    if "solstone-journal" in _names(cuda_deps):
        errors.append("CUDA leaf must not depend on solstone-journal")

    # 11. uv workspace members/sources.
    workspace_members = root_uv.get("workspace", {}).get("members", [])
    if workspace_members != WORKSPACE_MEMBERS:
        errors.append(
            f"root [tool.uv.workspace].members must be exactly {WORKSPACE_MEMBERS}; "
            f"found {workspace_members}"
        )
    root_sources = root_uv.get("sources", {})
    for name in sorted(WORKSPACE_SOURCES):
        if root_sources.get(name) != {"workspace": True}:
            errors.append(f"root [tool.uv.sources].{name} must be {{workspace = true}}")
    if "solstone-journal-host" in root_sources:
        errors.append("root [tool.uv.sources] must not include solstone-journal-host")

    # 12. uv override prunes tombstone pins from workspace resolution.
    override_deps = root_uv.get("override-dependencies", [])
    if not any(dep.split(";", 1)[0].strip() == TOMBSTONE_PIN for dep in override_deps):
        errors.append(
            "[tool.uv].override-dependencies must contain "
            f"{TOMBSTONE_PIN!r} with any marker"
        )
    core_tombstone_pin = solstone_core_unsupported_platform_pin(root_version or "")
    if not any(
        dep.split(";", 1)[0].strip()
        == f"solstone-core-unsupported-platform=={root_version or ''}"
        for dep in override_deps
    ):
        errors.append(
            "[tool.uv].override-dependencies must contain "
            f"{core_tombstone_pin!r} with any marker"
        )

    # 13. Makefile no longer installs retired journal extras.
    try:
        makefile_text = makefile.read_text(encoding="utf-8")
    except FileNotFoundError:
        errors.append("missing Makefile")
    else:
        for spelling in ("--extra journal", "--extra journal-cuda"):
            if spelling in makefile_text:
                errors.append(f"Makefile must not contain {spelling!r}")

    if errors:
        print("ERROR: package-menu consistency check failed", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
