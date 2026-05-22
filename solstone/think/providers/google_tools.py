# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Google SDK cogitate tools and policy gates."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any

from google.genai import types

from solstone.think.providers.cli import cogitate_sol_tool_hint

READ_FILE_MAX_CHARS = 8000
GLOB_CAP = 50
LIST_DIR_CAP = 200
GREP_STDOUT_CAP = 6000
GREP_STDERR_CAP = 1000
GREP_TIMEOUT_S = 10
SHELL_STDOUT_CAP = 6000
SHELL_STDERR_CAP = 6000
SHELL_TIMEOUT_S = 30


def _check_workspace_boundary(target: Path, allowed_roots: list[Path]) -> Path | None:
    """Resolve target and return it only if it is inside an allowed root."""
    resolved = Path(target).expanduser().resolve()
    for root in allowed_roots:
        allowed = Path(root).expanduser().resolve()
        if resolved == allowed:
            return resolved
        try:
            resolved.relative_to(allowed)
            return resolved
        except ValueError:
            continue
    return None


def _boundary_error(path: str) -> dict[str, str]:
    return {"error": f"workspace_boundary: path outside allowed roots: {path}"}


def _truncate_text(text: str, cap: int) -> tuple[str, bool]:
    if len(text) <= cap:
        return text, False
    return text[:cap] + "\n... [truncated]", True


def read_file(
    file_path: str, *, allowed_roots: list[Path] | None = None
) -> dict[str, Any]:
    """Read a UTF-8 text file inside the allowed workspace."""
    if allowed_roots is not None:
        path = _check_workspace_boundary(Path(file_path), allowed_roots)
        if path is None:
            return _boundary_error(file_path)
    else:
        path = Path(file_path).expanduser().resolve()

    if not path.exists():
        return {"error": f"not_found: {path}"}
    if not path.is_file():
        return {"error": f"not_a_file: {path}"}

    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError as exc:
        return {"error": f"decode_error: {exc}"}
    except PermissionError as exc:
        return {"error": f"permission_denied: {exc}"}
    except OSError as exc:
        return {"error": str(exc)}

    text, _truncated = _truncate_text(text, READ_FILE_MAX_CHARS)
    return {"content": text}


def glob(
    pattern: str, path: str = ".", *, allowed_roots: list[Path] | None = None
) -> dict[str, Any]:
    """Glob a pattern from an allowed base directory."""
    if allowed_roots is not None:
        base = _check_workspace_boundary(Path(path), allowed_roots)
        if base is None:
            return _boundary_error(path)
    else:
        base = Path(path).expanduser().resolve()

    if not base.exists():
        return {"error": f"not_found: {base}"}
    if not base.is_dir():
        return {"error": f"not_a_directory: {base}"}

    try:
        matches = [str(match) for match in sorted(base.glob(pattern))]
    except PermissionError as exc:
        return {"error": f"permission_denied: {exc}"}
    except OSError as exc:
        return {"error": str(exc)}
    return {"matches": matches[:GLOB_CAP], "truncated": len(matches) > GLOB_CAP}


def list_directory(
    dir_path: str, *, allowed_roots: list[Path] | None = None
) -> dict[str, Any]:
    """List directory entries inside the allowed workspace."""
    if allowed_roots is not None:
        path = _check_workspace_boundary(Path(dir_path), allowed_roots)
        if path is None:
            return _boundary_error(dir_path)
    else:
        path = Path(dir_path).expanduser().resolve()

    if not path.exists():
        return {"error": f"not_found: {path}"}
    if not path.is_dir():
        return {"error": f"not_a_directory: {path}"}

    try:
        entries = sorted(child.name for child in path.iterdir())
    except PermissionError as exc:
        return {"error": f"permission_denied: {exc}"}
    except OSError as exc:
        return {"error": str(exc)}
    return {"entries": entries[:LIST_DIR_CAP]}


def grep_search(
    pattern: str,
    path: str = ".",
    include: str = "",
    *,
    allowed_roots: list[Path] | None = None,
) -> dict[str, Any]:
    """Search with ripgrep under an allowed base path."""
    if allowed_roots is not None:
        base = _check_workspace_boundary(Path(path), allowed_roots)
        if base is None:
            return _boundary_error(path)
    else:
        base = Path(path).expanduser().resolve()

    if not base.exists():
        return {"error": f"not_found: {base}"}
    if not base.is_dir():
        return {"error": f"not_a_directory: {base}"}

    cmd = ["rg", "-n", "--max-count=20", pattern, str(base)]
    if include:
        cmd.extend(["--glob", include])

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=GREP_TIMEOUT_S,
            check=False,
        )
    except FileNotFoundError:
        return {"error": "command_not_found: rg"}
    except subprocess.TimeoutExpired:
        return {"error": f"timeout: grep_search exceeded {GREP_TIMEOUT_S}s"}
    except PermissionError as exc:
        return {"error": f"permission_denied: {exc}"}
    except OSError as exc:
        return {"error": str(exc)}

    stdout, stdout_truncated = _truncate_text(result.stdout, GREP_STDOUT_CAP)
    stderr, stderr_truncated = _truncate_text(result.stderr, GREP_STDERR_CAP)
    return {
        "output": stdout,
        "stderr": stderr,
        "truncated": stdout_truncated or stderr_truncated,
    }


def run_shell_command(command: str) -> dict[str, Any]:
    """Run a shell command after policy approval."""
    try:
        result = subprocess.run(
            ["bash", "-lc", command],
            capture_output=True,
            text=True,
            timeout=SHELL_TIMEOUT_S,
            check=False,
        )
    except FileNotFoundError:
        return {"error": "command_not_found: bash"}
    except subprocess.TimeoutExpired:
        return {"error": f"timeout: run_shell_command exceeded {SHELL_TIMEOUT_S}s"}
    except PermissionError as exc:
        return {"error": f"permission_denied: {exc}"}
    except OSError as exc:
        return {"error": str(exc)}

    stdout, _stdout_truncated = _truncate_text(result.stdout, SHELL_STDOUT_CAP)
    stderr, _stderr_truncated = _truncate_text(result.stderr, SHELL_STDERR_CAP)
    return {
        "stdout": stdout,
        "stderr": stderr,
        "returncode": result.returncode,
    }


def build_tool_declarations() -> list[types.FunctionDeclaration]:
    """Build Gemini FunctionDeclarations for the cogitate tool registry."""
    return [
        types.FunctionDeclaration(
            name="read_file",
            description="Read a single UTF-8 text file from disk.",
            parameters_json_schema={
                "type": "object",
                "properties": {"file_path": {"type": "string"}},
                "required": ["file_path"],
            },
        ),
        types.FunctionDeclaration(
            name="glob",
            description="Glob a pattern from a base directory.",
            parameters_json_schema={
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                },
                "required": ["pattern"],
            },
        ),
        types.FunctionDeclaration(
            name="list_directory",
            description="List entries in a directory.",
            parameters_json_schema={
                "type": "object",
                "properties": {"dir_path": {"type": "string"}},
                "required": ["dir_path"],
            },
        ),
        types.FunctionDeclaration(
            name="grep_search",
            description="Search files with ripgrep under a base directory.",
            parameters_json_schema={
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "include": {"type": "string"},
                },
                "required": ["pattern"],
            },
        ),
        types.FunctionDeclaration(
            name="run_shell_command",
            description=cogitate_sol_tool_hint("run_shell_command"),
            parameters_json_schema={
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
            },
        ),
    ]


def load_history(path: Path) -> list[types.Content]:
    """Load SDK chat history from disk."""
    if not path.exists():
        return []
    with path.open(encoding="utf-8") as handle:
        raw = json.load(handle)
    if hasattr(types.Content, "model_validate"):
        return [types.Content.model_validate(item) for item in raw]
    return [types.Content(**item) for item in raw]


def save_history(path: Path, contents: list[types.Content]) -> None:
    """Save SDK chat history to disk using JSON-safe Pydantic serialization."""
    path.parent.mkdir(parents=True, exist_ok=True)
    serialized = [
        content.model_dump(exclude_none=True, mode="json") for content in contents
    ]
    with path.open("w", encoding="utf-8") as handle:
        json.dump(serialized, handle)
