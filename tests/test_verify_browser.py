# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import base64
import sys
from io import BytesIO
from pathlib import Path

import pytest
from PIL import Image

sys.path.insert(0, str(Path(__file__).parent))
import verify_browser as vb  # noqa: E402


def _png(color: tuple[int, int, int, int], size: tuple[int, int] = (4, 4)) -> bytes:
    image = Image.new("RGBA", size, color)
    buffer = BytesIO()
    image.save(buffer, format="PNG")
    return buffer.getvalue()


class FakeConnection:
    def __init__(
        self,
        runtime_values: list[object] | None = None,
        *,
        fail_method: str | None = None,
    ) -> None:
        self.runtime_values = list(runtime_values or [])
        self.fail_method = fail_method
        self.calls: list[tuple[str, dict | None]] = []

    def call(
        self,
        method: str,
        params: dict | None = None,
        *,
        timeout: float | None = None,
    ) -> dict:
        del timeout
        self.calls.append((method, params))
        if self.fail_method == method:
            raise vb.CdpError(f"{method} failed")
        if method == "Runtime.evaluate":
            value = self.runtime_values.pop(0) if self.runtime_values else None
            return {"result": {"value": value}}
        if method == "Page.captureScreenshot":
            return {"data": base64.b64encode(b"image").decode("ascii")}
        return {}

    def wait_for_event(self, method: str, *, timeout: float = 10.0) -> dict:
        del method, timeout
        return {}

    def close(self) -> None:
        return None


class FakeBrowser:
    def __init__(self, connection: FakeConnection) -> None:
        self.connection = connection
        self.closed_targets: list[str] = []

    def create_page(self, url: str = "about:blank") -> vb.CdpPage:
        del url
        return vb.CdpPage("target", self.connection, self)

    def close_target(self, target_id: str) -> None:
        self.closed_targets.append(target_id)


def _page(connection: FakeConnection) -> vb.CdpPage:
    return vb.CdpPage("target", connection, FakeBrowser(connection))


def test_parse_remote_debugging_port_from_arg_list() -> None:
    assert (
        vb.parse_remote_debugging_port(
            ["chrome", "--headless", "--remote-debugging-port=9869"]
        )
        == 9869
    )


def test_parse_remote_debugging_port_from_proc_cmdline() -> None:
    cmdline = "chrome\0--remote-debugging-port=19871\0--other-flag\0"
    assert vb.parse_remote_debugging_port(cmdline) == 19871


def test_parse_remote_debugging_port_rejects_absent_or_invalid_values() -> None:
    assert vb.parse_remote_debugging_port(["chrome"]) is None
    assert vb.parse_remote_debugging_port(["--remote-debugging-port=0"]) is None
    assert vb.parse_remote_debugging_port(["--remote-debugging-port=99999"]) is None
    assert vb.parse_remote_debugging_port(["--remote-debugging-port=abc"]) is None


def test_build_device_metrics_payload() -> None:
    assert vb.build_device_metrics_payload(1265, 500) == {
        "width": 1265,
        "height": 500,
        "deviceScaleFactor": 1,
        "mobile": False,
    }


def test_baseline_path_uses_jpg_for_human_review_and_png_for_diff() -> None:
    assert vb.baseline_path({"app": "transcripts", "name": "smoke"}) == Path(
        "tests/baselines/visual/transcripts/smoke.jpg"
    )
    assert vb.baseline_path(
        {"app": "transcripts", "name": "day-short", "diff": True}
    ) == Path("tests/baselines/visual/transcripts/day-short.png")


def test_select_scenarios_rejects_unknown_filter() -> None:
    try:
        vb._select_scenarios(["transcripts/missing"])
    except ValueError as exc:
        assert "unknown browser scenario filter" in str(exc)
        assert "transcripts/day-short" in str(exc)
    else:
        raise AssertionError("expected unknown scenario filter to fail")


def test_compare_png_passes_identical_images() -> None:
    png = _png((255, 255, 255, 255))
    result = vb.compare_png(
        png,
        png,
        channel_delta_threshold=0,
        changed_pixels_pct_threshold=0.0,
    )
    assert result.passed
    assert result.changed_pixels == 0
    assert result.max_channel_delta == 0
    assert result.diff_image_bytes


def test_compare_png_fails_dimension_mismatch() -> None:
    actual = _png((255, 255, 255, 255), size=(4, 4))
    baseline = _png((255, 255, 255, 255), size=(3, 4))
    result = vb.compare_png(actual, baseline)
    assert not result.passed
    assert "dimension mismatch" in result.message
    assert result.diff_image_bytes


def test_compare_png_fails_over_tolerance_and_emits_diff() -> None:
    actual = _png((255, 0, 0, 255))
    baseline = _png((255, 255, 255, 255))
    result = vb.compare_png(
        actual,
        baseline,
        channel_delta_threshold=1,
        changed_pixels_pct_threshold=0.0,
    )
    assert not result.passed
    assert result.changed_pixels == 16
    assert result.max_channel_delta == 255
    assert result.diff_image_bytes.startswith(b"\x89PNG")


def test_visual_artifact_paths() -> None:
    actual, diff = vb.visual_artifact_paths(
        Path("tests/baselines/visual/transcripts/day-short.png")
    )
    assert actual == Path("tests/baselines/visual/transcripts/day-short.actual.png")
    assert diff == Path("tests/baselines/visual/transcripts/day-short.diff.png")


def test_cdp_page_text_reads_innerText() -> None:
    connection = FakeConnection(["Hello"])
    page = _page(connection)

    assert page.text() == "Hello"
    assert (
        "Runtime.evaluate",
        {
            "expression": "document.body.innerText",
            "returnByValue": True,
            "awaitPromise": True,
        },
    ) in connection.calls


def test_cdp_page_assert_text_case_insensitive() -> None:
    page = _page(FakeConnection(["Hello WORLD", "Hello WORLD"]))

    assert page.assert_text("hello")
    assert not page.assert_text("xyz")


def test_cdp_page_evaluate_json_parses_value() -> None:
    page = _page(FakeConnection(['{"a":1}']))

    assert page.evaluate_json("({a: 1})") == {"a": 1}


def test_cdp_page_snapshot_returns_nodes_shape() -> None:
    payload = (
        '{"nodes":[{"ref":"e1","role":"textbox","tag":"input",'
        '"name":"q","text":"","label":"","value":""}]}'
    )
    connection = FakeConnection([payload])
    snapshot = _page(connection).snapshot()

    assert set(snapshot["nodes"][0]) == {
        "ref",
        "role",
        "tag",
        "name",
        "text",
        "label",
        "value",
    }
    assert "__solstoneRefs" in connection.calls[-1][1]["expression"]


def test_cdp_page_type_ref_focuses_then_inserts() -> None:
    connection = FakeConnection([None])
    _page(connection).type_ref("e1", "romeo")
    action_calls = connection.calls[2:]

    assert action_calls[0] == (
        "Runtime.evaluate",
        {
            "expression": 'window.__solstoneRefs.get("e1").focus()',
            "returnByValue": True,
            "awaitPromise": True,
        },
    )
    assert action_calls[1] == ("Input.insertText", {"text": "romeo"})


def test_cdp_page_type_ref_raises_on_rpc_error() -> None:
    page = _page(FakeConnection(fail_method="Runtime.evaluate"))

    with pytest.raises(vb.CdpError):
        page.type_ref("e1", "romeo")


def test_run_cdp_scenario_snapshot_find_input_type_pipeline(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    snapshot = (
        '{"nodes":[{"ref":"e1","role":"searchbox","tag":"input",'
        '"name":"","text":"","label":"","value":""}]}'
    )
    connection = FakeConnection([snapshot, None, None, "[]"])
    baseline = tmp_path / "search-flow.jpg"
    baseline.write_bytes(b"baseline")
    monkeypatch.setattr(vb, "baseline_path", lambda _scenario: baseline)
    monkeypatch.setattr(vb.time, "sleep", lambda _seconds: None)

    scenario = next(
        s for s in vb.SCENARIOS if vb._scenario_id(s) == "search/search-flow"
    )
    result = vb.run_cdp_scenario(
        FakeBrowser(connection), scenario, "http://base", "verify"
    )

    assert result["ok"]
    assert ("Input.insertText", {"text": "romeo"}) in connection.calls


def test_run_cdp_scenario_missing_variable_in_type_raises() -> None:
    scenario = {
        "app": "x",
        "name": "missing",
        "steps": [{"do": "type", "var": "q", "text": "x"}],
    }
    result = vb.run_cdp_scenario(
        FakeBrowser(FakeConnection()), scenario, "http://base", "verify"
    )

    assert not result["ok"]
    assert "type step missing variable 'q'" in result["errors"][0]


def test_run_cdp_scenario_assert_text_missing_appends_error() -> None:
    scenario = {
        "app": "x",
        "name": "assert",
        "steps": [{"do": "assert_text", "text": "PASS"}],
    }
    result = vb.run_cdp_scenario(
        FakeBrowser(FakeConnection(["nope"])), scenario, "http://base", "verify"
    )

    assert not result["ok"]
    assert "assert_text failed" in result["errors"][0]


def test_run_cdp_scenario_installs_error_listener() -> None:
    connection = FakeConnection()
    result = vb.run_cdp_scenario(
        FakeBrowser(connection),
        {"app": "x", "name": "empty", "steps": []},
        "http://base",
        "verify",
    )

    assert result["ok"]
    assert (
        "Page.addScriptToEvaluateOnNewDocument",
        {"source": vb._ERROR_LISTENER_JS},
    ) in connection.calls


def test_cdp_page_wait_step_sleeps(monkeypatch: pytest.MonkeyPatch) -> None:
    sleeps: list[float] = []
    monkeypatch.setattr(vb.time, "sleep", sleeps.append)
    scenario = {"app": "x", "name": "wait", "steps": [{"do": "wait", "ms": 250}]}

    vb.run_cdp_scenario(
        FakeBrowser(FakeConnection()), scenario, "http://base", "verify"
    )

    assert sleeps == [0.25]


def test_cdp_page_evaluate_step_calls_runtime() -> None:
    expression = "document.cookie='facet=work;path=/'"
    connection = FakeConnection([None])
    scenario = {
        "app": "x",
        "name": "evaluate",
        "steps": [{"do": "evaluate", "expression": expression}],
    }
    result = vb.run_cdp_scenario(
        FakeBrowser(connection), scenario, "http://base", "verify"
    )

    assert result["ok"]
    assert (
        "Runtime.evaluate",
        {"expression": expression, "returnByValue": True, "awaitPromise": True},
    ) in connection.calls
