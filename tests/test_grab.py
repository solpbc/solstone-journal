# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the journal grab CLI."""

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import Mock

import pytest
from PIL import Image

FIXTURE_ROOT = Path(__file__).parent / "fixtures" / "sol_grab"
FIXTURE_JOURNAL = FIXTURE_ROOT / "journal"


def _expected(name: str) -> dict:
    return json.loads((FIXTURE_ROOT / name).read_text(encoding="utf-8"))


def _invoke_grab(monkeypatch, capsys, *argv: str):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(FIXTURE_JOURNAL))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    monkeypatch.setattr("sys.argv", ["sol grab", *argv])

    from solstone.observe.grab import main

    exit_code = 0
    exit_message = ""
    try:
        main()
    except SystemExit as exc:
        if isinstance(exc.code, int):
            exit_code = exc.code
        elif exc.code is None:
            exit_code = 0
        else:
            exit_code = 1
            exit_message = str(exc.code)
    captured = capsys.readouterr()
    return exit_code, exit_message, captured.out, captured.err


def _normalize_saved_paths(actual: dict, expected: dict) -> dict:
    normalized = json.loads(json.dumps(actual))
    for actual_item, expected_item in zip(
        normalized["data"]["saved"], expected["data"]["saved"], strict=True
    ):
        actual_item["path"] = expected_item["path"]
    return normalized


def test_grab_level_0_json_matches_fixture(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(monkeypatch, capsys, "--json")
    assert code == 0
    assert message == ""
    assert err == ""
    assert json.loads(out) == _expected("level_0.json")


def test_grab_level_0_human_lists_days_with_counts(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(monkeypatch, capsys)
    assert code == 0
    assert message == ""
    assert err == ""
    assert "day" in out
    assert "20240102" in out
    assert "20240103" in out


def test_grab_level_0_human_ends_with_next_footer(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(monkeypatch, capsys)
    assert code == 0
    assert message == ""
    assert err == ""
    assert out.rstrip().endswith("Next: journal grab <day>")


def test_grab_level_1_json_matches_fixture(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(monkeypatch, capsys, "--json", "20240102")
    assert code == 0
    assert message == ""
    assert err == ""
    assert json.loads(out) == _expected("level_1.json")


def test_grab_level_1_human_ends_with_next_footer(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(monkeypatch, capsys, "20240102")
    assert code == 0
    assert message == ""
    assert err == ""
    assert out.rstrip().endswith("Next: journal grab <day> <stream>")


def test_grab_missing_day_errors(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(monkeypatch, capsys, "20990101")
    assert code == 1
    assert out == ""
    assert err == ""
    assert message.startswith("day 20990101 not found\n\n")
    assert "Available days (closest 5):\n  20240102" in message


def test_grab_level_2_json_matches_fixture(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "--json", "20240103", "default"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert json.loads(out) == _expected("level_2.json")


def test_grab_level_2_human_ends_with_next_footer(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(monkeypatch, capsys, "20240103", "default")
    assert code == 0
    assert message == ""
    assert err == ""
    assert out.rstrip().endswith("Next: journal grab <day> <stream> <segment>")


def test_grab_missing_stream_errors(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(monkeypatch, capsys, "20240102", "missing")
    assert code == 1
    assert out == ""
    assert err == ""
    assert (
        message
        == "stream missing not found in 20240102\n\nAvailable streams in 20240102:\n  default"
    )


def test_grab_level_3_json_matches_fixture(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "--json", "20240103", "default", "110000_300"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert json.loads(out) == _expected("level_3.json")


def test_grab_level_3_purged_json_matches_fixture(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "--json", "20240104", "default", "120000_300"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert json.loads(out) == _expected("level_3_purged.json")


def test_grab_level_3_pins_all_status_strings(monkeypatch, capsys):
    statuses = set()
    for args in (
        ("20240102", "default", "233000_300"),
        ("20240103", "default", "110000_300"),
        ("20240104", "default", "120000_300"),
    ):
        code, message, out, err = _invoke_grab(monkeypatch, capsys, "--json", *args)
        assert code == 0
        assert message == ""
        assert err == ""
        payload = json.loads(out)
        statuses.update(screen["status"] for screen in payload["data"]["screens"])

    assert statuses == {
        "analyzed",
        "analyzed; raw media purged by retention",
        "captured but not analyzed",
    }


def test_grab_level_3_lists_named_monitors(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "--json", "20240103", "default", "100000_300"
    )
    payload = json.loads(out)
    assert code == 0
    assert message == ""
    assert err == ""
    assert [screen["screen"] for screen in payload["data"]["screens"]] == [
        "left_DP-1",
        "right_HDMI-1",
    ]
    assert payload["data"]["screens"][0]["position"] == "left"
    assert payload["data"]["screens"][1]["connector"] == "HDMI-1"


def test_grab_level_3_human_ends_with_next_footer(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240103", "default", "100000_300"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert out.rstrip().endswith("Next: journal grab <day> <stream> <segment> <screen>")


def test_grab_missing_segment_errors(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240103", "default", "999999_300"
    )
    assert code == 1
    assert out == ""
    assert err == ""
    assert (
        message == "segment 999999_300 not found in 20240103/default\n\n"
        "Available segments in 20240103/default:\n"
        "  100000_300\n"
        "  110000_300"
    )


def test_grab_level_4_json_matches_fixture(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "--json", "20240102", "default", "233000_300", "screen"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert json.loads(out) == _expected("level_4.json")


def test_grab_level_4_purged_json_matches_fixture(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "--json", "20240104", "default", "120000_300", "screen"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert json.loads(out) == _expected("level_4_purged.json")


def test_grab_level_4_header_only_json_matches_fixture(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "--json", "20240105", "default", "130000_300", "screen"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert json.loads(out) == _expected("level_4_header_only.json")


def test_grab_level_4_human_includes_error_notes(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240102", "default", "233000_300", "screen"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert "frame_id" in out
    assert "error: Vision request timed out while describing frame 18." in out


def test_grab_level_4_human_includes_extraction_footer(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240102", "default", "233000_300", "screen"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert out.rstrip().endswith(
        "Inspect:    journal grab <day> <stream> <segment> <screen> <id>\n"
        "Save one:   journal grab <day> <stream> <segment> <screen> <id> --out PATH\n"
        "Save many:  journal grab <day> <stream> <segment> <screen> "
        "<id1>,<id2>,... --out PATH\n"
        "\n"
        "How extraction works:\n"
        "  Decoding walks the video linearly from frame 0 — seeking is unsafe at the\n"
        "  1 Hz capture rate. Cost is dominated by the highest requested frame_id, not\n"
        "  the count. Asking for ids 7,12,23 costs the same as asking for 23 alone.\n"
        "  Prefer batch mode when you want more than one frame from the same screen."
    )


def test_grab_level_4_legacy_schema_reports_zero_frames(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240101", "default", "123456_300", "screen"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert out.strip() == "0 frames analyzed: file uses pre-frame_id schema"


def test_grab_level_4_header_only_reports_no_qualified_frames(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240105", "default", "130000_300", "screen"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert out == "No qualified frames in this screen's analysis.\n"


def test_grab_level_4_legacy_and_header_only_are_distinct(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "--json", "20240101", "default", "123456_300", "screen"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert json.loads(out)["data"]["summary"]["legacy_schema"] is True

    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "--json", "20240105", "default", "130000_300", "screen"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    payload = json.loads(out)
    assert payload["data"]["summary"]["frames_analyzed"] == 0
    assert payload["data"]["frames"] == []
    assert payload["data"]["summary"]["legacy_schema"] is False


def test_grab_level_4_purged_human_uses_metadata_only_footer(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240104", "default", "120000_300", "screen"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert "--out PATH" not in out
    assert out.rstrip().endswith(
        "Save mode unavailable: raw video has been purged by retention.\n"
        "Frame metadata above is still readable.\n"
        "\n"
        "Inspect: journal grab <day> <stream> <segment> <screen> <id>"
    )


def test_grab_level_4_captured_but_not_analyzed_errors(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240103", "default", "110000_300", "screen"
    )
    assert code == 1
    assert out == ""
    assert err == ""
    assert message == "screen screen in 110000_300 is captured but not analyzed"


def test_grab_missing_screen_errors(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240102", "default", "233000_300", "missing_screen"
    )
    assert code == 1
    assert out == ""
    assert err == ""
    assert (
        message == "screen missing_screen not found in 20240102/default/233000_300\n\n"
        "Available screens in 20240102/default/233000_300:\n"
        "  screen"
    )


def test_grab_level_5a_json_matches_fixture(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch,
        capsys,
        "--json",
        "20240102",
        "default",
        "233000_300",
        "screen",
        "7",
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert json.loads(out) == _expected("level_5a.json")


def test_grab_level_5a_human_shows_frame_metadata(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240102", "default", "233000_300", "screen", "7"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert "Screen: screen" in out
    assert '"frame_id": 7' in out


def test_grab_level_5a_human_shows_save_and_batch_footer(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240102", "default", "233000_300", "screen", "7"
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert out.rstrip().endswith(
        "Save: journal grab <day> <stream> <segment> <screen> <id> --out PATH\n"
        "Batch: journal grab <day> <stream> <segment> <screen> "
        "<id1>,<id2>,... --out PATH"
    )


def test_grab_json_outputs_do_not_include_footer_prose(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(monkeypatch, capsys, "--json")
    assert code == 0
    assert message == ""
    assert err == ""
    assert "Next:" not in out
    assert "Save:" not in out
    assert "How extraction works:" not in out


def test_grab_level_5a_legacy_schema_errors(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240101", "default", "123456_300", "screen", "1"
    )
    assert code == 1
    assert out == ""
    assert err == ""
    assert (
        message
        == "screen file uses pre-frame_id schema; frame selection is unavailable"
    )


def test_grab_missing_frame_id_errors(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240102", "default", "233000_300", "screen", "999"
    )
    assert code == 1
    assert out == ""
    assert err == ""
    assert message == "frame id 999 not found in screen for 233000_300"


def test_grab_save_purged_video_reports_retention_message(
    monkeypatch, capsys, tmp_path
):
    code, message, out, err = _invoke_grab(
        monkeypatch,
        capsys,
        "--out",
        str(tmp_path / "frame.png"),
        "20240104",
        "default",
        "120000_300",
        "screen",
        "7",
    )
    assert code == 1
    assert out == ""
    assert err == ""
    assert "purged by retention" in message
    assert "journal grab 20240104 default 120000_300 screen 7" in message


def test_grab_level_5b_json_matches_fixture_and_writes_png(
    monkeypatch, capsys, tmp_path
):
    out_path = tmp_path / "frame.png"
    code, message, out, err = _invoke_grab(
        monkeypatch,
        capsys,
        "--json",
        "--out",
        str(out_path),
        "20240102",
        "default",
        "233000_300",
        "screen",
        "7",
    )
    actual = json.loads(out)
    expected = _expected("level_5b.json")
    assert code == 0
    assert message == ""
    assert err == ""
    assert _normalize_saved_paths(actual, expected) == expected
    assert out_path.is_file()
    with Image.open(out_path) as image:
        assert image.size == (64, 48)


def test_grab_level_5b_refuses_overwrite_without_force(monkeypatch, capsys, tmp_path):
    out_path = tmp_path / "frame.png"
    out_path.write_bytes(b"existing")
    code, message, out, err = _invoke_grab(
        monkeypatch,
        capsys,
        "--out",
        str(out_path),
        "20240102",
        "default",
        "233000_300",
        "screen",
        "7",
    )
    assert code == 1
    assert out == ""
    assert err == ""
    assert "output path exists" in message


def test_grab_level_5b_force_replaces_existing_file(monkeypatch, capsys, tmp_path):
    out_path = tmp_path / "frame.png"
    out_path.write_bytes(b"existing")
    code, message, out, err = _invoke_grab(
        monkeypatch,
        capsys,
        "--force",
        "--out",
        str(out_path),
        "20240102",
        "default",
        "233000_300",
        "screen",
        "7",
    )
    assert code == 0
    assert message == ""
    assert err == ""
    assert out_path.is_file()
    with Image.open(out_path) as image:
        assert image.size == (64, 48)


def test_grab_level_5b_unknown_suffix_is_argparse_error(monkeypatch, capsys, tmp_path):
    code, message, out, err = _invoke_grab(
        monkeypatch,
        capsys,
        "--out",
        str(tmp_path / "frame.gif"),
        "20240102",
        "default",
        "233000_300",
        "screen",
        "7",
    )
    assert code == 2
    assert message == ""
    assert out == ""
    assert "--out must end in .png, .jpg, .jpeg, or .webp" in err


def test_grab_level_5c_json_matches_fixture_and_writes_numbered_files(
    monkeypatch, capsys, tmp_path
):
    out_path = tmp_path / "frame.png"
    code, message, out, err = _invoke_grab(
        monkeypatch,
        capsys,
        "--json",
        "--out",
        str(out_path),
        "20240102",
        "default",
        "233000_300",
        "screen",
        "7,12,23",
    )
    actual = json.loads(out)
    expected = _expected("level_5c.json")
    assert code == 0
    assert message == ""
    assert err == ""
    assert _normalize_saved_paths(actual, expected) == expected
    for frame_id in (7, 12, 23):
        saved = tmp_path / f"frame_{frame_id}.png"
        assert saved.is_file()
        with Image.open(saved) as image:
            assert image.size == (64, 48)


def test_grab_level_5c_conflict_scan_happens_before_decode(
    monkeypatch, capsys, tmp_path
):
    out_path = tmp_path / "frame.png"
    (tmp_path / "frame_12.png").write_bytes(b"existing")
    decode_mock = Mock(side_effect=AssertionError("decode should not run"))
    monkeypatch.setattr("solstone.observe.grab.decode_frames", decode_mock)
    code, message, out, err = _invoke_grab(
        monkeypatch,
        capsys,
        "--out",
        str(out_path),
        "20240102",
        "default",
        "233000_300",
        "screen",
        "7,12,23",
    )
    assert code == 1
    assert out == ""
    assert err == ""
    assert "output path exists" in message
    decode_mock.assert_not_called()


def test_grab_level_5c_decode_failure_writes_no_files(monkeypatch, capsys, tmp_path):
    out_path = tmp_path / "frame.png"
    monkeypatch.setattr(
        "solstone.observe.grab.decode_frames",
        Mock(side_effect=RuntimeError("decode blew up")),
    )
    code, message, out, err = _invoke_grab(
        monkeypatch,
        capsys,
        "--out",
        str(out_path),
        "20240102",
        "default",
        "233000_300",
        "screen",
        "7,12,23",
    )
    assert code == 1
    assert out == ""
    assert err == ""
    assert message == "decode blew up"
    assert list(tmp_path.iterdir()) == []


def test_grab_level_5c_requires_out_for_multiple_frame_ids(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "20240102", "default", "233000_300", "screen", "7,12,23"
    )
    assert code == 2
    assert message == ""
    assert out == ""
    assert "multiple frame ids require --out" in err


def test_grab_rejects_more_than_five_positionals(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "a", "b", "c", "d", "e", "f"
    )
    assert code == 2
    assert message == ""
    assert out == ""
    assert "at most 5 positional tokens" in err


def test_grab_force_requires_out(monkeypatch, capsys):
    code, message, out, err = _invoke_grab(monkeypatch, capsys, "--force")
    assert code == 2
    assert message == ""
    assert out == ""
    assert "--force requires --out" in err


def test_grab_out_requires_level_5(monkeypatch, capsys, tmp_path):
    code, message, out, err = _invoke_grab(
        monkeypatch, capsys, "--out", str(tmp_path / "frame.png"), "20240102"
    )
    assert code == 2
    assert message == ""
    assert out == ""
    assert "--out requires day stream segment screen and frame-id" in err


def test_grab_malformed_jsonl_quiet_by_default(monkeypatch, capsys, caplog):
    code, message, out, err = _invoke_grab(monkeypatch, capsys)
    assert code == 0
    assert message == ""
    assert "20240106" in out
    assert "WARNING:observe.utils:" not in err
    assert "Invalid JSON" not in caplog.text


def test_grab_malformed_jsonl_warns_with_verbose(monkeypatch, capsys, caplog):
    code, message, out, err = _invoke_grab(monkeypatch, capsys, "-v")
    assert code == 0
    assert message == ""
    assert "20240106" in out
    assert "Invalid JSON" in caplog.text


@pytest.mark.parametrize("token", ["0", "-1", "abc", "7,7", "1,,2"])
def test_grab_frame_id_token_rejects_invalid_values(token):
    from solstone.observe.grab import parse_frame_id_token

    with pytest.raises(ValueError):
        parse_frame_id_token(token)


def test_grab_frame_id_token_sorts_batch_ids():
    from solstone.observe.grab import parse_frame_id_token

    assert parse_frame_id_token("23,7,12") == [7, 12, 23]
