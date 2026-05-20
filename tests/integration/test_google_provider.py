# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Integration test for Google provider with real API calls."""

import json
import os
import subprocess
from pathlib import Path

import pytest
from dotenv import load_dotenv

from solstone.think.models import GEMINI_FLASH, GEMINI_PRO


def get_fixtures_env():
    """Load the tests/fixtures/.env file and return the environment."""
    fixtures_env = Path(__file__).parent.parent / "fixtures" / ".env"
    if not fixtures_env.exists():
        return None, None, None

    # Load the env file
    load_dotenv(fixtures_env, override=True)

    api_key = os.getenv("GOOGLE_API_KEY")
    journal_path = os.getenv("SOLSTONE_JOURNAL")

    return fixtures_env, api_key, journal_path


@pytest.mark.integration
@pytest.mark.requires_api
def test_google_provider_basic():
    """Test Google provider with basic prompt."""
    fixtures_env, api_key, journal_path = get_fixtures_env()

    if not fixtures_env:
        pytest.skip("tests/fixtures/.env not found")

    if not api_key:
        pytest.skip("GOOGLE_API_KEY not found in tests/fixtures/.env file")

    if not journal_path:
        pytest.skip("SOLSTONE_JOURNAL not found in tests/fixtures/.env file")

    # Prepare environment
    env = os.environ.copy()
    env["SOLSTONE_JOURNAL"] = journal_path
    env["GOOGLE_API_KEY"] = api_key

    # Create NDJSON input (no tool config)
    ndjson_input = json.dumps(
        {
            "prompt": "what is 1+1? Just give me the number.",
            "provider": "google",
            "name": "default",
            "model": GEMINI_FLASH,
            "max_output_tokens": 100,
        }
    )

    # Run the sol solstone.think.talents command
    cmd = ["sol", "providers", "check"]
    result = subprocess.run(
        cmd,
        env=env,
        input=ndjson_input,
        capture_output=True,
        text=True,
        timeout=30,
    )

    # Check that the command succeeded
    assert result.returncode == 0, f"Command failed with stderr: {result.stderr}"

    # Parse stdout events (should be JSONL format)
    stdout_lines = result.stdout.strip().split("\n")
    events = []
    for line in stdout_lines:
        if line:
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError as e:
                pytest.fail(f"Failed to parse JSON line: {line}\nError: {e}")

    # Verify we have events
    assert len(events) >= 2, (
        f"Expected at least start and finish events, got {len(events)}"
    )

    # Check start event
    start_event = events[0]
    assert start_event["event"] == "start"
    assert start_event["prompt"] == "what is 1+1? Just give me the number."
    assert start_event["model"] == GEMINI_FLASH
    assert start_event["name"] == "default"
    assert isinstance(start_event["ts"], int)

    # Check finish event
    finish_event = events[-1]
    assert finish_event["event"] == "finish"
    assert isinstance(finish_event["ts"], int)
    assert "result" in finish_event

    # The result should contain "2"
    result_text = finish_event["result"].lower()
    assert "2" in result_text or "two" in result_text, (
        f"Expected '2' in response, got: {finish_event['result']}"
    )

    # Check for no errors
    error_events = [e for e in events if e.get("event") == "error"]
    assert len(error_events) == 0, f"Found error events: {error_events}"

    # Verify stderr has no errors (warnings about thought_signature are OK)
    if result.stderr:
        assert (
            "error" not in result.stderr.lower() or "thought_signature" in result.stderr
        ), f"Unexpected stderr content: {result.stderr}"


@pytest.mark.integration
@pytest.mark.requires_api
def test_google_provider_with_thinking():
    """Test Google provider with thinking enabled."""
    fixtures_env, api_key, journal_path = get_fixtures_env()

    if not fixtures_env:
        pytest.skip("tests/fixtures/.env not found")

    if not api_key:
        pytest.skip("GOOGLE_API_KEY not found in tests/fixtures/.env file")

    if not journal_path:
        pytest.skip("SOLSTONE_JOURNAL not found in tests/fixtures/.env file")

    # Prepare environment
    env = os.environ.copy()
    env["SOLSTONE_JOURNAL"] = journal_path
    env["GOOGLE_API_KEY"] = api_key

    # Create NDJSON input with thinking model (if available)
    ndjson_input = json.dumps(
        {
            "prompt": "What is the square root of 16? Just the number please.",
            "provider": "google",
            "name": "default",
            "model": GEMINI_PRO,  # Pro model for thinking
            "max_output_tokens": 2000,
        }
    )

    # Run the sol solstone.think.talents command
    cmd = ["sol", "providers", "check"]
    result = subprocess.run(
        cmd,
        env=env,
        input=ndjson_input,
        capture_output=True,
        text=True,
        timeout=30,
    )

    # Allow for model unavailability
    if result.returncode != 0:
        if (
            "model not found" in result.stderr.lower()
            or "invalid model" in result.stderr.lower()
        ):
            pytest.skip("Thinking model not available")
        assert False, f"Command failed with stderr: {result.stderr}"

    # Parse events
    stdout_lines = result.stdout.strip().split("\n")
    events = [json.loads(line) for line in stdout_lines if line]

    # Check for thinking events (may be present with thinking models)
    # thinking_events may be present with thinking models (not asserted)
    # With thinking models, we might get thinking events

    # Verify the answer is correct
    finish_event = events[-1]

    # Check if this was an API error (intermittent failures)
    if finish_event.get("event") == "error":
        error_msg = finish_event.get("error", "Unknown error")
        trace = finish_event.get("trace", "")
        if (
            "quota" in error_msg.lower()
            or "rate" in error_msg.lower()
            or "retry" in error_msg.lower()
        ):
            pytest.skip(f"Intermittent Google API error: {error_msg}")
        else:
            pytest.fail(f"Unexpected error: {error_msg}\nTrace: {trace}")

    assert finish_event["event"] == "finish", (
        f"Expected finish event, got: {finish_event}"
    )
    assert "result" in finish_event, f"No result in finish event: {finish_event}"
    if finish_event["result"]:
        result_text = finish_event["result"].lower()
        assert "4" in result_text or "four" in result_text, (
            f"Expected '4' in response, got: {finish_event['result']}"
        )


@pytest.mark.integration
@pytest.mark.requires_api
def test_google_json_truncation_detection():
    """Test that Google provider detects JSON response truncation via finish_reason.

    Uses a very small max_output_tokens to force truncation, verifying that
    the provider returns finish_reason='max_tokens' which callers can use
    to detect incomplete responses.
    """
    fixtures_env, api_key, _ = get_fixtures_env()

    if not fixtures_env:
        pytest.skip("tests/fixtures/.env not found")

    if not api_key:
        pytest.skip("GOOGLE_API_KEY not found in tests/fixtures/.env file")

    # Import provider directly for this test
    from solstone.think.providers import google as google_provider

    # Request JSON output with very small token limit to force truncation
    # Use run_generate which returns GenerateResult, then check finish_reason
    result = google_provider.run_generate(
        contents="Return a JSON array of the first 50 prime numbers.",
        model=GEMINI_FLASH,
        json_output=True,
        max_output_tokens=10,  # Too small to complete the response
    )

    # Verify truncation was detected via finish_reason
    assert result["finish_reason"] == "max_tokens", (
        f"Expected max_tokens finish_reason, got: {result['finish_reason']}"
    )
    # Partial text should be present
    assert isinstance(result["text"], str)
