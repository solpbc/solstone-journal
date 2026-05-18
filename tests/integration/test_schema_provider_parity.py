# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Raw-SDK strict schema parity tests for req_bfbdbux6 portable schemas."""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any, Callable

import pytest
from dotenv import load_dotenv
from jsonschema import Draft202012Validator

from solstone.apps.timeline.rollup import build_rollup_schema
from solstone.think.models import CLAUDE_SONNET_4, GEMINI_FLASH, GPT_5

REPO_ROOT = Path(__file__).resolve().parents[2]
FACET_SENTINEL = "__RUNTIME_FACETS__"

PORTABLE_SCHEMAS = [
    (
        "describe",
        "solstone/observe/describe.schema.json",
        "Categorize a hypothetical code-editor-with-terminal screenshot.",
    ),
    (
        "extract",
        "solstone/observe/extract.schema.json",
        "Select frame ids 1 and 3 from hypothetical screen frames. Return frame_ids [1, 3].",
    ),
    (
        "meeting",
        "solstone/observe/categories/meeting.schema.json",
        "Hypothetical 2-person Zoom: Alice speaking video-on box [10,20,30,40]; "
        "Bob muted video-off; no screen share.",
    ),
    (
        "enrich",
        "solstone/observe/enrich.schema.json",
        "Enrich a hypothetical transcript: one corrected statement, neutral emotion; "
        "topics/setting/warning short.",
    ),
    (
        "transcribe_gemini",
        "solstone/observe/transcribe/gemini.schema.json",
        "One transcript segment: start '00:05', speaker 'Alice', text 'hello'.",
    ),
    (
        "detect_created",
        "solstone/think/detect_created.schema.json",
        "Detect created date: day '20260518', time '143000', confidence high, "
        "source 'header', utc false.",
    ),
    (
        "detect_transcript_json",
        "solstone/think/detect_transcript_json.schema.json",
        "One entry: start '00:00:05', speaker 'Alice', text 'hi'; "
        "topics/setting short.",
    ),
    (
        "detect_transcript_segment",
        "solstone/think/detect_transcript_segment.schema.json",
        "Segment a hypothetical transcript into two boundaries: 12:00:00 line 1 "
        "and 12:05:00 line 3.",
    ),
    (
        "daily_schedule",
        "solstone/talent/daily_schedule.schema.json",
        "primary '09:00', fallback '13:30'.",
    ),
    (
        "entity_observer",
        "solstone/apps/entities/talent/entity_observer.schema.json",
        "One observation entity_id 'e1' with one item content 'c' reasoning 'r'; "
        "skipped []; summary 's'.",
    ),
    (
        "sense",
        "solstone/talent/sense.schema.json",
        "Hypothetical idle frame: density idle, content_type idle, summary 'idle', "
        "no entities, one facet, meeting_detected false, no speakers, all recommend "
        "false, emotional_register neutral.",
    ),
    (
        "story",
        "solstone/talent/story.schema.json",
        "Short story body 's', topics ['t'], confidence 0.5, no commitments/"
        "closures/decisions.",
    ),
    (
        "speaker_attribution",
        "solstone/talent/speaker_attribution.schema.json",
        "One attribution: sentence_id 1, speaker Alice, reasoning Introduced herself.",
    ),
    (
        "schedule",
        "solstone/talent/schedule.schema.json",
        "One future meeting event: target_date 2026-05-20, start 09:00:00, "
        "end 09:30:00, title Planning Call, description Planning call, details "
        "Google Meet, one attendee Alice from screen confidence 0.9 context "
        "calendar invite, participation_confidence 0.8, facet work, cancelled false.",
    ),
    (
        "segment_summary",
        "solstone/apps/timeline/talent/segment_summary.schema.json",
        "title 'Dev Env', description 'Sets up the environment.'",
    ),
    (
        "participation",
        "solstone/talent/participation.schema.json",
        "One participant: name 'Alice', role attendee, source voice, confidence 0.9, "
        "context 'c', entity_id null; participation_confidence 0.8.",
    ),
    (
        "participation_entry",
        "solstone/talent/participation_entry.schema.json",
        "name 'Alice', role attendee, source voice, confidence 0.9, context 'c', "
        "entity_id null.",
    ),
    (
        "chat",
        "solstone/talent/chat.schema.json",
        "Hypothetical chat backend turn. Owner asks for a synthesis of their last two "
        "weeks across the journal, which needs fresh multi-day lookup. Produce JSON "
        "only: message a one-sentence acknowledgement; notes a brief internal reason; "
        'talent_request non-null with target "exec", task a one-sentence synthesis '
        'request, and context exactly "{\\"window\\":\\"14d\\"}".',
    ),
    (
        "build_rollup_schema",
        "build_rollup_schema(3)",
        "Given four hypothetical timeline candidates, return picks [0, 2, 3] "
        "and a short rationale for choosing shipped schema portability, fixed CI, "
        "and updated docs.",
    ),
]


def get_fixtures_env(api_key_name: str):
    fixtures_env = Path(__file__).parent.parent / "fixtures" / ".env"
    if not fixtures_env.exists():
        return None, None, None
    load_dotenv(fixtures_env, override=True)
    return fixtures_env, os.getenv(api_key_name), os.getenv("SOLSTONE_JOURNAL")


def hydrate(node: Any) -> None:
    if isinstance(node, dict):
        if node.get("enum") == [FACET_SENTINEL]:
            node["enum"] = ["work", "personal", "health"]
        for value in node.values():
            hydrate(value)
    elif isinstance(node, list):
        for value in node:
            hydrate(value)


def load_schema(rel_path: str) -> dict[str, Any]:
    if rel_path == "build_rollup_schema(3)":
        schema = build_rollup_schema(3)
        hydrate(schema)
        return schema
    schema = json.loads((REPO_ROOT / rel_path).read_text(encoding="utf-8"))
    hydrate(schema)
    return schema


def conforms(schema: dict[str, Any], text: str) -> tuple[bool, str]:
    try:
        obj = json.loads(text)
    except Exception as exc:
        return False, f"not JSON: {exc}: {text[:120]!r}"
    errors = sorted(Draft202012Validator(schema).iter_errors(obj), key=str)
    if errors:
        return False, f"NONCONFORM: {errors[0].message[:160]}"
    return True, json.dumps(obj)[:140]


def call_anthropic(schema: dict[str, Any], prompt: str, api_key: str) -> str:
    from anthropic import Anthropic

    message = Anthropic(api_key=api_key).messages.create(
        model=CLAUDE_SONNET_4,
        max_tokens=2048,
        temperature=0.3,
        messages=[{"role": "user", "content": prompt + " Respond JSON only."}],
        output_config={"format": {"type": "json_schema", "schema": schema}},
    )
    return "".join(
        block.text
        for block in message.content
        if getattr(block, "type", None) == "text"
    )


def call_openai(schema: dict[str, Any], prompt: str, api_key: str) -> str:
    import openai

    response = openai.OpenAI(api_key=api_key).responses.create(
        model=GPT_5,
        input=prompt + " Respond JSON only.",
        max_output_tokens=2048,
        text={
            "format": {
                "type": "json_schema",
                "name": "r",
                "schema": schema,
                "strict": True,
            }
        },
    )
    return response.output_text or ""


_GOOGLE_CLIENTS: dict[str, Any] = {}


def call_google(schema: dict[str, Any], prompt: str, api_key: str) -> str:
    from google import genai
    from google.genai import types

    # Hold the client across parametrized cases. A per-call inline
    # ``genai.Client(...).models...`` temporary is closed before the request
    # completes ("Cannot send a request, as the client has been closed").
    client = _GOOGLE_CLIENTS.get(api_key)
    if client is None:
        client = genai.Client(api_key=api_key, vertexai=False)
        _GOOGLE_CLIENTS[api_key] = client

    response = client.models.generate_content(
        model=GEMINI_FLASH,
        contents=[prompt + " Respond JSON only."],
        config=types.GenerateContentConfig(
            temperature=0.3,
            max_output_tokens=2048,
            response_mime_type="application/json",
            response_json_schema=schema,
            thinking_config=types.ThinkingConfig(thinking_budget=0),
        ),
    )
    return response.text or ""


PROVIDERS: tuple[tuple[str, str, Callable[[dict[str, Any], str, str], str]], ...] = (
    ("google", "GOOGLE_API_KEY", call_google),
    ("openai-strict", "OPENAI_API_KEY", call_openai),
    ("anthropic-strict", "ANTHROPIC_API_KEY", call_anthropic),
)


@pytest.mark.integration
@pytest.mark.requires_api
@pytest.mark.parametrize(
    ("provider_name", "api_key_name", "caller"),
    [pytest.param(*provider, id=provider[0]) for provider in PROVIDERS],
)
@pytest.mark.parametrize(
    ("schema_name", "schema_path", "prompt"),
    [pytest.param(*schema_case, id=schema_case[0]) for schema_case in PORTABLE_SCHEMAS],
)
def test_schema_provider_parity(
    provider_name: str,
    api_key_name: str,
    caller: Callable[[dict[str, Any], str, str], str],
    schema_name: str,
    schema_path: str,
    prompt: str,
) -> None:
    fixtures_env, api_key, journal_path = get_fixtures_env(api_key_name)
    if not fixtures_env:
        pytest.skip("tests/fixtures/.env not found")
    if not api_key:
        pytest.skip(f"{api_key_name} not found in tests/fixtures/.env file")
    if not journal_path:
        pytest.skip("SOLSTONE_JOURNAL not found in tests/fixtures/.env file")

    schema = load_schema(schema_path)
    try:
        output = caller(schema, prompt, api_key)
    except Exception as exc:
        pytest.fail(
            f"{provider_name} rejected {schema_name}: {type(exc).__name__}: {exc}"
        )

    ok, detail = conforms(schema, output)
    assert ok, f"{provider_name} returned invalid {schema_name}: {detail}"
