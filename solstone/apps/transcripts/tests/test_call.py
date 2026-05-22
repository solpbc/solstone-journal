# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for transcripts CLI commands (sol call transcripts ...)."""

from typer.testing import CliRunner

from solstone.think.call import call_app

runner = CliRunner()


def _write_segment(
    journal_root,
    day: str,
    segment: str,
    *,
    audio_jsonl: bool = False,
    audio_flac: bool = False,
    screen_jsonl: bool = False,
) -> None:
    segment_dir = journal_root / "chronicle" / day / "default" / segment
    segment_dir.mkdir(parents=True, exist_ok=True)
    if audio_jsonl:
        (segment_dir / "audio.jsonl").write_text(
            '{"raw": "audio.flac"}\n{"start": "00:00:01", "text": "audio"}\n',
            encoding="utf-8",
        )
    if audio_flac:
        (segment_dir / "audio.flac").write_bytes(b"audio")
    if screen_jsonl:
        (segment_dir / "screen.jsonl").write_text(
            '{"raw": "screen.webm"}\n'
            '{"timestamp": 1, "analysis": {"primary": "work"}}\n',
            encoding="utf-8",
        )


class TestScan:
    def test_scan_day(self):
        result = runner.invoke(call_app, ["transcripts", "scan", "20240101"])
        assert result.exit_code == 0
        assert "Transcripts:" in result.output
        assert "Percepts:" in result.output

    def test_scan_empty_day(self):
        result = runner.invoke(call_app, ["transcripts", "scan", "20990101"])
        assert result.exit_code == 0
        assert "(none)" in result.output

    def test_scan_output_byte_identical_when_no_pending_segments(
        self, tmp_path, monkeypatch
    ):
        day = "20990102"
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        _write_segment(
            tmp_path,
            day,
            "090000_300",
            audio_jsonl=True,
            screen_jsonl=True,
        )

        result = runner.invoke(call_app, ["transcripts", "scan", day])

        assert result.exit_code == 0
        assert result.output == (
            "Transcripts:\n  09:00 - 09:15\nPercepts:\n  09:00 - 09:15\n"
        )

    def test_scan_output_annotates_pending_inside_range(self, tmp_path, monkeypatch):
        day = "20990103"
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        _write_segment(tmp_path, day, "090000_300", audio_jsonl=True)
        _write_segment(tmp_path, day, "090500_300", audio_flac=True)

        result = runner.invoke(call_app, ["transcripts", "scan", day])

        assert result.exit_code == 0
        assert result.output == (
            "Transcripts:\n"
            "  09:00 - 09:15 (1 segment pending at 09:05)\n"
            "Percepts:\n"
            "  (none)\n"
        )

    def test_scan_output_reports_pending_only_range(self, tmp_path, monkeypatch):
        day = "20990104"
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        _write_segment(tmp_path, day, "091500_300", audio_flac=True)

        result = runner.invoke(call_app, ["transcripts", "scan", day])

        assert result.exit_code == 0
        assert result.output == (
            "Transcripts:\n"
            "  09:15 - 09:30 (1 segment pending at 09:15)\n"
            "Percepts:\n"
            "  (none)\n"
        )

    def test_scan_output_pluralizes_multiple_pending(self, tmp_path, monkeypatch):
        day = "20990105"
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        _write_segment(tmp_path, day, "090000_300", audio_jsonl=True)
        _write_segment(tmp_path, day, "090500_300", audio_flac=True)
        _write_segment(tmp_path, day, "091000_300", audio_flac=True)

        result = runner.invoke(call_app, ["transcripts", "scan", day])

        assert result.exit_code == 0
        assert result.output == (
            "Transcripts:\n"
            "  09:00 - 09:15 (2 segments pending at 09:05, 09:10)\n"
            "Percepts:\n"
            "  (none)\n"
        )


class TestSegments:
    def test_segments_day(self):
        result = runner.invoke(call_app, ["transcripts", "segments", "20240101"])
        assert result.exit_code == 0
        assert "123456_300" in result.output

    def test_segments_empty(self):
        result = runner.invoke(call_app, ["transcripts", "segments", "20990101"])
        assert result.exit_code == 0
        assert "No segments" in result.output


class TestRead:
    def test_read_default(self):
        result = runner.invoke(call_app, ["transcripts", "read", "20240101"])
        assert result.exit_code == 0
        assert "## " in result.output

    def test_read_full(self):
        result = runner.invoke(call_app, ["transcripts", "read", "20240101", "--full"])
        assert result.exit_code == 0

    def test_read_raw(self):
        result = runner.invoke(call_app, ["transcripts", "read", "20240101", "--raw"])
        assert result.exit_code == 0

    def test_read_segment(self):
        result = runner.invoke(
            call_app, ["transcripts", "read", "20240101", "--segment", "123456_300"]
        )
        assert result.exit_code == 0

    def test_read_range(self):
        result = runner.invoke(
            call_app,
            ["transcripts", "read", "20240101", "--start", "123456", "--length", "5"],
        )
        assert result.exit_code == 0

    def test_read_full_and_raw_error(self):
        result = runner.invoke(
            call_app, ["transcripts", "read", "20240101", "--full", "--raw"]
        )
        assert result.exit_code == 1
        assert "Cannot use --full and --raw" in result.output

    def test_read_start_without_length(self):
        result = runner.invoke(
            call_app, ["transcripts", "read", "20240101", "--start", "123456"]
        )
        assert result.exit_code == 1
        assert "--start and --length must be used together" in result.output

    def test_read_segment_with_start(self):
        result = runner.invoke(
            call_app,
            [
                "transcripts",
                "read",
                "20240101",
                "--segment",
                "123456_300",
                "--start",
                "123456",
            ],
        )
        assert result.exit_code == 1


class TestStats:
    def test_stats_month(self):
        result = runner.invoke(call_app, ["transcripts", "stats", "202401"])
        assert result.exit_code == 0
        assert "20240101" in result.output
        assert "Total: 1 days with data" in result.output

    def test_stats_empty(self):
        result = runner.invoke(call_app, ["transcripts", "stats", "209901"])
        assert result.exit_code == 0
        assert "No data" in result.output


class TestSolEnvResolution:
    """Tests for SOL_* env var resolution in transcripts commands."""

    def test_scan_from_sol_day(self, monkeypatch):
        """scan with SOL_DAY env and no arg works."""
        monkeypatch.setenv("SOL_DAY", "20240101")
        result = runner.invoke(call_app, ["transcripts", "scan"])
        assert result.exit_code == 0
        assert "Transcripts:" in result.output

    def test_read_from_sol_day(self, monkeypatch):
        """read with SOL_DAY env and no arg works."""
        monkeypatch.setenv("SOL_DAY", "20240101")
        result = runner.invoke(call_app, ["transcripts", "read"])
        assert result.exit_code == 0

    def test_read_from_sol_day_and_segment(self, monkeypatch):
        """read with SOL_DAY + SOL_SEGMENT env works."""
        monkeypatch.setenv("SOL_DAY", "20240101")
        monkeypatch.setenv("SOL_SEGMENT", "123456_300")
        result = runner.invoke(call_app, ["transcripts", "read"])
        assert result.exit_code == 0
