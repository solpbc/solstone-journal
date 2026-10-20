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

from refresh_windows_rust_notices import shipped_feature_arguments

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
SCREEN_OCR_MANIFEST = ROOT / "core/screen-ocr-helper/Cargo.toml"
SCREEN_OCR_LOCK = ROOT / "core/screen-ocr-helper/Cargo.lock"
SCREEN_OCR_RECIPE = ROOT / "core/screen-ocr-helper/recipe.toml"
SCREEN_OCR_SOURCE_DIRECTIONS = ROOT / "core/screen-ocr-helper/SOURCE-DIRECTIONS.md"
SCREEN_OCR_NOTICE = DIST / "screen-ocr-rust-NOTICES.txt"
SCREEN_OCR_INDEX = DIST / "screen-ocr-rust-sources.json"
SCREEN_OCR_TARGETS = {
    "linux-x86_64": "x86_64-unknown-linux-gnu",
    "linux-aarch64": "aarch64-unknown-linux-gnu",
    "macos-arm64": "aarch64-apple-darwin",
    "windows-x86_64": "x86_64-pc-windows-msvc",
}
OAR_URL = "https://github.com/GreatV/oar-ocr"
OAR_REVISION = "84817f58d7a0932d5368a9fa18105ba5d842af29"
ORT_SOURCE_URL = "https://github.com/microsoft/onnxruntime"
ORT_SOURCE_REVISION = "402979ac1bef89dcd55add41eaa7277c407599eb"
EIGEN_SOURCE_URL = "https://gitlab.com/libeigen/eigen"
EIGEN_SOURCE_REVISION = "1d8b82b0740839c0de7f1242a3585e3390ff5f33"


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


VELOPACK_IDENTITY = "velopack@1.2.0 (path+core/vendor/velopack)"
VELOPACK_PUBLISHED_CRATE_SHA256 = "bd3fd8e7b771e0d0fc4ca67249273ad4eacfabe2562617b3ca42065a5972a58c"
VELOPACK_DONOR_CRATE_SHA256 = "cbfbd9ee9818d1155813c4a1024a459763c3782405d5c1ad8f116bd8adaf0b88"
VELOPACK_DONOR_VERSION = "1.3.39-g8d2844f"
VELOPACK_LICENSE_URL = (
    "https://raw.githubusercontent.com/velopack/velopack/"
    "f2edcbcafb81da5b3c884aaea330e225ad91d8b6/LICENSE"
)
VELOPACK_LICENSE_COMMIT = "f2edcbcafb81da5b3c884aaea330e225ad91d8b6"
VELOPACK_LICENSE_SHA256 = "91845db83551c877ebbb1118e0fb92e4e527290d23b995c55dcd438b3293943f"
VELOPACK_LICENSE_LENGTH = 1091
VELOPACK_NOT_THE_PUBLISHED_CRATE = (
    "This tree is not the published crates.io crate velopack-1.2.0."
)


def patched_velopack_tree_sha256(vendor: Path) -> str:
    files: list[tuple[str, bytes]] = []

    def visit(current: Path) -> None:
        for path in sorted(current.iterdir(), key=lambda item: item.name):
            if path.is_dir():
                visit(path)
                continue
            if not path.is_file():
                continue
            relative = path.relative_to(vendor).as_posix()
            if relative == "ORIGIN.toml":
                continue
            files.append((relative, path.read_bytes()))

    visit(vendor)
    files.sort(key=lambda item: item[0])
    hasher = hashlib.sha256()
    for relative, data in files:
        hasher.update(relative.encode())
        hasher.update(b"\x00")
        hasher.update(len(data).to_bytes(8, "little"))
        hasher.update(data)
    return hasher.hexdigest()


def prove_velopack_provenance() -> bytes:
    vendor = ROOT / "core/vendor/velopack"
    manifest = tomllib.loads((vendor / "Cargo.toml").read_text())
    package = manifest.get("package") or {}
    if package.get("name") != "velopack" or package.get("version") != "1.2.0":
        raise RuntimeError(
            "vendored velopack Cargo.toml must be name velopack version 1.2.0"
        )
    origin = tomllib.loads((vendor / "ORIGIN.toml").read_text())
    expected_tree = patched_velopack_tree_sha256(vendor)
    expected = {
        "published_crate_sha256": VELOPACK_PUBLISHED_CRATE_SHA256,
        "donor_crate_sha256": VELOPACK_DONOR_CRATE_SHA256,
        "donor_version": VELOPACK_DONOR_VERSION,
        "license_url": VELOPACK_LICENSE_URL,
        "license_commit": VELOPACK_LICENSE_COMMIT,
        "license_sha256": VELOPACK_LICENSE_SHA256,
        "license_length": VELOPACK_LICENSE_LENGTH,
        "patched_tree_sha256": expected_tree,
        "not_the_published_crate": VELOPACK_NOT_THE_PUBLISHED_CRATE,
    }
    for key, value in expected.items():
        if origin.get(key) != value:
            raise RuntimeError(f"velopack ORIGIN.toml {key} mismatch: {origin.get(key)!r}")
    vendor_license = (vendor / "LICENSE").read_bytes()
    if (
        len(vendor_license) != VELOPACK_LICENSE_LENGTH
        or digest(vendor_license) != VELOPACK_LICENSE_SHA256
    ):
        raise RuntimeError("vendored velopack LICENSE digest or length mismatch")
    shipped = (DIST / "licenses/velopack/LICENSE").read_bytes()
    if shipped != vendor_license:
        raise RuntimeError(
            "core/distribution/licenses/velopack/LICENSE is not byte-identical "
            "to the vendor LICENSE"
        )
    return shipped


def velopack_notice_row(license_bytes: bytes) -> dict:
    return {
        "identity": VELOPACK_IDENTITY,
        "name": "velopack",
        "version": "1.2.0",
        "source": "path+core/vendor/velopack",
        "patched_tree_sha256": patched_velopack_tree_sha256(ROOT / "core/vendor/velopack"),
        "license_expression": "MIT",
        "notice_references": [
            {
                "sha256": digest(license_bytes),
                "bytes": len(license_bytes),
                "source": {
                    "kind": "shipped-license",
                    "member": "LICENSE",
                    "path": "core/distribution/licenses/velopack/LICENSE",
                },
            }
        ],
    }


def owned_manifests() -> set[Path]:
    workspace = tomllib.loads((ROOT / "core/Cargo.toml").read_text())["workspace"]
    return {
        (ROOT / "core" / member / "Cargo.toml").resolve()
        for member in [*workspace.get("members", []), *[
            item for item in workspace.get("exclude", [])
            if item == "crates/solstone-core-service-legacy-evidence"
        ]]
    }


def path_bindings(identities: list[str]) -> dict[str, str]:
    if VELOPACK_IDENTITY not in identities:
        return {}
    prove_velopack_provenance()
    return {VELOPACK_IDENTITY: patched_velopack_tree_sha256(ROOT / "core/vendor/velopack")}


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
        *shipped_feature_arguments(ROOT),
    ]
    result = subprocess.run(command, capture_output=True, check=False, timeout=300)
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
    external = {}
    owned = owned_manifests()
    for key in seen:
        package = packages[key]
        if package["source"]:
            identity = f"{package['name']}@{package['version']} ({package['source']})"
            external[identity] = package
            continue
        if package["name"] == "velopack":
            if package["version"] != "1.2.0":
                raise RuntimeError(
                    f"path-patched velopack must be 1.2.0, got {package['version']}"
                )
            prove_velopack_provenance()
            if Path(package["manifest_path"]).resolve() != (ROOT / "core/vendor/velopack/Cargo.toml").resolve():
                raise RuntimeError("velopack does not resolve to the reviewed vendor path")
            external[VELOPACK_IDENTITY] = package
            continue
        manifest = Path(package["manifest_path"]).resolve()
        if manifest in owned:
            continue
        if package["name"] == "ffmpeg-sys-next" and manifest == (ROOT / "core/vendor/ffmpeg-sys-next/Cargo.toml").resolve():
            continue
        raise RuntimeError(f"unaccounted no-source dependency: {package['name']}@{package['version']}")
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
    registry_src = (cargo_home / "registry/src").resolve()
    manifest = Path(package.get("manifest_path", "")).resolve()
    source = manifest.parent
    try:
        source.relative_to(registry_src)
    except ValueError as error:
        raise RuntimeError(f"Cargo metadata source is outside the registry cache for {name}") from error
    if manifest.name != "Cargo.toml" or source.name != name or not source.is_dir():
        raise RuntimeError(f"Cargo metadata source does not identify one cached registry package: {name}")
    return source


def new_registry_row(
    identity: str,
    package: dict,
    lock_path: Path = LOCK,
    *,
    allow_source_declaration: bool = False,
) -> dict:
    if not package["source"].startswith("registry+"):
        raise RuntimeError(f"source provenance needs review for {identity}")
    source = registry_dir(package)
    files = sorted(
        path for path in source.iterdir()
        if path.is_file() and path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT"))
    )
    license_material = "license-text"
    if not files:
        manifest = source / "Cargo.toml"
        readme = source / "README.md"
        if (not allow_source_declaration or not package.get("license")
                or not manifest.is_file() or not readme.is_file()):
            raise RuntimeError(f"no licence text found for {identity}")
        files = [manifest, readme]
        license_material = "source-declaration-only"
    lock = tomllib.loads(lock_path.read_text())
    locked = [
        row for row in lock["package"]
        if row["name"] == package["name"] and row["version"] == package["version"]
        and row.get("source") == package["source"]
    ]
    if len(locked) != 1 or "checksum" not in locked[0]:
        raise RuntimeError(f"no unique checksummed lock row for {identity}")
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    archives = sorted((cargo_home / "registry/cache").glob(
        f"*/{package['name']}-{package['version']}.crate"
    ))
    matching_archives = [
        archive for archive in archives
        if digest(archive.read_bytes()) == locked[0]["checksum"]
    ]
    if not matching_archives:
        raise RuntimeError(f"cached crate archive does not match Cargo.lock for {identity}")
    archive = matching_archives[0]
    url = f"https://static.crates.io/crates/{package['name']}/{package['name']}-{package['version']}.crate"
    with tarfile.open(archive) as source_archive:
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
        "license_material": license_material,
        "notice_references": references,
    }


def source_text(reference: dict, package: dict) -> bytes:
    source = reference["source"]
    if source["kind"] == "shipped-license":
        path = ROOT / source["path"]
        if path != DIST / "licenses/velopack/LICENSE":
            raise RuntimeError(f"invalid shipped licence path for {package['name']}: {path}")
        data = path.read_bytes()
    elif source["kind"] == "cargo-registry-archive":
        member = source["member"]
        if Path(member).name != member:
            raise RuntimeError(f"invalid crate notice member for {package['name']}: {member}")
        data = (registry_dir(package) / member).read_bytes()
    elif source["kind"] == "cargo-git-checkout":
        if source.get("git") != OAR_URL or source.get("revision") != OAR_REVISION:
            raise RuntimeError(f"unreviewed git source for {package['name']}")
        checkout = git_checkout(package, OAR_REVISION)
        member = source["member"]
        if Path(member).name != member:
            raise RuntimeError(f"invalid git notice member for {package['name']}: {member}")
        data = (checkout / member).read_bytes()
    else:
        url = source.get("source_url")
        if not isinstance(url, str) or not url.startswith("https://"):
            raise RuntimeError(f"no HTTPS source URL for {package['name']} notice")
        with urllib.request.urlopen(url, timeout=30) as response:
            data = response.read()
    if digest(data) != reference["sha256"]:
        raise RuntimeError(f"source text digest mismatch for {package['name']}: {source}")
    return data


def git_checkout(package: dict, revision: str) -> Path:
    manifest = Path(package["manifest_path"]).resolve()
    result = subprocess.run(
        ["git", "-C", str(manifest.parent), "rev-parse", "--show-toplevel"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    if result.returncode:
        raise RuntimeError(f"cannot verify git source root for {package['name']}")
    root = Path(result.stdout.decode().strip()).resolve()
    head = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "HEAD"],
        capture_output=True,
        check=False,
        timeout=30,
    )
    if head.returncode or head.stdout.decode().strip() != revision:
        raise RuntimeError(f"git source revision mismatch for {package['name']}")
    return root


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
        if identity == VELOPACK_IDENTITY:
            row = velopack_notice_row(prove_velopack_provenance())
        else:
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
        "shipped_feature_arguments": shipped_feature_arguments(ROOT),
        "cargo_lock_sha256": digest(LOCK.read_bytes()),
        "notices_sha256": digest(notices),
        "path_dependency_trees": path_bindings(identities),
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
        or index.get("shipped_feature_arguments") != shipped_feature_arguments(ROOT)
        or index["cargo_lock_sha256"] != digest(LOCK.read_bytes())
        or index["notices_sha256"] != digest(notices)
        or index.get("path_dependency_trees", {}) != path_bindings(identities)
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


def screen_ocr_git_source() -> str:
    return f"git+{OAR_URL}?rev={OAR_REVISION}#{OAR_REVISION}"


def verify_screen_ocr_inputs() -> tuple[dict, dict[str, dict]]:
    manifest = tomllib.loads(SCREEN_OCR_MANIFEST.read_text())
    package = manifest.get("package") or {}
    if package.get("name") != "solstone-screen-ocr-helper" or package.get("license") != "AGPL-3.0-only":
        raise RuntimeError("screen OCR helper manifest identity or product licence mismatch")
    recipe = tomllib.loads(SCREEN_OCR_RECIPE.read_text())
    source = recipe.get("source") or {}
    if (
        source.get("oar_git") != OAR_URL
        or source.get("oar_revision") != OAR_REVISION
        or source.get("oar_default_features") is not False
    ):
        raise RuntimeError("screen OCR recipe does not bind the pinned OAR source")
    lock = tomllib.loads(SCREEN_OCR_LOCK.read_text())
    git_rows = [row for row in lock["package"] if row.get("source", "").startswith("git+")]
    if not git_rows or any(row["source"] != screen_ocr_git_source() for row in git_rows):
        raise RuntimeError("screen OCR Cargo.lock has an unpinned git source")
    directions = SCREEN_OCR_SOURCE_DIRECTIONS.read_text()
    for value in (
        ORT_SOURCE_URL,
        ORT_SOURCE_REVISION,
        EIGEN_SOURCE_URL,
        EIGEN_SOURCE_REVISION,
    ):
        if value not in directions:
            raise RuntimeError(f"screen OCR source directions omit pinned source: {value}")
    return recipe, {row["name"] + "@" + row["version"]: row for row in lock["package"]}


def screen_ocr_closure(target: str, triple: str) -> tuple[list[str], dict[str, dict]]:
    command = [
        "cargo", "metadata", "--manifest-path", str(SCREEN_OCR_MANIFEST),
        "--locked", "--offline", "--format-version", "1", "--filter-platform", triple,
    ]
    result = subprocess.run(command, capture_output=True, check=False, timeout=300)
    if result.returncode:
        raise RuntimeError(
            f"screen OCR {target}: cargo metadata exited {result.returncode}: "
            f"{result.stderr.decode(errors='replace')}"
        )
    metadata = json.loads(result.stdout)
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    manifest = SCREEN_OCR_MANIFEST.resolve()
    roots = [
        key for key, package in packages.items()
        if not package["source"] and Path(package["manifest_path"]).resolve() == manifest
    ]
    if len(roots) != 1:
        raise RuntimeError(f"screen OCR {target}: independent helper root missing or duplicated")
    pending = [roots[0]]
    seen: set[str] = set()
    while pending:
        key = pending.pop()
        if key in seen:
            continue
        seen.add(key)
        for dependency in nodes[key]["deps"]:
            if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"]):
                pending.append(dependency["pkg"])

    identities: dict[str, dict] = {}
    lock_rows = tomllib.loads(SCREEN_OCR_LOCK.read_text())["package"]
    for key in seen:
        package = packages[key]
        if not package["source"]:
            if key != roots[0]:
                raise RuntimeError(f"screen OCR {target}: unowned path dependency {package['name']}")
            continue
        source = package["source"]
        if source.startswith("git+") and source != screen_ocr_git_source():
            raise RuntimeError(f"screen OCR {target}: unpinned git dependency {package['name']}")
        if not (source.startswith("registry+") or source == screen_ocr_git_source()):
            raise RuntimeError(f"screen OCR {target}: unsupported dependency source {source}")
        if not isinstance(package.get("license"), str) or not package["license"].strip():
            raise RuntimeError(f"screen OCR {target}: missing license expression for {package['name']}")
        locked = [
            row for row in lock_rows
            if row["name"] == package["name"]
            and row["version"] == package["version"]
            and row.get("source") == source
        ]
        if len(locked) != 1:
            raise RuntimeError(f"screen OCR {target}: dependency does not have one lock row: {package['name']}")
        identity = f"{package['name']}@{package['version']} ({source})"
        identities[identity] = package
    return sorted(identities), identities


def screen_ocr_git_row(identity: str, package: dict) -> dict:
    if package["source"] != screen_ocr_git_source():
        raise RuntimeError(f"screen OCR source provenance needs review for {identity}")
    checkout = git_checkout(package, OAR_REVISION)
    files = sorted(
        path for path in checkout.iterdir()
        if path.is_file() and path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT"))
    )
    if not files:
        raise RuntimeError(f"no OAR licence text found for {identity}")
    references = [
        {
            "sha256": digest(path.read_bytes()),
            "bytes": path.stat().st_size,
            "source": {
                "kind": "cargo-git-checkout",
                "member": path.name,
                "git": OAR_URL,
                "revision": OAR_REVISION,
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


def screen_ocr_source_row(identity: str, package: dict) -> dict:
    if package["source"] == screen_ocr_git_source():
        return screen_ocr_git_row(identity, package)
    return new_registry_row(
        identity,
        package,
        SCREEN_OCR_LOCK,
        allow_source_declaration=True,
    )


def screen_ocr_notice_payload() -> tuple[bytes, bytes]:
    recipe, _ = verify_screen_ocr_inputs()
    target_closures = []
    packages: dict[str, dict] = {}
    for target, triple in SCREEN_OCR_TARGETS.items():
        identities, metadata = screen_ocr_closure(target, triple)
        target_closures.append({"target": target, "filter_platform": triple, "packages": identities})
        packages.update(metadata)

    selected = []
    texts: dict[str, bytes] = {}
    for identity in sorted(packages):
        package = packages[identity]
        row = screen_ocr_source_row(identity, package)
        references = row["notice_references"]
        if not references:
            raise RuntimeError(f"no notice references for {identity}")
        selected.append({
            "identity": identity,
            "license_expression": row["license_expression"],
            "license_material": row.get("license_material", "license-text"),
            "notice_references": references,
        })
        for reference in references:
            sha = reference["sha256"]
            texts[sha] = source_text(reference, package)
            if digest(texts[sha]) != sha:
                raise RuntimeError(f"screen OCR notice digest mismatch for {identity}")

    notices = bytearray(b"Screen OCR helper Rust dependency notices\n\n")
    text_index = []
    for sha, data in sorted(texts.items()):
        notices.extend(f"SHA-256: {sha}\n\n".encode())
        start = len(notices)
        notices.extend(data)
        end = len(notices)
        notices.extend(b"\n\n")
        text_index.append({"sha256": sha, "byte_start_inclusive": start, "byte_end_exclusive": end})
    lock_bytes = SCREEN_OCR_LOCK.read_bytes()
    manifest_bytes = SCREEN_OCR_MANIFEST.read_bytes()
    recipe_bytes = SCREEN_OCR_RECIPE.read_bytes()
    directions_bytes = SCREEN_OCR_SOURCE_DIRECTIONS.read_bytes()
    source_index = {
        "schema": "solstone.screen-ocr-rust-notices.v1",
        "helper_manifest_sha256": digest(manifest_bytes),
        "cargo_lock_sha256": digest(lock_bytes),
        "recipe_sha256": digest(recipe_bytes),
        "source_directions_sha256": digest(directions_bytes),
        "oar_git": OAR_URL,
        "oar_revision": recipe["source"]["oar_revision"],
        "native_source_directions": {
            "ort_git": ORT_SOURCE_URL,
            "ort_revision": ORT_SOURCE_REVISION,
            "eigen_git": EIGEN_SOURCE_URL,
            "eigen_revision": EIGEN_SOURCE_REVISION,
        },
        "target_closures": target_closures,
        "notices_sha256": digest(notices),
        "packages": selected,
        "notice_texts": text_index,
    }
    index_bytes = json.dumps(source_index, indent=2, sort_keys=True).encode() + b"\n"
    return bytes(notices), index_bytes


def write_screen_ocr_notices() -> None:
    notices, index = screen_ocr_notice_payload()
    SCREEN_OCR_NOTICE.write_bytes(notices)
    SCREEN_OCR_INDEX.write_bytes(index)
    print(f"screen OCR: wrote {len(json.loads(index)['packages'])} packages")


def check_screen_ocr_notices() -> None:
    expected_notices, expected_index = screen_ocr_notice_payload()
    if SCREEN_OCR_NOTICE.read_bytes() != expected_notices or SCREEN_OCR_INDEX.read_bytes() != expected_index:
        raise RuntimeError("screen OCR Rust notices are missing or stale; regenerate with --write --screen-ocr")
    print(f"screen OCR: PASS {len(json.loads(expected_index)['packages'])} packages")


def check_bundled_source_notices() -> None:
    pin = tomllib.loads((DIST / "builder-inputs.toml").read_text())["ffmpeg"]
    archive = Path(os.environ.get(
        "SOLSTONE_FFMPEG_SOURCE_ARCHIVE",
        ROOT / "target/ffmpeg-source-cache/ffmpeg.tar.gz",
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
    parser.add_argument("--screen-ocr", action="store_true")
    args = parser.parse_args()
    if args.screen_ocr:
        if args.target:
            parser.error("--target applies only to the main workspace notice profile")
        if args.write:
            write_screen_ocr_notices()
        else:
            check_screen_ocr_notices()
        return 0
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
