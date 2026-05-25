# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import Any, Literal

logger = logging.getLogger(__name__)

NeedsYouKind = Literal["chat", "confirm", "route"]


@dataclass(frozen=True)
class NeedsYouItem:
    text: str
    kind: NeedsYouKind
    payload: dict[str, Any]

    def to_dict(self) -> dict[str, Any]:
        return {
            "text": self.text,
            "kind": self.kind,
            "payload": self.payload,
        }


def classify_needs_you(
    attention: Any,
    pulse_needs: list[Any],
    todos: list[dict[str, Any]],
) -> list[NeedsYouItem]:
    items: list[NeedsYouItem] = []

    if attention:
        item = _classify_safely("attention", attention, _classify_attention)
        if item is not None:
            items.append(item)

    for pulse_need in pulse_needs:
        item = _classify_safely("pulse need", pulse_need, _classify_pulse_need)
        if item is not None:
            items.append(item)

    for todo in todos[:7]:
        item = _classify_safely("todo", todo, _classify_todo)
        if item is not None:
            items.append(item)

    return items


def _classify_safely(
    label: str,
    value: Any,
    classifier: Any,
) -> NeedsYouItem | None:
    try:
        return classifier(value)
    except (TypeError, ValueError) as exc:
        logger.warning("omitting malformed needs-you %s: %s", label, exc)
        return None


def _classify_attention(attention: Any) -> NeedsYouItem:
    if isinstance(attention, dict):
        placeholder_text = attention.get("placeholder_text")
    else:
        placeholder_text = getattr(attention, "placeholder_text", None)
    text = _require_text(placeholder_text, "attention placeholder_text")
    return _chat_item(text, f"what happened with {text}?")


def _classify_pulse_need(item: Any) -> NeedsYouItem | None:
    if isinstance(item, dict):
        return _classify_generated_item(
            item,
            default_prompt=lambda text: f"let's dig into {text}",
        )
    text = _require_text(item, "pulse need")
    return _chat_item(text, f"let's dig into {text}")


def _classify_todo(todo: dict[str, Any]) -> NeedsYouItem:
    if not isinstance(todo, dict):
        raise TypeError("todo must be an object")
    text = _require_text(todo.get("text"), "todo text")
    return _chat_item(text, f"what's the context on: {text}")


def _classify_generated_item(
    item: dict[str, Any],
    *,
    default_prompt: Any,
) -> NeedsYouItem | None:
    text = _require_text(item.get("text"), "generated item text")
    kind = item.get("kind")
    payload = item.get("payload") or {}
    if not isinstance(payload, dict):
        raise TypeError("generated item payload must be an object")

    if kind == "chat":
        prompt = payload.get("prompt")
        if not isinstance(prompt, str) or not prompt.strip():
            prompt = default_prompt(text)
        return _chat_item(text, prompt)

    if kind == "confirm":
        return _chat_item(text, default_prompt(text))

    if kind == "route":
        route_payload = _normalize_route_payload(payload)
        if route_payload is None:
            return None
        return NeedsYouItem(text=text, kind="route", payload=route_payload)

    raise ValueError(f"unknown kind: {kind}")


def _normalize_route_payload(payload: Any) -> dict[str, str] | None:
    if not isinstance(payload, dict):
        logger.warning("omitting needs-you route with malformed payload")
        return None
    href = payload.get("href")
    if not isinstance(href, str) or not href.startswith("/") or href.startswith("//"):
        logger.warning("omitting needs-you route with off-origin href: %r", href)
        return None
    return {"href": href}


def _chat_item(text: str, prompt: str) -> NeedsYouItem:
    return NeedsYouItem(text=text, kind="chat", payload={"prompt": prompt})


def _require_text(value: Any, label: str) -> str:
    if not isinstance(value, str):
        raise TypeError(f"{label} must be a string")
    text = value.strip()
    if not text:
        raise ValueError(f"{label} is empty")
    return text
