#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Repack selected files from a tagged llama.cpp CUDA 13 OCI image.

The image digest is authoritative: tag inputs are resolved to raw manifest
bytes, cross-checked against Docker-Content-Digest when present, and the
resolved tag, index digest, and per-arch manifest digest are recorded.

Each selected file is resolved under the extracted rootfs, hashed before it is
staged, copied byte-for-byte, and inventoried with its rootfs-relative resolved
source path. NVIDIA object code is never stripped, patched, relinked, or
otherwise transformed.

Use the server-cuda13 family only. The plain server-cuda family is CUDA 12.8,
which is the wrong runtime family for the CUDA 13.3 packages, arch set, and
driver gate.

Accepted gap: this does not compute the full DT_NEEDED closure. A new upstream
runtime dependency outside the wanted-file directories surfaces at 1d's live
Spark e2e, not here.

Third-party verification is per-file, not archive-level. Archive-level
byte-identity across different zlib builds is not promised.

Imports of _-prefixed helpers from oci_image.py are deliberate: OCI layer,
whiteout, traversal, blob, and digest semantics are single-sourced there.

CLI convention: the pin snippet is printed on stdout; progress, warnings, and
diagnostics go to stderr; this module does not use logging.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from collections.abc import Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

from solstone.think.providers.local import LocalProviderError
from solstone.think.providers import local_install
from solstone.think.providers.oci_image import (
    _MANIFEST_ACCEPT,
    OciImageError,
    _download_blob,
    _extract_layer,
    _fetch_token,
    _layer_digests,
    _registry_headers,
    _resolved_under_root,
    _select_arch_manifest,
    _sha256_file,
    _valid_digest_ref,
    _verify_sha256,
)

GHCR_HOST = "ghcr.io"
REPACK_REVISION = "sol1"
CUDA_EULA_SHA256 = "088381bc2d891e719a2a9398645b00bb45f3b24231473a8283ac7e3e66b8a028"
NVIDIA_PACKAGES = ("cuda-cudart-13-3", "libcublas-13-3")
NVIDIA_PACKAGE_FAMILY = "13-3"
NVIDIA_EULA_DOC_PATHS = (
    Path("usr/share/doc/cuda-cudart-13-3/copyright"),
    Path("usr/share/doc/libcublas-13-3/copyright"),
)
NOTICE_FILES = {
    "NVIDIA-CUDA-EULA-13.3.txt": Path("licenses/NVIDIA-CUDA-EULA-13.3.txt"),
    "llama.cpp-LICENSE.txt": Path("licenses/llama.cpp-LICENSE.txt"),
}
REVISION_LABEL = "org.opencontainers.image.revision"
ARTIFACT_RE = re.compile(r"^server-cuda13-(?P<tag>[^:@/]+)$")
LIB_SIBLING_RE = re.compile(r"^lib.*\.so.*$")
CPU_VARIANT_RE = re.compile(r"^libggml-cpu-.*\.so$")
REPO_REF_RE = re.compile(
    r"^(?:(?P<host>[^/]+)/)?(?P<repo>[^:@]+(?:/[^:@]+)+)"
    r"(?::(?P<tag>[^@]+))?(?:@(?P<digest>sha256:[0-9a-f]{64}))?$"
)


@dataclass(frozen=True)
class NormalizedRef:
    repo: str
    tag: str
    build_tag: str
    digest_ref: str | None

    @property
    def upstream_image_ref(self) -> str:
        if self.digest_ref is None:
            return f"{GHCR_HOST}/{self.repo}:{self.tag}"
        return f"{GHCR_HOST}/{self.repo}:{self.tag}@{self.digest_ref}"


@dataclass(frozen=True)
class ManifestPayload:
    data: dict[str, Any]
    digest_ref: str


@dataclass(frozen=True)
class FileInventory:
    name: str
    sha256: str
    size: int
    resolved_source_path: str
    source_path: Path


@dataclass(frozen=True)
class RepackArtifact:
    arch: str
    release_tag: str
    stage_dir: Path
    tarball_path: Path
    sidecar_path: Path
    tarball_sha256: str
    provenance: dict[str, Any]


def parse_image_ref(image_ref: str) -> NormalizedRef:
    match = REPO_REF_RE.fullmatch(image_ref)
    if match is None:
        raise OciImageError("invalid_image_ref", f"invalid OCI image ref: {image_ref}")
    host = match.group("host")
    if host not in {None, GHCR_HOST}:
        raise OciImageError(
            "invalid_image_ref", f"OCI image ref must use ghcr.io: {image_ref}"
        )
    tag = match.group("tag")
    if tag is None:
        raise OciImageError(
            "invalid_image_ref",
            "OCI image ref must carry a server-cuda13-* tag; "
            "the build tag is required for artifact naming",
        )
    tag_match = ARTIFACT_RE.fullmatch(tag)
    if tag_match is None:
        raise OciImageError(
            "invalid_image_ref",
            f"OCI image ref tag must be server-cuda13-*, got {tag!r}; "
            "plain server-cuda is CUDA 12.8 and is not accepted",
        )
    return NormalizedRef(
        repo=match.group("repo"),
        tag=tag,
        build_tag=str(tag_match.group("tag")),
        digest_ref=match.group("digest"),
    )


def fetch_manifest_raw(
    client: Any,
    repo: str,
    reference: str,
    token: str,
) -> ManifestPayload:
    url = f"https://{GHCR_HOST}/v2/{repo}/manifests/{reference}"
    headers = _registry_headers(token)
    headers["Accept"] = _MANIFEST_ACCEPT
    try:
        response = client.get(url, headers=headers)
        response.raise_for_status()
        body = response.content
        data = response.json()
    except Exception as exc:
        raise OciImageError(
            "manifest_fetch_failed", f"failed to fetch OCI manifest {reference}: {exc}"
        ) from exc
    if not isinstance(data, dict):
        raise OciImageError(
            "manifest_fetch_failed", f"OCI manifest {reference} was not an object"
        )
    digest_ref = f"sha256:{hashlib.sha256(body).hexdigest()}"
    header_digest = response.headers.get("Docker-Content-Digest")
    if header_digest is not None and header_digest != digest_ref:
        raise OciImageError(
            "manifest_digest_mismatch",
            (
                f"manifest {reference} digest mismatch: header {header_digest}, "
                f"computed {digest_ref}"
            ),
        )
    if reference.startswith("sha256:") and reference != digest_ref:
        raise OciImageError(
            "manifest_digest_mismatch",
            (
                f"manifest {reference} digest mismatch: expected {reference}, "
                f"computed {digest_ref}"
            ),
        )
    return ManifestPayload(data=data, digest_ref=digest_ref)


def resolve_index_manifest(
    client: Any,
    normalized: NormalizedRef,
    token: str,
) -> ManifestPayload:
    by_tag = fetch_manifest_raw(client, normalized.repo, normalized.tag, token)
    if normalized.digest_ref is not None:
        if by_tag.digest_ref != normalized.digest_ref:
            raise OciImageError(
                "tag_digest_mismatch",
                (
                    f"tag {normalized.tag} resolves to {by_tag.digest_ref}, "
                    f"not {normalized.digest_ref}"
                ),
            )
        by_digest = fetch_manifest_raw(
            client, normalized.repo, normalized.digest_ref, token
        )
        if by_digest.digest_ref != by_tag.digest_ref:
            raise OciImageError(
                "manifest_digest_mismatch",
                (
                    f"digest fetch for {normalized.digest_ref} returned "
                    f"{by_digest.digest_ref}"
                ),
            )
        return by_digest
    return by_tag


def select_arch_manifest(
    client: Any,
    repo: str,
    index: ManifestPayload,
    arch: str,
    token: str,
) -> ManifestPayload:
    manifests = index.data.get("manifests")
    if not isinstance(manifests, list):
        raise OciImageError(
            "manifest_not_index", "top-level OCI manifest is not an index"
        )
    manifest_digest = _select_arch_manifest(manifests, arch)
    return fetch_manifest_raw(client, repo, manifest_digest, token)


def fetch_config_json(
    client: Any,
    repo: str,
    manifest: dict[str, Any],
    blobs_dir: Path,
    token: str,
) -> dict[str, Any]:
    descriptor = manifest.get("config")
    if not isinstance(descriptor, dict):
        raise OciImageError("config_descriptor_invalid", "OCI manifest lacks config")
    if descriptor.get("mediaType") != "application/vnd.oci.image.config.v1+json":
        raise OciImageError(
            "config_descriptor_invalid", "OCI config descriptor has invalid mediaType"
        )
    digest = descriptor.get("digest")
    if not _valid_digest_ref(digest):
        raise OciImageError(
            "config_descriptor_invalid", "OCI config descriptor has invalid digest"
        )
    config_path = blobs_dir / "config.json"
    _download_blob(client, repo, str(digest), config_path, token)
    try:
        data = json.loads(config_path.read_text(encoding="utf-8"))
    except Exception as exc:
        raise OciImageError(
            "config_blob_invalid", f"invalid OCI config JSON: {exc}"
        ) from exc
    if not isinstance(data, dict):
        raise OciImageError("config_blob_invalid", "OCI config JSON was not an object")
    return data


def revision_label(config_json: dict[str, Any]) -> str:
    config = config_json.get("config")
    labels = config.get("Labels") if isinstance(config, dict) else None
    revision = labels.get(REVISION_LABEL) if isinstance(labels, dict) else None
    if not isinstance(revision, str) or not revision:
        raise OciImageError(
            "revision_label_missing",
            f"OCI config label {REVISION_LABEL!r} is required",
        )
    return revision


def config_env(config_json: dict[str, Any]) -> dict[str, str]:
    config = config_json.get("config")
    env = config.get("Env") if isinstance(config, dict) else None
    result: dict[str, str] = {}
    if isinstance(env, list):
        for item in env:
            if isinstance(item, str) and "=" in item:
                key, value = item.split("=", 1)
                result[key] = value
    return result


def apply_layers(
    client: Any,
    repo: str,
    manifest: dict[str, Any],
    rootfs: Path,
    blobs_dir: Path,
    token: str,
) -> None:
    for index, layer_digest in enumerate(_layer_digests(manifest), start=1):
        blob_path = blobs_dir / f"layer-{index}.tar"
        _download_blob(client, repo, layer_digest, blob_path, token)
        _extract_layer(blob_path, rootfs)


def path_for_provenance(path: Path, rootfs: Path) -> str:
    return path.relative_to(rootfs).as_posix()


def find_candidates(rootfs: Path, name: str) -> list[Path]:
    candidates: dict[str, Path] = {}
    direct = rootfs / name
    if direct.is_file():
        candidates[direct.relative_to(rootfs).as_posix()] = direct
    for path in rootfs.rglob(name):
        if path.is_file():
            candidates[path.relative_to(rootfs).as_posix()] = path
    return [candidates[key] for key in sorted(candidates)]


def resolve_wanted_file(
    rootfs: Path, rootfs_resolved: Path, name: str
) -> FileInventory:
    candidates = find_candidates(rootfs, name)
    if not candidates:
        raise OciImageError("wanted_file_missing", f"wanted file missing: {name}")

    records: list[tuple[Path, Path, str, int]] = []
    for candidate in candidates:
        resolved = _resolved_under_root(candidate, rootfs_resolved)
        digest = _sha256_file(resolved)
        size = resolved.stat().st_size
        records.append((candidate, resolved, digest, size))

    digests = {record[2] for record in records}
    if len(digests) > 1:
        paths = ", ".join(path_for_provenance(record[0], rootfs) for record in records)
        raise OciImageError(
            "wanted_file_ambiguous",
            f"wanted file {name} has multiple different-content matches: {paths}",
        )

    records.sort(
        key=lambda record: (len(record[0].relative_to(rootfs).parts), str(record[0]))
    )
    _candidate, resolved, digest, size = records[0]
    return FileInventory(
        name=name,
        sha256=digest,
        size=size,
        resolved_source_path=path_for_provenance(resolved, rootfs),
        source_path=resolved,
    )


def resolve_inventory(rootfs: Path, wanted_files: Sequence[str]) -> list[FileInventory]:
    rootfs_resolved = rootfs.resolve()
    return [resolve_wanted_file(rootfs, rootfs_resolved, name) for name in wanted_files]


def scan_drift(
    rootfs: Path,
    inventory: Sequence[FileInventory],
    wanted_files: Sequence[str],
    stderr: Any,
) -> list[dict[str, Any]]:
    wanted = set(wanted_files)
    cpu_expected = {name for name in wanted if CPU_VARIANT_RE.fullmatch(name)}
    cpu_variants = sorted(
        path for path in rootfs.rglob("libggml-cpu-*.so") if path.is_file()
    )
    unexpected_cpu = [
        path.relative_to(rootfs).as_posix()
        for path in cpu_variants
        if path.name not in cpu_expected
    ]
    if unexpected_cpu:
        raise OciImageError(
            "unexpected_cpu_variant",
            "unexpected libggml-cpu variants: " + ", ".join(unexpected_cpu),
        )

    warnings: list[dict[str, Any]] = []
    directories = sorted({item.source_path.parent for item in inventory})
    seen_paths: set[str] = set()
    for directory in directories:
        for child in sorted(directory.iterdir(), key=lambda path: path.name):
            if not child.is_file() or not LIB_SIBLING_RE.fullmatch(child.name):
                continue
            if child.name in wanted:
                continue
            rel = child.relative_to(rootfs).as_posix()
            if rel in seen_paths:
                continue
            seen_paths.add(rel)
            warning = {
                "code": "unexpected_lib_sibling",
                "message": f"unexpected library sibling not packaged: {rel}",
                "details": {
                    "basename": child.name,
                    "directory": directory.relative_to(rootfs).as_posix(),
                    "path": rel,
                },
            }
            print(f"warning: {warning['message']}", file=stderr)
            warnings.append(warning)
    return warnings


def parse_dpkg_status(status_path: Path) -> dict[str, str]:
    if not status_path.is_file():
        return {}
    versions: dict[str, str] = {}
    current_name: str | None = None
    current_version: str | None = None
    for line in status_path.read_text(encoding="utf-8").splitlines() + [""]:
        if not line:
            if current_name in NVIDIA_PACKAGES and current_version:
                versions[current_name] = current_version
            current_name = None
            current_version = None
            continue
        if line.startswith("Package: "):
            current_name = line.removeprefix("Package: ").strip()
        elif line.startswith("Version: "):
            current_version = line.removeprefix("Version: ").strip()
    return versions


def resolve_versions(
    rootfs: Path, env: dict[str, str]
) -> tuple[dict[str, Any], dict[str, str]]:
    dpkg_versions = parse_dpkg_status(rootfs / "var/lib/dpkg/status")
    env_versions = {
        "cuda-cudart-13-3": env.get("NV_CUDA_CUDART_VERSION"),
        "libcublas-13-3": env.get("NV_LIBCUBLAS_VERSION"),
    }
    packages: dict[str, Any] = {}
    resolution: dict[str, str] = {}
    for package in NVIDIA_PACKAGES:
        if package in dpkg_versions:
            version = dpkg_versions[package]
            rung = "dpkg_status"
        elif env_versions.get(package):
            version = str(env_versions[package])
            rung = "oci_config_env"
        else:
            version = None
            rung = "family_only"
        packages[package] = {
            "family": NVIDIA_PACKAGE_FAMILY,
            "version": version,
        }
        resolution[package] = rung
    return packages, resolution


def repo_root_from_script() -> Path:
    return Path(__file__).resolve().parents[1]


def load_notice_files(
    root: Path,
    expected_eula_sha256: str,
) -> dict[str, bytes]:
    notices: dict[str, bytes] = {}
    for name, relpath in NOTICE_FILES.items():
        path = root / relpath
        if not path.is_file():
            raise OciImageError(
                "notice_file_missing", f"notice file missing: {relpath}"
            )
        notices[name] = path.read_bytes()
    try:
        _verify_sha256(
            root / NOTICE_FILES["NVIDIA-CUDA-EULA-13.3.txt"],
            expected_eula_sha256,
        )
    except OciImageError as exc:
        raise OciImageError(
            "eula_sha_mismatch",
            "committed NVIDIA EULA sha mismatch; trigger CLO re-check",
        ) from exc
    return notices


def verify_image_eula(rootfs: Path, expected_eula_sha256: str) -> None:
    paths = [rootfs / path for path in NVIDIA_EULA_DOC_PATHS]
    missing = [path for path in paths if not path.is_file()]
    if missing:
        names = ", ".join(path.relative_to(rootfs).as_posix() for path in missing)
        raise OciImageError(
            "notice_file_missing", f"NVIDIA EULA doc path missing: {names}"
        )
    contents = [path.read_bytes() for path in paths]
    if contents[0] != contents[1]:
        raise OciImageError(
            "eula_sha_mismatch",
            "NVIDIA EULA doc-path copyright files differ; trigger CLO re-check",
        )
    actual = hashlib.sha256(contents[0]).hexdigest()
    if actual != expected_eula_sha256:
        raise OciImageError(
            "eula_sha_mismatch",
            (
                "NVIDIA EULA doc-path copyright sha mismatch; trigger CLO re-check: "
                f"expected {expected_eula_sha256}, got {actual}"
            ),
        )


def notice_sha256s(notices: dict[str, bytes]) -> dict[str, str]:
    return {
        name: hashlib.sha256(data).hexdigest() for name, data in sorted(notices.items())
    }


def git_commit(root: Path) -> tuple[str | None, list[dict[str, Any]]]:
    warnings: list[dict[str, Any]] = []
    try:
        completed = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "--verify", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )
    except Exception as exc:
        return None, [
            {
                "code": "git_commit_unavailable",
                "message": f"git commit unavailable: {exc}",
                "details": {},
            }
        ]
    commit = completed.stdout.strip() if completed.returncode == 0 else None
    if commit is None:
        warnings.append(
            {
                "code": "git_commit_unavailable",
                "message": (completed.stderr or "git commit unavailable").strip(),
                "details": {},
            }
        )
    try:
        dirty = subprocess.run(
            ["git", "-C", str(root), "status", "--porcelain"],
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )
        if dirty.returncode == 0 and dirty.stdout.strip():
            warnings.append(
                {
                    "code": "git_tree_dirty",
                    "message": "git tree is dirty",
                    "details": {"line_count": len(dirty.stdout.splitlines())},
                }
            )
    except Exception as exc:
        warnings.append(
            {
                "code": "git_status_unavailable",
                "message": f"git status unavailable: {exc}",
                "details": {},
            }
        )
    return commit, warnings


def inventory_payload(inventory: Sequence[FileInventory]) -> list[dict[str, Any]]:
    return [
        {
            "name": item.name,
            "resolved_source_path": item.resolved_source_path,
            "sha256": item.sha256,
            "size": item.size,
        }
        for item in sorted(inventory, key=lambda item: item.name)
    ]


def build_provenance(
    *,
    normalized: NormalizedRef,
    index_digest: str,
    manifest_digest: str,
    revision: str,
    packages: dict[str, Any],
    version_resolution: dict[str, str],
    arch: str,
    build_timestamp_epoch: int,
    inventory: Sequence[FileInventory],
    notices: dict[str, bytes],
    git: str | None,
    verifier: str,
    warnings: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    return {
        "arch": arch,
        "build_tag": normalized.build_tag,
        "build_timestamp_epoch": build_timestamp_epoch,
        "files": inventory_payload(inventory),
        "index_digest": index_digest,
        "manifest_digest": manifest_digest,
        "notice_file_sha256s": notice_sha256s(notices),
        "nvidia_packages": packages,
        "pipeline_git_commit": git,
        "pipeline_script": "scripts/repack_cuda_runtime.py",
        "revision_label": revision,
        "upstream_image_ref": normalized.upstream_image_ref,
        "upstream_tag": normalized.tag,
        "verifier": verifier,
        "version_resolution": version_resolution,
        "warnings": list(warnings),
    }


def copy_stage_files(
    stage_dir: Path,
    inventory: Sequence[FileInventory],
    notices: dict[str, bytes],
    provenance: dict[str, Any],
) -> None:
    try:
        if stage_dir.exists():
            shutil.rmtree(stage_dir)
        stage_dir.mkdir(parents=True)
        for item in inventory:
            shutil.copy2(item.source_path, stage_dir / item.name)
        licenses = stage_dir / "licenses"
        licenses.mkdir()
        for name, data in sorted(notices.items()):
            (licenses / name).write_bytes(data)
        (stage_dir / "provenance.json").write_text(
            json.dumps(provenance, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    except Exception as exc:
        raise OciImageError(
            "artifact_write_failed", f"failed to stage artifact files: {exc}"
        ) from exc


def tar_mode(path: Path) -> int:
    return 0o755 if path.stat().st_mode & 0o111 else 0o644


def write_deterministic_tar(
    stage_dir: Path, tarball_path: Path, timestamp: int
) -> None:
    files = sorted(path for path in stage_dir.rglob("*") if path.is_file())
    tarball_path.parent.mkdir(parents=True, exist_ok=True)
    tmp = tarball_path.with_suffix(tarball_path.suffix + ".tmp")
    try:
        with tmp.open("wb") as raw:
            with gzip.GzipFile(
                filename="",
                mode="wb",
                fileobj=raw,
                compresslevel=9,
                mtime=0,
            ) as gz:
                with tarfile.open(
                    fileobj=gz, mode="w", format=tarfile.PAX_FORMAT
                ) as tar:
                    for path in files:
                        arcname = path.relative_to(stage_dir).as_posix()
                        info = tar.gettarinfo(str(path), arcname=arcname)
                        info.uid = 0
                        info.gid = 0
                        info.uname = ""
                        info.gname = ""
                        info.mtime = timestamp
                        info.mode = tar_mode(path)
                        info.pax_headers = {}
                        with path.open("rb") as handle:
                            tar.addfile(info, handle)
        tmp.replace(tarball_path)
    except Exception as exc:
        tmp.unlink(missing_ok=True)
        raise OciImageError(
            "artifact_write_failed", f"failed to write deterministic tarball: {exc}"
        ) from exc


def write_sidecar(tarball_path: Path) -> Path:
    try:
        sha = _sha256_file(tarball_path)
        sidecar = tarball_path.with_suffix(tarball_path.suffix + ".sha256")
        sidecar.write_text(f"{sha}  {tarball_path.name}\n", encoding="utf-8")
        return sidecar
    except Exception as exc:
        raise OciImageError(
            "artifact_write_failed", f"failed to write artifact sidecar: {exc}"
        ) from exc


def artifact_basename(build_tag: str, arch: str) -> str:
    return f"llama-{build_tag}-bin-linux-cuda13-{arch}-{REPACK_REVISION}"


def validate_arch(arch: str) -> tuple[str, ...]:
    try:
        return local_install.cuda_server_pin().wanted_files_for_arch(arch)
    except LocalProviderError as exc:
        raise OciImageError(
            "unsupported_arch", f"unsupported OCI architecture {arch}: {exc}"
        ) from exc


def repack_arch(
    *,
    client: Any,
    normalized: NormalizedRef,
    index: ManifestPayload,
    arch: str,
    output_dir: Path,
    timestamp: int,
    token: str,
    verifier: str,
    notices: dict[str, bytes],
    expected_eula_sha256: str,
    git: str | None,
    git_warnings: Sequence[dict[str, Any]],
    stderr: Any,
) -> RepackArtifact:
    wanted_files = validate_arch(arch)
    with tempfile.TemporaryDirectory(dir=output_dir) as temp:
        work = Path(temp)
        rootfs = work / "rootfs"
        blobs = work / "blobs"
        rootfs.mkdir()
        blobs.mkdir()
        manifest = select_arch_manifest(client, normalized.repo, index, arch, token)
        config_json = fetch_config_json(
            client, normalized.repo, manifest.data, blobs, token
        )
        revision = revision_label(config_json)
        apply_layers(client, normalized.repo, manifest.data, rootfs, blobs, token)
        verify_image_eula(rootfs, expected_eula_sha256)
        inventory = resolve_inventory(rootfs, wanted_files)
        warnings = list(git_warnings)
        warnings.extend(scan_drift(rootfs, inventory, wanted_files, stderr))
        packages, version_resolution = resolve_versions(rootfs, config_env(config_json))
        provenance = build_provenance(
            normalized=normalized,
            index_digest=index.digest_ref,
            manifest_digest=manifest.digest_ref,
            revision=revision,
            packages=packages,
            version_resolution=version_resolution,
            arch=arch,
            build_timestamp_epoch=timestamp,
            inventory=inventory,
            notices=notices,
            git=git,
            verifier=verifier,
            warnings=warnings,
        )
        basename = artifact_basename(normalized.build_tag, arch)
        stage_dir = output_dir / basename
        copy_stage_files(stage_dir, inventory, notices, provenance)
        tarball = output_dir / f"{basename}.tar.gz"
        write_deterministic_tar(stage_dir, tarball, timestamp)
        sidecar = write_sidecar(tarball)
        return RepackArtifact(
            arch=arch,
            release_tag=normalized.build_tag,
            stage_dir=stage_dir,
            tarball_path=tarball,
            sidecar_path=sidecar,
            tarball_sha256=_sha256_file(tarball),
            provenance=provenance,
        )


def remove_published_path(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink(missing_ok=True)


def publish_artifacts(
    artifacts: Sequence[RepackArtifact],
    output_dir: Path,
) -> list[RepackArtifact]:
    final_artifacts: list[RepackArtifact] = []
    final_paths: list[Path] = []
    for artifact in artifacts:
        final_stage = output_dir / artifact.stage_dir.name
        final_tarball = output_dir / artifact.tarball_path.name
        final_sidecar = output_dir / artifact.sidecar_path.name
        final_paths.extend([final_stage, final_tarball, final_sidecar])
        final_artifacts.append(
            replace(
                artifact,
                stage_dir=final_stage,
                tarball_path=final_tarball,
                sidecar_path=final_sidecar,
            )
        )

    existing = [path for path in final_paths if path.exists() or path.is_symlink()]
    if existing:
        names = ", ".join(path.name for path in existing)
        raise OciImageError(
            "artifact_write_failed", f"artifact output already exists: {names}"
        )

    try:
        output_dir.mkdir(parents=True, exist_ok=True)
        for source, final in zip(artifacts, final_artifacts, strict=True):
            shutil.move(str(source.stage_dir), final.stage_dir)
            shutil.move(str(source.tarball_path), final.tarball_path)
            shutil.move(str(source.sidecar_path), final.sidecar_path)
        return final_artifacts
    except Exception as exc:
        for path in reversed(final_paths):
            try:
                remove_published_path(path)
            except OSError:
                pass
        raise OciImageError(
            "artifact_write_failed", f"failed to publish artifacts: {exc}"
        ) from exc


def ensure_client(client: Any | None) -> tuple[Any, bool]:
    if client is not None:
        return client, False
    import httpx

    return httpx.Client(follow_redirects=True, timeout=600.0), True


def repack_cuda_runtime(
    *,
    image_ref: str,
    arches: Sequence[str],
    build_timestamp_epoch: int,
    verifier: str,
    output_dir: Path,
    client: Any | None = None,
    expected_eula_sha256: str = CUDA_EULA_SHA256,
    repo_root: Path | None = None,
    stderr: Any = sys.stderr,
) -> list[RepackArtifact]:
    if build_timestamp_epoch < 0:
        raise OciImageError(
            "invalid_build_timestamp", "build timestamp must be non-negative"
        )
    if not verifier:
        raise OciImageError("invalid_verifier", "verifier is required")
    normalized = parse_image_ref(image_ref)
    root = repo_root or repo_root_from_script()
    notices = load_notice_files(root, expected_eula_sha256)
    git, git_warnings = git_commit(root)
    http_client, created = ensure_client(client)
    try:
        token = _fetch_token(http_client, normalized.repo)
        index = resolve_index_manifest(http_client, normalized, token)
        try:
            output_dir.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(dir=output_dir.parent) as temp:
                staging_output = Path(temp) / "outputs"
                staging_output.mkdir()
                artifacts: list[RepackArtifact] = []
                for arch in arches:
                    print(
                        f"repacking {arch} from {normalized.upstream_image_ref}",
                        file=stderr,
                    )
                    artifacts.append(
                        repack_arch(
                            client=http_client,
                            normalized=normalized,
                            index=index,
                            arch=arch,
                            output_dir=staging_output,
                            timestamp=build_timestamp_epoch,
                            token=token,
                            verifier=verifier,
                            notices=notices,
                            expected_eula_sha256=expected_eula_sha256,
                            git=git,
                            git_warnings=git_warnings,
                            stderr=stderr,
                        )
                    )
                return publish_artifacts(artifacts, output_dir)
        except OciImageError:
            raise
        except Exception as exc:
            raise OciImageError(
                "artifact_write_failed", f"failed to prepare artifact staging: {exc}"
            ) from exc
    finally:
        if created:
            http_client.close()


def supported_arches() -> tuple[str, ...]:
    return tuple(local_install.cuda_server_pin().cpu_wanted_files_by_arch)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image-ref", required=True)
    parser.add_argument(
        "--arch",
        action="append",
        choices=supported_arches(),
        dest="arches",
        help="OCI architecture to repackage; repeat for multiple arches",
    )
    parser.add_argument("--build-timestamp", required=True, type=int)
    parser.add_argument("--verifier", required=True)
    parser.add_argument("--output-dir", required=True, type=Path)
    return parser.parse_args(argv)


def print_pin_snippet(artifacts: Sequence[RepackArtifact]) -> None:
    pin = local_install.cuda_server_pin()
    payload = {
        artifact.arch: {
            "binary_name": pin.binary_name,
            "filename": artifact.tarball_path.name,
            "release_tag": artifact.release_tag,
            "sha256": artifact.tarball_sha256,
        }
        for artifact in artifacts
    }
    print(json.dumps(payload, indent=2, sort_keys=True))


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        artifacts = repack_cuda_runtime(
            image_ref=args.image_ref,
            arches=args.arches or supported_arches(),
            build_timestamp_epoch=args.build_timestamp,
            verifier=args.verifier,
            output_dir=args.output_dir,
        )
    except OciImageError as exc:
        print(f"error[{exc.reason_code}]: {exc}", file=sys.stderr)
        return 1
    print_pin_snippet(artifacts)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
