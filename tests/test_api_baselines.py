# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Verify API baselines match checked-in fixture responses."""

from __future__ import annotations

import json

import pytest
from freezegun import freeze_time

from tests._baseline_harness import (
    FROZEN_DATE,
    FROZEN_TZ_OFFSET,
    isolated_app_env,
    make_logged_in_test_client,
    prepare_isolated_journal,
)
from tests.conftest import _install_heavy_module_stubs
from tests.verify_api import (
    ENDPOINTS,
    baseline_path,
    fetch_endpoint,
    normalize_for_compare,
)

FREEZEGUN_IGNORE = [
    "_pytest",
    "librosa",
    "numba",
    "pandas",
    "pyarrow",
    "pytest",
    "scipy",
    "sentencepiece",
    "sklearn",
    "torch",
    "transformers",
]


@pytest.fixture(scope="module", autouse=True)
def _install_stubs():
    _install_heavy_module_stubs()


@pytest.fixture(scope="module", autouse=True)
def _freeze_time():
    with freeze_time(
        FROZEN_DATE,
        tz_offset=FROZEN_TZ_OFFSET,
        ignore=FREEZEGUN_IGNORE,
    ):
        yield


@pytest.fixture(scope="module")
def _baseline_journal(tmp_path_factory):
    dst = tmp_path_factory.mktemp("baseline_journal") / "journal"
    return prepare_isolated_journal(dst)


@pytest.fixture(scope="module")
def client(_baseline_journal):
    with isolated_app_env(_baseline_journal):
        yield make_logged_in_test_client(_baseline_journal)


@pytest.fixture(scope="module")
def journal_path(_baseline_journal):
    return str(_baseline_journal)


@pytest.fixture(autouse=True)
def _reapply_isolated_override(_baseline_journal, monkeypatch):
    """Re-apply the isolated journal override after conftest's per-test autouse
    (`set_test_journal_path`) flips the env var back to the in-tree fixture.

    Without this, each test runs against `tests/fixtures/journal/` — whose
    gitignored `indexer/journal.sqlite` contains populated data from live use,
    breaking both determinism and the module-scoped `isolated_app_env` harness.
    """
    journal = _baseline_journal.resolve()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))


@pytest.mark.parametrize(
    "endpoint", ENDPOINTS, ids=[endpoint["name"] for endpoint in ENDPOINTS]
)
def test_api_baseline(client, journal_path, endpoint):
    """Verify endpoint response matches stored baseline."""
    if endpoint.get("sandbox_only"):
        pytest.skip("sandbox-only baseline (differs in Flask test client)")
    path = baseline_path(endpoint)
    if not path.exists():
        pytest.skip(f"No baseline file: {path}")

    status, payload = fetch_endpoint(client, endpoint)
    assert status == endpoint["status"], (
        f"Expected status {endpoint['status']}, got {status}"
    )

    actual = normalize_for_compare(endpoint, payload, journal_path)
    expected = normalize_for_compare(
        endpoint, json.loads(path.read_text()), journal_path
    )

    assert actual == expected, (
        f"Baseline mismatch for {endpoint['app']}/{endpoint['name']}. "
        "Run 'make update-api-baselines' to update."
    )
