# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
import json
import os
import sys
import types
from collections.abc import Sequence
from pathlib import Path
from unittest.mock import Mock

import numpy as np
import pytest

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))


def _install_heavy_module_stubs():
    if "usearch.index" not in sys.modules:
        usearch = types.ModuleType("usearch")
        index_mod = types.ModuleType("usearch.index")

        class DummyIndex:
            def __init__(self, *a, **k):
                pass

            def save(self, *a, **k):
                pass

            @classmethod
            def restore(cls, *a, **k):
                return cls()

            def remove(self, *a, **k):
                pass

            def add(self, *a, **k):
                pass

            def search(self, *a, **k):
                class Res:
                    keys = [1]
                    distances = [0.0]

                return Res()

        index_mod.Index = DummyIndex
        usearch.index = index_mod
        sys.modules["usearch"] = usearch
        sys.modules["usearch.index"] = index_mod
    if "sentence_transformers" not in sys.modules:
        st_mod = types.ModuleType("sentence_transformers")

        class DummyST:
            def __init__(self, *a, **k):
                pass

            def get_sentence_embedding_dimension(self):
                return 384

            def encode(self, texts):
                if isinstance(texts, str):
                    texts = [texts]
                return [([0.0] * 384) for _ in texts]

        st_mod.SentenceTransformer = DummyST
        sys.modules["sentence_transformers"] = st_mod
    # NOTE: do NOT stub sklearn. scikit-learn is dev/test-only now, but it is
    # still genuinely installed in the dev environment. tests/speaker_oracle/
    # and the speaker discovery differential use the real implementation. A
    # persistent sys.modules stub here would leak into whichever co-scheduled
    # test imported it first under xdist. Only genuinely-absent heavy deps
    # (usearch, sentence_transformers) belong in this stub set.
    if "dotenv" not in sys.modules:
        dotenv_mod = types.ModuleType("dotenv")

        def load_dotenv(*a, **k):
            return True

        def dotenv_values(*a, **k):
            return {}

        dotenv_mod.load_dotenv = load_dotenv
        dotenv_mod.dotenv_values = dotenv_values
        sys.modules["dotenv"] = dotenv_mod


from solstone.convey.chat import stop_all_chat_runtime
from solstone.think.link.runtime import stop_all_link_runtime
from solstone.think.push.runtime import stop_all_push_runtime
from solstone.think.utils import now_ms
from solstone.think.voice import brain as voice_brain
from solstone.think.voice.runtime import stop_all_voice_runtime
from tests._baseline_harness import copytree_tracked


def write_health_approval_artifact(
    journal_root: Path,
    *,
    importers: Sequence[str],
    raw_retention_decision: str = "retain_parsed",
    unparsed_sensitive_modalities_acknowledged: bool | None = None,
) -> Path:
    from solstone.think.importers.pre_save_gate import (
        APPROVAL_SCHEMA,
        CHECKLIST_DESTINATIONS,
        CHECKLIST_VERSION,
        approval_path_for_journal,
    )

    resolved = journal_root.resolve()
    approval_path = approval_path_for_journal(resolved)
    approval_path.parent.mkdir(parents=True, exist_ok=True)
    raw_retention = {
        "decision": raw_retention_decision,
        "notes": "Synthetic test decision.",
    }
    if unparsed_sensitive_modalities_acknowledged is not None:
        raw_retention["unparsed_sensitive_modalities_acknowledged"] = (
            unparsed_sensitive_modalities_acknowledged
        )
    artifact = {
        "schema": APPROVAL_SCHEMA,
        "checklist_version": CHECKLIST_VERSION,
        "approved_by": "Test Owner",
        "approved_at": "2026-07-03T23:22:00-06:00",
        "journal_root": str(resolved),
        "approved_importers": list(importers),
        "replication_destinations": {
            destination: {
                "decision": "approved" if destination == "time_machine" else "excluded",
                "notes": "Synthetic test decision.",
            }
            for destination in CHECKLIST_DESTINATIONS
        },
        "raw_retention": raw_retention,
        "requires_per_run_confirmation": True,
        "no_real_health_data_in_artifact": True,
    }
    approval_path.write_text(json.dumps(artifact), encoding="utf-8")
    return approval_path


@pytest.fixture(autouse=True)
def _isolate_os_environ():
    """Restore os.environ after every test so raw env writes can't leak.

    Production code mutates os.environ by raw assignment (not monkeypatch): the
    supervisor sets ``SOL_SUPERVISOR_SPAWNED`` and rewrites ``PATH``
    (``supervisor.py``), talents set ``SOL_SEGMENT`` (``talents.py``), and
    settings write provider keys (``settings/routes.py``). When a test exercises
    one of those paths the change persists into every later test in the same
    worker — the root cause of the ``SOL_SUPERVISOR_SPAWNED`` flake and the
    reason ~20 test files defensively ``delenv`` it. Snapshotting here makes
    every test hermetic against that whole class of env leak, so new tests no
    longer need their own defensive ``delenv``. Defined first so it brackets the
    other autouse fixtures (notably the monkeypatch env setup below).
    """
    saved = dict(os.environ)
    try:
        yield
    finally:
        if dict(os.environ) != saved:
            os.environ.clear()
            os.environ.update(saved)


@pytest.fixture(autouse=True)
def set_test_journal_path(monkeypatch, _isolate_os_environ):
    """Set SOLSTONE_JOURNAL to tests/fixtures/journal for all unit tests.

    This ensures all tests have a valid SOLSTONE_JOURNAL without needing
    to explicitly set it in each test.
    """
    import solstone.think.utils as think_utils

    monkeypatch.setenv(
        "SOLSTONE_JOURNAL",
        str(Path("tests/fixtures/journal").resolve()),
    )
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    monkeypatch.setenv("SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES", "1")
    think_utils._journal_path_cache = None
    yield
    think_utils._journal_path_cache = None


@pytest.fixture(autouse=True)
def _native_index_read_bridge(monkeypatch: pytest.MonkeyPatch) -> None:
    """Route read-surface tests through the checked-out native query binary."""
    from solstone.think import core_handshake
    from solstone.think.indexer import journal, native

    helper = ROOT / "core" / "target" / "debug" / "solstone-core"
    if not helper.is_file():
        return

    kwargs = {
        "handshake_checker": lambda: core_handshake.CoreHandshakeResult("ok"),
        "helper_locator": lambda: helper,
        "platform_reader": lambda: ("linux", "x86_64"),
        "platform_tag_reader": lambda: {"manylinux2014_x86_64"},
    }
    monkeypatch.setattr(
        journal,
        "run_native_indexer_search",
        lambda query, journal_path, **options: native.run_native_indexer_search(
            query, journal_path, **options, **kwargs
        ),
    )
    monkeypatch.setattr(
        journal,
        "run_native_indexer_agents",
        lambda journal_path: native.run_native_indexer_agents(journal_path, **kwargs),
    )
    monkeypatch.setattr(
        journal,
        "run_native_indexer_coverage",
        lambda journal_path: native.run_native_indexer_coverage(journal_path, **kwargs),
    )


@pytest.fixture(autouse=True)
def _speakers_analyze_startup_invariant_ready(
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
    tmp_path: Path,
) -> None:
    """Keep unit tests independent of the installed native helper wheel.

    tests/test_speakers_analyze_installation.py exercises the real invariant
    directly; other unit tests patch specific failure modes when needed.
    """
    if Path(str(request.node.path)).name == "test_speakers_analyze_installation.py":
        return

    from solstone.think import speakers_analyze_installation as installation
    from tests.helpers.speakers_analyze import install_enter_generation_stub

    monkeypatch.setattr(
        installation,
        "check_speakers_analyze_installation",
        lambda **_kwargs: installation.SpeakersAnalyzeInstallationResult("ok"),
    )
    install_enter_generation_stub(monkeypatch, tmp_path)


@pytest.fixture(autouse=True)
def _default_local_backend_vulkan(monkeypatch, request):
    from solstone.think.providers import local_cuda, local_install

    if request.node.get_closest_marker("real_local_backend_probe") is None:
        monkeypatch.setattr(
            local_cuda,
            "probe_nvidia_gpu",
            lambda: local_cuda.NvidiaProbe(
                index=None,
                compute_cap=None,
                driver_cuda_version=None,
                vram_mib=None,
                tiering_memory_mib=None,
                memory_source=local_cuda.MEMORY_SOURCE_UNAVAILABLE,
                detected=False,
            ),
        )
        monkeypatch.setattr(
            local_install,
            "probe_cuda_runtime_artifact_trust",
            lambda _pin, **_kwargs: local_cuda.ArtifactTrust.ABSENT,
        )
        monkeypatch.setattr(
            local_install,
            "has_persisted_installed_cuda_target",
            lambda **_kwargs: False,
        )


@pytest.fixture(autouse=True)
def _cleanup_voice_runtime():
    yield
    stop_all_voice_runtime()
    voice_brain.clear_brain_state()


@pytest.fixture(autouse=True)
def _cleanup_push_runtime():
    yield
    stop_all_push_runtime()


@pytest.fixture(autouse=True)
def _cleanup_link_runtime():
    yield
    stop_all_link_runtime()


def _reset_chat_module_state():
    """Bring solstone.convey.chat to a clean slate between tests.

    stop_all_chat_runtime() cancels watchdog timers and clears the runtime,
    _reserved_use_ids, and the thinking buffers. The remaining module-global
    singletons are test-state only — notably _last_use_id, a monotonic id
    counter production must NOT reset (resetting it live would collide
    use_ids) — so they are cleared here in the fixture rather than in
    production stop_all_chat_runtime().
    """
    import solstone.convey.chat as chat

    stop_all_chat_runtime()
    with chat._state_lock:
        chat._current_chat_use_id = None
        chat._current_chat_state = None
        chat._queued_triggers.clear()
        chat._active_talents.clear()
        chat._last_use_id = 0


@pytest.fixture(autouse=True)
def _cleanup_chat_runtime():
    # Reset before and after every test so a sibling that mutates chat's
    # module-global singletons can't bleed into the next test, regardless of
    # whether the test called its own _reset_chat_state helper.
    _reset_chat_module_state()
    yield
    _reset_chat_module_state()


@pytest.fixture
def journal_copy(tmp_path, monkeypatch):
    """Copy git-tracked fixture files to tmp_path for mutation tests."""
    src = Path(__file__).resolve().parent / "fixtures" / "journal"
    dst = tmp_path / "journal"
    copytree_tracked(src, dst)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(dst.resolve()))
    import solstone.think.utils as think_utils

    think_utils._journal_path_cache = None
    return dst


@pytest.fixture(autouse=True)
def add_module_stubs(monkeypatch):
    _install_heavy_module_stubs()
    # Import real observe package first to avoid shadowing with stubs
    if "solstone.observe" not in sys.modules:
        importlib.import_module("solstone.observe")
    if "solstone.observe.detect" not in sys.modules:
        detect_mod = types.ModuleType("solstone.observe.detect")

        def input_detect():
            return None, None

        detect_mod.input_detect = input_detect
        sys.modules["solstone.observe.detect"] = detect_mod
        observe_pkg = sys.modules.get("solstone.observe")
        setattr(observe_pkg, "detect", detect_mod)
    if "solstone.observe.hear" not in sys.modules:
        # Import the real module for format_audio and load_transcript
        hear_mod = importlib.import_module("solstone.observe.hear")
        sys.modules["solstone.observe.hear"] = hear_mod
        observe_pkg = sys.modules.get("solstone.observe")
        setattr(observe_pkg, "hear", hear_mod)
    if "solstone.observe.utils" not in sys.modules:
        # Import the real module
        utils_mod = importlib.import_module("solstone.observe.utils")
        sys.modules["solstone.observe.utils"] = utils_mod
        observe_pkg = sys.modules.get("solstone.observe")
        setattr(observe_pkg, "utils", utils_mod)
    if "solstone.observe.screen" not in sys.modules:
        # Import the real module for format_screen
        screen_mod = importlib.import_module("solstone.observe.screen")
        sys.modules["solstone.observe.screen"] = screen_mod
        observe_pkg = sys.modules.get("solstone.observe")
        setattr(observe_pkg, "screen", screen_mod)
    if "gi" not in sys.modules:
        gi_mod = types.ModuleType("gi")
        gi_mod.require_version = lambda *a, **k: None

        class Dummy(types.ModuleType):
            pass

        repo = types.ModuleType("gi.repository")
        repo.Gdk = Dummy("Gdk")
        repo.Gtk = Dummy("Gtk")
        gi_mod.repository = repo
        sys.modules["gi"] = gi_mod
        sys.modules["gi.repository"] = repo
        sys.modules["Gdk"] = repo.Gdk
        sys.modules["Gtk"] = repo.Gtk
    if "cv2" not in sys.modules:
        cv2_mod = types.ModuleType("cv2")
        cv2_mod.__spec__ = importlib.machinery.ModuleSpec("cv2", loader=None)
        cv2_mod.COLOR_RGB2LAB = 0

        def cvtColor(arr, code):
            arr = np.asarray(arr)
            gray = arr.mean(axis=2)
            return np.stack([gray, gray, gray], axis=2)

        cv2_mod.cvtColor = cvtColor
        sys.modules["cv2"] = cv2_mod
    for name in [
        "noisereduce",
    ]:
        if name not in sys.modules:
            sys.modules[name] = types.ModuleType(name)


@pytest.fixture
def mock_callosum(monkeypatch):
    """Mock Callosum connections to capture emitted events without real I/O.

    This fixture provides a MockCallosumConnection class that:
    - Enforces the start-before-emit requirement
    - Broadcasts events to all listeners (like the real Callosum)
    - Works without real socket connections

    Usage:
        def test_example(mock_callosum):
            from solstone.think.callosum import CallosumConnection

            received = []
            listener = CallosumConnection()
            listener.start(callback=lambda msg: received.append(msg))

            # Now emit events and they'll be captured in received
    """
    all_listeners = []

    class MockCallosumConnection:
        def __init__(self, socket_path=None):
            self.socket_path = socket_path
            self.callback = None
            self.thread = None

        def start(self, callback=None):
            """Simulate starting the background thread."""
            self.callback = callback
            self.thread = Mock()
            self.thread.is_alive.return_value = True
            if callback:
                all_listeners.append(self)

        def emit(self, tract, event, **kwargs):
            """Emit event and broadcast to all listeners."""
            # Return False if not started yet (matches real behavior)
            if self.thread is None or not self.thread.is_alive():
                return False

            # Build message
            msg = {"tract": tract, "event": event, **kwargs}
            if "ts" not in msg:
                msg["ts"] = now_ms()

            # Broadcast to all listeners
            for listener in all_listeners:
                if listener.callback:
                    listener.callback(msg)

            return True

        def stop(self):
            """Stop connection and remove from listeners."""
            if self in all_listeners:
                all_listeners.remove(self)
            self.thread = None
            self.callback = None

    # Patch both import locations
    monkeypatch.setattr(
        "solstone.think.runner.CallosumConnection", MockCallosumConnection
    )
    monkeypatch.setattr(
        "solstone.think.callosum.CallosumConnection", MockCallosumConnection
    )


# Convey fixtures, folded in from the retired solstone/convey/tests/conftest.py
# (2026-07-24) when convey's dark test modules moved into this directory.
@pytest.fixture
def convey_env(tmp_path, monkeypatch):
    def _create():
        journal = tmp_path / "journal"
        journal.mkdir()

        config_dir = journal / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_file = config_dir / "journal.json"
        config_file.write_text(
            json.dumps(
                {
                    "setup": {"completed_at": 1700000000000},
                },
                indent=2,
            )
        )

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

        from solstone.convey import create_app

        app = create_app(journal=str(journal))
        client = app.test_client()

        class Env:
            def __init__(self):
                self.journal = journal
                self.client = client
                self.app = app

        return Env()

    return _create


@pytest.fixture
def convey_env_setup_pending(tmp_path, monkeypatch):
    def _create():
        journal = tmp_path / "journal"
        journal.mkdir()

        config_dir = journal / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_file = config_dir / "journal.json"
        config_file.write_text(
            json.dumps(
                {
                    "setup": {},
                },
                indent=2,
            )
        )

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

        from solstone.convey import create_app

        app = create_app(journal=str(journal))
        client = app.test_client()

        class Env:
            def __init__(self):
                self.journal = journal
                self.client = client
                self.app = app

        return Env()

    return _create
