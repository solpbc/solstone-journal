# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import signal
from pathlib import Path

import pytest

from solstone.think import supervisor
from solstone.think.supervisor import _MANAGED_SERVICE_PROCTITLES

TEST_JOURNAL = Path("/journal/test")


class _FakeProcess:
    def __init__(
        self,
        *,
        pid: int,
        name: str = "journal:sense",
        ppid: int = 1,
        username: str = "jer",
        name_error: Exception | None = None,
        ppid_error: Exception | None = None,
        username_error: Exception | None = None,
    ):
        self.pid = pid
        self._name = name
        self._ppid = ppid
        self._username = username
        self._name_error = name_error
        self._ppid_error = ppid_error
        self._username_error = username_error

    def name(self) -> str:
        if self._name_error:
            raise self._name_error
        return self._name

    def ppid(self) -> int:
        if self._ppid_error:
            raise self._ppid_error
        return self._ppid

    def username(self) -> str:
        if self._username_error:
            raise self._username_error
        return self._username


class TestOrphanSweep:
    def _patch_common(self, monkeypatch, procs):
        kills = []
        monkeypatch.setattr(supervisor.sys, "platform", "linux")
        monkeypatch.setattr(supervisor.getpass, "getuser", lambda: "jer")
        monkeypatch.setattr(supervisor.psutil, "process_iter", lambda _attrs: procs)
        monkeypatch.setattr(supervisor, "_candidate_journal", lambda proc: TEST_JOURNAL)
        monkeypatch.setattr(
            supervisor.os, "kill", lambda pid, sig: kills.append((pid, sig))
        )
        return kills

    @pytest.mark.parametrize("proctitle", sorted(_MANAGED_SERVICE_PROCTITLES))
    def test_managed_service_proctitles_are_sigtermed(self, monkeypatch, proctitle):
        procs = [_FakeProcess(pid=111, name=proctitle)]
        kills = self._patch_common(monkeypatch, procs)
        monkeypatch.setattr(supervisor.psutil, "pid_exists", lambda _pid: False)

        assert supervisor._sweep_orphaned_sol_processes(journal=TEST_JOURNAL) == 1
        assert kills == [(111, signal.SIGTERM)]

    @pytest.mark.parametrize(
        "proctitle",
        ["journal:think", "journal:heartbeat", "sol:call"],
    )
    def test_non_managed_orphan_proctitles_are_ignored(self, monkeypatch, proctitle):
        procs = [_FakeProcess(pid=111, name=proctitle)]
        kills = self._patch_common(monkeypatch, procs)

        assert supervisor._sweep_orphaned_sol_processes(journal=TEST_JOURNAL) == 0
        assert kills == []

    def test_non_matching_processes_are_ignored(self, monkeypatch):
        monkeypatch.setattr(supervisor.os, "getpid", lambda: 555)
        procs = [
            _FakeProcess(pid=111, username="other"),
            _FakeProcess(pid=112, name="llama-server", username="other"),
            _FakeProcess(pid=222, ppid=2),
            _FakeProcess(pid=223, name="llama-server", ppid=2),
            _FakeProcess(pid=333, name="python"),
            _FakeProcess(pid=444, name="solstone:convey"),
            _FakeProcess(pid=555),
        ]
        kills = self._patch_common(monkeypatch, procs)

        assert supervisor._sweep_orphaned_sol_processes(journal=TEST_JOURNAL) == 0
        assert kills == []

    def test_survivors_after_grace_are_sigkilled(self, monkeypatch):
        procs = [_FakeProcess(pid=111), _FakeProcess(pid=222)]
        kills = self._patch_common(monkeypatch, procs)
        monkeypatch.setattr(supervisor.psutil, "pid_exists", lambda pid: pid == 222)
        monkeypatch.setattr(supervisor.time, "sleep", lambda _seconds: None)

        assert (
            supervisor._sweep_orphaned_sol_processes(
                journal=TEST_JOURNAL,
                grace=0.0,
            )
            == 2
        )
        assert kills == [
            (111, signal.SIGTERM),
            (222, signal.SIGTERM),
            (222, signal.SIGKILL),
        ]

    def test_process_access_errors_are_swallowed(self, monkeypatch):
        procs = [
            _FakeProcess(pid=111, name_error=supervisor.psutil.NoSuchProcess(pid=111)),
            _FakeProcess(
                pid=222,
                username_error=supervisor.psutil.AccessDenied(pid=222),
            ),
            _FakeProcess(pid=333),
        ]
        kills = self._patch_common(monkeypatch, procs)
        monkeypatch.setattr(supervisor.psutil, "pid_exists", lambda _pid: False)

        assert supervisor._sweep_orphaned_sol_processes(journal=TEST_JOURNAL) == 1
        assert kills == [(333, signal.SIGTERM)]

    @pytest.mark.parametrize("platform", ["linux", "darwin", "freebsd"])
    def test_runs_on_all_platforms(self, monkeypatch, platform):
        procs = [_FakeProcess(pid=111)]
        kills = self._patch_common(monkeypatch, procs)
        monkeypatch.setattr(supervisor.sys, "platform", platform)
        monkeypatch.setattr(supervisor.psutil, "pid_exists", lambda _pid: False)

        assert supervisor._sweep_orphaned_sol_processes(journal=TEST_JOURNAL) == 1
        assert kills == [(111, signal.SIGTERM)]

    def test_candidate_in_different_journal_is_skipped(self, monkeypatch):
        procs = [_FakeProcess(pid=111), _FakeProcess(pid=222, name="llama-server")]
        kills = self._patch_common(monkeypatch, procs)
        monkeypatch.setattr(
            supervisor,
            "_candidate_journal",
            lambda proc: Path("/journal/other"),
        )

        assert supervisor._sweep_orphaned_sol_processes(journal=TEST_JOURNAL) == 0
        assert kills == []

    @pytest.mark.parametrize(
        "reason",
        ["access_denied", "missing_key", "malformed_value"],
    )
    def test_unknown_journal_candidate_is_skipped(self, monkeypatch, reason):
        procs = [_FakeProcess(pid=111)]
        kills = self._patch_common(monkeypatch, procs)
        monkeypatch.setattr(supervisor, "_candidate_journal", lambda proc: None)

        assert supervisor._sweep_orphaned_sol_processes(journal=TEST_JOURNAL) == 0
        assert kills == []

    def test_same_journal_candidate_is_swept(self, monkeypatch):
        procs = [_FakeProcess(pid=111)]
        kills = self._patch_common(monkeypatch, procs)
        monkeypatch.setattr(
            supervisor,
            "_candidate_journal",
            lambda proc: TEST_JOURNAL,
        )
        monkeypatch.setattr(supervisor.psutil, "pid_exists", lambda _pid: False)

        assert supervisor._sweep_orphaned_sol_processes(journal=TEST_JOURNAL) == 1
        assert kills == [(111, signal.SIGTERM)]
