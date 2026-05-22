# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# AC 1: Single-turn no-tool happy path.
# AC 2: Multi-turn tool-calling loop.
# AC 3: Streaming text_delta incremental events.
# AC 4: Thinking events use explicit thought flag.
# AC 5: Read-only policy denies writes while allowing reads in the same turn.
# AC 6: Read-only policy discriminates shell commands in the same turn.
# AC 13: Quota exhaustion maps to QuotaExhaustedError with retryDelay when present.
# AC 14: Read-call budget preserves the tool_budget_exhausted RuntimeError prefix.
# AC 15: Usage metadata is emitted on finish.
# AC 16: Google provider metadata has no cogitate_cli.
# AC 17: Google provider status ignores gemini on PATH.
# AC 18: Mocked SDK chat emits a well-formed cogitate event sequence.
# AC 21: GenerateContentConfig enables thoughts, dynamic budget, and disables AFC.
# AC 23: Unknown part kinds are skipped.
# AC 24: Empty or whitespace-only thinking parts are skipped.
# AC 25: max_turns ceiling raises MaxTurnsExhausted.
# AC 26: Missing API key emits a guided error event.
# AC 27: automatic_function_calling_history is ignored.

from __future__ import annotations

import asyncio
from types import SimpleNamespace
from typing import Any

import pytest
from google.genai import errors as google_errors
from google.genai import types

from solstone.think.cogitate_policy import MAX_TURNS, MaxTurnsExhausted
from solstone.think.models import GEMINI_FLASH
from solstone.think.providers import PROVIDER_METADATA, build_provider_status, google
from solstone.think.providers.cli import QuotaExhaustedError


class FakeChat:
    def __init__(
        self,
        turns: list[list[Any] | BaseException],
        history: list[types.Content] | None = None,
    ) -> None:
        self.turns = list(turns)
        self.history = list(history or [])
        self.messages: list[Any] = []
        self.history_curated_flags: list[bool] = []

    def send_message_stream(self, message: Any) -> Any:
        self.messages.append(message)
        turn = self.turns.pop(0)
        if isinstance(turn, BaseException):
            raise turn
        return iter(turn)

    def get_history(self, curated: bool = False) -> list[types.Content]:
        self.history_curated_flags.append(curated)
        return self.history


class FakeChats:
    def __init__(self, chat: FakeChat) -> None:
        self.chat = chat
        self.created: dict[str, Any] | None = None

    def create(
        self,
        *,
        model: str,
        config: types.GenerateContentConfig | None = None,
        history: list[types.Content] | None = None,
    ) -> FakeChat:
        self.created = {
            "model": model,
            "config": config,
            "history": history,
        }
        return self.chat


class FakeClient:
    def __init__(self, chat: FakeChat) -> None:
        self.chats = FakeChats(chat)


def _part_chunk(parts: list[Any], *, usage: Any = None, afc_history: Any = None) -> Any:
    return SimpleNamespace(
        candidates=[
            SimpleNamespace(
                content=SimpleNamespace(parts=parts),
            )
        ],
        usage_metadata=usage,
        automatic_function_calling_history=afc_history,
    )


def _usage() -> Any:
    return SimpleNamespace(
        prompt_token_count=2,
        candidates_token_count=3,
        total_token_count=9,
        cached_content_token_count=1,
        thoughts_token_count=4,
    )


def _function_call(name: str, args: dict[str, Any]) -> types.Part:
    return types.Part.from_function_call(name=name, args=args)


def _base_config(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path,
    **overrides: Any,
) -> dict[str, Any]:
    journal = tmp_path / "journal"
    journal.mkdir(exist_ok=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    config: dict[str, Any] = {
        "name": "sdk-test",
        "type": "cogitate",
        "prompt": "hello",
        "model": GEMINI_FLASH,
        "day": "20260520",
        "read_scope": [str(tmp_path)],
    }
    config.update(overrides)
    return config


def _install_client(monkeypatch: pytest.MonkeyPatch, chat: FakeChat) -> FakeClient:
    client = FakeClient(chat)
    monkeypatch.setattr(google, "get_or_create_client", lambda _client=None: client)
    return client


def _run(config: dict[str, Any], events: list[dict[str, Any]]) -> str:
    return asyncio.run(google.run_cogitate(config, on_event=events.append))


def test_single_turn_no_tool_happy_path(monkeypatch, tmp_path) -> None:
    events: list[dict[str, Any]] = []
    chat = FakeChat([[_part_chunk([types.Part(text="ok")])]])
    _install_client(monkeypatch, chat)

    result = _run(_base_config(monkeypatch, tmp_path), events)

    assert result == "ok"
    assert [event["event"] for event in events] == ["text_delta", "finish"]
    assert events[0]["delta"] == "ok"
    assert events[-1]["result"] == "ok"


def test_multiturn_tool_calling_loop(monkeypatch, tmp_path) -> None:
    target = tmp_path / "note.txt"
    target.write_text("alpha\nbeta\n", encoding="utf-8")
    chat = FakeChat(
        [
            [
                _part_chunk(
                    [
                        _function_call("read_file", {"file_path": str(target)}),
                        _function_call(
                            "grep_search",
                            {"pattern": "beta", "path": str(tmp_path)},
                        ),
                    ]
                )
            ],
            [_part_chunk([types.Part(text="done")])],
        ]
    )
    _install_client(monkeypatch, chat)
    events: list[dict[str, Any]] = []

    result = _run(_base_config(monkeypatch, tmp_path), events)

    assert result == "done"
    assert [event["event"] for event in events] == [
        "tool_start",
        "tool_end",
        "tool_start",
        "tool_end",
        "text_delta",
        "finish",
    ]
    assert events[1]["result"]["content"] == "alpha\nbeta\n"
    assert "note.txt:2:beta" in events[3]["result"]["output"]
    assert len(chat.messages) == 2
    assert [part.function_response.name for part in chat.messages[1]] == [
        "read_file",
        "grep_search",
    ]


def test_streaming_text_delta_and_usage(monkeypatch, tmp_path) -> None:
    chat = FakeChat(
        [
            [
                _part_chunk([types.Part(text="hel")]),
                _part_chunk([types.Part(text="lo")], usage=_usage()),
            ]
        ]
    )
    _install_client(monkeypatch, chat)
    events: list[dict[str, Any]] = []

    result = _run(_base_config(monkeypatch, tmp_path), events)

    assert result == "hello"
    assert [event["delta"] for event in events if event["event"] == "text_delta"] == [
        "hel",
        "lo",
    ]
    finish = events[-1]
    assert finish["usage"] == {
        "input_tokens": 2,
        "output_tokens": 3,
        "total_tokens": 9,
        "cached_tokens": 1,
        "reasoning_tokens": 4,
    }


@pytest.mark.parametrize(
    ("part", "thinking_count"),
    [
        (types.Part(text="reason", thought=True), 1),
        (types.Part(text="visible", thought=False), 0),
        (types.Part(text="   ", thought=True), 0),
    ],
)
def test_thinking_requires_explicit_thought_flag(
    monkeypatch,
    tmp_path,
    part: types.Part,
    thinking_count: int,
) -> None:
    chat = FakeChat([[_part_chunk([part, types.Part(text="ok")])]])
    _install_client(monkeypatch, chat)
    events: list[dict[str, Any]] = []

    _run(_base_config(monkeypatch, tmp_path), events)

    thinking = [event for event in events if event["event"] == "thinking"]
    assert len(thinking) == thinking_count
    if thinking:
        assert thinking[0]["summary"] == "reason"
        assert thinking[0]["model"] == GEMINI_FLASH


def test_readonly_policy_denies_write_and_allows_read_same_turn(
    monkeypatch,
    tmp_path,
) -> None:
    target = tmp_path / "allowed.txt"
    target.write_text("readable", encoding="utf-8")
    chat = FakeChat(
        [
            [
                _part_chunk(
                    [
                        _function_call("write_file", {"file_path": str(target)}),
                        _function_call("read_file", {"file_path": str(target)}),
                    ]
                )
            ],
            [_part_chunk([types.Part(text="done")])],
        ]
    )
    _install_client(monkeypatch, chat)
    events: list[dict[str, Any]] = []

    _run(_base_config(monkeypatch, tmp_path), events)

    tool_results = [event["result"] for event in events if event["event"] == "tool_end"]
    assert tool_results[0]["error"].startswith("policy_deny:")
    assert tool_results[1] == {"content": "readable"}


def test_readonly_policy_discriminates_shell_commands_same_turn(
    monkeypatch,
    tmp_path,
) -> None:
    chat = FakeChat(
        [
            [
                _part_chunk(
                    [
                        _function_call(
                            "run_shell_command",
                            {"command": "rm -rf /tmp/not-allowed"},
                        ),
                        _function_call(
                            "run_shell_command",
                            {"command": "sol call journal search hello"},
                        ),
                    ]
                )
            ],
            [_part_chunk([types.Part(text="done")])],
        ]
    )
    _install_client(monkeypatch, chat)
    monkeypatch.setattr(
        google,
        "run_shell_command",
        lambda command: {"stdout": f"ran {command}", "stderr": "", "returncode": 0},
    )
    events: list[dict[str, Any]] = []

    _run(_base_config(monkeypatch, tmp_path), events)

    tool_results = [event["result"] for event in events if event["event"] == "tool_end"]
    assert tool_results[0]["error"].startswith("policy_deny:")
    assert tool_results[1] == {
        "stdout": "ran sol call journal search hello",
        "stderr": "",
        "returncode": 0,
    }


@pytest.mark.parametrize(
    ("response_json", "retry_delay_ms"),
    [
        (
            {
                "error": {
                    "code": 429,
                    "message": "quota",
                    "status": "RESOURCE_EXHAUSTED",
                    "details": [
                        {
                            "@type": "type.googleapis.com/google.rpc.RetryInfo",
                            "retryDelay": "2.5s",
                        }
                    ],
                }
            },
            2500,
        ),
        (
            {
                "error": {
                    "code": 429,
                    "message": "quota",
                    "status": "RESOURCE_EXHAUSTED",
                }
            },
            None,
        ),
    ],
)
def test_quota_exhausted_mapping(
    monkeypatch,
    tmp_path,
    response_json: dict[str, Any],
    retry_delay_ms: int | None,
) -> None:
    chat = FakeChat([google_errors.ClientError(429, response_json)])
    _install_client(monkeypatch, chat)
    events: list[dict[str, Any]] = []

    with pytest.raises(QuotaExhaustedError) as exc_info:
        _run(_base_config(monkeypatch, tmp_path), events)

    assert exc_info.value.retry_delay_ms == retry_delay_ms
    assert events == []


def test_read_call_budget_exhausted_preserves_runtimeerror_prefix(
    monkeypatch,
    tmp_path,
) -> None:
    chat = FakeChat([[_part_chunk([_function_call("read_file", {"file_path": "x"})])]])
    _install_client(monkeypatch, chat)
    events: list[dict[str, Any]] = []

    with pytest.raises(RuntimeError, match=r"^tool_budget_exhausted:"):
        _run(_base_config(monkeypatch, tmp_path, read_call_budget=0), events)

    assert events[-1]["event"] == "tool_budget_exhausted"


def test_generate_content_config_pins_thinking_and_manual_tools(
    monkeypatch,
    tmp_path,
) -> None:
    chat = FakeChat([[_part_chunk([types.Part(text="ok")])]])
    client = _install_client(monkeypatch, chat)

    _run(_base_config(monkeypatch, tmp_path), [])

    config = client.chats.created["config"]
    assert config.thinking_config.include_thoughts is True
    assert config.thinking_config.thinking_budget == -1
    assert config.automatic_function_calling.disable is True
    assert config.tool_config.function_calling_config.mode == (
        types.FunctionCallingConfigMode.AUTO
    )
    assert config.tools[0].function_declarations


def test_google_provider_metadata_has_no_cogitate_cli() -> None:
    assert "cogitate_cli" not in PROVIDER_METADATA["google"]


@pytest.mark.parametrize(
    ("api_key", "vertex_creds_configured", "ready", "issues"),
    [
        ("", False, False, ["GOOGLE_API_KEY not set"]),
        ("key", False, True, []),
        ("", True, True, []),
    ],
)
def test_google_provider_status_ignores_gemini_path(
    monkeypatch,
    api_key: str,
    vertex_creds_configured: bool,
    ready: bool,
    issues: list[str],
) -> None:
    if api_key:
        monkeypatch.setenv("GOOGLE_API_KEY", api_key)
    else:
        monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.setattr(
        "solstone.think.providers.shutil.which",
        lambda _name: (_ for _ in ()).throw(AssertionError("which should not run")),
    )

    status = build_provider_status(
        [{"name": "google", "env_key": "GOOGLE_API_KEY"}],
        vertex_creds_configured=vertex_creds_configured,
    )["google"]

    assert status == {
        "configured": ready,
        "generate_ready": ready,
        "cogitate_ready": ready,
        "issues": issues,
    }


def test_unknown_part_kind_and_afc_history_are_ignored(monkeypatch, tmp_path) -> None:
    unknown_part = SimpleNamespace(
        text=None,
        thought=False,
        function_call=None,
        executable_code=SimpleNamespace(code="print(1)"),
    )
    chat = FakeChat(
        [
            [
                _part_chunk(
                    [unknown_part, types.Part(text="ok")],
                    afc_history=[types.Content(role="model", parts=[])],
                )
            ]
        ]
    )
    _install_client(monkeypatch, chat)
    events: list[dict[str, Any]] = []

    result = _run(_base_config(monkeypatch, tmp_path), events)

    assert result == "ok"
    assert [event["event"] for event in events] == ["text_delta", "finish"]


def test_e2e_smoke_event_sequence(monkeypatch, tmp_path) -> None:
    chat = FakeChat(
        [
            [
                _part_chunk([types.Part(text="a")]),
                _part_chunk([types.Part(text="b")]),
            ]
        ]
    )
    _install_client(monkeypatch, chat)
    events: list[dict[str, Any]] = []

    _run(_base_config(monkeypatch, tmp_path), events)

    assert all("ts" in event for event in events)
    assert [event["event"] for event in events] == [
        "text_delta",
        "text_delta",
        "finish",
    ]


def test_max_turns_ceiling_raises_typed_exception(monkeypatch, tmp_path) -> None:
    chat = FakeChat(
        [[_part_chunk([_function_call("unknown_tool", {})])] for _ in range(MAX_TURNS)]
    )
    _install_client(monkeypatch, chat)
    events: list[dict[str, Any]] = []

    with pytest.raises(MaxTurnsExhausted):
        _run(_base_config(monkeypatch, tmp_path, write=True), events)

    assert events[-1]["event"] == "max_turns_exhausted"


def test_missing_api_key_emits_error_event(monkeypatch, tmp_path) -> None:
    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path / "journal"))
    monkeypatch.setattr(
        "solstone.think.utils.get_config",
        lambda: {"providers": {"google_backend": "aistudio"}},
    )
    events: list[dict[str, Any]] = []

    with pytest.raises(ValueError) as exc_info:
        _run(
            {
                "name": "sdk-test",
                "type": "cogitate",
                "prompt": "hello",
                "model": GEMINI_FLASH,
                "day": "20260520",
            },
            events,
        )

    assert "GOOGLE_API_KEY" in str(exc_info.value)
    assert getattr(exc_info.value, "_evented") is True
    assert events[-1]["event"] == "error"
    assert "GOOGLE_API_KEY" in events[-1]["error"]
