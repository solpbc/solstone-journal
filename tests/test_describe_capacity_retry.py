# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import io
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from PIL import Image

from solstone.observe import describe as describe_module
from solstone.think.responsiveness import (
    NON_RESPONSIVE_OUTPUT_MESSAGE,
    NON_RESPONSIVE_REASON_CODE,
)


def _video_path(tmp_path: Path) -> Path:
    segment_dir = tmp_path / "chronicle" / "20250101" / "default" / "143022_300"
    segment_dir.mkdir(parents=True)
    video_path = segment_dir / "screen.webm"
    video_path.write_text("video", encoding="utf-8")
    return video_path


def _png_bytes() -> bytes:
    image_bytes = io.BytesIO()
    Image.new("RGB", (8, 8), "white").save(image_bytes, format="PNG")
    return image_bytes.getvalue()


def _frame(frame_id: int, frame_bytes: bytes) -> dict:
    return {
        "frame_id": frame_id,
        "timestamp": float(frame_id),
        "frame_bytes": frame_bytes,
        "aruco": None,
    }


def _processor(video_path: Path, frames: list[dict], monkeypatch) -> object:
    processor = describe_module.VideoProcessor.__new__(describe_module.VideoProcessor)
    processor.video_path = video_path
    processor.first_hash = None
    processor.last_hash = None
    processor.qualified_count = len(frames)
    processor.qualified_frames = []
    monkeypatch.setattr(processor, "process", lambda: frames)
    return processor


def _jsonl_rows(path: Path) -> list[dict]:
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line
    ]


def _assert_no_describe_temp(directory: Path) -> None:
    names = [path.name for path in directory.iterdir()]
    assert not any(
        name.startswith(".describe_") or name.endswith(".tmp") for name in names
    )


def _capacity_error():
    from solstone.think.providers import local as local_provider
    from solstone.think.providers.local_endpoint import LocalEndpoint

    inner = type("ReadTimeout", (Exception,), {})("capacity wait timed out")
    outer = RuntimeError("outer")
    outer.__cause__ = inner
    endpoint = LocalEndpoint(
        base_url="http://endpoint.test",
        served_model_id="m",
        credential=None,
        is_bundled=False,
    )
    return local_provider._classify_byo_generate_error(outer, endpoint)


class RetryBatch:
    mode = "phase1"
    attempts: dict[tuple, int] = {}

    def __init__(self, max_concurrent=5, client=None):
        self.max_concurrent = max_concurrent
        self.client = client
        self.pending_tasks = set()
        self.queue = []

    async def aclose(self):
        # The session restructure gave Batch a real close: drain, reap the
        # child, reject later additions. The double records it so a caller
        # that forgets to close is visible rather than silently fine.
        self.closed = getattr(self, "closed", 0) + 1

    def create(self, **kwargs):
        return SimpleNamespace(
            **kwargs,
            response=None,
            error=None,
            duration=0.01,
            model_used="gemini-test",
            provider=None,
            reason_code=None,
            reset_at_ms=None,
        )

    def add(self, request):
        self.queue.append(request)

    def update(self, request, **kwargs):
        for key, value in kwargs.items():
            setattr(request, key, value)
        request.error = None
        request.reason_code = None
        self.add(request)

    async def drain_batch(self):
        while self.queue:
            pending = self.queue
            self.queue = []
            for request in pending:
                key = (
                    request.request_type.value,
                    request.frame_id,
                    getattr(request, "extraction_category", None),
                )
                attempt = self.attempts.get(key, 0)
                self.attempts[key] = attempt + 1

                should_fail = False
                if self.mode == "phase1" and request.request_type.value == "describe":
                    should_fail = attempt == 0
                elif self.mode == "phase3" and request.request_type.value == "category":
                    should_fail = attempt == 0
                elif (
                    self.mode == "phase3_exhausted"
                    and request.request_type.value == "category"
                    and request.frame_id == 1
                ):
                    should_fail = True
                elif self.mode == "one_frame_exhausted" and request.frame_id == 1:
                    should_fail = request.request_type.value == "describe"

                if should_fail:
                    error = _capacity_error()
                    request.error = str(error)
                    request.reason_code = error.reason_code
                else:
                    request.error = None
                    request.reason_code = None
                    if request.request_type.value == "describe":
                        request.response = json.dumps(
                            {"primary": "code", "secondary": "none", "overlap": True}
                        )
                    else:
                        request.response = "extracted text"
                yield request


def _install_fakes(monkeypatch, *, mode: str) -> None:
    from solstone.think import batch as batch_module
    from solstone.think import models

    RetryBatch.mode = mode
    RetryBatch.attempts = {}
    monkeypatch.setattr(batch_module, "Batch", RetryBatch)
    monkeypatch.setattr(models, "resolve_provider", lambda _interface: ("google", "g"))
    monkeypatch.setattr(describe_module, "callosum_send", lambda *args, **kwargs: True)


def _install_real_batch_provider(monkeypatch, outcomes: list[dict]) -> SimpleNamespace:
    import solstone.think.providers as providers_package
    from solstone.think import batch as batch_module
    from solstone.think import models

    provider_module = SimpleNamespace(
        run_agenerate=AsyncMock(side_effect=outcomes),
    )
    monkeypatch.setattr(models, "resolve_provider", lambda _interface: ("fake", "m"))
    monkeypatch.setattr(
        batch_module, "resolve_provider", lambda _interface: ("fake", "m")
    )
    monkeypatch.setattr(
        providers_package,
        "get_provider_module",
        lambda _provider: provider_module,
    )
    monkeypatch.setattr(describe_module, "callosum_send", lambda *args, **kwargs: True)
    return provider_module


def _non_responsive_result() -> dict:
    return {
        "text": "I cannot describe this screen.",
        "model": "provider-model",
        "finish_reason": "stop",
    }


def _describe_result(primary: str) -> dict:
    return {
        "text": json.dumps(
            {
                "visual_description": "A code editor is open.",
                "primary": primary,
                "secondary": "none",
                "overlap": True,
            }
        ),
        "model": "provider-model",
        "finish_reason": "stop",
    }


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["phase1", "phase3"])
async def test_capacity_class_describe_errors_retry_and_promote_output(
    tmp_path, monkeypatch, mode
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path, [_frame(1, frame_bytes), _frame(2, frame_bytes)], monkeypatch
    )
    _install_fakes(monkeypatch, mode=mode)
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [1] if mode == "phase3" else [],
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    rows = _jsonl_rows(output_path)
    assert rows[0]["_solstone_processing"]["state"] == "analyzed"
    assert {row["frame_id"] for row in rows[1:]} == {1, 2}
    if mode == "phase3":
        enhanced = next(row for row in rows[1:] if row["frame_id"] == 1)
        assert enhanced["enhanced"] is True
        assert enhanced["content"]["code"] == "extracted text"
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_capacity_class_exhausted_frame_with_successful_sibling_marks_failed(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path, [_frame(1, frame_bytes), _frame(2, frame_bytes)], monkeypatch
    )
    _install_fakes(monkeypatch, mode="one_frame_exhausted")
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    rows = _jsonl_rows(output_path)
    assert rows[0]["_solstone_processing"]["state"] == "failed"
    assert rows[0]["_solstone_processing"]["reason_code"] == "analysis_failed"
    failed = next(row for row in rows[1:] if row["frame_id"] == 1)
    succeeded = next(row for row in rows[1:] if row["frame_id"] == 2)
    assert "error" in failed
    assert succeeded["analysis"]["primary"] == "code"
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_describe_non_responsive_phase1_does_not_retry(tmp_path, monkeypatch):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path, [_frame(1, frame_bytes), _frame(2, frame_bytes)], monkeypatch
    )
    provider_module = _install_real_batch_provider(
        monkeypatch,
        [_non_responsive_result(), _describe_result("code")],
    )
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    assert provider_module.run_agenerate.await_count == 2


@pytest.mark.asyncio
async def test_describe_non_responsive_single_frame_decline_preserves_other_frames(
    tmp_path, monkeypatch
):
    from solstone.convey.provider_readiness import is_blocking_reason

    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path, [_frame(1, frame_bytes), _frame(2, frame_bytes)], monkeypatch
    )
    _install_real_batch_provider(
        monkeypatch,
        [_non_responsive_result(), _describe_result("code")],
    )
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    assert is_blocking_reason(NON_RESPONSIVE_REASON_CODE) is False

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    rows = _jsonl_rows(output_path)
    assert rows[0]["_solstone_processing"]["state"] == "failed"
    assert rows[0]["_solstone_processing"]["reason_code"] == "analysis_failed"
    declined = next(row for row in rows[1:] if row["frame_id"] == 1)
    sibling = next(row for row in rows[1:] if row["frame_id"] == 2)
    assert NON_RESPONSIVE_OUTPUT_MESSAGE in declined["error"]
    assert sibling["analysis"]["primary"] == "code"
    assert "error" not in sibling
    assert output_path.exists()
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_describe_non_responsive_phase3_does_not_retry_and_does_not_block(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(video_path, [_frame(1, frame_bytes)], monkeypatch)
    provider_module = _install_real_batch_provider(
        monkeypatch,
        [_describe_result("code"), _non_responsive_result()],
    )
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [1],
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    rows = _jsonl_rows(output_path)
    assert provider_module.run_agenerate.await_count == 2
    assert rows[0]["_solstone_processing"]["state"] == "failed"
    row = rows[1]
    assert row["frame_id"] == 1
    assert row["enhanced"] is True
    assert row["content"] == {}
    assert NON_RESPONSIVE_OUTPUT_MESSAGE in row["error"]
    assert "retries" not in row["requests"][-1]
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_phase3_extraction_error_demotes_record_but_ships_row(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path, [_frame(1, frame_bytes), _frame(2, frame_bytes)], monkeypatch
    )
    _install_fakes(monkeypatch, mode="phase3_exhausted")
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [1],
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    rows = _jsonl_rows(output_path)
    assert rows[0]["_solstone_processing"]["state"] == "failed"
    assert rows[0]["_solstone_processing"]["reason_code"] == "analysis_failed"
    assert {row["frame_id"] for row in rows[1:]} == {1, 2}
    enhanced = next(row for row in rows[1:] if row["frame_id"] == 1)
    sibling = next(row for row in rows[1:] if row["frame_id"] == 2)
    assert enhanced["enhanced"] is True
    assert enhanced["content"] == {}
    assert "error" in enhanced
    assert "pending" not in enhanced
    assert enhanced["requests"][-1]["category"] == "code"
    assert enhanced["requests"][-1]["retries"] == 4
    assert sibling["enhanced"] is False
    assert "error" not in sibling
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
@pytest.mark.parametrize("jobs", [1, 7])
async def test_describe_explicit_jobs_wins_without_policy_resolution(
    tmp_path, monkeypatch, jobs
):
    video_path = _video_path(tmp_path)
    observed = []

    async def fake_process_with_vision(
        self,
        max_concurrent: int,
        output_path: Path | None = None,
        work_key: str | None = None,
        previous_attempts: int = 0,
        incremental_source_path: Path | None = None,
    ) -> None:
        del self, output_path, work_key, previous_attempts, incremental_source_path
        observed.append(max_concurrent)

    def fail_policy(_effective_procs: int) -> int:
        raise AssertionError("explicit -j must not resolve through policy")

    monkeypatch.setattr(describe_module, "require_solstone", lambda: None)
    monkeypatch.setattr(
        describe_module.VideoProcessor,
        "process_with_vision",
        fake_process_with_vision,
    )
    monkeypatch.setattr(
        "solstone.think.providers.fanout_policy.describe_per_proc_jobs",
        fail_policy,
    )
    monkeypatch.setattr(
        "sys.argv",
        ["journal describe", str(video_path), "-j", str(jobs)],
    )

    await describe_module.async_main()

    assert observed == [jobs]
