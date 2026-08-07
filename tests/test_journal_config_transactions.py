# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import multiprocessing
import os
import time
import traceback
from pathlib import Path
from queue import Empty
from typing import Any

from tests.helpers.journal_config import seed_journal_config


def _install_built_core() -> None:
    from solstone.think import core_handshake

    helper = Path(__file__).resolve().parents[1] / "core" / "target" / "debug" / "solstone-core"
    core_handshake.check_solstone_core_handshake = lambda: core_handshake.CoreHandshakeResult(
        "ok"
    )
    core_handshake.helper_path_for_executable = lambda: helper


def _read_config(journal: Path) -> dict[str, Any]:
    return json.loads((journal / "config" / "journal.json").read_text("utf-8"))


def _drain_errors(errors: Any) -> list[str]:
    found = []
    while True:
        try:
            found.append(errors.get_nowait())
        except Empty:
            return found


def _join_processes(processes: list[Any], errors: Any) -> None:
    for process in processes:
        process.start()
    for process in processes:
        process.join(timeout=15)
    for process in processes:
        if process.is_alive():
            process.terminate()
            process.join(timeout=2)

    error_text = "\n".join(_drain_errors(errors))
    assert all(not process.is_alive() for process in processes), error_text
    assert all(process.exitcode == 0 for process in processes), error_text


def _disjoint_mutation_worker(
    journal_path: str,
    barrier: Any,
    errors: Any,
    key: str,
    value: str,
) -> None:
    os.environ["SOLSTONE_JOURNAL"] = journal_path
    try:
        _install_built_core()
        barrier.wait(timeout=5)
        from solstone.think.journal_config import (
            JournalConfigMutation,
            mutate_journal_config,
        )

        def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
            time.sleep(0.05)
            config.setdefault("concurrent", {})[key] = value
            return JournalConfigMutation(changed=True, value=None)

        mutate_journal_config(apply)
    except BaseException:
        errors.put(traceback.format_exc())
        raise


def _provider_progress_worker(journal_path: str, barrier: Any, errors: Any) -> None:
    os.environ["SOLSTONE_JOURNAL"] = journal_path
    try:
        _install_built_core()
        barrier.wait(timeout=5)
        from solstone.think.providers.install_state import (
            make_idle_status,
            transition_state,
            write_install_status,
        )

        status = transition_state(make_idle_status("parakeet"), new_state="downloading")
        write_install_status(status)
    except BaseException:
        errors.put(traceback.format_exc())
        raise


def _onboarding_finalize_worker(journal_path: str, barrier: Any, errors: Any) -> None:
    os.environ["SOLSTONE_JOURNAL"] = journal_path
    os.environ["SOL_SKIP_SUPERVISOR_CHECK"] = "1"
    try:
        _install_built_core()
        barrier.wait(timeout=5)
        import solstone.convey.root as root

        real_mutate = root.mutate_journal_config

        def slow_mutate(mutator, *args, **kwargs):
            def slow(config: dict[str, Any]):
                mutation = mutator(config)
                time.sleep(0.2)
                return mutation

            return real_mutate(slow, *args, **kwargs)

        root.mutate_journal_config = slow_mutate
        root.locked_modify_convey_config = lambda _fn: None
        root.start_secure_listener = lambda _app: None

        from solstone.convey import create_app
        from solstone.think.link.ca import load_or_generate_ca
        from solstone.think.link.paths import ca_dir

        load_or_generate_ca(ca_dir())
        app = create_app(journal_path)
        app.config["TESTING"] = True
        response = app.test_client().post(
            "/init/finalize",
            json={
                "name": "Owner",
                "preferred": "O",
                "timezone": "UTC",
                "retention_mode": "days",
                "retention_days": 14,
            },
        )
        if response.status_code != 200:
            raise AssertionError(
                f"finalize failed {response.status_code}: "
                f"{response.get_data(as_text=True)}"
            )
    except BaseException:
        errors.put(traceback.format_exc())
        raise


def _canonical_marker_worker(
    journal_path: str,
    barrier: Any,
    errors: Any,
    key: str,
) -> None:
    os.environ["SOLSTONE_JOURNAL"] = journal_path
    try:
        _install_built_core()
        barrier.wait(timeout=5)
        from solstone.think.journal_config import (
            JournalConfigMutation,
            mutate_journal_config,
        )

        def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
            time.sleep(0.05)
            config.setdefault("canonical_markers", {})[key] = True
            return JournalConfigMutation(changed=True, value=None)

        mutate_journal_config(apply)
    except BaseException:
        errors.put(traceback.format_exc())
        raise


def _spl_disable_worker(journal_path: str, barrier: Any, errors: Any) -> None:
    os.environ["SOLSTONE_JOURNAL"] = journal_path
    try:
        _install_built_core()
        barrier.wait(timeout=5)
        from solstone.think.services.spl import disable_spl

        disable_spl()
    except BaseException:
        errors.put(traceback.format_exc())
        raise


def test_two_process_disjoint_journal_config_mutations_both_survive(
    tmp_path: Path,
) -> None:
    ctx = multiprocessing.get_context("spawn")
    journal = tmp_path / "journal"
    seed_journal_config({"identity": {"name": "Base"}}, journal)
    barrier = ctx.Barrier(2)
    errors = ctx.Queue()
    processes = [
        ctx.Process(
            target=_disjoint_mutation_worker,
            args=(str(journal), barrier, errors, "a", "one"),
        ),
        ctx.Process(
            target=_disjoint_mutation_worker,
            args=(str(journal), barrier, errors, "b", "two"),
        ),
    ]

    _join_processes(processes, errors)

    data = _read_config(journal)
    assert data["concurrent"] == {"a": "one", "b": "two"}
    assert data["identity"]["name"] == "Base"


def test_onboarding_finalize_interleaves_with_provider_progress(
    tmp_path: Path,
) -> None:
    ctx = multiprocessing.get_context("spawn")
    journal = tmp_path / "journal"
    seed_journal_config(
        {
            "providers": {
                "active": {"provider": "google", "model": "gemini-3.5-flash"},
                "bundled": {},
            }
        },
        journal,
    )
    barrier = ctx.Barrier(2)
    errors = ctx.Queue()
    processes = [
        ctx.Process(
            target=_onboarding_finalize_worker, args=(str(journal), barrier, errors)
        ),
        ctx.Process(
            target=_provider_progress_worker, args=(str(journal), barrier, errors)
        ),
    ]

    _join_processes(processes, errors)

    data = _read_config(journal)
    assert data["setup"]["completed_at"]
    assert data["identity"] == {"name": "Owner", "preferred": "O", "timezone": "UTC"}
    assert data["retention"] == {"raw_media": "days", "raw_media_days": 14}
    assert data["providers"]["active"] == {
        "provider": "google",
        "model": "gemini-3.5-flash",
    }
    from solstone.think.providers.install_state import read_install_status

    status = read_install_status(name="parakeet", journal_path=journal)
    assert status["install_state"] == "downloading"


def test_spl_coordinates_with_canonical_writer(
    tmp_path: Path,
) -> None:
    ctx = multiprocessing.get_context("spawn")
    journal = tmp_path / "journal-spl"
    seed_journal_config(
        {
            "link": {"posture": "spl"},
        },
        journal,
    )
    barrier = ctx.Barrier(2)
    errors = ctx.Queue()
    processes = [
        ctx.Process(target=_spl_disable_worker, args=(str(journal), barrier, errors)),
        ctx.Process(
            target=_canonical_marker_worker,
            args=(str(journal), barrier, errors, "spl"),
        ),
    ]

    _join_processes(processes, errors)

    data = _read_config(journal)
    assert data["canonical_markers"]["spl"] is True
    assert data["link"]["posture"] == "direct"
