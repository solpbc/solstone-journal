# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
import sys
import types
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
    if "sklearn.metrics.pairwise" not in sys.modules:
        pairwise = types.ModuleType("pairwise")

        def cosine_similarity(a, b):
            return [[1.0]]

        pairwise.cosine_similarity = cosine_similarity
        metrics = types.ModuleType("metrics")
        metrics.pairwise = pairwise

        cluster = types.ModuleType("sklearn.cluster")

        class DummyHDBSCAN:
            def __init__(self, **k):
                pass

            def fit(self, X):
                self.labels_ = np.full(len(X), -1, dtype=int)
                return self

        cluster.HDBSCAN = DummyHDBSCAN

        sklearn = types.ModuleType("sklearn")
        sklearn.metrics = metrics
        sklearn.cluster = cluster
        sklearn.__spec__ = importlib.machinery.ModuleSpec("sklearn", loader=None)
        metrics.__spec__ = importlib.machinery.ModuleSpec(
            "sklearn.metrics", loader=None
        )
        pairwise.__spec__ = importlib.machinery.ModuleSpec(
            "sklearn.metrics.pairwise", loader=None
        )
        cluster.__spec__ = importlib.machinery.ModuleSpec(
            "sklearn.cluster", loader=None
        )
        sys.modules["sklearn"] = sklearn
        sys.modules["sklearn.metrics"] = metrics
        sys.modules["sklearn.metrics.pairwise"] = pairwise
        sys.modules["sklearn.cluster"] = cluster
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
from solstone.think.entities.journal import clear_journal_entity_cache
from solstone.think.entities.loading import clear_entity_loading_cache
from solstone.think.entities.observations import clear_observation_cache
from solstone.think.entities.relationships import clear_relationship_caches
from solstone.think.push.runtime import stop_all_push_runtime
from solstone.think.utils import now_ms
from solstone.think.voice import brain as voice_brain
from solstone.think.voice.runtime import stop_all_voice_runtime
from tests._baseline_harness import copytree_tracked


@pytest.fixture(autouse=True)
def set_test_journal_path(monkeypatch):
    """Set SOLSTONE_JOURNAL to tests/fixtures/journal for all unit tests.

    This ensures all tests have a valid SOLSTONE_JOURNAL without needing
    to explicitly set it in each test.
    """
    monkeypatch.setenv(
        "SOLSTONE_JOURNAL",
        str(Path("tests/fixtures/journal").resolve()),
    )
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")


@pytest.fixture(autouse=True)
def _clear_entity_caches():
    """Clear all entity caches before/after each test."""
    clear_entity_loading_cache()
    clear_journal_entity_cache()
    clear_relationship_caches()
    clear_observation_cache()
    yield
    clear_entity_loading_cache()
    clear_journal_entity_cache()
    clear_relationship_caches()
    clear_observation_cache()


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
def _cleanup_chat_runtime():
    yield
    stop_all_chat_runtime()


@pytest.fixture
def journal_copy(tmp_path, monkeypatch):
    """Copy git-tracked fixture files to tmp_path for mutation tests."""
    src = Path(__file__).resolve().parent / "fixtures" / "journal"
    dst = tmp_path / "journal"
    copytree_tracked(src, dst)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(dst.resolve()))
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
    if "solstone.observe.sense" not in sys.modules:
        # Import the real module - it has minimal dependencies
        sense_mod = importlib.import_module("solstone.observe.sense")
        sys.modules["solstone.observe.sense"] = sense_mod
        observe_pkg = sys.modules.get("solstone.observe")
        setattr(observe_pkg, "sense", sense_mod)
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
    google_mod = sys.modules.get("google", types.ModuleType("google"))
    genai_mod = types.ModuleType("google.genai")

    class DummyModels:
        def generate_content(self, *, model, contents, config=None):
            return types.SimpleNamespace(text="[]", candidates=[], usage_metadata=None)

    class DummyClient:
        def __init__(self, *a, **k):
            self.models = DummyModels()

    genai_mod.Client = DummyClient

    # Mock Content type for type hints
    class MockContent:
        pass

    # Mock config builders
    class MockHttpOptions:
        def __init__(self, **k):
            self.timeout = k.get("timeout")

    class MockThinkingConfig:
        def __init__(self, **k):
            self.thinking_budget = k.get("thinking_budget")

    class MockGenerateContentConfig:
        def __init__(self, **k):
            for key, value in k.items():
                setattr(self, key, value)

    class MockHttpRetryOptions:
        def __init__(self, **k):
            pass

    genai_mod.types = types.SimpleNamespace(
        GenerateContentConfig=MockGenerateContentConfig,
        Content=MockContent,
        HttpOptions=MockHttpOptions,
        HttpRetryOptions=MockHttpRetryOptions,
        ThinkingConfig=MockThinkingConfig,
    )
    google_mod.genai = genai_mod
    sys.modules["google"] = google_mod
    sys.modules["google.genai"] = genai_mod
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


@pytest.fixture(autouse=True)
def reset_supervisor_state():
    """Reset supervisor module state before/after tests to prevent cross-test pollution."""
    try:
        import solstone.think.supervisor as mod

        # Reset before test
        mod._daily_state["last_day"] = None
        mod._is_remote_mode = False
        # Create fresh task queue
        mod._task_queue = mod.TaskQueue(on_queue_change=None)
    except ImportError:
        pass  # supervisor not loaded yet
    yield
    try:
        import solstone.think.supervisor as mod

        # Reset after test
        mod._daily_state["last_day"] = None
        mod._is_remote_mode = False
        mod._observer_health = {}
        mod._enabled_observers = set()
        # Create fresh task queue
        mod._task_queue = mod.TaskQueue(on_queue_change=None)
    except ImportError:
        pass


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
    monkeypatch.setattr(
        "solstone.think.supervisor.CallosumConnection", MockCallosumConnection
    )


def setup_google_genai_stub(monkeypatch, *, with_thinking=False):
    """Set up a complete Google GenAI stub for testing.

    Args:
        monkeypatch: pytest monkeypatch fixture
        with_thinking: If True, mock responses include thinking parts

    Returns:
        The DummyChat class for inspection if needed
    """
    from types import SimpleNamespace

    google_mod = types.ModuleType("google")
    genai_mod = types.ModuleType("google.genai")
    errors_mod = types.ModuleType("google.genai.errors")

    # Error classes matching actual SDK structure
    class APIError(Exception):
        pass

    class ServerError(APIError):
        pass

    class ClientError(APIError):
        pass

    errors_mod.APIError = APIError
    errors_mod.ServerError = ServerError
    errors_mod.ClientError = ClientError

    class DummyChat:
        """Mock chat that optionally returns thinking parts."""

        kwargs = None  # Class var to capture last call for inspection

        def __init__(self, model, history=None, config=None):
            self.model = model
            self.history = list(history or [])
            self.config = config

        def get_history(self):
            return list(self.history)

        def record_history(self, content):
            self.history.append(content)

        async def send_message(self, message, config=None):
            DummyChat.kwargs = {
                "message": message,
                "config": config,
                "model": self.model,
            }
            if with_thinking:
                # Response with thinking parts matching actual SDK structure
                thinking_part = SimpleNamespace(
                    thought=True,
                    text="I need to analyze this step by step.",
                )
                answer_part = SimpleNamespace(
                    thought=False,
                    text="ok",
                )
                candidate = SimpleNamespace(
                    content=SimpleNamespace(parts=[thinking_part, answer_part]),
                )
                return SimpleNamespace(text="ok", candidates=[candidate])
            else:
                # Simple response without thinking
                return SimpleNamespace(text="ok")

    class DummyChats:
        def create(self, *, model, config=None, history=None):
            return DummyChat(model, history=history, config=config)

    class DummyModels:
        """Mock for client.models.generate_content (non-chat generate API)."""

        def generate_content(self, *, model, contents, config=None):
            return SimpleNamespace(text="[]", candidates=[], usage_metadata=None)

    class DummyClient:
        def __init__(self, *a, **k):
            self.chats = DummyChats()
            self.models = DummyModels()
            self.aio = SimpleNamespace(chats=DummyChats(), models=DummyModels())

    genai_mod.Client = DummyClient
    genai_mod.errors = errors_mod
    genai_mod.types = SimpleNamespace(
        GenerateContentConfig=lambda **k: SimpleNamespace(**k),
        ToolConfig=lambda **k: SimpleNamespace(**k),
        FunctionCallingConfig=lambda **k: SimpleNamespace(**k),
        ThinkingConfig=lambda **k: SimpleNamespace(**k),
        Content=lambda **k: SimpleNamespace(**k),
        Part=lambda **k: SimpleNamespace(**k),
        HttpOptions=lambda **k: SimpleNamespace(**k),
        HttpRetryOptions=lambda **k: SimpleNamespace(**k),
    )
    google_mod.genai = genai_mod
    monkeypatch.setitem(sys.modules, "google", google_mod)
    monkeypatch.setitem(sys.modules, "google.genai", genai_mod)
    monkeypatch.setitem(sys.modules, "google.genai.errors", errors_mod)

    return DummyChat
