# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path
from threading import Thread

import pytest
import typer

from solstone.convey import create_app
from solstone.convey.reasons import CONFIG_BUSY
from solstone.think import journal_config
from solstone.think.journal_config import (
    JournalConfigMutation,
    ensure_journal_config,
    get_journal_config_path,
    mutate_journal_config,
    read_journal_config,
)
from solstone.think.journal_io import LockTimeout
from solstone.think.journal_io.locking import hold_lock
from solstone.think.utils import CorruptConfigError
from tests.helpers.journal_config import seed_journal_config


@pytest.fixture(autouse=True)
def use_built_core(monkeypatch):
    helper = Path(__file__).resolve().parents[1] / "core" / "target" / "debug" / "solstone-core"
    monkeypatch.setattr(
        journal_config.core_handshake,
        "check_solstone_core_handshake",
        lambda: journal_config.core_handshake.CoreHandshakeResult("ok"),
    )
    monkeypatch.setattr(
        journal_config.core_handshake,
        "helper_path_for_executable",
        lambda: helper,
    )


def _config_path(journal: Path) -> Path:
    return journal / "config" / "journal.json"


def test_falsey_mutator_value_still_persists_changed_config(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    seed_journal_config({"identity": {"name": "Before"}}, tmp_path)

    def apply(draft: dict) -> JournalConfigMutation[str]:
        draft["identity"]["name"] = ""
        return JournalConfigMutation(changed=True, value="")

    result = mutate_journal_config(apply)

    assert result.value == ""
    assert result.changed is True
    assert result.written is True
    assert read_journal_config(tmp_path)["identity"]["name"] == ""


def test_truthy_mutator_value_does_not_force_unchanged_write(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config_path = seed_journal_config({"identity": {"name": "Stable"}}, tmp_path)
    before = config_path.read_bytes()
    before_mtime = config_path.stat().st_mtime_ns

    result = mutate_journal_config(
        lambda draft: JournalConfigMutation(changed=False, value={"ok": True})
    )

    assert result.value == {"ok": True}
    assert result.changed is False
    assert result.written is False
    assert config_path.read_bytes() == before
    assert config_path.stat().st_mtime_ns == before_mtime


def test_mutator_raise_leaves_prior_doc_valid_and_lock_released(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config_path = seed_journal_config({"identity": {"name": "Before"}}, tmp_path)
    before = config_path.read_bytes()

    def fail(draft: dict) -> JournalConfigMutation[None]:
        draft["identity"]["name"] = "Partial"
        raise RuntimeError("mutator failed")

    with pytest.raises(RuntimeError, match="mutator failed"):
        mutate_journal_config(fail)

    assert config_path.read_bytes() == before
    assert list(config_path.parent.glob(".tmp_*")) == []

    mutate_journal_config(
        lambda draft: (
            draft["identity"].update({"name": "After"})
            or JournalConfigMutation(changed=True, value=None)
        )
    )
    assert read_journal_config(tmp_path)["identity"]["name"] == "After"


def test_missing_file_materialization_does_not_clobber_racing_initializer(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config_path = get_journal_config_path(tmp_path)
    observed: dict[str, object] = {}

    def worker() -> None:
        try:
            observed["config"] = ensure_journal_config(tmp_path)
        except BaseException as exc:  # pragma: no cover - surfaced below
            observed["error"] = exc

    with hold_lock(config_path, timeout=1, mode=0o600):
        thread = Thread(target=worker)
        thread.start()
        seed_journal_config({"identity": {"name": "Racing Init"}}, tmp_path)

    thread.join(timeout=5)
    assert not thread.is_alive()
    assert "error" not in observed
    assert observed["config"] == {"identity": {"name": "Racing Init"}}
    assert read_journal_config(tmp_path) == {"identity": {"name": "Racing Init"}}


def test_malformed_existing_doc_raises_and_is_not_replaced(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    config_path = _config_path(tmp_path)
    config_path.parent.mkdir(parents=True)
    config_path.write_bytes(b"{ invalid json }")
    before = config_path.read_bytes()

    with pytest.raises(CorruptConfigError):
        mutate_journal_config(
            lambda draft: JournalConfigMutation(changed=True, value=None)
        )

    assert config_path.read_bytes() == before




def test_tools_call_lock_timeout_exits_nonzero_with_retry_message(
    tmp_path, monkeypatch, capsys
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    seed_journal_config({"retention": {"raw_media": "keep"}}, tmp_path)
    import solstone.think.tools.call as call_module

    timeout = LockTimeout(path=Path("busy.lock"), timeout=0.01)
    monkeypatch.setattr(
        call_module,
        "mutate_journal_config",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(timeout),
    )

    with pytest.raises(typer.Exit) as exc_info:
        call_module.config(mode="days", days=3, stream=None, clear=False)

    assert exc_info.value.exit_code == 1
    assert "Journal config is busy; try again." in capsys.readouterr().err
    assert read_journal_config(tmp_path)["retention"]["raw_media"] == "keep"
