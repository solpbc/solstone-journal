#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture the frozen Python grammar contract for convey and restart-convey.

The source checkout normally has no Python environment.  This script therefore
requires a known runnable reference interpreter and records it in provenance.
It deliberately intercepts Convey's terminal ``os.execv``: accepted Convey
forms are a parser/forwarding contract, not a contract with whichever sibling
``solstone-core`` happens to live beside the reference interpreter.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = ROOT / "core/fixtures/convey_restart_reference_grammar.json"
DEFAULT_REFERENCE_PYTHON = ROOT / ".venv/bin/python"
SCRATCH_JOURNAL = Path("/var/tmp/solstone-restart-capture-journal")
PUBLIC_REFERENCE_ROOT = Path("/workspace/reference")
PUBLIC_REFERENCE_PYTHON = PUBLIC_REFERENCE_ROOT / ".venv/bin/python"
REFERENCE_REV = "c1f9da3e0d4b55cbef68e68876065ac213721d6b"
ALLOWED_CAPTURE_PATHS = {
    Path("scripts/capture_convey_restart_reference_grammar.py"),
    Path("core/fixtures/convey_restart_reference_grammar.json"),
    Path("core/fixtures/convey_restart_divergence_ledger.json"),
}

# Keep these seven lines byte-for-byte equivalent in behavior to
# solstone-core-journal-cli/src/runner.rs:11.  The optional interception below
# happens only after import, immediately before Convey's terminal execv.
PYTHON_BOOTSTRAP = """import importlib, logging, sys
module = sys.argv[1]
display_argv0 = sys.argv[2]
verbose_marker = sys.argv[3]
if verbose_marker == \"1\":
    logging.basicConfig(level=logging.DEBUG)
sys.argv = [display_argv0, *sys.argv[4:]]
"""

CONVEY_INTERCEPT = (
    PYTHON_BOOTSTRAP
    + """import json, os
from pathlib import Path
target = importlib.import_module(module)
def capture_execv(_path, argv):
    Path(os.environ['CAPTURE_FORWARDED_ARGV']).write_text(json.dumps(argv[1:]), encoding='utf-8')
    raise SystemExit(0)
target.os.execv = capture_execv
result = target.main()
sys.exit(0 if result is None else int(result))
"""
)

RESTART_PARSE_INTERCEPT = (
    PYTHON_BOOTSTRAP
    + """import json, os
from pathlib import Path
from solstone.think import utils
parsed = {}
def capture_runtime(verbose, debug):
    parsed['parsed_verbose'] = verbose
    parsed['parsed_debug'] = debug
utils.init_cli_runtime = capture_runtime
target = importlib.import_module(module)
target.require_solstone = lambda: None
def capture_wait(timeout, verbose):
    parsed['parsed_timeout'] = timeout
    parsed['parsed_verbose'] = verbose
    Path(os.environ['CAPTURE_RESTART_PARSE']).write_text(json.dumps(parsed, sort_keys=True), encoding='utf-8')
    raise SystemExit(0)
target.wait_for_convey_restart = capture_wait
result = target.main()
sys.exit(0 if result is None else int(result))
"""
)

REFERENCE_BOOTSTRAP = (
    PYTHON_BOOTSTRAP
    + """result = importlib.import_module(module).main()
sys.exit(0 if result is None else int(result))
"""
)

TEMPLATE_DRIVER = r"""
import sys
from solstone.convey import restart

mode = sys.argv[1]
sys.argv = ["journal restart-convey", "--timeout", "1"]

class Base:
    def __init__(self):
        self.callback = None
    def start(self, callback=None):
        self.callback = callback
    def emit(self, *args, **kwargs):
        return True
    def stop(self):
        pass

class StartFails(Base):
    def start(self, callback=None):
        raise OSError("fixture connect failure")

class EmitFails(Base):
    def emit(self, *args, **kwargs):
        return False

class Started(Base):
    def emit(self, *args, **kwargs):
        self.callback({"tract": "supervisor", "event": "started", "service": "convey", "ref": "restart-ref", "pid": 321})
        return True

class CrashWithLog(Base):
    def emit(self, *args, **kwargs):
        self.callback({"tract": "logs", "event": "line", "name": "convey", "ts": 0, "stream": "stderr", "line": "fixture log"})
        self.callback({"tract": "supervisor", "event": "started", "service": "convey", "ref": "one", "pid": 1})
        self.callback({"tract": "supervisor", "event": "started", "service": "convey", "ref": "two", "pid": 2})
        return True

class VerboseStarted(Base):
    def emit(self, *args, **kwargs):
        self.callback({"tract": "supervisor", "event": "restarting", "service": "convey"})
        self.callback({"tract": "supervisor", "event": "stopped", "service": "convey", "exit_code": 0})
        self.callback({"tract": "supervisor", "event": "started", "service": "convey", "ref": "restart-ref", "pid": 321})
        self.callback({"tract": "logs", "event": "line", "name": "Convey worker", "ts": 0, "stream": "stdout", "line": "fixture log"})
        return True

types = {
    "connect_failure": StartFails,
    "emit_failure": EmitFails,
    "timeout": Base,
    "success_known": Started,
    "success_unknown": Started,
    "crash_logs": CrashWithLog,
    "verbose_success": VerboseStarted,
}
restart.CallosumConnection = types[mode]
restart.require_solstone = lambda: None
restart.read_service_port = (lambda _service: 5015) if mode in {"success_known", "verbose_success"} else (lambda _service: None)
if mode == "timeout":
    sys.argv = ["journal restart-convey", "--timeout", "0"]
if mode == "verbose_success":
    sys.argv.append("-v")
restart.main()
"""


def run_git(*arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    ).stdout


def checked_provenance(reference_python: Path, output: Path) -> dict[str, Any]:
    if not reference_python.is_file() or not os.access(reference_python, os.X_OK):
        raise RuntimeError(f"reference interpreter is missing or not executable: {reference_python}")
    revision = run_git("rev-parse", "HEAD").strip()
    if revision != REFERENCE_REV:
        raise RuntimeError(f"expected reference revision {REFERENCE_REV}, found {revision}")

    allowed = set(ALLOWED_CAPTURE_PATHS)
    try:
        allowed.add(output.relative_to(ROOT))
    except ValueError as error:
        raise RuntimeError("output must remain inside the repository") from error
    dirty = [line for line in run_git("status", "--porcelain").splitlines() if Path(line[3:]) not in allowed]
    if dirty:
        raise RuntimeError(f"refusing capture from a dirty tree: {dirty}")

    python_version = subprocess.run(
        [str(reference_python), "-c", "import sys; print(sys.version.split()[0])"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    if python_version != "3.12.13":
        raise RuntimeError(f"expected Python 3.12.13, found {python_version}")
    import_check = subprocess.run(
        [
            str(reference_python),
            "-c",
            "import solstone.convey.cli, solstone.convey.restart",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env=environment(),
        check=False,
    )
    if import_check.returncode:
        raise RuntimeError(
            "reference interpreter cannot import Convey references: "
            f"{import_check.stderr.strip()}"
        )
    return {
        "captured_from_rev": revision,
        "interpreter": str(PUBLIC_REFERENCE_PYTHON),
        "python": python_version,
        "pythonpath": str(PUBLIC_REFERENCE_ROOT),
        "solstone_journal": str(SCRATCH_JOURNAL),
        "tree_cleanliness": {
            "source_dirty_files": 0,
            "allowed_capture_artifacts": sorted(str(path) for path in allowed),
        },
        "bootstrap": {
            "source": "core/crates/solstone-core-journal-cli/src/runner.rs:11,125",
            "argv0": {
                "convey": "journal convey",
                "restart_convey": "journal restart-convey",
            },
            "description": "sys.argv is reset to [display_argv0, *sys.argv[4:]] before main()",
        },
        "convey_exec_capture": {
            "intercepted": "solstone.convey.cli.os.execv",
            "records": "forwarded_argv excluding the helper binary path",
            "note": "accepted Convey cases are parse-accept records without an exit code. The previously verified external interpreter /workspace/reference/.venv/bin/python has sibling solstone-core 0.8.9, so post-exec output is never corpus behavior.",
        },
        "guard": "This is immutable committed capture data. A build worktree without a runnable Python environment must not regenerate it.",
    }


def environment(extra: dict[str, str] | None = None) -> dict[str, str]:
    result = os.environ.copy()
    result["PYTHONPATH"] = str(ROOT)
    result["SOLSTONE_JOURNAL"] = str(SCRATCH_JOURNAL)
    result.pop("SOL_SKIP_SUPERVISOR_CHECK", None)
    result.pop("SOL_SUPERVISOR_SPAWNED", None)
    if extra:
        result.update(extra)
    return result


def normalize(text: str) -> str:
    text = text.replace(str(SCRATCH_JOURNAL), "{journal}")
    return re.sub(r"\[\d{2}:\d{2}:\d{2}\]", "[{time}]", text)


def run_reference(
    reference_python: Path,
    module: str,
    display_argv0: str,
    argv: list[str],
    *,
    extra_env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(reference_python), "-c", REFERENCE_BOOTSTRAP, module, display_argv0, "0", *argv],
        capture_output=True,
        text=True,
        env=environment(extra_env),
        check=False,
    )


def reference_case(
    reference_python: Path,
    label: str,
    module: str,
    display_argv0: str,
    argv: list[str],
    phase: str = "parse",
    extra_env: dict[str, str] | None = None,
) -> dict[str, Any]:
    result = run_reference(reference_python, module, display_argv0, argv, extra_env=extra_env)
    return {
        "label": label,
        "argv": argv,
        "phase": phase,
        "exit": result.returncode,
        "stdout": normalize(result.stdout),
        "stderr": normalize(result.stderr),
    }


def convey_accept_case(
    reference_python: Path, label: str, argv: list[str]
) -> dict[str, Any]:
    forwarded = SCRATCH_JOURNAL / f"forwarded-{label}.json"
    child_env = environment({"CAPTURE_FORWARDED_ARGV": str(forwarded)})
    result = subprocess.run(
        [
            str(reference_python),
            "-c",
            CONVEY_INTERCEPT,
            "solstone.convey.cli",
            "journal convey",
            "0",
            *argv,
        ],
        capture_output=True,
        text=True,
        env=child_env,
        check=False,
    )
    if result.returncode != 0 or not forwarded.is_file():
        raise RuntimeError(
            f"convey exec interception failed for {label}: exit={result.returncode} stderr={result.stderr!r}"
        )
    return {
        "label": label,
        "argv": argv,
        "phase": "parse-accept",
        "forwarded_argv": json.loads(forwarded.read_text(encoding="utf-8")),
        "stdout": normalize(result.stdout),
        "stderr": normalize(result.stderr),
    }


def restart_accept_case(
    reference_python: Path, label: str, argv: list[str]
) -> dict[str, Any]:
    parsed_path = SCRATCH_JOURNAL / f"restart-parsed-{label}.json"
    result = subprocess.run(
        [
            str(reference_python),
            "-c",
            RESTART_PARSE_INTERCEPT,
            "solstone.convey.restart",
            "journal restart-convey",
            "0",
            *argv,
        ],
        capture_output=True,
        text=True,
        env=environment({"CAPTURE_RESTART_PARSE": str(parsed_path)}),
        check=False,
    )
    if result.returncode != 0 or not parsed_path.is_file():
        raise RuntimeError(
            f"restart parse interception failed for {label}: exit={result.returncode} stderr={result.stderr!r}"
        )
    parsed = json.loads(parsed_path.read_text(encoding="utf-8"))
    return {
        "label": label,
        "argv": argv,
        "phase": "parse-accept",
        "parsed_timeout": parsed["parsed_timeout"],
        "parsed_verbose": parsed["parsed_verbose"],
        "parsed_debug": parsed["parsed_debug"],
    }


def template_case(reference_python: Path, label: str, mode: str) -> dict[str, Any]:
    result = subprocess.run(
        [str(reference_python), "-c", TEMPLATE_DRIVER, mode],
        capture_output=True,
        text=True,
        env=environment(),
        check=False,
    )
    return {
        "label": label,
        "argv": [],
        "phase": "template",
        "exit": result.returncode,
        "stdout": normalize(result.stdout),
        "stderr": normalize(result.stderr),
    }


def capture(reference_python: Path, output: Path) -> dict[str, Any]:
    SCRATCH_JOURNAL.mkdir(parents=True, exist_ok=True)
    provenance = checked_provenance(reference_python, output)
    convey: list[dict[str, Any]] = [
        reference_case(reference_python, "help-long", "solstone.convey.cli", "journal convey", ["--help"]),
        reference_case(reference_python, "help-short", "solstone.convey.cli", "journal convey", ["-h"]),
        reference_case(reference_python, "missing-port", "solstone.convey.cli", "journal convey", []),
        reference_case(reference_python, "port-non-integer", "solstone.convey.cli", "journal convey", ["--port", "nope"]),
        convey_accept_case(reference_python, "port-negative", ["--port", "-1"]),
        convey_accept_case(reference_python, "port-zero", ["--port", "0"]),
        convey_accept_case(reference_python, "port-repeated-last-wins", ["--port", "5", "--port", "6"]),
        convey_accept_case(reference_python, "port-equals", ["--port=5"]),
        convey_accept_case(reference_python, "port-spaced", ["--port", "5"]),
        reference_case(reference_python, "unknown-flag", "solstone.convey.cli", "journal convey", ["--port", "5", "--nonsense"]),
        reference_case(reference_python, "double-dash", "solstone.convey.cli", "journal convey", ["--port", "5", "--", "--nonsense"]),
    ]
    for label, argv in [
        ("verbose-short", ["--port", "5", "-v"]),
        ("verbose-long", ["--port", "5", "--verbose"]),
        ("debug-short", ["--port", "5", "-d"]),
        ("debug-long", ["--port", "5", "--debug"]),
        ("verbose-short-debug-short", ["--port", "5", "-v", "-d"]),
        ("verbose-short-debug-long", ["--port", "5", "-v", "--debug"]),
        ("verbose-long-debug-short", ["--port", "5", "--verbose", "-d"]),
        ("verbose-long-debug-long", ["--port", "5", "--verbose", "--debug"]),
    ]:
        convey.append(convey_accept_case(reference_python, label, argv))

    restart: list[dict[str, Any]] = [
        reference_case(reference_python, "help-long", "solstone.convey.restart", "journal restart-convey", ["--help"]),
        reference_case(reference_python, "help-short", "solstone.convey.restart", "journal restart-convey", ["-h"]),
        restart_accept_case(reference_python, "timeout-integer", ["--timeout", "30"]),
        restart_accept_case(reference_python, "timeout-decimal", ["--timeout", "30.0"]),
        restart_accept_case(reference_python, "timeout-exponent", ["--timeout", "1e1"]),
        restart_accept_case(reference_python, "timeout-dot", ["--timeout", ".5"]),
        restart_accept_case(reference_python, "timeout-negative", ["--timeout", "-1"]),
        restart_accept_case(reference_python, "timeout-zero", ["--timeout", "0"]),
        reference_case(reference_python, "timeout-non-numeric", "solstone.convey.restart", "journal restart-convey", ["--timeout", "nope"]),
        reference_case(reference_python, "timeout-missing-value", "solstone.convey.restart", "journal restart-convey", ["--timeout"]),
        restart_accept_case(reference_python, "timeout-repeated-last-wins", ["--timeout", "1", "--timeout", "2"]),
        reference_case(reference_python, "unknown-flag", "solstone.convey.restart", "journal restart-convey", ["--nonsense"]),
        reference_case(reference_python, "double-dash", "solstone.convey.restart", "journal restart-convey", ["--", "--nonsense"]),
        reference_case(reference_python, "positional", "solstone.convey.restart", "journal restart-convey", ["hello"]),
        reference_case(reference_python, "preflight-stack-down", "solstone.convey.restart", "journal restart-convey", [], "preflight"),
        reference_case(reference_python, "preflight-supervisor-spawned", "solstone.convey.restart", "journal restart-convey", [], "preflight", {"SOL_SUPERVISOR_SPAWNED": "1"}),
    ]
    for label, argv in [
        ("verbose-short", ["-v"]),
        ("verbose-long", ["--verbose"]),
        ("debug-short", ["-d"]),
        ("debug-long", ["--debug"]),
        ("verbose-short-debug-short", ["-v", "-d"]),
        ("verbose-short-debug-long", ["-v", "--debug"]),
        ("verbose-long-debug-short", ["--verbose", "-d"]),
        ("verbose-long-debug-long", ["--verbose", "--debug"]),
    ]:
        restart.append(restart_accept_case(reference_python, label, argv))
    for label, mode in [
        ("template-connect-failure", "connect_failure"),
        ("template-emit-failure", "emit_failure"),
        ("template-timeout", "timeout"),
        ("template-success-port-known", "success_known"),
        ("template-success-port-unknown", "success_unknown"),
        ("template-second-start-log-dump", "crash_logs"),
        ("template-verbose-live-stream", "verbose_success"),
    ]:
        restart.append(template_case(reference_python, label, mode))
    return {
        "schema": "convey-restart-reference-grammar/1",
        "provenance": provenance,
        "commands": {
            "convey": {"cases": convey},
            "restart_convey": {"cases": restart},
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", type=Path, default=DEFAULT_REFERENCE_PYTHON)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    output = args.output.resolve()
    reference_python = (
        args.python if args.python.is_absolute() else (ROOT / args.python)
    ).absolute()
    corpus = capture(reference_python, output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(corpus, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
