# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import logging
import queue
import time

from solstone.think.importers.cli import _wait_for_segments


def _status_event(index: int) -> dict:
    return {
        "tract": "observe",
        "event": "status",
        "screen": {
            "running": {
                "file": f"chronicle/20260502/default/seg-{index}/screen.png",
                "ref": f"screen-{index}",
            },
            "max_age_seconds": 0,
        },
        "audio": {
            "queued": [
                {
                    "file": f"chronicle/20260502/default/seg-{index}/audio.wav",
                    "age_seconds": index,
                }
            ],
            "max_age_seconds": index,
        },
    }


def _observed_event(segment: str) -> dict:
    return {
        "tract": "observe",
        "event": "observed",
        "segment": segment,
        "day": "20260502",
        "stream": "default",
    }


def test_status_only_stream_stalls(caplog):
    message_queue: queue.Queue[dict] = queue.Queue()
    for index in range(5):
        message_queue.put(_status_event(index))

    pending = {"seg-a", "seg-b"}
    caplog.set_level(logging.ERROR, logger="solstone.think.importers.cli")

    started_at = time.monotonic()
    failed_segments, completed_count = _wait_for_segments(
        message_queue,
        pending,
        segment_timeout=0.15,
        poll_timeout=0.02,
    )
    elapsed = time.monotonic() - started_at

    assert elapsed < 2.0
    assert sorted(failed_segments) == ["seg-a", "seg-b"]
    assert completed_count == 0
    assert "Transcription stalled: no progress for " in caplog.text
    assert "0/2 segments completed, 2 still pending: ['seg-a', 'seg-b']" in caplog.text


def test_mixed_observed_and_status_stalls_remaining_segments(caplog):
    message_queue: queue.Queue[dict] = queue.Queue()
    message_queue.put(_observed_event("seg-a"))
    message_queue.put(_observed_event("seg-b"))
    for index in range(5):
        message_queue.put(_status_event(index))

    pending = {"seg-a", "seg-b", "seg-c", "seg-d"}
    caplog.set_level(logging.ERROR, logger="solstone.think.importers.cli")

    started_at = time.monotonic()
    failed_segments, completed_count = _wait_for_segments(
        message_queue,
        pending,
        segment_timeout=0.15,
        poll_timeout=0.02,
    )
    elapsed = time.monotonic() - started_at

    assert elapsed < 2.0
    assert completed_count == 2
    assert pending == {"seg-c", "seg-d"}
    assert sorted(failed_segments) == ["seg-c", "seg-d"]
    assert "Transcription stalled: no progress for " in caplog.text
    assert "2/4 segments completed, 2 still pending: ['seg-c', 'seg-d']" in caplog.text
