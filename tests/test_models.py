# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think.models module."""

import asyncio
import json
import logging
import os
from pathlib import Path

import pytest

import solstone.think.models as models_module
from solstone.think.models import (
    CLAUDE_OPUS_4,
    CLAUDE_SONNET_4,
    DEFAULT_MODEL_BY_PROVIDER,
    GEMINI_FLASH,
    GEMMA4_26B_A4B_4BIT,
    GPT_5_MINI,
    LOCAL_MODEL,
    NO_BRAIN_PROVIDER,
    PROMPT_PATHS,
    QWEN_35_9B,
    IncompleteJSONError,
    NoBrainConfiguredError,
    ProviderResponseInvalidError,
    _Family,
    _find_pricing_fallback,
    _parse_family_anthropic,
    _parse_family_gemini,
    _parse_family_openai,
    agenerate,
    agenerate_with_result,
    calc_agent_cost,
    calc_token_cost,
    default_model_for_provider,
    generate,
    generate_with_result,
    get_context_registry,
    get_model_provider,
    get_usage_cost,
    is_local_provider_needed,
    iter_token_log,
    model_supports,
    resolve_provider,
)


def test_calc_token_cost_basic():
    """Test basic cost calculation with a known model."""
    token_data = {
        "model": "gpt-4o",
        "usage": {
            "input_tokens": 1000,
            "output_tokens": 100,
            "total_tokens": 1100,
        },
    }

    result = calc_token_cost(token_data)

    assert result is not None
    assert "total_cost" in result
    assert "input_cost" in result
    assert "output_cost" in result
    assert "currency" in result
    assert result["currency"] == "USD"
    assert result["total_cost"] > 0
    assert result["input_cost"] > 0
    assert result["output_cost"] > 0


def test_calc_token_cost_with_cache():
    """Test cost calculation with cached tokens."""
    token_data = {
        "model": "claude-sonnet-4-20250514",
        "usage": {
            "input_tokens": 1000,
            "output_tokens": 100,
            "cached_tokens": 500,
            "total_tokens": 1600,
        },
    }

    result = calc_token_cost(token_data)

    assert result is not None
    assert result["total_cost"] > 0
    # Cached tokens should reduce the cost compared to all uncached
    assert result["input_cost"] >= 0


def test_calc_agent_cost_uses_resolved_model_version_from_usage():
    usage = {
        "input_tokens": 1000,
        "output_tokens": 500,
        "model_version": "gemini-2.5-flash",
    }

    cost = calc_agent_cost("gemini-3.5-flash", usage)

    assert cost is not None and cost > 0


def test_calc_token_cost_unknown_model():
    """Test that unknown models return None."""
    token_data = {
        "model": "random-model-xyz",
        "usage": {
            "input_tokens": 1000,
            "output_tokens": 100,
        },
    }

    result = calc_token_cost(token_data)
    assert result is None


def test_get_model_provider_mlx_backend_models_are_local():
    assert get_model_provider(GEMMA4_26B_A4B_4BIT) == "local"
    assert get_model_provider(QWEN_35_9B) == "local"


@pytest.mark.parametrize("reason", ["length", "max_tokens", "MAX_TOKENS", " Length "])
def test_incomplete_json_error_sets_length_reason_code(reason):
    exc = IncompleteJSONError(reason, "")

    assert exc.reason_code == "incomplete_json_length"


@pytest.mark.parametrize("reason", ["safety", "content_filter", "recitation", "error"])
def test_incomplete_json_error_non_length_reasons_have_no_reason_code(reason):
    exc = IncompleteJSONError(reason, "")

    assert not hasattr(exc, "reason_code")


def test_incomplete_json_error_preserves_positional_and_keyword_construction():
    positional = IncompleteJSONError("length", "partial")
    keyword = IncompleteJSONError(reason="max_tokens", partial_text="body")

    assert positional.reason == "length"
    assert positional.partial_text == "partial"
    assert keyword.reason == "max_tokens"
    assert keyword.partial_text == "body"
    assert positional.reason_code == "incomplete_json_length"
    assert keyword.reason_code == "incomplete_json_length"


def test_classify_provider_error_uses_incomplete_json_reason_code():
    from solstone.think.providers.shared import classify_provider_error

    assert (
        classify_provider_error(IncompleteJSONError("length", ""), "local")
        == "incomplete_json_length"
    )
    assert (
        classify_provider_error(IncompleteJSONError("safety", ""), "local")
        != "incomplete_json_length"
    )


def test_calc_token_cost_gemma4_zero_cost():
    token_data = {
        "model": GEMMA4_26B_A4B_4BIT,
        "usage": {
            "input_tokens": 1000,
            "output_tokens": 100,
        },
    }

    assert calc_token_cost(token_data) == {
        "total_cost": 0.0,
        "input_cost": 0.0,
        "output_cost": 0.0,
        "currency": "USD",
    }


def test_calc_token_cost_missing_data():
    """Test that missing data returns None."""
    # Missing model
    assert calc_token_cost({"usage": {"input_tokens": 1000}}) is None

    # Missing usage
    assert calc_token_cost({"model": "gpt-4o"}) is None

    # Empty dict
    assert calc_token_cost({}) is None


def test_calc_token_cost_with_reasoning_tokens():
    """Test cost calculation includes reasoning tokens in output."""
    token_data = {
        "model": "gpt-4o",
        "usage": {
            "input_tokens": 1000,
            "output_tokens": 100,
            "reasoning_tokens": 50,
            "total_tokens": 1150,
        },
    }

    result = calc_token_cost(token_data)

    # Should succeed - reasoning tokens are implicitly part of output pricing
    assert result is not None
    assert result["total_cost"] > 0


# ---------------------------------------------------------------------------
# resolve_provider tests
# ---------------------------------------------------------------------------


@pytest.fixture
def use_fixtures_journal(monkeypatch):
    """Use the fixtures journal for provider config tests."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")


def _write_tmp_journal_config(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, config: dict
) -> None:
    config_dir = tmp_path / "config"
    config_dir.mkdir()
    (config_dir / "journal.json").write_text(json.dumps(config))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))


def test_resolve_provider_default_generate(use_fixtures_journal):
    """Generate resolves the configured active provider/model."""
    provider, model = resolve_provider("generate")
    assert provider == "google"
    assert model == "gemini-custom-flash-test"


def test_resolve_provider_default_cogitate(use_fixtures_journal):
    """Cogitate resolves the configured active provider/model."""
    provider, model = resolve_provider("cogitate")
    assert provider == "google"
    assert model == "gemini-custom-flash-test"


def test_resolve_provider_contexts_are_inert(monkeypatch, tmp_path):
    """Legacy exact/glob contexts cannot influence active route resolution."""
    _write_tmp_journal_config(
        tmp_path,
        monkeypatch,
        {
            "providers": {
                "active": {
                    "provider": "google",
                    "model": "gemini-3.5-flash",
                },
                "contexts": {
                    "test.openai": {
                        "provider": "openai",
                        "model": "gpt-5-mini",
                    },
                    "observe.*": {
                        "provider": "anthropic",
                        "model": "claude-haiku-4-5",
                    },
                },
            }
        },
    )

    provider, model = resolve_provider("generate")
    assert provider == "google"
    assert model == GEMINI_FLASH


def test_resolve_provider_ordering_witness(monkeypatch, tmp_path):
    """Explicit provider/model wins over key-presence fallback."""
    _write_tmp_journal_config(
        tmp_path,
        monkeypatch,
        {
            "providers": {
                "active": {
                    "provider": "anthropic",
                    "model": "claude-haiku-4-5",
                }
            }
        },
    )
    monkeypatch.setenv("OPENAI_API_KEY", "test-openai-key")

    assert resolve_provider("generate") == ("anthropic", "claude-haiku-4-5")


def test_resolve_provider_empty_context(use_fixtures_journal):
    """The resolver takes only an interface, not a context."""
    assert resolve_provider("generate") == ("google", "gemini-custom-flash-test")


def test_resolve_provider_no_config(monkeypatch, tmp_path):
    """Test no-brain resolution when no provider config exists."""
    # Use a journal path with no config
    empty_journal = tmp_path / "empty_journal"
    empty_journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(empty_journal))
    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    provider, model = resolve_provider("generate")
    assert provider == NO_BRAIN_PROVIDER
    assert provider != "google"
    assert model == ""

    provider, model = resolve_provider("cogitate")
    assert provider == NO_BRAIN_PROVIDER
    assert provider != "google"
    assert model == ""


def test_prompt_paths_exist():
    """Test all PROMPT_PATHS files exist and have valid frontmatter."""
    from pathlib import Path

    import frontmatter

    base_dir = Path(__file__).parent.parent / "solstone"  # Package root
    required_keys = {"context", "label", "group"}

    for rel_path in PROMPT_PATHS:
        path = base_dir / rel_path
        assert path.exists(), f"Prompt file not found: {rel_path}"

        post = frontmatter.load(path)
        meta = post.metadata or {}

        assert required_keys <= set(meta.keys()), (
            f"{rel_path} missing keys: {required_keys - set(meta.keys())}"
        )
        assert isinstance(meta["label"], str) and meta["label"], (
            f"{rel_path} has invalid label: {meta['label']}"
        )
        assert isinstance(meta["group"], str) and meta["group"], (
            f"{rel_path} has invalid group: {meta['group']}"
        )


def test_prompt_contexts_in_registry():
    """Test prompt contexts are discovered and in registry."""
    registry = get_context_registry()

    # Verify known prompt contexts exist with correct values
    assert "observe.describe.frame" in registry
    assert registry["observe.describe.frame"]["group"] == "Observe"

    assert "detect.created" in registry


def test_default_model_by_provider():
    assert DEFAULT_MODEL_BY_PROVIDER == {
        "google": GEMINI_FLASH,
        "openai": GPT_5_MINI,
        "anthropic": CLAUDE_SONNET_4,
        "local": LOCAL_MODEL,
    }
    for provider, model in DEFAULT_MODEL_BY_PROVIDER.items():
        assert default_model_for_provider(provider) == model


@pytest.mark.parametrize(
    "config",
    [
        {"providers": {"active": {"provider": "local"}}},
    ],
)
def test_is_local_provider_needed_true_for_selected_surfaces(config):
    assert is_local_provider_needed(config) is True


@pytest.mark.parametrize(
    "config",
    [
        {},
        {"providers": {"active": {"provider": "google"}}},
        {"providers": {"contexts": {"talent.*": {"provider": "anthropic"}}}},
        {"providers": {"contexts": {"talent.*": {"provider": "local"}}}},
        {"providers": []},
    ],
)
def test_is_local_provider_needed_false_when_not_selected(config, monkeypatch):
    assert is_local_provider_needed(config) is False


def test_is_local_provider_needed_does_not_infer_from_runtime(monkeypatch):
    monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)

    assert is_local_provider_needed({}) is False


def test_resolve_provider_legacy_keys_are_inert(
    use_fixtures_journal, monkeypatch, tmp_path
):
    """Tier/backup/contexts/models legacy keys do not affect active routing."""
    _write_tmp_journal_config(
        tmp_path,
        monkeypatch,
        {
            "providers": {
                "active": {
                    "provider": "google",
                    "model": "gemini-3.5-flash",
                    "tier": 1,
                    "backup": "anthropic",
                },
                "contexts": {
                    "talent.timeline.segment_summary": {"provider": "local"},
                    "observe.*": {"provider": "anthropic", "tier": 3},
                },
                "models": {
                    "google": {"1": "gemini-3.5-flash"},
                    "anthropic": {"3": "claude-haiku-4-5"},
                },
            }
        },
    )

    assert resolve_provider("generate") == ("google", GEMINI_FLASH)


def test_resolve_provider_model_key_wins_even_when_tier_present(
    use_fixtures_journal, monkeypatch, tmp_path
):
    """The retired AC3a quirk is gone: model is honored even with tier present."""
    _write_tmp_journal_config(
        tmp_path,
        monkeypatch,
        {
            "providers": {
                "active": {
                    "provider": "google",
                    "tier": 1,
                    "model": "gemini-custom-flash-test",
                }
            }
        },
    )

    assert resolve_provider("generate") == ("google", "gemini-custom-flash-test")


def test_resolve_provider_local_type_default_ignores_context_pins(
    use_fixtures_journal, monkeypatch, tmp_path
):
    """Context pins cannot push a local active interface onto cloud."""
    _write_tmp_journal_config(
        tmp_path,
        monkeypatch,
        {
            "providers": {
                "active": {"provider": "local"},
                "contexts": {
                    "talent.timeline.segment_summary": {
                        "provider": "google",
                        "model": "gemini-3.1-flash-lite",
                    },
                },
            }
        },
    )

    assert resolve_provider("generate") == ("local", LOCAL_MODEL)


def test_legacy_context_toggles_remain_on_disk_but_not_routing(
    use_fixtures_journal, monkeypatch, tmp_path
):
    """Grandfathered context keys stay inert next to disabled/extract toggles."""
    _write_tmp_journal_config(
        tmp_path,
        monkeypatch,
        {
            "providers": {
                "active": {"provider": "google", "model": GEMINI_FLASH},
            },
            "talent_overrides": {"talent.x": {"disabled": True, "extract": "foo"}},
        },
    )

    assert resolve_provider("generate") == ("google", GEMINI_FLASH)

    stored = json.loads((tmp_path / "config" / "journal.json").read_text())
    context = stored["talent_overrides"]["talent.x"]
    assert context["disabled"] is True
    assert context["extract"] == "foo"


def test_prepare_config_legacy_context_routing_keys_are_inert(
    journal_copy: Path,
) -> None:
    """Legacy contexts survive on disk but cannot change prepared identity."""
    from solstone.think.talents import prepare_config

    config_path = journal_copy / "config" / "journal.json"
    config = json.loads(config_path.read_text(encoding="utf-8"))
    config["providers"] = {
        "active": {"provider": "anthropic", "model": CLAUDE_SONNET_4},
    }
    config["talent_overrides"] = {
        "talent.timeline.segment_summary": {
            "disabled": False,
        }
    }
    config_path.write_text(json.dumps(config), encoding="utf-8")

    prepared = prepare_config({"name": "timeline:segment_summary"})

    assert prepared["provider"] == "anthropic"
    assert prepared["model"] == CLAUDE_SONNET_4


def test_prepare_config_frontmatter_provider_pin_is_dead_through_dispatch_identity(
    journal_copy: Path,
) -> None:
    """Removed google frontmatter pins do not override the active brain."""
    from solstone.think.talents import prepare_config

    config_path = journal_copy / "config" / "journal.json"
    config = json.loads(config_path.read_text(encoding="utf-8"))
    config["env"] = {"GOOGLE_API_KEY": "test-google-key"}
    config["providers"] = {
        "active": {"provider": "anthropic", "model": CLAUDE_SONNET_4}
    }
    config_path.write_text(json.dumps(config), encoding="utf-8")

    segment = prepare_config({"name": "timeline:segment_summary"})
    detection = prepare_config({"name": "entities:detection"})

    assert segment["provider"] == "anthropic"
    assert segment["model"] == CLAUDE_SONNET_4
    assert detection["provider"] == "anthropic"
    assert detection["model"] == CLAUDE_SONNET_4


def test_resolve_provider_cogitate_system_talents_stay_local(
    use_fixtures_journal, monkeypatch, tmp_path
):
    """Cogitate system talents stay local when the cogitate lane is local."""
    _write_tmp_journal_config(
        tmp_path,
        monkeypatch,
        {"providers": {"active": {"provider": "local"}}},
    )

    assert resolve_provider("cogitate") == ("local", LOCAL_MODEL)


def test_generate_and_cogitate_share_one_active_profile(
    use_fixtures_journal, monkeypatch, tmp_path
):
    """Both runtime interfaces resolve the same active profile."""
    _write_tmp_journal_config(
        tmp_path,
        monkeypatch,
        {
            "providers": {
                "active": {"provider": "openai"},
            }
        },
    )

    assert resolve_provider("generate") == ("openai", GPT_5_MINI)
    assert resolve_provider("cogitate") == ("openai", GPT_5_MINI)


# ---------------------------------------------------------------------------
# Dynamic context registry tests
# ---------------------------------------------------------------------------


def test_context_registry_includes_prompt_contexts():
    """Test that registry includes all contexts from PROMPT_PATHS."""
    from pathlib import Path

    import frontmatter

    registry = get_context_registry()
    base_dir = Path(__file__).parent.parent / "solstone"

    # All prompt contexts should be in registry with matching metadata
    for rel_path in PROMPT_PATHS:
        path = base_dir / rel_path
        post = frontmatter.load(path)
        meta = post.metadata or {}
        context = meta.get("context")

        assert context in registry, f"Prompt context {context} not in registry"
        assert registry[context]["label"] == meta["label"]
        assert registry[context]["group"] == meta["group"]


def test_context_registry_includes_categories():
    """Test that registry includes discovered category contexts."""
    registry = get_context_registry()

    # Should have category entries (from observe/categories/*.md)
    category_contexts = [k for k in registry if k.startswith("observe.describe.")]

    # Should have frame + all categories (browsing, code, gaming, etc.)
    assert len(category_contexts) > 5, "Should discover category contexts"

    # Each category context should have required fields
    for context in category_contexts:
        assert "label" in registry[context]
        assert "group" in registry[context]


def test_context_registry_includes_talent_configs():
    """Test that registry includes discovered talent contexts (agents + generators)."""
    registry = get_context_registry()

    # Should have talent entries (from talent/*.md and apps/*/talent/*.md)
    talent_contexts = [k for k in registry if k.startswith("talent.")]

    # Should have multiple talent contexts (agents + generators)
    assert len(talent_contexts) > 1, "Should discover talent contexts"

    # Should have system talent configs
    system_talent = [k for k in talent_contexts if k.startswith("talent.system.")]
    assert len(system_talent) > 0, "Should discover system talent configs"

    # Should have app talent configs
    app_talent = [
        k
        for k in talent_contexts
        if k.startswith("talent.") and not k.startswith("talent.system.")
    ]
    assert len(app_talent) > 0, "Should discover app talent configs"

    # Should include type field for talent contexts
    for context in talent_contexts:
        assert "type" in registry[context], f"{context} missing type field"


def test_context_registry_structure():
    """Test that all registry entries have required fields."""
    registry = get_context_registry()
    required_keys = {"label", "group"}

    for context, config in registry.items():
        assert isinstance(config, dict), f"{context} should be a dict"
        assert required_keys <= set(config.keys()), (
            f"{context} missing keys: {required_keys - set(config.keys())}"
        )


def test_context_registry_is_cached():
    """Test that registry is built once and cached."""
    registry1 = get_context_registry()
    registry2 = get_context_registry()

    # Should return the same object (cached)
    assert registry1 is registry2


# ---------------------------------------------------------------------------
# Model pricing support tests
# ---------------------------------------------------------------------------


def test_all_default_models_have_pricing():
    """Verify all default provider models have genai-prices support.

    This test ensures that when default models are updated, we catch any
    missing pricing data early. If this test fails:

    1. Run: make update-prices
    2. Re-run this test
    3. If still failing, the model may be too new for genai-prices

    See think/models.py model constants section for more details.
    """
    all_models = set(DEFAULT_MODEL_BY_PROVIDER.values())
    all_models.add(CLAUDE_OPUS_4)

    missing_pricing = []
    for model in sorted(all_models):
        token_data = {
            "model": model,
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 100,
                "total_tokens": 1100,
            },
        }
        result = calc_token_cost(token_data)
        if result is None:
            missing_pricing.append(model)

    if missing_pricing:
        pytest.fail(
            f"Models missing genai-prices support: {missing_pricing}\n"
            "Run 'make update-prices' and re-test. "
            "If still failing, model may be too new for genai-prices."
        )


@pytest.mark.parametrize(
    ("model", "expected"),
    [
        ("gpt-5.5", _Family(("openai", None), (5, 5))),
        ("gpt-5", _Family(("openai", None), (5, 0))),
        ("gpt-5.5-mini", _Family(("openai", "mini"), (5, 5))),
        ("gpt-5.4-nano", _Family(("openai", "nano"), (5, 4))),
        ("gpt-5.2-pro", _Family(("openai", "pro"), (5, 2))),
    ],
)
def test_parse_family_openai(model, expected):
    assert _parse_family_openai(model) == expected


@pytest.mark.parametrize(
    "model",
    [
        "ft:gpt-5",
        "gpt-5-image",
        "gpt-5-image-mini",
        "gpt-5.1-codex-mini",
        "gpt-4o",
        "o3",
        "text-embedding-3-small",
    ],
)
def test_parse_family_openai_rejects_unsupported_models(model):
    assert _parse_family_openai(model) is None


@pytest.mark.parametrize(
    ("model", "expected"),
    [
        ("claude-opus-4", _Family(("anthropic", "opus"), (4, 0))),
        ("claude-opus-4-7", _Family(("anthropic", "opus"), (4, 7))),
        ("claude-sonnet-4-6", _Family(("anthropic", "sonnet"), (4, 6))),
        ("claude-haiku-5", _Family(("anthropic", "haiku"), (5, 0))),
    ],
)
def test_parse_family_anthropic(model, expected):
    assert _parse_family_anthropic(model) == expected


@pytest.mark.parametrize(
    "model",
    [
        "claude-3-opus-latest",
        "claude-3-5-haiku-latest",
        "claude-v2",
        "claude-2",
        "claude-sonnet-4-6-latest",
    ],
)
def test_parse_family_anthropic_rejects_unsupported_models(model):
    assert _parse_family_anthropic(model) is None


@pytest.mark.parametrize(
    ("model", "expected"),
    [
        ("gemini-3.5-flash", _Family(("gemini", "flash"), (3, 5))),
        ("gemini-3-flash", _Family(("gemini", "flash"), (3, 0))),
        ("gemini-3.1-flash-lite", _Family(("gemini", "flash-lite"), (3, 1))),
        ("gemini-2.5-pro-preview", _Family(("gemini", "pro"), (2, 5))),
        ("gemini-flash-latest", _Family(("gemini", "flash"), (0, 0))),
        ("gemini-pro-latest", _Family(("gemini", "pro"), (0, 0))),
        ("gemini-flash-lite-latest", _Family(("gemini", "flash-lite"), (0, 0))),
    ],
)
def test_parse_family_gemini(model, expected):
    assert _parse_family_gemini(model) == expected


@pytest.mark.parametrize(
    "model",
    [
        "gemini-3-pro-image-preview",
        "gemini-2.5-flash-image",
        "gemini-pro",
        "gemini-embedding-001",
        "gemini-live-2.5-flash-preview",
        "gemini-flash-1.5",
        "gemma-3",
    ],
)
def test_parse_family_gemini_rejects_unsupported_models(model):
    assert _parse_family_gemini(model) is None


@pytest.mark.parametrize(
    ("model", "provider_id", "expected"),
    [
        ("gemini-3.5-flash", "google", "gemini-3-flash-preview"),
        ("gemini-3.1-flash-lite", "google", "gemini-2.5-flash-lite"),
        ("gemini-flash-latest", "google", "gemini-3-flash-preview"),
        ("gemini-pro-latest", "google", "gemini-3.1-pro-preview"),
        ("gemini-flash-lite-latest", "google", "gemini-2.5-flash-lite"),
        ("claude-opus-4-7", "anthropic", "claude-opus-4-6"),
        ("claude-sonnet-4-6", "anthropic", "claude-sonnet-4-6"),
        ("claude-haiku-5", "anthropic", "claude-haiku-4-5"),
        ("gpt-5.5", "openai", "gpt-5.2"),
        ("gpt-5.5-mini", "openai", "gpt-5-mini"),
        ("totally-fake-model", "openai", None),
        ("text-embedding-3-small", "openai", None),
    ],
)
def test_find_pricing_fallback(model, provider_id, expected):
    _find_pricing_fallback.cache_clear()

    assert _find_pricing_fallback(model, provider_id) == expected


@pytest.mark.parametrize(
    "model",
    [
        "gemini-3.5-flash",
        "gemini-3.1-flash-lite",
        "claude-opus-4-7",
        "gpt-5.5",
    ],
)
def test_calc_token_cost_fallback(model):
    result = calc_token_cost(
        {"model": model, "usage": {"input_tokens": 1000, "output_tokens": 100}}
    )

    assert result is not None
    assert result["total_cost"] > 0


def test_calc_token_cost_fallback_returns_none_for_unknown_model():
    assert (
        calc_token_cost(
            {
                "model": "totally-fake-model",
                "usage": {"input_tokens": 1000, "output_tokens": 100},
            }
        )
        is None
    )


def test_calc_token_cost_fallback_keeps_local_free():
    assert (
        calc_token_cost(
            {
                "model": LOCAL_MODEL,
                "usage": {"input_tokens": 1000, "output_tokens": 100},
            }
        )["total_cost"]
        == 0.0
    )


def test_fallback_logging(caplog):
    models_module._LOGGED_FALLBACKS.clear()

    with caplog.at_level(logging.INFO, logger="solstone.think.models"):
        calc_token_cost(
            {
                "model": "gemini-3.5-flash",
                "usage": {"input_tokens": 1000, "output_tokens": 100},
            }
        )
        calc_token_cost(
            {
                "model": "gemini-3.5-flash",
                "usage": {"input_tokens": 1000, "output_tokens": 100},
            }
        )

    fallback_logs = [
        record.getMessage()
        for record in caplog.records
        if record.getMessage().startswith("pricing: family-fallback")
    ]
    assert fallback_logs == [
        "pricing: family-fallback gemini-3.5-flash -> gemini-3-flash-preview"
    ]

    with caplog.at_level(logging.INFO, logger="solstone.think.models"):
        calc_token_cost(
            {
                "model": "claude-opus-4-7",
                "usage": {"input_tokens": 1000, "output_tokens": 100},
            }
        )

    fallback_logs = [
        record.getMessage()
        for record in caplog.records
        if record.getMessage().startswith("pricing: family-fallback")
    ]
    assert fallback_logs == [
        "pricing: family-fallback gemini-3.5-flash -> gemini-3-flash-preview",
        "pricing: family-fallback claude-opus-4-7 -> claude-opus-4-6",
    ]


# ---------------------------------------------------------------------------
# get_usage_cost tests
# ---------------------------------------------------------------------------


def test_get_usage_cost_nonexistent_day(use_fixtures_journal):
    """Test that nonexistent day returns zeros."""
    result = get_usage_cost("19000101")
    assert result == {"requests": 0, "tokens": 0, "cost": 0.0}


def test_get_usage_cost_day_total(use_fixtures_journal):
    """Test aggregating all entries for a day."""
    # 20250823 has test entries with gemini models
    result = get_usage_cost("20250823")
    assert result["requests"] > 0
    assert isinstance(result["tokens"], int)
    assert isinstance(result["cost"], float)


def test_iter_token_log_preserves_type_field(use_fixtures_journal):
    """Token log iterator should preserve top-level type field."""
    entries = list(iter_token_log("20250823"))
    generate_entries = [entry for entry in entries if entry.get("type") == "generate"]

    assert generate_entries
    assert any(
        entry.get("context") == "think.detect_created.classify_new_file"
        for entry in generate_entries
    )


def test_get_usage_cost_context_filter(use_fixtures_journal):
    """Test filtering by context prefix."""
    # Filter to test contexts
    result = get_usage_cost("20250823", context="tests.test_gemini")
    assert result["requests"] > 0

    # Filter to non-matching context should return zeros
    result_empty = get_usage_cost("20250823", context="nonexistent.context")
    assert result_empty["requests"] == 0


def test_get_usage_cost_segment_filter(use_fixtures_journal):
    """Test filtering by segment key."""
    # Fixture data includes one entry tagged with segment 143022_300
    result = get_usage_cost("20250823", segment="143022_300")
    assert result["requests"] == 1
    assert result["tokens"] == 7000
    assert result["cost"] > 0.0


def test_get_usage_cost_combined_filters(use_fixtures_journal):
    """Test combined segment and context filters."""
    # With both filters, entries must match both
    result = get_usage_cost(
        "20250823",
        segment="nonexistent",
        context="tests.test_gemini",
    )
    # Segment doesn't exist, so no matches
    assert result["requests"] == 0


# ---------------------------------------------------------------------------
# log_token_usage normalization tests
# ---------------------------------------------------------------------------


def test_log_token_usage_computes_total_tokens(tmp_path, monkeypatch):
    """total_tokens is computed from input+output when missing (Codex CLI format)."""
    import json

    from solstone.think.models import log_token_usage

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Codex CLI format: no total_tokens
    log_token_usage(
        model="gpt-5.2",
        usage={"input_tokens": 1000, "output_tokens": 200},
        context="test",
    )

    log_file = tmp_path / "tokens" / (__import__("time").strftime("%Y%m%d") + ".jsonl")
    entry = json.loads(log_file.read_text().strip())
    assert entry["usage"]["total_tokens"] == 1200
    assert entry["usage"]["input_tokens"] == 1000
    assert entry["usage"]["output_tokens"] == 200


def test_log_token_usage_preserves_existing_total_tokens(tmp_path, monkeypatch):
    """total_tokens is preserved when already present and non-zero."""
    import json

    from solstone.think.models import log_token_usage

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    log_token_usage(
        model="gpt-5.2",
        usage={"input_tokens": 1000, "output_tokens": 200, "total_tokens": 1500},
        context="test",
    )

    log_file = tmp_path / "tokens" / (__import__("time").strftime("%Y%m%d") + ".jsonl")
    entry = json.loads(log_file.read_text().strip())
    assert entry["usage"]["total_tokens"] == 1500


def test_log_token_usage_maps_cached_input_tokens(tmp_path, monkeypatch):
    """cached_input_tokens (Codex CLI format) maps to cached_tokens."""
    import json

    from solstone.think.models import log_token_usage

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    log_token_usage(
        model="gpt-5.2",
        usage={
            "input_tokens": 1000,
            "cached_input_tokens": 800,
            "output_tokens": 200,
        },
        context="test",
    )

    log_file = tmp_path / "tokens" / (__import__("time").strftime("%Y%m%d") + ".jsonl")
    entry = json.loads(log_file.read_text().strip())
    assert entry["usage"]["cached_tokens"] == 800
    assert entry["usage"]["total_tokens"] == 1200


def test_log_token_usage_passes_through_reasoning_tokens(tmp_path, monkeypatch):
    """reasoning_tokens from provider-normalized usage are preserved in log."""
    import json

    from solstone.think.models import log_token_usage

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Normalized usage from Google provider (the bug: reasoning_tokens were dropped)
    log_token_usage(
        model="gemini-3-flash-preview",
        usage={
            "input_tokens": 13319,
            "output_tokens": 969,
            "total_tokens": 37878,
            "reasoning_tokens": 23590,
        },
        context="test",
    )

    log_file = tmp_path / "tokens" / (__import__("time").strftime("%Y%m%d") + ".jsonl")
    entry = json.loads(log_file.read_text().strip())
    assert entry["usage"]["reasoning_tokens"] == 23590
    assert entry["usage"]["total_tokens"] == 37878
    assert entry["usage"]["input_tokens"] == 13319
    assert entry["usage"]["output_tokens"] == 969


def test_log_token_usage_passes_through_cache_creation_tokens(tmp_path, monkeypatch):
    """cache_creation_tokens from Anthropic provider are preserved in log."""
    import json

    from solstone.think.models import log_token_usage

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    log_token_usage(
        model="claude-sonnet-4-5",
        usage={
            "input_tokens": 5000,
            "output_tokens": 1000,
            "total_tokens": 6000,
            "cached_tokens": 3000,
            "cache_creation_tokens": 2000,
        },
        context="test",
    )

    log_file = tmp_path / "tokens" / (__import__("time").strftime("%Y%m%d") + ".jsonl")
    entry = json.loads(log_file.read_text().strip())
    assert entry["usage"]["cache_creation_tokens"] == 2000
    assert entry["usage"]["cached_tokens"] == 3000


def test_log_token_usage_logs_append_failure(tmp_path, monkeypatch, caplog):
    import builtins

    from solstone.think.models import log_token_usage

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    real_open = builtins.open

    def fail_token_open(path, *args, **kwargs):
        if str(path).endswith(".jsonl"):
            raise OSError("disk full")
        return real_open(path, *args, **kwargs)

    monkeypatch.setattr(builtins, "open", fail_token_open)

    with caplog.at_level(logging.WARNING, logger="solstone.think.models"):
        log_token_usage(
            model="gpt-5.2",
            usage={"input_tokens": 1000, "output_tokens": 200},
            context="test",
        )

    assert "failed to log token usage" in caplog.text


class TestModelSupports:
    def test_opus_4_7_temperature_not_supported(self):
        assert model_supports(CLAUDE_OPUS_4, "temperature") is False

    def test_sonnet_4_6_temperature_supported(self):
        assert model_supports(CLAUDE_SONNET_4, "temperature") is True

    def test_unlisted_param_defaults_supported(self):
        assert model_supports(CLAUDE_OPUS_4, "max_tokens") is True

    def test_unlisted_model_defaults_supported(self):
        assert model_supports("gpt-5.5", "temperature") is True


def _native_generated_response(
    *, text: str = "native OK", finish_reason: str = "stop"
) -> dict:
    return {
        "schema": "solstone-generate-response-v2",
        "id": None,
        "outcome": "generated",
        "text": text,
        "model": "native-model",
        "usage": {"input_tokens": 2, "output_tokens": 1, "total_tokens": 3},
        "finish_reason": finish_reason,
        "thinking": None,
        "schema_validation": None,
        "input_budget": None,
        "request_budget": None,
        "inference": None,
    }


class _NativeOneShotProcess:
    def __init__(self, response: dict, captured: dict):
        self._response = response
        self._captured = captured
        self.returncode = 0

    def communicate(self, input_text: str) -> tuple[str, str]:
        self._captured["request"] = json.loads(input_text)
        return json.dumps(self._response), ""


def test_native_generate_sync_round_trip_never_resolves_provider(monkeypatch):
    from solstone.think import generate_client

    captured: dict = {}

    def fail_if_called(*_args, **_kwargs):
        raise AssertionError(
            "native generate client must not use Python generate internals"
        )

    def fake_popen(args, **_kwargs):
        captured["args"] = args
        return _NativeOneShotProcess(_native_generated_response(), captured)

    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(generate_client.subprocess, "Popen", fake_popen)
    monkeypatch.setattr(models_module, "resolve_provider", fail_if_called)
    monkeypatch.setattr(models_module, "log_token_usage", fail_if_called)

    result = generate_with_result("hello", "test.context")
    assert result["text"] == "native OK"
    assert generate("hello", "test.context") == "native OK"
    assert captured["args"] == ["/native/core", "generate", "--one-shot"]
    assert captured["request"]["context"] == "test.context"
    assert captured["request"]["contents"] == [{"type": "text", "text": "hello"}]
    assert "provider" not in captured["request"]
    assert "model" not in captured["request"]


def test_native_generate_child_environment_does_not_mutate_parent(monkeypatch):
    from solstone.think import generate_client

    captured: dict = {}

    def fake_popen(args, **kwargs):
        captured["args"] = args
        captured["environment"] = kwargs["env"]
        return _NativeOneShotProcess(_native_generated_response(), captured)

    monkeypatch.delenv("SOLSTONE_GENERATE_API_KEY_OVERRIDE", raising=False)
    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(generate_client.subprocess, "Popen", fake_popen)

    generate_client.generate_with_result(
        "hello",
        "test.context",
        child_environment={"SOLSTONE_GENERATE_API_KEY_OVERRIDE": "candidate-secret"},
    )

    assert captured["environment"]["SOLSTONE_GENERATE_API_KEY_OVERRIDE"] == (
        "candidate-secret"
    )
    assert "SOLSTONE_GENERATE_API_KEY_OVERRIDE" not in os.environ


def test_native_generate_async_round_trip_uses_async_subprocess(monkeypatch):
    from solstone.think import generate_client

    captured: dict = {}

    class AsyncNativeOneShotProcess:
        returncode = 0

        async def communicate(self, input_bytes: bytes) -> tuple[bytes, bytes]:
            captured["request"] = json.loads(input_bytes)
            return json.dumps(_native_generated_response()).encode(), b""

    async def fake_create_subprocess_exec(*args, **_kwargs):
        captured["args"] = args
        return AsyncNativeOneShotProcess()

    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(
        generate_client.asyncio,
        "create_subprocess_exec",
        fake_create_subprocess_exec,
    )

    result = asyncio.run(agenerate_with_result("hello", "test.context"))
    assert result["text"] == "native OK"
    assert asyncio.run(agenerate("hello", "test.context")) == "native OK"
    assert captured["args"] == ("/native/core", "generate", "--one-shot")
    assert captured["request"]["contents"] == [{"type": "text", "text": "hello"}]


def test_native_generate_refusal_preserves_native_classification(monkeypatch):
    from solstone.think import generate_client

    contract = generate_client._generate_contract()
    refusal = dict(
        next(
            vector["response"]
            for vector in contract["conformance_vectors"]
            if vector.get("source", {}).get("exception") == "NoBrainConfiguredError"
        )
    )
    refusal["id"] = None
    captured: dict = {}

    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(
        generate_client.subprocess,
        "Popen",
        lambda *_args, **_kwargs: _NativeOneShotProcess(refusal, captured),
    )

    with pytest.raises(NoBrainConfiguredError) as raised:
        generate("hello", "test.context")

    assert raised.value.reason == refusal["reason"]
    assert raised.value.blocking is refusal["blocking"]
    assert raised.value.retryable is refusal["retryable"]


def test_native_generate_non_json_truncation_returns_text(monkeypatch):
    from solstone.think import generate_client

    captured: dict = {}
    response = _native_generated_response(
        text="truncated but usable", finish_reason="length"
    )

    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(
        generate_client.subprocess,
        "Popen",
        lambda *_args, **_kwargs: _NativeOneShotProcess(response, captured),
    )

    assert (
        generate("hello", "test.context", json_output=False) == "truncated but usable"
    )


def test_native_generate_provider_invalid_refusals_preserve_wire_classification(
    monkeypatch,
):
    from solstone.think import generate_client

    contract = generate_client._generate_contract()
    blank_stop_refusal = dict(
        next(
            vector["response"]
            for vector in contract["conformance_vectors"]
            if vector.get("source", {}).get("exception")
            == "ProviderResponseInvalidError"
        )
    )
    blank_stop_refusal["id"] = None
    unrelated_provider_failure = {
        **blank_stop_refusal,
        "detail": "mocked malformed provider response",
    }
    captured: dict = {}
    refusals = iter((blank_stop_refusal, unrelated_provider_failure))

    monkeypatch.setattr(generate_client, "_native_binary", lambda: Path("/native/core"))
    monkeypatch.setattr(
        generate_client.subprocess,
        "Popen",
        lambda *_args, **_kwargs: _NativeOneShotProcess(next(refusals), captured),
    )

    with pytest.raises(ProviderResponseInvalidError) as blank_stop:
        generate("hello", "test.context")
    with pytest.raises(ProviderResponseInvalidError) as provider_failure:
        generate("hello", "test.context")

    for raised, refusal in (
        (blank_stop, blank_stop_refusal),
        (provider_failure, unrelated_provider_failure),
    ):
        assert raised.value.reason == refusal["reason"]
        assert raised.value.reason_code == refusal["reason_code"]
        assert raised.value.blocking is refusal["blocking"]
        assert raised.value.retryable is refusal["retryable"]
    assert blank_stop.value.reason == provider_failure.value.reason
