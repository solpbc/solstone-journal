#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Smoke guard for the native `sol` access surface."""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUST_MANIFEST = ROOT / "core" / "Cargo.toml"
BLOCKED_FAMILIES = (
    "flask",
    "werkzeug",
    "jinja2",
    "anthropic",
    "openai",
    "httpx",
    "numpy",
    "PIL",
    "soundfile",
    "av",
    "pypdfium2",
    "frontmatter",
    "cryptography",
    "OpenSSL",
    "argon2",
    "websockets",
)

try:
    from scripts.build_native_sol_journal_host_commands import (
        extract as extract_journal_host_commands,
    )
    from scripts.check_wheel_contents import CORE_SCRIPT_NAMES, ROOT_LAUNCHER_NAMES
except ModuleNotFoundError:  # pragma: no cover - direct script execution path.
    from build_native_sol_journal_host_commands import (  # type: ignore[no-redef]
        extract as extract_journal_host_commands,
    )
    from check_wheel_contents import (  # type: ignore[no-redef]
        CORE_SCRIPT_NAMES,
        ROOT_LAUNCHER_NAMES,
    )

from solstone.think.generated.access_rejections import JOURNAL_ACCESS_ONLY_COMMANDS

if "import" not in JOURNAL_ACCESS_ONLY_COMMANDS:
    raise RuntimeError(
        "generated journal access rejection inventory is missing 'import'"
    )

NATIVE_CASES: tuple[tuple[str, list[str]], ...] = (
    ("sol", ["sol"]),
    ("sol -h", ["sol", "-h"]),
    ("sol help", ["sol", "help"]),
    ("sol -v", ["sol", "-v"]),
    ("sol -v --help", ["sol", "-v", "--help"]),
    ("sol --help", ["sol", "--help"]),
    ("sol --version", ["sol", "--version"]),
    ("sol -V", ["sol", "-V"]),
    ("sol root", ["sol", "root"]),
    ("sol status", ["sol", "status"]),
    ("sol chat --help", ["sol", "chat", "--help"]),
    ("sol import --help", ["sol", "import", "--help"]),
    ("sol link --help", ["sol", "link", "--help"]),
    ("sol link join --help", ["sol", "link", "join", "--help"]),
    ("sol link serve --help", ["sol", "link", "serve", "--help"]),
    ("sol call --help", ["sol", "call", "--help"]),
    ("sol call activities --help", ["sol", "call", "activities", "--help"]),
    (
        "sol call activities list --help",
        ["sol", "call", "activities", "list", "--help"],
    ),
)
JOURNAL_CASES: tuple[tuple[str, list[str]], ...] = (
    ("journal transcribe --help", ["journal", "transcribe", "--help"]),
)
JOURNAL_FAILURE_CASES: tuple[tuple[str, list[str], int, str], ...] = (
    (
        "journal retired access spelling",
        ["journal", "import"],
        64,
        "Usage: journal <command> [args...]",
    ),
)
NATIVE_FAILURE_CASES: tuple[tuple[str, list[str], int, str], ...] = (
    (
        "native moved-stub remains native",
        ["sol", "call", "identity"],
        2,
        "Moved to `journal identity`",
    ),
)
ACCESS_CASES: tuple[tuple[str, list[str]], ...] = NATIVE_CASES + JOURNAL_CASES
SERVICE_MOVED_ROUTING_CASES: tuple[tuple[str, list[str], str], ...] = tuple(
    (
        f"service-only native command {command}",
        ["sol", command, "--help"],
        f"'{command}' moved to 'journal {command}' — run that instead.",
    )
    for command in extract_journal_host_commands()
)
ROUTING_CASES: tuple[tuple[str, list[str], str], ...] = (
    (
        "retired native path flag",
        ["sol", "--path"],
        "Usage: sol <command> [args...]",
    ),
    (
        "retired native path command",
        ["sol", "path"],
        "Unsupported native sol command.",
    ),
    (
        "unknown native command",
        ["sol", "does-not-exist"],
        "Unsupported native sol command.",
    ),
    (
        "unknown native call group",
        ["sol", "call", "not-real", "--help"],
        "Unsupported native sol command.",
    ),
) + SERVICE_MOVED_ROUTING_CASES
EXPECTED_SCRIPT_OWNERS = {
    **{name: ["solstone"] for name in ROOT_LAUNCHER_NAMES},
    **{name: ["solstone-core"] for name in CORE_SCRIPT_NAMES},
    "solstone-core-sol": ["solstone-core-sol"],
    "solstone-core-journal": ["solstone-core-journal"],
}
JOURNAL_SCRIPT_OWNER_OPTIONS = (["solstone-journal"], ["solstone-journal-cuda"])
_SOURCE_NATIVE_BIN_DIR: Path | None = None

def _source_native_bin_dir() -> Path:
    global _SOURCE_NATIVE_BIN_DIR
    if _SOURCE_NATIVE_BIN_DIR is not None:
        return _SOURCE_NATIVE_BIN_DIR
    subprocess.run(
        [
            "cargo",
            "build",
            "--quiet",
            "--manifest-path",
            str(RUST_MANIFEST),
            "-p",
            "solstone-core-sol-bin",
            "-p",
            "solstone-core-journal-bin",
            "--locked",
        ],
        cwd=ROOT,
        check=True,
        timeout=900,
    )
    _SOURCE_NATIVE_BIN_DIR = ROOT / "core" / "target" / "debug"
    for name in ROOT_LAUNCHER_NAMES:
        launcher = ROOT / "scripts" / "root-launchers" / name
        target = _SOURCE_NATIVE_BIN_DIR / name
        shutil.copy2(launcher, target)
        target.chmod(0o755)
    journal_launcher = ROOT / "packages" / "solstone-journal" / "scripts" / "journal"
    journal_target = _SOURCE_NATIVE_BIN_DIR / "journal"
    shutil.copy2(journal_launcher, journal_target)
    journal_target.chmod(0o755)
    return _SOURCE_NATIVE_BIN_DIR


def _bin_dir(python: str | None, *, source_root_on_path: bool) -> Path:
    if source_root_on_path:
        return ROOT / ".venv" / "bin"
    if python is None:
        raise RuntimeError("installed mode requires a Python executable")
    return Path(python).parent


def _run_native_case(
    label: str,
    argv: list[str],
    *,
    python: str | None = None,
    source_root_on_path: bool = True,
) -> subprocess.CompletedProcess[str]:
    if source_root_on_path and argv[0] in {"sol", "solstone"}:
        bin_dir = _source_native_bin_dir()
    else:
        bin_dir = _bin_dir(python, source_root_on_path=source_root_on_path)
    executable = bin_dir / argv[0]
    env = os.environ.copy()
    env.setdefault("SOLSTONE_JOURNAL", str(ROOT / "tests" / "fixtures" / "journal"))
    if source_root_on_path:
        env["PYTHONPATH"] = (
            str(ROOT)
            if not env.get("PYTHONPATH")
            else str(ROOT) + os.pathsep + env["PYTHONPATH"]
        )
    return _run_with_terminal_stdin(
        [str(executable), *argv[1:]],
        cwd=ROOT,
        env=env,
        timeout=90,
    )


def _run_with_terminal_stdin(
    argv: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: int,
) -> subprocess.CompletedProcess[str]:
    if not hasattr(os, "openpty"):
        return subprocess.run(
            argv,
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    master_fd, slave_fd = os.openpty()
    try:
        return subprocess.run(
            argv,
            cwd=cwd,
            env=env,
            stdin=slave_fd,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout if isinstance(error.stdout, str) else ""
        stderr = error.stderr if isinstance(error.stderr, str) else ""
        return subprocess.CompletedProcess(
            argv,
            124,
            stdout,
            stderr
            + "\naccess-imports-clean: native command timed out with terminal stdin\n",
        )
    finally:
        with contextlib.suppress(OSError):
            os.close(slave_fd)
        with contextlib.suppress(OSError):
            os.close(master_fd)


def _format_failure(label: str, result: subprocess.CompletedProcess[str]) -> str:
    return (
        f"access-imports-clean: FAIL {label} exited {result.returncode}\n"
        f"--- stdout ---\n{result.stdout}\n"
        f"--- stderr ---\n{result.stderr}"
    )


def _has_traceback(result: subprocess.CompletedProcess[str]) -> bool:
    return "Traceback (most recent call last)" in result.stdout + result.stderr


def _check_success_result(
    label: str, result: subprocess.CompletedProcess[str]
) -> list[str]:
    failures: list[str] = []
    if result.returncode != 0:
        failures.append(_format_failure(label, result))
        return failures
    if _has_traceback(result):
        failures.append(f"access-imports-clean: FAIL {label} printed a traceback")
    return failures


def _check_routing_result(
    label: str, result: subprocess.CompletedProcess[str], expected: str
) -> list[str]:
    output = result.stdout + result.stderr
    failures: list[str] = []
    if result.returncode == 0:
        failures.append(_format_failure(label, result))
    if expected not in output:
        failures.append(
            f"access-imports-clean: FAIL {label} missing routing text: {expected}"
        )
    if _has_traceback(result):
        failures.append(f"access-imports-clean: FAIL {label} printed a traceback")
    return failures


def _check_failure_result(
    label: str,
    result: subprocess.CompletedProcess[str],
    expected_exit: int,
    expected_text: str,
) -> list[str]:
    output = result.stdout + result.stderr
    failures: list[str] = []
    if result.returncode != expected_exit:
        failures.append(_format_failure(label, result))
    if expected_text not in output:
        failures.append(
            f"access-imports-clean: FAIL {label} missing failure text: {expected_text}"
        )
    if _has_traceback(result):
        failures.append(f"access-imports-clean: FAIL {label} printed a traceback")
    return failures


def run_checks(
    *,
    python: str | None = None,
    source_root_on_path: bool = True,
) -> list[str]:
    failures: list[str] = []
    for label, argv in NATIVE_CASES + JOURNAL_CASES:
        if argv[0] == "journal" and not source_root_on_path:
            continue
        failures.extend(
            _check_success_result(
                label,
                _run_native_case(
                    label,
                    argv,
                    python=python,
                    source_root_on_path=source_root_on_path,
                ),
            )
        )
    for label, argv, expected_exit, expected_text in JOURNAL_FAILURE_CASES:
        if not source_root_on_path:
            continue
        failures.extend(
            _check_failure_result(
                label,
                _run_native_case(
                    label,
                    argv,
                    python=python,
                    source_root_on_path=source_root_on_path,
                ),
                expected_exit,
                expected_text,
            )
        )
    for label, argv, expected_exit, expected_text in NATIVE_FAILURE_CASES:
        failures.extend(
            _check_failure_result(
                label,
                _run_native_case(
                    label,
                    argv,
                    python=python,
                    source_root_on_path=source_root_on_path,
                ),
                expected_exit,
                expected_text,
            )
        )
    for label, argv, expected in ROUTING_CASES:
        failures.extend(
            _check_routing_result(
                label,
                _run_native_case(
                    label,
                    argv,
                    python=python,
                    source_root_on_path=source_root_on_path,
                ),
                expected,
            )
        )
    return failures


def _real_base_python(root: Path, tmpdir: str) -> str:
    """Build a fresh venv with the REAL thin base partition (no extras)."""
    venv = Path(tmpdir) / "thin-base-venv"
    uv = shutil.which("uv")
    if not uv:
        raise RuntimeError("--real-install requires uv")
    dist = Path(tmpdir) / "thin-base-dist"
    dist.mkdir()
    subprocess.run([uv, "venv", str(venv)], check=True, capture_output=True, text=True)
    subprocess.run(
        [uv, "build", "--package", "solstone-core", "--wheel", "--out-dir", str(dist)],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
        timeout=900,
    )
    subprocess.run(
        [uv, "build", "--wheel", "--out-dir", str(dist)],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
        timeout=900,
    )
    python = str(venv / "bin" / "python")
    root_wheels = sorted(
        path
        for path in dist.glob("solstone-*.whl")
        if path.name.endswith("-py3-none-any.whl")
    )
    core_wheels = sorted(dist.glob("solstone_core-*.whl"))
    if len(root_wheels) != 1 or len(core_wheels) != 1:
        raise RuntimeError(
            "real thin-base install expected one root wheel and one core wheel; "
            f"found root={len(root_wheels)} core={len(core_wheels)}"
        )
    subprocess.run(
        [
            uv,
            "pip",
            "install",
            "--python",
            python,
            "--offline",
            "--find-links",
            str(dist),
            str(root_wheels[0]),
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=900,
    )
    return python


def _check_heavy_absent(python: str) -> list[str]:
    """Assert no blocked heavy family is importable in the real thin base."""
    families = sorted({family.split(".")[0] for family in BLOCKED_FAMILIES})
    probe = (
        "import importlib.util as u, json, sys\n"
        "present = []\n"
        "for m in json.loads(sys.argv[1]):\n"
        "    try:\n"
        "        if u.find_spec(m) is not None: present.append(m)\n"
        "    except Exception:\n"
        "        pass\n"
        "print(json.dumps(present))\n"
    )
    result = subprocess.run(
        [python, "-c", probe, json.dumps(families)],
        capture_output=True,
        text=True,
        timeout=60,
    )
    if result.returncode != 0:
        return [
            "access-imports-clean: FAIL heavy-absence probe errored\n"
            f"{result.stdout}\n{result.stderr}"
        ]
    present = json.loads(result.stdout or "[]")
    if present:
        return [
            "access-imports-clean: FAIL real base partition contains heavy "
            f"families: {present}"
        ]
    return []


def _check_script_owners(python: str) -> list[str]:
    bin_dir = Path(python).parent
    scripts = (*EXPECTED_SCRIPT_OWNERS, "journal")
    for script in scripts:
        path = bin_dir / script
        if not path.exists() or not os.access(path, os.X_OK):
            return [
                "access-imports-clean: FAIL installed script is missing or not "
                f"executable: {path}"
            ]
    probe = (
        "import importlib.metadata as md, json, pathlib, sys\n"
        "scripts = {pathlib.Path(p).resolve(): pathlib.Path(p).name for p in sys.argv[1:]}\n"
        "owners = {name: [] for name in scripts.values()}\n"
        "for dist in md.distributions():\n"
        "    name = dist.metadata.get('Name')\n"
        "    for file in dist.files or []:\n"
        "        try:\n"
        "            located = dist.locate_file(file).resolve()\n"
        "        except OSError:\n"
        "            continue\n"
        "        if located in scripts:\n"
        "            owners[scripts[located]].append(name)\n"
        "print(json.dumps({k: sorted(v) for k, v in owners.items()}, sort_keys=True))\n"
    )
    result = subprocess.run(
        [
            python,
            "-c",
            probe,
            *(str(bin_dir / name) for name in scripts),
        ],
        capture_output=True,
        text=True,
        timeout=60,
    )
    if result.returncode != 0:
        return [
            "access-imports-clean: FAIL script-owner probe errored\n"
            f"{result.stdout}\n{result.stderr}"
        ]
    owners = json.loads(result.stdout)
    failures: list[str] = []
    for script, expected in sorted(EXPECTED_SCRIPT_OWNERS.items()):
        actual = owners.get(script, [])
        if actual != expected:
            failures.append(
                f"access-imports-clean: FAIL {script} owners {actual!r} != {expected!r}"
            )
    journal_owners = owners.get("journal", [])
    if journal_owners not in JOURNAL_SCRIPT_OWNER_OPTIONS:
        failures.append(
            "access-imports-clean: FAIL journal owners "
            f"{journal_owners!r} not in {JOURNAL_SCRIPT_OWNER_OPTIONS!r}"
        )
    return failures


def _check_installed_sol_root_canonicalization(
    python: str, source_root: Path
) -> list[str]:
    failures: list[str] = []
    bin_dir = Path(python).parent
    prefix = bin_dir.parent
    lib = prefix / "lib"
    lib64 = prefix / "lib64"
    if not lib.is_dir():
        return [
            "access-imports-clean: FAIL installed sol root check expected venv lib "
            f"directory: {lib}"
        ]
    if not lib64.exists():
        if not hasattr(os, "symlink"):
            return [
                "access-imports-clean: FAIL installed sol root check cannot "
                f"fabricate lib64 alias on this platform: {lib64}"
            ]
        os.symlink("lib", lib64)
        print(
            "access-imports-clean: fabricated lib64 -> lib alias for installed "
            f"sol root check: {lib64}"
        )
    else:
        print(
            "access-imports-clean: found existing lib64 for installed sol root "
            f"check: {lib64}"
        )
    if not lib64.is_dir():
        return [
            "access-imports-clean: FAIL installed sol root check expected venv "
            f"lib64 directory or alias: {lib64}"
        ]

    env = os.environ.copy()
    env.pop("PYTHONPATH", None)
    probe = (
        "import importlib.metadata as md, importlib.util as u, json, pathlib, sys\n"
        "spec = u.find_spec('solstone')\n"
        "assert spec and spec.origin\n"
        "dist = md.distribution('solstone')\n"
        "print(json.dumps({\n"
        "    'root': str(pathlib.Path(spec.origin).resolve().parent.parent),\n"
        "    'dist': str(pathlib.Path(dist.locate_file('')).resolve()),\n"
        "}, sort_keys=True))\n"
    )
    with tempfile.TemporaryDirectory(prefix="solstone-root-cwd-") as tmpdir:
        unrelated = Path(tmpdir) / "unrelated"
        unrelated.mkdir()
        probe_result = subprocess.run(
            [python, "-c", probe],
            cwd=unrelated,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        if probe_result.returncode != 0:
            return [
                "access-imports-clean: FAIL installed sol root expected-root probe "
                f"exited {probe_result.returncode}\n--- stdout ---\n"
                f"{probe_result.stdout}\n--- stderr ---\n{probe_result.stderr}"
            ]
        payload = json.loads(probe_result.stdout)
        expected_root = Path(payload["root"])
        dist_root = Path(payload["dist"])
        source_root = source_root.resolve()
        if expected_root.is_relative_to(source_root) or dist_root.is_relative_to(
            source_root
        ):
            return [
                "access-imports-clean: FAIL installed sol root probe resolved the "
                f"checkout instead of the venv: root={expected_root} dist={dist_root}"
            ]

        executable = bin_dir / "sol"
        expected_stdout = f"{expected_root}\n"
        for label, cwd in (
            ("unrelated cwd", unrelated),
            ("source checkout cwd", source_root),
        ):
            result = _run_with_terminal_stdin(
                [str(executable), "root"],
                cwd=cwd,
                env=env,
                timeout=90,
            )
            if result.returncode != 0:
                failures.append(_format_failure(f"sol root {label}", result))
                continue
            if result.stdout != expected_stdout:
                failures.append(
                    "access-imports-clean: FAIL sol root "
                    f"{label} stdout {result.stdout!r} != {expected_stdout!r}"
                )
            if result.stderr != "":
                failures.append(
                    "access-imports-clean: FAIL sol root "
                    f"{label} stderr {result.stderr!r} != ''"
                )
            if result.stdout == expected_stdout and result.stderr == "":
                print(
                    f"access-imports-clean: sol root {label} matched installed "
                    f"root: {expected_root}"
                )
    return failures


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument(
        "--real-install",
        action="store_true",
        help=(
            "build a fresh venv with the real thin base partition (no extras) "
            "and assert against it, instead of the meta_path simulation"
        ),
    )
    args = parser.parse_args(argv)

    root = args.root.resolve()
    if root != ROOT:
        raise RuntimeError("--root override is no longer supported by this gate")
    if args.real_install:
        with tempfile.TemporaryDirectory(prefix="solstone-thin-base-") as tmpdir:
            print("access-imports-clean: building real thin-base venv (no extras)...")
            python = _real_base_python(root, tmpdir)
            failures = _check_heavy_absent(python)
            failures.extend(_check_script_owners(python))
            failures.extend(_check_installed_sol_root_canonicalization(python, root))
            failures.extend(
                run_checks(
                    python=python,
                    source_root_on_path=False,
                )
            )
    else:
        failures = run_checks()

    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1
    mode = "real-install" if args.real_install else "simulated"
    print(f"access-imports-clean: pass ({mode})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
