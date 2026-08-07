# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import base64
import importlib
import inspect
import io
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

import pytest
from PIL import Image

from solstone.think.providers import PROVIDER_REGISTRY, local, openhands
from tests.openhands_fakes import install_fake_openhands


@dataclass(frozen=True)
class _VisionLane:
    chokepoint: Callable[..., Any]
    driver: str


@dataclass(frozen=True)
class _ImageCase:
    source_format: str
    representation: str
    make_part: Callable[[], Any]
    size: tuple[int, int]


_LANES = {
    "anthropic": _VisionLane(openhands._message_content, "openhands"),
    "google": _VisionLane(openhands._message_content, "openhands"),
    "local": _VisionLane(local.run_generate, "local"),
    "openai": _VisionLane(openhands._message_content, "openhands"),
}

_EXPECTED_MEDIA_TYPES = {
    "anthropic": {
        "GIF": "image/gif",
        "JPEG": "image/jpeg",
        "PNG": "image/png",
        "TIFF": "image/png",
        "WEBP": "image/webp",
    },
    "google": {
        "GIF": "image/gif",
        "JPEG": "image/jpeg",
        "PNG": "image/png",
        "TIFF": "image/png",
        "WEBP": "image/webp",
    },
    "local": {
        "GIF": "image/gif",
        "JPEG": "image/jpeg",
        "PNG": "image/png",
        "TIFF": "image/png",
        "WEBP": "image/png",
    },
    "openai": {
        "GIF": "image/gif",
        "JPEG": "image/jpeg",
        "PNG": "image/png",
        "TIFF": "image/png",
        "WEBP": "image/webp",
    },
}

_FORMAT_BY_MEDIA_TYPE = {
    "image/gif": "GIF",
    "image/jpeg": "JPEG",
    "image/png": "PNG",
    "image/webp": "WEBP",
}

_IMAGE_SIZE = (3, 2)


@pytest.fixture(autouse=True)
def _isolate_local_admission(monkeypatch: pytest.MonkeyPatch, tmp_path: Any) -> None:
    from solstone.think.providers import local_admission

    monkeypatch.setattr(
        local_admission,
        "_admission_dir",
        lambda: tmp_path / "local-inference-admission",
    )
    monkeypatch.setattr(local_admission, "record_local_inference", lambda _record: None)


def _source_bytes(source_format: str) -> bytes:
    image = Image.new("RGB", _IMAGE_SIZE, "red")
    buf = io.BytesIO()
    image.save(buf, format=source_format)
    return buf.getvalue()


def _source_pil(source_format: str) -> Image.Image:
    image = Image.open(io.BytesIO(_source_bytes(source_format)))
    image.load()
    return image


_IMAGE_CASES = [
    _ImageCase(
        source_format=source_format,
        representation=representation,
        make_part=make_part,
        size=_IMAGE_SIZE,
    )
    for source_format in ("PNG", "JPEG", "GIF", "WEBP")
    for representation, make_part in (
        ("bytes", lambda source_format=source_format: _source_bytes(source_format)),
        ("pil", lambda source_format=source_format: _source_pil(source_format)),
    )
] + [
    _ImageCase(
        source_format="TIFF",
        representation="pil",
        make_part=lambda: _source_pil("TIFF"),
        size=_IMAGE_SIZE,
    )
]


def _case_id(case: _ImageCase) -> str:
    return f"{case.representation}-{case.source_format}"


def test_provider_vision_lane_table_matches_registry() -> None:
    assert set(_LANES) == set(PROVIDER_REGISTRY)
    for name, lane in _LANES.items():
        assert inspect.getmodule(lane.chokepoint) is importlib.import_module(
            PROVIDER_REGISTRY[name]
        )


def _local_data_url(part: Any) -> str:
    return local._image_content_part(part)["image_url"]["url"]


def _openhands_data_url(monkeypatch: pytest.MonkeyPatch, part: Any) -> str:
    fake_openhands = install_fake_openhands(monkeypatch)
    blocks = openhands._message_content(["look", part])
    image_blocks = [
        block for block in blocks if isinstance(block, fake_openhands.ImageContent)
    ]
    assert len(image_blocks) == 1
    return image_blocks[0].image_urls[0]


def _data_url_payload(url: str) -> tuple[str, bytes]:
    prefix, payload = url.split(",", 1)
    media_type = prefix.removeprefix("data:").removesuffix(";base64")
    return media_type, base64.b64decode(payload)


@pytest.mark.parametrize("name", sorted(_LANES))
@pytest.mark.parametrize("image_case", _IMAGE_CASES, ids=_case_id)
def test_provider_vision_media_matrix(
    monkeypatch: pytest.MonkeyPatch,
    name: str,
    image_case: _ImageCase,
) -> None:
    part = image_case.make_part()
    lane = _LANES[name]
    if lane.driver == "local":
        url = _local_data_url(part)
    else:
        url = _openhands_data_url(monkeypatch, part)

    media_type, payload = _data_url_payload(url)
    expected_media_type = _EXPECTED_MEDIA_TYPES[name][image_case.source_format]

    assert media_type == expected_media_type
    decoded = Image.open(io.BytesIO(payload))
    decoded.load()
    assert decoded.size == image_case.size
    assert decoded.format == _FORMAT_BY_MEDIA_TYPE[expected_media_type]
