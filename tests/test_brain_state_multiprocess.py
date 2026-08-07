# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from solstone.think.models import LOCAL_MODEL
from solstone.think.providers.brain_state import (
    DEFAULT_READY_EVIDENCE_TTL,
    begin_brain_refresh,
    finish_brain_refresh,
)

NOW = datetime.now(timezone.utc)


@pytest.fixture(autouse=True)
def native_brain_binary(monkeypatch: pytest.MonkeyPatch) -> None:
    from solstone.think.providers import brain_state

    binary = Path(__file__).resolve().parents[1] / "core/target/debug/solstone-core"
    assert binary.is_file()
    monkeypatch.setattr(brain_state, "_native_binary", lambda **_kwargs: binary)


def _env(journal: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["SOLSTONE_JOURNAL"] = str(journal)
    env["SOLSTONE_TEST_CORE_BINARY"] = str(
        Path(__file__).resolve().parents[1] / "core/target/debug/solstone-core"
    )
    return env


def _write_config(journal: Path) -> None:
    path = journal / "config" / "journal.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(
            {
                "providers": {"active": {"provider": "openai", "model": "gpt-5"}},
                "env": {"OPENAI_API_KEY": "secret"},
            }
        ),
        encoding="utf-8",
    )


def _write_spp_config(journal: Path) -> None:
    credential = "endpoint-secret"
    path = journal / "config" / "journal.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(
            {
                "providers": {
                    "active": {"provider": "local", "model": LOCAL_MODEL},
                    "local": {
                        "endpoint_url": "https://brain.example.test/v1",
                        "served_model_id": "served-model",
                        "credential": credential,
                    },
                },
                "services": {
                    "confidential": {
                        "enabled_at": NOW.isoformat(),
                        "account_id": "acct-a",
                        "endpoint_url": "https://brain.example.test/v1",
                        "served_model_id": "served-model",
                        "credential_created_at": NOW.isoformat(),
                        "credential_fingerprint_sha256": hashlib.sha256(
                            credential.encode("utf-8")
                        ).hexdigest(),
                        "prior_active": {
                            "provider": "google",
                            "model": "gemini-flash-latest",
                        },
                        "prior_local_endpoint": None,
                    }
                },
                "env": {},
            }
        ),
        encoding="utf-8",
    )


def _component(now: datetime) -> dict[str, str]:
    return {
        "status": "ok",
        "observed_at": now.isoformat(),
        "expires_at": (now + DEFAULT_READY_EVIDENCE_TTL).isoformat(),
    }


def _write_spp_ready_record(journal: Path) -> None:
    _write_spp_config(journal)
    permit = begin_brain_refresh(NOW, journal_path=journal)
    assert permit is not None
    finish_brain_refresh(
        permit,
        {
            "configuration": _component(NOW),
            "lane_prerequisites": _component(NOW),
            "generate": _component(NOW),
            "cogitate": _component(NOW),
        },
        NOW,
        journal_path=journal,
    )


def _wait_for_contender_status(code: str, journal: Path) -> str:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        result = subprocess.run(
            [sys.executable, "-c", code],
            cwd=Path.cwd(),
            env=_env(journal),
            capture_output=True,
            text=True,
            check=True,
        )
        status = result.stdout.strip()
        if status != "busy":
            return status
        time.sleep(0.05)
    raise AssertionError("contender remained busy after the holder died")


def test_refresh_permit_excludes_contender_and_crash_releases(tmp_path: Path) -> None:
    _write_config(tmp_path)
    ready = tmp_path / "ready"
    holder_code = f"""
import pathlib
import os
import time
from datetime import datetime, timezone
from solstone.think.providers import brain_state
brain_state._native_binary = lambda **_kwargs: pathlib.Path(os.environ["SOLSTONE_TEST_CORE_BINARY"])
from solstone.think.providers.brain_state import begin_brain_refresh
now = datetime.fromisoformat({NOW.isoformat()!r})
permit = begin_brain_refresh(now, journal_path={str(tmp_path)!r})
assert permit is not None
pathlib.Path({str(ready)!r}).write_text("ready")
while True:
    time.sleep(0.05)
"""
    contender_code = f"""
from datetime import datetime
import os
from pathlib import Path
from solstone.think.providers import brain_state
brain_state._native_binary = lambda **_kwargs: Path(os.environ["SOLSTONE_TEST_CORE_BINARY"])
from solstone.think.providers.brain_state import begin_brain_refresh
now = datetime.fromisoformat({NOW.isoformat()!r})
permit = begin_brain_refresh(now, journal_path={str(tmp_path)!r})
if permit is None:
    print("busy", flush=True)
else:
    print("free", flush=True)
    permit.release()
"""
    holder = subprocess.Popen(
        [sys.executable, "-c", holder_code],
        cwd=Path.cwd(),
        env=_env(tmp_path),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        deadline = time.monotonic() + 10
        while not ready.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        assert ready.exists()

        busy = subprocess.run(
            [sys.executable, "-c", contender_code],
            cwd=Path.cwd(),
            env=_env(tmp_path),
            capture_output=True,
            text=True,
            check=True,
        )
        assert busy.stdout.strip() == "busy"

        holder.terminate()
        stdout, stderr = holder.communicate(timeout=10)
        assert holder.returncode is not None, (stdout, stderr)

        assert _wait_for_contender_status(contender_code, tmp_path) == "free"
    finally:
        if holder.poll() is None:
            holder.terminate()
            holder.wait(timeout=5)


def test_prerequisite_renewal_permit_excludes_contender_and_crash_releases(
    tmp_path: Path,
) -> None:
    _write_spp_ready_record(tmp_path)
    ready = tmp_path / "ready"
    holder_code = f"""
import pathlib
import os
import time
from datetime import datetime
from solstone.think.providers import brain_state
brain_state._native_binary = lambda **_kwargs: pathlib.Path(os.environ["SOLSTONE_TEST_CORE_BINARY"])
from solstone.think.providers.brain_state import begin_brain_prerequisite_renewal
now = datetime.fromisoformat({(NOW + timedelta(seconds=1)).isoformat()!r})
result = begin_brain_prerequisite_renewal(now, journal_path={str(tmp_path)!r})
assert result["status"] == "started", result
pathlib.Path({str(ready)!r}).write_text("ready")
while True:
    time.sleep(0.05)
"""
    contender_code = f"""
from datetime import datetime
import os
from pathlib import Path
from solstone.think.providers import brain_state
brain_state._native_binary = lambda **_kwargs: Path(os.environ["SOLSTONE_TEST_CORE_BINARY"])
from solstone.think.providers.brain_state import begin_brain_prerequisite_renewal
now = datetime.fromisoformat({(NOW + timedelta(seconds=2)).isoformat()!r})
result = begin_brain_prerequisite_renewal(now, journal_path={str(tmp_path)!r})
print(result["status"], flush=True)
if result["status"] == "started":
    result["permit"].release()
"""
    holder = subprocess.Popen(
        [sys.executable, "-c", holder_code],
        cwd=Path.cwd(),
        env=_env(tmp_path),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        deadline = time.monotonic() + 10
        while not ready.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        assert ready.exists()

        busy = subprocess.run(
            [sys.executable, "-c", contender_code],
            cwd=Path.cwd(),
            env=_env(tmp_path),
            capture_output=True,
            text=True,
            check=True,
        )
        assert busy.stdout.strip() == "busy"

        holder.terminate()
        stdout, stderr = holder.communicate(timeout=10)
        assert holder.returncode is not None, (stdout, stderr)

        # The holder's native session sees its dead Python caller as bare EOF
        # and abandons the stale prerequisite result. It must no longer be
        # busy; reseeding ready evidence then proves renewal can acquire again.
        assert _wait_for_contender_status(contender_code, tmp_path) == "unsafe"
        _write_spp_ready_record(tmp_path)
        assert _wait_for_contender_status(contender_code, tmp_path) == "started"
    finally:
        if holder.poll() is None:
            holder.terminate()
            holder.wait(timeout=5)
