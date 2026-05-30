# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Manual journal config migration from ollama to local provider."""

from __future__ import annotations

import argparse
import copy
import json
from typing import Any

from solstone.think.journal_config import read_journal_config, write_journal_config
from solstone.think.models import LOCAL_MODEL

OLD_OLLAMA_PRO = "ollama-local/qwen3.5:35b-a3b-bf16"
OLD_OLLAMA_FLASH = "ollama-local/qwen3.5:9b"
OLD_OLLAMA_LITE = "ollama-local/qwen3.5:2b"

OLD_MODEL_MAP = {
    OLD_OLLAMA_PRO: LOCAL_MODEL,
    OLD_OLLAMA_FLASH: LOCAL_MODEL,
    OLD_OLLAMA_LITE: LOCAL_MODEL,
}


def _change(
    changes: list[dict[str, Any]],
    path: str,
    old: Any,
    new: Any,
    *,
    warning: str | None = None,
) -> None:
    item = {"path": path, "old": old, "new": new}
    if warning:
        item["warning"] = warning
    changes.append(item)


def _rewrite_model_string(value: str) -> tuple[str, str | None]:
    mapped = OLD_MODEL_MAP.get(value)
    if mapped:
        return mapped, None
    if value.startswith("ollama-local/"):
        return "local/" + value[len("ollama-local/") :], "unsupported_model"
    return value, None


def _child_path(path: str, key: str) -> str:
    return f"{path}.{key}" if path else key


def _rewrite_model_values(value: Any, path: str, changes: list[dict[str, Any]]) -> Any:
    if isinstance(value, str):
        new_value, warning = _rewrite_model_string(value)
        if new_value != value:
            _change(changes, path, value, new_value, warning=warning)
        return new_value
    if isinstance(value, list):
        return [
            _rewrite_model_values(item, f"{path}[{idx}]", changes)
            for idx, item in enumerate(value)
        ]
    if isinstance(value, dict):
        return {
            key: _rewrite_model_values(item, _child_path(path, str(key)), changes)
            for key, item in value.items()
        }
    return value


def _rewrite_provider_type(
    providers: dict[str, Any],
    agent_type: str,
    changes: list[dict[str, Any]],
) -> None:
    section = providers.get(agent_type)
    if not isinstance(section, dict):
        return
    for key in ("provider", "backup"):
        if section.get(key) == "ollama":
            section[key] = "local"
            _change(
                changes,
                f"providers.{agent_type}.{key}",
                "ollama",
                "local",
            )


def _move_simple_provider_key(
    providers: dict[str, Any],
    key: str,
    changes: list[dict[str, Any]],
) -> None:
    section = providers.get(key)
    if not isinstance(section, dict) or "ollama" not in section:
        return
    old_value = section.pop("ollama")
    if "local" not in section:
        section["local"] = old_value
        _change(changes, f"providers.{key}.ollama", old_value, old_value)
        return
    if section["local"] != old_value:
        _change(
            changes,
            f"providers.{key}.ollama",
            old_value,
            section["local"],
            warning="conflict_local_kept",
        )


def _move_models_block(
    providers: dict[str, Any], changes: list[dict[str, Any]]
) -> None:
    models = providers.get("models")
    if not isinstance(models, dict) or "ollama" not in models:
        return
    old_block = models.pop("ollama")
    if not isinstance(old_block, dict):
        if "local" not in models:
            models["local"] = old_block
        _change(changes, "providers.models.ollama", old_block, models.get("local"))
        return

    rewritten_old = {
        tier: _rewrite_model_values(value, f"providers.models.ollama.{tier}", changes)
        for tier, value in old_block.items()
    }
    local_block = models.get("local")
    if not isinstance(local_block, dict):
        models["local"] = rewritten_old
        _change(changes, "providers.models.ollama", old_block, rewritten_old)
        return

    for tier, value in rewritten_old.items():
        if tier not in local_block:
            local_block[tier] = value
            _change(
                changes,
                f"providers.models.local.{tier}",
                None,
                value,
            )
        elif local_block[tier] != value:
            _change(
                changes,
                f"providers.models.ollama.{tier}",
                value,
                local_block[tier],
                warning="conflict_local_kept",
            )


def _rewrite_context_providers(
    providers: dict[str, Any],
    changes: list[dict[str, Any]],
) -> None:
    contexts = providers.get("contexts")
    if not isinstance(contexts, dict):
        return
    for pattern, context_config in contexts.items():
        if not isinstance(context_config, dict):
            continue
        if context_config.get("provider") == "ollama":
            context_config["provider"] = "local"
            _change(
                changes,
                f"providers.contexts.{pattern}.provider",
                "ollama",
                "local",
            )


def migrate_config(config: dict[str, Any]) -> tuple[dict[str, Any], dict[str, Any]]:
    migrated = copy.deepcopy(config)
    changes: list[dict[str, Any]] = []
    providers = migrated.setdefault("providers", {})
    if not isinstance(providers, dict):
        return migrated, {"changed": False, "changes": []}

    _rewrite_provider_type(providers, "generate", changes)
    _rewrite_provider_type(providers, "cogitate", changes)
    _move_models_block(providers, changes)
    _move_simple_provider_key(providers, "auth", changes)
    _move_simple_provider_key(providers, "key_validation", changes)
    _rewrite_context_providers(providers, changes)

    migrated = _rewrite_model_values(migrated, "", changes)
    return migrated, {"changed": bool(changes), "changes": changes}


def run_migration(*, commit: bool = False) -> dict[str, Any]:
    config = read_journal_config()
    migrated, report = migrate_config(config)
    report["committed"] = False
    if commit and report["changed"]:
        write_journal_config(migrated)
        report["committed"] = True
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--commit", action="store_true", help="Persist rewrites.")
    parser.add_argument("--json", action="store_true", help="Print JSON output.")
    args = parser.parse_args()
    report = run_migration(commit=args.commit)
    if args.json:
        print(json.dumps(report, indent=2))
    elif report["changed"]:
        print("REPORT ONLY - pass --commit to persist.")
        for change in report["changes"]:
            print(f"{change['path']}: {change['old']!r} -> {change['new']!r}")
    else:
        print("No ollama config entries found.")


if __name__ == "__main__":
    main()
