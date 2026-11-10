#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Build and package the immutable callable backup archive for the iOS host."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
BUILDER = ROOT / "scripts/backup_bridge_build.py"
sys.path.insert(0, str(ROOT / "scripts"))
spec = importlib.util.spec_from_file_location("ios_backup_immutable_recipe", BUILDER)
recipe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recipe)

REQUIRED_STEPS = frozenset((
    "library-safety-overlay", "rooted-restorer-overlay", "existing-root-overlay",
    "go-download", "go-verify", "go-deps", "go-test", "rooted-metadata-tests",
    "existing-root-tests", "receipt-control-tests", "go-compile-integration", "go-run-integration",
    "host-c-archive-build", "host-c-link", "host-c-run", "metadata-c-link", "metadata-c-run",
    "selection-c-link", "selection-c-run", "options-c-link", "options-c-run", "identity-c-link",
    "identity-c-run", "directories-c-link", "directories-c-run", "existing-root-c-link", "existing-root-c-run", "workspace-c-link",
    "workspace-c-run", "s3-c-link", "s3-c-run", "c-archive-build", "target-source-selection",
    "target-archive-symbols", "c-link-check", "caller-platform",
))
SYMBOLS = (
    "restic_bridge_build_identity_v1", "restic_bridge_operation_directories_v1_start",
    "restic_bridge_s3_operation_directories_v1_start", "restic_bridge_wait", "restic_bridge_free",
    "restic_bridge_cancel", "restic_bridge_result_metadata", "sb_s3_owner_file_policy",
    "restic_bridge_s3_archive_sources_v2_start",
)


def pairs(values):
    result = {}
    for key, value in values:
        if key in result:
            raise ValueError("duplicate backup build record key")
        result[key] = value
    return result


def read_regular(path, maximum):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        before = os.fstat(stream.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size > maximum:
            raise ValueError("invalid backup build file")
        data = stream.read(maximum + 1)
        after = os.fstat(stream.fileno())
        bound = path.lstat()
        identity = lambda value: (value.st_dev, value.st_ino, value.st_mode, value.st_nlink, value.st_size, value.st_mtime_ns, value.st_ctime_ns)
        if len(data) != before.st_size or identity(before) != identity(after) or identity(bound) != identity(before):
            raise ValueError("backup build file changed")
        return data


def sha(data):
    return hashlib.sha256(data).hexdigest()


def validate_record(data):
    if len(data) > 1024 * 1024:
        raise ValueError("backup build record exceeds bound")
    record = json.loads(data, object_pairs_hook=pairs)
    inputs = json.loads(recipe.INPUTS_PATH.read_bytes(), object_pairs_hook=pairs)
    expected, fingerprint = recipe.linked_build_identity("ios", "arm64", inputs["inputs"]["go-ios"]["sha256"])
    if not (record.get("status") == "passed" and record.get("complete") is True
            and type(record.get("exit")) is int and record["exit"] == 0
            and type(record.get("compile_exit")) is int and record["compile_exit"] == 0
            and record.get("target") == "ios" and record.get("goos") == "ios" and record.get("goarch") == "arm64"
            and record.get("linked_build_identity") == expected and record.get("linked_build_identity_sha256") == fingerprint):
        raise ValueError("backup record does not bind the current immutable iOS inputs")
    for key, value in expected.items():
        if key.endswith("_sha256") and key != "acquisition_helper_sha256" and record.get(key) != value:
            raise ValueError("backup record source digest disagreement")
    steps = record.get("steps", [])
    names = [step.get("name") for step in steps]
    if len(names) != len(REQUIRED_STEPS) or set(names) != REQUIRED_STEPS:
        raise ValueError("backup recipe step set differs")
    for step in steps:
        if type(step.get("exit")) is not int or step["exit"] != 0:
            raise ValueError("backup recipe step failed")
        for digest in [step.get("log_sha256")]:
            if not isinstance(digest, str) or len(digest) != 64 or any(ch not in "0123456789abcdef" for ch in digest):
                raise ValueError("backup recipe log digest invalid")
    for key in ["archive_sha256", "header_sha256"]:
        digest = record.get(key)
        if not isinstance(digest, str) or len(digest) != 64 or any(ch not in "0123456789abcdef" for ch in digest):
            raise ValueError("backup artifact digest invalid")
    return record


def validate_output(work):
    for path in [BUILDER, recipe.INPUTS_PATH, ROOT / "scripts/android_cross_build.py", *recipe.BRIDGE_SRC_PATH.rglob("*")]:
        if path.is_symlink() or (not path.is_file() and not path.is_dir()):
            raise ValueError("foreign immutable backup source member")
    record_path = work / "receipts/backup-bridge-build.json"
    data = read_regular(record_path, 1024 * 1024)
    record = validate_record(data)
    for step in record["steps"]:
        path = work / "receipts" / (step["name"] + ".log")
        if Path(step["log"]).name != path.name or sha(read_regular(path, 64 * 1024 * 1024)) != step["log_sha256"]:
            raise ValueError("backup recipe log differs from measured record")
    archive = read_regular(work / "artifacts/librestic_bridge.a", 256 * 1024 * 1024)
    header = read_regular(work / "artifacts/librestic_bridge.h", 1024 * 1024)
    if sha(archive) != record["archive_sha256"] or sha(header) != record["header_sha256"]:
        raise ValueError("backup linked archive/header differs from build record")
    return data, archive


def create_or_confirm(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with path.open("xb") as stream:
            stream.write(data)
        path.chmod(0o444)
    except FileExistsError:
        if read_regular(path, max(len(data), 1)) != data:
            raise ValueError("existing backup output differs; use a fresh build output")


def confirm_resource(directory, record):
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("backup resource directory invalid")
    members = list(directory.iterdir())
    if len(members) != 1 or members[0].name != "backup-build.json" or read_regular(members[0], 1024 * 1024) != record:
        raise ValueError("backup signed resource closure differs")


def check_symbols(executable):
    result = subprocess.run(["nm", "-gU", str(executable)], capture_output=True, text=True, timeout=120, check=False)
    if result.returncode != 0:
        raise ValueError("could not inspect backup linked closure")
    names = {line.split()[-1] for line in result.stdout.splitlines() if line.split()}
    if any("_" + symbol not in names for symbol in SYMBOLS):
        raise ValueError("backup linked operation/identity/policy closure is incomplete")


def run_recipe(argv):
    process = subprocess.Popen(argv, start_new_session=True)
    try:
        code = process.wait(timeout=5400)
    except BaseException:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=60)
        raise
    if code != 0:
        raise subprocess.CalledProcessError(code, argv)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--work-dir", required=True, type=Path)
    parser.add_argument("--cache-dir", required=True, type=Path)
    parser.add_argument("--output-lib", required=True, type=Path)
    parser.add_argument("--resource-dir", required=True, type=Path)
    parser.add_argument("--jobs", type=int, default=6)
    args = parser.parse_args()
    work = args.work_dir.resolve()
    if not work.exists():
        run_recipe([sys.executable, str(BUILDER), "--target", "ios", "--work-dir", str(work), "--cache-dir", str(args.cache_dir.resolve()), "--jobs", str(args.jobs)])
    record, archive = validate_output(work)
    create_or_confirm(args.output_lib, archive)
    args.resource_dir.mkdir(parents=True, exist_ok=True)
    if list(args.resource_dir.iterdir()) and {p.name for p in args.resource_dir.iterdir()} != {"backup-build.json"}:
        raise ValueError("foreign backup resource member")
    create_or_confirm(args.resource_dir / "backup-build.json", record)
    confirm_resource(args.resource_dir, record)
    print(json.dumps({"archive_sha256":sha(archive), "record_sha256":sha(record), "boundary":"Build and packaged inputs only; actual linked startup and callable/device/lifetime gates remain separate."}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
