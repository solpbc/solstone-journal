#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Build, sign, notarize, and record every declared macOS native package."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import zipfile
from pathlib import Path

from scripts.record_macos_native_wheel import write_macos_native_record
from scripts.release_package_inventory import (
    NativePackage,
    load_release_package_inventory,
    macos_native_record_name,
    native_role,
    normalized_distribution,
)
from scripts.repack_wheel_record import repack
from scripts.stage_speakers_analyze_runtime import (
    DEFAULT_LINK_ROOT as SPEAKERS_LINK_ROOT,
)
from scripts.stage_speakers_analyze_runtime import (
    TARGETS as SPEAKERS_TARGETS,
)
from solstone.think.probe import (
    SOLSTONE_CORE_PLATFORM_TAGS,
    SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS,
)

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / "dist"
MACOS_PLATFORM = ("darwin", "arm64")
MACOS_TARGET = "aarch64-apple-darwin"


def _run(argv: list[str], *, env: dict[str, str] | None = None) -> str:
    result = subprocess.run(
        argv,
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no output"
        raise SystemExit(f"{' '.join(argv)} failed ({result.returncode}): {detail}")
    return result.stdout


def _wheel_path(package: NativePackage) -> Path:
    tag = (
        SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS[MACOS_PLATFORM]
        if package.target_family == "speakers-analyze"
        else SOLSTONE_CORE_PLATFORM_TAGS[MACOS_PLATFORM]
    )
    return DIST / (
        f"{normalized_distribution(package.distribution)}-{package.version}"
        f"-py3-none-{tag}.whl"
    )


def _build_env(package: NativePackage) -> dict[str, str]:
    env = os.environ.copy()
    env.update(
        {
            "MACOSX_DEPLOYMENT_TARGET": "14.0",
            "MATURIN_PEP517_ARGS": f"--locked --target {MACOS_TARGET}",
            "PYTHONNOUSERSITE": "1",
        }
    )
    if package.target_family == "speakers-analyze":
        env["ORT_LIB_PATH"] = str(
            (SPEAKERS_LINK_ROOT / SPEAKERS_TARGETS["macos-arm64"].key).resolve()
        )
        env["ORT_PREFER_DYNAMIC_LINK"] = "true"
    return env


def _sign_member(path: Path) -> dict[str, object]:
    stdout = _run([str(ROOT / "scripts" / "sign-and-notarize-helper.sh"), str(path)])
    try:
        payload = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise SystemExit(f"signing facts for {path.name} are invalid JSON: {exc}")
    if not isinstance(payload, dict):
        raise SystemExit(f"signing facts for {path.name} are not an object")
    return payload


def _members_to_sign(package: NativePackage, unpacked: Path) -> dict[str, Path]:
    binary_matches = list(unpacked.glob(f"*.data/scripts/{package.binary}"))
    if len(binary_matches) != 1:
        raise SystemExit(
            f"{package.distribution}: expected one {package.binary} wheel member, "
            f"found {len(binary_matches)}"
        )
    members = {package.binary: binary_matches[0]}
    if package.target_family == "speakers-analyze":
        dylib_name = SPEAKERS_TARGETS["macos-arm64"].runtime_staged_name
        dylib_matches = list(unpacked.glob(f"*.data/data/lib/**/{dylib_name}"))
        if len(dylib_matches) != 1:
            raise SystemExit(
                f"{package.distribution}: expected one {dylib_name} wheel member, "
                f"found {len(dylib_matches)}"
            )
        members[dylib_name] = dylib_matches[0]
    return members


def _source_commit() -> str:
    return _run(["git", "rev-parse", "HEAD"]).strip()


def main() -> int:
    inventory = load_release_package_inventory(ROOT)
    source_commit = _source_commit()
    core_lock_sha256 = hashlib.sha256(
        (ROOT / "core" / "Cargo.lock").read_bytes()
    ).hexdigest()
    speaker_stage = ROOT / "packages" / "solstone-core-speakers-analyze" / "wheel-data"
    DIST.mkdir(parents=True, exist_ok=True)
    try:
        for package in inventory.macos_native_packages:
            role = native_role(package)
            if package.target_family == "speakers-analyze":
                _run(
                    [
                        "python3",
                        "scripts/stage_speakers_analyze_runtime.py",
                        "--target",
                        "macos-arm64",
                    ]
                )
            print(f"==> building {package.distribution} for macOS/arm64")
            _run(
                ["uv", "build", "--package", package.distribution, "--wheel"],
                env=_build_env(package),
            )
            wheel_path = _wheel_path(package)
            if not wheel_path.is_file():
                raise SystemExit(f"expected wheel was not built: {wheel_path.name}")
            with tempfile.TemporaryDirectory(prefix="solstone-macos-wheel-") as raw:
                unpacked = Path(raw)
                with zipfile.ZipFile(wheel_path) as wheel:
                    wheel.extractall(unpacked)
                facts = {
                    name: _sign_member(member)
                    for name, member in _members_to_sign(package, unpacked).items()
                }
                repack(unpacked, wheel_path)
                facts_path = unpacked / "signing-facts.json"
                facts_path.write_text(
                    json.dumps({"members": facts}, sort_keys=True) + "\n",
                    encoding="utf-8",
                )
                write_macos_native_record(
                    role=role,
                    wheel_path=wheel_path,
                    signing_facts_path=facts_path,
                    output_path=DIST / macos_native_record_name(role),
                    source_commit=source_commit,
                    core_lock_sha256=core_lock_sha256,
                )
            print(f"built and recorded: {wheel_path.name}")
    finally:
        shutil.rmtree(speaker_stage, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
