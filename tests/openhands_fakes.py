# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import inspect
import sys
import types
from types import SimpleNamespace
from typing import Any


class FakeModel:
    def __init__(self, **kwargs: Any) -> None:
        self.__dict__.update(kwargs)

    def model_dump(self, mode: str = "json") -> dict[str, Any]:
        del mode
        return {key: _dump(value) for key, value in self.__dict__.items()}


def _dump(value: Any) -> Any:
    if isinstance(value, SimpleNamespace):
        return {key: _dump(item) for key, item in vars(value).items()}
    if isinstance(value, FakeModel):
        return value.model_dump()
    if isinstance(value, list):
        return [_dump(item) for item in value]
    if isinstance(value, tuple):
        return [_dump(item) for item in value]
    if isinstance(value, dict):
        return {str(key): _dump(item) for key, item in value.items()}
    if isinstance(value, str | int | float | bool | type(None)):
        return value
    return repr(value)


class Action(FakeModel):
    pass


class Observation(FakeModel):
    @classmethod
    def from_text(
        cls,
        text: str,
        is_error: bool = False,
        **kwargs: Any,
    ) -> Observation:
        return cls(text=text, is_error=is_error, **kwargs)


class ToolExecutor:
    def __call__(self, action: Action, conversation: Any = None) -> Observation:
        raise NotImplementedError


class ToolDefinition:
    name = ""

    def __init_subclass__(cls, **kwargs: Any) -> None:
        super().__init_subclass__(**kwargs)
        if "name" not in cls.__dict__:
            cls.name = cls.__name__.removesuffix("Tool").lower()

    def __init__(self, **kwargs: Any) -> None:
        self.__dict__.update(kwargs)
        self.name = self.__class__.name

    def __class_getitem__(cls, _item: Any) -> type[ToolDefinition]:
        return cls

    @classmethod
    def create(cls, *args: Any, **kwargs: Any) -> list[Any]:
        del args, kwargs
        return []

    def action_from_arguments(self, arguments: dict[str, Any]) -> Action:
        return self.action_type(**arguments)

    def __call__(self, action: Action, conversation: Any = None) -> Observation:
        return self.executor(action, conversation)


class ToolAnnotations(FakeModel):
    pass


class FinishTool:
    @classmethod
    def create(cls, *args: Any, **kwargs: Any) -> list[Any]:
        del args, kwargs
        return [cls()]


class Tool(FakeModel):
    pass


_REGISTERED_TOOLS: dict[str, Any] = {}


def register_tool(name: str, factory: Any) -> None:
    _REGISTERED_TOOLS[name] = factory


class TokenUsage(FakeModel):
    prompt_tokens = 0
    completion_tokens = 0
    cache_read_tokens = 0
    cache_write_tokens = 0
    reasoning_tokens = 0
    per_turn_token = 0


class Metrics(FakeModel):
    def __init__(self) -> None:
        super().__init__(
            accumulated_cost=0.0,
            accumulated_token_usage=TokenUsage(),
            token_usages=[],
        )


def _direct_generation_response(model: str) -> SimpleNamespace:
    message = SimpleNamespace(
        content=[SimpleNamespace(text="fake response")],
        thinking_blocks=[],
        reasoning_content=None,
        responses_reasoning_item=None,
    )
    return SimpleNamespace(
        message=message,
        metrics=Metrics(),
        raw_response={
            "model": model,
            "choices": [{"finish_reason": "stop"}],
        },
    )


class LLM(FakeModel):
    instances: list[LLM] = []

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self.metrics = Metrics()
        self.effective_max_input_tokens = None
        self.last_completion_messages = None
        self.last_completion_kwargs = None
        self.last_completion_merged_kwargs = None
        self.last_responses_messages = None
        self.last_responses_kwargs = None
        self.last_responses_merged_kwargs = None
        type(self).instances.append(self)

    def completion(self, messages: Any, **kwargs: Any) -> SimpleNamespace:
        merged = dict(timeout=self.timeout, **kwargs)
        self.last_completion_messages = messages
        self.last_completion_kwargs = dict(kwargs)
        self.last_completion_merged_kwargs = merged
        return _direct_generation_response(self.model)

    def responses(self, messages: Any, **kwargs: Any) -> SimpleNamespace:
        merged = {"timeout": self.timeout, **kwargs}
        self.last_responses_messages = messages
        self.last_responses_kwargs = dict(kwargs)
        self.last_responses_merged_kwargs = merged
        return _direct_generation_response(self.model)

    async def acompletion(self, messages: Any, **kwargs: Any) -> SimpleNamespace:
        return self.completion(messages, **kwargs)

    async def aresponses(self, messages: Any, **kwargs: Any) -> SimpleNamespace:
        return self.responses(messages, **kwargs)


class Agent(FakeModel):
    pass


class Conversation(FakeModel):
    arun_impl = None
    instances: list[Conversation] = []

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self.messages: list[str] = []
        self.paused = False
        self.interrupted = False
        self.closed = False
        self.state = SimpleNamespace(execution_status=None)
        type(self).instances.append(self)

    def send_message(self, message: str) -> None:
        self.messages.append(message)

    def pause(self) -> None:
        self.paused = True

    def interrupt(self) -> None:
        self.interrupted = True
        self.state.execution_status = "paused"

    def close(self) -> None:
        self.closed = True

    async def arun(self) -> None:
        if type(self).arun_impl is None:
            return
        result = type(self).arun_impl(self)
        if inspect.isawaitable(result):
            await result


class ActionEvent(FakeModel):
    pass


class ObservationEvent(FakeModel):
    pass


class MessageEvent(FakeModel):
    pass


class AgentErrorEvent(FakeModel):
    pass


class ConversationErrorEvent(FakeModel):
    pass


def install_fake_openhands(monkeypatch: Any) -> types.SimpleNamespace:
    Conversation.instances = []
    Conversation.arun_impl = None
    LLM.instances = []
    _REGISTERED_TOOLS.clear()

    root_mod = types.ModuleType("openhands")
    sdk_mod = types.ModuleType("openhands.sdk")
    event_mod = types.ModuleType("openhands.sdk.event")
    conversation_error_mod = types.ModuleType("openhands.sdk.event.conversation_error")
    tool_mod = types.ModuleType("openhands.sdk.tool")
    schema_mod = types.ModuleType("openhands.sdk.tool.schema")
    builtins_mod = types.ModuleType("openhands.sdk.tool.builtins")
    finish_mod = types.ModuleType("openhands.sdk.tool.builtins.finish")
    registry_mod = types.ModuleType("openhands.sdk.tool.registry")
    spec_mod = types.ModuleType("openhands.sdk.tool.spec")

    sdk_mod.LLM = LLM
    sdk_mod.Agent = Agent
    sdk_mod.Conversation = Conversation
    event_mod.ActionEvent = ActionEvent
    event_mod.ObservationEvent = ObservationEvent
    event_mod.MessageEvent = MessageEvent
    event_mod.AgentErrorEvent = AgentErrorEvent
    conversation_error_mod.ConversationErrorEvent = ConversationErrorEvent
    tool_mod.ToolDefinition = ToolDefinition
    tool_mod.ToolExecutor = ToolExecutor
    tool_mod.ToolAnnotations = ToolAnnotations
    schema_mod.Action = Action
    schema_mod.Observation = Observation
    finish_mod.FinishTool = FinishTool
    registry_mod.register_tool = register_tool
    spec_mod.Tool = Tool

    root_mod.sdk = sdk_mod
    sdk_mod.event = event_mod
    sdk_mod.tool = tool_mod
    event_mod.conversation_error = conversation_error_mod
    tool_mod.schema = schema_mod
    tool_mod.builtins = builtins_mod
    tool_mod.registry = registry_mod
    tool_mod.spec = spec_mod
    builtins_mod.finish = finish_mod

    modules = {
        "openhands": root_mod,
        "openhands.sdk": sdk_mod,
        "openhands.sdk.event": event_mod,
        "openhands.sdk.event.conversation_error": conversation_error_mod,
        "openhands.sdk.tool": tool_mod,
        "openhands.sdk.tool.schema": schema_mod,
        "openhands.sdk.tool.builtins": builtins_mod,
        "openhands.sdk.tool.builtins.finish": finish_mod,
        "openhands.sdk.tool.registry": registry_mod,
        "openhands.sdk.tool.spec": spec_mod,
    }
    for name, module in modules.items():
        monkeypatch.setitem(sys.modules, name, module)

    return types.SimpleNamespace(
        Action=Action,
        Observation=Observation,
        ActionEvent=ActionEvent,
        ObservationEvent=ObservationEvent,
        MessageEvent=MessageEvent,
        AgentErrorEvent=AgentErrorEvent,
        ConversationErrorEvent=ConversationErrorEvent,
        Conversation=Conversation,
        LLM=LLM,
    )
