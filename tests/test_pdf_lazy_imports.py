# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import dataclasses
import json
import subprocess
import sys

import frontmatter
import pytest

from solstone.think import features as features_module
from solstone.think.features import Feature, MissingExtraError

PROBE = """
import json
import sys
import solstone.apps.reflections.routes  # noqa: F401
import solstone.think.importers.documents  # noqa: F401
import solstone.think.importers.text  # noqa: F401
print("MODULES_JSON:" + json.dumps(sorted(sys.modules)))
"""


# A fresh interpreter is the point: it gives a pristine sys.modules unaffected by
# whatever else ran on this xdist worker, so the guard measures the real static
# import graph rather than accumulated in-process state. That import graph is
# heavyweight (~900 modules incl. numpy + PIL, plus a possible cold .pyc compile),
# so the 15s default unit-test budget is too tight on a saturated CI box and would
# intermittently trip pytest-timeout. Give it real headroom and bound the
# subprocess explicitly (90s < the 120s mark) so a genuine hang surfaces as a
# clean TimeoutExpired with captured output, not an opaque signal kill.
@pytest.mark.timeout(120)
def test_pdf_modules_are_not_loaded_by_static_imports():
    result = subprocess.run(
        [sys.executable, "-c", PROBE],
        capture_output=True,
        text=True,
        timeout=90,
    )
    assert result.returncode == 0, (
        f"probe subprocess exited {result.returncode}\n"
        f"--- stdout ---\n{result.stdout}\n--- stderr ---\n{result.stderr}"
    )
    # Read only the sentinel line, so a stray warning printed to stdout during
    # import can't corrupt the module set or break the presence assertions.
    sentinel = [
        line for line in result.stdout.splitlines() if line.startswith("MODULES_JSON:")
    ]
    assert len(sentinel) == 1, (
        f"expected exactly one MODULES_JSON line, got {len(sentinel)}\n"
        f"--- stdout ---\n{result.stdout}\n--- stderr ---\n{result.stderr}"
    )
    modules = set(json.loads(sentinel[0][len("MODULES_JSON:") :]))

    assert "solstone.apps.reflections.routes" in modules
    assert "solstone.think.importers.documents" in modules
    assert "solstone.think.importers.text" in modules
    assert "weasyprint" not in modules
    assert "pypdf" not in modules
    assert "pdf2image" not in modules


def _force_missing_pdf(monkeypatch):
    real = features_module.FEATURES["pdf"]
    fake: Feature = dataclasses.replace(
        real,
        pip_modules=("definitely_not_installed_xyz",),
    )
    monkeypatch.setitem(features_module.FEATURES, "pdf", fake)


def test_render_reflection_pdf_missing_extra(monkeypatch, tmp_path):
    _force_missing_pdf(monkeypatch)
    from solstone.apps.reflections.routes import _render_reflection_pdf

    with pytest.raises(MissingExtraError) as exc:
        _render_reflection_pdf(tmp_path / "20260308.md", frontmatter.Post("# hi"))

    assert "pip install 'solstone[pdf]'" in str(exc.value)


def test_document_importer_process_pdf_missing_extra(monkeypatch, tmp_path):
    _force_missing_pdf(monkeypatch)
    from solstone.think.importers.documents import DocumentImporter

    pdf = tmp_path / "x.pdf"
    pdf.write_bytes(b"")

    with pytest.raises(MissingExtraError) as exc:
        DocumentImporter().process(pdf, tmp_path)

    assert "pip install 'solstone[pdf]'" in str(exc.value)


def test_read_transcript_pdf_missing_extra(monkeypatch, tmp_path):
    _force_missing_pdf(monkeypatch)
    from solstone.think.importers.text import _read_transcript

    pdf = tmp_path / "x.pdf"
    pdf.write_bytes(b"")

    with pytest.raises(MissingExtraError) as exc:
        _read_transcript(str(pdf))

    assert "pip install 'solstone[pdf]'" in str(exc.value)
