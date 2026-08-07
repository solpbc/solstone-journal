# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib.util
from pathlib import Path


def _load_entity_describe_module():
    path = (
        Path(__file__).resolve().parents[1]
        / "solstone"
        / "apps"
        / "entities"
        / "talent"
        / "entity_describe.py"
    )
    spec = importlib.util.spec_from_file_location("test_entity_describe_hook", path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _prompt(current: str = "Existing description") -> str:
    return "\n".join(
        [
            "Entity Type: Person",
            "Entity Name: Alice Example",
            "Facet: work",
            f"Current Description: {current}",
        ]
    )


def test_entity_describe_pre_hook_renders_found_evidence(monkeypatch):
    module = _load_entity_describe_module()
    calls = []

    def fake_search(query, **kwargs):
        calls.append((query, kwargs))
        return (
            1,
            [
                {
                    "id": "20260422/work/090000_300/talents/sense.md:0",
                    "text": "Alice Example led the rollout planning.",
                    "metadata": {
                        "day": "20260422",
                        "facet": "work",
                    },
                }
            ],
        )

    monkeypatch.setattr(module, "search_journal", fake_search)

    vars_ = module.pre_process({"prompt": _prompt()})["template_vars"]

    assert vars_["entity_type"] == "Person"
    assert vars_["entity_name"] == "Alice Example"
    assert vars_["facet"] == "work"
    assert vars_["current_description"] == "Existing description"
    assert "Alice Example led the rollout planning." in vars_["evidence"]
    assert calls == [
        ("Alice Example", {"limit": 5, "facet": "work", "include_total": False})
    ]


def test_entity_describe_pre_hook_empty_evidence_preserves_generic_inputs(
    monkeypatch,
):
    module = _load_entity_describe_module()
    monkeypatch.setattr(module, "search_journal", lambda *_args, **_kwargs: (0, []))

    vars_ = module.pre_process({"prompt": _prompt("(none)")})["template_vars"]

    assert vars_["entity_type"] == "Person"
    assert vars_["entity_name"] == "Alice Example"
    assert vars_["facet"] == "work"
    assert vars_["current_description"] == "(none)"
    assert vars_["evidence"] == "No journal evidence found for this entity."


def test_entity_describe_ad_hoc_generate_has_no_output_path():
    from solstone.think.talents import prepare_config

    config = prepare_config(
        {
            "name": "entities:entity_describe",
            "prompt": _prompt(),
        }
    )

    assert config["type"] == "generate"
    assert config["output"] == "md"
    assert "output_path" not in config
