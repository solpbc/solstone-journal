# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import platform
import types
from unittest.mock import Mock

import numpy as np
import pytest

if platform.system() != "Linux":
    pytest.skip("Linux-only ONNX test", allow_module_level=True)

import solstone.observe.transcribe._parakeet_onnx as parakeet_onnx


@pytest.fixture(autouse=True)
def _reset_caches():
    parakeet_onnx._ADAPTER_CACHE.clear()
    parakeet_onnx._WARNED_INT8_CUDA.clear()
    yield
    parakeet_onnx._ADAPTER_CACHE.clear()
    parakeet_onnx._WARNED_INT8_CUDA.clear()


def _install_fake_modules(
    monkeypatch: pytest.MonkeyPatch,
    *,
    providers: list[str],
    adapter: object | None = None,
    ort_version: str = "1.25.0",
):
    fake_ort = types.ModuleType("onnxruntime")
    fake_ort.__version__ = ort_version
    fake_ort.get_available_providers = lambda: providers
    fake_ort.preload_dlls = Mock()

    if adapter is None:
        adapter = Mock()

    load_model = Mock(
        return_value=types.SimpleNamespace(with_timestamps=lambda: adapter)
    )
    fake_onnx_asr = types.ModuleType("onnx_asr")
    fake_onnx_asr.load_model = load_model

    monkeypatch.setitem(__import__("sys").modules, "onnxruntime", fake_ort)
    monkeypatch.setitem(__import__("sys").modules, "onnx_asr", fake_onnx_asr)
    return fake_ort, load_model, adapter


class _FakeAdapter:
    def __init__(self, results: list[list[dict]]):
        self._results = list(results)
        self.calls = []

    def recognize(self, audio: np.ndarray, sample_rate: int) -> list[dict]:
        self.calls.append((len(audio), sample_rate))
        if not self._results:
            return []
        return self._results.pop(0)


def test_chunked_recognize_short_audio_single_call():
    audio = np.zeros(5 * 16000, dtype=np.float32)
    adapter = _FakeAdapter(
        [[{"token": "hello.", "start": 0.1, "end": 0.4, "logprob": 0.0}]]
    )

    words, audio_sec = parakeet_onnx._chunked_recognize(adapter, audio, 16000)

    assert audio_sec == 5.0
    assert len(adapter.calls) == 1
    assert words == [
        {
            "word": " hello.",
            "start": 0.1,
            "end": 0.4,
            "probability": 1.0,
        }
    ]


def test_chunked_recognize_overlap_drop_semantics():
    audio = np.zeros(60 * 16000, dtype=np.float32)
    adapter = _FakeAdapter(
        [
            [
                {"token": "one", "start": 1.0, "end": 1.2, "logprob": 0.0},
                {"token": "drop", "start": 24.5, "end": 24.8, "logprob": 0.0},
            ],
            [
                {"token": "two", "start": 0.5, "end": 0.8, "logprob": 0.0},
                {"token": "three", "start": 10.0, "end": 10.3, "logprob": 0.0},
                {"token": "drop2", "start": 24.1, "end": 24.4, "logprob": 0.0},
            ],
            [
                {"token": "four", "start": 0.1, "end": 0.4, "logprob": 0.0},
                {"token": "five", "start": 11.0, "end": 11.4, "logprob": 0.0},
            ],
        ]
    )

    words, audio_sec = parakeet_onnx._chunked_recognize(adapter, audio, 16000)

    assert audio_sec == 60.0
    assert len(adapter.calls) == 3
    assert [word["word"] for word in words] == [
        " one",
        " two",
        " three",
        " four",
        " five",
    ]
    assert [round(word["start"], 1) for word in words] == [1.0, 24.5, 34.0, 48.1, 59.0]


def test_chunked_recognize_token_at_stride_sec_dropped_nonfinal_kept_final():
    audio = np.zeros(73 * 16000, dtype=np.float32)
    adapter = _FakeAdapter(
        [
            [{"token": "drop", "start": 24.0, "end": 24.3, "logprob": 0.0}],
            [],
            [{"token": "keep", "start": 24.0, "end": 24.3, "logprob": 0.0}],
        ]
    )

    words, _audio_sec = parakeet_onnx._chunked_recognize(adapter, audio, 16000)

    assert [word["word"] for word in words] == [" keep"]
    assert [round(word["start"], 1) for word in words] == [72.0]


def test_chunked_recognize_empty_audio_returns_empty_and_zero():
    adapter = _FakeAdapter([])

    words, audio_sec = parakeet_onnx._chunked_recognize(
        adapter, np.array([], dtype=np.float32), 16000
    )

    assert words == []
    assert audio_sec == 0.0


def test_validate_config_accepts_supported_values():
    assert parakeet_onnx._validate_config(
        {
            "model_version": "v2",
            "device": "cpu",
            "timeout_sec": 42,
            "quantization": "int8",
        }
    ) == ("v2", "cpu", 42.0, "int8")


def test_validate_config_rejects_bad_model_version():
    with pytest.raises(ValueError, match="v2, v3"):
        parakeet_onnx._validate_config({"model_version": "v9"})


def test_validate_config_rejects_bad_device():
    with pytest.raises(ValueError, match="auto, cpu, cuda"):
        parakeet_onnx._validate_config({"device": "tpu"})


def test_validate_config_rejects_bad_quantization():
    with pytest.raises(ValueError, match="auto, fp32, int8"):
        parakeet_onnx._validate_config({"quantization": "bf16"})


def test_resolve_runtime_auto_resolves_to_fp32_on_cpu(monkeypatch: pytest.MonkeyPatch):
    _install_fake_modules(monkeypatch, providers=["CPUExecutionProvider"])

    resolved_device, resolved_quantization, providers = parakeet_onnx._resolve_runtime(
        "auto", "auto"
    )

    assert (resolved_device, resolved_quantization, providers) == (
        "cpu",
        "fp32",
        ["CPUExecutionProvider"],
    )


def test_resolve_runtime_auto_resolves_to_fp32_on_cuda(monkeypatch: pytest.MonkeyPatch):
    _install_fake_modules(
        monkeypatch, providers=["CUDAExecutionProvider", "CPUExecutionProvider"]
    )

    resolved_device, resolved_quantization, providers = parakeet_onnx._resolve_runtime(
        "auto", "auto"
    )

    assert (resolved_device, resolved_quantization, providers) == (
        "cuda",
        "fp32",
        ["CUDAExecutionProvider", "CPUExecutionProvider"],
    )


def test_resolve_runtime_explicit_cuda_without_provider_raises_with_remediation(
    monkeypatch: pytest.MonkeyPatch,
):
    _install_fake_modules(monkeypatch, providers=["CPUExecutionProvider"])

    with pytest.raises(RuntimeError, match="PARAKEET_ONNX_VARIANT=cuda make install"):
        parakeet_onnx._resolve_runtime("cuda", "fp32")


def test_get_adapter_caches_by_resolved_tuple_and_preloads_dlls_for_cuda(
    monkeypatch: pytest.MonkeyPatch,
):
    fake_adapter = object()
    fake_ort, load_model, _adapter = _install_fake_modules(
        monkeypatch,
        providers=["CUDAExecutionProvider", "CPUExecutionProvider"],
        adapter=fake_adapter,
    )

    first = parakeet_onnx._get_adapter("v3", "cuda", "fp32")
    second = parakeet_onnx._get_adapter("v3", "cuda", "fp32")

    assert first is fake_adapter
    assert second is fake_adapter
    fake_ort.preload_dlls.assert_called_once_with(cuda=True, cudnn=True)
    load_model.assert_called_once()

    _cpu_ort, cpu_load_model, _cpu_adapter = _install_fake_modules(
        monkeypatch,
        providers=["CPUExecutionProvider"],
        adapter=object(),
    )
    parakeet_onnx._get_adapter("v3", "cpu", "fp32")
    assert cpu_load_model.call_count == 1


def test_get_adapter_cuda_int8_warns_once(
    monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture
):
    _install_fake_modules(
        monkeypatch, providers=["CUDAExecutionProvider", "CPUExecutionProvider"]
    )

    caplog.set_level("WARNING")
    parakeet_onnx._resolve_runtime("cuda", "int8")
    parakeet_onnx._resolve_runtime("cuda", "int8")

    warnings_seen = [
        record.message
        for record in caplog.records
        if "may underperform fp32" in record.message
    ]
    assert len(warnings_seen) == 1


def test_get_model_info_shape_reports_per_word_confidence_true(
    monkeypatch: pytest.MonkeyPatch,
):
    _install_fake_modules(monkeypatch, providers=["CPUExecutionProvider"])

    info = parakeet_onnx.get_model_info({"device": "cpu"})

    assert info == {
        "model": "istupakov/parakeet-tdt-0.6b-v3-onnx",
        "device": "cpu",
        "compute_type": "fp32",
        "per_word_confidence": True,
        "onnxruntime_version": "1.25.0",
        "providers": ["CPUExecutionProvider"],
    }


def test_transcribe_rebuilds_words_with_single_leading_space_and_speaker_none(
    monkeypatch: pytest.MonkeyPatch,
):
    adapter = _FakeAdapter(
        [
            [
                {"token": "hello.", "start": 0.0, "end": 0.4, "logprob": 0.0},
                {"token": "world", "start": 0.5, "end": 0.8, "logprob": 0.0},
            ]
        ]
    )
    _install_fake_modules(
        monkeypatch, providers=["CPUExecutionProvider"], adapter=adapter
    )

    statements = parakeet_onnx.transcribe(
        np.zeros(5 * 16000, dtype=np.float32), 16000, {}
    )

    assert statements
    assert all(statement["speaker"] is None for statement in statements)
    assert all(
        word["word"].startswith(" ") and not word["word"].startswith("  ")
        for statement in statements
        for word in statement["words"]
    )
