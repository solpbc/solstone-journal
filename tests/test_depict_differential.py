# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Parsed-JSON parity between Python depict and the standalone native helper."""

from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
from pathlib import Path

from PIL import Image

from solstone.observe import depict

ROOT = Path(__file__).parents[1]


def _image(root: Path, name: str) -> Path:
    segment = root / "chronicle" / "20240101" / "default" / "123456_300"
    segment.mkdir(parents=True)
    path = segment / name
    Image.new("RGB", (4, 4), "red").save(path)
    return path


def _executable(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def _native_copy(tmp_path: Path) -> Path:
    subprocess.run(["cargo", "build", "-p", "solstone-core-depict"], cwd=ROOT / "core", check=True)
    source = ROOT / "core" / "target" / "debug" / "solstone-core-depict"
    if not source.exists():
        source = ROOT / "target" / "debug" / "solstone-core-depict"
    target = tmp_path / "bin" / "solstone-core-depict"
    target.parent.mkdir()
    shutil.copy2(source, target)
    target.chmod(target.stat().st_mode | stat.S_IXUSR)
    return target


def _install_native_stubs(binary: Path, detector: Path | None = None, description: str = "A concise image description") -> None:
    directory = binary.parent
    _executable(
        directory / "solstone-core",
        f"#!/usr/bin/env python3\nimport json, sys\nassert sys.argv[1:] == ['generate', '--one-shot']\njson.load(sys.stdin)\nprint(json.dumps({{'schema':'solstone-generate-response-v2','outcome':'generated','text':{description!r},'model':'stub','usage':{{}},'finish_reason':'stop'}}))\n",
    )
    if detector is None:
        _executable(directory / "python3", "#!/bin/sh\nprintf '%s\\n' '{\"status\":\"not_installed\",\"binary_path\":null,\"model_path\":null}'\n")
    else:
        _executable(
            directory / "python3",
            f"#!/bin/sh\nprintf '%s\\n' '{{\"status\":\"installed\",\"binary_path\":{json.dumps(str(detector))},\"model_path\":\"model.bin\"}}'\n",
        )


def _rows(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]


def test_native_depict_matches_python_values_with_metadata_and_detection(tmp_path, monkeypatch):
    py_image = _image(tmp_path / "python", "photo.png")
    native_image = _image(tmp_path / "native", "photo.png")
    monkeypatch.setenv("OBSERVER_NAME", "camera")
    monkeypatch.setenv("SEGMENT_META", json.dumps({"stream": "default", "facet": "personal"}))
    canned = {"image": {"width": 4, "height": 4}, "detections": [{"class_name": "bottle", "score": 0.67}]}
    monkeypatch.setattr(depict, "generate", lambda **_: "A concise image description")
    monkeypatch.setattr(depict, "detect_objects", lambda _: canned)
    depict.run(py_image)

    detector = tmp_path / "detector"
    _executable(detector, "#!/usr/bin/env python3\nimport json, sys\nout=sys.argv[sys.argv.index('--output')+1]\njson.dump({'image':{'width':4,'height':4},'detections':[{'class_name':'bottle','score':0.67}]},open(out,'w'))\n")
    binary = _native_copy(tmp_path)
    _install_native_stubs(binary, detector)
    subprocess.run([binary, native_image], check=True, env=os.environ.copy())
    assert _rows(native_image.with_suffix(".jsonl")) == _rows(py_image.with_suffix(".jsonl"))


def test_native_and_python_agree_on_skip_redo_and_no_engine(tmp_path, monkeypatch):
    py_image = _image(tmp_path / "python", "photo.png")
    native_image = _image(tmp_path / "native", "photo.png")
    py_output, native_output = py_image.with_suffix(".jsonl"), native_image.with_suffix(".jsonl")
    py_output.write_text("old\n", encoding="utf-8")
    native_output.write_text("old\n", encoding="utf-8")
    monkeypatch.setattr(depict, "generate", lambda **_: "replacement")
    assert depict.run(py_image) is None
    binary = _native_copy(tmp_path)
    _install_native_stubs(binary)
    subprocess.run([binary, native_image], check=True)
    assert py_output.read_text() == native_output.read_text() == "old\n"
    depict.run(py_image, redo=True)
    _install_native_stubs(binary, description="replacement")
    subprocess.run([binary, native_image, "--redo"], check=True)
    assert _rows(py_output) == _rows(native_output)

    py_output.unlink(); native_output.unlink()
    monkeypatch.setattr(depict, "generate", lambda **_: (_ for _ in ()).throw(depict.NoBrainConfiguredError()))
    assert depict.run(py_image) is None
    _executable(binary.parent / "solstone-core", "#!/bin/sh\n[ \"$1\" = generate ] && [ \"$2\" = --one-shot ] || exit 2\nprintf '%s\\n' '{\"schema\":\"solstone-generate-response-v2\",\"outcome\":\"refused\",\"id\":null,\"reason\":\"no-engine-configured\",\"reason_code\":null,\"retryable\":false,\"blocking\":true,\"reset_at_ms\":null,\"provider\":\"none\",\"detail\":\"none\"}'\n")
    subprocess.run([binary, native_image], check=True)
    assert not py_output.exists() and not native_output.exists()
