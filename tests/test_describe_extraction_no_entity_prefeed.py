# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import inspect
from pathlib import Path

from solstone.observe.describe import VideoProcessor


def test_user_contents_matches_extraction_request_shape():
    vp = VideoProcessor(Path("/nonexistent/video.mp4"))
    sentinel = object()

    contents = vp._user_contents("Analyze this code screenshot.", sentinel)

    assert contents == ["Analyze this code screenshot.", sentinel]
    assert len(contents) == 2
    assert contents[1] is sentinel


def test_user_contents_never_includes_entity_name_payload():
    vp = VideoProcessor(Path("/nonexistent/video.mp4"))
    sentinel = object()

    contents = vp._user_contents("Analyze this code screenshot.", sentinel)

    assert "frequently used names" not in contents[0]
    assert not any(
        isinstance(element, str) and "frequently used names" in element
        for element in contents
    )


def test_user_contents_has_no_entity_opt_in_seam():
    vp = VideoProcessor(Path("/nonexistent/video.mp4"))

    signature = inspect.signature(VideoProcessor._user_contents)

    assert tuple(signature.parameters) == ("self", "prompt", "image")
    assert not hasattr(vp, "entity_names")
