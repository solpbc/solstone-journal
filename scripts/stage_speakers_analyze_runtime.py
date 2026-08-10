#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Stage CPU ONNX Runtime bytes for the ONNX-bundling helper wheels.

The pinned wheel URL/digest table below is the security-relevant part of this
script and is deliberately single-sourced: every helper package that bundles the
same CPU ONNX Runtime (`solstone-core-speakers-analyze`,
`solstone-core-vad-analyze`) stages from this one table. Only the *destination*
varies, and it is derived from the target package directory's own name so the
staged payload always matches that package's `build.rs` rpath
(`$ORIGIN/../lib/<package>`).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import shutil
import sys
import tempfile
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parent.parent
PACKAGE_DIR = ROOT / "packages" / "solstone-core-speakers-analyze"
DEFAULT_CACHE_DIR = ROOT / "target" / "speakers-analyze-runtime-cache"
DEFAULT_LINK_ROOT = ROOT / "target" / "speakers-analyze-runtime-link"
DEFAULT_RECEIPT_ROOT = ROOT / "target" / "speakers-analyze-runtime-provenance"


def runtime_install_dir(package_name: str) -> Path:
    """Return the wheel-data runtime library directory for ``package_name``.

    This must stay in lockstep with the crate's `build.rs` rpath, which is
    `$ORIGIN/../lib/<package_name>` on Linux.
    """

    return Path("data/lib") / package_name


def notice_install_dir(package_name: str) -> Path:
    """Return the wheel-data licence directory for ``package_name``."""

    return Path("data/share") / package_name / "licenses"


RUNTIME_INSTALL_DIR = runtime_install_dir("solstone-core-speakers-analyze")
NOTICE_INSTALL_DIR = notice_install_dir("solstone-core-speakers-analyze")
LIB_MODE = 0o755
NOTICE_MODE = 0o644
FORBIDDEN_GPU_MEMBERS = (
    "onnxruntime/capi/libonnxruntime_providers_cuda.so",
    "onnxruntime/capi/libonnxruntime_providers_tensorrt.so",
)


class StageError(RuntimeError):
    """Raised when runtime staging cannot proceed safely."""


@dataclass(frozen=True)
class NoticeSpec:
    source_member: str
    staged_name: str
    sha256: str


@dataclass(frozen=True)
class TargetSpec:
    key: str
    wheel_url: str
    wheel_sha256: str
    runtime_member: str
    runtime_sha256: str
    runtime_staged_name: str
    link_names: tuple[str, ...]
    notices: tuple[NoticeSpec, ...]

    @property
    def wheel_name(self) -> str:
        return self.wheel_url.rsplit("/", 1)[-1]


COMMON_NOTICES = (
    NoticeSpec(
        source_member="onnxruntime/LICENSE",
        staged_name="onnxruntime-LICENSE.txt",
        sha256="2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c",
    ),
    NoticeSpec(
        source_member="onnxruntime/ThirdPartyNotices.txt",
        staged_name="onnxruntime-ThirdPartyNotices.txt",
        sha256="0e07b95f3a8d6230037707c5c4a2b554d12c4cb67369669ac255635528ffcee2",
    ),
)

TARGETS = {
    "linux-x86_64": TargetSpec(
        key="linux-x86_64",
        wheel_url=(
            "https://files.pythonhosted.org/packages/a9/1b/"
            "d681878f227513917d8620e4ea504af5eb3313fc01f8aea7b19a976c65db/"
            "onnxruntime-1.25.0-cp312-cp312-manylinux_2_27_x86_64."
            "manylinux_2_28_x86_64.whl"
        ),
        wheel_sha256="be93baa694ef8e5831fcb7b542da21f502b122918b5b9612d9f02972e043ee01",
        runtime_member="onnxruntime/capi/libonnxruntime.so.1.25.0",
        runtime_sha256="6976c9c6b2db120e835a7091e2f4bd2308a76d3856a7181beb7e7a9b1e08f9e5",
        runtime_staged_name="libonnxruntime.so.1",
        link_names=(
            "libonnxruntime.so.1.25.0",
            "libonnxruntime.so.1",
            "libonnxruntime.so",
        ),
        notices=COMMON_NOTICES,
    ),
    "linux-aarch64": TargetSpec(
        key="linux-aarch64",
        wheel_url=(
            "https://files.pythonhosted.org/packages/5a/c6/"
            "19c5bfbc60396791e975652f982bcff9ff4b27947c8e2bf0064ac5d5727b/"
            "onnxruntime-1.25.0-cp312-cp312-manylinux_2_27_aarch64."
            "manylinux_2_28_aarch64.whl"
        ),
        wheel_sha256="9c99238d20bfa80ac68c7b03c2c936d389189ae40997f78a30d151570d7e18bf",
        runtime_member="onnxruntime/capi/libonnxruntime.so.1.25.0",
        runtime_sha256="d47425026b2474e1deb0b8cf22f74cd943539af85873aa3fb8052862445beef3",
        runtime_staged_name="libonnxruntime.so.1",
        link_names=(
            "libonnxruntime.so.1.25.0",
            "libonnxruntime.so.1",
            "libonnxruntime.so",
        ),
        notices=COMMON_NOTICES,
    ),
    "macos-arm64": TargetSpec(
        key="macos-arm64",
        wheel_url=(
            "https://files.pythonhosted.org/packages/7a/69/"
            "f98c6bda4c34ac382b70c36033a989ceffd1caf5afba47bd2ef26535850f/"
            "onnxruntime-1.25.0-cp312-cp312-macosx_14_0_arm64.whl"
        ),
        wheel_sha256="8ecd3362de3fb496fb3e2d055a95d5acab611cf759a27609c6d99704c9d8f184",
        runtime_member="onnxruntime/capi/libonnxruntime.1.25.0.dylib",
        runtime_sha256="bafe7d3f3fa8e31195501e5694e73ef240708d5df039feb272b8d506d2783a74",
        runtime_staged_name="libonnxruntime.1.25.0.dylib",
        link_names=("libonnxruntime.1.25.0.dylib", "libonnxruntime.dylib"),
        notices=COMMON_NOTICES,
    ),
}


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _fail(label: str, *, expected: str, actual: str, repair: str) -> None:
    raise StageError(
        f"{label}\n  expected: {expected}\n  actual: {actual}\n  repair: {repair}"
    )


def _relative(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def _default_target() -> str:
    system = platform.system()
    machine = platform.machine().lower()
    if system == "Darwin" and machine in {"arm64", "aarch64"}:
        return "macos-arm64"
    if system == "Linux" and machine in {"x86_64", "amd64"}:
        return "linux-x86_64"
    if system == "Linux" and machine in {"aarch64", "arm64"}:
        return "linux-aarch64"
    _fail(
        "unsupported host for default speakers-analyze runtime target",
        expected=", ".join(sorted(TARGETS)),
        actual=f"{system}/{machine}",
        repair="pass --target explicitly for a supported runtime target",
    )
    raise AssertionError("unreachable")


def _assert_lock_contains(spec: TargetSpec) -> None:
    lock_path = ROOT / "uv.lock"
    try:
        lock_text = lock_path.read_text(encoding="utf-8")
    except FileNotFoundError as exc:
        _fail(
            "uv.lock is missing",
            expected="uv.lock containing pinned onnxruntime wheel URLs",
            actual="missing",
            repair="restore uv.lock before staging runtime bytes",
        )
        raise AssertionError("unreachable") from exc

    url_text = f'url = "{spec.wheel_url}"'
    hash_text = f'hash = "sha256:{spec.wheel_sha256}"'
    if url_text not in lock_text or hash_text not in lock_text:
        _fail(
            "pinned onnxruntime wheel is not present in uv.lock",
            expected=f"{url_text} with {hash_text}",
            actual="missing URL or hash",
            repair=(
                "restore the accepted uv.lock entry or update "
                "scripts/stage_speakers_analyze_runtime.py through design review"
            ),
        )


def _download(url: str, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        prefix=f"{dest.name}.",
        suffix=".part",
        dir=dest.parent,
        delete=False,
    ) as tmp:
        tmp_path = Path(tmp.name)
    try:
        with urllib.request.urlopen(url) as response, tmp_path.open("wb") as handle:
            shutil.copyfileobj(response, handle)
        tmp_path.replace(dest)
    except Exception:
        tmp_path.unlink(missing_ok=True)
        raise


def _ensure_wheel(spec: TargetSpec, cache_dir: Path, *, offline: bool) -> Path:
    cache_path = cache_dir / spec.wheel_name
    if cache_path.exists():
        actual = _sha256_file(cache_path)
        if actual != spec.wheel_sha256:
            _fail(
                "cached onnxruntime wheel digest mismatch",
                expected=spec.wheel_sha256,
                actual=actual,
                repair=f"delete {_relative(cache_path)} and rerun staging",
            )
        return cache_path

    if offline:
        _fail(
            "onnxruntime wheel cache miss in offline mode",
            expected=_relative(cache_path),
            actual="missing",
            repair="run staging once with network access or preseed the verified cache",
        )

    _download(spec.wheel_url, cache_path)
    actual = _sha256_file(cache_path)
    if actual != spec.wheel_sha256:
        cache_path.unlink(missing_ok=True)
        _fail(
            "downloaded onnxruntime wheel digest mismatch",
            expected=spec.wheel_sha256,
            actual=actual,
            repair="retry the download; if it persists, stop because the pinned bytes changed",
        )
    return cache_path


def _zip_member(zip_file: ZipFile, member: str) -> bytes:
    try:
        return zip_file.read(member)
    except KeyError:
        _fail(
            "onnxruntime wheel missing expected member",
            expected=member,
            actual="missing",
            repair="restore the pinned onnxruntime wheel or update the target table through design review",
        )
        raise AssertionError("unreachable")


def _write_file(path: Path, data: bytes, mode: int) -> dict[str, object]:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(mode)
    return {
        "path": _relative(path),
        "sha256": _sha256_bytes(data),
        "size": len(data),
    }


def _stage_link_dir(spec: TargetSpec, link_dir: Path, runtime_data: bytes) -> None:
    shutil.rmtree(link_dir, ignore_errors=True)
    link_dir.mkdir(parents=True)
    primary_name = spec.link_names[0]
    primary = link_dir / primary_name
    primary.write_bytes(runtime_data)
    primary.chmod(LIB_MODE)
    for name in spec.link_names[1:]:
        link_path = link_dir / name
        try:
            link_path.symlink_to(primary_name)
        except OSError:
            shutil.copy2(primary, link_path)


def stage_runtime(
    *,
    spec: TargetSpec,
    package_dir: Path,
    cache_dir: Path,
    link_root: Path,
    receipt_path: Path,
    offline: bool,
) -> dict[str, object]:
    _assert_lock_contains(spec)
    wheel_path = _ensure_wheel(spec, cache_dir, offline=offline)
    wheel_digest = _sha256_file(wheel_path)
    if wheel_digest != spec.wheel_sha256:
        _fail(
            "onnxruntime wheel digest mismatch before extraction",
            expected=spec.wheel_sha256,
            actual=wheel_digest,
            repair=f"delete {_relative(wheel_path)} and rerun staging",
        )

    wheel_data_root = package_dir / "wheel-data"
    shutil.rmtree(wheel_data_root, ignore_errors=True)
    link_dir = link_root / spec.key
    # Derived from the destination package, never from a module constant: the
    # staged payload has to land where that package's own build.rs rpath looks.
    package_runtime_dir = runtime_install_dir(package_dir.name)
    package_notice_dir = notice_install_dir(package_dir.name)

    with ZipFile(wheel_path) as zip_file:
        names = set(zip_file.namelist())
        forbidden = sorted(name for name in names if name in FORBIDDEN_GPU_MEMBERS)
        if forbidden:
            _fail(
                "onnxruntime CPU wheel contains forbidden GPU provider members",
                expected="no CUDA or TensorRT provider libraries",
                actual=", ".join(forbidden),
                repair="use the pinned CPU onnxruntime wheel, never onnxruntime-gpu",
            )

        runtime_data = _zip_member(zip_file, spec.runtime_member)
        runtime_sha = _sha256_bytes(runtime_data)
        if runtime_sha != spec.runtime_sha256:
            _fail(
                "extracted onnxruntime library digest mismatch",
                expected=spec.runtime_sha256,
                actual=runtime_sha,
                repair="restore the pinned wheel bytes or update the digest table through design review",
            )

        runtime_path = wheel_data_root / package_runtime_dir / spec.runtime_staged_name
        runtime_record = _write_file(runtime_path, runtime_data, LIB_MODE)
        _stage_link_dir(spec, link_dir, runtime_data)

        notice_records = []
        for notice in spec.notices:
            notice_data = _zip_member(zip_file, notice.source_member)
            notice_sha = _sha256_bytes(notice_data)
            if notice_sha != notice.sha256:
                _fail(
                    "extracted onnxruntime notice digest mismatch",
                    expected=f"{notice.source_member} sha256 {notice.sha256}",
                    actual=notice_sha,
                    repair="restore the pinned wheel bytes or update the notice digest through design review",
                )
            notice_path = wheel_data_root / package_notice_dir / notice.staged_name
            notice_record = _write_file(notice_path, notice_data, NOTICE_MODE)
            notice_record["source_member"] = notice.source_member
            notice_records.append(notice_record)

    receipt = {
        "schema": "solstone.speakers-analyze-runtime-provenance.v1",
        "package": package_dir.name,
        "target": spec.key,
        "wheel": {
            "url": spec.wheel_url,
            "sha256": spec.wheel_sha256,
            "path": _relative(wheel_path),
            "size": wheel_path.stat().st_size,
        },
        "runtime_library": {
            "source_member": spec.runtime_member,
            "sha256": spec.runtime_sha256,
            **runtime_record,
        },
        "notices": notice_records,
        "forbidden_gpu_members_checked": list(FORBIDDEN_GPU_MEMBERS),
        "wheel_data_root": _relative(wheel_data_root),
        "link_dir": _relative(link_dir),
    }

    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    return receipt


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Stage pinned CPU ONNX Runtime bytes for an ONNX-bundling helper wheel."
        )
    )
    parser.add_argument(
        "--target",
        choices=sorted(TARGETS),
        default=None,
        help="runtime target to stage; defaults to the current supported host",
    )
    parser.add_argument(
        "--package-dir",
        type=Path,
        default=PACKAGE_DIR,
        help=(
            "destination packaging leaf; its directory name selects the "
            "wheel-data install paths"
        ),
    )
    parser.add_argument(
        "--cache-dir",
        type=Path,
        default=DEFAULT_CACHE_DIR,
        help="verified wheel cache directory",
    )
    parser.add_argument(
        "--link-root",
        type=Path,
        default=DEFAULT_LINK_ROOT,
        help="root directory for target-specific build-time link helpers",
    )
    parser.add_argument(
        "--receipt",
        type=Path,
        default=None,
        help="JSON provenance receipt path",
    )
    parser.add_argument(
        "--offline",
        action="store_true",
        help="require the pinned wheel to already be present in the cache",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    target = args.target or _default_target()
    spec = TARGETS[target]
    receipt_path = args.receipt or (DEFAULT_RECEIPT_ROOT / f"{target}.json")
    try:
        receipt = stage_runtime(
            spec=spec,
            package_dir=args.package_dir,
            cache_dir=args.cache_dir,
            link_root=args.link_root,
            receipt_path=receipt_path,
            offline=args.offline,
        )
    except StageError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1
    except OSError as exc:
        print(f"ERROR: staging filesystem operation failed: {exc}", file=sys.stderr)
        return 1

    print(json.dumps(receipt, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
