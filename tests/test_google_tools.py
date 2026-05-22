# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# AC 5: Read-only policy denies write tools while allowing read tools.
# AC 6: Read-only policy discriminates shell commands by sol invocation regex.
# AC 7: Policy regex edge cases are pinned.
# AC 8: Write mode allows all tools.
# AC 9: Workspace-boundary in-scope reads return content.
# AC 10: Workspace-boundary out-of-scope reads return keyed errors.
# AC 11: Workspace-boundary helper handles traversal and symlinks.
# AC 12: Curated history round-trips thought_signature bytes.
# AC 22: Tool error vocabulary is keyed.

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest
from google.genai import types

from solstone.think.cogitate_policy import _SOL_INVOCATION_RE, CogitatePolicy
from solstone.think.providers import google_tools


# AC 7: Policy regex edge-case pin.
@pytest.mark.parametrize(
    ("command", "allowed"),
    [
        ("sol call activities list", True),
        ("sol status", True),
        ("echo hi | sol call journal search", True),
        ("cd /tmp && sol call entities list", True),
        ("rm -rf /tmp/notarealfile", False),
        ("python -c 'print(1)'", False),
        ("absol call activities list", False),
        ("SOL call activities list", False),
    ],
)
def test_policy_regex_edge_cases(command: str, allowed: bool) -> None:
    assert bool(_SOL_INVOCATION_RE.search(command)) is allowed


# AC 5, 6, 8: Policy semantics for read-only and write modes.
def test_cogitate_policy_readonly_and_write_modes(tmp_path: Path) -> None:
    readonly = CogitatePolicy(write=False, allowed_roots=[tmp_path])
    assert readonly.check("write_file", {"file_path": "x"})[0] is False
    assert readonly.check("replace", {"file_path": "x"})[0] is False
    assert readonly.check("read_file", {"file_path": "x"}) == (True, "ok")
    assert readonly.check(
        "run_shell_command", {"command": "sol call activities list"}
    ) == (
        True,
        "ok",
    )
    allowed, reason = readonly.check(
        "run_shell_command", {"command": "rm -rf /tmp/notarealfile"}
    )
    assert allowed is False
    assert reason.startswith("policy_deny:")

    write = CogitatePolicy(write=True, allowed_roots=[tmp_path])
    assert write.check("write_file", {"file_path": "x"}) == (True, "ok")
    assert write.check(
        "run_shell_command", {"command": "rm -rf /tmp/notarealfile"}
    ) == (
        True,
        "ok",
    )


# AC 11: Workspace-boundary helper table.
@pytest.mark.parametrize("case_index", range(12))
def test_workspace_boundary_table(tmp_path: Path, case_index: int) -> None:
    root = tmp_path / "root"
    other_root = tmp_path / "other"
    outside = tmp_path / "outside"
    root.mkdir()
    other_root.mkdir()
    outside.mkdir()
    (root / "file.txt").write_text("ok", encoding="utf-8")
    (root / "nested").mkdir()
    (root / "nested" / "file.txt").write_text("nested", encoding="utf-8")
    (other_root / "file.txt").write_text("other", encoding="utf-8")
    (outside / "file.txt").write_text("outside", encoding="utf-8")
    (tmp_path / "root_evil").mkdir()
    (tmp_path / "root_evil" / "file.txt").write_text("evil", encoding="utf-8")

    symlink_out = root / "symlink-out"
    symlink_in = root / "symlink-in"
    symlink_out.symlink_to(outside / "file.txt")
    symlink_in.symlink_to(root / "file.txt")

    cases = [
        (root, True),
        (root / "file.txt", True),
        (root / "nested" / "file.txt", True),
        (root / "nested" / ".." / "file.txt", True),
        (other_root, True),
        (other_root / "file.txt", True),
        (root / "missing.txt", True),
        (outside / "file.txt", False),
        (tmp_path / "root_evil" / "file.txt", False),
        (symlink_out, False),
        (symlink_in, True),
        (tmp_path / "missing-outside.txt", False),
    ]
    target, expected = cases[case_index]
    result = google_tools._check_workspace_boundary(target, [root, other_root])
    assert (result is not None) is expected


# AC 9, 10: Tool-level workspace boundary behavior.
def test_read_file_workspace_boundary(tmp_path: Path) -> None:
    allowed = tmp_path / "allowed"
    outside = tmp_path / "outside"
    allowed.mkdir()
    outside.mkdir()
    in_scope = allowed / "note.txt"
    out_scope = outside / "note.txt"
    in_scope.write_text("hello", encoding="utf-8")
    out_scope.write_text("secret", encoding="utf-8")

    assert google_tools.read_file(str(in_scope), allowed_roots=[allowed]) == {
        "content": "hello"
    }
    denied = google_tools.read_file(str(out_scope), allowed_roots=[allowed])
    assert denied["error"].startswith("workspace_boundary:")


# AC 22: Tool error vocabulary is keyed.
@pytest.mark.parametrize(
    "case_name",
    [
        "not_found",
        "not_a_file",
        "not_a_directory",
        "workspace_boundary",
        "command_not_found",
        "timeout",
        "decode_error",
        "permission_denied",
    ],
)
def test_tool_error_vocabulary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, case_name: str
) -> None:
    root = tmp_path / "root"
    outside = tmp_path / "outside"
    root.mkdir()
    outside.mkdir()

    if case_name == "not_found":
        result = google_tools.read_file(str(root / "missing.txt"), allowed_roots=[root])
    elif case_name == "not_a_file":
        result = google_tools.read_file(str(root), allowed_roots=[root])
    elif case_name == "not_a_directory":
        file_path = root / "file.txt"
        file_path.write_text("ok", encoding="utf-8")
        result = google_tools.list_directory(str(file_path), allowed_roots=[root])
    elif case_name == "workspace_boundary":
        file_path = outside / "file.txt"
        file_path.write_text("no", encoding="utf-8")
        result = google_tools.read_file(str(file_path), allowed_roots=[root])
    elif case_name == "command_not_found":
        monkeypatch.setattr(
            google_tools.subprocess,
            "run",
            lambda *_args, **_kwargs: (_ for _ in ()).throw(FileNotFoundError()),
        )
        result = google_tools.grep_search("x", str(root), allowed_roots=[root])
    elif case_name == "timeout":
        monkeypatch.setattr(
            google_tools.subprocess,
            "run",
            lambda *_args, **_kwargs: (_ for _ in ()).throw(
                subprocess.TimeoutExpired(cmd="bash", timeout=30)
            ),
        )
        result = google_tools.run_shell_command("sleep 60")
    elif case_name == "decode_error":
        binary = root / "binary.txt"
        binary.write_bytes(b"\xff\xfe\xfd")
        result = google_tools.read_file(str(binary), allowed_roots=[root])
    else:
        denied = root / "denied.txt"
        denied.write_text("no", encoding="utf-8")
        monkeypatch.setattr(
            Path,
            "read_text",
            lambda *_args, **_kwargs: (_ for _ in ()).throw(PermissionError("no")),
        )
        result = google_tools.read_file(str(denied), allowed_roots=[root])

    assert result["error"].startswith(f"{case_name}:")


# AC 22: grep_search zero matches are not an error.
def test_grep_search_zero_matches_shape(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "root"
    root.mkdir()
    monkeypatch.setattr(
        google_tools.subprocess,
        "run",
        lambda *_args, **_kwargs: subprocess.CompletedProcess(
            args=["rg"], returncode=1, stdout="", stderr=""
        ),
    )

    assert google_tools.grep_search("missing", str(root), allowed_roots=[root]) == {
        "output": "",
        "stderr": "",
        "truncated": False,
    }


# AC 19, 22: Hard limits are applied to tool success shapes.
def test_tool_hard_limits(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    root = tmp_path / "root"
    root.mkdir()
    text_file = root / "large.txt"
    text_file.write_text(
        "a" * (google_tools.READ_FILE_MAX_CHARS + 50), encoding="utf-8"
    )
    for index in range(google_tools.GLOB_CAP + 5):
        (root / f"item-{index}.txt").write_text("x", encoding="utf-8")
    for index in range(google_tools.LIST_DIR_CAP + 5):
        (root / f"entry-{index}.md").write_text("x", encoding="utf-8")
    monkeypatch.setattr(
        google_tools.subprocess,
        "run",
        lambda *_args, **_kwargs: subprocess.CompletedProcess(
            args=["rg"],
            returncode=0,
            stdout="o" * (google_tools.GREP_STDOUT_CAP + 10),
            stderr="e" * (google_tools.GREP_STDERR_CAP + 10),
        ),
    )

    read_result = google_tools.read_file(str(text_file), allowed_roots=[root])
    assert len(read_result["content"]) <= google_tools.READ_FILE_MAX_CHARS + 16
    assert read_result["content"].endswith("... [truncated]")
    glob_result = google_tools.glob("*.txt", str(root), allowed_roots=[root])
    assert len(glob_result["matches"]) == google_tools.GLOB_CAP
    assert glob_result["truncated"] is True
    list_result = google_tools.list_directory(str(root), allowed_roots=[root])
    assert len(list_result["entries"]) == google_tools.LIST_DIR_CAP
    grep_result = google_tools.grep_search("x", str(root), allowed_roots=[root])
    assert grep_result["truncated"] is True


# AC 12: Session resume round-trip with thought_signature bytes.
def test_history_serialization_round_trips_thought_signature(tmp_path: Path) -> None:
    history_path = tmp_path / ".cache" / "cogitate-history" / "sess.json"
    content = types.Content(
        role="model",
        parts=[
            types.Part(
                text="thinking",
                thought=True,
                thought_signature=b"signature-bytes",
            )
        ],
    )

    google_tools.save_history(history_path, [content])
    loaded = google_tools.load_history(history_path)

    assert loaded[0].parts[0].thought_signature == b"signature-bytes"


# AC 12: Missing session history starts fresh.
def test_load_history_missing_file_returns_empty(tmp_path: Path) -> None:
    assert google_tools.load_history(tmp_path / "missing.json") == []


# AC 2, 6: FunctionDeclaration schemas expose fixed tool parameter names.
def test_tool_declarations_use_fixed_parameter_names() -> None:
    declarations = {decl.name: decl for decl in google_tools.build_tool_declarations()}

    assert set(declarations) == {
        "read_file",
        "glob",
        "list_directory",
        "grep_search",
        "run_shell_command",
    }
    assert declarations["read_file"].parameters_json_schema["required"] == ["file_path"]
    assert declarations["list_directory"].parameters_json_schema["required"] == [
        "dir_path"
    ]
    assert declarations["run_shell_command"].parameters_json_schema["required"] == [
        "command"
    ]
    assert "run_shell_command" in declarations["run_shell_command"].description
    assert (
        "Do not invent or call a tool literally named `sol`."
        in declarations["run_shell_command"].description
    )
