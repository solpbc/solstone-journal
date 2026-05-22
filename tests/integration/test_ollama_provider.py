# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Integration tests for Ollama provider with a live local Ollama instance."""

import asyncio
import shutil

import pytest

from solstone.think.models import OLLAMA_LITE, OLLAMA_PRO

# Use the smallest available model for fast integration tests
_TEST_MODEL = OLLAMA_LITE


def _ollama_reachable() -> bool:
    """Check if the local Ollama instance is reachable."""
    try:
        from solstone.think.providers.ollama import validate_key

        return validate_key("ollama", "")["valid"]
    except Exception:
        return False


# Skip all tests in this module if Ollama is not running
pytestmark = [
    pytest.mark.integration,
    pytest.mark.skipif(
        not _ollama_reachable(),
        reason="Local Ollama instance not reachable",
    ),
]


class TestOllamaGenerate:
    def test_basic_generation(self):
        from solstone.think.providers.ollama import run_generate

        result = run_generate(
            "What is 2 + 2? Reply with just the number.",
            model=_TEST_MODEL,
            max_output_tokens=64,
            thinking_budget=0,
        )

        assert result["text"]
        assert "4" in result["text"]
        assert result["usage"] is not None
        assert result["usage"]["input_tokens"] > 0
        assert result["usage"]["output_tokens"] > 0
        assert result["finish_reason"] == "stop"
        # With think=False via native API, thinking should be absent
        assert result["thinking"] is None

    def test_system_instruction(self):
        from solstone.think.providers.ollama import run_generate

        result = run_generate(
            "What color is the sky?",
            model=_TEST_MODEL,
            max_output_tokens=64,
            system_instruction="Always respond in exactly one word.",
            thinking_budget=0,
        )

        assert result["text"]

    def test_json_output(self):
        from solstone.think.providers.ollama import run_generate

        result = run_generate(
            'Return a JSON object with key "answer" and value 42.',
            model=_TEST_MODEL,
            max_output_tokens=256,
            json_output=True,
            thinking_budget=0,
        )

        # The response should contain JSON content. Small models may wrap
        # it in markdown fences, so we check for the key rather than strict
        # parsing. (JSON validation is handled centrally by think/models.py.)
        assert result["text"]
        assert "answer" in result["text"]

    def test_thinking_enabled(self):
        from solstone.think.providers.ollama import run_generate

        result = run_generate(
            "What is 15 * 17?",
            model=_TEST_MODEL,
            max_output_tokens=512,
            thinking_budget=4096,
        )

        assert result["text"]
        assert result["usage"] is not None
        # With think=True, thinking content should be present
        assert result["thinking"] is not None
        assert len(result["thinking"]) > 0
        assert result["thinking"][0]["summary"]

    def test_thinking_disabled_no_reasoning(self):
        """Verify that think=False actually suppresses reasoning on the native API."""
        from solstone.think.providers.ollama import run_generate

        result = run_generate(
            "What is 2 + 2? Reply with just the number.",
            model=_TEST_MODEL,
            max_output_tokens=64,
            thinking_budget=0,
        )

        assert result["text"]
        assert result["thinking"] is None


class TestOllamaAgenerate:
    def test_async_generation(self):
        from solstone.think.providers.ollama import run_agenerate

        result = asyncio.run(
            run_agenerate(
                "What is 3 + 5? Reply with just the number.",
                model=_TEST_MODEL,
                max_output_tokens=64,
                thinking_budget=0,
            )
        )

        assert result["text"]
        assert "8" in result["text"]
        assert result["usage"] is not None


class TestOllamaListModels:
    def test_list_models(self):
        from solstone.think.providers.ollama import list_models

        models = list_models("ollama")
        assert isinstance(models, list)
        assert len(models) > 0
        # Native API returns models with "name" field
        assert "name" in models[0]


class TestOllamaValidateKey:
    def test_reachable(self):
        from solstone.think.providers.ollama import validate_key

        result = validate_key("ollama", "")
        assert result["valid"] is True


def _opencode_available() -> bool:
    """Check if the OpenCode CLI is installed."""
    return shutil.which("opencode") is not None


@pytest.mark.skipif(
    not _opencode_available(),
    reason="OpenCode CLI not installed",
)
class TestOllamaCogitate:
    def test_basic_cogitate(self):
        """Test cogitate with a simple prompt that doesn't require tool use."""
        from solstone.think.providers.ollama import run_cogitate

        events = []
        result = asyncio.run(
            run_cogitate(
                {
                    "prompt": "Say the word 'solstone' and nothing else.",
                    "model": OLLAMA_PRO,
                },
                on_event=lambda e: events.append(e),
            )
        )

        assert result

        # Should have emitted a finish event
        finish_events = [e for e in events if e.get("event") == "finish"]
        assert len(finish_events) == 1
        assert finish_events[0]["result"] == result

    def test_cogitate_with_tool_use(self):
        """Test cogitate with a prompt that triggers bash tool use."""
        from solstone.think.providers.ollama import run_cogitate

        events = []
        result = asyncio.run(
            run_cogitate(
                {
                    "prompt": "Use the bash tool to run 'echo solstone_test_ok' and tell me exactly what it output.",
                    "model": OLLAMA_PRO,
                },
                on_event=lambda e: events.append(e),
            )
        )

        assert result
        assert "solstone_test_ok" in result

        # Should have tool_start and tool_end events
        tool_starts = [e for e in events if e.get("event") == "tool_start"]
        tool_ends = [e for e in events if e.get("event") == "tool_end"]
        assert len(tool_starts) >= 1
        assert len(tool_ends) >= 1
