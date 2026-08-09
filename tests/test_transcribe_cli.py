# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unit tests for journal transcribe CLI (M3, M8, M9)."""

import argparse
import importlib
import json
import logging
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from solstone.observe.transcribe.native import SpeakerTranscriptWriteResponse
from solstone.observe.transcribe.speakers_analyze_errors import SpeakerAnalyzeError
from solstone.observe.vad import VadResult
from solstone.think.speakers_analyze_installation import (
    SpeakersAnalyzeInstallationResult,
)
from tests.helpers.journal_config import seed_journal_config


@pytest.fixture(autouse=True)
def _speaker_installation_ready(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "solstone.think.speakers_analyze_installation."
        "check_speakers_analyze_installation",
        lambda: SpeakersAnalyzeInstallationResult("ok"),
    )


def _args(backend: str | None = None) -> argparse.Namespace:
    return argparse.Namespace(backend=backend, cpu=False, model=None, redo=False)


def _confidential_block() -> dict:
    return {
        "enabled_at": "2026-05-24T00:00:00Z",
        "account_id": "acct-test",
        "endpoint_url": "https://spp.example.test",
        "served_model_id": "confidential-model",
        "credential_fingerprint_sha256": "fingerprint",
    }


def _stranded_confidential_config(*, transcribe: dict | None = None) -> dict:
    config = {
        "services": {"confidential": _confidential_block()},
        "providers": {"local": {}},
    }
    if transcribe is not None:
        config["transcribe"] = transcribe
    return config


def _healthy_confidential_config(*, transcribe: dict | None = None) -> dict:
    config = {
        "services": {"confidential": _confidential_block()},
        "providers": {
            "local": {
                "endpoint_url": "https://spp.example.test/v1",
                "served_model_id": "confidential-model",
                "credential": "confidential-credential",
            }
        },
    }
    if transcribe is not None:
        config["transcribe"] = transcribe
    return config


def _seed_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, config: dict) -> dict:
    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    seed_journal_config(config, journal)
    return config


def test_main_accepts_journal_relative_path(tmp_path, monkeypatch):
    """main() resolves audio_path relative to journal when absolute path fails."""
    seg_dir = tmp_path / "chronicle" / "20260201" / "default" / "090000_300"
    seg_dir.mkdir(parents=True)
    audio_file = seg_dir / "audio.wav"
    audio_file.touch()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "sys.argv", ["sol transcribe", "20260201/default/090000_300/audio.wav"]
    )

    mock_load = MagicMock(return_value=MagicMock())
    mock_vad_result = VadResult(
        duration=5.0,
        speech_duration=0.0,
        has_speech=False,
        speech_segments=[],
    )
    mock_vad = MagicMock(return_value=mock_vad_result)

    with (
        patch("solstone.observe.transcribe.main.load_audio", mock_load),
        patch("solstone.observe.vad.run_vad", mock_vad),
        patch("solstone.observe.transcribe.main.tag_audio", return_value=None),
        patch("solstone.observe.transcribe.main.callosum_send"),
        patch(
            "solstone.observe.transcribe.main.write_speaker_transcript",
            return_value=SpeakerTranscriptWriteResponse(
                jsonl_path=str(audio_file.with_suffix(".jsonl")),
                npz_path=str(audio_file.with_suffix(".npz")),
                statement_count=0,
                embedding_row_count=0,
            ),
        ),
        patch(
            "solstone.observe.transcribe.main.get_segment_key",
            return_value="090000_300",
        ),
        patch("solstone.observe.transcribe.main._build_base_event", return_value={}),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
    ):
        from solstone.observe.transcribe.main import main

        main()

    mock_load.assert_called_once()


def test_main_errors_on_nonexistent_absolute_path(tmp_path, monkeypatch, capsys):
    """main() errors clearly when path doesn't exist as absolute or journal-relative."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr("sys.argv", ["sol transcribe", "/nonexistent/path/audio.wav"])

    from solstone.observe.transcribe.main import main

    with (
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
    ):
        with pytest.raises(SystemExit):
            main()

    captured = capsys.readouterr()
    assert "Tried absolute" in captured.err or "not found" in captured.err.lower()


def test_setup_cli_no_message_on_project_journal(tmp_path, monkeypatch, capsys):
    """setup_cli() prints no informational message — journal path is always deterministic."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    with (
        patch("solstone.think.utils.get_journal", return_value=str(tmp_path)),
        patch("solstone.think.utils.get_config", return_value={}),
    ):
        from solstone.think.utils import setup_cli

        parser = argparse.ArgumentParser()
        monkeypatch.setattr("sys.argv", ["test"])
        setup_cli(parser)

    captured = capsys.readouterr()
    assert "docs/INSTALL.md" not in captured.err


def _make_batch_journal(tmp_path: Path) -> Path:
    """Create a minimal temp journal with three segments for batch testing."""
    seg1 = tmp_path / "chronicle" / "20260101" / "default" / "090000_300"
    seg1.mkdir(parents=True)
    (seg1 / "audio.flac").touch()

    seg2 = tmp_path / "chronicle" / "20260101" / "default" / "140000_300"
    seg2.mkdir(parents=True)
    (seg2 / "audio.flac").touch()
    (seg2 / "audio.jsonl").touch()

    seg3 = tmp_path / "chronicle" / "20260101" / "default" / "180000_300"
    seg3.mkdir(parents=True)
    (seg3 / "screen.png").touch()

    return tmp_path


def test_all_batch_processes_unprocessed_skips_transcribed(
    tmp_path, monkeypatch, capsys
):
    """--all processes unprocessed audio, skips already-transcribed, ignores non-audio."""
    journal = _make_batch_journal(tmp_path)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr("sys.argv", ["sol transcribe", "--all"])

    mock_process_one = MagicMock()

    with (
        patch("solstone.observe.transcribe.main._process_one", mock_process_one),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
    ):
        from solstone.observe.transcribe.main import main

        main()

    assert mock_process_one.call_count == 1
    called_path = mock_process_one.call_args[0][0]
    assert called_path.name == "audio.flac"
    assert "090000_300" in str(called_path)

    captured = capsys.readouterr()
    assert "1 processed" in captured.out
    assert "1 skipped" in captured.out


def test_all_batch_counts_typed_speaker_failure_and_continues(
    tmp_path, monkeypatch, capsys
):
    journal = _make_batch_journal(tmp_path)
    extra = journal / "chronicle" / "20260101" / "default" / "180000_300" / "audio.flac"
    extra.touch()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr("sys.argv", ["sol transcribe", "--all"])

    calls: list[Path] = []

    def process_one(audio_path, *_args):
        calls.append(audio_path)
        if len(calls) == 1:
            raise SpeakerAnalyzeError(
                path=audio_path,
                stage="invoke",
                reason="unavailable",
                native_exit_code=75,
            )

    with (
        patch("solstone.observe.transcribe.main._process_one", side_effect=process_one),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
    ):
        from solstone.observe.transcribe.main import main

        main()

    assert len(calls) == 2
    captured = capsys.readouterr()
    assert "1 processed" in captured.out
    assert "1 skipped" in captured.out
    assert "1 failed" in captured.out


def test_all_batch_generic_exception_aborts(tmp_path, monkeypatch):
    journal = _make_batch_journal(tmp_path)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr("sys.argv", ["sol transcribe", "--all"])

    with (
        patch(
            "solstone.observe.transcribe.main._process_one",
            side_effect=RuntimeError("boom"),
        ),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
    ):
        from solstone.observe.transcribe.main import main

        with pytest.raises(RuntimeError, match="boom"):
            main()


def test_all_batch_provider_blocked_is_deferred_not_failed(
    tmp_path, monkeypatch, capsys
):
    from solstone.observe.exit_codes import EXIT_PROVIDER_BLOCKED

    journal = _make_batch_journal(tmp_path)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr("sys.argv", ["sol transcribe", "--all"])

    with (
        patch(
            "solstone.observe.transcribe.main._process_one",
            side_effect=SystemExit(EXIT_PROVIDER_BLOCKED),
        ),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
    ):
        from solstone.observe.transcribe.main import main

        main()

    captured = capsys.readouterr()
    assert "1 deferred" in captured.out
    assert "failed" not in captured.out


def test_single_file_typed_speaker_failure_exits_one(tmp_path, monkeypatch):
    journal = _make_batch_journal(tmp_path)
    audio_file = (
        journal / "chronicle" / "20260101" / "default" / "090000_300" / "audio.flac"
    )
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr("sys.argv", ["sol transcribe", str(audio_file)])

    with (
        patch(
            "solstone.observe.transcribe.main._process_one",
            side_effect=SpeakerAnalyzeError(
                path=audio_file,
                stage="invoke",
                reason="unavailable",
                native_exit_code=75,
            ),
        ),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
    ):
        from solstone.observe.transcribe.main import main

        with pytest.raises(SystemExit) as exc:
            main()

    assert exc.value.code == 1


def test_all_redo_reprocesses_transcribed(tmp_path, monkeypatch):
    """--all --redo reprocesses even segments that already have .jsonl."""
    journal = _make_batch_journal(tmp_path)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr("sys.argv", ["sol transcribe", "--all", "--redo"])

    mock_process_one = MagicMock()

    with (
        patch("solstone.observe.transcribe.main._process_one", mock_process_one),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
    ):
        from solstone.observe.transcribe.main import main

        main()

    assert mock_process_one.call_count == 2


def test_all_and_audio_path_mutually_exclusive(tmp_path, monkeypatch):
    """Providing both --all and audio_path produces a clear error."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr("sys.argv", ["sol transcribe", "--all", "some/audio.wav"])

    from solstone.observe.transcribe.main import main

    with (
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=8 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
    ):
        with pytest.raises(SystemExit):
            main()


def test_neither_all_nor_audio_path_errors(tmp_path, monkeypatch):
    """Providing neither --all nor audio_path produces a clear error."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr("sys.argv", ["sol transcribe"])

    from solstone.observe.transcribe.main import main

    with pytest.raises(SystemExit):
        main()


def test_main_google_key_decoy_below_floor_surfaces_local_requirement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture
) -> None:
    journal = tmp_path / "journal"
    audio_file = (
        journal / "chronicle" / "20260201" / "default" / "090000_300" / "audio.wav"
    )
    audio_file.parent.mkdir(parents=True)
    audio_file.write_bytes(b"audio")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("GOOGLE_API_KEY", "test-key")
    monkeypatch.setattr("sys.argv", ["sol transcribe", str(audio_file)])

    from solstone.observe.transcribe.main import main

    with (
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            return_value=2 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
        patch("solstone.observe.transcribe.main._process_one") as mock_process_one,
    ):
        with caplog.at_level(logging.ERROR):
            with pytest.raises(SystemExit) as exc_info:
                main()

    assert exc_info.value.code == 1
    assert audio_file.exists()
    mock_process_one.assert_not_called()
    assert "local transcription needs about 4 GB" in caplog.text


def test_resolve_default_backend_stranded_low_ram_surfaces_requirement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    config = _seed_config(tmp_path, monkeypatch, _stranded_confidential_config())

    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 2 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    with pytest.raises(SystemExit) as exc_info:
        transcribe_main.resolve_default_backend(
            _args(),
            config.get("transcribe", {}),
        )

    assert exc_info.value.code == 1


def test_resolve_default_backend_stranded_adequate_ram_uses_local_backend(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    transcribe_dispatch = importlib.import_module("solstone.observe.transcribe")
    config = _seed_config(tmp_path, monkeypatch, _stranded_confidential_config())

    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 8 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")
    expected_backend = transcribe_main.local_stt_backend()
    backend_module = MagicMock()
    backend_module.transcribe.return_value = [{"text": "local"}]
    get_backend = MagicMock(return_value=backend_module)
    monkeypatch.setattr(transcribe_dispatch, "get_backend", get_backend)

    assert expected_backend is not None
    resolved_backend = transcribe_main.resolve_default_backend(
        _args(), config.get("transcribe", {})
    )
    assert resolved_backend == expected_backend

    assert transcribe_dispatch.transcribe(resolved_backend, [], 16000, {}) == [
        {"text": "local"}
    ]
    get_backend.assert_called_once_with(expected_backend)
    backend_module.transcribe.assert_called_once_with([], 16000, {})


def test_main_stranded_low_ram_preserves_audio_before_processing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from solstone.think.retention import resolve_segment_gate

    journal = tmp_path / "journal"
    config = _stranded_confidential_config()
    _seed_config(tmp_path, monkeypatch, config)
    audio_file = (
        journal / "chronicle" / "20260722" / "_default" / "120000_0010" / "audio.wav"
    )
    audio_file.parent.mkdir(parents=True)
    audio_file.write_bytes(b"not decoded on the surface path")

    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    monkeypatch.setattr("sys.argv", ["sol transcribe", str(audio_file)])
    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 2 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")
    process_one = MagicMock(return_value=None)
    monkeypatch.setattr(transcribe_main, "_process_one", process_one)

    with pytest.raises(SystemExit) as exc_info:
        transcribe_main.main()

    assert exc_info.value.code == 1
    process_one.assert_not_called()
    assert audio_file.exists()
    assert not audio_file.with_suffix(".jsonl").exists()
    assert resolve_segment_gate(audio_file.parent).verdict == "incomplete"


def test_resolve_default_backend_healthy_channel_without_brain_uses_confidential(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    config = _seed_config(tmp_path, monkeypatch, _healthy_confidential_config())

    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 1 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    assert (
        transcribe_main.resolve_default_backend(
            _args(),
            config.get("transcribe", {}),
            journal_config=config,
        )
        == "confidential"
    )


def test_resolve_default_backend_auto_selects_confidential_under_lane(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    config = _seed_config(tmp_path, monkeypatch, _healthy_confidential_config())

    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 1 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    assert (
        transcribe_main.resolve_default_backend(
            _args(),
            config.get("transcribe", {}),
            journal_config=config,
        )
        == "confidential"
    )


def test_resolve_default_backend_explicit_local_wins_under_lane(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    config = _seed_config(tmp_path, monkeypatch, _healthy_confidential_config())

    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 1 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    assert (
        transcribe_main.resolve_default_backend(
            _args(),
            {"backend": "parakeet"},
            journal_config=config,
        )
        == "parakeet"
    )


def test_resolve_default_backend_confidential_fallback_never_cloud(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, caplog
):
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    config = _seed_config(tmp_path, monkeypatch, _healthy_confidential_config())

    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 1 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    with caplog.at_level(logging.WARNING):
        backend = transcribe_main.resolve_default_backend(
            _args(),
            {"backend": "confidential", "confidential_audio": False},
            journal_config=config,
        )

    assert backend == "parakeet"
    assert "confidential audio is disabled" in caplog.text


def test_resolve_default_backend_healthy_channel_audio_disabled_uses_local_backend(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    config = _seed_config(
        tmp_path,
        monkeypatch,
        _healthy_confidential_config(transcribe={"confidential_audio": False}),
    )

    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 1 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")
    expected_backend = transcribe_main.local_stt_backend()

    assert expected_backend is not None
    assert (
        transcribe_main.resolve_default_backend(
            _args(),
            config.get("transcribe", {}),
            journal_config=config,
        )
        == expected_backend
    )


def test_resolve_default_backend_warns_when_confidential_channel_incomplete(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, caplog
) -> None:
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    config = _seed_config(tmp_path, monkeypatch, _stranded_confidential_config())

    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 8 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    with caplog.at_level(logging.WARNING):
        backend = transcribe_main.resolve_default_backend(
            _args(),
            {"backend": "confidential"},
            journal_config=config,
        )

    assert backend == "parakeet"
    assert (
        "confidential channel is incomplete: missing credential, endpoint URL, "
        "and served model ID"
    ) in caplog.text


def test_resolve_default_backend_surfaces_when_no_viable_backend(monkeypatch):
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")

    calls = 0

    def fake_read_available_bytes():
        nonlocal calls
        calls += 1
        return 2 * 1024**3

    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.setattr(
        transcribe_main, "read_available_bytes", fake_read_available_bytes
    )
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    with pytest.raises(SystemExit) as exc_info:
        transcribe_main.resolve_default_backend(_args(), {})

    assert exc_info.value.code == 1
    assert calls == 1


def test_resolve_default_backend_warns_but_honors_explicit_local(monkeypatch, caplog):
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")

    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 2 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    with caplog.at_level(logging.WARNING):
        backend = transcribe_main.resolve_default_backend(_args(backend="parakeet"), {})

    assert backend == "parakeet"
    assert "free memory is below 4 GB" in caplog.text


def test_resolve_default_backend_stale_config_routes_to_confidential_under_lane(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, caplog
):
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")
    config = _seed_config(tmp_path, monkeypatch, _healthy_confidential_config())

    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 2 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    with caplog.at_level(logging.WARNING):
        backend = transcribe_main.resolve_default_backend(
            _args(),
            {"backend": "removed-stt"},
            journal_config=config,
        )

    assert backend == "confidential"
    assert caplog.messages == [
        "Configured STT backend 'removed-stt' is unavailable; treating it as unset"
    ]


def test_resolve_default_backend_uses_parakeet_when_memory_fits(monkeypatch):
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")

    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 5 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    assert transcribe_main.resolve_default_backend(_args(), {}) == "parakeet"


def test_resolve_default_backend_falls_back_when_configured_backend_is_removed(
    monkeypatch, caplog
):
    transcribe_main = importlib.import_module("solstone.observe.transcribe.main")

    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.setattr(transcribe_main, "read_available_bytes", lambda: 5 * 1024**3)
    monkeypatch.setattr(transcribe_main, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    monkeypatch.setattr(transcribe_main, "local_stt_backend", lambda: "parakeet")

    with caplog.at_level(logging.WARNING):
        backend = transcribe_main.resolve_default_backend(
            _args(), {"backend": "removed-local"}
        )

    assert backend == "parakeet"
    assert caplog.messages == [
        "Configured STT backend 'removed-local' is unavailable; treating it as unset"
    ]


def test_all_batch_reads_memory_once_and_reuses_default_backend(tmp_path, monkeypatch):
    journal = _make_batch_journal(tmp_path)
    config_dir = journal / "config"
    config_dir.mkdir()
    (config_dir / "journal.json").write_text(json.dumps({"identity": {"name": "Test"}}))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr("sys.argv", ["sol transcribe", "--all", "--redo"])
    calls = 0

    def fake_read_available_bytes():
        nonlocal calls
        calls += 1
        return 5 * 1024**3

    mock_process_one = MagicMock()

    with (
        patch("solstone.observe.transcribe.main._process_one", mock_process_one),
        patch(
            "solstone.observe.transcribe.main.read_available_bytes",
            fake_read_available_bytes,
        ),
        patch(
            "solstone.observe.transcribe.main.stt_local_floor_bytes",
            return_value=4 * 1024**3,
        ),
        patch(
            "solstone.observe.transcribe.main.local_stt_backend",
            return_value="parakeet",
        ),
    ):
        from solstone.observe.transcribe.main import main

        main()

    assert calls == 1
    assert mock_process_one.call_count == 2
    assert {call.args[3] for call in mock_process_one.call_args_list} == {"parakeet"}
