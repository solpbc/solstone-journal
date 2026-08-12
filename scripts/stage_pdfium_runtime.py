#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Stage the pinned PDFium runtime and notices for the sol-pdf wheel.

The archive hashes pin the bytes downloaded from bblanchon/pdfium-binaries.
GitHub's attestation verification is deliberately separate: a matching digest
alone does not establish that the release workflow produced those bytes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACKAGE_DIR = ROOT / "packages" / "solstone-core-pdf"
DEFAULT_LINK_ROOT = ROOT / "target" / "pdfium-runtime-link"
DEFAULT_RECEIPT_ROOT = ROOT / "target" / "pdfium-runtime-provenance"
RELEASE_TAG = "chromium/7920"
RELEASE_URL = (
    "https://github.com/bblanchon/pdfium-binaries/releases/download/"
    f"{RELEASE_TAG}"
)
ATTESTATION_NAME = "pdfium-attestation.json"
ATTESTATION_SHA256 = "41cdeff1f9db4f340e80857fcdec11e9ef168204b9aafb663aaf0a34c6052aee"
ATTESTATION_URL = f"{RELEASE_URL}/{ATTESTATION_NAME}"
ATTESTATION_REPOSITORY = "bblanchon/pdfium-binaries"
LIB_MODE = 0o755
NOTICE_MODE = 0o644


class StageError(RuntimeError):
    """Raised when pinned PDFium bytes cannot be staged safely."""


@dataclass(frozen=True)
class TargetSpec:
    key: str
    archive_name: str
    archive_sha256: str
    library_member: str
    library_name: str
    library_sha256: str

    @property
    def archive_url(self) -> str:
        return f"{RELEASE_URL}/{self.archive_name}"


TARGETS = {
    "linux-x86_64": TargetSpec(
        key="linux-x86_64",
        archive_name="pdfium-linux-x64.tgz",
        archive_sha256="49ab3afbd4e6c1e284b5f2898129c8bb8a10fd785c1c5392c8c1fc70242f9ced",
        library_member="lib/libpdfium.so",
        library_name="libpdfium.so",
        library_sha256="687dce861f959c7097d47c5864509d51a926a71b38322596a8ee3e7a99c6b96e",
    ),
    "linux-aarch64": TargetSpec(
        key="linux-aarch64",
        archive_name="pdfium-linux-arm64.tgz",
        archive_sha256="00551476a77fbc1a31c37573eadc9b63f1c366f65ad727539326927da083bb4d",
        library_member="lib/libpdfium.so",
        library_name="libpdfium.so",
        library_sha256="933f3d620cc8b58fb30a7f12a1bce8bf276da65caf39ff8fb2d04bc1268d53a3",
    ),
    "macos-x86_64": TargetSpec(
        key="macos-x86_64",
        archive_name="pdfium-mac-x64.tgz",
        archive_sha256="0c78b8d55a4c97e02c9bb516997253cb972739373009cf29554c959a2f6b194a",
        library_member="lib/libpdfium.dylib",
        library_name="libpdfium.dylib",
        library_sha256="8fdf8fc61c85676515321b0c214fb1afa0e157cffdadbdff40802e7b4bed7ad6",
    ),
    "macos-arm64": TargetSpec(
        key="macos-arm64",
        archive_name="pdfium-mac-arm64.tgz",
        archive_sha256="c032aa59be58b0f12e41e76a8ef707e347b9841b0426446f646b2568d350ec4f",
        library_member="lib/libpdfium.dylib",
        library_name="libpdfium.dylib",
        library_sha256="df568fcd17a6a6296956aa79abea1181db187458432f360b084fec1cea7cd4d9",
    ),
}

ROOT_MEMBERS = frozenset({"LICENSE", "PDFiumConfig.cmake", "VERSION", "args.gn"})
HEADER_MEMBERS = frozenset(
    {
        "include/cpp/fpdf_deleters.h",
        "include/cpp/fpdf_scopers.h",
        "include/fpdf_annot.h",
        "include/fpdf_attachment.h",
        "include/fpdf_catalog.h",
        "include/fpdf_dataavail.h",
        "include/fpdf_doc.h",
        "include/fpdf_edit.h",
        "include/fpdf_ext.h",
        "include/fpdf_flatten.h",
        "include/fpdf_formfill.h",
        "include/fpdf_fwlevent.h",
        "include/fpdf_javascript.h",
        "include/fpdf_ppo.h",
        "include/fpdf_progressive.h",
        "include/fpdf_save.h",
        "include/fpdf_searchex.h",
        "include/fpdf_signature.h",
        "include/fpdf_structtree.h",
        "include/fpdf_sysfontinfo.h",
        "include/fpdf_text.h",
        "include/fpdf_thumbnail.h",
        "include/fpdf_transformpage.h",
        "include/fpdfview.h",
        "include/fpdfview.h.orig",
    }
)
NOTICE_MEMBERS = frozenset(
    {
        "licenses/abseil.txt",
        "licenses/agg23.txt",
        "licenses/fast_float.txt",
        "licenses/freetype.txt",
        "licenses/icu.txt",
        "licenses/lcms.txt",
        "licenses/libjpeg_turbo.ijg",
        "licenses/libjpeg_turbo.md",
        "licenses/libopenjpeg.txt",
        "licenses/libpng.txt",
        "licenses/libtiff.txt",
        "licenses/llvm-libc.txt",
        "licenses/pdfium.txt",
        "licenses/simdutf.txt",
        "licenses/zlib.txt",
    }
)
NOTICE_SHA256 = {
    "licenses/abseil.txt": "c79a7fea0e3cac04cd43f20e7b648e5a0ff8fa5344e644b0ee09ca1162b62747",
    "licenses/agg23.txt": "c110d3ea2ad77467ce0dcff7d3337e6c8be8049a5103f4b9bd5fd911a77972e5",
    "licenses/fast_float.txt": "e562f3f974ced7e69dd1db77b820b36bcf8f30377f1aa105723fba449c53c4e6",
    "licenses/freetype.txt": "f4b133e25df1f86ad3ffea453aa0e613f0474f34778dbbb3e437e7b2724937d8",
    "licenses/icu.txt": "e55522d81edc687a341a4411e0776e54ca654e90147f354a90458aaced4116af",
    "licenses/lcms.txt": "7312b68c5b25e9bf2b828706fb4e29588f22705112f411fd42e1f7d84c3d139a",
    "licenses/libjpeg_turbo.ijg": "75815e3bf6484201a3c3d17a1bbf10f2e8e3237f84df10a2357ea896db2a81d6",
    "licenses/libjpeg_turbo.md": "96f5b328adbb78eeaaec6980d73fd558cb1e4d62560ed615646bc3cf5e532430",
    "licenses/libopenjpeg.txt": "c5ab0890a737c2dfa7ba675036554f6d17741d98629b0c2a145354d00617e6b2",
    "licenses/libpng.txt": "bdb0a645ea18c60507d0368379b1ac5474b92255fcc2d115e07486a7672ba526",
    "licenses/libtiff.txt": "92b72ba97e6c2749c2a94bc0ef646b47080217f1e772a482b33cf5a5f98a6506",
    "licenses/llvm-libc.txt": "ebcd9bbf783a73d05c53ba4d586b8d5813dcdf3bbec50265860ccc885e606f47",
    "licenses/pdfium.txt": "961eacd9633fff6d051db7208b755e9210e30efac7adec3e6a6d52798f0ccf0e",
    "licenses/simdutf.txt": "fc8dbc04e03ad4efc08a647ffe7f995b811a95bc04c0e85a56d5277c6593fa5f",
    "licenses/zlib.txt": "33fd641c9f3b0e0be64bc78fea9e94807674cdd70c48477599226cb8956565fe",
}
MACOS_PDFIUM_NOTICE_SHA256 = (
    "1fe9dea718fbd75cf149adaf4d8a22a4335604d964ddb76d1b45383dec8668c9"
)
DIRECTORY_MEMBERS = frozenset({"include", "include/cpp", "lib", "licenses"})


def runtime_install_dir(package_name: str) -> Path:
    return Path("data/lib") / package_name


def notice_install_dir(package_name: str) -> Path:
    return Path("data/share") / package_name / "licenses"


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _relative(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def _fail(message: str) -> None:
    raise StageError(message)


def _download(url: str, destination: Path) -> None:
    with urllib.request.urlopen(url) as response, destination.open("wb") as handle:
        shutil.copyfileobj(response, handle)


def _download_verified(url: str, expected_sha256: str, destination: Path, label: str) -> None:
    _download(url, destination)
    actual_sha256 = _sha256_file(destination)
    if actual_sha256 != expected_sha256:
        destination.unlink(missing_ok=True)
        _fail(
            f"{label} digest mismatch\n"
            f"  expected: {expected_sha256}\n"
            f"  actual: {actual_sha256}\n"
            "  repair: stop; the pinned upstream bytes changed"
        )


def _verify_attestation(archive: Path) -> dict[str, object]:
    command = [
        "gh",
        "attestation",
        "verify",
        str(archive),
        "--repo",
        ATTESTATION_REPOSITORY,
    ]
    try:
        completed = subprocess.run(command, capture_output=True, text=True, check=False)
    except FileNotFoundError as exc:
        _fail(
            "GitHub CLI is required to verify the pinned PDFium build attestation; "
            "install and authenticate gh rather than bypassing this control"
        )
        raise AssertionError("unreachable") from exc
    if completed.returncode != 0:
        _fail(
            "PDFium build attestation verification failed\n"
            f"  command: {' '.join(command)}\n"
            f"  stdout: {completed.stdout.strip() or '<empty>'}\n"
            f"  stderr: {completed.stderr.strip() or '<empty>'}"
        )
    return {
        "command": command,
        "returncode": completed.returncode,
        "stdout": completed.stdout.strip(),
        "stderr": completed.stderr.strip(),
    }


def _expected_members(spec: TargetSpec) -> frozenset[str]:
    return frozenset(
        {
            *ROOT_MEMBERS,
            *HEADER_MEMBERS,
            *NOTICE_MEMBERS,
            *DIRECTORY_MEMBERS,
            spec.library_member,
        }
    )


def _expected_member_sha256(spec: TargetSpec, member: str) -> str:
    if member == spec.library_member:
        return spec.library_sha256
    if member == "licenses/pdfium.txt" and spec.key.startswith("macos-"):
        return MACOS_PDFIUM_NOTICE_SHA256
    return NOTICE_SHA256[member]


def _read_members(archive: tarfile.TarFile, spec: TargetSpec) -> dict[str, bytes]:
    members = archive.getmembers()
    names = {member.name for member in members}
    expected = _expected_members(spec)
    if names != expected:
        _fail(
            "PDFium archive member set is not the reviewed chromium/7920 layout\n"
            f"  missing: {', '.join(sorted(expected - names)) or '<none>'}\n"
            f"  unexpected: {', '.join(sorted(names - expected)) or '<none>'}\n"
            "  repair: stop and review the upstream archive layout before changing this allowlist"
        )
    members_by_name = {member.name: member for member in members}
    for name in expected - DIRECTORY_MEMBERS:
        if not members_by_name[name].isreg():
            _fail(f"PDFium archive member is not a regular file: {name}")
    extracted: dict[str, bytes] = {}
    for name in {spec.library_member, *NOTICE_MEMBERS}:
        stream = archive.extractfile(members_by_name[name])
        if stream is None:
            _fail(f"PDFium archive member cannot be read: {name}")
        data = stream.read()
        actual_sha256 = _sha256_bytes(data)
        expected_sha256 = _expected_member_sha256(spec, name)
        if actual_sha256 != expected_sha256:
            _fail(
                f"PDFium extracted member digest mismatch: {name}\n"
                f"  expected: {expected_sha256}\n"
                f"  actual: {actual_sha256}\n"
                "  repair: stop; the pinned upstream archive contents changed"
            )
        extracted[name] = data
    return extracted


def _write_file(path: Path, data: bytes, mode: int) -> dict[str, object]:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(mode)
    return {"path": _relative(path), "sha256": _sha256_bytes(data), "size": len(data)}


def _default_target() -> str:
    system = platform.system()
    machine = platform.machine().lower()
    if system == "Linux" and machine in {"x86_64", "amd64"}:
        return "linux-x86_64"
    if system == "Linux" and machine in {"aarch64", "arm64"}:
        return "linux-aarch64"
    if system == "Darwin" and machine in {"x86_64", "amd64"}:
        return "macos-x86_64"
    if system == "Darwin" and machine in {"aarch64", "arm64"}:
        return "macos-arm64"
    _fail(f"unsupported host for default PDFium runtime target: {system}/{machine}")
    raise AssertionError("unreachable")


def stage_runtime(
    *,
    spec: TargetSpec,
    package_dir: Path,
    receipt_path: Path,
) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="solstone-pdfium-") as temporary:
        temporary_dir = Path(temporary)
        archive_path = temporary_dir / spec.archive_name
        attestation_path = temporary_dir / ATTESTATION_NAME
        _download_verified(spec.archive_url, spec.archive_sha256, archive_path, "PDFium archive")
        _download_verified(
            ATTESTATION_URL,
            ATTESTATION_SHA256,
            attestation_path,
            "PDFium attestation",
        )
        attestation = _verify_attestation(archive_path)
        with tarfile.open(archive_path, mode="r:gz") as archive:
            extracted = _read_members(archive, spec)

    wheel_data_root = package_dir / "wheel-data"
    shutil.rmtree(wheel_data_root, ignore_errors=True)
    runtime_dir = wheel_data_root / runtime_install_dir(package_dir.name)
    notice_dir = wheel_data_root / notice_install_dir(package_dir.name)
    runtime_record = _write_file(
        runtime_dir / spec.library_name, extracted[spec.library_member], LIB_MODE
    )
    notice_records = []
    for member in sorted(NOTICE_MEMBERS):
        record = _write_file(notice_dir / Path(member).name, extracted[member], NOTICE_MODE)
        record["source_member"] = member
        notice_records.append(record)

    link_dir = DEFAULT_LINK_ROOT / spec.key
    shutil.rmtree(link_dir, ignore_errors=True)
    link_dir.mkdir(parents=True)
    _write_file(link_dir / spec.library_name, extracted[spec.library_member], LIB_MODE)

    receipt = {
        "schema": "solstone.pdfium-runtime-provenance.v1",
        "package": package_dir.name,
        "target": spec.key,
        "release_tag": RELEASE_TAG,
        "archive": {
            "url": spec.archive_url,
            "sha256": spec.archive_sha256,
            "name": spec.archive_name,
        },
        "attestation": {
            "url": ATTESTATION_URL,
            "sha256": ATTESTATION_SHA256,
            "verification": attestation,
        },
        "runtime_library": {"source_member": spec.library_member, **runtime_record},
        "notices": notice_records,
        "wheel_data_root": _relative(wheel_data_root),
        "link_dir": _relative(link_dir),
    }
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return receipt


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Stage pinned PDFium bytes for solstone-core-pdf.")
    parser.add_argument("--target", choices=sorted(TARGETS), default=None)
    parser.add_argument("--package-dir", type=Path, default=PACKAGE_DIR)
    parser.add_argument("--receipt", type=Path, default=None)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    target = args.target or _default_target()
    receipt_path = args.receipt or (DEFAULT_RECEIPT_ROOT / f"{target}.json")
    try:
        receipt = stage_runtime(
            spec=TARGETS[target],
            package_dir=args.package_dir,
            receipt_path=receipt_path,
        )
    except (OSError, StageError, tarfile.TarError, urllib.error.URLError) as exc:
        print(f"PDFium runtime staging failed: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(receipt, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
