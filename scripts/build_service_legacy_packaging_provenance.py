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
import shutil
import subprocess
import tempfile
import zipfile
from email.parser import BytesParser
from email.policy import default
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE_ROOT = ROOT / "core/fixtures/service_legacy_evidence"
OUTPUT = EVIDENCE_ROOT / "packaging-provenance.json"
SCRATCH_ROOT = ROOT / "scratch"
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
        raise ProvenanceError(f"required build tool is unavailable on PATH: {executable}")
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic output"
        raise ProvenanceError(f"{' '.join(command)} failed: {detail}")
    return result.stdout.strip()


def git_head() -> str:
    return command_output(["git", "rev-parse", "HEAD"])


def tool_versions() -> dict[str, str]:
    return {
        "cargo": command_output(["cargo", "--version"]),
        "maturin": command_output(["maturin", "--version"]),
        "uv": command_output(["uv", "--version"]),
    }


def build_wheel(package: str, out_dir: Path) -> Path:
    env = os.environ.copy()
    env["SOURCE_DATE_EPOCH"] = SOURCE_DATE_EPOCH
    env["UV_NO_PROGRESS"] = "1"
    result = subprocess.run(
        ["uv", "build", "--package", package, "--wheel", "--out-dir", str(out_dir)],
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
    )
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic output"
        raise ProvenanceError(f"uv build for {package} failed: {detail}")
    stem = package.replace("-", "_")
    wheels = sorted(out_dir.glob(f"{stem}-*.whl"))
    if len(wheels) != 1:
        raise ProvenanceError(f"expected one {package} wheel, found {len(wheels)} in {out_dir}")
    return wheels[0]


def record_entry(rows: dict[str, tuple[str, str]], member: str, contents: bytes) -> dict[str, Any]:
    try:
        encoded, size = rows[member]
    except KeyError as error:
        raise ProvenanceError(f"wheel RECORD is missing {member}") from error
    if not encoded.startswith("sha256="):
        raise ProvenanceError(f"wheel RECORD does not sha256-protect {member}")
    digest = encoded.removeprefix("sha256=")
    expected = base64.urlsafe_b64encode(hashlib.sha256(contents).digest()).decode().rstrip("=")
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


def wheel_contents(path: Path) -> tuple[zipfile.ZipFile, list[str], dict[str, tuple[str, str]]]:
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
        raise ProvenanceError(f"RECORD inventory differs from wheel members: {path.name}")
    return wheel, names, rows


def only_member(names: list[str], suffix: str, label: str) -> str:
    matches = [name for name in names if name.endswith(suffix)]
    if len(matches) != 1:
        raise ProvenanceError(f"expected one {label} member ending {suffix}, found {len(matches)}")
    return matches[0]


def metadata_fact(wheel: zipfile.ZipFile, names: list[str], rows: dict[str, tuple[str, str]]) -> dict[str, Any]:
    member = only_member(names, ".dist-info/METADATA", "METADATA")
    contents = wheel.read(member)
    message = BytesParser(policy=default).parsebytes(contents)
    name = message["Name"]
    version = message["Version"]
    if name is None or version is None:
        raise ProvenanceError("wheel METADATA is missing Name or Version")
    return {
        "name": name,
        "record": record_entry(rows, member, contents),
        "requires_dist": message.get_all("Requires-Dist", []),
        "version": version,
    }


def journal_wheel_fact(path: Path) -> dict[str, Any]:
    wheel, names, rows = wheel_contents(path)
    try:
        metadata = metadata_fact(wheel, names, rows)
        if metadata["name"] != JOURNAL_PACKAGE:
            raise ProvenanceError(f"journal wheel METADATA names {metadata['name']!r}")
        dependencies = [item for item in metadata["requires_dist"] if item.startswith(CORE_JOURNAL_PACKAGE)]
        if not dependencies:
            raise ProvenanceError("journal METADATA does not select solstone-core-journal")
        launcher_member = only_member(names, ".data/scripts/journal", "journal script-files launcher")
        launcher = wheel.read(launcher_member)
        source = LAUNCHER.read_bytes()
        if launcher != source:
            raise ProvenanceError("wheel journal script-files launcher differs from its source script")
        entry_points_member = only_member(names, ".dist-info/entry_points.txt", "entry-points")
        entry_points = wheel.read(entry_points_member)
        if b"mlx-vlm-server = solstone.think.providers.mlx_server:main" not in entry_points:
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
            raise ProvenanceError(f"core journal wheel METADATA names {metadata['name']!r}")
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


def build_once() -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="service-legacy-wheel-", dir=SCRATCH_ROOT) as temporary:
        out_dir = Path(temporary) / "dist"
        out_dir.mkdir()
        journal = journal_wheel_fact(build_wheel(JOURNAL_PACKAGE, out_dir))
        core = core_journal_wheel_fact(build_wheel(CORE_JOURNAL_PACKAGE, out_dir))
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
        raise ProvenanceError("native process table does not select solstone.think.service")
    if "importlib.import_module(module).main()" not in runner:
        raise ProvenanceError("native runner does not invoke the selected module main()")


def payload(wheels: dict[str, Any], tools: dict[str, str], source_commit: str) -> dict[str, Any]:
    journal = wheels["solstone_journal"]
    core = wheels["solstone_core_journal"]
    verify_native_dispatch_sources()
    return {
        "build": {
            "source_date_epoch": SOURCE_DATE_EPOCH,
            "tools": tools,
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
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(temporary, OUTPUT)


def main() -> int:
    source_commit = git_head()
    tools = tool_versions()
    SCRATCH_ROOT.mkdir(exist_ok=True)
    first = payload(build_once(), tools, source_commit)
    second = payload(build_once(), tools, source_commit)
    if first != second:
        raise ProvenanceError("two fixed-epoch wheel builds produced different provenance facts")
    write_payload(first)
    print("wrote deterministic journal packaging provenance for two wheels")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
