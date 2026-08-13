#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Generate the Python category registry from native describe metadata."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
CATEGORIES_DIR = ROOT / "solstone" / "observe" / "categories"
OUTPUT_PATH = ROOT / "solstone" / "observe" / "category_registry.json"


def _default_label(name: str) -> str:
    return name.replace("_", " ").title()


def _expected_registry() -> dict[str, dict[str, Any]]:
    registry: dict[str, dict[str, Any]] = {}
    for path in sorted(CATEGORIES_DIR.glob("*.md")):
        source = path.read_text(encoding="utf-8")
        frontmatter_end = source.find("\n}\n")
        if frontmatter_end < 0:
            raise ValueError(f"category frontmatter is invalid: {path}")
        metadata = json.loads(source[: frontmatter_end + 2])
        if not isinstance(metadata, dict) or not isinstance(metadata.get("description"), str):
            raise ValueError(f"category metadata is invalid: {path}")
        name = path.stem
        metadata.setdefault("output", "markdown")
        metadata.setdefault("max_output_tokens", 4096)
        metadata.setdefault("label", _default_label(name))
        metadata.setdefault("group", "Screen Analysis")
        metadata["context"] = f"observe.describe.{name}"
        prompt = source[frontmatter_end + 2 :].strip()
        if prompt:
            metadata["prompt"] = prompt
        schema_path = path.with_suffix(".schema.json")
        if schema_path.exists():
            metadata["json_schema"] = json.loads(schema_path.read_text(encoding="utf-8"))
        registry[name] = metadata
    return registry


def _native_registry() -> dict[str, dict[str, Any]]:
    completed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            "core/Cargo.toml",
            "-p",
            "solstone-core-describe",
            "--bin",
            "solstone-core-describe",
            "--locked",
            "--",
            "--category-registry",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr or completed.stdout)
    registry = json.loads(completed.stdout)
    if not isinstance(registry, dict) or not all(
        isinstance(name, str) and isinstance(metadata, dict)
        for name, metadata in registry.items()
    ):
        raise ValueError("native category registry has an invalid shape")
    return registry


def _render(registry: dict[str, dict[str, Any]]) -> str:
    return json.dumps(registry, indent=2, sort_keys=True) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)

    native = _native_registry()
    expected = _expected_registry()
    if native != expected:
        raise ValueError("native category registry does not exactly match category metadata")
    rendered = _render(native)
    if args.check:
        current = OUTPUT_PATH.read_text(encoding="utf-8") if OUTPUT_PATH.exists() else ""
        if current != rendered:
            print(
                "Observe category registry is stale: "
                "solstone/observe/category_registry.json. "
                "Run: make core-fixtures",
                file=sys.stderr,
            )
            return 1
        return 0
    OUTPUT_PATH.write_text(rendered, encoding="utf-8")
    print(f"wrote {OUTPUT_PATH.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
