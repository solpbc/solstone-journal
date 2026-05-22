# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI entrypoint for provider connectivity checks."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

from solstone.think.utils import get_journal, require_solstone, setup_cli

_OPENHANDS_BACKED_PROVIDERS = {"anthropic", "openai", "google"}


def _provider_status(provider_name: str) -> dict[str, object]:
    from solstone.think.providers import build_provider_status, get_provider_list

    provider = next(
        (item for item in get_provider_list() if item["name"] == provider_name),
        None,
    )
    if provider is None:
        return {}
    return build_provider_status([provider], vertex_creds_configured=False).get(
        provider_name,
        {},
    )


def _check_generate(provider_name: str, tier: int, timeout: int) -> tuple[str, str]:
    """Check generate interface for a provider."""
    from solstone.think.models import PROVIDER_DEFAULTS
    from solstone.think.providers import PROVIDER_METADATA, get_provider_module

    env_key = PROVIDER_METADATA[provider_name]["env_key"]
    if env_key and not os.getenv(env_key):
        label = PROVIDER_METADATA[provider_name]["label"]
        return "skip", f"{label} not configured (no {env_key})"

    if not env_key:
        from solstone.think.providers import validate_key

        result = validate_key(provider_name, "")
        if not result.get("valid"):
            return (
                "skip",
                f"Ollama not reachable ({result.get('error', 'unreachable')})",
            )

    try:
        module = get_provider_module(provider_name)
        model = PROVIDER_DEFAULTS[provider_name][tier]
        result = module.run_generate(
            contents="Say OK",
            model=model,
            provider=provider_name,
            temperature=0,
            max_output_tokens=16,
            system_instruction=None,
            json_output=False,
            thinking_budget=None,
            timeout_s=timeout,
        )
        text = result.get("text", "") if isinstance(result, dict) else ""
        if text:
            usage = result.get("usage") if isinstance(result, dict) else None
            if usage:
                from solstone.think.models import log_token_usage

                log_token_usage(
                    model=PROVIDER_DEFAULTS[provider_name][tier],
                    usage=usage,
                    context="health.check.generate",
                    type="generate",
                )
            return "ok", "OK"
        return "fail", "FAIL: empty response text"
    except Exception as exc:
        return "fail", f"FAIL: {exc}"


async def _check_cogitate(
    provider_name: str, tier: int, timeout: int
) -> tuple[str, str]:
    """Check cogitate interface for a provider by running a real prompt."""
    from solstone.think.models import PROVIDER_DEFAULTS
    from solstone.think.providers import PROVIDER_METADATA, get_provider_module

    env_key = PROVIDER_METADATA[provider_name]["env_key"]
    if provider_name in _OPENHANDS_BACKED_PROVIDERS:
        label = PROVIDER_METADATA[provider_name]["label"]
        status = _provider_status(provider_name)
        if not status.get("configured"):
            return "skip", f"{label} not configured (no {env_key})"
        if not status.get("cogitate_cli_found"):
            binary = str(status.get("cogitate_cli") or "openhands-sdk")
            return "skip", f"{binary} runtime not installed"
        if not status.get("cogitate_ready"):
            issues = status.get("issues") or []
            message = "; ".join(str(issue) for issue in issues) or "cogitate not ready"
            return "skip", message
    elif env_key and not os.getenv(env_key):
        label = PROVIDER_METADATA[provider_name]["label"]
        return "skip", f"{label} not configured (no {env_key})"

    if not env_key:
        from solstone.think.providers import validate_key

        result = validate_key(provider_name, "")
        if not result.get("valid"):
            return (
                "skip",
                f"Ollama not reachable ({result.get('error', 'unreachable')})",
            )

    binary = PROVIDER_METADATA[provider_name].get("cogitate_cli", "")
    if binary and not shutil.which(binary):
        return "skip", f"{binary} CLI not installed"

    try:
        module = get_provider_module(provider_name)
        model = PROVIDER_DEFAULTS[provider_name][tier]
        config = {"prompt": "Say OK", "model": model, "provider": provider_name}
        result = await asyncio.wait_for(
            module.run_cogitate(config=config, on_event=None),
            timeout=timeout,
        )
        if result:
            return "ok", "OK"
        return "fail", "FAIL: empty response"
    except asyncio.TimeoutError:
        return "fail", f"FAIL: timed out after {timeout}s"
    except Exception as exc:
        return "fail", f"FAIL: {exc}"


async def _run_check(args: argparse.Namespace) -> None:
    """Run connectivity checks against AI providers."""
    from solstone.think.models import PROVIDER_DEFAULTS, TIER_FLASH, TIER_LITE, TIER_PRO
    from solstone.think.providers import PROVIDER_REGISTRY

    targeted_pairs = None
    if args.targeted and not args.provider and not args.tier:
        import fcntl

        from solstone.think.models import TYPE_DEFAULTS, get_backup_provider
        from solstone.think.utils import get_config

        targeted_pairs = set()
        config = get_config()
        providers_config = config.get("providers", {})
        for talent_type, defaults in TYPE_DEFAULTS.items():
            type_config = providers_config.get(talent_type, {})
            provider = type_config.get("provider", defaults["provider"])
            tier = type_config.get("tier", defaults["tier"])
            targeted_pairs.add((provider, tier))
            backup = get_backup_provider(talent_type)
            if backup:
                targeted_pairs.add((backup, tier))

        lock_dir = Path(get_journal()) / "health"
        lock_dir.mkdir(parents=True, exist_ok=True)
        lock_fd = open(lock_dir / "recheck.lock", "w")
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            lock_fd.close()
            return

    if args.provider:
        providers = args.provider
        for name in providers:
            if name not in PROVIDER_REGISTRY:
                available = ", ".join(PROVIDER_REGISTRY.keys())
                print(
                    f"Unknown provider: {name}. Available providers: {available}",
                    file=sys.stderr,
                )
                sys.exit(1)
    else:
        providers = list(PROVIDER_REGISTRY.keys())

    interfaces = [args.interface] if args.interface else ["generate", "cogitate"]
    tier_names = {1: "pro", 2: "flash", 3: "lite"}
    tiers = [args.tier] if args.tier else [TIER_PRO, TIER_FLASH, TIER_LITE]

    provider_width = max(len(n) for n in providers) if providers else 0
    tier_width = max(len(tier_names[t]) for t in tiers)
    model_names = {PROVIDER_DEFAULTS[p][t] for p in providers for t in tiers}
    model_width = max(len(m) for m in model_names) if model_names else 0
    interface_width = max(len(n) for n in interfaces) if interfaces else 0

    total = 0
    passed = 0
    failed = 0
    skipped = 0
    results: list[dict[str, object]] = []
    cache: dict[tuple[str, str, str], tuple[str, str, str]] = {}

    for provider_name in providers:
        for tier in tiers:
            if (
                targeted_pairs is not None
                and (provider_name, tier) not in targeted_pairs
            ):
                continue
            model = PROVIDER_DEFAULTS[provider_name][tier]
            for interface_name in interfaces:
                cache_key = (provider_name, model, interface_name)
                if cache_key in cache:
                    status, message, source_tier = cache[cache_key]
                    elapsed_s = 0.0
                    elapsed_s_rounded = 0.0
                    reused_from = source_tier
                else:
                    start = time.perf_counter()
                    if interface_name == "generate":
                        status, message = _check_generate(
                            provider_name, tier, args.timeout
                        )
                    else:
                        status, message = await _check_cogitate(
                            provider_name, tier, args.timeout
                        )
                    elapsed_s = time.perf_counter() - start
                    elapsed_s_rounded = round(elapsed_s, 1)
                    cache[cache_key] = (status, message, tier_names[tier])
                    reused_from = None

                result: dict[str, object] = {
                    "provider": provider_name,
                    "tier": tier_names[tier],
                    "model": model,
                    "interface": interface_name,
                    "ok": status != "fail",
                    "status": status,
                    "message": str(message),
                    "elapsed_s": elapsed_s_rounded,
                }
                if reused_from:
                    result["reused_from"] = reused_from
                results.append(result)

                if not args.json:
                    if reused_from:
                        mark = "="
                        display_message = f"{message} (={reused_from})"
                    else:
                        if status == "ok":
                            mark = "✓"
                        elif status == "skip":
                            mark = "-"
                        else:
                            mark = "✗"
                        display_message = str(message)
                    print(
                        f"{mark} "
                        f"{provider_name:<{provider_width}}  "
                        f"{tier_names[tier]:<{tier_width}}  "
                        f"{model:<{model_width}}  "
                        f"{interface_name:<{interface_width}}  "
                        f"{display_message} ({elapsed_s:.1f}s)"
                    )

                total += 1
                if status == "ok":
                    passed += 1
                elif status == "skip":
                    skipped += 1
                else:
                    failed += 1

    any_failed = any(r["status"] == "fail" for r in results)

    payload = {
        "results": results,
        "summary": {
            "total": total,
            "passed": passed,
            "skipped": skipped,
            "failed": failed,
        },
        "checked_at": datetime.now(timezone.utc).isoformat(),
    }
    health_dir = Path(get_journal()) / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "talents.json").write_text(json.dumps(payload, indent=2))

    if args.json:
        print(
            json.dumps(
                {
                    "results": results,
                    "summary": {
                        "total": total,
                        "passed": passed,
                        "skipped": skipped,
                        "failed": failed,
                    },
                },
                indent=2,
            )
        )
    else:
        print(f"{total} checks: {passed} passed, {skipped} skipped, {failed} failed")
    sys.exit(1 if any_failed else 0)


async def main_async() -> None:
    """CLI entrypoint for provider connectivity checks."""
    from solstone.think.providers import PROVIDER_REGISTRY

    parser = argparse.ArgumentParser(description="solstone Provider CLI")
    subparsers = parser.add_subparsers(dest="subcommand")
    check_parser = subparsers.add_parser("check", help="Check AI provider connectivity")
    check_parser.add_argument(
        "--provider",
        action="append",
        help=f"Provider to check (repeatable). Available: {', '.join(PROVIDER_REGISTRY.keys())}",
    )
    check_parser.add_argument(
        "--interface",
        choices=["generate", "cogitate"],
        default=None,
        help="Interface to check (default: both)",
    )
    check_parser.add_argument(
        "--timeout",
        type=int,
        default=30,
        help="Timeout in seconds for generate checks (default: 30)",
    )
    check_parser.add_argument(
        "--tier",
        type=int,
        choices=[1, 2, 3],
        default=None,
        help="Tier to check (1=pro, 2=flash, 3=lite; default: all)",
    )
    check_parser.add_argument(
        "--json", action="store_true", help="Output results as JSON"
    )
    check_parser.add_argument(
        "--targeted",
        action="store_true",
        help="Only check configured provider+tier pairs (used by automated rechecks)",
    )

    args = setup_cli(parser)
    require_solstone()
    if args.subcommand != "check":
        parser.print_help()
        sys.exit(1)
    await _run_check(args)


def main() -> None:
    """Entry point wrapper."""
    asyncio.run(main_async())
