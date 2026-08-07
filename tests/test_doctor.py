# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think import install_guard
from solstone.think.probe import ExecutionError, ProbeOutput

ROOT = Path(__file__).resolve().parent.parent


@pytest.fixture
def doctor():
    from solstone.think import doctor as doctor_module

    yield doctor_module


@pytest.fixture
def home_root(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    return home


def args(doctor, *, port: int = 5015, feature: str | None = None):
    return doctor.Args(
        verbose=False,
        json=False,
        jsonl=False,
        port=port,
        feature=feature,
    )


def make_repo(tmp_path: Path, *, worktree: bool = False) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    if worktree:
        (repo / ".git").write_text("gitdir: /tmp/worktree\n", encoding="utf-8")
    else:
        (repo / ".git").mkdir()
    return repo


def ensure_expected_target(repo: Path) -> Path:
    target = repo / ".venv" / "bin" / "sol"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("", encoding="utf-8")
    return target


def make_alias(home_root: Path, target: Path | str, binary: str = "sol") -> Path:
    alias = home_root / ".local" / "bin" / binary
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.symlink_to(target)
    return alias


def other_target(tmp_path: Path) -> Path:
    target = tmp_path / "other" / ".venv" / "bin" / "sol"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("", encoding="utf-8")
    return target


def test_install_guard_import_succeeds_when_frontmatter_is_shadowed(tmp_path):
    shadow_dir = tmp_path / "shadow"
    shadow_dir.mkdir()
    (shadow_dir / "frontmatter.py").write_text(
        'raise ImportError("blocked for test")\n',
        encoding="utf-8",
    )
    env = os.environ.copy()
    pythonpath_parts = [str(shadow_dir), str(ROOT)]
    existing_pythonpath = env.get("PYTHONPATH")
    if existing_pythonpath:
        pythonpath_parts.append(existing_pythonpath)
    env["PYTHONPATH"] = os.pathsep.join(pythonpath_parts)

    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "from solstone.think.install_guard import parse_wrapper; print('ok')",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        env=env,
    )

    assert result.returncode == 0
    assert result.stdout.strip() == "ok"


class TestPythonVersion:
    def test_ok(self, doctor):
        result = doctor.python_sanity_check(args(doctor))
        assert result.status == "ok"

    def test_fail_when_too_old(self, doctor, monkeypatch):
        monkeypatch.setattr(doctor.sys, "version_info", (3, 9, 18))
        result = doctor.python_sanity_check(args(doctor))
        assert result.status == "fail"
        assert "does not satisfy" in result.detail


class TestSolImportable:
    def test_skip_when_absent(self, doctor, monkeypatch, tmp_path):
        monkeypatch.setattr(doctor, "ROOT", tmp_path)
        result = doctor.sol_importable_check(args(doctor))
        assert result.status == "skip"

    def test_ok(self, doctor, monkeypatch, tmp_path):
        monkeypatch.setattr(doctor, "ROOT", tmp_path)
        python_bin = tmp_path / ".venv" / "bin" / "python"
        python_bin.parent.mkdir(parents=True)
        python_bin.write_text("", encoding="utf-8")
        monkeypatch.setattr(
            doctor,
            "run_probe",
            lambda *_args, **_kwargs: ProbeOutput("", "", 0),
        )
        result = doctor.sol_importable_check(args(doctor))
        assert result.status == "ok"
        assert (
            result.detail
            == "from solstone.think.utils import get_journal succeeded outside repo cwd"
        )

    def test_fail_on_module_not_found(self, doctor, monkeypatch, tmp_path):
        monkeypatch.setattr(doctor, "ROOT", tmp_path)
        python_bin = tmp_path / ".venv" / "bin" / "python"
        python_bin.parent.mkdir(parents=True)
        python_bin.write_text("", encoding="utf-8")
        monkeypatch.setattr(
            doctor,
            "run_probe",
            lambda *_args, **_kwargs: ProbeOutput(
                "",
                "Traceback (most recent call last):\nModuleNotFoundError: No module named 'solstone'\n",
                1,
            ),
        )
        result = doctor.sol_importable_check(args(doctor))
        assert result.status == "fail"
        assert result.detail == "ModuleNotFoundError: No module named 'solstone'"

    def test_fail_on_other_exception(self, doctor, monkeypatch, tmp_path):
        monkeypatch.setattr(doctor, "ROOT", tmp_path)
        python_bin = tmp_path / ".venv" / "bin" / "python"
        python_bin.parent.mkdir(parents=True)
        python_bin.write_text("", encoding="utf-8")
        monkeypatch.setattr(
            doctor,
            "run_probe",
            lambda *_args, **_kwargs: ProbeOutput(
                "", "SyntaxError: broken import\n", 1
            ),
        )
        result = doctor.sol_importable_check(args(doctor))
        assert result.status == "fail"
        assert result.detail == "SyntaxError: broken import"


class TestPackagedInstall:
    def setup_packaged(self, doctor, monkeypatch, tmp_path):
        monkeypatch.setattr(doctor, "ROOT", tmp_path)
        monkeypatch.setattr(doctor, "is_packaged_install", lambda: True)

    def test_python_version_uses_metadata_when_pyproject_absent(
        self, doctor, monkeypatch, tmp_path
    ):
        self.setup_packaged(doctor, monkeypatch, tmp_path)
        monkeypatch.setattr(
            doctor,
            "distribution",
            lambda name: SimpleNamespace(metadata={"Requires-Python": ">=3.11"}),
        )

        result = doctor.python_sanity_check(args(doctor))

        assert result.status == "ok"
        assert ">=3.11" in result.detail

    def test_sol_importable_uses_in_process_import(self, doctor, monkeypatch, tmp_path):
        self.setup_packaged(doctor, monkeypatch, tmp_path)

        result = doctor.sol_importable_check(args(doctor))

        assert result.status == "ok"
        assert result.detail == "import solstone succeeded in packaged install"


class TestDefaultSttReady:
    @staticmethod
    def setup_linux_parakeet_default(doctor, monkeypatch, tmp_path):
        journal = tmp_path / "journal"
        monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))
        monkeypatch.setattr(
            doctor.parakeet_readiness,
            "_platform_info",
            lambda: ("linux", "x86_64"),
        )
        return journal

    @staticmethod
    def make_ready_files(doctor, cache_root: Path, artifact_key: str) -> None:
        for backend in doctor.parakeet_readiness.PARAKEET_CPP_BINARY_BACKENDS:
            path = doctor.parakeet_readiness.parakeet_cpp_binary_path(
                cache_root, artifact_key, backend
            )
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("#!/bin/sh\n", encoding="utf-8")
            path.chmod(0o755)
        model_path = doctor.parakeet_readiness.parakeet_cpp_model_path(cache_root)
        model_path.parent.mkdir(parents=True, exist_ok=True)
        model_path.write_text("model", encoding="utf-8")

    def test_ok_when_linux_runtime_and_model_ready(self, doctor, monkeypatch, tmp_path):
        from solstone.think.providers import parakeet_server

        journal = self.setup_linux_parakeet_default(doctor, monkeypatch, tmp_path)
        artifact_key = "x86_64-unknown-linux-gnu"
        cache_root = doctor.parakeet_readiness.parakeet_cpp_cache_root(journal)
        self.make_ready_files(doctor, cache_root, artifact_key)
        monkeypatch.setattr(
            parakeet_server,
            "probe_state",
            lambda: (parakeet_server.STATE_READY, None),
        )

        result = doctor.default_stt_ready_check(args(doctor))

        assert result.status == "ok"
        assert result.detail == (
            "parakeet-cpp ready (binaries + model installed, server reachable)"
        )

    def test_warns_when_linux_cpp_artifacts_missing(
        self, doctor, monkeypatch, tmp_path
    ):
        self.setup_linux_parakeet_default(doctor, monkeypatch, tmp_path)

        result = doctor.default_stt_ready_check(args(doctor))

        assert result.status == "warn"
        assert "missing" in result.detail
        assert result.fix == doctor._PARAKEET_CPP_INSTALL_FIX

    def test_warns_when_linux_cpp_server_not_started(
        self, doctor, monkeypatch, tmp_path
    ):
        from solstone.think.providers import parakeet_server

        journal = self.setup_linux_parakeet_default(doctor, monkeypatch, tmp_path)
        artifact_key = "x86_64-unknown-linux-gnu"
        cache_root = doctor.parakeet_readiness.parakeet_cpp_cache_root(journal)
        self.make_ready_files(doctor, cache_root, artifact_key)
        monkeypatch.setattr(
            parakeet_server,
            "probe_state",
            lambda: (parakeet_server.STATE_FAILED, "no port"),
        )

        result = doctor.default_stt_ready_check(args(doctor))

        assert result.status == "warn"
        assert result.detail == "parakeet-server not reachable: no port"
        assert result.fix == doctor._PARAKEET_CPP_START_FIX

    def test_default_stt_fix_strings_are_distinct(self, doctor):
        assert doctor._PARAKEET_CPP_INSTALL_FIX != doctor._PARAKEET_CPP_START_FIX

    def test_skips_when_configured_backend_is_not_parakeet(
        self, doctor, monkeypatch, tmp_path
    ):
        journal = tmp_path / "journal"
        config_path = journal / "config" / "journal.json"
        config_path.parent.mkdir(parents=True)
        config_path.write_text(
            json.dumps({"transcribe": {"backend": "confidential"}}),
            encoding="utf-8",
        )
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
        monkeypatch.setattr(
            doctor.parakeet_readiness,
            "_platform_info",
            lambda: pytest.fail("platform should not be checked"),
        )

        result = doctor.default_stt_ready_check(args(doctor))

        assert result.status == "skip"
        assert "confidential" in result.detail

    def test_journal_less_host_defaults_to_parakeet_without_creating_journal(
        self, doctor, monkeypatch, tmp_path
    ):
        journal = tmp_path / "missing-journal"
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
        monkeypatch.setattr(
            doctor.parakeet_readiness,
            "_platform_info",
            lambda: ("linux", "x86_64"),
        )

        result = doctor.default_stt_ready_check(args(doctor))

        assert result.status == "warn"
        assert "missing" in result.detail
        assert "configured backend" not in result.detail
        assert not journal.exists()

    def test_skips_arm64_linux_in_runner(self, doctor, monkeypatch, tmp_path):
        self.setup_linux_parakeet_default(doctor, monkeypatch, tmp_path)
        monkeypatch.setattr(
            doctor.parakeet_readiness,
            "_platform_info",
            lambda: ("linux", "aarch64"),
        )

        result = doctor.default_stt_ready_check(args(doctor))

        assert result.status == "skip"
        assert result.detail == "parakeet not supported on this platform"

    def test_registered_for_journal_and_journal_readiness_only(self, doctor):
        journal_names = {check.name for check, _runner in doctor.JOURNAL_CHECKS}
        readiness_names = {check.name for check, _runner in doctor.READINESS_CHECKS}
        journal_readiness_names = {
            check.name for check, _runner in doctor.JOURNAL_READINESS_CHECKS
        }
        universal_names = {check.name for check, _runner in doctor.UNIVERSAL_CHECKS}

        assert "default_stt_ready" in journal_names
        assert "default_stt_ready" not in readiness_names
        assert "default_stt_ready" in journal_readiness_names
        assert "default_stt_ready" not in universal_names
        assert doctor.DEFAULT_STT_READY_CHECK in {
            check for check, _runner in doctor.JOURNAL_CHECKS
        }
        assert doctor.DEFAULT_STT_READY_CHECK in {
            check for check, _runner in doctor.JOURNAL_READINESS_CHECKS
        }
        assert doctor.DEFAULT_STT_READY_CHECK not in {
            check for check, _runner in doctor.READINESS_CHECKS
        }
        assert doctor.CHECK_MAP["default_stt_ready"].severity == "advisory"


class TestParakeetCppSttReady:
    @staticmethod
    def make_ready_files(doctor, cache_root: Path, artifact_key: str) -> None:
        for backend in doctor.parakeet_readiness.PARAKEET_CPP_BINARY_BACKENDS:
            path = doctor.parakeet_readiness.parakeet_cpp_binary_path(
                cache_root, artifact_key, backend
            )
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("#!/bin/sh\n", encoding="utf-8")
            path.chmod(0o755)
        model_path = doctor.parakeet_readiness.parakeet_cpp_model_path(cache_root)
        model_path.parent.mkdir(parents=True, exist_ok=True)
        model_path.write_text("model", encoding="utf-8")

    def test_registered_for_journal_and_journal_readiness_only(self, doctor):
        pair = (
            doctor.PARAKEET_CPP_STT_READY_CHECK,
            doctor.parakeet_cpp_stt_ready_check,
        )

        assert doctor.PARAKEET_CPP_STT_READY_CHECK.name in doctor.CHECK_MAP
        assert pair in doctor.JOURNAL_CHECKS
        assert pair in doctor.JOURNAL_READINESS_CHECKS
        assert pair not in doctor.UNIVERSAL_CHECKS
        assert pair not in doctor.READINESS_CHECKS

    def test_skips_when_backend_is_not_parakeet_cpp(self, doctor, monkeypatch):
        monkeypatch.setattr(doctor, "_resolve_configured_backend", lambda: "parakeet")

        result = doctor.parakeet_cpp_stt_ready_check(args(doctor))

        assert result.status == "skip"
        assert (
            result.detail
            == "configured backend is not parakeet-cpp; check not applicable"
        )

    def test_skips_off_linux(self, doctor, monkeypatch):
        monkeypatch.setattr(
            doctor, "_resolve_configured_backend", lambda: "parakeet-cpp"
        )
        monkeypatch.setattr(
            doctor.parakeet_readiness,
            "_platform_info",
            lambda: ("darwin", "arm64"),
        )

        result = doctor.parakeet_cpp_stt_ready_check(args(doctor))

        assert result.status == "skip"
        assert result.detail == "parakeet-cpp is only supported on Linux"

    def test_warns_with_install_fix_when_files_missing(
        self, doctor, monkeypatch, tmp_path
    ):
        journal = tmp_path / "journal"
        monkeypatch.setattr(
            doctor, "_resolve_configured_backend", lambda: "parakeet-cpp"
        )
        monkeypatch.setattr(
            doctor.parakeet_readiness,
            "_platform_info",
            lambda: ("linux", "x86_64"),
        )
        monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))

        result = doctor.parakeet_cpp_stt_ready_check(args(doctor))

        assert result.status == "warn"
        assert "missing" in result.detail
        assert result.fix == doctor._PARAKEET_CPP_INSTALL_FIX

    def test_warns_with_start_fix_when_server_not_ready(
        self, doctor, monkeypatch, tmp_path
    ):
        from solstone.think.providers import parakeet_server

        journal = tmp_path / "journal"
        artifact_key = "x86_64-unknown-linux-gnu"
        cache_root = doctor.parakeet_readiness.parakeet_cpp_cache_root(journal)
        self.make_ready_files(doctor, cache_root, artifact_key)
        monkeypatch.setattr(
            doctor, "_resolve_configured_backend", lambda: "parakeet-cpp"
        )
        monkeypatch.setattr(
            doctor.parakeet_readiness,
            "_platform_info",
            lambda: ("linux", "x86_64"),
        )
        monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))
        monkeypatch.setattr(
            parakeet_server,
            "probe_state",
            lambda: (parakeet_server.STATE_FAILED, "no port"),
        )

        result = doctor.parakeet_cpp_stt_ready_check(args(doctor))

        assert result.status == "warn"
        assert result.detail == "parakeet-server not reachable: no port"
        assert result.fix == doctor._PARAKEET_CPP_START_FIX

    def test_warns_with_openmp_fix_before_server_probe(
        self, doctor, monkeypatch, tmp_path
    ):
        from solstone.think.providers import parakeet_server

        journal = tmp_path / "journal"
        artifact_key = "x86_64-unknown-linux-gnu"
        cache_root = doctor.parakeet_readiness.parakeet_cpp_cache_root(journal)
        self.make_ready_files(doctor, cache_root, artifact_key)
        monkeypatch.setattr(
            doctor, "_resolve_configured_backend", lambda: "parakeet-cpp"
        )
        monkeypatch.setattr(
            doctor.parakeet_readiness,
            "_platform_info",
            lambda: ("linux", "x86_64"),
        )
        monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))
        monkeypatch.setattr(
            doctor.parakeet_readiness,
            "probe_parakeet_cpp_binary",
            lambda _path: doctor.parakeet_readiness.ParakeetCppProbe(
                runnable=False,
                reason_code="openmp_runtime_unavailable",
                detail=(
                    "error while loading shared libraries: libgomp.so.1: "
                    "cannot open shared object file"
                ),
            ),
        )
        monkeypatch.setattr(
            doctor.parakeet_readiness.platform,
            "freedesktop_os_release",
            lambda: {"ID": "fedora"},
        )
        monkeypatch.setattr(
            parakeet_server,
            "probe_state",
            lambda: pytest.fail("server probe must follow binary readiness"),
        )

        result = doctor.parakeet_cpp_stt_ready_check(args(doctor))

        assert result.status == "warn"
        assert result.detail == (
            "parakeet-cpp cannot start: OpenMP runtime unavailable (libgomp.so.1)"
        )
        assert result.fix == (
            "install the OpenMP runtime with: sudo dnf install libgomp"
        )

    def test_ok_when_files_present_and_server_ready(
        self, doctor, monkeypatch, tmp_path
    ):
        from solstone.think.providers import parakeet_server

        journal = tmp_path / "journal"
        artifact_key = "x86_64-unknown-linux-gnu"
        cache_root = doctor.parakeet_readiness.parakeet_cpp_cache_root(journal)
        self.make_ready_files(doctor, cache_root, artifact_key)
        monkeypatch.setattr(
            doctor, "_resolve_configured_backend", lambda: "parakeet-cpp"
        )
        monkeypatch.setattr(
            doctor.parakeet_readiness,
            "_platform_info",
            lambda: ("linux", "x86_64"),
        )
        monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))
        monkeypatch.setattr(
            parakeet_server,
            "probe_state",
            lambda: (parakeet_server.STATE_READY, None),
        )

        result = doctor.parakeet_cpp_stt_ready_check(args(doctor))

        assert result.status == "ok"
        assert result.detail == (
            "parakeet-cpp ready (binaries + model installed, server reachable)"
        )

    def test_does_not_create_absent_journal_when_not_configured(
        self, doctor, monkeypatch, tmp_path
    ):
        journal = tmp_path / "missing-journal"
        monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))

        result = doctor.parakeet_cpp_stt_ready_check(args(doctor))

        assert result.status == "skip"
        assert not journal.exists()


class TestSpeakersAnalyzeInstallation:
    def test_registered_for_journal_and_journal_readiness_only(self, doctor):
        pair = (
            doctor.SPEAKERS_ANALYZE_INSTALLATION_CHECK,
            doctor.speakers_analyze_installation_check,
        )

        assert doctor.SPEAKERS_ANALYZE_INSTALLATION_CHECK.name in doctor.CHECK_MAP
        assert pair in doctor.JOURNAL_CHECKS
        assert pair in doctor.JOURNAL_READINESS_CHECKS
        assert pair not in doctor.UNIVERSAL_CHECKS
        assert pair not in doctor.READINESS_CHECKS
        assert doctor.CHECK_MAP["speakers_analyze_installation"].severity == "blocker"

    def test_ok_when_shared_invariant_is_ok(self, doctor, monkeypatch):
        from solstone.think import speakers_analyze_installation as installation

        monkeypatch.setattr(
            installation,
            "check_speakers_analyze_installation",
            lambda: installation.SpeakersAnalyzeInstallationResult("ok"),
        )

        result = doctor.speakers_analyze_installation_check(args(doctor))

        assert result.status == "ok"
        assert result.detail == "speakers-analyze installation ready"

    def test_fails_with_shared_repair_text(self, doctor, monkeypatch):
        from solstone.think import speakers_analyze_installation as installation

        monkeypatch.setattr(
            installation,
            "check_speakers_analyze_installation",
            lambda: installation.SpeakersAnalyzeInstallationResult(
                "asset-missing", "wespeaker"
            ),
        )

        result = doctor.speakers_analyze_installation_check(args(doctor))

        assert result.status == "fail"
        assert "asset-missing" in result.detail
        assert result.fix == installation.speakers_analyze_repair_text()


class TestHostDependencies:
    def test_client_readiness_battery_is_host_free(self, doctor, monkeypatch):
        expected = [
            "python_version",
            "sol_importable",
            "local_bin_sol_reachable",
            "stale_alias_symlink",
            "disk_space",
            "journal_dir_writable",
        ]
        readiness_names = [check.name for check, _runner in doctor.READINESS_CHECKS]

        assert readiness_names == expected
        for name in [
            "host_dependencies",
            "default_stt_ready",
            "speakers_analyze_installation",
            "feature:pdf-import",
            "feature:pdf-export",
        ]:
            assert name not in readiness_names

        monkeypatch.setattr(doctor.sys, "argv", ["sol doctor"])
        assert "host_dependencies" not in {
            check.name for check, _runner in doctor.select_battery(args(doctor))
        }
        assert "host_dependencies" not in {
            check.name
            for check, _runner in doctor.select_battery(
                doctor.parse_args(["--readiness"])
            )
        }

    def test_journal_readiness_selects_host_stack_first(self, doctor, monkeypatch):
        parsed = doctor.parse_args(["--readiness"])
        monkeypatch.setattr(doctor.sys, "argv", ["journal doctor"])

        selected = doctor.select_battery(parsed)

        assert selected is doctor.JOURNAL_READINESS_CHECKS
        assert selected[0][0] is doctor.HOST_DEPENDENCIES_CHECK
        assert selected[0][0].name == "host_dependencies"
        assert "default_stt_ready" in {check.name for check, _runner in selected}
        assert "speakers_analyze_installation" in {
            check.name for check, _runner in selected
        }
        assert "feature:pdf-import" in {check.name for check, _runner in selected}
        assert "feature:pdf-export" in {check.name for check, _runner in selected}

    def test_host_dependencies_registered_in_journal_and_check_map(self, doctor):
        assert "host_dependencies" in {
            check.name for check, _runner in doctor.JOURNAL_CHECKS
        }
        assert "host_dependencies" in {
            check.name for check, _runner in doctor.JOURNAL_READINESS_CHECKS
        }
        assert "host_dependencies" not in {
            check.name for check, _runner in doctor.READINESS_CHECKS
        }
        assert doctor.CHECK_MAP["host_dependencies"].severity == "blocker"

    def test_maint_tasks_registered_in_journal_and_check_map(self, doctor):
        pair = (
            doctor.JOURNAL_MAINT_TASKS_CHECK,
            doctor.journal_maint_tasks_check,
        )

        assert doctor.JOURNAL_MAINT_TASKS_CHECK.name == "journal_maint_tasks"
        assert doctor.JOURNAL_MAINT_TASKS_CHECK.name in doctor.CHECK_MAP
        assert doctor.CHECK_MAP["journal_maint_tasks"].severity == "blocker"
        assert pair in doctor.JOURNAL_CHECKS
        assert pair not in doctor.UNIVERSAL_CHECKS
        assert pair not in doctor.READINESS_CHECKS
        assert pair not in doctor.JOURNAL_READINESS_CHECKS

    def test_host_dependencies_ok_when_present(self, doctor, monkeypatch):
        monkeypatch.setattr(doctor, "_host_module_present", lambda _module: True)

        result = doctor.host_dependencies_check(args(doctor))

        assert result.status == "ok"
        assert result.detail == (
            "journal host dependencies present: python-frontmatter, Flask, ONNX runtime"
        )

    @pytest.mark.parametrize(
        ("missing_module", "label"),
        [
            ("frontmatter", "python-frontmatter"),
            ("flask", "Flask"),
            ("onnxruntime", "ONNX runtime"),
        ],
    )
    def test_host_dependencies_fail_when_find_spec_returns_none(
        self, doctor, monkeypatch, missing_module, label
    ):
        def fake_find_spec(module):
            return None if module == missing_module else object()

        monkeypatch.setattr(doctor.importlib.util, "find_spec", fake_find_spec)

        result = doctor.host_dependencies_check(args(doctor))

        assert result.status == "fail"
        assert result.severity == "blocker"
        assert label in result.detail
        assert "journal host stack incomplete" in result.detail
        assert result.fix == doctor.HOST_DEPENDENCY_REINSTALL_GUIDANCE

    def test_host_dependencies_fail_when_find_spec_raises(self, doctor, monkeypatch):
        def fake_find_spec(module):
            if module == "frontmatter":
                raise ModuleNotFoundError("No module named 'frontmatter'")
            return object()

        monkeypatch.setattr(doctor.importlib.util, "find_spec", fake_find_spec)

        result = doctor.host_dependencies_check(args(doctor))

        assert result.status == "fail"
        assert "python-frontmatter" in result.detail
        assert result.fix == doctor.HOST_DEPENDENCY_REINSTALL_GUIDANCE


class TestPackagingChecks:
    def set_versions(self, doctor, monkeypatch, **overrides):
        versions = {
            "solstone": None,
            "solstone-journal": None,
            "solstone-journal-cuda": None,
            "solstone-journal-host": None,
        }
        versions.update(overrides)
        monkeypatch.setattr(doctor, "_installed_packaging_versions", lambda: versions)

    def test_leaf_exclusivity_fails_when_both_leaves_installed(
        self, doctor, monkeypatch
    ):
        self.set_versions(
            doctor,
            monkeypatch,
            **{
                "solstone-journal": "9.9.9",
                "solstone-journal-cuda": "9.9.8",
            },
        )

        result = doctor.journal_leaf_exclusivity_check(args(doctor))

        assert result.status == "fail"
        assert result.severity == "blocker"
        assert "solstone-journal 9.9.9" in result.detail
        assert "solstone-journal-cuda 9.9.8" in result.detail
        assert "CPU and CUDA ONNX runtimes own the same files" in result.detail

    @pytest.mark.parametrize(
        ("leaf_name", "other_leaf"),
        [
            ("solstone-journal", "solstone-journal-cuda"),
            ("solstone-journal-cuda", "solstone-journal"),
        ],
    )
    def test_package_version_fails_when_leaf_mismatches_solstone(
        self, doctor, monkeypatch, leaf_name, other_leaf
    ):
        self.set_versions(
            doctor,
            monkeypatch,
            solstone="9.9.9",
            **{leaf_name: "9.9.8"},
        )

        result = doctor.journal_package_version_check(args(doctor))

        assert result.status == "fail"
        assert result.severity == "blocker"
        assert leaf_name in result.detail
        if other_leaf == "solstone-journal":
            assert "solstone-journal " not in result.detail
        else:
            assert other_leaf not in result.detail
        assert "9.9.9" in result.detail
        assert "9.9.8" in result.detail
        assert leaf_name in (result.fix or "")

    def test_retired_host_shim_warns_alongside_leaf(self, doctor, monkeypatch):
        self.set_versions(
            doctor,
            monkeypatch,
            **{
                "solstone-journal": "9.9.9",
                "solstone-journal-host": "0.7.0",
            },
        )

        result = doctor.retired_host_shim_check(args(doctor))

        assert result.status == "warn"
        assert result.severity == "advisory"
        assert "solstone-journal-host 0.7.0" in result.detail
        assert "solstone-journal 9.9.9" in result.detail
        assert result.fix == "pip uninstall solstone-journal-host"

    def test_healthy_cuda_leaf_is_ok(self, doctor, monkeypatch):
        self.set_versions(
            doctor,
            monkeypatch,
            solstone="9.9.9",
            **{"solstone-journal-cuda": "9.9.9"},
        )

        exclusivity = doctor.journal_leaf_exclusivity_check(args(doctor))
        version = doctor.journal_package_version_check(args(doctor))
        shim = doctor.retired_host_shim_check(args(doctor))

        assert exclusivity.status == "ok"
        assert version.status == "ok"
        assert shim.status == "ok"

    def test_journal_readiness_json_fails_on_missing_host_dependency(
        self, doctor, monkeypatch, capsys
    ):
        def fake_find_spec(module):
            return None if module == "frontmatter" else object()

        monkeypatch.setattr(doctor.importlib.util, "find_spec", fake_find_spec)
        monkeypatch.setattr(doctor.sys, "argv", ["journal doctor"])

        rc = doctor.main(["--readiness", "--json"])
        payload = json.loads(capsys.readouterr().out)
        host_check = next(
            check for check in payload["checks"] if check["name"] == "host_dependencies"
        )

        assert rc == 1
        assert host_check["severity"] == "blocker"
        assert host_check["status"] == "fail"
        assert "python-frontmatter" in host_check["detail"]
        assert host_check["fix"] == doctor.HOST_DEPENDENCY_REINSTALL_GUIDANCE

    def test_journal_readiness_jsonl_fails_on_missing_host_dependency(
        self, doctor, monkeypatch, capsys
    ):
        def fake_find_spec(module):
            return None if module == "flask" else object()

        monkeypatch.setattr(doctor.importlib.util, "find_spec", fake_find_spec)
        monkeypatch.setattr(doctor.sys, "argv", ["journal doctor"])

        rc = doctor.main(["--readiness", "--jsonl"])
        events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
        host_index, host_event = next(
            (index, event)
            for index, event in enumerate(events)
            if event["event"] == "check.completed"
            and event["name"] == "host_dependencies"
        )
        completed_index = next(
            index
            for index, event in enumerate(events)
            if event["event"] == "doctor.completed"
        )

        assert rc == 1
        assert host_index < completed_index
        assert host_event["severity"] == "blocker"
        assert host_event["status"] == "failed"
        assert "Flask" in host_event["detail"]
        assert host_event["fix"] == doctor.HOST_DEPENDENCY_REINSTALL_GUIDANCE
        assert events[completed_index]["status"] == "failed"


class TestPortCheckRemoved:
    def test_port_check_is_not_registered(self, doctor):
        assert "port_5015_free" not in doctor.CHECK_MAP
        assert "port_5015_free" not in {
            check.name for check, _runner in doctor.UNIVERSAL_CHECKS
        }


def test_default_universal_battery_check_names(doctor):
    assert {check.name for check, _runner in doctor.UNIVERSAL_CHECKS} == {
        "python_version",
        "sol_importable",
        "journal_leaf_exclusivity",
        "journal_package_version",
        "retired_host_shim",
        "local_bin_sol_reachable",
        "stale_alias_symlink",
        "skill_state",
    }


class TestStaleAliasSymlink:
    @pytest.fixture(autouse=True)
    def isolated_legacy_backups(self, doctor, monkeypatch, tmp_path):
        backup_dir = tmp_path / "legacy-backups"
        backup_dir.mkdir()
        monkeypatch.setattr(install_guard, "_legacy_backup_dir", lambda: backup_dir)
        self.backup_dir = backup_dir

    def setup_import(self, doctor, monkeypatch):
        monkeypatch.setattr(
            doctor,
            "import_install_guard",
            lambda: (install_guard.AliasState, install_guard.check_alias),
        )

    @staticmethod
    def make_existing_target(path: Path) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")
        return path

    @staticmethod
    def write_executable_script(path: Path, content: str = "#!/bin/sh\n") -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        path.chmod(0o755)
        return path

    def patch_runtime_bin(self, monkeypatch, tmp_path: Path) -> Path:
        bin_dir = tmp_path / "runtime" / "bin"
        for binary in ("python", "sol", "journal"):
            self.write_executable_script(bin_dir / binary)
        monkeypatch.setattr(sys, "executable", str(bin_dir / "python"))
        return bin_dir

    @staticmethod
    def write_regular_alias(home_root: Path, binary: str, content: str) -> Path:
        alias = home_root / ".local" / "bin" / binary
        alias.parent.mkdir(parents=True, exist_ok=True)
        alias.write_text(content, encoding="utf-8")
        alias.chmod(0o755)
        return alias

    @staticmethod
    def assert_foreign_alias_warning(result, binary: str) -> None:
        assert result.status == "warn"
        assert result.detail == f"~/.local/bin/{binary} exists but is not a symlink"

    def run_app_owned_check(
        self,
        doctor,
        monkeypatch,
        home_root: Path,
        tmp_path: Path,
        binary: str,
        content: str,
    ):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        self.write_regular_alias(home_root, binary, content)
        monkeypatch.setattr(doctor, "ROOT", repo)
        return doctor.stale_alias_symlink_check(args(doctor), binary=binary)

    @staticmethod
    def legacy_target(
        home_root: Path, legacy_parts: tuple[str, ...], binary: str
    ) -> Path:
        return home_root.joinpath(*legacy_parts) / "bin" / binary

    def assert_report_only_alias(
        self,
        result,
        alias: Path,
        original_target: str,
        expected_tag: str,
        other_binary: str,
        home_root: Path,
    ) -> None:
        assert result.status == "warn"
        assert expected_tag in result.detail
        assert result.fix is not None
        assert "journal setup" in result.fix
        assert alias.is_symlink()
        assert os.readlink(alias) == original_target
        assert list(self.backup_dir.glob("*.old-symlink-*")) == []
        assert not (home_root / ".local" / "bin" / other_binary).exists()

    @pytest.mark.parametrize("binary", ("sol", "journal"))
    def test_app_owned_child_launcher_ok(
        self, doctor, monkeypatch, home_root, tmp_path, binary
    ):
        bin_dir = self.patch_runtime_bin(monkeypatch, tmp_path)
        expected = bin_dir / binary
        foreign = self.write_executable_script(
            tmp_path / "foreign-runtime" / "bin" / binary
        )
        assert expected != foreign
        assert os.access(expected, os.X_OK)
        assert os.access(foreign, os.X_OK)
        content = install_guard.render_app_owned_child_launcher(str(expected), binary)

        result = self.run_app_owned_check(
            doctor, monkeypatch, home_root, tmp_path, binary, content
        )

        assert result.status == "ok"
        assert "is not a symlink" not in result.detail
        assert result.fix is None

    @pytest.mark.parametrize("binary", ("sol", "journal"))
    def test_app_owned_child_launcher_marker_only_warns(
        self, doctor, monkeypatch, home_root, tmp_path, binary
    ):
        self.patch_runtime_bin(monkeypatch, tmp_path)
        content = "#!/bin/sh\n# managed-version: app-owned-child\n"

        result = self.run_app_owned_check(
            doctor, monkeypatch, home_root, tmp_path, binary, content
        )

        self.assert_foreign_alias_warning(result, binary)

    @pytest.mark.parametrize("binary", ("sol", "journal"))
    def test_app_owned_child_launcher_malformed_body_warns(
        self, doctor, monkeypatch, home_root, tmp_path, binary
    ):
        self.patch_runtime_bin(monkeypatch, tmp_path)
        content = (
            '#!/bin/sh\n# managed-version: app-owned-child\nexec /not-quoted "$@"\n'
        )

        result = self.run_app_owned_check(
            doctor, monkeypatch, home_root, tmp_path, binary, content
        )

        self.assert_foreign_alias_warning(result, binary)

    @pytest.mark.parametrize("binary", ("sol", "journal"))
    def test_app_owned_child_launcher_other_binary_warns(
        self, doctor, monkeypatch, home_root, tmp_path, binary
    ):
        bin_dir = self.patch_runtime_bin(monkeypatch, tmp_path)
        other_binary = "journal" if binary == "sol" else "sol"
        content = install_guard.render_app_owned_child_launcher(
            str(bin_dir / other_binary),
            other_binary,
        )

        result = self.run_app_owned_check(
            doctor, monkeypatch, home_root, tmp_path, binary, content
        )

        self.assert_foreign_alias_warning(result, binary)

    @pytest.mark.parametrize("binary", ("sol", "journal"))
    def test_app_owned_child_launcher_missing_target_warns(
        self, doctor, monkeypatch, home_root, tmp_path, binary
    ):
        bin_dir = self.patch_runtime_bin(monkeypatch, tmp_path)
        expected = bin_dir / binary
        content = install_guard.render_app_owned_child_launcher(str(expected), binary)
        expected.unlink()

        result = self.run_app_owned_check(
            doctor, monkeypatch, home_root, tmp_path, binary, content
        )

        self.assert_foreign_alias_warning(result, binary)

    @pytest.mark.parametrize("binary", ("sol", "journal"))
    def test_app_owned_child_launcher_non_executable_target_warns(
        self, doctor, monkeypatch, home_root, tmp_path, binary
    ):
        bin_dir = self.patch_runtime_bin(monkeypatch, tmp_path)
        expected = bin_dir / binary
        content = install_guard.render_app_owned_child_launcher(str(expected), binary)
        expected.chmod(0o644)

        result = self.run_app_owned_check(
            doctor, monkeypatch, home_root, tmp_path, binary, content
        )

        self.assert_foreign_alias_warning(result, binary)

    @pytest.mark.parametrize("binary", ("sol", "journal"))
    def test_app_owned_child_launcher_outside_executable_target_warns(
        self, doctor, monkeypatch, home_root, tmp_path, binary
    ):
        bin_dir = self.patch_runtime_bin(monkeypatch, tmp_path)
        expected = bin_dir / binary
        foreign = self.write_executable_script(
            tmp_path / "foreign-runtime" / "bin" / binary
        )
        assert expected != foreign
        content = install_guard.render_app_owned_child_launcher(str(foreign), binary)

        result = self.run_app_owned_check(
            doctor, monkeypatch, home_root, tmp_path, binary, content
        )

        self.assert_foreign_alias_warning(result, binary)

    @pytest.mark.parametrize("binary", ("sol", "journal"))
    def test_app_owned_child_launcher_outside_symlink_to_expected_target_warns(
        self, doctor, monkeypatch, home_root, tmp_path, binary
    ):
        bin_dir = self.patch_runtime_bin(monkeypatch, tmp_path)
        expected = bin_dir / binary
        outside = tmp_path / "foreign-runtime" / "bin" / binary
        outside.parent.mkdir(parents=True, exist_ok=True)
        outside.symlink_to(expected)
        assert outside.resolve() == expected
        content = install_guard.render_app_owned_child_launcher(str(outside), binary)

        result = self.run_app_owned_check(
            doctor, monkeypatch, home_root, tmp_path, binary, content
        )

        self.assert_foreign_alias_warning(result, binary)

    def test_absent_ok(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "ok"

    def test_owned_ok(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        make_alias(home_root, ensure_expected_target(repo))
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "ok"

    def test_cross_repo_warns(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        make_alias(home_root, other_target(tmp_path))
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "warn"
        assert list(self.backup_dir.glob("sol.old-symlink-*")) == []

    def test_dangling_warns(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        missing = tmp_path / "missing" / ".venv" / "bin" / "sol"
        make_alias(home_root, missing)
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "warn"
        assert (home_root / ".local" / "bin" / "sol").is_symlink()

    def test_not_symlink_warns(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        alias = home_root / ".local" / "bin" / "sol"
        alias.parent.mkdir(parents=True, exist_ok=True)
        alias.write_text("not a symlink", encoding="utf-8")
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "warn"
        assert list(self.backup_dir.glob("sol.old-symlink-*")) == []

    def test_worktree_skip(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path, worktree=True)
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "skip"

    def test_import_failure_skips(self, doctor, monkeypatch):
        monkeypatch.setattr(
            doctor,
            "import_install_guard",
            lambda: (_ for _ in ()).throw(ImportError("boom")),
        )
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "skip"
        assert "could not import solstone.think.install_guard" in result.detail

    @pytest.mark.parametrize(
        ("legacy_parts", "expected_tag"),
        [
            ((".local", "share", "uv", "tools", "solstone"), "uv-tool"),
            ((".local", "share", "pipx", "venvs", "solstone"), "pipx-xdg"),
            ((".local", "pipx", "venvs", "solstone"), "pipx-legacy"),
        ],
    )
    @pytest.mark.parametrize("binary", ("sol", "journal"))
    def test_legacy_alias_reports_only(
        self,
        doctor,
        monkeypatch,
        home_root,
        tmp_path,
        legacy_parts,
        expected_tag,
        binary,
    ):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        target = self.make_existing_target(
            self.legacy_target(home_root, legacy_parts, binary)
        )
        alias = make_alias(home_root, target, binary=binary)
        original_target = os.readlink(alias)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary=binary)

        other_binary = "journal" if binary == "sol" else "sol"
        self.assert_report_only_alias(
            result,
            alias,
            original_target,
            expected_tag,
            other_binary,
            home_root,
        )

    @pytest.mark.parametrize(
        ("legacy_parts", "expected_tag"),
        [
            ((".local", "share", "uv", "tools", "solstone"), "uv-tool"),
            ((".local", "share", "pipx", "venvs", "solstone"), "pipx-xdg"),
        ],
    )
    @pytest.mark.parametrize("binary", ("sol", "journal"))
    def test_dangling_legacy_alias_reports_only(
        self,
        doctor,
        monkeypatch,
        home_root,
        tmp_path,
        legacy_parts,
        expected_tag,
        binary,
    ):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        target = self.legacy_target(home_root, legacy_parts, binary)
        target.parent.mkdir(parents=True, exist_ok=True)
        alias = make_alias(home_root, target, binary=binary)
        original_target = os.readlink(alias)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary=binary)

        other_binary = "journal" if binary == "sol" else "sol"
        self.assert_report_only_alias(
            result,
            alias,
            original_target,
            expected_tag,
            other_binary,
            home_root,
        )

    def test_non_legacy_target_warns(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        target = self.make_existing_target(
            tmp_path / "opt" / "random" / "foo" / "bin" / "sol"
        )
        make_alias(home_root, target)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")

        assert result.status == "warn"
        assert list(self.backup_dir.glob("sol.old-symlink-*")) == []

    def test_partial_migration_recovery_detail(
        self, doctor, monkeypatch, home_root, tmp_path
    ):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        backup = self.backup_dir / "sol.old-symlink-20260101000000"
        backup.write_text("", encoding="utf-8")
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")

        assert result.status == "warn"
        assert "partial migration detected" in result.detail
        assert str(backup) in result.detail

    def test_journal_binary_not_owned_warns(
        self, doctor, monkeypatch, home_root, tmp_path
    ):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        target = tmp_path / "other" / ".venv" / "bin" / "journal"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("", encoding="utf-8")
        make_alias(home_root, target, binary="journal")
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="journal")

        assert result.status == "warn"

    def test_journal_battery_warn_does_not_fail_exit(
        self, doctor, monkeypatch, home_root, tmp_path, capsys
    ):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        target = tmp_path / "other" / ".venv" / "bin" / "journal"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("", encoding="utf-8")
        make_alias(home_root, target, binary="journal")
        monkeypatch.setattr(doctor, "ROOT", repo)

        def journal_stale_check(check_args):
            return doctor.stale_alias_symlink_check(check_args, binary="journal")

        monkeypatch.setattr(
            doctor,
            "select_battery",
            lambda _args: [(doctor.STALE_ALIAS_CHECK, journal_stale_check)],
        )

        assert doctor.main([]) == 0
        assert "1 warnings" in capsys.readouterr().out


def test_capture_health_check_omits_unbound_suffix(doctor, monkeypatch):
    monkeypatch.setattr(
        doctor,
        "get_capture_health",
        lambda: {
            "status": "degraded",
            "observers": [{"name": "desktop", "status": "degraded", "unbound": True}],
        },
    )

    result = doctor.capture_health_check(args(doctor))

    assert result.status == "warn"
    assert "rollup=degraded" in result.detail
    assert "desktop=degraded" in result.detail
    assert "(unbound)" not in result.detail
    assert result.fix == doctor._CAPTURE_HEALTH_FIX


class TestJsonAndExitCodes:
    def test_json_output(self, doctor, monkeypatch, capsys):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult("a", "blocker", "ok", "fine", None),
                doctor.CheckResult("b", "advisory", "warn", "careful", "fix me"),
            ],
        )
        rc = doctor.main(["--json"])
        payload = json.loads(capsys.readouterr().out)
        assert rc == 0
        assert sorted(payload) == ["checks", "summary"]
        assert set(payload["checks"][0]) == {
            "name",
            "severity",
            "status",
            "detail",
            "fix",
            "execution_error",
        }
        assert payload["checks"][0]["execution_error"] is None
        assert list(payload["summary"]) == [
            "total",
            "failed",
            "warnings",
            "skipped",
            "errors",
        ]
        assert payload["summary"]["errors"] == 0

    def test_exit_code_matrix(self, doctor, monkeypatch, capsys):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [doctor.CheckResult("a", "blocker", "fail", "boom", None)],
        )
        assert doctor.main([]) == 1
        capsys.readouterr()

        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [doctor.CheckResult("a", "advisory", "fail", "boom", None)],
        )
        assert doctor.main([]) == 0
        capsys.readouterr()

        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [doctor.CheckResult("a", "blocker", "skip", "skip", None)],
        )
        assert doctor.main([]) == 0

    def test_advisory_execution_error_returns_nonzero(
        self, doctor, monkeypatch, capsys
    ):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult(
                    "a",
                    "advisory",
                    "fail",
                    "check execution failed: RuntimeError: boom",
                    None,
                    execution_error=ExecutionError("RuntimeError", "boom"),
                )
            ],
        )

        assert doctor.main([]) == 1
        output = capsys.readouterr().out
        assert "ERROR a" in output
        assert output.rstrip().endswith(
            "doctor: 1 checks, 1 failed, 0 warnings, 0 skipped, 1 errors"
        )

    def test_summary_line_format(self, doctor, monkeypatch, capsys):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult("a", "blocker", "fail", "boom", None),
                doctor.CheckResult("b", "advisory", "warn", "warn", None),
                doctor.CheckResult("c", "blocker", "skip", "skip", None),
            ],
        )
        doctor.main([])
        output = capsys.readouterr().out.strip().splitlines()
        assert (
            output[-1] == "doctor: 3 checks, 1 failed, 1 warnings, 1 skipped, 0 errors"
        )

    def test_run_checks_isolates_exception_and_continues(self, doctor):
        before_check = doctor.Check("before_check", "blocker", ("linux", "darwin"))
        raising_check = doctor.Check("raising_check", "advisory", ("linux", "darwin"))
        after_check = doctor.Check("after_check", "blocker", ("linux", "darwin"))
        calls: list[str] = []

        def before(_args):
            calls.append("before")
            return doctor.make_result(before_check, "ok", "before complete")

        def raising(_args):
            calls.append("raising")
            raise RuntimeError("boom")

        def after(_args):
            calls.append("after")
            return doctor.make_result(after_check, "ok", "after complete")

        results = doctor.run_checks(
            args(doctor),
            checks=[
                (before_check, before),
                (raising_check, raising),
                (after_check, after),
            ],
        )

        assert [result.name for result in results] == [
            "before_check",
            "raising_check",
            "after_check",
        ]
        assert calls == ["before", "raising", "after"]
        error = results[1]
        assert error.status == "fail"
        assert error.severity == "advisory"
        assert error.fix is None
        assert error.execution_error == ExecutionError("RuntimeError", "boom")
        assert "RuntimeError: boom" in error.detail
        summary = doctor.summary_counts(results)
        assert summary["errors"] == 1
        assert summary["failed"] >= summary["errors"]

    @pytest.mark.parametrize("exception", [KeyboardInterrupt(), SystemExit(7)])
    def test_run_checks_propagates_base_exceptions(self, doctor, exception):
        raising_check = doctor.Check("raising_check", "blocker", ("linux", "darwin"))

        def raising(_args):
            raise exception

        with pytest.raises(type(exception)):
            doctor.run_checks(args(doctor), checks=[(raising_check, raising)])

    def test_run_checks_truncates_execution_error_without_traceback(self, doctor):
        raising_check = doctor.Check("raising_check", "blocker", ("linux", "darwin"))
        long_message = " ".join(["overflow"] * 120)

        def raising(_args):
            raise PermissionError(long_message)

        result = doctor.run_checks(
            args(doctor),
            checks=[(raising_check, raising)],
        )[0]

        assert doctor.has_execution_error(result)
        assert result.execution_error.type == "PermissionError"
        assert len(result.execution_error.message) <= 512
        assert result.execution_error.message.endswith("...")
        for public_text in [result.detail, result.execution_error.message]:
            assert "Traceback" not in public_text
            assert "line " not in public_text
            assert str(Path(__file__)) not in public_text

    def test_feature_run_isolates_exception(self, doctor, monkeypatch):
        feature_check = doctor.Check(
            "feature:raising-feature", "advisory", ("linux", "darwin")
        )

        def raising(_args):
            raise RuntimeError("feature boom")

        monkeypatch.setitem(
            doctor.FEATURE_CHECKS,
            "raising-feature",
            (feature_check, raising),
        )

        result = doctor.run_checks(args(doctor, feature="raising-feature"))[0]

        assert result.name == "feature:raising-feature"
        assert result.status == "fail"
        assert result.severity == "advisory"
        assert result.execution_error == ExecutionError("RuntimeError", "feature boom")

    def test_doctor_jsonl_emits_started_and_completed(
        self, doctor, monkeypatch, capsys
    ):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [doctor.CheckResult("a", "blocker", "ok", "fine", None)],
        )

        rc = doctor.main(["--jsonl"])
        events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]

        assert rc == 0
        assert events[0]["event"] == "doctor.started"
        assert events[-1]["event"] == "doctor.completed"
        assert events[-1]["status"] == "ok"
        assert events[-1]["summary"]["errors"] == 0

    def test_doctor_jsonl_emits_check_completed_per_check(
        self, doctor, monkeypatch, capsys
    ):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult(check.name, check.severity, "ok", "fine", None)
                for check, _func in doctor.UNIVERSAL_CHECKS
            ],
        )

        doctor.main(["--jsonl"])
        events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]

        checks = [event for event in events if event["event"] == "check.completed"]
        assert len(checks) == len(doctor.UNIVERSAL_CHECKS)
        assert all(check["execution_error"] is None for check in checks)

    def test_doctor_jsonl_status_translates_short_to_long(
        self, doctor, monkeypatch, capsys
    ):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult("ok", "blocker", "ok", "ok", None),
                doctor.CheckResult("warn", "advisory", "warn", "warn", None),
                doctor.CheckResult("fail", "advisory", "fail", "fail", None),
                doctor.CheckResult("skip", "blocker", "skip", "skip", None),
            ],
        )

        rc = doctor.main(["--jsonl"])
        events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]

        assert rc == 0
        statuses = {
            event["name"]: event["status"]
            for event in events
            if event["event"] == "check.completed"
        }
        assert statuses == {
            "ok": "ok",
            "warn": "warning",
            "fail": "failed",
            "skip": "skipped",
        }
        assert events[-1]["status"] == "warning"
        assert events[-1]["summary"]["errors"] == 0

    def test_doctor_jsonl_execution_error_fails_terminal_status(
        self, doctor, monkeypatch, capsys
    ):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult(
                    "advisory_error",
                    "advisory",
                    "fail",
                    "check execution failed: RuntimeError: boom",
                    None,
                    execution_error=ExecutionError("RuntimeError", "boom"),
                )
            ],
        )

        rc = doctor.main(["--jsonl"])
        events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]

        completed = [event for event in events if event["event"] == "check.completed"][
            0
        ]
        terminal = events[-1]
        assert rc == 1
        assert completed["status"] == "failed"
        assert completed["execution_error"] == {
            "type": "RuntimeError",
            "message": "boom",
        }
        assert terminal["event"] == "doctor.completed"
        assert terminal["status"] == "failed"
        assert terminal["summary"]["errors"] == 1

    def test_doctor_jsonl_json_and_jsonl_mutually_exclusive(self, doctor):
        with pytest.raises(SystemExit) as raised:
            doctor.main(["--json", "--jsonl"])

        assert raised.value.code == 2

    def test_doctor_jsonl_preserves_json_payload_unchanged(
        self, doctor, monkeypatch, capsys
    ):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult("a", "advisory", "warn", "careful", None)
            ],
        )

        rc = doctor.main(["--json"])
        payload = json.loads(capsys.readouterr().out)

        assert rc == 0
        assert payload["checks"][0]["status"] == "warn"
        assert payload["checks"][0]["execution_error"] is None

    def test_doctor_jsonl_subprocess_e2e(self):
        result = subprocess.run(
            [sys.executable, "-m", "solstone.think.sol_cli", "doctor", "--jsonl"],
            capture_output=True,
            text=True,
            cwd=ROOT,
            timeout=60,
        )

        assert result.returncode in (
            0,
            1,
        ), f"unexpected exit code {result.returncode}: {result.stderr}"
        lines = [line for line in result.stdout.splitlines() if line.strip()]
        assert lines
        for line in lines:
            json.loads(line)


def test_doctor_import_does_not_pull_numpy_or_observe_layers():
    snippet = (
        "import sys\n"
        "import solstone.think.doctor\n"
        "for m in sorted(sys.modules):\n"
        "    if (\n"
        "        m == 'numpy'\n"
        "        or m.startswith('numpy.')\n"
        "        or m == 'solstone.observe'\n"
        "        or m.startswith('solstone.observe.')\n"
        "    ):\n"
        "        print('LEAKED:' + m)\n"
        "        sys.exit(3)\n"
        "print('OK')\n"
    )
    result = subprocess.run(
        [sys.executable, "-c", snippet],
        capture_output=True,
        text=True,
        cwd=ROOT,
        env={
            **os.environ,
            "SOLSTONE_JOURNAL": str(ROOT / "tests" / "fixtures" / "journal"),
        },
        timeout=60,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert "LEAKED" not in result.stdout


def test_readiness_battery_does_not_import_inference_or_installer_layers():
    # Proves doctor's readiness battery does not import inference/installer layers.
    snippet = (
        "import sys\n"
        "from solstone.think import doctor\n"
        "doctor.run_checks(doctor.parse_args(['--readiness']))\n"
        "bad = sorted(\n"
        "    m\n"
        "    for m in sys.modules\n"
        "    if m == 'solstone.think.install_models'\n"
        "    or m == 'solstone.observe.transcribe'\n"
        "    or m.startswith('solstone.observe.transcribe.')\n"
        ")\n"
        "if bad:\n"
        "    print('LEAKED:' + ','.join(bad))\n"
        "    sys.exit(3)\n"
        "print('OK')\n"
    )
    result = subprocess.run(
        [sys.executable, "-c", snippet],
        capture_output=True,
        text=True,
        cwd=ROOT,
        env={
            **os.environ,
            "SOLSTONE_JOURNAL": str(ROOT / "tests" / "fixtures" / "journal"),
        },
        timeout=60,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert "LEAKED" not in result.stdout


def test_journal_readiness_battery_does_not_import_host_or_inference_modules():
    snippet = (
        "import sys\n"
        "sys.argv = ['journal doctor']\n"
        "from solstone.think import doctor\n"
        "doctor.run_checks(doctor.parse_args(['--readiness']))\n"
        "bad = sorted(\n"
        "    m\n"
        "    for m in sys.modules\n"
        "    if m in {'frontmatter', 'flask', 'onnxruntime', "
        "'solstone.think.install_models', 'solstone.observe.transcribe'}\n"
        "    or m.startswith('solstone.observe.transcribe.')\n"
        ")\n"
        "if bad:\n"
        "    print('LEAKED:' + ','.join(bad))\n"
        "    sys.exit(3)\n"
        "print('OK')\n"
    )
    result = subprocess.run(
        [sys.executable, "-c", snippet],
        capture_output=True,
        text=True,
        cwd=ROOT,
        env={
            **os.environ,
            "SOLSTONE_JOURNAL": str(ROOT / "tests" / "fixtures" / "journal"),
        },
        timeout=60,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert "LEAKED" not in result.stdout


def test_journal_doctor_readiness_subprocess_json_shape():
    """End-to-end: journal readiness doctor produces valid diagnostic JSON."""
    repo_root = Path(__file__).resolve().parent.parent
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "solstone.think.sol_cli",
            "doctor",
            "--readiness",
            "--json",
        ],
        capture_output=True,
        text=True,
        cwd=repo_root,
        timeout=60,
    )
    # Exit code: 0 if all checks pass, 1 if any blocker fails. Either is valid
    # here; this test asserts CLI routing and payload shape, not machine health.
    assert result.returncode in (
        0,
        1,
    ), f"unexpected exit code {result.returncode}: {result.stderr}"
    payload = json.loads(result.stdout)
    assert "checks" in payload and isinstance(payload["checks"], list)
    assert "summary" in payload and isinstance(payload["summary"], dict)
    assert list(payload["summary"]) == [
        "total",
        "failed",
        "warnings",
        "skipped",
        "errors",
    ]
    assert payload["summary"]["errors"] <= payload["summary"]["failed"]
    assert all(
        set(check)
        == {
            "name",
            "severity",
            "status",
            "detail",
            "fix",
            "execution_error",
        }
        for check in payload["checks"]
    )
    from solstone.think import doctor as doctor_module

    assert {check["name"] for check in payload["checks"]} == {
        check.name for check, _runner in doctor_module.JOURNAL_READINESS_CHECKS
    }


def test_doctor_runs_with_minimal_path_env(tmp_path):
    """Doctor must complete with PATH=/usr/bin:/bin (launchd-style minimal env)."""
    journal = tmp_path / "journal"
    journal.mkdir()
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(tmp_path),
        "SOLSTONE_JOURNAL": str(journal),
    }
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "solstone.think.sol_cli",
            "doctor",
            "--readiness",
            "--json",
        ],
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode in {0, 1}, (
        f"doctor crashed: rc={result.returncode}\n"
        f"stdout={result.stdout}\nstderr={result.stderr}"
    )
    payload = json.loads(result.stdout)
    from solstone.think import doctor as doctor_module

    names = {check["name"] for check in payload["checks"]}
    assert names == {
        check.name for check, _runner in doctor_module.JOURNAL_READINESS_CHECKS
    }
    assert not any(
        name.startswith("service_") or name == "journal_sync" for name in names
    )
