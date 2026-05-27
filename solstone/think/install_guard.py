# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Guard user-level sol alias ownership."""

from __future__ import annotations

import argparse
import fcntl
import os
import re
import sys
from contextlib import contextmanager
from enum import Enum
from pathlib import Path
from typing import Iterator, NamedTuple, TypedDict

try:
    import userpath  # type: ignore[import-not-found]
except ImportError:  # system python without the venv: doctor.stale_alias_symlink path
    userpath = None  # type: ignore[assignment]


WRAPPER_TEMPLATE = """\
#!/bin/bash
# {binary} — managed by 'journal config'. Edits will be overwritten.
# managed-version: 7
: "${{SOLSTONE_JOURNAL:={journal}}}"
export SOLSTONE_JOURNAL
SOL_BIN='{sol_bin}'
# Warn when pyproject.toml or uv.lock is newer than .installed.
# Skipped silently if .installed is absent.
REPO_ROOT="${{SOL_BIN%/.venv/bin/{binary}}}"
if [ -f "$REPO_ROOT/.installed" ]; then
  if [ "$REPO_ROOT/pyproject.toml" -nt "$REPO_ROOT/.installed" ] \\
     || [ "$REPO_ROOT/uv.lock" -nt "$REPO_ROOT/.installed" ]; then
    echo "{binary}: WARNING — venv is stale (pyproject.toml or uv.lock changed since last install). Run: cd $REPO_ROOT && make install" >&2
  fi
fi
if [ ! -x "$SOL_BIN" ]; then
    printf '{binary}: venv binary missing or not executable: %s\\n' "$SOL_BIN" >&2
    exit 127
fi
exec "$SOL_BIN" "$@"
"""

WRAPPER_MARKER = "# managed-version: 7"
WRAPPER_VERSION = 7

_RE_MARKER = re.compile(r"(?m)^# managed-version: (?P<version>[1-7])$")
_RE_JOURNAL = re.compile(r'(?m)^: "\$\{SOLSTONE_JOURNAL:=(?P<journal>[^\n]*)\}"$')
_RE_SOL_BIN = re.compile(r"(?m)^SOL_BIN='(?P<sol_bin>(?:[^']|'\\'')*)'$")

_INVALID_JOURNAL_CHARS = ("$", "`", '"', "\\", "\n")
_WRAPPER_BINARIES = ("sol", "journal")


class AliasState(Enum):
    WORKTREE = "worktree"
    ABSENT = "absent"
    OWNED = "owned"
    CROSS_REPO = "cross_repo"
    DANGLING = "dangling"
    FOREIGN = "foreign"


class ParsedWrapper(TypedDict):
    journal: str
    sol_bin: str
    version: int


class _PathSnapshot(NamedTuple):
    kind: str
    content: bytes | None = None
    mode: int | None = None
    target: str | None = None


def alias_paths() -> dict[str, Path]:
    return {
        binary: Path.home() / ".local" / "bin" / binary for binary in _WRAPPER_BINARIES
    }


def alias_path() -> Path:
    return alias_paths()["sol"]


def journal_alias_path() -> Path:
    return alias_paths()["journal"]


def expected_target(curdir: Path, binary: str = "sol") -> Path:
    _validate_binary(binary)
    return curdir / ".venv" / "bin" / binary


def _validate_binary(binary: str) -> None:
    if binary not in _WRAPPER_BINARIES:
        raise ValueError(f"unsupported wrapper binary: {binary}")


def render_wrapper(journal: str, sol_bin: str, binary: str) -> str:
    """Render the managed wrapper for ~/.local/bin/<binary>."""
    _validate_binary(binary)
    escaped_sol_bin = sol_bin.replace("'", "'\\''")
    return WRAPPER_TEMPLATE.format(
        binary=binary,
        journal=journal,
        sol_bin=escaped_sol_bin,
    )


def parse_wrapper(content: str) -> ParsedWrapper | None:
    """Return embedded paths if the content is a managed wrapper."""
    marker_match = _RE_MARKER.search(content)
    if not marker_match:
        return None
    journal_match = _RE_JOURNAL.search(content)
    sol_bin_match = _RE_SOL_BIN.search(content)
    if not journal_match or not sol_bin_match:
        return None
    return {
        "journal": journal_match.group("journal"),
        "sol_bin": sol_bin_match.group("sol_bin").replace("'\\''", "'"),
        "version": int(marker_match.group("version")),
    }


def _snapshot_path(path: Path) -> _PathSnapshot:
    if path.is_symlink():
        return _PathSnapshot("symlink", target=os.readlink(path))
    if path.exists():
        return _PathSnapshot(
            "file",
            content=path.read_bytes(),
            mode=path.stat().st_mode & 0o777,
        )
    return _PathSnapshot("absent")


def _restore_path(path: Path, snapshot: _PathSnapshot) -> None:
    if path.exists() or path.is_symlink():
        path.unlink()
    if snapshot.kind == "absent":
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    if snapshot.kind == "symlink":
        assert snapshot.target is not None
        path.symlink_to(snapshot.target)
        return
    if snapshot.kind == "file":
        assert snapshot.content is not None
        path.write_bytes(snapshot.content)
        os.chmod(path, snapshot.mode if snapshot.mode is not None else 0o755)
        return
    raise RuntimeError(f"unknown wrapper snapshot kind: {snapshot.kind}")


def _staged_path(path: Path, index: int) -> Path:
    return path.with_name(f".{path.name}.tmp-{os.getpid()}-{index}")


def write_wrappers_atomically(contents: dict[Path, str]) -> None:
    """Atomically write one or more wrappers, rolling back all targets on failure."""
    snapshots = {path: _snapshot_path(path) for path in contents}
    staged: dict[Path, Path] = {}
    committed: list[Path] = []
    try:
        for index, (path, content) in enumerate(contents.items()):
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp_path = _staged_path(path, index)
            if tmp_path.exists() or tmp_path.is_symlink():
                tmp_path.unlink()
            tmp_path.write_text(content, encoding="utf-8")
            os.chmod(tmp_path, 0o755)
            staged[path] = tmp_path
        for path, tmp_path in staged.items():
            os.replace(tmp_path, path)
            committed.append(path)
    except Exception:
        for path in contents:
            try:
                _restore_path(path, snapshots[path])
            except Exception:
                pass
        raise
    finally:
        for path, tmp_path in staged.items():
            if path not in committed and (tmp_path.exists() or tmp_path.is_symlink()):
                try:
                    tmp_path.unlink()
                except OSError:
                    pass


def install_wrappers(
    journal: str,
    sol_bins: dict[str, str],
    *,
    paths: dict[str, Path] | None = None,
) -> None:
    """Install both managed wrappers under one lock."""
    if paths is None:
        paths = alias_paths()
    contents = {
        paths[binary]: render_wrapper(journal, sol_bins[binary], binary)
        for binary in paths
    }
    with wrapper_lock():
        write_wrappers_atomically(contents)


def _install_wrappers_unlocked(
    journal: str,
    sol_bins: dict[str, str],
    *,
    paths: dict[str, Path] | None = None,
) -> None:
    if paths is None:
        paths = alias_paths()
    contents = {
        paths[binary]: render_wrapper(journal, sol_bins[binary], binary)
        for binary in paths
    }
    write_wrappers_atomically(contents)


@contextmanager
def wrapper_lock(lock_path: Path | None = None) -> Iterator[None]:
    """Hold an exclusive advisory lock while rewriting the wrapper."""
    if lock_path is None:
        lock_path = Path.home() / ".local" / "bin" / ".sol.lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with open(lock_path, "w", encoding="utf-8") as lock_fd:
        try:
            fcntl.flock(lock_fd.fileno(), fcntl.LOCK_EX)
            yield
        finally:
            fcntl.flock(lock_fd.fileno(), fcntl.LOCK_UN)


def validate_journal_path_for_wrapper(path: str) -> None:
    """Reject shell-active characters that would corrupt wrapper embedding."""
    for char in _INVALID_JOURNAL_CHARS:
        if char in path:
            raise ValueError(
                f"journal path contains shell-active character {char!r}: {path!r}"
            )


def check_alias(curdir: Path, binary: str = "sol") -> tuple[AliasState, Path | None]:
    _validate_binary(binary)
    if (curdir / ".git").is_file():
        return AliasState.WORKTREE, None

    alias = alias_paths()[binary]
    if not alias.exists() and not alias.is_symlink():
        return AliasState.ABSENT, None

    if alias.is_symlink():
        target = Path(os.readlink(alias))
        if not target.is_absolute():
            target = alias.parent / target
        target = target.resolve()
        if not target.exists():
            return AliasState.DANGLING, target
        if target == expected_target(curdir, binary).resolve():
            return AliasState.OWNED, target
        packaged_target = (Path(sys.executable).parent / binary).resolve()
        if target == packaged_target:
            return AliasState.OWNED, target
        return AliasState.CROSS_REPO, target

    try:
        content = alias.read_text(encoding="utf-8")
    except OSError:
        return AliasState.FOREIGN, None

    parsed = parse_wrapper(content)
    if parsed is None:
        return AliasState.FOREIGN, None

    target = Path(parsed["sol_bin"])
    if target.resolve() == expected_target(curdir, binary).resolve():
        return AliasState.OWNED, target.resolve()
    packaged_target = (Path(sys.executable).parent / binary).resolve()
    if target.resolve() == packaged_target:
        return AliasState.OWNED, target.resolve()
    return AliasState.FOREIGN, None


def _current_journal_for_alias() -> str:
    """Return the journal path a wrapper install would embed right now."""
    from solstone.think import utils as think_utils
    from solstone.think.user_config import default_journal

    try:
        path, _ = think_utils.get_journal_info()
    except getattr(think_utils, "SolstoneNotConfigured", RuntimeError):
        path = default_journal()
    return path


def _alias_results(curdir: Path) -> list[tuple[str, Path, AliasState, Path | None]]:
    return [
        (binary, alias, *check_alias(curdir, binary))
        for binary, alias in alias_paths().items()
    ]


def _first_alias_state(
    curdir: Path,
    states: set[AliasState],
) -> tuple[str, Path, AliasState, Path | None] | None:
    for result in _alias_results(curdir):
        if result[2] in states:
            return result
    return None


def _wrapper_is_current(curdir: Path, binary: str, alias: Path) -> bool:
    if alias.is_symlink():
        return False
    try:
        content = alias.read_text(encoding="utf-8")
    except OSError:
        return False
    parsed = parse_wrapper(content)
    if parsed is None:
        return False
    return (
        parsed["journal"] == _current_journal_for_alias()
        and parsed["sol_bin"] == str(expected_target(curdir, binary))
        and parsed["version"] == WRAPPER_VERSION
        and (alias.stat().st_mode & 0o111) == 0o111
    )


def check_alias_detail(curdir: Path) -> tuple[AliasState, str]:
    """Return alias state plus the cmd_check token for owned aliases."""
    results = _alias_results(curdir)
    if results[0][2] is AliasState.WORKTREE:
        return AliasState.WORKTREE, AliasState.WORKTREE.value

    for _binary, _alias, state, _other_target in results:
        if state in {
            AliasState.CROSS_REPO,
            AliasState.DANGLING,
            AliasState.FOREIGN,
        }:
            return state, state.value

    if all(state is AliasState.ABSENT for _binary, _alias, state, _target in results):
        return AliasState.ABSENT, "fresh"

    if all(
        state is AliasState.OWNED and _wrapper_is_current(curdir, binary, alias)
        for binary, alias, state, _target in results
    ):
        return AliasState.OWNED, "current"

    return AliasState.OWNED, "upgrade"


def format_error(
    state: AliasState,
    curdir: Path,
    alias: Path,
    other_target: Path | None,
    *,
    allow_force: bool = False,
) -> str:
    if state is AliasState.WORKTREE:
        return (
            f"ERROR: refusing to run from a git worktree ({curdir}). "
            "Run from the primary clone."
        )

    if state is AliasState.CROSS_REPO:
        installed = f"  installed:  {other_target}"
    elif state is AliasState.DANGLING:
        installed = f"  installed:  dangling: {other_target} does not exist"
    else:
        installed = "  installed:  not a symlink"

    alias_label = _alias_label(alias)
    lines = [
        f"ERROR: Another solstone install owns {alias_label}.",
        f"  this repo:  {curdir}",
        installed,
        "Run 'journal setup' from the installed repo first,",
        f"or remove {alias_label} manually if that repo is gone.",
    ]
    if allow_force:
        lines.append(
            "Rerun 'python -m solstone.think.install_guard install --force' only if you intend to replace it from this repo."
        )
    return "\n".join(lines)


def _alias_label(alias: Path) -> str:
    local_bin = Path.home() / ".local" / "bin"
    if alias.parent == local_bin:
        return f"~/.local/bin/{alias.name}"
    return str(alias)


def _print_error(
    state: AliasState,
    curdir: Path,
    alias: Path,
    other_target: Path | None,
    *,
    allow_force: bool = False,
) -> None:
    sys.stderr.write(
        format_error(
            state,
            curdir,
            alias,
            other_target,
            allow_force=allow_force,
        )
        + "\n"
    )


def _ensure_user_bin_on_path(user_bin: Path) -> None:
    # `userpath` is imported at module top with an ImportError guard, so this
    # module is importable from system python (where doctor runs) even when
    # `userpath` is not installed. This code path is only reached via
    # `cmd_install`, which only runs from inside the venv where `userpath` is
    # present; if somehow reached without `userpath`, we want a hard failure.
    if userpath is None:
        raise RuntimeError("userpath is not available; run `make install` first")
    user_bin_str = str(user_bin)
    try:
        if userpath.in_current_path(user_bin_str):
            print("path: ~/.local/bin already on PATH")
            return
        if userpath.append(user_bin_str, app_name="solstone", all_shells=True):
            if userpath.need_shell_restart(user_bin_str):
                print(
                    "path: added ~/.local/bin to shell PATH — restart your shell or run 'exec $SHELL -l' to pick it up"
                )
            else:
                print("path: added ~/.local/bin to shell PATH")
            return
        print(
            'path: could not auto-add ~/.local/bin to PATH — add this line to your shell rc manually: export PATH="$HOME/.local/bin:$PATH"'
        )
    except Exception as exc:
        print(
            f'path: could not auto-add ~/.local/bin to PATH ({type(exc).__name__}: {exc}) — add this line to your shell rc manually: export PATH="$HOME/.local/bin:$PATH"'
        )


def cmd_check(curdir: Path) -> int:
    state, token = check_alias_detail(curdir)

    if state is AliasState.WORKTREE:
        print("worktree")
        alias = alias_paths()["sol"]
        _print_error(state, curdir, alias, None)
        return 1
    if state is AliasState.ABSENT:
        print(token)
        return 0
    if state is AliasState.OWNED:
        print(token)
        return 0
    if state is AliasState.CROSS_REPO:
        print("cross_repo")
        result = _first_alias_state(curdir, {AliasState.CROSS_REPO})
        assert result is not None
        _binary, alias, _state, other = result
        _print_error(state, curdir, alias, other, allow_force=True)
        return 1
    if state is AliasState.DANGLING:
        print("dangling")
        result = _first_alias_state(curdir, {AliasState.DANGLING})
        assert result is not None
        _binary, alias, _state, other = result
        _print_error(state, curdir, alias, other, allow_force=True)
        return 1
    if state is AliasState.FOREIGN:
        print("not_symlink")
        result = _first_alias_state(curdir, {AliasState.FOREIGN})
        assert result is not None
        _binary, alias, _state, other = result
        _print_error(state, curdir, alias, other, allow_force=True)
        return 1

    return 1


def cmd_install(curdir: Path, *, force: bool = False) -> int:
    blocked_states = {
        AliasState.CROSS_REPO,
        AliasState.DANGLING,
        AliasState.FOREIGN,
    }
    worktree_result = _first_alias_state(curdir, {AliasState.WORKTREE})

    if worktree_result is not None:
        _binary, alias, state, other_target = worktree_result
        _print_error(state, curdir, alias, other_target)
        return 1

    blocked_result = _first_alias_state(curdir, blocked_states)
    if blocked_result is not None and not force:
        _binary, alias, state, other_target = blocked_result
        _print_error(state, curdir, alias, other_target, allow_force=True)
        return 1

    journal = _current_journal_for_alias()
    try:
        validate_journal_path_for_wrapper(journal)
    except ValueError as exc:
        print(f"refused: {exc}", file=sys.stderr)
        return 1

    aliases = alias_paths()
    sol_bins = {binary: str(expected_target(curdir, binary)) for binary in aliases}
    with wrapper_lock():
        locked_worktree = _first_alias_state(curdir, {AliasState.WORKTREE})
        if locked_worktree is not None:
            _binary, alias, locked_state, locked_other_target = locked_worktree
            _print_error(locked_state, curdir, alias, locked_other_target)
            return 1
        locked_blocked = _first_alias_state(curdir, blocked_states)
        if locked_blocked is not None and not force:
            _binary, alias, locked_state, locked_other_target = locked_blocked
            _print_error(
                locked_state,
                curdir,
                alias,
                locked_other_target,
                allow_force=True,
            )
            return 1
        try:
            _install_wrappers_unlocked(journal, sol_bins, paths=aliases)
        except OSError as exc:
            print(f"ERROR: failed to install wrappers: {exc}", file=sys.stderr)
            return 1

    print("installed")
    _ensure_user_bin_on_path(aliases["sol"].parent)
    return 0


def cmd_uninstall(curdir: Path) -> int:
    blocked_states = {
        AliasState.CROSS_REPO,
        AliasState.DANGLING,
        AliasState.FOREIGN,
    }
    worktree_result = _first_alias_state(curdir, {AliasState.WORKTREE})

    if worktree_result is not None:
        _binary, alias, state, other_target = worktree_result
        _print_error(state, curdir, alias, other_target)
        return 1
    results = _alias_results(curdir)
    if all(state is AliasState.ABSENT for _binary, _alias, state, _target in results):
        print("absent")
        return 0

    blocked_result = _first_alias_state(curdir, blocked_states)
    if blocked_result is not None:
        _binary, alias, state, other_target = blocked_result
        _print_error(state, curdir, alias, other_target)
        return 1

    with wrapper_lock():
        locked_worktree = _first_alias_state(curdir, {AliasState.WORKTREE})
        if locked_worktree is not None:
            _binary, alias, locked_state, locked_other_target = locked_worktree
            _print_error(locked_state, curdir, alias, locked_other_target)
            return 1
        locked_results = _alias_results(curdir)
        if all(
            state is AliasState.ABSENT
            for _binary, _alias, state, _target in locked_results
        ):
            print("absent")
            return 0
        locked_blocked = _first_alias_state(curdir, blocked_states)
        if locked_blocked is not None:
            _binary, alias, locked_state, locked_other_target = locked_blocked
            _print_error(locked_state, curdir, alias, locked_other_target)
            return 1
        for _binary, alias, state, _target in locked_results:
            if state is AliasState.OWNED:
                alias.unlink()

    print("uninstalled")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m solstone.think.install_guard")
    subparsers = parser.add_subparsers(dest="cmd", required=True)
    subparsers.add_parser("check")
    install_parser = subparsers.add_parser("install")
    install_parser.add_argument("--force", action="store_true")
    subparsers.add_parser("uninstall")
    args = parser.parse_args(argv)
    curdir = Path.cwd().resolve()
    if args.cmd == "check":
        return cmd_check(curdir)
    if args.cmd == "install":
        return cmd_install(curdir, force=args.force)
    return cmd_uninstall(curdir)


if __name__ == "__main__":
    sys.exit(main())
