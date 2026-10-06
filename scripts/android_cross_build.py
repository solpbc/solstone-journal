#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Acquire pinned inputs and compile the Android workspace; no installation."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import time
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
TARGET = "aarch64-linux-android"
API = 31
NDK_VERSION = "30.0.16248370"
NDK = {
    "filename": "android-ndk-r30-linux.zip",
    "url": "https://dl.google.com/android/repository/android-ndk-r30-linux.zip",
    "sha256": "753611f410d002cfcd3f3dc2ef49aad532089d3180b436c060a90bf0fcb64df2",
}
ORT = {
    "filename": "onnxruntime-7a71bc575b189cdedea7fa2c0f87389f870bd10e.tar.gz",
    "url": "https://codeload.github.com/microsoft/onnxruntime/tar.gz/7a71bc575b189cdedea7fa2c0f87389f870bd10e",
    "sha256": "a29022e00b3c6a1596807fc5a1e8852c186a4cebcc6e883092a5865d7189a695",
}


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def acquire(pin, cache):
    archive = cache / pin["filename"]
    if archive.is_file():
        if sha256(archive) != pin["sha256"]:
            raise ValueError(f"input digest mismatch: {archive}")
        return archive
    partial = archive.with_suffix(archive.suffix + ".part")
    print(f"acquire {pin['url']}", flush=True)
    with urllib.request.urlopen(pin["url"], timeout=120) as response, partial.open("wb") as output:
        shutil.copyfileobj(response, output)
    if sha256(partial) != pin["sha256"]:
        raise ValueError(f"download digest mismatch: {partial}")
    partial.replace(archive)
    return archive


def unpack_source(archive, destination):
    if destination.exists():
        return
    staging = destination.with_name(destination.name + ".extracting")
    if staging.exists():
        shutil.rmtree(staging)
    staging.mkdir()
    with tarfile.open(archive) as bundle:
        for member in bundle.getmembers():
            if "/" not in member.name:
                continue
            member.name = member.name.split("/", 1)[1]
            if member.name:
                bundle.extract(member, staging, filter="data")
    # Archive mtimes should not make a newly acquired builder look abandoned.
    for entry in staging.rglob("*"):
        if not entry.is_symlink():
            os.utime(entry, None)
    staging.replace(destination)


class Builder:
    def __init__(self, work, env):
        self.work = work
        self.env = env
        self.steps = []
        self.receipts = work / "receipts"
        self.receipts.mkdir(exist_ok=True)

    def run(self, name, argv, cwd=ROOT):
        log = self.receipts / f"{name}.log"
        print(f"{name}: {' '.join(map(str, argv))}", flush=True)
        started = time.time()
        with log.open("wb") as output:
            result = subprocess.run(list(map(str, argv)), cwd=cwd, env=self.env,
                                    stdout=output, stderr=subprocess.STDOUT)
        receipt = {"name": name, "argv": list(map(str, argv)), "cwd": str(cwd),
                   "started_epoch": started, "seconds": round(time.time() - started, 3),
                   "exit": result.returncode, "log": str(log), "log_sha256": sha256(log)}
        self.steps.append(receipt)
        (self.receipts / f"{name}.json").write_text(json.dumps(receipt, indent=2) + "\n")
        if result.returncode:
            print(log.read_text(errors="replace")[-10000:], file=sys.stderr)
            raise subprocess.CalledProcessError(result.returncode, argv)
        return log


def build(args):
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise ValueError("Android cross-build requires a Linux x86_64 builder")
    work = args.work_dir.resolve()
    work.mkdir(parents=True, exist_ok=True)
    cache = args.cache_dir.resolve() if args.cache_dir else work / "downloads"
    cache.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    # Keep target inputs local to this builder, independent of desktop CI state.
    env.update(TMPDIR=str(work), CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS=str(args.jobs))
    runner = Builder(work, env)
    pins = tomllib.loads((ROOT / "core/distribution/builder-inputs.toml").read_text())
    inputs = {"ndk": NDK, "onnxruntime": ORT, "ffmpeg": pins["ffmpeg"]}
    source_commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    status = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    summary = {"target": TARGET, "android_api": API, "ndk_version": NDK_VERSION,
               "source_commit": source_commit, "source_clean": not status,
               "driver_sha256": sha256(Path(__file__)), "python_version": sys.version,
               "scope": "workspace default features (unified), libraries/binaries, plus solstone-core/journal-mcp-endpoint",
               "inputs": inputs, "steps": runner.steps}
    if args.generate_bindings:
        summary["scope"] = "FFmpeg binding regeneration with workspace default-feature libraries/binaries; MCP not run"
    try:
        archives = {name: acquire(pin, cache) for name, pin in inputs.items()}
        ndk = work / "android-ndk-r30"
        if not ndk.exists():
            ndk_staging = work / "ndk-extracting"
            if ndk_staging.exists():
                shutil.rmtree(ndk_staging)
            ndk_staging.mkdir()
            runner.run("unpack-ndk", ["unzip", "-q", "-DD", archives["ndk"], "-d", ndk_staging])
            (ndk_staging / "android-ndk-r30").replace(ndk)
            ndk_staging.rmdir()
        properties = (ndk / "source.properties").read_text()
        if f"Pkg.Revision = {NDK_VERSION}" not in properties:
            raise ValueError(f"incorrect NDK revision in {ndk}")
        toolchain = ndk / "toolchains/llvm/prebuilt/linux-x86_64"
        compiler = toolchain / "bin" / f"{TARGET}{API}-clang"
        runner.run("ndk-clang", [compiler, "--version"])
        runner.run("cmake-version", ["cmake", "--version"])
        runner.run("rust-version", ["rustc", "-vV"])
        runner.run("rust-target", ["rustup", "target", "add", TARGET])
        ort_source = work / "onnxruntime-7a71bc575b189cdedea7fa2c0f87389f870bd10e"
        ort_build = work / "onnxruntime-build-7a71bc575b189cdedea7fa2c0f87389f870bd10e"
        unpack_source(archives["onnxruntime"], ort_source)
        runner.run("onnx-configure", [
            "cmake", "-S", ort_source / "cmake", "-B", ort_build,
            f"-DCMAKE_TOOLCHAIN_FILE={ndk}/build/cmake/android.toolchain.cmake",
            "-DANDROID_ABI=arm64-v8a", f"-DANDROID_PLATFORM=android-{API}",
            "-DANDROID_STL=c++_static", "-DCMAKE_BUILD_TYPE=Release",
            "-Donnxruntime_BUILD_SHARED_LIB=ON", "-Donnxruntime_BUILD_UNIT_TESTS=OFF",
            "-Donnxruntime_ENABLE_PYTHON=OFF", "-Donnxruntime_USE_CUDA=OFF",
            "-Donnxruntime_USE_NNAPI_BUILTIN=OFF", "-Donnxruntime_DISABLE_CONTRIB_OPS=OFF",
            "-Donnxruntime_DISABLE_ML_OPS=ON", "-Donnxruntime_ENABLE_TRAINING=OFF",
            f"-DPython_EXECUTABLE={sys.executable}", "-DFETCHCONTENT_QUIET=OFF",
        ], cwd=ort_source)
        runner.run("onnx-build", ["cmake", "--build", ort_build, "--target", "onnxruntime", f"-j{args.jobs}"])
        native_lib = work / "lib"
        native_lib.mkdir(exist_ok=True)
        shutil.copy2(ort_build / "libonnxruntime.so", native_lib / "libonnxruntime.so")
        summary["onnxruntime_library_sha256"] = sha256(native_lib / "libonnxruntime.so")
        (work / "ffmpeg-source-archive.path").write_text(str(archives["ffmpeg"]) + "\n")
        (work / "ffmpeg-source-archive.sha256").write_text(inputs["ffmpeg"]["sha256"] + "\n")
        (work / "onnxruntime-library.sha256").write_text(summary["onnxruntime_library_sha256"] + "\n")
        if args.prepare_only:
            summary["exit"] = 0
            summary["scope"] = "pinned native input preparation only; no Rust build"
            return
        mode = "--generate-bindings" if args.generate_bindings else "build"
        runner.run("android-rust-build", ["/bin/bash", ROOT / "scripts/check_android_cross_build.sh", work, mode])
        metadata_log = runner.receipts / "workspace-metadata.log"
        metadata = json.loads(metadata_log.read_text())
        members = set(metadata["workspace_members"])
        summary["workspace_packages"] = sorted(p["name"] for p in metadata["packages"] if p["id"] in members)
        names = ["workspace-metadata", "android-workspace"] + ([] if args.generate_bindings else ["android-mcp"])
        summary["rust_steps"] = [{"name": name, "exit": int((runner.receipts / f"{name}.exit").read_text()),
                                  "log_sha256": sha256(runner.receipts / f"{name}.log"),
                                  "stderr_sha256": sha256(runner.receipts / f"{name}.stderr")} for name in names]
        bindings = ROOT / "core/vendor/ffmpeg-sys-next/bindings" / f"{TARGET}.rs"
        summary["ffmpeg_bindings_sha256"] = sha256(bindings)
        summary["build_environment"] = (runner.receipts / "rust-build-environment.txt").read_text().splitlines()
        summary["exit"] = 0
    except BaseException as error:
        summary.update(exit=getattr(error, "returncode", 1), error=str(error))
        raise
    finally:
        summary["steps"] = runner.steps
        (runner.receipts / "android-cross-build.json").write_text(json.dumps(summary, indent=2) + "\n")
        print(f"receipt: {runner.receipts / 'android-cross-build.json'}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True, help="isolated on-disk native input/build directory")
    parser.add_argument("--cache-dir", type=Path, help="optional verified archive cache")
    parser.add_argument("--jobs", type=int, default=6)
    parser.add_argument("--prepare-only", action="store_true", help="acquire toolchain and build native link inputs only")
    parser.add_argument("--generate-bindings", action="store_true", help="regenerate target bindings using NDK libclang")
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    try:
        build(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"android cross-build failed: {error}", file=sys.stderr)
        return getattr(error, "returncode", 1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
