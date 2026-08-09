# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import io
import json
import logging
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from PIL import Image

from solstone.observe import describe as describe_module
from solstone.observe import detect as detect_module
from solstone.observe import processing_record as processing_record_module
from solstone.think.providers.rfdetr_install import RfdetrPaths


def _video_path(tmp_path: Path) -> Path:
    segment_dir = tmp_path / "chronicle" / "20250101" / "default" / "143022_300"
    segment_dir.mkdir(parents=True)
    video_path = segment_dir / "screen.webm"
    video_path.write_text("video", encoding="utf-8")
    return video_path


def _png_bytes(size: tuple[int, int] = (8, 8)) -> bytes:
    image_bytes = io.BytesIO()
    Image.new("RGB", size, "white").save(image_bytes, format="PNG")
    return image_bytes.getvalue()


def _frame(frame_id: int, timestamp: float, frame_bytes: bytes) -> dict:
    return {
        "frame_id": frame_id,
        "timestamp": timestamp,
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


def _assert_no_describe_temp(directory: Path) -> None:
    names = [path.name for path in directory.iterdir()]
    assert not any(
        name.startswith(".describe_") or name.endswith(".tmp") for name in names
    )


def _jsonl_rows(path: Path) -> list[dict]:
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line
    ]


def _generate_result(text: str, finish_reason: str = "stop") -> dict:
    return {"text": text, "finish_reason": finish_reason}


def _canned_detection() -> dict:
    return {
        "image": {"width": 8, "height": 8},
        "detections": [
            {
                "class_id": 42,
                "class_name": "cup",
                "score": 0.7,
                "bbox": [1, 2, 3, 4],
            }
        ],
    }


def _install_fakes(monkeypatch, outcomes: dict[int, dict]) -> list[tuple]:
    from solstone.think import batch as batch_module
    from solstone.think import models

    FakeBatch.instances = []
    FakeBatch.outcomes = outcomes
    monkeypatch.delenv("OBSERVER_NAME", raising=False)
    monkeypatch.delenv("SEGMENT_META", raising=False)
    monkeypatch.setattr(batch_module, "Batch", FakeBatch)
    monkeypatch.setattr(
        models,
        "resolve_provider",
        lambda _interface: ("google", "gemini-test"),
    )
    monkeypatch.setattr(
        processing_record_module, "now_iso_utc", lambda: "2026-06-30T12:00:00Z"
    )
    emitted = []
    monkeypatch.setattr(
        describe_module,
        "callosum_send",
        lambda tract, event, **kwargs: emitted.append((tract, event, kwargs)),
    )
    return emitted


def test_build_metadata_header_includes_static_single_frame_hash(tmp_path, monkeypatch):
    video_path = _video_path(tmp_path)
    processor = describe_module.VideoProcessor.__new__(describe_module.VideoProcessor)
    processor.video_path = video_path
    processor.first_hash = 0x1234
    processor.last_hash = 0x1234
    processor.qualified_count = 1
    monkeypatch.setenv("OBSERVER_NAME", "desk")
    monkeypatch.setenv("SEGMENT_META", json.dumps({"stream": "default"}))

    assert processor._build_metadata_header() == {
        "raw": video_path.name,
        "observer": "desk",
        "stream": "default",
        "first_hash": "0000000000001234",
        "last_hash": "0000000000001234",
        "qualified_count": 1,
    }


def test_describe_header_raw_is_producer_invariant(tmp_path, monkeypatch):
    video_path = _video_path(tmp_path)
    processor = describe_module.VideoProcessor.__new__(describe_module.VideoProcessor)
    processor.video_path = video_path
    processor.first_hash = None
    processor.last_hash = None
    processor.qualified_count = 1
    monkeypatch.delenv("OBSERVER_NAME", raising=False)
    monkeypatch.delenv("SEGMENT_META", raising=False)

    header = processor._build_metadata_header()

    # raw is the producer's invariant (relaxed from the shared floor), so the
    # describer must keep emitting it.
    assert "raw" in header
    assert header["raw"] == video_path.name


class FakeBatch:
    instances = []
    outcomes = {}

    def __init__(self, max_concurrent=5, client=None):
        self.max_concurrent = max_concurrent
        self.pending_tasks = set()
        self.queue = []
        self.add_count = 0
        FakeBatch.instances.append(self)

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
        self.add_count += 1
        self.queue.append(request)

    def update(self, request, **kwargs):
        for key, value in kwargs.items():
            setattr(request, key, value)
        request.error = None
        request.reason_code = None
        self.add(request)

    async def drain_batch(self):
        pending = self.queue
        self.queue = []
        for request in pending:
            outcome = FakeBatch.outcomes.get(request.frame_id, {})
            if outcome.get("fail"):
                request.error = outcome.get("error", "boom")
                request.reason_code = outcome.get("reason_code")
                request.retry_count = 4
            else:
                request.error = None
                request.response = outcome.get(
                    "response",
                    json.dumps(
                        {"primary": "code", "secondary": "none", "overlap": True}
                    ),
                )
            yield request


class MergeBatch:
    instances = []
    outcomes: dict[tuple, dict] = {}

    def __init__(self, max_concurrent=5, client=None):
        self.max_concurrent = max_concurrent
        self.pending_tasks = set()
        self.queue = []
        self.history = []
        MergeBatch.instances.append(self)

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
        pending = self.queue
        self.queue = []
        for request in pending:
            key = (
                request.request_type.value,
                request.frame_id,
                getattr(request, "extraction_category", None),
            )
            self.history.append(key)
            outcome = MergeBatch.outcomes.get(key, {})
            if outcome.get("fail"):
                request.error = outcome.get("error", "boom")
                request.reason_code = outcome.get("reason_code")
                request.retry_count = 4
            else:
                request.error = None
                request.reason_code = None
                request.response = outcome.get("response")
                if request.response is None:
                    if request.request_type == describe_module.RequestType.DESCRIBE:
                        request.response = json.dumps(
                            {
                                "primary": "code",
                                "secondary": "none",
                                "overlap": True,
                            }
                        )
                    else:
                        request.response = f"{request.extraction_category} text"
            yield request


def _install_merge_fakes(monkeypatch, outcomes: dict[tuple, dict] | None = None):
    from solstone.think import batch as batch_module
    from solstone.think import models

    MergeBatch.instances = []
    MergeBatch.outcomes = outcomes or {}
    monkeypatch.delenv("OBSERVER_NAME", raising=False)
    monkeypatch.delenv("SEGMENT_META", raising=False)
    monkeypatch.setattr(batch_module, "Batch", MergeBatch)
    monkeypatch.setattr(
        models,
        "resolve_provider",
        lambda _interface: ("google", "gemini-test"),
    )
    monkeypatch.setattr(
        processing_record_module, "now_iso_utc", lambda: "2026-06-30T12:00:00Z"
    )
    monkeypatch.setattr(describe_module, "callosum_send", lambda *a, **k: None)


def _fingerprinted_processor(
    video_path: Path,
    frames: list[dict],
    monkeypatch,
    *,
    first_hash: int | None = 0x1111,
    last_hash: int | None = 0x2222,
) -> object:
    processor = _processor(video_path, frames, monkeypatch)
    processor.first_hash = first_hash
    processor.last_hash = last_hash
    processor.qualified_count = len(frames)
    return processor


def _processing_record(
    video_path: Path,
    *,
    attempts: int | None = 1,
    input_size: int | None = None,
) -> dict:
    record = {
        "schema": "solstone.processing.v1",
        "state": "failed",
        "reason_code": "analysis_failed",
        "handler": "describe",
        "attempted_at": "2026-06-30T12:00:00Z",
        "input_size": video_path.stat().st_size if input_size is None else input_size,
    }
    if attempts is not None:
        record["attempts"] = attempts
    return record


def _existing_header(
    video_path: Path,
    *,
    first_hash: int | None = 0x1111,
    last_hash: int | None = 0x2222,
    qualified_count: int = 1,
    attempts: int | None = 1,
    input_size: int | None = None,
    observer: str | None = None,
) -> dict:
    header = {
        "raw": video_path.name,
        "first_hash": (None if first_hash is None else f"{first_hash:016x}"),
        "last_hash": None if last_hash is None else f"{last_hash:016x}",
        "qualified_count": qualified_count,
        "_solstone_processing": _processing_record(
            video_path,
            attempts=attempts,
            input_size=input_size,
        ),
    }
    if observer is not None:
        header["observer"] = observer
    return header


def _write_existing_output(
    output_path: Path,
    header: dict,
    row_lines: list[str],
) -> None:
    output_path.write_text(
        json.dumps(header) + "\n" + "".join(row_lines),
        encoding="utf-8",
    )


@pytest.mark.asyncio
async def test_success_with_mixed_results_promotes_byte_identical_jsonl(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path,
        [
            _frame(1, 0.0, frame_bytes),
            _frame(2, 1.25, frame_bytes),
        ],
        monkeypatch,
    )
    _install_fakes(
        monkeypatch,
        {
            1: {},
            2: {"fail": True, "error": "boom"},
        },
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

    request_type = describe_module.RequestType.DESCRIBE.value
    header = json.dumps(
        {
            "raw": video_path.name,
            "first_hash": None,
            "last_hash": None,
            "qualified_count": 2,
            "_solstone_thinking": {"provider": "google", "model": "gemini-test"},
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "failed",
                "reason_code": "analysis_failed",
                "handler": "describe",
                "attempted_at": "2026-06-30T12:00:00Z",
                "input_size": 5,
                "attempts": 1,
            },
        }
    )
    frame1 = json.dumps(
        {
            "frame_id": 1,
            "timestamp": 0.0,
            "requests": [
                {"type": request_type, "model": "gemini-test", "duration": 0.01}
            ],
            "analysis": {"primary": "code", "secondary": "none", "overlap": True},
            "enhanced": False,
        }
    )
    frame2 = json.dumps(
        {
            "frame_id": 2,
            "timestamp": 1.25,
            "requests": [
                {
                    "type": request_type,
                    "model": "gemini-test",
                    "duration": 0.01,
                    "retries": 4,
                }
            ],
            "error": "boom",
            "enhanced": False,
        }
    )
    expected = "".join(line + "\n" for line in [header, frame1, frame2])

    assert output_path.read_text() == expected
    assert output_path.name in [path.name for path in output_path.parent.iterdir()]
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_incremental_merge_reuses_clean_rows_and_requests_only_gaps(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    frames = [
        _frame(1, 0.0, frame_bytes),
        _frame(2, 1.25, frame_bytes),
        _frame(3, 2.5, frame_bytes),
    ]
    processor = _fingerprinted_processor(video_path, frames, monkeypatch)
    request_type = describe_module.RequestType.DESCRIBE.value
    category_type = describe_module.RequestType.CATEGORY.value
    clean_line = (
        '{"frame_id":1,"timestamp":0.0,"requests":[{"type":"describe",'
        '"model":"old","duration":0.01}],"analysis":{"primary":"code",'
        '"secondary":"none","overlap":true},"enhanced":false}\n'
    )
    phase1_gap = (
        json.dumps(
            {
                "frame_id": 2,
                "timestamp": 1.25,
                "requests": [{"type": request_type, "model": "old", "duration": 0.01}],
                "error": "boom",
                "enhanced": False,
            }
        )
        + "\n"
    )
    phase3_gap = (
        json.dumps(
            {
                "frame_id": 3,
                "timestamp": 2.5,
                "requests": [
                    {"type": request_type, "model": "old", "duration": 0.01},
                    {
                        "type": category_type,
                        "model": "old",
                        "duration": 0.01,
                        "category": "code",
                    },
                    {
                        "type": category_type,
                        "model": "old",
                        "duration": 0.01,
                        "category": "browsing",
                        "retries": 4,
                    },
                ],
                "analysis": {
                    "primary": "code",
                    "secondary": "browsing",
                    "overlap": False,
                },
                "enhanced": True,
                "content": {"code": "old code"},
                "error": "old browsing failure",
            }
        )
        + "\n"
    )
    _write_existing_output(
        output_path,
        _existing_header(video_path, qualified_count=3),
        [phase3_gap, clean_line, phase1_gap],
    )
    _install_merge_fakes(monkeypatch)
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
        previous_attempts=1,
        incremental_source_path=output_path,
    )

    history = MergeBatch.instances[0].history
    assert (request_type, 1, None) not in history
    assert (category_type, 1, "code") not in history
    assert history == [
        (request_type, 2, None),
        (category_type, 3, "browsing"),
    ]

    lines = output_path.read_text(encoding="utf-8").splitlines(keepends=True)
    rows = [json.loads(line) for line in lines]
    assert rows[0]["_solstone_processing"]["state"] == "analyzed"
    assert "attempts" not in rows[0]["_solstone_processing"]
    assert [row["frame_id"] for row in rows[1:]] == [1, 2, 3]
    assert lines[1] == clean_line
    assert rows[2]["frame_id"] == 2
    assert rows[2]["analysis"]["primary"] == "code"
    assert rows[2]["enhanced"] is False
    assert rows[3]["analysis"] == {
        "primary": "code",
        "secondary": "browsing",
        "overlap": False,
    }
    assert rows[3]["content"] == {
        "code": "old code",
        "browsing": "browsing text",
    }
    assert "error" not in rows[3]


@pytest.mark.asyncio
async def test_incremental_all_gap_frames_failed_preserves_reused_rows(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    frames = [_frame(1, 0.0, frame_bytes), _frame(2, 1.25, frame_bytes)]
    processor = _fingerprinted_processor(video_path, frames, monkeypatch)
    request_type = describe_module.RequestType.DESCRIBE.value
    clean_line = (
        '{"frame_id":1,"timestamp":0.0,"requests":[],"analysis":{"primary":"code",'
        '"secondary":"none","overlap":true},"enhanced":false}\n'
    )
    failed_line = (
        json.dumps(
            {
                "frame_id": 2,
                "timestamp": 1.25,
                "requests": [{"type": request_type, "model": "old", "duration": 0.01}],
                "error": "old failure",
                "enhanced": False,
            }
        )
        + "\n"
    )
    _write_existing_output(
        output_path,
        _existing_header(video_path, qualified_count=2),
        [clean_line, failed_line],
    )
    _install_merge_fakes(
        monkeypatch,
        {
            (request_type, 2, None): {
                "fail": True,
                "error": "still broken",
            }
        },
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
        previous_attempts=1,
        incremental_source_path=output_path,
    )

    lines = output_path.read_text(encoding="utf-8").splitlines(keepends=True)
    rows = [json.loads(line) for line in lines]
    assert rows[0]["_solstone_processing"]["state"] == "failed"
    assert rows[0]["_solstone_processing"]["attempts"] == 2
    assert [row["frame_id"] for row in rows[1:]] == [1, 2]
    assert lines[1] == clean_line
    assert rows[2]["error"] == "still broken"
    assert MergeBatch.instances[0].history == [(request_type, 2, None)]


@pytest.mark.parametrize(
    "case",
    [
        "first_hash_mismatch",
        "last_hash_mismatch",
        "qualified_count_mismatch",
        "input_size_mismatch",
        "current_first_hash_none",
        "row_parse_error",
        "row_id_outside_fresh_winnow",
    ],
)
@pytest.mark.asyncio
async def test_incremental_gate_failure_falls_back_to_full_describe(
    tmp_path, monkeypatch, case
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    first_hash = None if case == "current_first_hash_none" else 0x1111
    processor = _fingerprinted_processor(
        video_path,
        [_frame(1, 0.0, frame_bytes)],
        monkeypatch,
        first_hash=first_hash,
    )
    request_type = describe_module.RequestType.DESCRIBE.value
    header = _existing_header(video_path)
    if case == "first_hash_mismatch":
        header["first_hash"] = "0000000000009999"
    elif case == "last_hash_mismatch":
        header["last_hash"] = "0000000000009999"
    elif case == "qualified_count_mismatch":
        header["qualified_count"] = 2
    elif case == "input_size_mismatch":
        header["_solstone_processing"]["input_size"] = video_path.stat().st_size + 1

    old_row = (
        '{"frame_id":1,"timestamp":0.0,"requests":[],"analysis":{"primary":"code",'
        '"secondary":"none","overlap":true},"enhanced":false,"old":true}\n'
    )
    if case == "row_parse_error":
        row_lines = ["not json\n"]
    elif case == "row_id_outside_fresh_winnow":
        row_lines = [
            '{"frame_id":99,"timestamp":0.0,"requests":[],"analysis":'
            '{"primary":"code","secondary":"none","overlap":true},'
            '"enhanced":false}\n'
        ]
    else:
        row_lines = [old_row]
    _write_existing_output(output_path, header, row_lines)
    _install_merge_fakes(monkeypatch)
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
        previous_attempts=1,
        incremental_source_path=output_path,
    )

    rows = _jsonl_rows(output_path)
    assert MergeBatch.instances[0].history == [(request_type, 1, None)]
    assert rows[1]["frame_id"] == 1
    assert "old" not in rows[1]


@pytest.mark.asyncio
async def test_incremental_gate_mismatch_preserves_attempt_counter_on_fallback(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    request_type = describe_module.RequestType.DESCRIBE.value
    _write_existing_output(
        output_path,
        _existing_header(video_path, first_hash=0xAAAA, attempts=None),
        [
            '{"frame_id":1,"timestamp":0.0,"requests":[],"analysis":'
            '{"primary":"code","secondary":"none","overlap":true},'
            '"enhanced":false}\n'
        ],
    )
    _install_merge_fakes(
        monkeypatch,
        {
            (request_type, 1, None): {
                "fail": True,
                "error": "still broken",
            }
        },
    )

    attempts = []
    for current_hash in (0x1001, 0x1002, 0x1003):
        processor = _fingerprinted_processor(
            video_path,
            [_frame(1, 0.0, frame_bytes)],
            monkeypatch,
            first_hash=current_hash,
            last_hash=0x2222,
        )
        record = processing_record_module.read_processing_record_header(output_path)
        with pytest.raises(RuntimeError):
            await processor.process_with_vision(
                max_concurrent=1,
                output_path=output_path,
                work_key="20250101/143022_300/screen",
                previous_attempts=processing_record_module.record_attempts(record),
                incremental_source_path=output_path,
            )
        attempts.append(
            processing_record_module.read_processing_record_header(output_path)[
                "attempts"
            ]
        )

    assert attempts == [1, 2, 3]


@pytest.mark.parametrize("fallback", [False, True])
@pytest.mark.asyncio
async def test_incremental_reentry_carries_prior_observer(
    tmp_path, monkeypatch, fallback
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _fingerprinted_processor(
        video_path,
        [_frame(1, 0.0, frame_bytes)],
        monkeypatch,
    )
    header = _existing_header(
        video_path,
        first_hash=0x9999 if fallback else 0x1111,
        observer="desk",
    )
    _write_existing_output(
        output_path,
        header,
        [
            '{"frame_id":1,"timestamp":0.0,"requests":[],"analysis":'
            '{"primary":"code","secondary":"none","overlap":true},'
            '"enhanced":false}\n'
        ],
    )
    _install_merge_fakes(monkeypatch)
    monkeypatch.delenv("OBSERVER_NAME", raising=False)
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
        previous_attempts=1,
        incremental_source_path=output_path,
    )

    rows = _jsonl_rows(output_path)
    assert rows[0]["observer"] == "desk"


@pytest.mark.asyncio
async def test_mid_phase3_provider_deselect_flushes_failed_row(tmp_path, monkeypatch):
    from solstone.think import models

    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(video_path, [_frame(1, 0.0, frame_bytes)], monkeypatch)
    _install_fakes(
        monkeypatch,
        {
            1: {
                "response": json.dumps(
                    {"primary": "code", "secondary": "none", "overlap": True}
                )
            }
        },
    )
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [1],
    )
    provider_results = iter(
        [
            ("google", "gemini-test"),
            (models.NO_BRAIN_PROVIDER, ""),
        ]
    )
    monkeypatch.setattr(
        models,
        "resolve_provider",
        lambda _interface: next(provider_results),
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    rows = _jsonl_rows(output_path)
    assert rows[0]["_solstone_processing"]["state"] == "failed"
    assert rows[0]["_solstone_processing"]["reason_code"] == "analysis_failed"
    assert len(rows) == 2
    result = rows[1]
    assert result["frame_id"] == 1
    assert result["enhanced"] is True
    assert result["content"] == {}
    assert result["error"] == "Extraction never completed"
    assert "pending" not in result
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_partial_failure_describe_sidecar_is_not_terminal_processing_proof(
    tmp_path, monkeypatch
):
    from solstone.apps.observer.processing_proof import has_terminal_processing_proof

    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path,
        [
            _frame(1, 0.0, frame_bytes),
            _frame(2, 1.25, frame_bytes),
        ],
        monkeypatch,
    )
    _install_fakes(
        monkeypatch,
        {
            1: {},
            2: {"fail": True, "error": "boom"},
        },
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

    assert has_terminal_processing_proof(video_path, video_path.stat().st_size) is False

    clean_video_path = _video_path(tmp_path / "clean")
    clean_output_path = clean_video_path.with_suffix(".jsonl")
    clean_processor = _processor(
        clean_video_path,
        [_frame(1, 0.0, frame_bytes)],
        monkeypatch,
    )
    _install_fakes(monkeypatch, {1: {}})
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    await clean_processor.process_with_vision(
        max_concurrent=1,
        output_path=clean_output_path,
        work_key="20250101/143022_300/screen",
    )

    assert (
        has_terminal_processing_proof(clean_video_path, clean_video_path.stat().st_size)
        is True
    )
    _assert_no_describe_temp(output_path.parent)
    _assert_no_describe_temp(clean_output_path.parent)


@pytest.mark.asyncio
async def test_browsing_truncation_does_not_promote_category_content(
    tmp_path, monkeypatch
):
    from solstone.think import batch as batch_module
    from solstone.think import models

    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path,
        [_frame(1, 0.0, frame_bytes)],
        monkeypatch,
    )
    completed = []
    real_batch = batch_module.Batch

    class SpyBatch(real_batch):
        async def drain_batch(self):
            async for request in super().drain_batch():
                completed.append(
                    {
                        "request_type": getattr(request, "request_type", None),
                        "reason_code": getattr(request, "reason_code", None),
                        "retry_count": getattr(request, "retry_count", None),
                        "extraction_category": getattr(
                            request, "extraction_category", None
                        ),
                    }
                )
                yield request

    categorize = _generate_result(
        json.dumps(
            {
                "visual_description": "A browser page is open.",
                "primary": "browsing",
                "secondary": "none",
                "overlap": True,
            }
        )
    )
    truncated = _generate_result("partial browsing notes", "max_tokens")
    agenerate = AsyncMock(return_value=truncated)
    agenerate.side_effect = [categorize, *[truncated for _ in range(5)]]

    monkeypatch.setattr(batch_module, "Batch", SpyBatch)
    monkeypatch.setattr(batch_module, "agenerate_with_result", agenerate)
    monkeypatch.setattr(
        batch_module,
        "resolve_provider",
        lambda _interface: ("google", "gemini-test"),
    )
    monkeypatch.setattr(
        models,
        "resolve_provider",
        lambda _interface: ("google", "gemini-test"),
    )
    monkeypatch.setattr(
        processing_record_module, "now_iso_utc", lambda: "2026-06-30T12:00:00Z"
    )
    monkeypatch.setattr(describe_module, "callosum_send", lambda *a, **k: None)
    monkeypatch.setattr(describe_module, "get_config", lambda: {"describe": {}})
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
    result = rows[1]
    category_requests = [
        request
        for request in completed
        if request["request_type"] == describe_module.RequestType.CATEGORY
    ]

    assert agenerate.call_count == 6
    assert len(category_requests) == 5
    assert category_requests[-1]["reason_code"] == "incomplete_text_length"
    assert result["enhanced"] is True
    assert result["content"] == {}
    assert "browsing" not in result["content"]
    assert result["error"] == "Text response incomplete (reason: max_tokens)"
    assert result["requests"][-1]["category"] == "browsing"
    assert result["requests"][-1]["retries"] == 4


@pytest.mark.asyncio
async def test_detection_blocks_attach_to_media_and_social_frames(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path,
        [
            _frame(1, 0.0, frame_bytes),
            _frame(2, 1.25, frame_bytes),
        ],
        monkeypatch,
    )
    canned = _canned_detection()
    calls = []
    _install_fakes(
        monkeypatch,
        {
            1: {
                "response": json.dumps(
                    {"primary": "media", "secondary": "none", "overlap": True}
                )
            },
            2: {
                "response": json.dumps(
                    {"primary": "code", "secondary": "social", "overlap": True}
                )
            },
        },
    )
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    def fake_detect(image_bytes):
        calls.append(image_bytes)
        return canned

    monkeypatch.setattr(describe_module, "detect_objects", fake_detect)

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    frame1, frame2 = _jsonl_rows(output_path)[1:]
    assert frame1["detections"] == {
        "engine": "rf-detr.cpp",
        "engine_ref": "65c0ffcc",
        "model": "rfdetr-nano-f16",
        "threshold": 0.25,
        "source": "screen",
        "gate": "primary:media",
        "image": canned["image"],
        "objects": canned["detections"],
    }
    assert frame2["detections"] == {
        "engine": "rf-detr.cpp",
        "engine_ref": "65c0ffcc",
        "model": "rfdetr-nano-f16",
        "threshold": 0.25,
        "source": "screen",
        "gate": "secondary:social",
        "image": canned["image"],
        "objects": canned["detections"],
    }
    assert calls == [frame_bytes, frame_bytes]


@pytest.mark.asyncio
async def test_detection_gate_off_never_invokes_detector(tmp_path, monkeypatch):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path,
        [
            _frame(1, 0.0, frame_bytes),
            _frame(2, 1.25, frame_bytes),
            _frame(3, 2.5, frame_bytes),
        ],
        monkeypatch,
    )
    calls = 0
    _install_fakes(
        monkeypatch,
        {
            1: {
                "response": json.dumps(
                    {"primary": "code", "secondary": "none", "overlap": True}
                )
            },
            2: {
                "response": json.dumps(
                    {"primary": "terminal", "secondary": "none", "overlap": True}
                )
            },
            3: {
                "response": json.dumps(
                    {"primary": "browsing", "secondary": "none", "overlap": True}
                )
            },
        },
    )
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    def fake_detect(_image_bytes):
        nonlocal calls
        calls += 1
        return _canned_detection()

    monkeypatch.setattr(describe_module, "detect_objects", fake_detect)

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    rows = _jsonl_rows(output_path)[1:]
    assert all("detections" not in row for row in rows)
    assert calls == 0


@pytest.mark.asyncio
async def test_detection_skips_categorization_failed_frame(tmp_path, monkeypatch):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    gated_frame_bytes = _png_bytes((8, 8))
    failed_frame_bytes = _png_bytes((10, 10))
    processor = _processor(
        video_path,
        [
            _frame(1, 0.0, gated_frame_bytes),
            _frame(2, 1.25, failed_frame_bytes),
        ],
        monkeypatch,
    )
    calls = []
    _install_fakes(
        monkeypatch,
        {
            1: {
                "response": json.dumps(
                    {"primary": "media", "secondary": "none", "overlap": True}
                )
            },
            2: {"fail": True, "error": "boom"},
        },
    )
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    def fake_detect(image_bytes):
        calls.append(image_bytes)
        return _canned_detection()

    monkeypatch.setattr(describe_module, "detect_objects", fake_detect)

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    frame1, frame2 = _jsonl_rows(output_path)[1:]
    assert "detections" in frame1
    assert "detections" not in frame2
    assert calls == [gated_frame_bytes]


@pytest.mark.asyncio
async def test_detection_provider_absence_latches_across_gated_frames(
    tmp_path, monkeypatch, caplog
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path,
        [
            _frame(1, 0.0, frame_bytes),
            _frame(2, 1.25, frame_bytes),
            _frame(3, 2.5, frame_bytes),
        ],
        monkeypatch,
    )
    calls = 0
    monkeypatch.setattr(detect_module, "_disabled", False)
    monkeypatch.setattr(describe_module, "detect_objects", detect_module.detect_objects)

    def fake_paths():
        nonlocal calls
        calls += 1
        return RfdetrPaths(status="not_installed")

    monkeypatch.setattr(detect_module, "rfdetr_paths", fake_paths)
    caplog.set_level(logging.WARNING, logger=detect_module.LOG.name)
    _install_fakes(
        monkeypatch,
        {
            1: {
                "response": json.dumps(
                    {"primary": "media", "secondary": "none", "overlap": True}
                )
            },
            2: {
                "response": json.dumps(
                    {"primary": "social", "secondary": "none", "overlap": True}
                )
            },
            3: {
                "response": json.dumps(
                    {"primary": "code", "secondary": "social", "overlap": True}
                )
            },
        },
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

    rows = _jsonl_rows(output_path)[1:]
    warnings = [
        record
        for record in caplog.records
        if record.name == detect_module.LOG.name and record.levelno == logging.WARNING
    ]
    assert all("detections" not in row for row in rows)
    assert len(warnings) == 1
    assert warnings[0].getMessage() == (
        "object detection disabled: rf-detr provider not_installed"
    )
    assert calls == 1


@pytest.mark.asyncio
async def test_detection_empty_result_stores_empty_objects(tmp_path, monkeypatch):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes()
    processor = _processor(
        video_path,
        [_frame(1, 0.0, frame_bytes)],
        monkeypatch,
    )
    _install_fakes(
        monkeypatch,
        {
            1: {
                "response": json.dumps(
                    {"primary": "media", "secondary": "none", "overlap": True}
                )
            },
        },
    )
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )
    monkeypatch.setattr(
        describe_module,
        "detect_objects",
        lambda _image_bytes: {"image": {"width": 8, "height": 8}, "detections": []},
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    frame = _jsonl_rows(output_path)[1]
    assert frame["detections"]["objects"] == []


@pytest.mark.asyncio
async def test_detection_uses_full_resolution_frame_bytes(tmp_path, monkeypatch):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    frame_bytes = _png_bytes((2100, 20))
    processor = _processor(
        video_path,
        [_frame(1, 0.0, frame_bytes)],
        monkeypatch,
    )
    observed_sizes = []
    _install_fakes(
        monkeypatch,
        {
            1: {
                "response": json.dumps(
                    {"primary": "media", "secondary": "none", "overlap": True}
                )
            },
        },
    )
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *_args, **_kwargs: [],
    )

    def fake_detect(image_bytes):
        with Image.open(io.BytesIO(image_bytes)) as img:
            observed_sizes.append(img.size)
        return _canned_detection()

    monkeypatch.setattr(describe_module, "detect_objects", fake_detect)

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    assert observed_sizes == [(2100, 20)]
    assert observed_sizes[0][0] > 1920


@pytest.mark.asyncio
async def test_empty_run_promotes_header_only_file_for_event_precondition(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    processor = _processor(video_path, [], monkeypatch)
    _install_fakes(monkeypatch, {})
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

    assert output_path.exists()
    # async_main's completion event branch is unchanged and gated on this exists().
    assert output_path.read_text() == (
        json.dumps(
            {
                "raw": video_path.name,
                "first_hash": None,
                "last_hash": None,
                "qualified_count": 0,
                "_solstone_processing": {
                    "schema": "solstone.processing.v1",
                    "state": "empty",
                    "reason_code": "no_decodable_frames",
                    "handler": "describe",
                    "attempted_at": "2026-06-30T12:00:00Z",
                    "input_size": 5,
                },
            }
        )
        + "\n"
    )
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_all_frames_failed_promotes_header_only_then_raises(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    processor = _processor(video_path, [_frame(1, 0.0, _png_bytes())], monkeypatch)
    _install_fakes(monkeypatch, {1: {"fail": True, "error": "boom"}})

    with pytest.raises(RuntimeError):
        await processor.process_with_vision(
            max_concurrent=1,
            output_path=output_path,
            work_key="20250101/143022_300/screen",
        )

    assert output_path.exists()
    assert output_path.read_text() == (
        json.dumps(
            {
                "raw": video_path.name,
                "first_hash": None,
                "last_hash": None,
                "qualified_count": 1,
                "_solstone_thinking": {"provider": "google", "model": "gemini-test"},
                "_solstone_processing": {
                    "schema": "solstone.processing.v1",
                    "state": "failed",
                    "reason_code": "analysis_failed",
                    "handler": "describe",
                    "attempted_at": "2026-06-30T12:00:00Z",
                    "input_size": 5,
                    "attempts": 1,
                },
            }
        )
        + "\n"
    )
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_failed_promote_increments_previous_attempts(tmp_path, monkeypatch):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    processor = _processor(video_path, [_frame(1, 0.0, _png_bytes())], monkeypatch)
    _install_fakes(monkeypatch, {1: {"fail": True, "error": "boom"}})

    with pytest.raises(RuntimeError):
        await processor.process_with_vision(
            max_concurrent=1,
            output_path=output_path,
            work_key="20250101/143022_300/screen",
            previous_attempts=processing_record_module.FAILED_ATTEMPT_BOUND - 1,
        )

    record = json.loads(output_path.read_text(encoding="utf-8").splitlines()[0])[
        "_solstone_processing"
    ]
    assert record["attempts"] == processing_record_module.FAILED_ATTEMPT_BOUND


@pytest.mark.parametrize(
    ("case", "record", "expected_constructed", "expected_previous_attempts"),
    [
        (
            "analyzed",
            {
                "state": processing_record_module.STATE_ANALYZED,
                "handler": processing_record_module.HANDLER_DESCRIBE,
            },
            False,
            None,
        ),
        (
            "empty",
            {
                "state": processing_record_module.STATE_EMPTY,
                "handler": processing_record_module.HANDLER_DESCRIBE,
            },
            False,
            None,
        ),
        ("no_record", None, True, 0),
        (
            "corrupt_input",
            {
                "state": processing_record_module.STATE_FAILED,
                "handler": processing_record_module.HANDLER_DESCRIBE,
                "reason_code": processing_record_module.REASON_CORRUPT_INPUT,
            },
            False,
            None,
        ),
        (
            "exhausted_attempts",
            {
                "state": processing_record_module.STATE_FAILED,
                "handler": processing_record_module.HANDLER_DESCRIBE,
                "reason_code": processing_record_module.REASON_ANALYSIS_FAILED,
                "attempts": processing_record_module.FAILED_ATTEMPT_BOUND,
            },
            False,
            None,
        ),
        (
            "failed_transcribe",
            {
                "state": processing_record_module.STATE_FAILED,
                "handler": processing_record_module.HANDLER_TRANSCRIBE,
                "reason_code": processing_record_module.REASON_ANALYSIS_FAILED,
                "attempts": 1,
            },
            False,
            None,
        ),
        (
            "retryable_legacy_failed",
            {
                "state": processing_record_module.STATE_FAILED,
                "handler": processing_record_module.HANDLER_DESCRIBE,
                "reason_code": processing_record_module.REASON_ANALYSIS_FAILED,
            },
            True,
            0,
        ),
        (
            "retryable_failed_with_attempts",
            {
                "state": processing_record_module.STATE_FAILED,
                "handler": processing_record_module.HANDLER_DESCRIBE,
                "reason_code": processing_record_module.REASON_ANALYSIS_FAILED,
                "attempts": processing_record_module.FAILED_ATTEMPT_BOUND - 1,
            },
            True,
            processing_record_module.FAILED_ATTEMPT_BOUND - 1,
        ),
    ],
)
@pytest.mark.asyncio
async def test_existing_output_reenters_only_retryable_describe_failures(
    tmp_path,
    monkeypatch,
    case,
    record,
    expected_constructed,
    expected_previous_attempts,
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    header = {"raw": video_path.name, "case": case}
    if record is not None:
        header["_solstone_processing"] = record
    output_path.write_text(json.dumps(header) + "\n", encoding="utf-8")
    original = output_path.read_bytes()
    constructed = []
    observed_previous_attempts = []

    class FakeProcessor:
        def __init__(self, path):
            constructed.append(path)

        async def process_with_vision(self, **kwargs):
            observed_previous_attempts.append(kwargs["previous_attempts"])

    monkeypatch.setattr(describe_module, "VideoProcessor", FakeProcessor)
    monkeypatch.setattr(describe_module, "require_solstone", lambda: None)
    monkeypatch.setattr(describe_module, "callosum_send", lambda *a, **k: None)
    monkeypatch.setattr("sys.argv", ["journal describe", str(video_path)])

    await describe_module.async_main()

    if expected_constructed:
        assert constructed == [video_path]
        assert observed_previous_attempts == [expected_previous_attempts]
    else:
        assert constructed == []
        assert observed_previous_attempts == []
        assert output_path.read_bytes() == original


@pytest.mark.asyncio
async def test_redo_existing_output_does_not_read_previous_record(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    output_path.write_text(
        json.dumps(
            {
                "raw": video_path.name,
                "_solstone_processing": {
                    "state": processing_record_module.STATE_FAILED,
                    "handler": processing_record_module.HANDLER_DESCRIBE,
                    "attempts": processing_record_module.FAILED_ATTEMPT_BOUND,
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )
    observed_previous_attempts = []

    class FakeProcessor:
        def __init__(self, _path):
            pass

        async def process_with_vision(self, **kwargs):
            observed_previous_attempts.append(kwargs["previous_attempts"])

    def fail_if_read(_path):
        raise AssertionError("--redo must not read the previous record")

    monkeypatch.setattr(describe_module, "VideoProcessor", FakeProcessor)
    monkeypatch.setattr(describe_module, "read_processing_record_header", fail_if_read)
    monkeypatch.setattr(describe_module, "require_solstone", lambda: None)
    monkeypatch.setattr(describe_module, "callosum_send", lambda *a, **k: None)
    monkeypatch.setattr("sys.argv", ["journal describe", "--redo", str(video_path)])

    await describe_module.async_main()

    assert observed_previous_attempts == [0]


@pytest.mark.asyncio
async def test_unexpected_mid_job_exception_removes_temp_without_promoting(
    tmp_path, monkeypatch
):
    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    processor = _processor(video_path, [_frame(1, 0.0, _png_bytes())], monkeypatch)
    _install_fakes(monkeypatch, {1: {}})

    def raise_inject(*_args, **_kwargs):
        raise ValueError("inject")

    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        raise_inject,
    )

    with pytest.raises(ValueError):
        await processor.process_with_vision(
            max_concurrent=1,
            output_path=output_path,
            work_key="20250101/143022_300/screen",
        )

    assert not output_path.exists()
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_provider_blocked_promotes_nothing_and_records_nothing(
    tmp_path, monkeypatch
):
    import solstone.convey.provider_readiness as provider_readiness
    from solstone.observe.exit_codes import EXIT_PROVIDER_BLOCKED

    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    processor = _processor(video_path, [_frame(1, 0.0, _png_bytes())], monkeypatch)
    _install_fakes(
        monkeypatch,
        {1: {"fail": True, "error": "blocked", "reason_code": "rate_limited"}},
    )
    monkeypatch.setattr(provider_readiness, "is_blocking_reason", lambda _code: True)
    monkeypatch.setattr(provider_readiness, "present_for_reason", lambda *a, **k: {})
    monkeypatch.setattr(
        describe_module, "_emit_blocked_notification", lambda _view: None
    )

    with pytest.raises(SystemExit) as exc:
        await processor.process_with_vision(
            max_concurrent=1,
            output_path=output_path,
            work_key="20250101/143022_300/screen",
        )

    assert exc.value.code == EXIT_PROVIDER_BLOCKED
    assert not output_path.exists()
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_provider_blocked_abort_preserves_existing_attempts(
    tmp_path, monkeypatch
):
    import solstone.convey.provider_readiness as provider_readiness
    from solstone.observe.exit_codes import EXIT_PROVIDER_BLOCKED

    video_path = _video_path(tmp_path)
    output_path = video_path.with_suffix(".jsonl")
    existing_header = {
        "raw": video_path.name,
        "_solstone_processing": {
            "state": processing_record_module.STATE_FAILED,
            "reason_code": processing_record_module.REASON_ANALYSIS_FAILED,
            "handler": processing_record_module.HANDLER_DESCRIBE,
            "attempts": 2,
        },
    }
    output_path.write_text(json.dumps(existing_header) + "\n", encoding="utf-8")
    original = output_path.read_text(encoding="utf-8")
    processor = _processor(video_path, [_frame(1, 0.0, _png_bytes())], monkeypatch)
    _install_fakes(
        monkeypatch,
        {1: {"fail": True, "error": "blocked", "reason_code": "rate_limited"}},
    )
    monkeypatch.setattr(provider_readiness, "is_blocking_reason", lambda _code: True)
    monkeypatch.setattr(provider_readiness, "present_for_reason", lambda *a, **k: {})
    monkeypatch.setattr(
        describe_module, "_emit_blocked_notification", lambda _view: None
    )

    with pytest.raises(SystemExit) as exc:
        await processor.process_with_vision(
            max_concurrent=1,
            output_path=output_path,
            work_key="20250101/143022_300/screen",
            previous_attempts=2,
        )

    assert exc.value.code == EXIT_PROVIDER_BLOCKED
    assert output_path.read_text(encoding="utf-8") == original
    assert (
        json.loads(output_path.read_text(encoding="utf-8").splitlines()[0])[
            "_solstone_processing"
        ]["attempts"]
        == 2
    )
    _assert_no_describe_temp(output_path.parent)


@pytest.mark.asyncio
async def test_corrupt_input_records_failed_distinct_from_empty(tmp_path, monkeypatch):
    pytest.importorskip("av")

    seg = tmp_path / "chronicle" / "20250101" / "default" / "143022_300"
    seg.mkdir(parents=True)
    bad = seg / "screen.mp4"
    bad.write_bytes(b"not a real mp4 file at all")
    output_path = bad.with_suffix(".jsonl")
    processor = describe_module.VideoProcessor(bad)
    _install_fakes(monkeypatch, {})
    monkeypatch.setattr(
        describe_module,
        "select_frames_for_extraction",
        lambda *a, **k: [],
    )

    await processor.process_with_vision(
        max_concurrent=1,
        output_path=output_path,
        work_key="20250101/143022_300/screen",
    )

    corrupt_meta = json.loads(output_path.read_text().splitlines()[0])[
        "_solstone_processing"
    ]
    assert (corrupt_meta["state"], corrupt_meta["reason_code"]) == (
        "failed",
        "corrupt_input",
    )

    empty_video = _video_path(tmp_path / "empty")
    empty_output_path = empty_video.with_suffix(".jsonl")
    empty_processor = _processor(empty_video, [], monkeypatch)

    await empty_processor.process_with_vision(
        max_concurrent=1,
        output_path=empty_output_path,
        work_key="20250101/143022_300/screen",
    )

    empty_meta = json.loads(empty_output_path.read_text().splitlines()[0])[
        "_solstone_processing"
    ]
    assert (empty_meta["state"], empty_meta["reason_code"]) == (
        "empty",
        "no_decodable_frames",
    )
    assert (corrupt_meta["state"], corrupt_meta["reason_code"]) != (
        empty_meta["state"],
        empty_meta["reason_code"],
    )
