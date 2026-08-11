# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import importlib
import json
import sys
import traceback
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think import core_handshake
from solstone.think.models import (
    DEFAULT_MODEL_BY_PROVIDER,
    LOCAL_MODEL,
    get_model_provider,
)
from solstone.think.providers.artifact_proof import ReadinessOutcome
from solstone.think.responsiveness import NON_RESPONSIVE_REASON_CODE
from solstone.think.talents import TalentHookError


@pytest.fixture(autouse=True)
def _isolate_local_admission(monkeypatch, tmp_path):
    from solstone.think.providers import local_admission

    monkeypatch.setattr(
        local_admission,
        "_admission_dir",
        lambda: tmp_path / "local-inference-admission",
    )
    monkeypatch.setattr(local_admission, "record_local_inference", lambda _record: None)


@pytest.fixture(autouse=True)
def _default_endpoint_models_unknown(monkeypatch):
    import httpx

    from solstone.think.providers import local_endpoint

    local_endpoint.reset_endpoint_served_window_cache()

    def fake_get(url, **_kwargs):
        return httpx.Response(
            404,
            request=httpx.Request("GET", url),
            text="not found",
        )

    monkeypatch.setattr(httpx, "get", fake_get)
    yield
    local_endpoint.reset_endpoint_served_window_cache()


def _local_connect_outcome(
    outcome: str = "ready",
    *,
    port: int = 4321,
    served_model_id: str = LOCAL_MODEL,
    parallel_slots: int = 1,
    capacity_source: str = "default",
    profile: str = "floor",
    reason: str = "test outcome",
) -> dict:
    if outcome != "ready":
        return {"outcome": outcome, "reason": reason}
    return {
        "outcome": "ready",
        "server": {
            "model_id": LOCAL_MODEL,
            "served_model_id": served_model_id,
            "port": port,
            "base_url": f"http://127.0.0.1:{port}",
            "parallel_slots": parallel_slots,
            "capacity_source": capacity_source,
            "profile": profile,
        },
    }


@pytest.fixture(autouse=True)
def _local_core_connect(monkeypatch):
    from solstone.think.providers import local_server

    real = local_server._core_connect_outcome

    def inject(*outcomes: dict) -> list[list[str]]:
        calls: list[list[str]] = []
        pending = iter(outcomes)
        last = outcomes[-1]

        def runner(argv: list[str], **_kwargs) -> SimpleNamespace:
            calls.append(argv)
            return SimpleNamespace(
                stdout=json.dumps(next(pending, last)),
                stderr="",
                returncode=0,
            )

        monkeypatch.setattr(
            local_server,
            "_core_connect_outcome",
            lambda: real(
                handshake_checker=lambda: core_handshake.CoreHandshakeResult("ok"),
                helper_locator=lambda: Path("/tmp/bin/solstone-core"),
                runner=runner,
            ),
        )
        return calls

    inject(_local_connect_outcome())
    return inject


def _provider():
    providers_pkg = importlib.import_module("solstone.think.providers")
    if hasattr(providers_pkg, "local_budget"):
        delattr(providers_pkg, "local_budget")
    sys.modules.pop("solstone.think.providers.local_budget", None)
    return importlib.reload(importlib.import_module("solstone.think.providers.local"))


def _schema_keyword_paths(schema, keywords):
    found = []

    def walk(node, path="$"):
        if isinstance(node, dict):
            for key, value in node.items():
                child_path = f"{path}/{key}"
                if key in keywords:
                    found.append(child_path)
                walk(value, child_path)
        elif isinstance(node, list):
            for index, item in enumerate(node):
                walk(item, f"{path}[{index}]")

    walk(schema)
    return found


def _local_response(finish_reason):
    return {
        "choices": [
            {
                "message": {"content": "ok"},
                "finish_reason": finish_reason,
            }
        ]
    }


def _request_json_payload(url, body):
    import httpx

    request = httpx.Request("POST", url, json=body)
    decoded = request.content.decode("utf-8")
    return json.loads(decoded), decoded, request.content


def _has_surrogate_codepoint(text: str) -> bool:
    return any(0xD800 <= ord(char) <= 0xDFFF for char in text)


def _assert_no_surrogate_codepoint(text: str) -> None:
    assert not _has_surrogate_codepoint(text)


class _ChatResponse:
    def __init__(self, text: str = "hello") -> None:
        self.text = text

    def raise_for_status(self):
        return None

    def json(self):
        return {
            "choices": [
                {
                    "message": {"content": self.text},
                    "finish_reason": "stop",
                }
            ],
        }


_FAKE_MODELS_BODY = (
    '{"object":"list","data":[{"id":"Qwen/Qwen3.5-4B","object":"model",'
    '"created":1784825047,"owned_by":"sglang","root":"Qwen/Qwen3.5-4B",'
    '"parent":null,"max_model_len":16384}]}'
)
_FAKE_F1_COMPLETION_OVERFLOW_BODY = (
    '{"object":"error","message":"Requested token count exceeds the model\'s '
    "maximum context length of 16384 tokens. You requested a total of 16397 "
    "tokens: 13 tokens from the input messages and 16384 tokens for the "
    "completion. Please reduce the number of tokens in the input messages or "
    'the completion to fit within the limit.","type":"BadRequestError",'
    '"param":null,"code":400}'
)
_FAKE_F2_PROMPT_OVERFLOW_BODY = (
    '{"object":"error","message":"The input (18010 tokens) is longer than '
    'the model\'s context length (16384 tokens).","type":"BadRequestError",'
    '"param":null,"code":400}'
)
_FAKE_REJECT_WINDOW = 16384


class _FakeRejectEndpoint:
    def __init__(
        self,
        *,
        prompt_tokens: int = 13,
        models_body: str = _FAKE_MODELS_BODY,
        completion_overflow_body: str = _FAKE_F1_COMPLETION_OVERFLOW_BODY,
        prompt_overflow_body: str = _FAKE_F2_PROMPT_OVERFLOW_BODY,
        force_completion_overflow: bool = False,
        force_prompt_overflow: bool = False,
        force_non_context_400: bool = False,
        force_second_context_400: bool = False,
    ) -> None:
        self.prompt_tokens = prompt_tokens
        self.models_body = models_body
        self.completion_overflow_body = completion_overflow_body
        self.prompt_overflow_body = prompt_overflow_body
        self.force_completion_overflow = force_completion_overflow
        self.force_prompt_overflow = force_prompt_overflow
        self.force_non_context_400 = force_non_context_400
        self.force_second_context_400 = force_second_context_400
        self.gets: list[dict] = []
        self.posts: list[dict] = []

    @property
    def max_tokens(self) -> list[int]:
        return [int(post["json"]["max_tokens"]) for post in self.posts]

    def install(self, monkeypatch) -> None:
        import httpx

        monkeypatch.setattr(httpx, "get", self.get)
        monkeypatch.setattr(httpx, "post", self.post)

        fake_endpoint = self

        class AsyncClient:
            async def __aenter__(self):
                return self

            async def __aexit__(self, *_args):
                return None

            async def post(self, url, **kwargs):
                return fake_endpoint.post(url, **kwargs)

        monkeypatch.setattr(httpx, "AsyncClient", AsyncClient)

    def get(self, url, **kwargs):
        import httpx

        self.gets.append({"url": url, **kwargs})
        request = httpx.Request("GET", url)
        if str(url).endswith("/v1/models"):
            return httpx.Response(200, request=request, text=self.models_body)
        return httpx.Response(404, request=request, text="not found")

    def post(self, url, **kwargs):
        import httpx

        request = httpx.Request("POST", url)
        if str(url).endswith("/tokenize"):
            content = str((kwargs.get("json") or {}).get("content") or "")
            return httpx.Response(
                200,
                request=request,
                json={"tokens": list(range(max(1, len(content) // 3)))},
            )
        if not str(url).endswith("/v1/chat/completions"):
            raise AssertionError(f"unexpected local provider URL: {url}")

        body = kwargs["json"]
        self.posts.append({"url": url, **kwargs})
        if self.force_non_context_400:
            return httpx.Response(400, request=request, text="invalid temperature")
        if self.force_prompt_overflow or self.prompt_tokens >= _FAKE_REJECT_WINDOW:
            return httpx.Response(
                400,
                request=request,
                text=self.prompt_overflow_body,
            )
        if (
            self.force_completion_overflow
            or self.force_second_context_400
            and len(self.posts) > 1
            or self.prompt_tokens + int(body["max_tokens"]) > _FAKE_REJECT_WINDOW
        ):
            return httpx.Response(
                400,
                request=request,
                text=self.completion_overflow_body,
            )
        return httpx.Response(
            200,
            request=request,
            json={
                "choices": [
                    {
                        "message": {"content": "ok"},
                        "finish_reason": "stop",
                    }
                ],
                "usage": {
                    "prompt_tokens": self.prompt_tokens,
                    "completion_tokens": int(body["max_tokens"]),
                    "total_tokens": self.prompt_tokens + int(body["max_tokens"]),
                },
            },
        )


def _http_status_error(body: str):
    import httpx

    request = httpx.Request("POST", "http://byo.example/openai/v1/chat/completions")
    response = httpx.Response(400, request=request, text=body)
    return httpx.HTTPStatusError("bad request", request=request, response=response)


def test_local_model_prefix_maps_to_provider():
    assert get_model_provider(LOCAL_MODEL) == "local"


def test_local_model_specs():
    provider = _provider()

    assert set(provider.LOCAL_MODEL_SPECS) == {LOCAL_MODEL}
    spec = provider.LOCAL_MODEL_SPECS[LOCAL_MODEL]
    assert spec.repo == "unsloth/Qwen3.5-4B-GGUF"
    assert spec.filename == "Qwen3.5-4B-Q4_K_M.gguf"
    assert (
        spec.sha256
        == "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4"
    )
    assert spec.size_bytes == 2740937888
    assert spec.min_ram_bytes == 8 * 1024**3
    assert spec.mmproj_filename == "mmproj-F16.gguf"
    assert (
        spec.mmproj_sha256
        == "cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864"
    )
    assert spec.mmproj_size_bytes == 672423616


def test_local_provider_defaults_and_registry():
    from solstone.think.providers import PROVIDER_METADATA, PROVIDER_REGISTRY

    assert DEFAULT_MODEL_BY_PROVIDER["local"] == LOCAL_MODEL
    assert PROVIDER_REGISTRY["local"] == "solstone.think.providers.local"
    assert PROVIDER_METADATA["local"] == {
        "label": "Local (on-device)",
        "env_key": "",
    }


def test_list_models_returns_specs():
    models = _provider().list_models("local")

    assert [model["model"] for model in models] == [LOCAL_MODEL]
    assert models[0]["min_ram_bytes"] == 8 * 1024**3


def _byo_endpoint(
    credential: str | None = "test-token-PLACEHOLDER",
    parallel_slots: int | None = None,
):
    from solstone.think.providers.local_endpoint import (
        LocalEndpoint,
        normalize_local_endpoint_url,
    )

    return LocalEndpoint(
        base_url=normalize_local_endpoint_url("http://byo.example/openai/v1/"),
        served_model_id="served-model",
        credential=credential,
        is_bundled=False,
        parallel_slots=parallel_slots,
    )


def _qwen_byo_endpoint():
    from solstone.think.providers.local_endpoint import (
        LocalEndpoint,
        normalize_local_endpoint_url,
    )

    return LocalEndpoint(
        base_url=normalize_local_endpoint_url("http://byo.example/openai/v1/"),
        served_model_id="Qwen/Qwen3.5-4B",
        credential="test-token-PLACEHOLDER",
        is_bundled=False,
    )


def _bundled_endpoint():
    from solstone.think.providers.local_endpoint import LocalEndpoint

    return LocalEndpoint("", "", None, is_bundled=True)


def _patch_bundled_server(
    monkeypatch,
    *,
    window: int | None = None,
    slots: int | None = None,
    profile: str | None = None,
    served_model_id: str = LOCAL_MODEL,
):
    from solstone.think import utils
    from solstone.think.providers import local_server

    window = local_server.LOCAL_MIN_CONTEXT_TOKENS if window is None else window
    if slots is None:
        slots = local_server._slots_from_launched_tier(window) or 1
    if profile is None:
        profile = "capable" if slots == 2 else "floor"
    local_server.reset_parallel_slots_cache()

    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: SimpleNamespace(
            port=4321,
            base_url="http://127.0.0.1:4321",
            served_model_id=served_model_id,
        ),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.read_server_capacity",
        lambda: local_server.ServerCapacity(slots, "test", profile),
    )
    monkeypatch.setattr(utils, "read_service_port", lambda service: 4321)
    monkeypatch.setattr(
        local_server,
        "read_server_context_props",
        lambda port: local_server.ServerContextProps(
            n_ctx=window * slots,
            total_slots=slots,
        ),
    )


def test_cogitate_local_admission_timeout_reason_code_escapes(monkeypatch):
    provider = _provider()
    _patch_bundled_server(monkeypatch)

    from solstone.think.providers import local_admission

    async def raise_timeout(*_args, **_kwargs):
        raise local_admission.LocalAdmissionTimeout("busy")

    monkeypatch.setattr(local_admission, "acquire_local_slot_async", raise_timeout)

    with pytest.raises(local_admission.LocalAdmissionTimeout) as cogitate_exc:
        asyncio.run(provider.run_cogitate({"model": LOCAL_MODEL}))
    assert cogitate_exc.value.reason_code == "local_queue_timeout"


def test_run_cogitate_byo_acquires_permit_and_records_no_telemetry(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    from solstone.think.providers import local_admission

    records = []
    monkeypatch.setattr(local_admission, "record_local_inference", records.append)

    async def fake_cogitate(*_args, **_kwargs):
        with pytest.raises(local_admission.LocalAdmissionTimeout):
            local_admission.acquire_local_slot(1, 0.03)
        return "ok"

    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fake_cogitate,
    )

    result = asyncio.run(
        provider.run_cogitate({"model": LOCAL_MODEL, "timeout_seconds": 1})
    )

    assert result == "ok"
    assert records == []
    with local_admission.acquire_local_slot(1, 0.1) as permit:
        assert permit.slot_index == 0


def test_run_cogitate_local_delegated_non_responsive_single_event(
    monkeypatch,
):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    terminal_event = {
        "event": "error",
        "error": "non-responsive output",
        "reason_code": NON_RESPONSIVE_REASON_CODE,
        "provider": "local",
        "terminal": True,
        "raw": [{"reason_code": NON_RESPONSIVE_REASON_CODE}],
    }

    async def fake_cogitate(*_args, on_event=None, **_kwargs):
        on_event(terminal_event)
        return None

    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fake_cogitate,
    )
    events: list[dict] = []

    result = asyncio.run(
        provider.run_cogitate(
            {"model": LOCAL_MODEL, "timeout_seconds": 1},
            on_event=events.append,
        )
    )

    assert result is None
    assert events == [terminal_event]


def test_run_cogitate_byo_keeps_permit_for_non_sol_work(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    from solstone.think.providers import local_admission

    async def fake_cogitate(*_args, **_kwargs):
        with pytest.raises(local_admission.LocalAdmissionTimeout):
            local_admission.acquire_local_slot(1, 0.03)
        return "ok"

    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fake_cogitate,
    )

    assert (
        asyncio.run(provider.run_cogitate({"model": LOCAL_MODEL, "timeout_seconds": 1}))
        == "ok"
    )


@pytest.mark.parametrize("bundled", [False, True])
def test_run_cogitate_keeps_admission_held_for_native_client(
    monkeypatch,
    bundled,
):
    provider = _provider()
    if bundled:
        _patch_bundled_server(monkeypatch)
    else:
        monkeypatch.setattr(
            provider,
            "resolve_local_endpoint",
            lambda: _byo_endpoint(parallel_slots=1),
        )
        monkeypatch.setattr(
            "solstone.think.providers.local_server.connect",
            lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
        )

    from solstone.think.providers import local_admission

    async def fake_cogitate(*_args, **_kwargs):
        with pytest.raises(local_admission.LocalAdmissionTimeout):
            local_admission.acquire_local_slot(1, 0.03)
        return "ok"

    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fake_cogitate,
    )

    assert (
        asyncio.run(provider.run_cogitate({"model": LOCAL_MODEL, "timeout_seconds": 1}))
        == "ok"
    )
    assert not list(Path(local_admission._admission_dir()).glob("wait-*.ticket"))


def test_run_cogitate_byo_queue_timeout_preserves_exact_type(monkeypatch):
    provider = _provider()
    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(parallel_slots=1),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    async def fail_if_called(*_args, **_kwargs):
        raise AssertionError("native client must not run after queue timeout")

    from solstone.think.providers import local_admission

    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fail_if_called,
    )

    holder = local_admission.acquire_local_slot(1, 0.1)
    try:
        with pytest.raises(local_admission.LocalAdmissionTimeout) as exc:
            asyncio.run(
                provider.run_cogitate({"model": LOCAL_MODEL, "timeout_seconds": 0.03})
            )
        assert exc.type is local_admission.LocalAdmissionTimeout
        assert exc.value.reason_code == "local_queue_timeout"
    finally:
        holder.release()


def test_run_cogitate_confidential_with_stray_slots_skips_admission(monkeypatch):
    provider = _provider()
    configured_endpoint = "https://spp.example.test"
    config = {
        "providers": {
            "local": {
                "endpoint_url": configured_endpoint,
                "served_model_id": "confidential-model",
                "credential": "confidential-token",
                "parallel_slots": 1,
            }
        },
        "services": {"confidential": {"account_id": "acct"}},
    }
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    from solstone.think.providers import local_admission, local_endpoint

    async def fail_acquire(*_args, **_kwargs):
        raise AssertionError("confidential cogitate must not acquire admission")

    async def fake_cogitate(*_args, **_kwargs):
        return "ok"

    monkeypatch.setattr(local_endpoint, "read_journal_config", lambda: config)
    monkeypatch.setattr(local_admission, "acquire_local_slot_async", fail_acquire)
    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fake_cogitate,
    )

    assert asyncio.run(provider.run_cogitate({"model": LOCAL_MODEL})) == "ok"


def test_run_cogitate_byo_classified_error_uses_fixed_copy_and_redacts(
    monkeypatch,
):
    provider = _provider()
    token = "test-token-PLACEHOLDER"
    events: list[dict] = []

    class BadRequestError(RuntimeError):
        status_code = 400

    async def fail_cogitate(*_args, **_kwargs):
        raise BadRequestError(f"bad request with {token}")

    monkeypatch.setattr(
        provider, "resolve_local_endpoint", lambda: _byo_endpoint(token)
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )
    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fail_cogitate,
    )

    with pytest.raises(provider.LocalProviderError) as exc:
        asyncio.run(
            provider.run_cogitate({"model": LOCAL_MODEL}, on_event=events.append)
        )

    assert exc.value.reason_code == "local_endpoint_contract_failed"
    from solstone.think.providers.local_endpoint import LOCAL_ENDPOINT_CONTRACT_COPY

    assert str(exc.value) == LOCAL_ENDPOINT_CONTRACT_COPY
    assert token not in str(exc.value)
    assert getattr(exc.value, "_evented") is True
    assert events[0]["error"] == LOCAL_ENDPOINT_CONTRACT_COPY
    assert events[0]["reason_code"] == "local_endpoint_contract_failed"
    assert token not in events[0]["trace"]


def test_run_cogitate_byo_context_error_event_caps_error_field(monkeypatch):
    from solstone.think.providers.shared import PROVIDER_ERROR_TEXT_CAP_CHARS

    provider = _provider()
    token = "SENTINEL-BYO-CONTEXT-CRED-219a"
    events: list[dict] = []

    class BadRequestError(RuntimeError):
        status_code = 400

        def __init__(self) -> None:
            body = _FAKE_F2_PROMPT_OVERFLOW_BODY + f" {token}" + ("x" * 6000)
            super().__init__(body)
            self.message = body
            self.body = body

    async def fail_cogitate(*_args, **_kwargs):
        raise BadRequestError()

    monkeypatch.setattr(
        provider, "resolve_local_endpoint", lambda: _byo_endpoint(token)
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )
    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fail_cogitate,
    )

    with pytest.raises(BadRequestError):
        asyncio.run(
            provider.run_cogitate({"model": LOCAL_MODEL}, on_event=events.append)
        )

    assert events[0]["reason_code"] == "context_window_exceeded"
    assert len(events[0]["error"]) <= PROVIDER_ERROR_TEXT_CAP_CHARS
    assert token not in events[0]["error"]
    assert token not in events[0]["trace"]


def test_run_cogitate_byo_connection_error_records_no_success_telemetry(
    monkeypatch,
):
    provider = _provider()
    sentinel = "SENTINEL-BYO-CRED-9f3a2b"
    events: list[dict] = []
    records: list[dict] = []

    monkeypatch.setattr(
        provider,
        "resolve_local_endpoint",
        lambda: _byo_endpoint(sentinel),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )

    from solstone.think.providers import local_admission

    monkeypatch.setattr(local_admission, "record_local_inference", records.append)

    async def fail_cogitate(*_args, **_kwargs):
        import httpx

        from solstone.think.providers.local_endpoint import redact_exception_credential

        exc = httpx.ConnectError(f"connection refused {sentinel}")
        raise redact_exception_credential(exc, sentinel)

    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fail_cogitate,
    )

    with pytest.raises(provider.LocalProviderError) as exc:
        asyncio.run(
            provider.run_cogitate({"model": LOCAL_MODEL}, on_event=events.append)
        )

    assert exc.value.reason_code == "local_endpoint_unreachable"
    from solstone.think.providers.local_endpoint import LOCAL_ENDPOINT_UNREACHABLE_COPY

    assert str(exc.value) == LOCAL_ENDPOINT_UNREACHABLE_COPY
    assert sentinel not in str(exc.value)
    serialized = "".join(
        traceback.format_exception(
            type(exc.value),
            exc.value,
            exc.value.__traceback__,
        )
    )
    assert sentinel not in serialized
    assert sentinel not in json.dumps(events)
    assert records == []


def test_run_cogitate_talent_hook_error_bypasses_local_error_event(monkeypatch):
    provider = _provider()
    events: list[dict] = []
    hook_exc = TalentHookError(
        "post",
        "broken_hook",
        "chat",
        RuntimeError("hook exploded"),
    )

    async def fail_cogitate(*_args, **_kwargs):
        raise hook_exc

    monkeypatch.setattr(provider, "resolve_local_endpoint", _byo_endpoint)
    monkeypatch.setattr(
        "solstone.think.providers.local_server.connect",
        lambda: (_ for _ in ()).throw(AssertionError("connect not expected")),
    )
    monkeypatch.setattr(
        "solstone.think.cogitate_client.run_cogitate",
        fail_cogitate,
    )

    with pytest.raises(TalentHookError) as raised:
        asyncio.run(
            provider.run_cogitate({"model": LOCAL_MODEL}, on_event=events.append)
        )

    assert raised.value is hook_exc
    assert events == []
    assert not getattr(hook_exc, "_evented", False)


def test_select_server_tier_vram_thresholds():
    from solstone.think.providers import local_server

    cases = [
        (
            0,
            local_server.ServerTier(
                name="floor",
                context_tokens=16384,
                parallel_slots=1,
                prompt_cache_mib=0,
                resident_mib=4147,
            ),
        ),
        (
            15999,
            local_server.ServerTier(
                name="floor",
                context_tokens=16384,
                parallel_slots=1,
                prompt_cache_mib=0,
                resident_mib=4147,
            ),
        ),
        (
            16000,
            local_server.ServerTier(
                name="capable",
                context_tokens=32768,
                parallel_slots=2,
                prompt_cache_mib=2048,
                resident_mib=None,
            ),
        ),
        (
            24576,
            local_server.ServerTier(
                name="capable",
                context_tokens=32768,
                parallel_slots=2,
                prompt_cache_mib=2048,
                resident_mib=None,
            ),
        ),
    ]

    for vram_mib, expected in cases:
        tier = local_server.select_server_tier(vram_mib)
        assert tier == expected
        assert tier.context_tokens >= 16384
        assert tier.context_tokens > 0
    assert local_server._FLOOR_TIER.resident_mib == 4147
    assert local_server._CAPABLE_TIER.resident_mib is None


@pytest.mark.parametrize(
    ("props", "expected"),
    [
        ({"n_ctx": 32768}, 32768),
        ({"default_generation_settings": {"n_ctx": 16384}}, 16384),
        (
            {"n_ctx": 32768, "default_generation_settings": {"n_ctx": 16384}},
            32768,
        ),
        ({}, None),
        ({"default_generation_settings": {}}, None),
        ({"n_ctx": "abc"}, None),
        ({"n_ctx": None}, None),
        # Numeric strings are acceptable because _extract_n_ctx intentionally
        # uses int() coercion on reported llama-server values.
        ({"n_ctx": "32768"}, 32768),
    ],
)
def test_extract_n_ctx_props_shapes(props, expected):
    from solstone.think.providers import local_server

    assert local_server._extract_n_ctx(props) == expected


def test_read_server_context_props_fetch_props(monkeypatch):
    import httpx

    from solstone.think.providers import local_server

    class FakeResponse:
        status_code = 200

        def __init__(self, body=None, error: Exception | None = None):
            self.body = body
            self.error = error

        def json(self):
            if self.error is not None:
                raise self.error
            return self.body

    monkeypatch.setattr(
        httpx,
        "get",
        lambda url, timeout: FakeResponse({"n_ctx": 32768, "total_slots": 2}),
    )
    assert local_server.read_server_context_props(
        2468
    ) == local_server.ServerContextProps(
        n_ctx=32768,
        total_slots=2,
    )

    monkeypatch.setattr(
        httpx,
        "get",
        lambda url, timeout: FakeResponse(error=ValueError("bad json")),
    )
    assert local_server.read_server_context_props(2468) is None

    monkeypatch.setattr(httpx, "get", lambda url, timeout: FakeResponse(["n_ctx"]))
    assert local_server.read_server_context_props(2468) is None

    def raise_get(url, timeout):
        raise RuntimeError("network down")

    monkeypatch.setattr(httpx, "get", raise_get)
    assert local_server.read_server_context_props(2468) is None


def _select_local_provider(monkeypatch) -> None:
    monkeypatch.setattr(
        "solstone.think.models.get_config",
        lambda: {
            "providers": {"active": {"provider": "local", "model": "local/qwen3.5-4b"}}
        },
    )


def _provider_local_readiness(
    *,
    binary_installed: bool = True,
    model_installed: bool = True,
    ram_sufficient: bool = True,
    gpu_available: bool = True,
    gpu_probe_ok: bool = True,
    binary_path: str = "/fake/llama-server",
) -> ReadinessOutcome:
    ready = binary_installed and model_installed
    return ReadinessOutcome(
        provider="local",
        status="ready" if ready else "missing-or-mismatched",
        reason_code="ready" if ready else "manifest_missing",
        target={"model_id": LOCAL_MODEL},
        install={
            "install_state": "idle",
            "install_error": None,
            "error_code": None,
            "attempt_id": None,
            "progress_bytes_received": None,
            "progress_bytes_total": None,
            "last_transition_at": None,
            "last_progress_at": None,
        },
        host={
            "ram_sufficient": ram_sufficient,
            "gpu_available": gpu_available,
            "gpu_probe_ok": gpu_probe_ok,
            "backend": "vulkan",
            "backend_reason": "test vulkan",
        },
        artifacts={
            "binary_installed": binary_installed,
            "model_installed": model_installed,
            "binary_path": binary_path,
            "model_path": "/tmp/model.gguf",
            "mmproj_path": None,
            "model_id": LOCAL_MODEL,
        },
        proof={
            "binary": {
                "status": "ready" if binary_installed else "missing-or-mismatched",
                "reason_code": "ready" if binary_installed else "manifest_missing",
                "cache_hit": False,
            },
            "model": {
                "status": "ready" if model_installed else "missing-or-mismatched",
                "reason_code": "ready" if model_installed else "manifest_missing",
                "cache_hit": False,
            },
        },
    )


def test_build_provider_status_local_not_selected_is_inert(monkeypatch):
    from solstone.think.providers import build_provider_status

    health_calls = []
    monkeypatch.setattr(
        "solstone.think.models.get_config",
        lambda: {
            "providers": {"active": {"provider": "google", "model": "gemini-3.5-flash"}}
        },
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: _provider_local_readiness(),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy",
        lambda: health_calls.append("health") or True,
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["selected"] is False
    assert status["configured"] is True
    assert status["generate_ready"] is False
    assert status["cogitate_ready"] is False
    assert status["issues"] == []
    assert health_calls == []


def test_build_provider_status_local_readiness(monkeypatch):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: _provider_local_readiness(),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: True
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["configured"] is True
    assert status["generate_ready"] is True
    assert status["cogitate_ready"] is True
    assert status["issues"] == []


def test_build_provider_status_local_launch_failure_adds_probe_detail_and_hint(
    monkeypatch,
):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    detail = "dyld: Library not loaded: @rpath/libllama.dylib"
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: _provider_local_readiness(),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: False
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_install.probe_binary_runnable",
        lambda _path: (False, detail),
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["issues"] == [
        f"failed to launch: {detail}",
        "run `journal install-provider local`",
    ]
    assert "server_unhealthy" not in status["issues"]


def test_build_provider_status_local_server_unhealthy_when_probe_runnable(
    monkeypatch,
):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: _provider_local_readiness(),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: False
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_install.probe_binary_runnable",
        lambda _path: (True, None),
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["issues"] == ["server_unhealthy"]


def test_build_provider_status_local_healthy_skips_probe(monkeypatch):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    calls: list[str] = []

    def probe(_path):
        calls.append(_path)
        return False, "should not run"

    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: _provider_local_readiness(),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: True
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_install.probe_binary_runnable", probe
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["issues"] == []
    assert calls == []


def test_local_provider_status_carries_install_hint_substring(monkeypatch):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: _provider_local_readiness(
            binary_installed=False,
            model_installed=False,
            ram_sufficient=False,
        ),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: False
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["configured"] is False
    assert status["generate_ready"] is False
    assert status["cogitate_ready"] is False
    assert status["issues"] == [
        "binary_missing",
        "model_missing",
        "run `journal install-provider local`",
    ]
    assert any("journal install-provider local" in issue for issue in status["issues"])


def test_local_provider_status_reports_gpu_unavailable_issue(monkeypatch):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: _provider_local_readiness(gpu_available=False),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: True
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["issues"] == ["gpu_unavailable"]


def test_build_provider_status_local_configured_ignores_ram_flag(monkeypatch):
    from solstone.think.providers import build_provider_status

    _select_local_provider(monkeypatch)
    monkeypatch.setattr(
        "solstone.think.providers.local_install.inspect_readiness",
        lambda: _provider_local_readiness(ram_sufficient=False),
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_server.is_healthy", lambda: True
    )

    status = build_provider_status(
        [{"name": "local", "label": "Local (on-device)", "env_key": ""}]
    )["local"]

    assert status["configured"] is True
    assert status["generate_ready"] is True
    assert status["cogitate_ready"] is True
    assert status["issues"] == []


def test_local_server_connect_returns_healthy_service(_local_core_connect):
    from solstone.think.providers import local_server

    calls = _local_core_connect(
        _local_connect_outcome(
            port=2468,
            served_model_id="/path/to/snapshot",
        )
    )

    info = local_server.connect()

    assert info.model_id == LOCAL_MODEL
    assert info.served_model_id == "/path/to/snapshot"
    assert info.base_url == "http://127.0.0.1:2468"
    assert info.state == local_server.STATE_READY
    assert len(calls) == 1


def test_resolve_served_model_id_returns_valid_loaded_model_verbatim():
    from solstone.think.providers import local_server

    assert (
        local_server._resolve_served_model_id({"loaded_model": "/snap/dir"})
        == "/snap/dir"
    )


def test_resolve_served_model_id_falls_back_when_loaded_model_absent():
    from solstone.think.providers import local_server

    assert local_server._resolve_served_model_id({}) == LOCAL_MODEL
    assert local_server._resolve_served_model_id(None) == LOCAL_MODEL


@pytest.mark.parametrize(
    "body",
    [
        {"loaded_model": None},
        {"loaded_model": ""},
        {"loaded_model": "   "},
        {"loaded_model": 123},
    ],
)
def test_resolve_served_model_id_rejects_invalid_loaded_model(body):
    from solstone.think.providers import local_server

    assert local_server._resolve_served_model_id(body) is None


def test_local_server_connect_missing_port_raises_named_copy(_local_core_connect):
    from solstone.think.providers import local_server

    _local_core_connect(
        _local_connect_outcome("not-ready", reason="no local service port")
    )

    with pytest.raises(local_server.LocalProviderError) as exc:
        local_server.connect()

    assert exc.value.reason_code == "local_model_not_ready"
    assert str(exc.value) == local_server.LOCAL_MODEL_NOT_READY_COPY


def test_local_server_connect_failed_health_raises_named_copy(_local_core_connect):
    from solstone.think.providers import local_server

    _local_core_connect(_local_connect_outcome("failed", reason="connection refused"))

    with pytest.raises(local_server.LocalProviderError) as exc:
        local_server.connect()

    assert exc.value.reason_code == "local_model_not_ready"
    assert str(exc.value) == local_server.LOCAL_MODEL_NOT_READY_COPY


def test_local_server_connect_loading_health_raises_named_copy(_local_core_connect):
    from solstone.think.providers import local_server

    _local_core_connect(_local_connect_outcome("loading", reason="loading model"))

    with pytest.raises(local_server.LocalProviderError) as exc:
        local_server.connect()

    assert exc.value.reason_code == "local_model_loading"
    assert str(exc.value) == local_server.LOCAL_MODEL_NOT_READY_COPY


@pytest.mark.parametrize(
    "body",
    [
        {"loaded_model": None},
        {"loaded_model": ""},
    ],
)
def test_local_server_connect_invalid_loaded_model_raises_named_copy(
    body, _local_core_connect
):
    from solstone.think.providers import local_server

    _local_core_connect(
        _local_connect_outcome(
            "not-ready",
            reason=f"health loaded_model is blank or invalid: {body!r}",
        )
    )

    with pytest.raises(local_server.LocalProviderError) as exc:
        local_server.connect()

    assert exc.value.reason_code == "local_model_not_ready"
    assert str(exc.value) == local_server.LOCAL_MODEL_NOT_READY_COPY


def test_local_server_connect_linux_health_shape_uses_logical_model(
    _local_core_connect,
):
    from solstone.think.providers import local_server

    _local_core_connect(_local_connect_outcome(port=2468))

    info = local_server.connect()

    assert info.model_id == LOCAL_MODEL
    assert info.served_model_id == LOCAL_MODEL


# --- read_server_parallel_slots ---------------------------------------------


def _local_journal(monkeypatch, tmp_path: Path) -> Path:
    from solstone.think.providers import local_server

    journal = tmp_path / "journal"
    (journal / "health").mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    local_server.reset_parallel_slots_cache()
    return journal


def test_read_server_parallel_slots_prefers_live_props(
    monkeypatch, tmp_path, _local_core_connect
):
    from solstone.think.providers import local_server

    journal = _local_journal(monkeypatch, tmp_path)
    (journal / "health" / "local.port").write_text("2468")
    _local_core_connect(
        _local_connect_outcome(
            parallel_slots=2,
            capacity_source="props",
            profile="capable",
        )
    )

    # /props is ground truth; it wins over the persisted tier.
    assert local_server.read_server_parallel_slots() == 2
    assert local_server.read_server_capacity() == local_server.ServerCapacity(
        parallel_slots=2,
        source="props",
        profile="capable",
    )


def test_read_server_parallel_slots_no_port_returns_floor(
    monkeypatch, tmp_path, _local_core_connect
):
    from solstone.think.providers import local_server

    _local_journal(monkeypatch, tmp_path)

    _local_core_connect(
        _local_connect_outcome("not-ready", reason="no local service port")
    )
    assert local_server.read_server_parallel_slots() == 1


def test_server_capacity_uses_explicit_apple_profile(
    monkeypatch, tmp_path, _local_core_connect
):
    from solstone.think.providers import local_server

    _local_journal(monkeypatch, tmp_path)
    _local_core_connect(_local_connect_outcome(profile="apple"))

    assert local_server.read_server_capacity() == local_server.ServerCapacity(
        parallel_slots=1,
        source="default",
        profile="apple",
    )


@pytest.mark.parametrize("slots", [1, 2])
def test_read_server_parallel_slots_falls_back_to_launched_tier(
    monkeypatch, tmp_path, slots, _local_core_connect
):
    from solstone.think.providers import local_server

    tier = local_server._FLOOR_TIER if slots == 1 else local_server._CAPABLE_TIER
    _local_journal(monkeypatch, tmp_path)
    _local_core_connect(
        _local_connect_outcome(
            parallel_slots=tier.parallel_slots,
            capacity_source="local_ctx",
            profile=tier.name,
        )
    )

    assert local_server.read_server_parallel_slots() == tier.parallel_slots


def test_read_server_parallel_slots_unknown_context_window_returns_floor(
    monkeypatch, tmp_path, _local_core_connect
):
    from solstone.think.providers import local_server

    _local_journal(monkeypatch, tmp_path)
    _local_core_connect(
        _local_connect_outcome("not-ready", reason="no local service port")
    )

    assert local_server.read_server_parallel_slots() == 1


@pytest.mark.parametrize("props", [{}, {"total_slots": 0}, {"total_slots": "many"}])
def test_read_server_parallel_slots_rejects_unusable_total_slots(
    monkeypatch, tmp_path, props, _local_core_connect
):
    from solstone.think.providers import local_server

    _local_journal(monkeypatch, tmp_path)
    _local_core_connect(
        _local_connect_outcome("not-ready", reason=f"unusable total_slots: {props!r}")
    )

    assert local_server.read_server_parallel_slots() == 1


def test_read_server_parallel_slots_is_memoized_and_resettable(
    monkeypatch, tmp_path, _local_core_connect
):
    from solstone.think.providers import local_server

    journal = _local_journal(monkeypatch, tmp_path)
    (journal / "health" / "local.port").write_text("2468")
    calls = _local_core_connect(
        _local_connect_outcome(
            parallel_slots=2,
            capacity_source="props",
            profile="capable",
        )
    )

    assert local_server.read_server_parallel_slots() == 2
    assert local_server.read_server_parallel_slots() == 2
    assert len(calls) == 1

    local_server.reset_parallel_slots_cache()
    assert local_server.read_server_parallel_slots() == 2
    assert len(calls) == 2
