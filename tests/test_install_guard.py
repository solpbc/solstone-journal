# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from unittest.mock import Mock

import pytest

from solstone.think import install_guard

V1_WRAPPER_TEMPLATE = """\
#!/bin/sh
# sol — managed by 'sol config'. Edits will be overwritten.
# managed-version: 1
: "${{SOLSTONE_JOURNAL:={journal}}}"
export SOLSTONE_JOURNAL
SOL_BIN='{sol_bin}'
if [ ! -x "$SOL_BIN" ]; then
    printf 'sol: venv binary missing or not executable: %s\\n' "$SOL_BIN" >&2
    exit 127
fi
exec "$SOL_BIN" "$@"
"""

V2_WRAPPER_TEMPLATE = """\
#!/bin/sh
# sol — managed by 'sol config'. Edits will be overwritten.
# managed-version: 2
: "${{SOLSTONE_JOURNAL:={journal}}}"
export SOLSTONE_JOURNAL
SOL_BIN='{sol_bin}'
if [ ! -x "$SOL_BIN" ]; then
    printf 'sol: venv binary missing or not executable: %s\\n' "$SOL_BIN" >&2
    exit 127
fi
if [ "$1" = "supervisor" ]; then
    mkdir -p "$SOLSTONE_JOURNAL/health"
    exec >>"$SOLSTONE_JOURNAL/health/service.log" 2>&1
fi
exec "$SOL_BIN" "$@"
"""

V3_WRAPPER_TEMPLATE = """\
#!/bin/sh
# sol — managed by 'sol config'. Edits will be overwritten.
# managed-version: 3
: "${{SOLSTONE_JOURNAL:={journal}}}"
export SOLSTONE_JOURNAL
SOL_BIN='{sol_bin}'
if [ ! -x "$SOL_BIN" ]; then
    printf 'sol: venv binary missing or not executable: %s\\n' "$SOL_BIN" >&2
    exit 127
fi
if [ "$1" = "supervisor" ]; then
    mkdir -p "$SOLSTONE_JOURNAL/health"
    exec >>"$SOLSTONE_JOURNAL/health/service.log" 2>&1
    export PYTHONUNBUFFERED=1
fi
exec "$SOL_BIN" "$@"
"""

V4_WRAPPER_TEMPLATE = """\
#!/bin/bash
# sol — managed by 'sol config'. Edits will be overwritten.
# managed-version: 4
: "${{SOLSTONE_JOURNAL:={journal}}}"
export SOLSTONE_JOURNAL
SOL_BIN='{sol_bin}'
if [ ! -x "$SOL_BIN" ]; then
    printf 'sol: venv binary missing or not executable: %s\\n' "$SOL_BIN" >&2
    exit 127
fi
if [ "$1" = "supervisor" ]; then
    mkdir -p "$SOLSTONE_JOURNAL/health"
    exec > >(tee -a "$SOLSTONE_JOURNAL/health/service.log") 2>&1
    export PYTHONUNBUFFERED=1
fi
exec "$SOL_BIN" "$@"
"""

V5_WRAPPER_TEMPLATE = """\
#!/bin/bash
# sol — managed by 'sol config'. Edits will be overwritten.
# managed-version: 5
: "${{SOLSTONE_JOURNAL:={journal}}}"
export SOLSTONE_JOURNAL
SOL_BIN='{sol_bin}'
# Warn when pyproject.toml or uv.lock is newer than .installed.
# Skipped silently if .installed is absent.
REPO_ROOT="${{SOL_BIN%/.venv/bin/sol}}"
if [ -f "$REPO_ROOT/.installed" ]; then
  if [ "$REPO_ROOT/pyproject.toml" -nt "$REPO_ROOT/.installed" ] \\
     || [ "$REPO_ROOT/uv.lock" -nt "$REPO_ROOT/.installed" ]; then
    echo "sol: WARNING — venv is stale (pyproject.toml or uv.lock changed since last install). Run: cd $REPO_ROOT && make install" >&2
  fi
fi
if [ ! -x "$SOL_BIN" ]; then
    printf 'sol: venv binary missing or not executable: %s\\n' "$SOL_BIN" >&2
    exit 127
fi
if [ "$1" = "supervisor" ]; then
    mkdir -p "$SOLSTONE_JOURNAL/health"
    exec > >(tee -a "$SOLSTONE_JOURNAL/health/service.log") 2>&1
    export PYTHONUNBUFFERED=1
fi
exec "$SOL_BIN" "$@"
"""

V6_WRAPPER_TEMPLATE = """\
#!/bin/bash
# sol — managed by 'sol config'. Edits will be overwritten.
# managed-version: 6
: "${{SOLSTONE_JOURNAL:={journal}}}"
export SOLSTONE_JOURNAL
SOL_BIN='{sol_bin}'
# Warn when pyproject.toml or uv.lock is newer than .installed.
# Skipped silently if .installed is absent.
REPO_ROOT="${{SOL_BIN%/.venv/bin/sol}}"
if [ -f "$REPO_ROOT/.installed" ]; then
  if [ "$REPO_ROOT/pyproject.toml" -nt "$REPO_ROOT/.installed" ] \\
     || [ "$REPO_ROOT/uv.lock" -nt "$REPO_ROOT/.installed" ]; then
    echo "sol: WARNING — venv is stale (pyproject.toml or uv.lock changed since last install). Run: cd $REPO_ROOT && make install" >&2
  fi
fi
if [ ! -x "$SOL_BIN" ]; then
    printf 'sol: venv binary missing or not executable: %s\\n' "$SOL_BIN" >&2
    exit 127
fi
exec "$SOL_BIN" "$@"
"""


@pytest.fixture
def home_root(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    return home


def make_repo(tmp_path: Path, *, worktree: bool = False) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    if worktree:
        (repo / ".git").write_text("gitdir: /tmp/worktree\n", encoding="utf-8")
    else:
        (repo / ".git").mkdir()
    return repo


def ensure_expected_target(repo: Path, binary: str = "sol") -> Path:
    target = install_guard.expected_target(repo, binary)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("", encoding="utf-8")
    return target


def make_alias(home_root: Path, target: Path | str, binary: str = "sol") -> Path:
    alias = home_root / ".local" / "bin" / binary
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.symlink_to(target)
    return alias


def make_managed_wrapper(
    home_root: Path,
    *,
    journal: str,
    sol_bin: str,
    binary: str = "sol",
    mode: int = 0o755,
) -> Path:
    alias = home_root / ".local" / "bin" / binary
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.write_text(
        install_guard.render_wrapper(journal, sol_bin, binary),
        encoding="utf-8",
    )
    alias.chmod(mode)
    return alias


def make_current_wrappers(home_root: Path, repo: Path, *, journal: str) -> None:
    for binary in install_guard.alias_paths():
        target = ensure_expected_target(repo, binary)
        make_managed_wrapper(
            home_root,
            journal=journal,
            sol_bin=str(target),
            binary=binary,
        )


def render_v1_wrapper(journal: str, sol_bin: str) -> str:
    escaped_sol_bin = sol_bin.replace("'", "'\\''")
    return V1_WRAPPER_TEMPLATE.format(journal=journal, sol_bin=escaped_sol_bin)


def render_v2_wrapper(*, journal: str, sol_bin: str) -> str:
    escaped_sol_bin = sol_bin.replace("'", "'\\''")
    return V2_WRAPPER_TEMPLATE.format(journal=journal, sol_bin=escaped_sol_bin)


def render_v3_wrapper(*, journal: str, sol_bin: str) -> str:
    escaped_sol_bin = sol_bin.replace("'", "'\\''")
    return V3_WRAPPER_TEMPLATE.format(journal=journal, sol_bin=escaped_sol_bin)


def render_v4_wrapper(*, journal: str, sol_bin: str) -> str:
    escaped_sol_bin = sol_bin.replace("'", "'\\''")
    return V4_WRAPPER_TEMPLATE.format(journal=journal, sol_bin=escaped_sol_bin)


def render_v5_wrapper(*, journal: str, sol_bin: str) -> str:
    escaped_sol_bin = sol_bin.replace("'", "'\\''")
    return V5_WRAPPER_TEMPLATE.format(journal=journal, sol_bin=escaped_sol_bin)


def render_v6_wrapper(*, journal: str, sol_bin: str) -> str:
    escaped_sol_bin = sol_bin.replace("'", "'\\''")
    return V6_WRAPPER_TEMPLATE.format(journal=journal, sol_bin=escaped_sol_bin)


def make_v1_wrapper(
    home_root: Path,
    *,
    journal: str,
    sol_bin: str,
    mode: int = 0o755,
) -> Path:
    alias = home_root / ".local" / "bin" / "sol"
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.write_text(render_v1_wrapper(journal, sol_bin), encoding="utf-8")
    alias.chmod(mode)
    return alias


def make_v2_wrapper(
    home_root: Path,
    *,
    journal: str,
    sol_bin: str,
    mode: int = 0o755,
) -> Path:
    alias = home_root / ".local" / "bin" / "sol"
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.write_text(
        render_v2_wrapper(journal=journal, sol_bin=sol_bin),
        encoding="utf-8",
    )
    alias.chmod(mode)
    return alias


def make_v3_wrapper(
    home_root: Path,
    *,
    journal: str,
    sol_bin: str,
    mode: int = 0o755,
) -> Path:
    alias = home_root / ".local" / "bin" / "sol"
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.write_text(
        render_v3_wrapper(journal=journal, sol_bin=sol_bin),
        encoding="utf-8",
    )
    alias.chmod(mode)
    return alias


def make_v4_wrapper(
    home_root: Path,
    *,
    journal: str,
    sol_bin: str,
    mode: int = 0o755,
) -> Path:
    alias = home_root / ".local" / "bin" / "sol"
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.write_text(
        render_v4_wrapper(journal=journal, sol_bin=sol_bin),
        encoding="utf-8",
    )
    alias.chmod(mode)
    return alias


def make_v5_wrapper(
    home_root: Path,
    *,
    journal: str,
    sol_bin: str,
    mode: int = 0o755,
) -> Path:
    alias = home_root / ".local" / "bin" / "sol"
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.write_text(
        render_v5_wrapper(journal=journal, sol_bin=sol_bin),
        encoding="utf-8",
    )
    alias.chmod(mode)
    return alias


def other_target(tmp_path: Path) -> Path:
    target = tmp_path / "other" / ".venv" / "bin" / "sol"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("", encoding="utf-8")
    return target


def run_main(monkeypatch, capsys, repo: Path, *argv: str) -> tuple[int, str, str]:
    monkeypatch.chdir(repo)
    rc = install_guard.main(list(argv))
    captured = capsys.readouterr()
    return rc, captured.out, captured.err


def alias_error(curdir: Path, installed: str, *, allow_force: bool = False) -> str:
    message = (
        "ERROR: Another solstone install owns ~/.local/bin/sol.\n"
        f"  this repo:  {curdir}\n"
        f"{installed}\n"
        "Run 'journal setup' from the installed repo first,\n"
        "or remove ~/.local/bin/sol manually if that repo is gone.\n"
    )
    if allow_force:
        message += "Rerun 'python -m solstone.think.install_guard install --force' only if you intend to replace it from this repo.\n"
    return message


def worktree_error(curdir: Path) -> str:
    return f"ERROR: refusing to run from a git worktree ({curdir}). Run from the primary clone.\n"


def write_executable_script(path: Path, content: str) -> Path:
    path.write_text(content, encoding="utf-8")
    path.chmod(0o755)
    return path


class TestWrapperHelpers:
    def test_current_journal_for_alias_falls_back_to_home_journal(
        self, home_root, monkeypatch
    ):
        from solstone.think import utils as think_utils

        def raise_not_configured():
            raise think_utils.SolstoneNotConfigured("not configured")

        monkeypatch.setattr(think_utils, "get_journal_info", raise_not_configured)

        assert install_guard._current_journal_for_alias() == str(home_root / "journal")

    def test_render_wrapper_round_trip_simple(self):
        journal = "/tmp/solstone"
        sol_bin = "/tmp/repo/.venv/bin/sol"

        content = install_guard.render_wrapper(journal, sol_bin, "sol")

        assert install_guard.parse_wrapper(content) == {
            "journal": journal,
            "sol_bin": sol_bin,
            "version": 7,
        }

    def test_render_wrapper_round_trip_tricky_paths(self):
        journal = "/tmp/solstone notes/über"
        sol_bin = "/tmp/it's a test/über/.venv/bin/sol"

        content = install_guard.render_wrapper(journal, sol_bin, "sol")

        assert install_guard.parse_wrapper(content) == {
            "journal": journal,
            "sol_bin": sol_bin,
            "version": 7,
        }

    def test_render_wrapper_matches_spec_template(self):
        journal = "/Users/jer/Documents/Solstone"
        sol_bin = "/Users/jer/projects/solstone/.venv/bin/sol"

        content = install_guard.render_wrapper(journal, sol_bin, "sol")
        warning_line = (
            '    echo "sol: WARNING — venv is stale (pyproject.toml or uv.lock changed '
            'since last install). Run: cd $REPO_ROOT && make install" >&2\n'
        )

        expected = (
            "#!/bin/bash\n"
            "# sol — managed by 'journal config'. Edits will be overwritten.\n"
            "# managed-version: 7\n"
            ': "${SOLSTONE_JOURNAL:=/Users/jer/Documents/Solstone}"\n'
            "export SOLSTONE_JOURNAL\n"
            "SOL_BIN='/Users/jer/projects/solstone/.venv/bin/sol'\n"
            "# Warn when pyproject.toml or uv.lock is newer than .installed.\n"
            "# Skipped silently if .installed is absent.\n"
            'REPO_ROOT="${SOL_BIN%/.venv/bin/sol}"\n'
            'if [ -f "$REPO_ROOT/.installed" ]; then\n'
            '  if [ "$REPO_ROOT/pyproject.toml" -nt "$REPO_ROOT/.installed" ] \\\n'
            '     || [ "$REPO_ROOT/uv.lock" -nt "$REPO_ROOT/.installed" ]; then\n'
            + warning_line
            + "  fi\n"
            "fi\n"
            'if [ ! -x "$SOL_BIN" ]; then\n'
            "    printf 'sol: venv binary missing or not executable: %s\\n' \"$SOL_BIN\" >&2\n"
            "    exit 127\n"
            "fi\n"
            'exec "$SOL_BIN" "$@"\n'
        )
        assert content == expected

    def test_render_journal_wrapper_parameterizes_binary_strings(self):
        content = install_guard.render_wrapper(
            "/Users/jer/Documents/Solstone",
            "/Users/jer/projects/solstone/.venv/bin/journal",
            "journal",
        )

        assert "# journal — managed by 'journal config'." in content
        assert "# managed-version: 7" in content
        assert 'REPO_ROOT="${SOL_BIN%/.venv/bin/journal}"' in content
        assert "journal: WARNING — venv is stale" in content
        assert "printf 'journal: venv binary missing" in content

    def test_parse_wrapper_accepts_v1(self):
        journal = "/tmp/solstone"
        sol_bin = "/tmp/repo/.venv/bin/sol"

        content = render_v1_wrapper(journal, sol_bin)

        assert install_guard.parse_wrapper(content) == {
            "journal": journal,
            "sol_bin": sol_bin,
            "version": 1,
        }

    def test_parse_wrapper_accepts_v2(self):
        journal = "/tmp/solstone"
        sol_bin = "/tmp/repo/.venv/bin/sol"

        content = render_v2_wrapper(journal=journal, sol_bin=sol_bin)

        assert install_guard.parse_wrapper(content) == {
            "journal": journal,
            "sol_bin": sol_bin,
            "version": 2,
        }

    def test_parse_wrapper_accepts_v3(self):
        journal = "/tmp/solstone"
        sol_bin = "/tmp/repo/.venv/bin/sol"

        content = render_v3_wrapper(journal=journal, sol_bin=sol_bin)

        assert install_guard.parse_wrapper(content) == {
            "journal": journal,
            "sol_bin": sol_bin,
            "version": 3,
        }

    def test_parse_wrapper_accepts_v4(self):
        journal = "/tmp/solstone"
        sol_bin = "/tmp/repo/.venv/bin/sol"

        content = render_v4_wrapper(journal=journal, sol_bin=sol_bin)

        assert install_guard.parse_wrapper(content) == {
            "journal": journal,
            "sol_bin": sol_bin,
            "version": 4,
        }

    def test_parse_wrapper_accepts_v5(self):
        journal = "/tmp/solstone"
        sol_bin = "/tmp/repo/.venv/bin/sol"

        content = render_v5_wrapper(journal=journal, sol_bin=sol_bin)

        assert install_guard.parse_wrapper(content) == {
            "journal": journal,
            "sol_bin": sol_bin,
            "version": 5,
        }

    def test_parse_wrapper_accepts_v6(self):
        journal = "/tmp/solstone"
        sol_bin = "/tmp/repo/.venv/bin/sol"

        content = render_v6_wrapper(journal=journal, sol_bin=sol_bin)

        assert install_guard.parse_wrapper(content) == {
            "journal": journal,
            "sol_bin": sol_bin,
            "version": 6,
        }

    def test_parse_wrapper_accepts_v7(self):
        journal = "/tmp/solstone"
        sol_bin = "/tmp/repo/.venv/bin/sol"

        content = install_guard.render_wrapper(journal, sol_bin, "sol")

        assert install_guard.parse_wrapper(content) == {
            "journal": journal,
            "sol_bin": sol_bin,
            "version": 7,
        }

    @pytest.mark.parametrize("char", ["$", "`", '"', "\\"])
    def test_validate_journal_path_for_wrapper_rejects_invalid_chars(self, char: str):
        with pytest.raises(ValueError, match="shell-active character"):
            install_guard.validate_journal_path_for_wrapper(f"/tmp/bad{char}path")

    def test_validate_journal_path_for_wrapper_rejects_newline(self):
        with pytest.raises(ValueError, match="shell-active character"):
            install_guard.validate_journal_path_for_wrapper("/tmp/bad\npath")


class TestCheckAlias:
    def test_absent(self, home_root, tmp_path):
        repo = make_repo(tmp_path)

        state, other = install_guard.check_alias(repo)

        assert state is install_guard.AliasState.ABSENT
        assert other is None

    def test_owned_legacy_symlink(self, home_root, tmp_path):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        make_alias(home_root, target)

        state, other = install_guard.check_alias(repo)

        assert state is install_guard.AliasState.OWNED
        assert other == target.resolve()

    def test_owned_managed_wrapper(self, home_root, tmp_path):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        make_managed_wrapper(
            home_root,
            journal="/tmp/solstone",
            sol_bin=str(target),
        )

        state, other = install_guard.check_alias(repo)

        assert state is install_guard.AliasState.OWNED
        assert other == target.resolve()

    def test_packaged_install_symlink_is_owned(self, home_root, tmp_path, monkeypatch):
        curdir = tmp_path / "site-packages" / "solstone"
        curdir.mkdir(parents=True)
        bin_dir = tmp_path / "tools" / "solstone" / "bin"
        bin_dir.mkdir(parents=True)
        packaged_sol = write_executable_script(bin_dir / "sol", "#!/bin/sh\n")
        fake_python = write_executable_script(bin_dir / "python", "#!/bin/sh\n")
        monkeypatch.setattr(sys, "executable", str(fake_python))
        make_alias(home_root, packaged_sol)

        state, other = install_guard.check_alias(curdir)

        assert state is install_guard.AliasState.OWNED
        assert other == packaged_sol.resolve()

    def test_packaged_install_managed_wrapper_is_owned(
        self, home_root, tmp_path, monkeypatch
    ):
        # When a uv-tool / pipx install runs `journal start`, install_guard
        # writes the managed wrapper at ~/.local/bin/sol with `sol_bin` set to
        # the packaged binary at <sys.executable's parent>/sol — not the
        # source-checkout's `.venv/bin/sol`. The wrapper branch needs the same
        # packaged_target fallback the symlink branch already has, or the
        # alias is misclassified as FOREIGN and `sol doctor` surfaces a
        # spurious stale_alias_symlink blocker.
        curdir = tmp_path / "site-packages" / "solstone"
        curdir.mkdir(parents=True)
        bin_dir = tmp_path / "tools" / "solstone" / "bin"
        bin_dir.mkdir(parents=True)
        packaged_sol = write_executable_script(bin_dir / "sol", "#!/bin/sh\n")
        fake_python = write_executable_script(bin_dir / "python", "#!/bin/sh\n")
        monkeypatch.setattr(sys, "executable", str(fake_python))
        make_managed_wrapper(
            home_root,
            journal="/tmp/solstone",
            sol_bin=str(packaged_sol),
        )

        state, other = install_guard.check_alias(curdir)

        assert state is install_guard.AliasState.OWNED
        assert other == packaged_sol.resolve()

    def test_cross_repo(self, home_root, tmp_path):
        repo = make_repo(tmp_path)
        target = other_target(tmp_path)
        make_alias(home_root, target)

        state, other = install_guard.check_alias(repo)

        assert state is install_guard.AliasState.CROSS_REPO
        assert other == target.resolve()

    def test_dangling(self, home_root, tmp_path):
        repo = make_repo(tmp_path)
        target = tmp_path / "missing" / ".venv" / "bin" / "sol"
        make_alias(home_root, target)

        state, other = install_guard.check_alias(repo)

        assert state is install_guard.AliasState.DANGLING
        assert other == target.resolve()

    def test_foreign_regular_file(self, home_root, tmp_path):
        repo = make_repo(tmp_path)
        alias = install_guard.alias_path()
        alias.parent.mkdir(parents=True, exist_ok=True)
        alias.write_text("not a wrapper", encoding="utf-8")

        state, other = install_guard.check_alias(repo)

        assert state is install_guard.AliasState.FOREIGN
        assert other is None

    def test_worktree(self, home_root, tmp_path):
        repo = make_repo(tmp_path, worktree=True)

        state, other = install_guard.check_alias(repo)

        assert state is install_guard.AliasState.WORKTREE
        assert other is None

    def test_worktree_takes_precedence(self, home_root, tmp_path):
        repo = make_repo(tmp_path, worktree=True)
        target = ensure_expected_target(repo)
        make_alias(home_root, target)

        state, other = install_guard.check_alias(repo)

        assert state is install_guard.AliasState.WORKTREE
        assert other is None


class TestCheckCommand:
    def test_worktree(self, home_root, tmp_path, capsys):
        repo = make_repo(tmp_path, worktree=True).resolve()

        rc = install_guard.cmd_check(repo)
        captured = capsys.readouterr()

        assert rc == 1
        assert captured.out == "worktree\n"
        assert captured.err == worktree_error(repo)

    def test_absent(self, home_root, tmp_path, capsys):
        repo = make_repo(tmp_path).resolve()

        rc = install_guard.cmd_check(repo)
        captured = capsys.readouterr()

        assert rc == 0
        assert captured.out == "fresh\n"
        assert captured.err == ""

    def test_check_reports_current_for_v7_wrappers_with_matching_paths(
        self, home_root, tmp_path, capsys
    ):
        repo = make_repo(tmp_path)
        make_current_wrappers(
            home_root,
            repo,
            journal=install_guard._current_journal_for_alias(),
        )

        rc = install_guard.cmd_check(repo)
        captured = capsys.readouterr()

        assert rc == 0
        assert captured.out == "current\n"
        assert captured.err == ""

    def test_check_reports_upgrade_for_v1_wrapper_with_matching_paths(
        self, home_root, tmp_path
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        make_v1_wrapper(
            home_root,
            journal=install_guard._current_journal_for_alias(),
            sol_bin=str(target),
        )

        state, token = install_guard.check_alias_detail(repo)

        assert state is install_guard.AliasState.OWNED
        assert token == "upgrade"

    def test_check_reports_upgrade_for_v3_wrapper_with_matching_paths(
        self, home_root, tmp_path
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        make_v3_wrapper(
            home_root,
            journal=install_guard._current_journal_for_alias(),
            sol_bin=str(target),
        )

        state, token = install_guard.check_alias_detail(repo)

        assert state is install_guard.AliasState.OWNED
        assert token == "upgrade"

    def test_check_reports_upgrade_for_v2_wrapper_with_matching_paths(
        self, home_root, tmp_path
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        make_v2_wrapper(
            home_root,
            journal=install_guard._current_journal_for_alias(),
            sol_bin=str(target),
        )

        state, token = install_guard.check_alias_detail(repo)

        assert state is install_guard.AliasState.OWNED
        assert token == "upgrade"

    def test_check_reports_upgrade_for_legacy_symlink(
        self, home_root, tmp_path, capsys
    ):
        repo = make_repo(tmp_path)
        make_alias(home_root, ensure_expected_target(repo))

        rc = install_guard.cmd_check(repo)
        captured = capsys.readouterr()

        assert rc == 0
        assert captured.out == "upgrade\n"
        assert captured.err == ""

    def test_check_reports_upgrade_for_wrapper_with_stale_paths(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        old_journal = str((tmp_path / "old-journal").resolve())
        new_journal = str((tmp_path / "new-journal").resolve())
        make_managed_wrapper(home_root, journal=old_journal, sol_bin=str(target))
        monkeypatch.setenv("SOLSTONE_JOURNAL", new_journal)

        rc = install_guard.cmd_check(repo)
        captured = capsys.readouterr()

        assert rc == 0
        assert captured.out == "upgrade\n"
        assert captured.err == ""

    def test_cross_repo(self, home_root, tmp_path, capsys):
        repo = make_repo(tmp_path).resolve()
        target = other_target(tmp_path).resolve()
        make_alias(home_root, target)

        rc = install_guard.cmd_check(repo)
        captured = capsys.readouterr()

        assert rc == 1
        assert captured.out == "cross_repo\n"
        assert captured.err == alias_error(
            repo,
            f"  installed:  {target}",
            allow_force=True,
        )

    def test_dangling(self, home_root, tmp_path, capsys):
        repo = make_repo(tmp_path).resolve()
        target = (tmp_path / "missing" / ".venv" / "bin" / "sol").resolve()
        make_alias(home_root, target)

        rc = install_guard.cmd_check(repo)
        captured = capsys.readouterr()

        assert rc == 1
        assert captured.out == "dangling\n"
        assert captured.err == alias_error(
            repo,
            f"  installed:  dangling: {target} does not exist",
            allow_force=True,
        )

    def test_foreign(self, home_root, tmp_path, capsys):
        repo = make_repo(tmp_path).resolve()
        alias = install_guard.alias_path()
        alias.parent.mkdir(parents=True, exist_ok=True)
        alias.write_text("not a wrapper", encoding="utf-8")

        rc = install_guard.cmd_check(repo)
        captured = capsys.readouterr()

        assert rc == 1
        assert captured.out == "not_symlink\n"
        assert captured.err == alias_error(
            repo,
            "  installed:  not a symlink",
            allow_force=True,
        )


class TestInstall:
    @pytest.fixture(autouse=True)
    def path_already_present(self, monkeypatch):
        monkeypatch.setattr(
            "solstone.think.install_guard.userpath.in_current_path",
            lambda _path: True,
        )

    def test_install_upgrades_legacy_symlink_to_managed_wrapper(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        alias = make_alias(home_root, target)

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")

        assert rc == 0
        assert out == "installed\npath: ~/.local/bin already on PATH\n"
        assert err == ""
        assert not alias.is_symlink()
        assert os.access(alias, os.X_OK)
        assert alias.read_text(encoding="utf-8") == install_guard.render_wrapper(
            install_guard._current_journal_for_alias(),
            str(target),
            "sol",
        )
        assert install_guard.parse_wrapper(alias.read_text(encoding="utf-8")) == {
            "journal": install_guard._current_journal_for_alias(),
            "sol_bin": str(target),
            "version": 7,
        }

    def test_install_refuses_foreign_regular_file_without_force(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path).resolve()
        alias = install_guard.alias_path()
        alias.parent.mkdir(parents=True, exist_ok=True)
        alias.write_text("foreign", encoding="utf-8")

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")

        assert rc == 1
        assert out == ""
        assert err == alias_error(
            repo,
            "  installed:  not a symlink",
            allow_force=True,
        )
        assert alias.read_text(encoding="utf-8") == "foreign"

    def test_install_force_overwrites_foreign_regular_file(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        alias = install_guard.alias_path()
        alias.parent.mkdir(parents=True, exist_ok=True)
        alias.write_text("foreign", encoding="utf-8")

        rc, out, err = run_main(monkeypatch, capsys, repo, "install", "--force")

        assert rc == 0
        assert out == "installed\npath: ~/.local/bin already on PATH\n"
        assert err == ""
        assert install_guard.parse_wrapper(alias.read_text(encoding="utf-8")) == {
            "journal": install_guard._current_journal_for_alias(),
            "sol_bin": str(target),
            "version": 7,
        }
        assert os.access(alias, os.X_OK)

    def test_install_is_idempotent(self, home_root, tmp_path, monkeypatch, capsys):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)

        rc1, out1, err1 = run_main(monkeypatch, capsys, repo, "install")
        alias = install_guard.alias_path()
        first_content = alias.read_text(encoding="utf-8")

        rc2, out2, err2 = run_main(monkeypatch, capsys, repo, "install")

        assert rc1 == 0
        assert out1 == "installed\npath: ~/.local/bin already on PATH\n"
        assert err1 == ""
        assert rc2 == 0
        assert out2 == "installed\npath: ~/.local/bin already on PATH\n"
        assert err2 == ""
        assert alias.read_text(encoding="utf-8") == first_content
        assert install_guard.parse_wrapper(first_content) == {
            "journal": install_guard._current_journal_for_alias(),
            "sol_bin": str(target),
            "version": 7,
        }

    def test_install_writes_sol_and_journal_wrappers(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        for binary in install_guard.alias_paths():
            ensure_expected_target(repo, binary)

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")

        assert rc == 0
        assert out == "installed\npath: ~/.local/bin already on PATH\n"
        assert err == ""
        for binary, alias in install_guard.alias_paths().items():
            assert alias.exists()
            assert os.access(alias, os.X_OK)
            assert install_guard.parse_wrapper(alias.read_text(encoding="utf-8")) == {
                "journal": install_guard._current_journal_for_alias(),
                "sol_bin": str(install_guard.expected_target(repo, binary)),
                "version": 7,
            }

    def test_install_mid_failure_rolls_back_both_wrappers(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        old_journal = str(tmp_path / "old-journal")
        for binary in install_guard.alias_paths():
            target = ensure_expected_target(repo, binary)
            make_managed_wrapper(
                home_root,
                journal=old_journal,
                sol_bin=str(target),
                binary=binary,
                mode=0o700,
            )
        before = {
            binary: (
                alias.read_bytes(),
                alias.stat().st_mode & 0o777,
            )
            for binary, alias in install_guard.alias_paths().items()
        }
        real_replace = install_guard.os.replace

        def fail_second_replace(src: Path | str, dst: Path | str) -> None:
            if Path(dst).name == "journal":
                raise OSError("simulated replace failure")
            real_replace(src, dst)

        monkeypatch.setattr(install_guard.os, "replace", fail_second_replace)

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")

        assert rc == 1
        assert out == ""
        assert "ERROR: failed to install wrappers: simulated replace failure" in err
        for binary, alias in install_guard.alias_paths().items():
            assert alias.read_bytes() == before[binary][0]
            assert alias.stat().st_mode & 0o777 == before[binary][1]

    def test_v1_wrapper_upgrades_to_v6_end_to_end(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        alias = make_v1_wrapper(
            home_root,
            journal=install_guard._current_journal_for_alias(),
            sol_bin=str(target),
        )

        assert install_guard.cmd_check(repo) == 0
        captured = capsys.readouterr()
        assert captured.out == "upgrade\n"
        assert captured.err == ""

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")

        assert rc == 0
        assert out == "installed\npath: ~/.local/bin already on PATH\n"
        assert err == ""
        assert install_guard.parse_wrapper(alias.read_text(encoding="utf-8")) == {
            "journal": install_guard._current_journal_for_alias(),
            "sol_bin": str(target),
            "version": 7,
        }

        assert install_guard.cmd_check(repo) == 0
        captured = capsys.readouterr()
        assert captured.out == "current\n"
        assert captured.err == ""

    def test_v2_wrapper_upgrades_to_v6_end_to_end(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        alias = make_v2_wrapper(
            home_root,
            journal=install_guard._current_journal_for_alias(),
            sol_bin=str(target),
        )

        assert install_guard.cmd_check(repo) == 0
        captured = capsys.readouterr()
        assert captured.out == "upgrade\n"
        assert captured.err == ""

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")

        assert rc == 0
        assert out == "installed\npath: ~/.local/bin already on PATH\n"
        assert err == ""
        assert install_guard.parse_wrapper(alias.read_text(encoding="utf-8")) == {
            "journal": install_guard._current_journal_for_alias(),
            "sol_bin": str(target),
            "version": 7,
        }

        assert install_guard.cmd_check(repo) == 0
        captured = capsys.readouterr()
        assert captured.out == "current\n"
        assert captured.err == ""

    def test_v3_wrapper_upgrades_to_v6_end_to_end(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        alias = make_v3_wrapper(
            home_root,
            journal=install_guard._current_journal_for_alias(),
            sol_bin=str(target),
        )

        assert install_guard.cmd_check(repo) == 0
        captured = capsys.readouterr()
        assert captured.out == "upgrade\n"
        assert captured.err == ""

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")

        content = alias.read_text(encoding="utf-8")
        assert rc == 0
        assert out == "installed\npath: ~/.local/bin already on PATH\n"
        assert err == ""
        assert content.startswith("#!/bin/bash\n")
        assert (
            'exec > >(tee -a "$SOLSTONE_JOURNAL/health/service.log") 2>&1'
            not in content
        )
        assert install_guard.parse_wrapper(content) == {
            "journal": install_guard._current_journal_for_alias(),
            "sol_bin": str(target),
            "version": 7,
        }

        assert install_guard.cmd_check(repo) == 0
        captured = capsys.readouterr()
        assert captured.out == "current\n"
        assert captured.err == ""

    def test_v4_wrapper_upgrades_to_v6_end_to_end(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        alias = make_v4_wrapper(
            home_root,
            journal=install_guard._current_journal_for_alias(),
            sol_bin=str(target),
        )

        assert install_guard.cmd_check(repo) == 0
        captured = capsys.readouterr()
        assert captured.out == "upgrade\n"
        assert captured.err == ""

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")

        assert rc == 0
        assert out == "installed\npath: ~/.local/bin already on PATH\n"
        assert err == ""
        assert install_guard.parse_wrapper(alias.read_text(encoding="utf-8")) == {
            "journal": install_guard._current_journal_for_alias(),
            "sol_bin": str(target),
            "version": 7,
        }

        assert install_guard.cmd_check(repo) == 0
        captured = capsys.readouterr()
        assert captured.out == "current\n"
        assert captured.err == ""

    def test_v5_wrapper_upgrades_to_v6_end_to_end(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        alias = make_v5_wrapper(
            home_root,
            journal=install_guard._current_journal_for_alias(),
            sol_bin=str(target),
        )

        assert install_guard.cmd_check(repo) == 0
        captured = capsys.readouterr()
        assert captured.out == "upgrade\n"
        assert captured.err == ""

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")

        content = alias.read_text(encoding="utf-8")
        assert rc == 0
        assert out == "installed\npath: ~/.local/bin already on PATH\n"
        assert err == ""
        assert 'if [ "$1" = "supervisor" ]' not in content
        assert install_guard.parse_wrapper(content) == {
            "journal": install_guard._current_journal_for_alias(),
            "sol_bin": str(target),
            "version": 7,
        }

        assert install_guard.cmd_check(repo) == 0
        captured = capsys.readouterr()
        assert captured.out == "current\n"
        assert captured.err == ""

    def test_install_refuses_invalid_journal_path(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        monkeypatch.setenv("SOLSTONE_JOURNAL", "/tmp/bad$path")

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")

        assert rc == 1
        assert out == ""
        assert "refused: journal path contains shell-active character '$'" in err
        assert not install_guard.alias_path().exists()

    def test_path_appended_when_not_on_path(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        append_mock = Mock(return_value=True)
        restart_mock = Mock(return_value=False)
        monkeypatch.setattr(
            "solstone.think.install_guard.userpath.in_current_path",
            lambda _path: False,
        )
        monkeypatch.setattr("solstone.think.install_guard.userpath.append", append_mock)
        monkeypatch.setattr(
            "solstone.think.install_guard.userpath.need_shell_restart",
            restart_mock,
        )

        rc, out, err = run_main(monkeypatch, capsys, repo, "install")
        alias = install_guard.alias_path()

        assert rc == 0
        assert out == "installed\npath: added ~/.local/bin to shell PATH\n"
        assert err == ""
        assert install_guard.parse_wrapper(alias.read_text(encoding="utf-8")) == {
            "journal": install_guard._current_journal_for_alias(),
            "sol_bin": str(target),
            "version": 7,
        }
        append_mock.assert_called_once_with(
            str(alias.parent),
            app_name="solstone",
            all_shells=True,
        )
        restart_mock.assert_called_once_with(str(alias.parent))


class TestUninstall:
    def test_uninstall_removes_managed_wrapper(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        alias = make_managed_wrapper(
            home_root,
            journal=install_guard._current_journal_for_alias(),
            sol_bin=str(target),
        )

        rc, out, err = run_main(monkeypatch, capsys, repo, "uninstall")

        assert rc == 0
        assert out == "uninstalled\n"
        assert err == ""
        assert not alias.exists()
        assert not alias.is_symlink()

    def test_uninstall_removes_legacy_symlink(
        self, home_root, tmp_path, monkeypatch, capsys
    ):
        repo = make_repo(tmp_path)
        target = ensure_expected_target(repo)
        alias = make_alias(home_root, target)

        rc, out, err = run_main(monkeypatch, capsys, repo, "uninstall")

        assert rc == 0
        assert out == "uninstalled\n"
        assert err == ""
        assert not alias.exists()
        assert not alias.is_symlink()

    def test_noop_on_absent(self, home_root, tmp_path, monkeypatch, capsys):
        repo = make_repo(tmp_path)

        rc, out, err = run_main(monkeypatch, capsys, repo, "uninstall")

        assert rc == 0
        assert out == "absent\n"
        assert err == ""
        assert not install_guard.alias_path().exists()

    def test_refuses_foreign(self, home_root, tmp_path, monkeypatch, capsys):
        repo = make_repo(tmp_path).resolve()
        alias = install_guard.alias_path()
        alias.parent.mkdir(parents=True, exist_ok=True)
        alias.write_text("foreign", encoding="utf-8")

        rc, out, err = run_main(monkeypatch, capsys, repo, "uninstall")

        assert rc == 1
        assert out == ""
        assert err == alias_error(repo, "  installed:  not a symlink")
        assert alias.read_text(encoding="utf-8") == "foreign"

    def test_refuses_worktree(self, home_root, tmp_path, monkeypatch, capsys):
        repo = make_repo(tmp_path, worktree=True).resolve()

        rc, out, err = run_main(monkeypatch, capsys, repo, "uninstall")

        assert rc == 1
        assert out == ""
        assert err == worktree_error(repo)
        assert not install_guard.alias_path().exists()


class TestWrapperStalenessProbe:
    @staticmethod
    def _wrapper_env() -> dict[str, str]:
        env = os.environ.copy()
        env.pop("SOLSTONE_JOURNAL", None)
        return env

    @staticmethod
    def _write_repo_wrapper(tmp_path: Path) -> tuple[Path, Path, Path]:
        repo = tmp_path / "repo"
        journal = tmp_path / "journal"
        marker = tmp_path / "stub-ran"
        sol_bin = repo / ".venv" / "bin" / "sol"
        sol_bin.parent.mkdir(parents=True)
        write_executable_script(
            sol_bin,
            "#!{python}\n"
            "from pathlib import Path\n"
            "Path({marker!r}).write_text('ran', encoding='utf-8')\n".format(
                python=sys.executable,
                marker=str(marker),
            ),
        )
        (repo / "pyproject.toml").write_text("[project]\n", encoding="utf-8")
        (repo / "uv.lock").write_text("", encoding="utf-8")
        wrapper = write_executable_script(
            tmp_path / "sol",
            install_guard.render_wrapper(str(journal), str(sol_bin), "sol"),
        )
        return repo, wrapper, marker

    @staticmethod
    def _run_wrapper(wrapper: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(wrapper), "--help"],
            capture_output=True,
            text=True,
            check=False,
            env=TestWrapperStalenessProbe._wrapper_env(),
        )

    def test_warns_when_pyproject_is_newer_than_installed(self, tmp_path):
        repo, wrapper, marker = self._write_repo_wrapper(tmp_path)
        installed = repo / ".installed"
        installed.touch()
        os.utime(installed, (1000, 1000))
        os.utime(repo / "pyproject.toml", (2000, 2000))
        os.utime(repo / "uv.lock", (1000, 1000))

        result = self._run_wrapper(wrapper)

        assert result.returncode == 0
        assert "WARNING — venv is stale" in result.stderr
        assert "make install" in result.stderr
        assert marker.read_text(encoding="utf-8") == "ran"

    def test_skips_warning_when_installed_is_fresh(self, tmp_path):
        repo, wrapper, marker = self._write_repo_wrapper(tmp_path)
        installed = repo / ".installed"
        installed.touch()
        os.utime(repo / "pyproject.toml", (1000, 1000))
        os.utime(repo / "uv.lock", (1500, 1500))
        os.utime(installed, (2000, 2000))

        result = self._run_wrapper(wrapper)

        assert result.returncode == 0
        assert "WARNING — venv is stale" not in result.stderr
        assert marker.read_text(encoding="utf-8") == "ran"

    def test_skips_warning_when_installed_marker_is_missing(self, tmp_path):
        _repo, wrapper, marker = self._write_repo_wrapper(tmp_path)

        result = self._run_wrapper(wrapper)

        assert result.returncode == 0
        assert "WARNING — venv is stale" not in result.stderr
        assert marker.read_text(encoding="utf-8") == "ran"


class TestWrapperRuntime:
    @staticmethod
    def _wrapper_env() -> dict[str, str]:
        env = os.environ.copy()
        env.pop("SOLSTONE_JOURNAL", None)
        env.pop("PYTHONUNBUFFERED", None)
        return env

    @staticmethod
    def _write_stub_sol_bin(tmp_path: Path) -> Path:
        return write_executable_script(
            tmp_path / "stub-sol",
            "#!/bin/sh\nprintf 'OUT %s\\n' \"$*\"\nprintf 'ERR %s\\n' \"$*\" >&2\n",
        )

    @staticmethod
    def _write_wrapper(tmp_path: Path, *, journal: Path, sol_bin: Path) -> Path:
        return write_executable_script(
            tmp_path / "sol",
            install_guard.render_wrapper(str(journal), str(sol_bin), "sol"),
        )

    def test_rendered_v7_wrapper_has_no_supervisor_stdio_block(self):
        content = install_guard.render_wrapper(
            "/tmp/solstone",
            "/tmp/repo/.venv/bin/sol",
            "sol",
        )

        assert "# managed-version: 7" in content
        assert 'if [ "$1" = "supervisor" ]' not in content
        assert "tee -a" not in content
        assert "PYTHONUNBUFFERED" not in content

    def test_wrapper_supervisor_passthrough_does_not_redirect(self, tmp_path):
        journal = tmp_path / "j"
        sol_bin = self._write_stub_sol_bin(tmp_path)
        wrapper = self._write_wrapper(tmp_path, journal=journal, sol_bin=sol_bin)

        result = subprocess.run(
            [str(wrapper), "supervisor", "5015"],
            capture_output=True,
            text=True,
            check=False,
            env=self._wrapper_env(),
        )

        assert result.returncode == 0
        assert result.stdout == "OUT supervisor 5015\n"
        assert result.stderr == "ERR supervisor 5015\n"
        assert not (journal / "health" / "service.log").exists()

    def test_wrapper_does_not_export_pythonunbuffered(self, tmp_path):
        journal = tmp_path / "j"
        sol_bin = write_executable_script(
            tmp_path / "stub-sol",
            '#!/bin/sh\nprintf "%s\\n" "${PYTHONUNBUFFERED:-<unset>}"\n',
        )
        wrapper = self._write_wrapper(tmp_path, journal=journal, sol_bin=sol_bin)

        result = subprocess.run(
            [str(wrapper), "supervisor"],
            capture_output=True,
            text=True,
            check=False,
            env=self._wrapper_env(),
        )

        assert result.returncode == 0
        assert result.stdout.strip() == "<unset>"
        assert not (journal / "health" / "service.log").exists()

    def test_wrapper_passthrough_for_other_subcommands(self, tmp_path):
        journal = tmp_path / "j"
        sol_bin = self._write_stub_sol_bin(tmp_path)
        wrapper = self._write_wrapper(tmp_path, journal=journal, sol_bin=sol_bin)

        result = subprocess.run(
            [str(wrapper), "not-supervisor"],
            capture_output=True,
            text=True,
            check=False,
            env=self._wrapper_env(),
        )

        assert result.returncode == 0
        assert result.stdout == "OUT not-supervisor\n"
        assert result.stderr == "ERR not-supervisor\n"
        assert not (journal / "health" / "service.log").exists()
