#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Build restic-erase for every desktop target, reproducibly.

Inputs are pinned by SHA-256: the restic v0.19.0 source tarball (the same bytes
the iOS bridge and the Windows ARM64 build pin), the Go 1.26.4 toolchain for
the build host (restic 0.19.0's own release toolchain), and erase-paths.patch,
applied with `patch --fuzz=0`. Go modules are fetched through the module proxy
and checked against restic's own go.sum. Every binary is a static, cgo-free
cross build with no build id and trimmed paths, so two builds from the same
inputs produce the same bytes; the receipt records every input and output hash.

    build.py --source restic-0.19.0.tar.gz --go go1.26.4.linux-amd64.tar.gz --out OUT

The outputs are `restic-erase_<version>_<os>_<arch>[.exe]` and their `.bz2`
(POSIX) or `.zip` (Windows) archives, the form the catalog carries.
"""
import argparse
import bz2
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
# The catalog version of these builds. restic's own version is a constant and
# still reads 0.19.0; the file name says which program it is.
VERSION = "0.19.0-sol2"
SOURCE_SHA256 = "800779b6c4c2396971c0567b09ccdd435e03155e1a0ec94e8bbf3d98641a8bc2"
GO_SHA256 = {"go1.26.4.linux-amd64.tar.gz": "1153d3d50e0ac764b447adfe05c2bcf08e889d42a02e0fe0259bd47f6733ad7f"}
GO_SUM_SHA256 = "94840327111dc755890d04c23208d5b17f549846494d4225445cc70ba8ed0b2b"
TARGETS = [("linux", "amd64"), ("linux", "arm64"), ("darwin", "arm64"), ("windows", "amd64"), ("windows", "arm64")]
# The tags restic's own release builds use.
TAGS = "selfupdate,disable_grpc_modules"


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def pinned(path, want):
    got = sha256(path)
    if got != want:
        raise SystemExit(f"{path}: sha256 {got}, pinned {want}")


def run(args, **kwargs):
    subprocess.run(args, check=True, **kwargs)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--source", required=True, type=Path)
    ap.add_argument("--go", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    a = ap.parse_args()
    a.source, a.go, a.out = a.source.resolve(), a.go.resolve(), a.out.resolve()
    pinned(a.source, SOURCE_SHA256)
    pinned(a.go, GO_SHA256[a.go.name])
    patch = HERE / "erase-paths.patch"
    if a.out.exists():
        raise SystemExit(f"{a.out} exists: build into a fresh directory")
    a.out.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix="restic-erase-build-") as work:
        work = Path(work)
        run(["tar", "-xzf", str(a.source), "-C", str(work)])
        run(["tar", "-xzf", str(a.go), "-C", str(work)])
        src = work / "restic-0.19.0"
        pinned(src / "go.sum", GO_SUM_SHA256)
        run(["patch", "--batch", "--forward", "--fuzz=0", "-p1", "-i", str(patch)], cwd=src)
        go = work / "go" / "bin" / "go"
        env = {
            "PATH": f"{work / 'go' / 'bin'}:/usr/bin:/bin",
            "HOME": str(work / "home"),
            "GOPATH": str(work / "gopath"),
            "GOCACHE": str(work / "gocache"),
            "GOMODCACHE": str(work / "gopath" / "pkg" / "mod"),
            "GOTOOLCHAIN": "local",
            "GOFLAGS": "-mod=readonly",
            "GONOSUMDB": "",
            "CGO_ENABLED": "0",
        }
        run([str(go), "mod", "download"], cwd=src, env=env)
        run([str(go), "mod", "verify"], cwd=src, env=env)
        outputs = {}
        for goos, goarch in TARGETS:
            name = f"restic-erase_{VERSION}_{goos}_{goarch}" + (".exe" if goos == "windows" else "")
            binary = a.out / name
            run([str(go), "build", "-trimpath", f"-tags={TAGS}",
                 "-ldflags", "-s -w -buildid=",
                 "-o", str(binary), "./cmd/restic"],
                cwd=src, env={**env, "GOOS": goos, "GOARCH": goarch})
            if goos == "windows":
                archive = a.out / (name.removesuffix(".exe") + ".zip")
                info = zipfile.ZipInfo("restic-erase.exe", date_time=(1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o644 << 16
                with zipfile.ZipFile(archive, "w") as z:
                    z.writestr(info, binary.read_bytes())
            else:
                archive = a.out / (name + ".bz2")
                archive.write_bytes(bz2.compress(binary.read_bytes(), 9))
            outputs[f"{goos}-{goarch}"] = {
                "binary": name, "binary_sha256": sha256(binary), "binary_bytes": binary.stat().st_size,
                "archive": archive.name, "archive_sha256": sha256(archive), "archive_bytes": archive.stat().st_size,
            }
        go_version = subprocess.run([str(go), "version"], capture_output=True, text=True, env=env).stdout.strip()
    receipt = {
        "schema": "solstone.restic-erase-build.v1",
        "version": VERSION,
        "inputs": {"source": {"name": a.source.name, "sha256": SOURCE_SHA256},
                   "go": {"name": a.go.name, "sha256": GO_SHA256[a.go.name], "version": go_version},
                   "patch": {"name": patch.name, "sha256": sha256(patch)},
                   "go_sum_sha256": GO_SUM_SHA256, "tags": TAGS},
        "outputs": outputs,
    }
    (a.out / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()
