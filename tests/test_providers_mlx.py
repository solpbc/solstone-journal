# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import asyncio
import importlib
import sys
import types
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest
from PIL import Image


def _provider(monkeypatch):
    import solstone.think.providers.mlx as provider

    monkeypatch.setattr(provider, "_module_level_cache", {})
    return provider


def _install_mlx_stub(monkeypatch, *, load_exc=None, text="ok"):
    mlx_module = types.ModuleType("mlx_vlm")
    mlx_module.__path__ = []
    model = SimpleNamespace(config=SimpleNamespace(model_type="qwen3_5"))
    processor = SimpleNamespace(tokenizer=object())
    load_mock = MagicMock(side_effect=load_exc)
    if load_exc is None:
        load_mock = MagicMock(return_value=(model, processor))
    template_mock = MagicMock(return_value="templated")
    result = SimpleNamespace(
        text=text,
        prompt_tokens=7,
        generation_tokens=3,
        total_tokens=10,
    )
    generate_mock = MagicMock(return_value=result)
    mlx_module.load = load_mock
    mlx_module.apply_chat_template = template_mock
    mlx_module.generate = generate_mock

    structured_module = types.ModuleType("mlx_vlm.structured")
    logits_processor = object()
    build_schema_mock = MagicMock(return_value=logits_processor)
    structured_module.build_json_schema_logits_processor = build_schema_mock

    monkeypatch.setitem(sys.modules, "mlx_vlm", mlx_module)
    monkeypatch.setitem(sys.modules, "mlx_vlm.structured", structured_module)
    return SimpleNamespace(
        module=mlx_module,
        model=model,
        processor=processor,
        load=load_mock,
        template=template_mock,
        generate=generate_mock,
        result=result,
        structured=structured_module,
        build_schema=build_schema_mock,
        logits_processor=logits_processor,
    )


def _gemma4_stubs(*, optional_attrs: bool = True):
    image_processor = SimpleNamespace(max_soft_tokens=0)
    processor = SimpleNamespace(tokenizer=object(), image_processor=image_processor)
    pooler = SimpleNamespace(default_output_length=0)
    vision_tower = SimpleNamespace(
        pooling_kernel_size=3,
        max_patches=0,
        default_output_length=0,
        pooler=pooler,
    )
    if optional_attrs:
        image_processor.image_seq_length = 0
        processor.image_seq_length = 0
    else:
        delattr(vision_tower, "pooler")
    config = SimpleNamespace(vision_config=SimpleNamespace(position_embedding_size=10240))
    model = SimpleNamespace(vision_tower=vision_tower, config=config)
    return model, processor


def test_registration():
    from solstone.think.providers import (
        PROVIDER_METADATA,
        PROVIDER_REGISTRY,
        get_provider_module,
    )

    assert "mlx" in PROVIDER_REGISTRY
    assert PROVIDER_METADATA["mlx"] == {
        "label": "MLX (Local, Apple Silicon)",
        "env_key": "",
    }
    assert get_provider_module("mlx").__name__ == "solstone.think.providers.mlx"


def test_module_import_is_mlx_vlm_free(monkeypatch):
    monkeypatch.setitem(sys.modules, "mlx_vlm", None)
    import solstone.think.providers.mlx as provider

    provider = importlib.reload(provider)

    assert not hasattr(provider, "mlx_vlm")


@pytest.mark.parametrize(
    ("system", "machine", "ram_gb", "mlx_present", "expected"),
    [
        ("Linux", "arm64", 32, True, (False, "not running on macOS")),
        ("Darwin", "x86_64", 32, True, (False, "not running on Apple Silicon")),
        (
            "Darwin",
            "arm64",
            8,
            True,
            (False, "insufficient RAM (need 16 GB, have 8 GB)"),
        ),
        ("Darwin", "arm64", 32, False, (False, "mlx-vlm package not installed")),
        ("Darwin", "arm64", 32, True, (True, "")),
    ],
)
def test_is_mlx_available_parameterized(
    monkeypatch, system, machine, ram_gb, mlx_present, expected
):
    provider = _provider(monkeypatch)
    monkeypatch.setattr(provider.platform, "system", lambda: system)
    monkeypatch.setattr(provider.platform, "machine", lambda: machine)
    monkeypatch.setattr(
        provider.psutil,
        "virtual_memory",
        lambda: SimpleNamespace(total=ram_gb * 1024**3),
    )
    if mlx_present:
        monkeypatch.setitem(sys.modules, "mlx_vlm", types.ModuleType("mlx_vlm"))
    else:
        monkeypatch.setitem(sys.modules, "mlx_vlm", None)

    assert provider.is_mlx_available() == expected


@pytest.mark.parametrize("image_count", [1, 2])
def test_image_actually_reaches_mlx_vlm(monkeypatch, image_count):
    provider = _provider(monkeypatch)
    stub = _install_mlx_stub(monkeypatch)
    images = [Image.new("RGB", (8, 8)) for _ in range(image_count)]

    provider.run_generate(contents=["hi", *images])

    assert stub.template.call_args.kwargs["num_images"] == image_count
    passed_images = stub.generate.call_args.kwargs["image"]
    assert [id(image) for image in passed_images] == [id(image) for image in images]


def test_schema_mode_passes_logits_processor_and_raw_text(monkeypatch):
    provider = _provider(monkeypatch)
    stub = _install_mlx_stub(monkeypatch, text='{"ok": true}')
    schema = {"type": "object"}

    result = provider.run_generate("hi", json_schema=schema)

    stub.build_schema.assert_called_once_with(stub.processor.tokenizer, schema)
    assert stub.generate.call_args.kwargs["logits_processors"] == [
        stub.logits_processor
    ]
    assert result["text"] == '{"ok": true}'


def test_schema_mode_returns_invalid_json_verbatim(monkeypatch):
    provider = _provider(monkeypatch)
    _install_mlx_stub(monkeypatch, text="{")

    result = provider.run_generate("hi", json_schema={"type": "object"})

    assert result["text"] == "{"


def test_no_schema_path_does_not_build_logits_processor(monkeypatch):
    provider = _provider(monkeypatch)
    stub = _install_mlx_stub(monkeypatch)

    provider.run_generate("hi", json_schema=None)

    stub.build_schema.assert_not_called()
    assert "logits_processors" not in stub.generate.call_args.kwargs


def test_text_only_uses_no_images(monkeypatch):
    provider = _provider(monkeypatch)
    stub = _install_mlx_stub(monkeypatch, text="plain")

    result = provider.run_generate(contents=["just text"])

    assert stub.template.call_args.kwargs["num_images"] == 0
    assert stub.generate.call_args.kwargs["image"] is None
    assert result["text"] == "plain"


def test_run_cogitate_raises_unsupported():
    from solstone.think.providers import mlx

    with pytest.raises(RuntimeError) as exc_info:
        asyncio.run(mlx.run_cogitate(config={}))

    assert "vision" in str(exc_info.value)
    assert "v1" in str(exc_info.value)


def test_model_snapshot_missing_error_translated(monkeypatch):
    from huggingface_hub.errors import LocalEntryNotFoundError

    provider = _provider(monkeypatch)
    _install_mlx_stub(monkeypatch, load_exc=LocalEntryNotFoundError("missing"))

    with pytest.raises(provider.ModelSnapshotMissingError) as exc_info:
        provider.run_generate("hi")

    assert "model snapshot not present" in str(exc_info.value)


def test_other_load_errors_pass_through(monkeypatch):
    provider = _provider(monkeypatch)
    _install_mlx_stub(monkeypatch, load_exc=RuntimeError("disk full"))

    with pytest.raises(RuntimeError, match="disk full"):
        provider.run_generate("hi")


def test_cache_reuse(monkeypatch):
    provider = _provider(monkeypatch)
    stub = _install_mlx_stub(monkeypatch)

    provider.run_generate("one")
    provider.run_generate("two")

    stub.load.assert_called_once()


def test_registry_pins_qwen_spec(monkeypatch):
    from solstone.think.models import QWEN_35_9B

    provider = _provider(monkeypatch)

    spec = provider._MLX_MODEL_REGISTRY[QWEN_35_9B]
    assert spec.repo == "mlx-community/Qwen3.5-9B-MLX-8bit"
    assert spec.revision == "84f7c2deea248d8df56240f88102def51c7ed5d6"
    assert spec.min_ram_bytes == 16 * 1024**3
    assert spec.post_load is None


def test_registry_pins_gemma4_spec(monkeypatch):
    provider = _provider(monkeypatch)

    spec = provider._MLX_MODEL_REGISTRY[provider.GEMMA4_26B_A4B_4BIT]
    assert spec.repo == "mlx-community/gemma-4-26b-a4b-it-4bit"
    assert spec.revision == "efbeee6e582ebfd06abc9d65e90839c4b5d2116b"
    assert spec.min_ram_bytes == 24 * 1024**3
    assert spec.post_load is provider._gemma4_post_load


@pytest.mark.parametrize("suffix", ["REPO", "REVISION"])
def test_legacy_mlx_repo_constants_are_not_importable(suffix):
    name = "MLX_MODEL_" + suffix
    with pytest.raises(ImportError):
        exec(f"from solstone.think.providers.mlx import {name}", {})


def test_list_models_returns_registry_keys(monkeypatch):
    from solstone.think.models import QWEN_35_9B

    provider = _provider(monkeypatch)

    assert provider.list_models() == [QWEN_35_9B, provider.GEMMA4_26B_A4B_4BIT]


def test_load_model_rejects_unknown_model(monkeypatch):
    provider = _provider(monkeypatch)

    with pytest.raises(ValueError) as exc_info:
        provider._load_model("nope:1.0")

    assert "unknown MLX model" in str(exc_info.value)
    assert "nope:1.0" in str(exc_info.value)


def test_cache_is_keyed_by_model_name(monkeypatch):
    from solstone.think.models import QWEN_35_9B

    provider = _provider(monkeypatch)
    stub = _install_mlx_stub(monkeypatch)

    first = provider._load_model(QWEN_35_9B)
    second = provider._load_model(QWEN_35_9B)

    stub.load.assert_called_once()
    assert second is first


def test_cache_holds_multiple_models_independently(monkeypatch):
    from solstone.think.models import QWEN_35_9B

    provider = _provider(monkeypatch)
    stub = _install_mlx_stub(monkeypatch)
    other_spec = provider.MLXModelSpec(
        name="test:other",
        repo="example/test-mlx",
        revision="test-rev",
        min_ram_bytes=1,
        post_load=None,
    )
    monkeypatch.setitem(provider._MLX_MODEL_REGISTRY, "test:other", other_spec)

    first = provider._load_model(QWEN_35_9B)
    second = provider._load_model("test:other")

    assert set(provider._module_level_cache) == {QWEN_35_9B, "test:other"}
    assert second is not first
    assert stub.load.call_count == 2
    qwen_spec = provider._MLX_MODEL_REGISTRY[QWEN_35_9B]
    stub.load.assert_any_call(qwen_spec.repo, revision=qwen_spec.revision)
    stub.load.assert_any_call("example/test-mlx", revision="test-rev")


def test_post_load_runs_once_before_cache(monkeypatch):
    from solstone.think.models import QWEN_35_9B

    provider = _provider(monkeypatch)
    stub = _install_mlx_stub(monkeypatch)
    hook = MagicMock()
    base_spec = provider._MLX_MODEL_REGISTRY[QWEN_35_9B]
    monkeypatch.setitem(
        provider._MLX_MODEL_REGISTRY,
        QWEN_35_9B,
        provider.MLXModelSpec(
            name=base_spec.name,
            repo=base_spec.repo,
            revision=base_spec.revision,
            min_ram_bytes=base_spec.min_ram_bytes,
            post_load=hook,
        ),
    )

    provider._load_model(QWEN_35_9B)
    provider._load_model(QWEN_35_9B)

    hook.assert_called_once_with(stub.model, stub.processor)


def test_post_load_exception_leaves_cache_empty(monkeypatch):
    from solstone.think.models import QWEN_35_9B

    provider = _provider(monkeypatch)
    _install_mlx_stub(monkeypatch)
    base_spec = provider._MLX_MODEL_REGISTRY[QWEN_35_9B]
    monkeypatch.setitem(
        provider._MLX_MODEL_REGISTRY,
        QWEN_35_9B,
        provider.MLXModelSpec(
            name=base_spec.name,
            repo=base_spec.repo,
            revision=base_spec.revision,
            min_ram_bytes=base_spec.min_ram_bytes,
            post_load=MagicMock(side_effect=RuntimeError("post-load broke")),
        ),
    )

    with pytest.raises(RuntimeError, match="post-load broke"):
        provider.run_generate("hi")

    assert QWEN_35_9B not in provider._module_level_cache


def test_is_mlx_available_for_model_low_ram_includes_model_name(monkeypatch):
    from solstone.think.models import QWEN_35_9B

    provider = _provider(monkeypatch)
    monkeypatch.setattr(provider.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(provider.platform, "machine", lambda: "arm64")
    monkeypatch.setattr(
        provider.psutil,
        "virtual_memory",
        lambda: SimpleNamespace(total=8 * 1024**3),
    )
    monkeypatch.setitem(sys.modules, "mlx_vlm", types.ModuleType("mlx_vlm"))

    result = provider.is_mlx_available_for_model(
        provider._MLX_MODEL_REGISTRY[QWEN_35_9B]
    )

    assert result == (
        False,
        "insufficient RAM for qwen3.5:9b (need 16 GB, have 8 GB)",
    )


def test_is_mlx_available_for_gemma4_low_ram_includes_model_name(monkeypatch):
    provider = _provider(monkeypatch)
    monkeypatch.setattr(provider.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(provider.platform, "machine", lambda: "arm64")
    monkeypatch.setattr(
        provider.psutil,
        "virtual_memory",
        lambda: SimpleNamespace(total=16 * 1024**3),
    )
    monkeypatch.setitem(sys.modules, "mlx_vlm", types.ModuleType("mlx_vlm"))

    result = provider.is_mlx_available_for_model(
        provider._MLX_MODEL_REGISTRY[provider.GEMMA4_26B_A4B_4BIT]
    )

    assert result == (
        False,
        "insufficient RAM for gemma-4-26b-a4b-it-mlx-4bit (need 24 GB, have 16 GB)",
    )


def test_snapshot_missing_error_contains_repo_and_revision(monkeypatch):
    from huggingface_hub.errors import LocalEntryNotFoundError

    from solstone.think.models import QWEN_35_9B

    provider = _provider(monkeypatch)
    _install_mlx_stub(monkeypatch, load_exc=LocalEntryNotFoundError("missing"))

    with pytest.raises(provider.ModelSnapshotMissingError) as exc_info:
        provider.run_generate("hi")

    assert "model snapshot not present" in str(exc_info.value)
    spec = provider._MLX_MODEL_REGISTRY[QWEN_35_9B]
    assert f"{spec.repo}@{spec.revision}" in str(exc_info.value)


def test_gemma4_post_load_writes_all_patch_attributes(monkeypatch):
    provider = _provider(monkeypatch)
    model, processor = _gemma4_stubs()

    provider._gemma4_post_load(model, processor)

    assert processor.image_processor.max_soft_tokens == 1120
    assert processor.image_processor.image_seq_length == 1120
    assert processor.image_seq_length == 1120
    assert model.vision_tower.max_patches == 10080
    assert model.vision_tower.default_output_length == 1120
    assert model.vision_tower.pooler.default_output_length == 1120


def test_gemma4_post_load_refuses_small_position_embedding(monkeypatch):
    provider = _provider(monkeypatch)
    model, processor = _gemma4_stubs()
    model.config.vision_config.position_embedding_size = 5120

    with pytest.raises(RuntimeError) as exc_info:
        provider._gemma4_post_load(model, processor)

    message = str(exc_info.value)
    assert provider.GEMMA4_26B_A4B_4BIT in message
    assert "10240" in message
    assert "5120" in message


def test_gemma4_post_load_refuses_missing_position_embedding(monkeypatch):
    provider = _provider(monkeypatch)
    model, processor = _gemma4_stubs()
    delattr(model.config.vision_config, "position_embedding_size")

    with pytest.raises(RuntimeError) as exc_info:
        provider._gemma4_post_load(model, processor)

    message = str(exc_info.value)
    assert provider.GEMMA4_26B_A4B_4BIT in message
    assert "position_embedding_size" in message
    assert "missing" in message


@pytest.mark.parametrize(
    ("attr_name", "expected_message"),
    [
        ("max_soft_tokens", "processor.image_processor.max_soft_tokens"),
        ("max_patches", "model.vision_tower.max_patches"),
        ("default_output_length", "model.vision_tower.default_output_length"),
    ],
)
def test_gemma4_post_load_asserts_required_writes_stick(
    monkeypatch, attr_name, expected_message
):
    provider = _provider(monkeypatch)
    model, processor = _gemma4_stubs()

    if attr_name == "max_soft_tokens":

        class NoOpImageProcessor:
            image_seq_length = 0

            @property
            def max_soft_tokens(self):
                return 0

            @max_soft_tokens.setter
            def max_soft_tokens(self, _value):
                return None

        processor.image_processor = NoOpImageProcessor()
    elif attr_name == "max_patches":

        class NoOpMaxPatchesVisionTower:
            pooling_kernel_size = 3
            default_output_length = 0
            pooler = SimpleNamespace(default_output_length=0)

            @property
            def max_patches(self):
                return 0

            @max_patches.setter
            def max_patches(self, _value):
                return None

        model.vision_tower = NoOpMaxPatchesVisionTower()
    else:

        class NoOpDefaultOutputVisionTower:
            pooling_kernel_size = 3
            max_patches = 0
            pooler = SimpleNamespace(default_output_length=0)

            @property
            def default_output_length(self):
                return 0

            @default_output_length.setter
            def default_output_length(self, _value):
                return None

        model.vision_tower = NoOpDefaultOutputVisionTower()

    with pytest.raises(RuntimeError) as exc_info:
        provider._gemma4_post_load(model, processor)

    assert expected_message in str(exc_info.value)


def test_gemma4_post_load_skips_missing_conditional_writes(monkeypatch):
    provider = _provider(monkeypatch)
    model, processor = _gemma4_stubs(optional_attrs=False)

    provider._gemma4_post_load(model, processor)

    assert processor.image_processor.max_soft_tokens == 1120
    assert model.vision_tower.max_patches == 10080
    assert model.vision_tower.default_output_length == 1120
    assert not hasattr(processor.image_processor, "image_seq_length")
    assert not hasattr(processor, "image_seq_length")
    assert not hasattr(model.vision_tower, "pooler")


def test_resolve_default_model_reads_config_active_model(monkeypatch):
    provider = _provider(monkeypatch)
    monkeypatch.setattr(
        provider,
        "get_config",
        lambda: {"providers": {"mlx": {"active_model": provider.GEMMA4_26B_A4B_4BIT}}},
    )

    assert provider._resolve_default_model() == provider.GEMMA4_26B_A4B_4BIT


def test_run_generate_uses_active_model_when_model_omitted(monkeypatch):
    provider = _provider(monkeypatch)
    stub = _install_mlx_stub(monkeypatch)
    calls = []
    monkeypatch.setattr(
        provider,
        "get_config",
        lambda: {"providers": {"mlx": {"active_model": provider.GEMMA4_26B_A4B_4BIT}}},
    )

    def fake_load_model(model_name):
        calls.append(model_name)
        return stub.model, stub.processor, stub.model.config

    monkeypatch.setattr(provider, "_load_model", fake_load_model)

    provider.run_generate("hi")

    assert calls == [provider.GEMMA4_26B_A4B_4BIT]


def test_resolve_default_model_unknown_config_warns_and_falls_back(monkeypatch, caplog):
    from solstone.think.models import QWEN_35_9B

    provider = _provider(monkeypatch)
    monkeypatch.setattr(
        provider,
        "get_config",
        lambda: {"providers": {"mlx": {"active_model": "bogus"}}},
    )

    with caplog.at_level("WARNING"):
        assert provider._resolve_default_model() == QWEN_35_9B

    assert "bogus" in caplog.text


def test_resolve_default_model_unreadable_config_falls_back(monkeypatch, caplog):
    from solstone.think.models import QWEN_35_9B

    provider = _provider(monkeypatch)
    monkeypatch.setattr(
        provider,
        "get_config",
        MagicMock(side_effect=OSError("config unreadable")),
    )

    with caplog.at_level("WARNING"):
        assert provider._resolve_default_model() == QWEN_35_9B

    assert "config unreadable" in caplog.text


def test_run_generate_explicit_unknown_model_still_raises(monkeypatch):
    provider = _provider(monkeypatch)

    with pytest.raises(ValueError, match="unknown MLX model"):
        provider.run_generate("hi", model="bogus")
