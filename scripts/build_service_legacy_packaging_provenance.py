#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Build and capture journal-launcher wheel provenance once by hand.

This is a hand-run evidence-capture tool, not a CI dependency. It builds the
two wheels twice with a fixed SOURCE_DATE_EPOCH and refuses to write evidence
unless their content-addressed wheel facts are identical.
"""

from __future__ import annotations

import base64
import csv
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
import urllib.request
import zipfile
from email.parser import BytesParser
from email.policy import default
from pathlib import Path
from typing import Any

from service_legacy_paths import capture_input, evidence_root

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE_ROOT = evidence_root()
OUTPUT = EVIDENCE_ROOT / "packaging-provenance.json"
SCRATCH_ROOT = Path(
    os.environ.get("SERVICE_LEGACY_WHEEL_SCRATCH_ROOT", ROOT / "scratch")
).resolve()
SOURCE_DATE_EPOCH = "0"
SCHEMA = "service-legacy-packaging-provenance"
SCHEMA_VERSION = 1
JOURNAL_PACKAGE = "solstone-journal"
CORE_JOURNAL_PACKAGE = "solstone-core-journal"
LAUNCHER = ROOT / "packages/solstone-journal/scripts/journal"
DISPATCH_SOURCES = (
    ROOT / "core/crates/solstone-core-journal-cli/src/processes.rs",
    ROOT / "core/crates/solstone-core-journal-cli/src/runner.rs",
    ROOT / "core/crates/solstone-core-journal-cli/src/lib.rs",
)
PINNED_WHEELS = {
    "maturin": (
        "maturin-1.14.1-py3-none-manylinux_2_12_x86_64.manylinux2010_x86_64.musllinux_1_1_x86_64.whl",
        "https://files.pythonhosted.org/packages/1a/bd/9c0d5d6983905ce2c9edaa073a7e89355a9cf7f396988e05d32f1c37785d/maturin-1.14.1-py3-none-manylinux_2_12_x86_64.manylinux2010_x86_64.musllinux_1_1_x86_64.whl",
        "dfc54ae32e6fcb18302193ab9a30b0b25eefffba994ae13238974805533ef75e",
    ),
    "packaging": (
        "packaging-26.3-py3-none-any.whl",
        "https://files.pythonhosted.org/packages/63/34/ba1c580383c9eada3711951fef0795c80b829a078d72188184bcab9dd527/packaging-26.3-py3-none-any.whl",
        "d7193f7c8e4e93f444fde0262bf90af30e16fa0ad0ad44cb553c87339b23cd1c",
    ),
    "setuptools": (
        "setuptools-83.0.0-py3-none-any.whl",
        "https://files.pythonhosted.org/packages/5d/40/e1e72872c6354b306daef1703549e8e83b4d43cfea356311bf722a043752/setuptools-83.0.0-py3-none-any.whl",
        "29b23c360f22f414dc7336bb39178cc7bcbf6021ed2733cde173f09dba19abb3",
    ),
    "uv": (
        "uv-0.11.4-py3-none-manylinux_2_17_x86_64.manylinux2014_x86_64.whl",
        "https://files.pythonhosted.org/packages/45/f1/9c211df705e5414c631f836e45c6f98c4dc8ef6b9f4f704971483f08bb1c/uv-0.11.4-py3-none-manylinux_2_17_x86_64.manylinux2014_x86_64.whl",
        "a599dd5f563da775ca9ab9385a3d9b14f02de3c85c7ce7b41ec27996f03b137c",
    ),
    "wheel": (
        "wheel-0.47.0-py3-none-any.whl",
        "https://files.pythonhosted.org/packages/87/1b/9e33c09813d65e248f7f773119148a612516a4bea93e9c6f545f78455b7c/wheel-0.47.0-py3-none-any.whl",
        "212281cab4dff978f6cedd499cd893e1f620791ca6ff7107cf270781e587eced",
    ),
}


class ProvenanceError(RuntimeError):
    """The wheel build or its launcher-chain evidence is invalid."""


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def relative(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def command_output(command: list[str]) -> str:
    executable = command[0]
    if shutil.which(executable) is None:
        raise ProvenanceError(
            f"required build tool is unavailable on PATH: {executable}"
        )
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
    if result.returncode:
        detail = (
            result.stderr.strip() or result.stdout.strip() or "no diagnostic output"
        )
        raise ProvenanceError(f"{' '.join(command)} failed: {detail}")
    return result.stdout.strip()


def copy_file(source: str, destination: str) -> str:
    try:
        os.link(source, destination)
    except OSError:
        shutil.copy2(source, destination)
    return destination


def _validate_source_tree(source: Path, *, allow_external_symlinks: bool) -> None:
    resolved_root = source.resolve()
    for path in source.rglob("*"):
        if (
            path.is_symlink()
            and not allow_external_symlinks
            and not path.resolve().is_relative_to(resolved_root)
        ):
            raise ProvenanceError(f"source tree has escaping symlink: {path}")


def copy_tree(
    source: Path, destination: Path, *, allow_external_symlinks: bool = False
) -> None:
    _validate_source_tree(source, allow_external_symlinks=allow_external_symlinks)
    shutil.copytree(source, destination, copy_function=copy_file, symlinks=False)


def tree_fact(root: Path) -> dict[str, Any]:
    digest = hashlib.sha256()
    count = 0
    size = 0
    for path in sorted(root.rglob("*")):
        relative_path = path.relative_to(root).as_posix()
        metadata = path.lstat()
        mode = metadata.st_mode & 0o7777
        if path.is_symlink():
            raise ProvenanceError(f"bundle fact contains a symlink: {path}")
        if path.is_dir():
            digest.update(b"D\0" + relative_path.encode("utf-8") + b"\0")
            digest.update(f"{mode:o}\n".encode("ascii"))
            continue
        if not path.is_file():
            raise ProvenanceError(f"bundle fact contains a special file: {path}")
        contents_digest = sha256_file(path)
        length = metadata.st_size
        digest.update(b"F\0" + relative_path.encode("utf-8") + b"\0")
        digest.update(f"{mode:o}\0".encode("ascii"))
        digest.update(str(length).encode("ascii") + b"\0")
        digest.update(contents_digest.encode("ascii") + b"\n")
        count += 1
        size += length
    return {"files": count, "sha256": digest.hexdigest(), "size": size}


def download_exact(destination: Path, url: str, expected_sha256: str) -> None:
    request = urllib.request.Request(
        url, headers={"User-Agent": "service-legacy-evidence/1"}
    )
    with urllib.request.urlopen(request) as response, destination.open("wb") as output:
        shutil.copyfileobj(response, output)
    if sha256_file(destination) != expected_sha256:
        destination.unlink(missing_ok=True)
        raise ProvenanceError(f"download digest differs for {destination.name}")


def extract_wheel(wheel_path: Path, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(wheel_path) as wheel:
        for member in wheel.infolist():
            if member.is_dir():
                continue
            name = member.filename
            parts = name.split("/", 2)
            if (
                len(parts) >= 3
                and parts[0].endswith(".data")
                and parts[1]
                in {
                    "purelib",
                    "platlib",
                }
            ):
                name = parts[2]
            elif (
                len(parts) >= 3 and parts[0].endswith(".data") and parts[1] == "scripts"
            ):
                name = f"bin/{parts[2]}"
            elif len(parts) >= 2 and parts[0].endswith(".data"):
                continue
            target = destination / name
            if not target.resolve().is_relative_to(destination.resolve()):
                raise ProvenanceError(
                    f"wheel member escapes destination: {member.filename}"
                )
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(wheel.read(member))
            permissions = (member.external_attr >> 16) & 0o777
            target.chmod(permissions or (0o755 if name.startswith("bin/") else 0o644))


def selected_rust_tools() -> tuple[str, Path, Path, Path]:
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"))[
        "toolchain"
    ]["channel"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", channel):
        raise ProvenanceError("Rust toolchain channel is not an exact release")
    rustup = Path(os.environ.get("SERVICE_LEGACY_RUSTUP", shutil.which("rustup") or ""))
    if not rustup.is_file():
        raise ProvenanceError("rustup provisioning tool is unavailable")

    def selected(name: str) -> Path:
        rustup_home = Path(
            os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
        ).resolve()
        result = subprocess.run(
            [str(rustup), "which", "--toolchain", channel, name],
            cwd=ROOT,
            env={
                "HOME": str(Path.home()),
                "PATH": "/usr/bin:/bin",
                "RUSTUP_HOME": str(rustup_home),
            },
            text=True,
            capture_output=True,
            check=False,
        )
        if result.returncode:
            raise ProvenanceError(
                result.stderr.strip() or f"rustup cannot select {name}"
            )
        path = Path(result.stdout.strip()).resolve()
        if not path.is_file():
            raise ProvenanceError(f"selected Rust tool is unavailable: {path}")
        return path

    return channel, rustup.resolve(), selected("cargo"), selected("rustc")


def vendor_cargo_closure(cargo: Path, destination: Path, root: Path) -> None:
    source_cargo_home = Path(
        os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))
    ).resolve()
    if (source_cargo_home / "config").exists() or (
        source_cargo_home / "config.toml"
    ).exists():
        raise ProvenanceError("Cargo provisioning home contains caller configuration")
    environment = {
        "CARGO_HOME": str(source_cargo_home),
        "CARGO_NET_OFFLINE": "true",
        "HOME": str(root / "cargo-provision-home"),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "PATH": "/usr/bin:/bin",
        "RUSTC": str(cargo.parent / "rustc"),
        "TMPDIR": str(root / "cargo-provision-tmp"),
    }
    Path(environment["HOME"]).mkdir()
    Path(environment["TMPDIR"]).mkdir()
    result = subprocess.run(
        [
            str(cargo),
            "vendor",
            "--manifest-path",
            str(ROOT / "core/Cargo.toml"),
            "--locked",
            "--offline",
            "--versioned-dirs",
            str(destination),
        ],
        cwd=ROOT,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        raise ProvenanceError(result.stderr.strip() or "cargo vendor failed")
    config = result.stdout.replace(str(destination), "vendor")
    if "[source.crates-io]" not in config or 'directory = "vendor"' not in config:
        raise ProvenanceError(
            "cargo vendor did not emit the closed source configuration"
        )
    (destination.parent / "config.toml").write_text(
        "[net]\noffline = true\n" + config, encoding="utf-8"
    )


def provision_bundle(root: Path) -> tuple[Path, dict[str, Any]]:
    bundle = root / "bundle"
    bundle.mkdir()
    bin_root = bundle / "bin"
    bin_root.mkdir()
    channel, rustup_source, cargo_source, rustc_source = selected_rust_tools()

    host_tools: dict[str, dict[str, str]] = {}
    for name, source_name in (
        ("cc", "/usr/bin/cc"),
        ("ld", "/usr/bin/ld"),
        ("as", "/usr/bin/as"),
        ("ar", "/usr/bin/ar"),
        ("strip", "/usr/bin/strip"),
        ("sh", "/bin/sh"),
    ):
        source = Path(source_name).resolve()
        if not source.is_file():
            raise ProvenanceError(f"selected host tool is unavailable: {source_name}")
        shutil.copy2(source, bin_root / name)
        host_tools[name] = {
            "selected_path": source_name,
            "sha256": sha256_file(source),
        }
    compiler_helper = Path(
        subprocess.run(
            ["/usr/bin/cc", "-print-file-name=liblto_plugin.so"],
            check=True,
            text=True,
            capture_output=True,
        ).stdout.strip()
    ).resolve()
    if not compiler_helper.is_file():
        raise ProvenanceError("selected host compiler helper is unavailable")
    copy_tree(
        compiler_helper.parent,
        bundle / "compiler-helpers",
        allow_external_symlinks=True,
    )
    compiler_runtime = Path(
        subprocess.run(
            ["/usr/bin/cc", "-print-libgcc-file-name"],
            check=True,
            text=True,
            capture_output=True,
        ).stdout.strip()
    ).resolve()
    if not compiler_runtime.is_file():
        raise ProvenanceError("selected host compiler runtime is unavailable")
    copy_tree(
        compiler_runtime.parent,
        bundle / "compiler-runtime",
        allow_external_symlinks=True,
    )

    sysroot = Path(
        subprocess.run(
            [str(rustc_source), "--print", "sysroot"],
            check=True,
            text=True,
            capture_output=True,
        ).stdout.strip()
    ).resolve()
    toolchain_name = sysroot.name
    if not toolchain_name.startswith(channel):
        raise ProvenanceError(
            f"selected Rust sysroot {toolchain_name!r} differs from {channel!r}"
        )
    materialized_toolchain = bundle / "rustup-home/toolchains" / toolchain_name
    materialized_toolchain.parent.mkdir(parents=True)
    copy_tree(sysroot, materialized_toolchain)

    cargo_home = bundle / "cargo-home"
    vendor = cargo_home / "vendor"
    cargo_home.mkdir()
    vendor_cargo_closure(cargo_source, vendor, root)

    backend = bundle / "python"
    wheelhouse = bundle / "backend-wheels"
    wheelhouse.mkdir()
    archive_facts: dict[str, dict[str, str]] = {}
    for name, (filename, url, expected_sha256) in sorted(PINNED_WHEELS.items()):
        archive = wheelhouse / filename
        download_exact(archive, url, expected_sha256)
        archive_facts[name] = {
            "filename": filename,
            "sha256": expected_sha256,
            "url": url,
        }
        if name == "uv":
            extracted = root / "uv-wheel"
            extract_wheel(archive, extracted)
            shutil.copy2(extracted / "bin/uv", bin_root / "uv")
        else:
            extract_wheel(archive, backend)

    python = Path("/usr/bin/python3").resolve()
    if not python.is_file():
        raise ProvenanceError("pinned host Python is unavailable")
    rustc_vv = subprocess.run(
        [str(materialized_toolchain / "bin/rustc"), "-vV"],
        check=True,
        text=True,
        capture_output=True,
    ).stdout.strip()
    host_lines = [
        line.removeprefix("host: ")
        for line in rustc_vv.splitlines()
        if line.startswith("host: ")
    ]
    if len(host_lines) != 1:
        raise ProvenanceError("rustc -vV lacks one host target")
    release_lines = [
        line.removeprefix("release: ")
        for line in rustc_vv.splitlines()
        if line.startswith("release: ")
    ]
    if release_lines != [channel]:
        raise ProvenanceError(
            f"selected rustc release {release_lines!r} differs from {channel!r}"
        )
    facts = {
        "backend": tree_fact(backend),
        "backend_archives": archive_facts,
        "backend_wheels": tree_fact(wheelhouse),
        "cargo_vendor": tree_fact(vendor),
        "compiler_helpers": tree_fact(bundle / "compiler-helpers"),
        "compiler_runtime": tree_fact(bundle / "compiler-runtime"),
        "cargo_version": subprocess.run(
            [str(materialized_toolchain / "bin/cargo"), "--version"],
            check=True,
            text=True,
            capture_output=True,
        ).stdout.strip(),
        "host_tools": host_tools,
        "linker": {
            "sha256": sha256_file(bin_root / "cc"),
            "version": subprocess.run(
                [str(bin_root / "cc"), "--version"],
                check=True,
                text=True,
                capture_output=True,
            ).stdout.splitlines()[0],
        },
        "maturin_version": subprocess.run(
            [str(backend / "bin/maturin"), "--version"],
            check=True,
            text=True,
            capture_output=True,
        ).stdout.strip(),
        "host_target": host_lines[0],
        "python": {
            "path": "/usr/bin/python3",
            "sha256": sha256_file(python),
            "version": subprocess.run(
                [str(python), "--version"],
                check=True,
                text=True,
                capture_output=True,
            ).stdout.strip(),
        },
        "rustc_vv": rustc_vv,
        "rustup": {
            "sha256": sha256_file(rustup_source),
            "version": subprocess.run(
                [str(rustup_source), "--version"],
                check=True,
                text=True,
                capture_output=True,
            ).stdout.splitlines()[0],
        },
        "rust_toolchain": tree_fact(materialized_toolchain),
        "toolchain_name": toolchain_name,
        "uv": {
            "sha256": sha256_file(bin_root / "uv"),
            "version": subprocess.run(
                [str(bin_root / "uv"), "--version"],
                check=True,
                text=True,
                capture_output=True,
            ).stdout.strip(),
        },
    }
    facts["bundle_inventory"] = tree_fact(bundle)
    return bundle, facts


def materialize_bundle(bundle: Path, destination: Path) -> dict[str, Path]:
    destination.mkdir()
    for name in (
        "bin",
        "cargo-home",
        "compiler-helpers",
        "compiler-runtime",
        "python",
        "rustup-home",
    ):
        copy_tree(bundle / name, destination / name)
    roots = {
        "cargo_home": destination / "cargo-home",
        "home": destination / "home",
        "python": destination / "python",
        "rustup_home": destination / "rustup-home",
        "target": destination / "target",
        "tmp": destination / "tmp",
        "uv_cache": destination / "uv-cache",
        "xdg_config": destination / "xdg-config",
    }
    for name, path in roots.items():
        if name not in {"cargo_home", "python", "rustup_home"}:
            path.mkdir()
    config = roots["cargo_home"] / "config.toml"
    config_text = config.read_text(encoding="utf-8")
    marker = 'directory = "vendor"'
    if config_text.count(marker) != 1:
        raise ProvenanceError("bundle Cargo config lacks one canonical vendor role")
    config.unlink()
    config.write_text(
        config_text.replace(
            marker,
            f"directory = {json.dumps(str(roots['cargo_home'] / 'vendor'))}",
        ),
        encoding="utf-8",
    )
    return roots


def validate_role_bindings(run_root: Path, roots: dict[str, Path]) -> None:
    expected = {
        "cargo_home",
        "home",
        "python",
        "rustup_home",
        "target",
        "tmp",
        "uv_cache",
        "xdg_config",
    }
    if set(roots) != expected:
        raise ProvenanceError("wheel-build role bindings are incomplete")
    resolved_run_root = run_root.resolve()
    resolved = {name: path.resolve() for name, path in roots.items()}
    if len(set(resolved.values())) != len(resolved):
        raise ProvenanceError("wheel-build role bindings are not distinct")
    for name, path in resolved.items():
        if not path.is_relative_to(resolved_run_root):
            raise ProvenanceError(f"wheel-build role {name} escapes its run root")
        if not path.is_dir():
            raise ProvenanceError(f"wheel-build role {name} is not materialized")


def wheel_environment(
    run_root: Path, roots: dict[str, Path], toolchain_name: str, host_target: str
) -> dict[str, str]:
    validate_role_bindings(run_root, roots)
    toolchain = roots["rustup_home"] / "toolchains" / toolchain_name
    if (
        not (toolchain / "bin/cargo").is_file()
        or not (toolchain / "bin/rustc").is_file()
    ):
        raise ProvenanceError("materialized Rust toolchain is incomplete")
    for name in ("ar", "cc", "ld", "sh", "strip", "uv"):
        if not (run_root / "bin" / name).is_file():
            raise ProvenanceError(f"materialized host tool is absent: {name}")
    return {
        "AR": str(run_root / "bin/ar"),
        "CARGO": str(toolchain / "bin/cargo"),
        "CARGO_BUILD_TARGET": host_target,
        "CARGO_HOME": str(roots["cargo_home"]),
        "CARGO_INCREMENTAL": "0",
        "CARGO_NET_OFFLINE": "true",
        "CARGO_TARGET_DIR": str(roots["target"]),
        "C_INCLUDE_PATH": str(run_root / "compiler-runtime/include"),
        "CC": str(run_root / "bin/cc"),
        "COMPILER_PATH": str(run_root / "compiler-helpers"),
        "HOME": str(roots["home"]),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "LD": str(run_root / "bin/ld"),
        "LIBRARY_PATH": str(run_root / "compiler-runtime"),
        "PATH": (f"{run_root / 'bin'}:{roots['python'] / 'bin'}:{toolchain / 'bin'}"),
        "PYTHONPATH": str(roots["python"]),
        "RUSTC": str(toolchain / "bin/rustc"),
        "RUSTUP_HOME": str(roots["rustup_home"]),
        "RUSTFLAGS": (
            f"--remap-path-prefix={ROOT}=<SOURCE_ROOT> "
            f"--remap-path-prefix={run_root}=<BUILD_ROOT>"
        ),
        "SOURCE_DATE_EPOCH": SOURCE_DATE_EPOCH,
        "STRIP": str(run_root / "bin/strip"),
        "TMPDIR": str(roots["tmp"]),
        "TZ": "UTC",
        "UV_CACHE_DIR": str(roots["uv_cache"]),
        "UV_LINK_MODE": "copy",
        "UV_NO_BUILD_ISOLATION": "1",
        "UV_NO_CONFIG": "1",
        "UV_NO_PROGRESS": "1",
        "UV_OFFLINE": "1",
        "UV_PYTHON_DOWNLOADS": "never",
        "XDG_CONFIG_HOME": str(roots["xdg_config"]),
    }


def build_wheel(
    package: str,
    out_dir: Path,
    *,
    run_root: Path,
    roots: dict[str, Path],
    toolchain_name: str,
    host_target: str,
) -> Path:
    env = wheel_environment(run_root, roots, toolchain_name, host_target)
    command = [
        str(run_root / "bin/uv"),
        "build",
        "--package",
        package,
        "--wheel",
        "--out-dir",
        str(out_dir),
        "--no-build-isolation",
        "--offline",
        "--no-config",
        "--no-progress",
        "--python",
        "/usr/bin/python3",
    ]
    result = subprocess.run(
        command,
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
    )
    if result.returncode:
        detail = (
            result.stderr.strip() or result.stdout.strip() or "no diagnostic output"
        )
        raise ProvenanceError(f"uv build for {package} failed: {detail}")
    stem = package.replace("-", "_")
    wheels = sorted(out_dir.glob(f"{stem}-*.whl"))
    if len(wheels) != 1:
        raise ProvenanceError(
            f"expected one {package} wheel, found {len(wheels)} in {out_dir}"
        )
    return wheels[0]


def record_entry(
    rows: dict[str, tuple[str, str]], member: str, contents: bytes
) -> dict[str, Any]:
    try:
        encoded, size = rows[member]
    except KeyError as error:
        raise ProvenanceError(f"wheel RECORD is missing {member}") from error
    if not encoded.startswith("sha256="):
        raise ProvenanceError(f"wheel RECORD does not sha256-protect {member}")
    digest = encoded.removeprefix("sha256=")
    expected = (
        base64.urlsafe_b64encode(hashlib.sha256(contents).digest()).decode().rstrip("=")
    )
    if digest != expected or size != str(len(contents)):
        raise ProvenanceError(f"wheel RECORD hash/size mismatch for {member}")
    return {
        "content_sha256": sha256_bytes(contents),
        "path": member,
        "record_sha256": digest,
        "record_size": int(size),
    }


def record_file_fact(member: str, contents: bytes) -> dict[str, str]:
    """RECORD itself is deliberately unhashed in its own final CSV row."""
    return {"content_sha256": sha256_bytes(contents), "path": member}


def wheel_contents(
    path: Path,
) -> tuple[zipfile.ZipFile, list[str], dict[str, tuple[str, str]]]:
    wheel = zipfile.ZipFile(path)
    names = wheel.namelist()
    record_paths = [name for name in names if name.endswith(".dist-info/RECORD")]
    if len(record_paths) != 1:
        wheel.close()
        raise ProvenanceError(f"expected one RECORD in {path.name}")
    rows = {
        row[0]: (row[1], row[2])
        for row in csv.reader(wheel.read(record_paths[0]).decode("utf-8").splitlines())
    }
    if set(rows) != set(names):
        wheel.close()
        raise ProvenanceError(
            f"RECORD inventory differs from wheel members: {path.name}"
        )
    return wheel, names, rows


def only_member(names: list[str], suffix: str, label: str) -> str:
    matches = [name for name in names if name.endswith(suffix)]
    if len(matches) != 1:
        raise ProvenanceError(
            f"expected one {label} member ending {suffix}, found {len(matches)}"
        )
    return matches[0]


def metadata_fact(
    wheel: zipfile.ZipFile, names: list[str], rows: dict[str, tuple[str, str]]
) -> dict[str, Any]:
    member = only_member(names, ".dist-info/METADATA", "METADATA")
    contents = wheel.read(member)
    message = BytesParser(policy=default).parsebytes(contents)
    name = message["Name"]
    version = message["Version"]
    if name is None or version is None:
        raise ProvenanceError("wheel METADATA is missing Name or Version")
    return {
        "name": str(name),
        "record": record_entry(rows, member, contents),
        "requires_dist": [str(item) for item in message.get_all("Requires-Dist", [])],
        "version": str(version),
    }


def journal_wheel_fact(path: Path) -> dict[str, Any]:
    wheel, names, rows = wheel_contents(path)
    try:
        metadata = metadata_fact(wheel, names, rows)
        if metadata["name"] != JOURNAL_PACKAGE:
            raise ProvenanceError(f"journal wheel METADATA names {metadata['name']!r}")
        dependencies = [
            item
            for item in metadata["requires_dist"]
            if item.startswith(CORE_JOURNAL_PACKAGE)
        ]
        if not dependencies:
            raise ProvenanceError(
                "journal METADATA does not select solstone-core-journal"
            )
        launcher_member = only_member(
            names, ".data/scripts/journal", "journal script-files launcher"
        )
        launcher = wheel.read(launcher_member)
        source = LAUNCHER.read_bytes()
        if launcher != source:
            raise ProvenanceError(
                "wheel journal script-files launcher differs from its source script"
            )
        entry_points_member = only_member(
            names, ".dist-info/entry_points.txt", "entry-points"
        )
        entry_points = wheel.read(entry_points_member)
        if (
            b"mlx-vlm-server = solstone.think.providers.mlx_server:main"
            not in entry_points
        ):
            raise ProvenanceError("journal entry_points.txt lacks mlx-vlm-server")
        return {
            "filename": path.name,
            "metadata": metadata,
            "project_script": {
                "entry_points": record_entry(rows, entry_points_member, entry_points),
                "mechanism": "project.scripts",
                "name": "mlx-vlm-server",
                "part_of_journal_launcher_chain": False,
                "target": "solstone.think.providers.mlx_server:main",
            },
            "record": record_file_fact(
                only_member(names, ".dist-info/RECORD", "RECORD"),
                wheel.read(only_member(names, ".dist-info/RECORD", "RECORD")),
            ),
            "script_files_launcher": {
                "mechanism": "script-files",
                **record_entry(rows, launcher_member, launcher),
            },
            "sha256": sha256_file(path),
        }
    finally:
        wheel.close()


def core_journal_wheel_fact(path: Path) -> dict[str, Any]:
    wheel, names, rows = wheel_contents(path)
    try:
        metadata = metadata_fact(wheel, names, rows)
        if metadata["name"] != CORE_JOURNAL_PACKAGE:
            raise ProvenanceError(
                f"core journal wheel METADATA names {metadata['name']!r}"
            )
        binary_member = only_member(
            names,
            ".data/scripts/solstone-core-journal",
            "solstone-core-journal native executable",
        )
        return {
            "binary": record_entry(rows, binary_member, wheel.read(binary_member)),
            "filename": path.name,
            "metadata": metadata,
            "record": record_file_fact(
                only_member(names, ".dist-info/RECORD", "RECORD"),
                wheel.read(only_member(names, ".dist-info/RECORD", "RECORD")),
            ),
            "sha256": sha256_file(path),
        }
    finally:
        wheel.close()


def build_once(bundle: Path, facts: dict[str, Any], ordinal: int) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(
        prefix=f"service-legacy-wheel-{ordinal}-", dir=SCRATCH_ROOT
    ) as temporary:
        run_root = Path(temporary) / "run"
        roots = materialize_bundle(bundle, run_root)
        out_dir = Path(temporary) / "dist"
        out_dir.mkdir()
        journal_path = build_wheel(
            JOURNAL_PACKAGE,
            out_dir,
            run_root=run_root,
            roots=roots,
            toolchain_name=facts["toolchain_name"],
            host_target=facts["host_target"],
        )
        journal = journal_wheel_fact(journal_path)
        core_path = build_wheel(
            CORE_JOURNAL_PACKAGE,
            out_dir,
            run_root=run_root,
            roots=roots,
            toolchain_name=facts["toolchain_name"],
            host_target=facts["host_target"],
        )
        core = core_journal_wheel_fact(core_path)
    return {"solstone_core_journal": core, "solstone_journal": journal}


def dispatch_sources() -> list[dict[str, str]]:
    facts = []
    for path in DISPATCH_SOURCES:
        if not path.is_file():
            raise ProvenanceError(f"native dispatch source is missing: {path}")
        facts.append({"path": relative(path), "sha256": sha256_file(path)})
    return facts


def verify_native_dispatch_sources() -> None:
    processes = DISPATCH_SOURCES[0].read_text(encoding="utf-8")
    runner = DISPATCH_SOURCES[1].read_text(encoding="utf-8")
    for token in ('token: "service"', 'token: "up"', 'token: "down"'):
        if token not in processes:
            raise ProvenanceError(f"native process table lacks {token}")
    if 'module: "solstone.think.service"' not in processes:
        raise ProvenanceError(
            "native process table does not select solstone.think.service"
        )
    if "importlib.import_module(module).main()" not in runner:
        raise ProvenanceError(
            "native runner does not invoke the selected module main()"
        )


def payload(
    wheels: dict[str, Any], bundle_facts: dict[str, Any], source_commit: str
) -> dict[str, Any]:
    journal = wheels["solstone_journal"]
    core = wheels["solstone_core_journal"]
    verify_native_dispatch_sources()
    return {
        "build": {
            "bundle": {
                key: value
                for key, value in bundle_facts.items()
                if key not in {"cargo_version", "rustc_vv"}
            },
            "environment": {
                "AR": "<BUNDLE_BIN>/ar",
                "CARGO": "<RUST_TOOLCHAIN_BIN>/cargo",
                "CARGO_BUILD_TARGET": bundle_facts["host_target"],
                "CARGO_HOME": "<CARGO_HOME>",
                "CARGO_INCREMENTAL": "0",
                "CARGO_NET_OFFLINE": "true",
                "CARGO_TARGET_DIR": "<TARGET>",
                "C_INCLUDE_PATH": "<COMPILER_RUNTIME>/include",
                "CC": "<BUNDLE_BIN>/cc",
                "COMPILER_PATH": "<COMPILER_HELPERS>",
                "HOME": "<HOME>",
                "LANG": "C.UTF-8",
                "LC_ALL": "C.UTF-8",
                "LD": "<BUNDLE_BIN>/ld",
                "LIBRARY_PATH": "<COMPILER_RUNTIME>",
                "PATH": ("<BUNDLE_BIN>:<PYTHON_BACKENDS_BIN>:<RUST_TOOLCHAIN_BIN>"),
                "PYTHONPATH": "<PYTHON_BACKENDS>",
                "RUSTC": "<RUST_TOOLCHAIN_BIN>/rustc",
                "RUSTUP_HOME": "<RUSTUP_HOME>",
                "RUSTFLAGS": (
                    "--remap-path-prefix=<SOURCE_CHECKOUT>=<SOURCE_ROOT> "
                    "--remap-path-prefix=<RUN_ROOT>=<BUILD_ROOT>"
                ),
                "SOURCE_DATE_EPOCH": SOURCE_DATE_EPOCH,
                "STRIP": "<BUNDLE_BIN>/strip",
                "TMPDIR": "<TMPDIR>",
                "TZ": "UTC",
                "UV_CACHE_DIR": "<UV_CACHE>",
                "UV_LINK_MODE": "copy",
                "UV_NO_BUILD_ISOLATION": "1",
                "UV_NO_CONFIG": "1",
                "UV_NO_PROGRESS": "1",
                "UV_OFFLINE": "1",
                "UV_PYTHON_DOWNLOADS": "never",
                "XDG_CONFIG_HOME": "<XDG_CONFIG>",
            },
            "wheel_commands": [
                [
                    "<BUNDLE_BIN>/uv",
                    "build",
                    "--package",
                    package,
                    "--wheel",
                    "--out-dir",
                    "<OUTPUT>",
                    "--no-build-isolation",
                    "--offline",
                    "--no-config",
                    "--no-progress",
                    "--python",
                    "/usr/bin/python3",
                ]
                for package in (JOURNAL_PACKAGE, CORE_JOURNAL_PACKAGE)
            ],
            "host": {
                "architecture": platform.machine(),
                "kernel": platform.release(),
                "libc": list(platform.libc_ver()),
                "system": platform.system(),
            },
            "source_date_epoch": SOURCE_DATE_EPOCH,
            "tools": {
                "cargo": bundle_facts["cargo_version"],
                "maturin": bundle_facts["maturin_version"],
                "python": bundle_facts["python"],
                "rustc": bundle_facts["rustc_vv"],
                "rustup": bundle_facts["rustup"],
                "uv": bundle_facts["uv"]["version"],
            },
        },
        "launcher_chain": {
            "journal_launcher": {
                "mechanism": "script-files",
                "source_path": relative(LAUNCHER),
                "source_sha256": sha256_file(LAUNCHER),
                "wheel": journal["script_files_launcher"],
            },
            "native_binary": {
                "name": "solstone-core-journal",
                "wheel": core["binary"],
            },
            "native_dispatch": {
                "process_tokens": ["service", "up", "down"],
                "python_module": "solstone.think.service",
            },
            "native_dispatch_sources": dispatch_sources(),
            "sibling_binary": "solstone-core-journal",
        },
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "source": {"commit": source_commit},
        "wheels": wheels,
    }


def write_payload(value: dict[str, Any]) -> None:
    temporary = OUTPUT.with_suffix(".json.tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    os.replace(temporary, OUTPUT)


def mismatch_paths(left: Any, right: Any, path: str = "$") -> list[str]:
    if type(left) is not type(right):
        return [f"{path}: {left!r} != {right!r}"]
    if isinstance(left, dict):
        findings: list[str] = []
        for key in sorted(set(left) | set(right)):
            if key not in left or key not in right:
                findings.append(f"{path}.{key}: key presence differs")
            else:
                findings.extend(mismatch_paths(left[key], right[key], f"{path}.{key}"))
        return findings
    if isinstance(left, list):
        if len(left) != len(right):
            return [f"{path}: lengths {len(left)} != {len(right)}"]
        findings = []
        for index, (left_item, right_item) in enumerate(zip(left, right, strict=True)):
            findings.extend(mismatch_paths(left_item, right_item, f"{path}[{index}]"))
        return findings
    if left != right:
        return [f"{path}: {left!r} != {right!r}"]
    return []


def assert_environment_exact(actual: dict[str, str], expected: dict[str, str]) -> None:
    if actual != expected:
        details = mismatch_paths(actual, expected)
        raise ProvenanceError("wheel-build environment differs:\n" + "\n".join(details))


def canonical_environment(
    environment: dict[str, str],
    run_root: Path,
    roots: dict[str, Path],
    toolchain_name: str,
) -> dict[str, str]:
    replacements = {
        str(roots["cargo_home"]): "<CARGO_HOME>",
        str(roots["home"]): "<HOME>",
        str(roots["python"]): "<PYTHON_BACKENDS>",
        str(roots["rustup_home"]): "<RUSTUP_HOME>",
        str(roots["target"]): "<TARGET>",
        str(roots["tmp"]): "<TMPDIR>",
        str(roots["uv_cache"]): "<UV_CACHE>",
        str(roots["xdg_config"]): "<XDG_CONFIG>",
        str(run_root / "compiler-helpers"): "<COMPILER_HELPERS>",
        str(run_root / "compiler-runtime"): "<COMPILER_RUNTIME>",
        str(run_root / "bin"): "<BUNDLE_BIN>",
        str(
            roots["rustup_home"] / "toolchains" / toolchain_name / "bin"
        ): "<RUST_TOOLCHAIN_BIN>",
        str(run_root): "<RUN_ROOT>",
        str(ROOT): "<SOURCE_CHECKOUT>",
    }
    result = {}
    for key, value in environment.items():
        canonical = value
        for source, destination in sorted(
            replacements.items(), key=lambda item: len(item[0]), reverse=True
        ):
            canonical = canonical.replace(source, destination)
        result[key] = canonical
    return result


def environment_self_test() -> None:
    SCRATCH_ROOT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix="service-legacy-wheel-environment-", dir=SCRATCH_ROOT
    ) as temporary:
        root = Path(temporary)
        bundle = root / "bundle"
        for name in (
            "bin",
            "cargo-home/vendor/demo",
            "compiler-helpers",
            "compiler-runtime/include",
            "python/bin",
            "rustup-home/toolchains/test/bin",
        ):
            (bundle / name).mkdir(parents=True, exist_ok=True)
        for name in (
            "bin/ar",
            "bin/cc",
            "bin/ld",
            "bin/sh",
            "bin/strip",
            "bin/uv",
            "cargo-home/vendor/demo/source",
            "compiler-helpers/liblto_plugin.so",
            "compiler-runtime/crtbeginS.o",
            "compiler-runtime/include/stdarg.h",
            "python/backend",
            "rustup-home/toolchains/test/bin/cargo",
            "rustup-home/toolchains/test/bin/rustc",
        ):
            (bundle / name).write_bytes(name.encode("utf-8"))
        (bundle / "cargo-home/config.toml").write_text(
            "[net]\noffline = true\n[source.crates-io]\n"
            'replace-with = "service-legacy-vendor"\n'
            "[source.service-legacy-vendor]\n"
            'directory = "vendor"\n',
            encoding="utf-8",
        )
        original_fact = tree_fact(bundle)
        roots_one = materialize_bundle(bundle, root / "one")
        roots_two = materialize_bundle(bundle, root / "two")
        if tree_fact(bundle) != original_fact:
            raise ProvenanceError("materialization mutated its immutable source bundle")
        first = wheel_environment(
            root / "one", roots_one, "test", "x86_64-unknown-linux-gnu"
        )
        second = wheel_environment(
            root / "two", roots_two, "test", "x86_64-unknown-linux-gnu"
        )
        if canonical_environment(
            first, root / "one", roots_one, "test"
        ) != canonical_environment(second, root / "two", roots_two, "test"):
            raise ProvenanceError(
                "canonical wheel environment depends on binding roots"
            )

        caller = dict(os.environ)
        poisons = {
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": "/poison/linker",
            "HTTPS_PROXY": "http://poison.invalid",
            "MATURIN_CONFIG": "/poison/maturin.toml",
            "RUSTC_WRAPPER": "/poison/wrapper",
            "RUSTFLAGS": "--cfg poison",
            "UV_INDEX_URL": "https://poison.invalid/simple",
        }
        try:
            os.environ.update(poisons)
            if any(os.environ.get(key) != value for key, value in poisons.items()):
                raise ProvenanceError("environment poison positive control is inert")
            isolated = wheel_environment(
                root / "one", roots_one, "test", "x86_64-unknown-linux-gnu"
            )
        finally:
            os.environ.clear()
            os.environ.update(caller)
        for key, poison in poisons.items():
            if isolated.get(key) == poison:
                raise ProvenanceError(f"caller poison entered wheel environment: {key}")

        mutated = dict(first)
        mutated["UV_OFFLINE"] = "0"
        try:
            assert_environment_exact(mutated, first)
        except ProvenanceError:
            pass
        else:
            raise ProvenanceError("offline-environment mutation was accepted")

        escaped = dict(roots_one)
        escaped["tmp"] = root.parent
        try:
            validate_role_bindings(root / "one", escaped)
        except ProvenanceError:
            pass
        else:
            raise ProvenanceError("escaped role binding was accepted")

        digest_before = tree_fact(bundle)
        (bundle / "python/backend").write_bytes(b"mutated")
        if tree_fact(bundle) == digest_before:
            raise ProvenanceError("bundle-content mutation did not change its digest")
    print("service legacy wheel environment self-test passed")


def main() -> int:
    if sys.argv[1:] == ["--self-test"]:
        environment_self_test()
        return 0
    if sys.argv[1:]:
        raise ProvenanceError(f"unexpected arguments: {sys.argv[1:]}")
    source_commit = capture_input()
    SCRATCH_ROOT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix="service-legacy-bundle-", dir=SCRATCH_ROOT
    ) as temporary:
        bundle, facts = provision_bundle(Path(temporary))
        first = payload(build_once(bundle, facts, 1), facts, source_commit)
        second = payload(build_once(bundle, facts, 2), facts, source_commit)
        if first != second:
            details = mismatch_paths(first, second)
            raise ProvenanceError(
                "two closed-environment wheel builds produced different provenance facts:\n"
                + "\n".join(details[:40])
            )
        write_payload(first)
    print("wrote deterministic journal packaging provenance for two wheels")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
