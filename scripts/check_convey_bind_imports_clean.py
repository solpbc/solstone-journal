# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Guard that Convey bind imports stay clear of heavy native stacks."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HEAVY = (
    "numpy",
    "scipy",
    "sklearn",
    "onnxruntime",
    "pyarrow",
    "transformers",
    "cv2",
    "mlx",
    "mlx_lm",
    "av",
    "faster_whisper",
    "torch",
    "pandas",
    "huggingface_hub",
)

_INJECT_ENV = "SOLSTONE_CONVEY_BIND_GUARD_INJECT_HEAVY_MODULE"
_SENTINEL = "CONVEY_BIND_IMPORTS_CLEAN_LEAKED="

CHILD = rf"""
import importlib
import json
import os
import sys

payload = json.loads(sys.argv[1])
root = payload["root"]
if root not in sys.path:
    sys.path.insert(0, root)

inject = os.environ.get("{_INJECT_ENV}")
if inject:
    importlib.import_module(inject)

from solstone.convey import create_app
create_app()
from solstone.apps.events import discover_handlers
discover_handlers()

leaked = sorted(set(payload["heavy"]) & set(sys.modules))
print("{_SENTINEL}" + json.dumps(leaked))
"""


def _run_probe(
    root: Path, inject_heavy_module: str | None
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env.setdefault("SOLSTONE_JOURNAL", str(root / "tests" / "fixtures" / "journal"))
    env["PYTHONPATH"] = (
        str(root)
        if not env.get("PYTHONPATH")
        else str(root) + os.pathsep + env["PYTHONPATH"]
    )
    if inject_heavy_module:
        env[_INJECT_ENV] = inject_heavy_module

    payload = {
        "root": str(root),
        "heavy": HEAVY,
    }
    return subprocess.run(
        [sys.executable, "-c", CHILD, json.dumps(payload)],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
    )


def _parse_leaked(stdout: str) -> list[str] | None:
    for line in stdout.splitlines():
        if not line.startswith(_SENTINEL):
            continue
        return list(json.loads(line[len(_SENTINEL) :]))
    return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Assert Convey bind path does not import heavy modules."
    )
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--inject-heavy-module")
    args = parser.parse_args(argv)

    root = args.root.resolve()
    result = _run_probe(root, args.inject_heavy_module)
    leaked = _parse_leaked(result.stdout)
    if result.returncode != 0:
        print(
            f"convey-bind-imports-clean: FAIL probe exited {result.returncode}",
            file=sys.stderr,
        )
        if result.stdout:
            print("--- stdout ---", file=sys.stderr)
            print(result.stdout, file=sys.stderr, end="")
        if result.stderr:
            print("--- stderr ---", file=sys.stderr)
            print(result.stderr, file=sys.stderr, end="")
        return 1
    if leaked is None:
        print(
            "convey-bind-imports-clean: FAIL probe did not report leaked modules",
            file=sys.stderr,
        )
        if result.stdout:
            print("--- stdout ---", file=sys.stderr)
            print(result.stdout, file=sys.stderr, end="")
        return 1
    if leaked:
        print(
            f"convey-bind-imports-clean: FAIL leaked heavy modules: {leaked}",
            file=sys.stderr,
        )
        return 1

    print("convey-bind-imports-clean: pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
