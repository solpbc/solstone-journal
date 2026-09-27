#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Generate and check target-specific Rust notices for journal releases.

The Windows source index records verified licence provenance for the lock's
external packages. This command selects each non-Windows binary closure from
Cargo metadata and carries only its licence texts into that target's payload.
Run --write when the lock or inventory changes; run --check before a release.
Neither operation belongs in routine make ci.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import tarfile
import tomllib
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / "core/distribution"
LOCK = ROOT / "core/Cargo.lock"
INDEX = DIST / "windows-rust-sources.json"
NOTICES = DIST / "windows-rust-NOTICES.txt"
TARGETS = {
    "linux-x86_64": "x86_64-unknown-linux-gnu",
    "linux-aarch64": "aarch64-unknown-linux-gnu",
    "macos-arm64": "aarch64-apple-darwin",
}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def closure(target: str, triple: str) -> tuple[list[str], dict[str, dict]]:
    inventory = tomllib.loads((DIST / "inventory.toml").read_text())
    roots = {
        entry["package"]
        for entry in inventory["entry"]
        if entry["kind"] == "bin" and target in entry.get("targets", [])
    }
    command = [
        "cargo", "metadata", "--manifest-path", str(ROOT / "core/Cargo.toml"),
        "--locked", "--offline", "--format-version", "1", "--filter-platform", triple,
    ]
    result = subprocess.run(command, capture_output=True, check=False)
    if result.returncode:
        raise RuntimeError(
            f"{target}: cargo metadata exited {result.returncode}: "
            f"{result.stderr.decode(errors='replace')}"
        )
    metadata = json.loads(result.stdout)
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    pending = [key for key, package in packages.items() if not package["source"] and package["name"] in roots]
    if len(pending) != len(roots):
        raise RuntimeError(f"{target}: missing inventory binary roots in Cargo metadata")
    seen: set[str] = set()
    while pending:
        key = pending.pop()
        if key in seen:
            continue
        seen.add(key)
        for dep in nodes[key]["deps"]:
            if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]):
                pending.append(dep["pkg"])
    external = {
        f"{packages[key]['name']}@{packages[key]['version']} ({packages[key]['source']})": packages[key]
        for key in seen
        if packages[key]["source"]
    }
    return sorted(external), external


def prior_texts(index: dict, notices: bytes) -> dict[str, bytes]:
    if digest(notices) != index["notices_sha256"]:
        raise RuntimeError("Windows source index does not match its notice file")
    result = {}
    for item in index["notice_texts"]:
        data = notices[item["byte_start_inclusive"]:item["byte_end_exclusive"]]
        if digest(data) != item["sha256"]:
            raise RuntimeError(f"Windows notice text digest mismatch: {item['sha256']}")
        result[item["sha256"]] = data
    return result


def registry_dir(package: dict) -> Path:
    name = f"{package['name']}-{package['version']}"
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    candidates = list((cargo_home / "registry/src").glob(f"*/{name}"))
    if len(candidates) != 1:
        raise RuntimeError(f"one cached registry source required for {name}, found {len(candidates)}")
    return candidates[0]


def new_registry_row(identity: str, package: dict) -> dict:
    if not package["source"].startswith("registry+"):
        raise RuntimeError(f"source provenance needs review for {identity}")
    source = registry_dir(package)
    files = sorted(
        path for path in source.iterdir()
        if path.is_file() and path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT"))
    )
    if not files:
        raise RuntimeError(f"no licence text found for {identity}")
    lock = tomllib.loads(LOCK.read_text())
    locked = [
        row for row in lock["package"]
        if row["name"] == package["name"] and row["version"] == package["version"]
        and row.get("source") == package["source"]
    ]
    if len(locked) != 1 or "checksum" not in locked[0]:
        raise RuntimeError(f"no unique checksummed lock row for {identity}")
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    archives = list((cargo_home / "registry/cache").glob(
        f"*/{package['name']}-{package['version']}.crate"
    ))
    if len(archives) != 1 or digest(archives[0].read_bytes()) != locked[0]["checksum"]:
        raise RuntimeError(f"cached crate archive does not match Cargo.lock for {identity}")
    url = f"https://static.crates.io/crates/{package['name']}/{package['name']}-{package['version']}.crate"
    with tarfile.open(archives[0]) as source_archive:
        for path in files:
            member = source_archive.extractfile(f"{package['name']}-{package['version']}/{path.name}")
            if member is None or member.read() != path.read_bytes():
                raise RuntimeError(f"cached source does not match crate archive for {identity}: {path.name}")
    references = [
        {
            "sha256": digest(path.read_bytes()),
            "bytes": path.stat().st_size,
            "source": {
                "kind": "cargo-registry-archive",
                "member": path.name,
                "source_url": url,
                "archive_sha256": locked[0]["checksum"],
            },
        }
        for path in files
    ]
    return {
        "identity": identity,
        "name": package["name"],
        "version": package["version"],
        "source": package["source"],
        "license_expression": package["license"],
        "notice_references": references,
    }


def source_text(reference: dict, package: dict) -> bytes:
    source = reference["source"]
    if source["kind"] == "cargo-registry-archive":
        member = source["member"]
        if Path(member).name != member:
            raise RuntimeError(f"invalid crate notice member for {package['name']}: {member}")
        data = (registry_dir(package) / member).read_bytes()
    else:
        url = source.get("source_url")
        if not isinstance(url, str) or not url.startswith("https://"):
            raise RuntimeError(f"no HTTPS source URL for {package['name']} notice")
        with urllib.request.urlopen(url, timeout=30) as response:
            data = response.read()
    if digest(data) != reference["sha256"]:
        raise RuntimeError(f"source text digest mismatch for {package['name']}: {source}")
    return data


def paths(target: str) -> tuple[Path, Path]:
    return (
        DIST / f"{target}-rust-NOTICES.txt",
        DIST / f"{target}-rust-sources.json",
    )


def write(target: str, triple: str, old_index: dict, known_texts: dict[str, bytes]) -> None:
    identities, metadata = closure(target, triple)
    source_rows = {row["identity"]: row for row in old_index["packages"]}
    selected = []
    texts = {}
    for identity in identities:
        package = metadata[identity]
        row = source_rows.get(identity) or new_registry_row(identity, package)
        if row.get("notice_status") not in (None, "texts-acquired"):
            raise RuntimeError(f"licence text needs review for {identity}")
        references = row["notice_references"]
        if not references:
            raise RuntimeError(f"no notice references for {identity}")
        selected.append({
            "identity": identity,
            "license_expression": row["license_expression"],
            "notice_references": references,
        })
        for reference in references:
            sha = reference["sha256"]
            texts[sha] = known_texts.get(sha) or source_text(reference, package)
            if digest(texts[sha]) != sha:
                raise RuntimeError(f"notice digest mismatch for {identity}")

    notices = bytearray(b"Rust dependency notices\n\n")
    text_index = []
    for sha, data in sorted(texts.items()):
        notices.extend(f"SHA-256: {sha}\n\n".encode())
        start = len(notices)
        notices.extend(data)
        end = len(notices)
        notices.extend(b"\n\n")
        text_index.append({"sha256": sha, "byte_start_inclusive": start, "byte_end_exclusive": end})
    source_index = {
        "schema": "solstone.rust-notices.v1",
        "target": target,
        "filter_platform": triple,
        "cargo_lock_sha256": digest(LOCK.read_bytes()),
        "notices_sha256": digest(notices),
        "packages": selected,
        "notice_texts": text_index,
    }
    notice_path, index_path = paths(target)
    notice_path.write_bytes(notices)
    index_path.write_text(json.dumps(source_index, indent=2, sort_keys=True) + "\n")
    print(f"{target}: wrote {len(selected)} packages, {len(text_index)} notice texts")


def check(target: str, triple: str) -> None:
    notice_path, index_path = paths(target)
    notices = notice_path.read_bytes()
    index = json.loads(index_path.read_bytes())
    identities, _ = closure(target, triple)
    if (
        index["schema"] != "solstone.rust-notices.v1"
        or index["target"] != target
        or index["filter_platform"] != triple
        or index["cargo_lock_sha256"] != digest(LOCK.read_bytes())
        or index["notices_sha256"] != digest(notices)
        or [package["identity"] for package in index["packages"]] != identities
    ):
        raise RuntimeError(f"{target}: Rust notices are missing or stale; regenerate with --write")
    texts = {}
    for item in index["notice_texts"]:
        data = notices[item["byte_start_inclusive"]:item["byte_end_exclusive"]]
        if digest(data) != item["sha256"]:
            raise RuntimeError(f"{target}: corrupted notice text {item['sha256']}")
        texts[item["sha256"]] = data
    for package in index["packages"]:
        if not package["notice_references"]:
            raise RuntimeError(f"{target}: no notices for {package['identity']}")
        for reference in package["notice_references"]:
            if reference["sha256"] not in texts:
                raise RuntimeError(f"{target}: missing text for {package['identity']}")
    print(f"{target}: PASS {len(identities)} packages, {len(texts)} notice texts")


def check_bundled_source_notices() -> None:
    pin = tomllib.loads((DIST / "builder-inputs.toml").read_text())["ffmpeg"]
    archive = Path(os.environ.get(
        "SOLSTONE_FFMPEG_SOURCE_ARCHIVE",
        ROOT / "core/target/ffmpeg-source-cache/ffmpeg.tar.gz",
    ))
    if digest(archive.read_bytes()) != pin["sha256"]:
        raise RuntimeError("pinned FFmpeg source archive digest mismatch")
    source_notice = (DIST / "ffmpeg-source-NOTICE.md").read_text()
    if pin["commit"] not in source_notice or pin["sha256"] not in source_notice or pin["url"] not in source_notice:
        raise RuntimeError("FFmpeg source notice does not identify the pinned source")
    prefix = f"FFmpeg-{pin['commit']}/"
    with tarfile.open(archive) as source:
        for name in ("COPYING.GPLv2", "COPYING.GPLv3", "COPYING.LGPLv2.1", "COPYING.LGPLv3", "LICENSE.md"):
            member = source.extractfile(prefix + name)
            if member is None or member.read() != (DIST / "licenses/ffmpeg-source" / name).read_bytes():
                raise RuntimeError(f"FFmpeg source licence text is missing or stale: {name}")
    rfdetr_licenses = []
    for suffix in ("linux-cpu-x64", "linux-cpu-arm64", "macos-metal-arm64"):
        name = f"rfdetr-v0.1.0-solpbc.5-bin-{suffix}"
        with tarfile.open(ROOT / "core/models/assets/rfdetr" / f"{name}.tar.gz") as source:
            member = source.extractfile(f"{name}/LICENSE")
            if member is None:
                raise RuntimeError(f"RF-DETR archive has no licence: {name}")
            rfdetr_licenses.append(member.read())
    if any(data != (DIST / "licenses/rfdetr/LICENSE").read_bytes() for data in rfdetr_licenses):
        raise RuntimeError("RF-DETR licence differs from a bundled engine archive")
    inventory = tomllib.loads((DIST / "inventory.toml").read_text())
    model_notice = (DIST / "model-notices.md").read_text()
    third_party = (ROOT / "THIRD_PARTY_NOTICES.md").read_text()
    for entry in inventory["entry"]:
        if entry["kind"] != "model-asset" or not any(target in TARGETS for target in entry["targets"]):
            continue
        source_path = ROOT / entry["source"]
        source_digest = digest(source_path.read_bytes())
        if source_digest not in third_party:
            raise RuntimeError(f"missing or stale third-party attribution for {entry['source']}")
        if source_path.suffix in (".onnx", ".gguf") and source_digest not in model_notice:
            raise RuntimeError(f"missing or stale model attribution for {entry['source']}")
    print("bundled FFmpeg and RF-DETR licence texts: PASS")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--write", action="store_true")
    action.add_argument("--check", action="store_true")
    parser.add_argument("--target", choices=TARGETS, action="append")
    args = parser.parse_args()
    targets = args.target or list(TARGETS)
    if args.write:
        old_index = json.loads(INDEX.read_bytes())
        known_texts = prior_texts(old_index, NOTICES.read_bytes())
        for target in targets:
            write(target, TARGETS[target], old_index, known_texts)
    else:
        check_bundled_source_notices()
        for target in targets:
            check(target, TARGETS[target])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
