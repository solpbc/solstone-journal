# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import sys
from typing import Any

TOOL_DESCRIPTION = """Terminal tool for ending the run with its final result.

Call this tool exactly once when the run is complete. The content argument is the final result the system should carry forward.

Artifact talents: when the talent produces an artifact, content is the complete artifact body itself, such as the markdown or text to save. Do not wrap it in commentary or describe the artifact instead of providing it.

Action talents: when the talent's work was done through side-effecting commands during the run, content is a concise, signal-carrying record of what changed, what was found, and why. Do not emit a bare "done".

No-op: call this tool even when no changes were needed. Emit a brief result explaining why nothing changed rather than ending silently.
"""


# Lazy cache for the openhands-derived EmitFinal* classes. The classes have to
# live at module level (i.e. without `<locals>` in their __qualname__ and
# discoverable as attributes on this module) because openhands-sdk persists tool
# events to disk and re-validates them via `Event.model_validate_json`, which
# rejects subclasses whose qualname contains "<locals>". OpenHands is installed
# on demand, so define the classes lazily and promote them into this module.
_EMIT_FINAL_TYPES: dict[str, Any] = {}


def _ensure_emit_final_types() -> dict[str, Any]:
    if _EMIT_FINAL_TYPES:
        return _EMIT_FINAL_TYPES

    from openhands.sdk.tool import ToolAnnotations, ToolDefinition, ToolExecutor
    from openhands.sdk.tool.schema import Action, Observation
    from pydantic import Field

    class EmitFinalAction(Action):
        content: str = Field(
            description=(
                "Final result to carry forward: artifact body or concise record "
                "of what changed."
            )
        )

    class EmitFinalObservation(Observation):
        pass

    class EmitFinalExecutor(ToolExecutor):
        def __call__(
            self,
            action: Any,
            conversation: Any = None,
        ) -> Any:
            del conversation
            return EmitFinalObservation.from_text(text=action.content)

    class EmitFinalTool(ToolDefinition[EmitFinalAction, EmitFinalObservation]):
        name = "emit_final"

        @classmethod
        def create(cls, *args: Any, **kwargs: Any) -> list[Any]:
            del args, kwargs
            return []

    # Promote the closure-defined classes onto this module so they look
    # module-level to openhands-sdk's serialization machinery. Without
    # this, `__qualname__` carries `<locals>` and re-deserializing tool
    # events fails inside stuck_detector with
    # "Local classes not supported".
    module = sys.modules[__name__]
    for cls in (
        EmitFinalAction,
        EmitFinalObservation,
        EmitFinalExecutor,
        EmitFinalTool,
    ):
        cls.__module__ = __name__
        cls.__qualname__ = cls.__name__
        setattr(module, cls.__name__, cls)

    _EMIT_FINAL_TYPES.update(
        EmitFinalAction=EmitFinalAction,
        EmitFinalObservation=EmitFinalObservation,
        EmitFinalExecutor=EmitFinalExecutor,
        EmitFinalTool=EmitFinalTool,
        ToolAnnotations=ToolAnnotations,
    )
    return _EMIT_FINAL_TYPES


def build_emit_final_tools() -> list[Any]:
    types = _ensure_emit_final_types()
    emit_final_action = types["EmitFinalAction"]
    emit_final_observation = types["EmitFinalObservation"]
    emit_final_executor_cls = types["EmitFinalExecutor"]
    emit_final_tool_cls = types["EmitFinalTool"]
    tool_annotations = types["ToolAnnotations"]

    tool = emit_final_tool_cls(
        description=TOOL_DESCRIPTION,
        action_type=emit_final_action,
        observation_type=emit_final_observation,
        executor=emit_final_executor_cls(),
        annotations=tool_annotations(
            title="emit_final",
            readOnlyHint=True,
            destructiveHint=False,
            idempotentHint=True,
            openWorldHint=False,
        ),
    )
    return [tool]
