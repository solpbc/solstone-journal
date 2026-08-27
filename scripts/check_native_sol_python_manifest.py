#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
from __future__ import annotations

import hashlib
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
PRE_CUTOVER_COMMIT = "dd04f55c8"
RETIRED_SOL_CLI = "solstone/think/sol_cli.py"
SURVIVING_JOURNAL_CALL_ORACLE = "solstone/think/tools/call.py"

# The compatibility bridge was added after the pre-cutover deletion manifest.
# These are current-tree closure checks rather than historical blob assertions.
COMPAT_BRIDGE_DELETED_PATHS = (
    "solstone/think/sol_compat_cli.py",
    "solstone/think/sol_compat_inventory.py",
    "solstone/think/call.py",
    "scripts/check_native_sol_compat.py",
    "tests/test_sol_compat_cli.py",
    "solstone/apps/body/routes.py",
    "solstone/apps/body/contract.py",
    "solstone/apps/body/events.py",
    "solstone/apps/body/workspace.html",
    "solstone/apps/body/tests/test_body_app.py",
    "solstone/apps/body/tests/conftest.py",
)

RUST_COMPAT_FORBIDDEN_SYMBOLS = (
    "COMPAT_MODULE",
    "COMPAT_SENTINEL",
    "COMPAT_ARGV0_MARKER_PREFIX",
    "COMPAT_SENTINEL_ARMED",
    "COMPAT_RECURSION_ERROR",
    "delegate_to_compat",
    "exec_compat",
    "sibling_python_for_executable",
    "is_compat_public_args",
    "should_delegate_to_compat_after_native_miss",
    "CompatPythonError",
)

# Pre-cutover deletion-owner manifest. `convey_client.py` intentionally
# survives because the OpenAPI contract tests import `resolve_base_url`.
EXPECTED_BLOBS = {
    "solstone/apps/activities/call.py": "b50d44bfee5b1649bc506c39063fb78234966f95",
    "solstone/apps/awareness/call.py": "c8607807ac37ea9ec596f5a77b9171707ab9b844",
    "solstone/apps/body/call.py": "14b1ccacec66cda2cc288c3700720ca436fa4308",
    "solstone/apps/chat/call.py": "bf679409d2463280f2dcb6db7a844302e859c52c",
    "solstone/apps/entities/call.py": "cde1cfc193f5e6537c6b620c2fc910dc0c584082",
    "solstone/apps/facets/call.py": "8a0a3a2b4d7086f52af739a87d21294c51bc360d",
    "solstone/apps/import/call.py": "ec3991bf55ee9c6e157a6aba079c9a1d9f076ae7",
    "solstone/apps/network/call.py": "763c129a30fe623bc5c9c0a9e44fd9aa2058a5de",
    "solstone/apps/settings/call.py": "09b9a77917cd756515d8a404efb2acf24f93906a",
    "solstone/apps/sol/call.py": "486861e6557e2c2758abf321e7b77730d358557e",
    "solstone/apps/speakers/call.py": "852454b0cfa11d548042ac3e708c879b5eae2a39",
    "solstone/apps/support/call.py": "5f0802c8a9acde604e70b3a921dbdc36d4579d1f",
    "solstone/apps/thinking/call.py": "aa2cb8c5985aeab1864479c22326e33a8a45ae2a",
    "solstone/apps/transcripts/call.py": "c5d9e81b965d8231903632c7f945c4613cf3d2e4",
    "solstone/think/tools/health.py": "09be4da53688e1d91a10b8bb38c3e9a2a34712b4",
    "solstone/think/tools/profile.py": "96fe4319bdce13029060806aea8523ca680ac2fb",
    "solstone/think/import_client.py": "31a6c12e341e9144b0f9c1567613abf0aabddc3a",
    "solstone/think/chat_cli.py": "f41e0523c6e5574f1d920c5304cdaf96643bdeee",
    RETIRED_SOL_CLI: "a20570fc0994f6215a013e8c89ce7776ddec7d17",
}
EXPECTED_SHA256 = hashlib.sha256(
    "".join(
        f"{path}\t{blob}\n" for path, blob in sorted(EXPECTED_BLOBS.items())
    ).encode("utf-8")
).hexdigest()


def manifest_bytes() -> bytes:
    raw = subprocess.check_output(
        ["git", "ls-tree", PRE_CUTOVER_COMMIT, "--", *EXPECTED_BLOBS],
        cwd=REPO_ROOT,
    )
    lines = raw.splitlines()
    if len(lines) != len(EXPECTED_BLOBS):
        found_paths = {
            line.decode("utf-8").split("\t", maxsplit=1)[1]
            for line in lines
            if b"\t" in line
        }
        missing = sorted(set(EXPECTED_BLOBS) - found_paths)
        raise RuntimeError(
            f"git ls-tree returned {len(lines)} entries; missing: {', '.join(missing)}"
        )
    actual: dict[str, str] = {}
    for line in lines:
        meta, path = line.decode("utf-8").split("\t", maxsplit=1)
        parts = meta.split()
        if len(parts) < 3:
            raise RuntimeError(f"unexpected git ls-tree row: {line!r}")
        actual[path] = parts[2]
    drift = {
        path: (EXPECTED_BLOBS[path], actual.get(path))
        for path in sorted(EXPECTED_BLOBS)
        if actual.get(path) != EXPECTED_BLOBS[path]
    }
    if drift:
        details = "; ".join(
            f"{path}: expected {expected}, actual {actual_blob}"
            for path, (expected, actual_blob) in drift.items()
        )
        raise RuntimeError(f"native sol Python manifest blob drifted: {details}")
    return "".join(f"{path}\t{blob}\n" for path, blob in sorted(actual.items())).encode(
        "utf-8"
    )


def deletion_errors() -> list[str]:
    errors: list[str] = []
    for path in sorted(EXPECTED_BLOBS):
        current = REPO_ROOT / path
        if current.exists():
            errors.append(f"{path} still exists after native-sol cutover")
    errors.extend(active_sol_cli_reference_errors())
    return errors


def compat_bridge_deletion_errors() -> list[str]:
    errors: list[str] = []
    for path in COMPAT_BRIDGE_DELETED_PATHS:
        if (REPO_ROOT / path).exists():
            errors.append(f"{path} still exists after compat-bridge closure")
    if not (REPO_ROOT / SURVIVING_JOURNAL_CALL_ORACLE).is_file():
        errors.append(
            f"{SURVIVING_JOURNAL_CALL_ORACLE} must survive as the journal call oracle"
        )
    return errors


def compat_executor_errors() -> list[str]:
    errors: list[str] = []
    rust_path = REPO_ROOT / "core/crates/solstone-core-sol/src/lib.rs"
    rust_text = rust_path.read_text(encoding="utf-8")
    for symbol in RUST_COMPAT_FORBIDDEN_SYMBOLS:
        if symbol in rust_text:
            errors.append(f"{rust_path.relative_to(REPO_ROOT)} still contains {symbol}")
    makefile_text = (REPO_ROOT / "Makefile").read_text(encoding="utf-8")
    for symbol in ("check-native-sol-compat", "sol_compat_cli"):
        if symbol in makefile_text:
            errors.append(f"Makefile still contains removed compat reference {symbol}")
    return errors


def active_sol_cli_reference_errors() -> list[str]:
    errors: list[str] = []
    tracked = subprocess.check_output(
        [
            "git",
            "ls-files",
            "-z",
            "--",
            "Makefile",
            "pyproject.toml",
            "packages",
            "solstone",
        ],
        cwd=REPO_ROOT,
    )
    candidates = [
        REPO_ROOT / path.decode("utf-8") for path in tracked.split(b"\0") if path
    ]
    for path in candidates:
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        if "solstone.think.sol_cli" in text or "solstone/think/sol_cli.py" in text:
            errors.append(
                f"{path.relative_to(REPO_ROOT)} still references the retired Python CLI"
            )
    return errors


def main() -> int:
    data = manifest_bytes()
    digest = hashlib.sha256(data).hexdigest()
    errors = deletion_errors()
    errors.extend(compat_bridge_deletion_errors())
    errors.extend(compat_executor_errors())
    if digest != EXPECTED_SHA256:
        errors.insert(
            0,
            "native sol Python manifest digest drifted "
            f"(expected {EXPECTED_SHA256}, actual {digest})",
        )
    if errors:
        print("native sol Python deletion manifest failed:")
        for error in errors:
            print(f"- {error}")
        return 1
    print(
        "native sol Python deletion manifest ok: "
        f"pre_cutover={PRE_CUTOVER_COMMIT} files={len(EXPECTED_BLOBS)} "
        f"sha256={digest}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
