# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI commands for journal settings management.

Auto-discovered by ``think.call`` and mounted as ``sol call settings ...``.
"""

import json
import os
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlparse

import typer

from solstone.apps.settings.copy import (
    CONVEY_HOST_URL_CLEARED,
    CONVEY_HOST_URL_FLAG_CONFLICT,
    CONVEY_HOST_URL_INVALID,
    CONVEY_HOST_URL_SET_DONE,
    CONVEY_NETWORK_DISABLE_DONE,
    CONVEY_NETWORK_DISABLE_PROGRESS,
    CONVEY_NETWORK_ENABLE_DONE,
    CONVEY_NETWORK_ENABLE_PROGRESS,
    CONVEY_REFUSE_NO_PASSWORD_NETWORK,
    CONVEY_REFUSE_NO_PASSWORD_TRUST,
    CONVEY_RESTART_TIMEOUT,
    CONVEY_TRUST_DISABLE_DONE,
    CONVEY_TRUST_ENABLE_DONE,
    format_convey_status,
)
from solstone.think.pairing.config import get_host_url
from solstone.think.service import DEFAULT_SERVICE_PORT
from solstone.think.utils import get_project_root, require_solstone

app = typer.Typer(
    help="Journal settings — keys, providers, transcription, identity, and observer."
)


@app.callback()
def _require_up() -> None:
    require_solstone()


keys_app = typer.Typer(help="API key management.")
app.add_typer(keys_app, name="keys")
providers_app = typer.Typer(help="AI provider configuration.")
app.add_typer(providers_app, name="providers")
google_backend_app = typer.Typer(help="Google backend selection.")
app.add_typer(google_backend_app, name="google-backend")
vertex_app = typer.Typer(help="Vertex AI service account credentials.")
app.add_typer(vertex_app, name="vertex-credentials")
transcribe_app = typer.Typer(help="Transcription backend configuration.")
app.add_typer(transcribe_app, name="transcribe")
identity_app = typer.Typer(help="Journal owner identity.")
app.add_typer(identity_app, name="identity")
observer_app = typer.Typer(help="Observer capture settings.")
app.add_typer(observer_app, name="observer")
convey_app = typer.Typer(help="Convey access configuration.")
app.add_typer(convey_app, name="convey")
network_access_app = typer.Typer(help="Convey network exposure.")
convey_app.add_typer(network_access_app, name="network-access")
trust_localhost_app = typer.Typer(help="Localhost password-bypass behavior.")
convey_app.add_typer(trust_localhost_app, name="trust-localhost")


def _get_config():
    """Read journal config."""
    from solstone.think.journal_config import read_journal_config

    return read_journal_config()


def _write_config(config: dict) -> None:
    """Write journal config with indent=2, trailing newline, 0o600."""
    from solstone.think.journal_config import write_journal_config

    write_journal_config(config)


def _convey_password_is_set(config: dict) -> bool:
    from solstone.apps.settings.routes import (
        _convey_password_is_set as _route_password_is_set,
    )

    return _route_password_is_set(config)


def _convey_port() -> int:
    from solstone.think.utils import read_service_port

    return read_service_port("convey") or DEFAULT_SERVICE_PORT


def _network_access_enabled(config: dict) -> bool:
    return bool(config.get("convey", {}).get("allow_network_access", False))


def _trust_localhost_enabled(config: dict) -> bool:
    return bool(config.get("convey", {}).get("trust_localhost", True))


def _host_url_status_value(config: dict) -> str:
    pairing_host_url = config.get("pairing", {}).get("host_url")
    if isinstance(pairing_host_url, str) and pairing_host_url.strip():
        return f"{get_host_url()} (manual override)"
    if _network_access_enabled(config):
        return f"{get_host_url()} (auto-detected)"
    return f"{get_host_url()} (localhost — network access off)"


def _validate_host_url_or_exit(url: str) -> str:
    cleaned = url.strip()
    parsed = urlparse(cleaned)
    if not cleaned or not parsed.scheme or not parsed.netloc:
        typer.echo(CONVEY_HOST_URL_INVALID, err=True)
        raise typer.Exit(1)
    return cleaned


def _restart_convey_or_exit() -> None:
    from solstone.convey.restart import wait_for_convey_restart

    restart_ok, _ = wait_for_convey_restart(timeout=15.0)
    if restart_ok:
        return
    typer.echo(CONVEY_RESTART_TIMEOUT, err=True)
    raise typer.Exit(1)


def _provider_for_env_var(env_var: str) -> str | None:
    """Return the provider mapped to an API env var, if any."""
    from solstone.think.providers import PROVIDER_METADATA

    env_to_provider = {
        meta["env_key"]: name
        for name, meta in PROVIDER_METADATA.items()
        if "env_key" in meta
    }
    return env_to_provider.get(env_var)


@network_access_app.command("enable")
def convey_network_access_enable() -> None:
    """Enable non-loopback access to Convey and restart it."""

    config = _get_config()
    if not _convey_password_is_set(config):
        typer.echo(CONVEY_REFUSE_NO_PASSWORD_NETWORK, err=True)
        raise typer.Exit(1)
    config.setdefault("convey", {})["allow_network_access"] = True
    _write_config(config)
    typer.echo(CONVEY_NETWORK_ENABLE_PROGRESS)
    _restart_convey_or_exit()
    typer.echo(CONVEY_NETWORK_ENABLE_DONE.format(host_url=get_host_url()))


@network_access_app.command("disable")
def convey_network_access_disable() -> None:
    """Restrict Convey to localhost and restart it."""

    config = _get_config()
    config.setdefault("convey", {})["allow_network_access"] = False
    _write_config(config)
    typer.echo(CONVEY_NETWORK_DISABLE_PROGRESS)
    _restart_convey_or_exit()
    typer.echo(CONVEY_NETWORK_DISABLE_DONE.format(port=_convey_port()))


@trust_localhost_app.command("enable")
def convey_trust_localhost_enable() -> None:
    """Enable localhost password bypass."""

    config = _get_config()
    config.setdefault("convey", {})["trust_localhost"] = True
    _write_config(config)
    typer.echo(CONVEY_TRUST_ENABLE_DONE)


@trust_localhost_app.command("disable")
def convey_trust_localhost_disable() -> None:
    """Disable localhost password bypass."""

    config = _get_config()
    if not _convey_password_is_set(config):
        typer.echo(CONVEY_REFUSE_NO_PASSWORD_TRUST, err=True)
        raise typer.Exit(1)
    config.setdefault("convey", {})["trust_localhost"] = False
    _write_config(config)
    typer.echo(CONVEY_TRUST_DISABLE_DONE)


@convey_app.command("host-url")
def convey_host_url(
    url: str | None = typer.Argument(
        None, help="Absolute URL to advertise to devices."
    ),
    auto: bool = typer.Option(
        False, "--auto", help="Clear the manual host URL override."
    ),
    show: bool = typer.Option(False, "--show", help="Show the effective host URL."),
) -> None:
    """Manage the host URL advertised to remote devices."""

    if sum(bool(flag) for flag in (url is not None, auto, show)) != 1:
        typer.echo(CONVEY_HOST_URL_FLAG_CONFLICT, err=True)
        raise typer.Exit(1)
    if show:
        typer.echo(get_host_url())
        return
    config = _get_config()
    config.setdefault("pairing", {})
    if auto:
        config["pairing"]["host_url"] = None
        _write_config(config)
        typer.echo(CONVEY_HOST_URL_CLEARED)
        return
    assert url is not None
    cleaned = _validate_host_url_or_exit(url)
    config["pairing"]["host_url"] = cleaned
    _write_config(config)
    typer.echo(CONVEY_HOST_URL_SET_DONE.format(url=cleaned))


@convey_app.command("status")
def convey_status() -> None:
    """Show Convey network and host-URL status."""

    from solstone.convey.cli import _resolve_bind_host

    config = _get_config()
    network_access = "on" if _network_access_enabled(config) else "localhost only"
    bind_host = _resolve_bind_host()
    password = "set" if _convey_password_is_set(config) else "not set"
    trust_localhost = "yes" if _trust_localhost_enabled(config) else "no"
    typer.echo(
        format_convey_status(
            bind=f"{bind_host}:{_convey_port()}",
            host_url=_host_url_status_value(config),
            network_access=network_access,
            password=password,
            trust_localhost=trust_localhost,
        )
    )


def _validate_env_var_or_exit(env_var: str) -> None:
    """Exit if env_var is not a supported API key variable."""
    from solstone.apps.settings.routes import API_KEY_ENV_VARS

    if env_var not in API_KEY_ENV_VARS:
        typer.echo(
            f"Invalid env var: {env_var}. Must be one of: {', '.join(API_KEY_ENV_VARS)}",
            err=True,
        )
        raise typer.Exit(1)


def _set_provider_type(
    agent_type: str,
    provider: str | None,
    tier: int | None,
    backup: str | None,
) -> dict:
    """Validate and update the provider settings for a single agent type."""
    from solstone.think.providers import PROVIDER_REGISTRY

    config = _get_config()
    config.setdefault("providers", {})
    config["providers"].setdefault(agent_type, {})

    if provider is not None:
        if provider not in PROVIDER_REGISTRY:
            typer.echo(
                f"Invalid provider: {provider}. Must be one of: {', '.join(sorted(PROVIDER_REGISTRY.keys()))}",
                err=True,
            )
            raise typer.Exit(1)
        config["providers"][agent_type]["provider"] = provider

    if tier is not None:
        if tier not in {1, 2, 3}:
            typer.echo(f"Invalid tier: {tier}. Must be 1, 2, or 3.", err=True)
            raise typer.Exit(1)
        config["providers"][agent_type]["tier"] = tier

    if backup is not None:
        if backup not in PROVIDER_REGISTRY:
            typer.echo(
                f"Invalid backup provider: {backup}. Must be one of: {', '.join(sorted(PROVIDER_REGISTRY.keys()))}",
                err=True,
            )
            raise typer.Exit(1)
        config["providers"][agent_type]["backup"] = backup

    _write_config(config)
    return config["providers"][agent_type]


@app.command("show")
def show() -> None:
    """Show a summary of journal settings."""
    from solstone.apps.settings.routes import API_KEY_ENV_VARS
    from solstone.think.models import TYPE_DEFAULTS

    config = _get_config()
    providers_config = config.get("providers", {})
    type_settings = {}
    for agent_type in ("generate", "cogitate"):
        defaults = TYPE_DEFAULTS[agent_type]
        type_config = providers_config.get(agent_type, {})
        type_settings[agent_type] = {
            "provider": type_config.get("provider", defaults["provider"]),
            "tier": type_config.get("tier", defaults["tier"]),
            "backup": type_config.get("backup", defaults["backup"]),
        }

    summary = {
        "identity": config.get("identity", {}),
        "providers": {
            "generate": type_settings["generate"],
            "cogitate": type_settings["cogitate"],
            "google_backend": providers_config.get("google_backend", "auto"),
            "auth": providers_config.get("auth", {}),
            "key_validation": providers_config.get("key_validation", {}),
        },
        "transcribe": config.get("transcribe", {}),
        "observe": config.get("observe", {}),
        "keys": {k: bool(config.get("env", {}).get(k)) for k in API_KEY_ENV_VARS},
    }
    typer.echo(json.dumps(summary, indent=2))


@keys_app.command("show")
def keys_show() -> None:
    """Show configured API key status."""
    from solstone.apps.settings.routes import API_KEY_ENV_VARS

    config = _get_config()
    env_config = config.get("env", {})
    status = {k: bool(env_config.get(k)) for k in API_KEY_ENV_VARS}
    typer.echo(json.dumps(status, indent=2))


@keys_app.command("set")
def keys_set(
    env_var: str = typer.Argument(..., help="Environment variable to set."),
    value: str = typer.Argument(..., help="API key value."),
) -> None:
    """Set an API key in journal config."""
    from solstone.think.providers import validate_key

    _validate_env_var_or_exit(env_var)
    config = _get_config()
    config.setdefault("env", {})
    config["env"][env_var] = value
    os.environ[env_var] = value

    validation = None
    provider = _provider_for_env_var(env_var)
    if provider:
        config.setdefault("providers", {})
        config["providers"].setdefault("auth", {})
        config["providers"]["auth"][provider] = "api_key"
        validation = validate_key(provider, value)
        validation["timestamp"] = datetime.now(timezone.utc).isoformat()
        config["providers"].setdefault("key_validation", {})
        config["providers"]["key_validation"][provider] = validation

    _write_config(config)
    typer.echo(
        json.dumps(
            {"env_var": env_var, "set": True, "validation": validation},
            indent=2,
        )
    )


@keys_app.command("clear")
def keys_clear(
    env_var: str = typer.Argument(..., help="Environment variable to clear."),
) -> None:
    """Clear an API key from journal config."""
    _validate_env_var_or_exit(env_var)
    config = _get_config()
    env_config = config.setdefault("env", {})
    env_config.pop(env_var, None)
    os.environ.pop(env_var, None)

    provider = _provider_for_env_var(env_var)
    if provider:
        config.setdefault("providers", {})
        config["providers"].setdefault("auth", {})
        config["providers"]["auth"][provider] = "platform"
        config["providers"].setdefault("key_validation", {})
        config["providers"]["key_validation"].pop(provider, None)

    _write_config(config)
    typer.echo(json.dumps({"env_var": env_var, "cleared": True}, indent=2))


@keys_app.command("validate")
def keys_validate(
    cache_result: bool = typer.Option(
        False, "--cache-result", help="Persist results to providers.key_validation."
    ),
) -> None:
    """Validate all configured API keys without persisting by default."""
    from solstone.think.providers import PROVIDER_METADATA, validate_key
    from solstone.think.providers.google import validate_vertex_credentials

    config = _get_config()
    env_config = config.get("env", {})
    env_to_provider = {
        meta["env_key"]: name
        for name, meta in PROVIDER_METADATA.items()
        if "env_key" in meta
    }

    key_validation = {}
    for env_var, provider in env_to_provider.items():
        api_key = env_config.get(env_var, "")
        if api_key:
            result = validate_key(provider, api_key)
            result["timestamp"] = datetime.now(timezone.utc).isoformat()
            key_validation[provider] = result

    providers_config = config.get("providers", {})
    if providers_config.get("google_backend") == "vertex" and providers_config.get(
        "vertex_credentials"
    ):
        result = validate_vertex_credentials(providers_config["vertex_credentials"])
        result["timestamp"] = datetime.now(timezone.utc).isoformat()
        key_validation["google"] = result

    if cache_result:
        config.setdefault("providers", {})
        config["providers"]["key_validation"] = key_validation
        _write_config(config)
    typer.echo(json.dumps({"key_validation": key_validation}, indent=2))


@providers_app.command("show")
def providers_show(
    human: bool = typer.Option(False, "--human", help="Print one-line statuses."),
) -> None:
    """Show provider configuration."""
    from solstone.think.models import TYPE_DEFAULTS
    from solstone.think.providers import build_provider_status, get_provider_list

    config = _get_config()
    providers_config = config.get("providers", {})
    type_settings = {}
    for agent_type in ("generate", "cogitate"):
        defaults = TYPE_DEFAULTS[agent_type]
        type_config = providers_config.get(agent_type, {})
        type_settings[agent_type] = {
            "provider": type_config.get("provider", defaults["provider"]),
            "tier": type_config.get("tier", defaults["tier"]),
            "backup": type_config.get("backup", defaults["backup"]),
        }

    providers_list = get_provider_list()
    api_keys = {}
    for provider in providers_list:
        env_key = provider.get("env_key", "")
        api_keys[provider["name"]] = bool(os.getenv(env_key)) if env_key else False

    auth_config = providers_config.get("auth", {})
    auth = {
        provider["name"]: auth_config.get(provider["name"], "platform")
        for provider in providers_list
    }
    vertex_creds_path = providers_config.get("vertex_credentials")
    vertex_creds_configured = bool(
        vertex_creds_path and Path(vertex_creds_path).exists()
    )
    provider_status = build_provider_status(providers_list, vertex_creds_configured)
    result = {
        "providers": providers_list,
        "provider_status": provider_status,
        "generate": type_settings["generate"],
        "cogitate": type_settings["cogitate"],
        "api_keys": api_keys,
        "auth": auth,
        "key_validation": providers_config.get("key_validation", {}),
    }
    if human:
        for name in sorted(provider_status):
            status = provider_status[name]
            issues = status.get("issues", [])
            if issues:
                status_text = issues[0]
            elif status.get("cogitate_ready") or (
                not status.get("cogitate_cli") and status.get("generate_ready")
            ):
                status_text = "ready"
            else:
                status_text = "not ready"
            typer.echo(f"{name}: {status_text}")
        return
    typer.echo(json.dumps(result, indent=2))


def _bundled_status_payload(name: str | None) -> dict:
    from solstone.think.providers import bundled

    if name:
        return bundled.get_provider_state(name)
    return {
        provider: bundled.get_provider_state(provider)
        for provider in ("anthropic", "openai", "openhands")
    }


def _echo_bundled_result(payload: dict, *, human: bool = False) -> None:
    if not human:
        typer.echo(json.dumps(payload, indent=2))
        return

    def _render_binary_path(value: str | None) -> str:
        if not value:
            return "-"
        return value if len(value) <= 32 else "..." + value[-29:]

    rows = payload.values() if "install_state" not in payload else [payload]
    headers = ("provider", "install", "key", "binary", "issues")
    rendered = []
    for row in rows:
        rendered.append(
            (
                row["name"],
                row["install_state"],
                row["key_status"],
                _render_binary_path(row["binary_path"]),
                ", ".join(row.get("issues", [])),
            )
        )
    widths = [
        max(len(str(value)) for value in (header, *(row[idx] for row in rendered)))
        for idx, header in enumerate(headers)
    ]
    typer.echo(
        "  ".join(header.ljust(widths[idx]) for idx, header in enumerate(headers))
    )
    typer.echo("  ".join("-" * width for width in widths))
    for row in rendered:
        typer.echo(
            "  ".join(str(value).ljust(widths[idx]) for idx, value in enumerate(row))
        )


def _bundled_error_exit(exc: Exception) -> None:
    typer.echo(
        json.dumps(
            {
                "error": str(exc),
                "type": exc.__class__.__name__,
            },
            indent=2,
        ),
        err=True,
    )
    raise typer.Exit(1)


@providers_app.command("status")
def providers_bundled_status(
    name: str | None = typer.Argument(None, help="Bundled provider name."),
    json_flag: bool = typer.Option(False, "--json", help="Print JSON output."),
    human: bool = typer.Option(False, "--human", help="Print a compact table."),
) -> None:
    """Show bundled cogitate provider status."""
    from solstone.think.providers import bundled

    if json_flag and human:
        typer.echo("--json and --human cannot be used together.", err=True)
        raise typer.Exit(1)
    try:
        _echo_bundled_result(_bundled_status_payload(name), human=human)
    except bundled.BundledProviderError as exc:
        _bundled_error_exit(exc)


@providers_app.command("install")
def providers_bundled_install(
    name: str = typer.Argument(..., help="Bundled provider name."),
) -> None:
    """Install or retry a bundled cogitate provider."""
    from solstone.think.providers import bundled

    try:
        typer.echo(json.dumps(bundled.install_provider(name), indent=2))
    except bundled.BundledProviderError as exc:
        _bundled_error_exit(exc)


@providers_app.command("uninstall")
def providers_bundled_uninstall(
    name: str = typer.Argument(..., help="Bundled provider name."),
) -> None:
    """Uninstall a bundled cogitate provider."""
    from solstone.think.providers import bundled

    try:
        typer.echo(json.dumps(bundled.uninstall_provider(name), indent=2))
    except bundled.BundledProviderError as exc:
        _bundled_error_exit(exc)


@providers_app.command("disable")
def providers_bundled_disable(
    name: str = typer.Argument(..., help="Bundled provider name."),
) -> None:
    """Disable a bundled cogitate provider."""
    from solstone.think.providers import bundled

    try:
        typer.echo(json.dumps(bundled.disable_provider(name), indent=2))
    except bundled.BundledProviderError as exc:
        _bundled_error_exit(exc)


@providers_app.command("enable")
def providers_bundled_enable(
    name: str = typer.Argument(..., help="Bundled provider name."),
) -> None:
    """Enable a bundled cogitate provider."""
    from solstone.think.providers import bundled

    try:
        typer.echo(json.dumps(bundled.enable_provider(name), indent=2))
    except bundled.BundledProviderError as exc:
        _bundled_error_exit(exc)


@providers_app.command("validate-key")
def providers_bundled_validate_key(
    name: str = typer.Argument(..., help="Bundled provider name."),
) -> None:
    """Validate a bundled provider API key."""
    from solstone.think.providers import bundled

    try:
        typer.echo(json.dumps(bundled.validate_key(name), indent=2))
    except bundled.BundledProviderError as exc:
        _bundled_error_exit(exc)


@providers_app.command("migrate-ollama-to-local")
def providers_migrate_ollama_to_local(
    commit: bool = typer.Option(False, "--commit", help="Persist config rewrites."),
    json_flag: bool = typer.Option(False, "--json", help="Print JSON output."),
) -> None:
    """Dry-run or apply the Ollama-to-Local provider config migration."""
    from solstone.apps.settings.maint._migrate_ollama_to_local import migrate_config

    config = _get_config()
    migrated, report = migrate_config(config)
    report["committed"] = False
    if commit and report["changed"]:
        _write_config(migrated)
        report["committed"] = True

    if json_flag:
        typer.echo(json.dumps(report, indent=2))
        return
    if not report["changed"]:
        typer.echo("No ollama config entries found.")
        return
    if not commit:
        typer.echo("REPORT ONLY - pass --commit to persist.")
    for change in report["changes"]:
        warning = f" ({change['warning']})" if change.get("warning") else ""
        typer.echo(f"{change['path']}: {change['old']!r} -> {change['new']!r}{warning}")


@providers_app.command("set-generate")
def providers_set_generate(
    provider: str | None = typer.Option(None, "--provider", help="Primary provider."),
    tier: int | None = typer.Option(None, "--tier", help="Tier (1, 2, or 3)."),
    backup: str | None = typer.Option(None, "--backup", help="Backup provider."),
) -> None:
    """Set generate provider defaults."""
    typer.echo(
        json.dumps(_set_provider_type("generate", provider, tier, backup), indent=2)
    )


@providers_app.command("set-cogitate")
def providers_set_cogitate(
    provider: str | None = typer.Option(None, "--provider", help="Primary provider."),
    tier: int | None = typer.Option(None, "--tier", help="Tier (1, 2, or 3)."),
    backup: str | None = typer.Option(None, "--backup", help="Backup provider."),
) -> None:
    """Set cogitate provider defaults."""
    typer.echo(
        json.dumps(_set_provider_type("cogitate", provider, tier, backup), indent=2)
    )


@providers_app.command("set-auth")
def providers_set_auth(
    provider: str = typer.Argument(..., help="Provider name."),
    mode: str = typer.Argument(..., help="Auth mode."),
) -> None:
    """Set provider auth mode."""
    from solstone.think.providers import PROVIDER_REGISTRY

    if provider not in PROVIDER_REGISTRY:
        typer.echo(f"Invalid provider in auth: {provider}", err=True)
        raise typer.Exit(1)
    if mode not in ("platform", "api_key"):
        typer.echo(
            f"Invalid auth mode: {mode}. Must be 'platform' or 'api_key'.",
            err=True,
        )
        raise typer.Exit(1)

    config = _get_config()
    config.setdefault("providers", {})
    config["providers"].setdefault("auth", {})
    config["providers"]["auth"][provider] = mode
    _write_config(config)
    typer.echo(json.dumps({provider: mode}, indent=2))


@google_backend_app.command("show")
def google_backend_show() -> None:
    """Show Google backend status."""
    config = _get_config()
    providers_config = config.get("providers", {})
    google_backend = providers_config.get("google_backend", "auto")
    vertex_creds_path = providers_config.get("vertex_credentials")
    vertex_configured = False
    vertex_email = ""
    if vertex_creds_path and Path(vertex_creds_path).exists():
        vertex_configured = True
        try:
            creds_data = json.loads(Path(vertex_creds_path).read_text())
            vertex_email = creds_data.get("client_email", "")
        except Exception:
            pass
    result = {
        "google_backend": google_backend,
        "vertex_credentials_configured": vertex_configured,
        "vertex_credentials_email": vertex_email,
    }
    typer.echo(json.dumps(result, indent=2))


@google_backend_app.command("set")
def google_backend_set(
    backend: str = typer.Argument(..., help="Google backend to use."),
) -> None:
    """Set the Google provider backend."""
    if backend not in ("auto", "aistudio", "vertex"):
        typer.echo(
            f"Invalid google_backend: {backend}. Must be 'auto', 'aistudio', or 'vertex'.",
            err=True,
        )
        raise typer.Exit(1)

    config = _get_config()
    config.setdefault("providers", {})
    config["providers"]["google_backend"] = backend
    _write_config(config)
    typer.echo(json.dumps({"google_backend": backend}, indent=2))


@vertex_app.command("show")
def vertex_credentials_show() -> None:
    """Show Vertex credential status without secrets."""
    config = _get_config()
    providers_config = config.get("providers", {})
    vertex_creds_path = providers_config.get("vertex_credentials")
    configured = False
    email = ""
    if vertex_creds_path and Path(vertex_creds_path).exists():
        configured = True
        try:
            creds_data = json.loads(Path(vertex_creds_path).read_text())
            email = creds_data.get("client_email", "")
        except Exception:
            pass
    validation = providers_config.get("key_validation", {}).get("google_vertex", {})
    result = {
        "configured": configured,
        "email": email,
        "path": vertex_creds_path or "",
        "validation": validation,
    }
    typer.echo(json.dumps(result, indent=2))


@vertex_app.command("import")
def vertex_credentials_import(
    file_path: str = typer.Argument(..., help="Path to service account JSON."),
    skip_validation: bool = typer.Option(
        False, "--skip-validation", help="Skip API validation of credentials."
    ),
) -> None:
    """Import Vertex service account credentials into the journal config."""
    from solstone.think.providers.google import validate_vertex_credentials
    from solstone.think.utils import get_journal

    source = Path(file_path)
    if not source.exists():
        typer.echo(f"Credential file not found: {file_path}", err=True)
        raise typer.Exit(1)

    try:
        creds_data = json.loads(source.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        typer.echo(f"Invalid JSON in credential file: {file_path}", err=True)
        raise typer.Exit(1)

    required_fields = ("type", "project_id", "client_email", "private_key")
    missing = [field for field in required_fields if field not in creds_data]
    if missing:
        typer.echo(f"Missing required fields: {', '.join(missing)}", err=True)
        raise typer.Exit(1)

    journal_root = Path(get_journal())
    creds_dir = journal_root / ".config"
    creds_dir.mkdir(parents=True, exist_ok=True)
    creds_file = creds_dir / "vertex-credentials.json"

    with open(creds_file, "w", encoding="utf-8") as f:
        json.dump(creds_data, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.chmod(creds_file, 0o600)

    config = _get_config()
    config.setdefault("providers", {})
    config["providers"]["vertex_credentials"] = str(creds_file)

    validation = None
    if not skip_validation:
        validation = validate_vertex_credentials(str(creds_file))
        validation["timestamp"] = datetime.now(timezone.utc).isoformat()
        config["providers"].setdefault("key_validation", {})
        config["providers"]["key_validation"]["google_vertex"] = validation

    _write_config(config)
    typer.echo(
        json.dumps(
            {
                "configured": True,
                "email": creds_data.get("client_email", ""),
                "path": str(creds_file),
                "validation": validation,
            },
            indent=2,
        )
    )


@vertex_app.command("clear")
def vertex_credentials_clear() -> None:
    """Clear stored Vertex credentials."""
    from solstone.think.utils import get_journal

    config = _get_config()
    config.setdefault("providers", {})
    old_path = config["providers"].get("vertex_credentials")
    if old_path:
        canonical = Path(get_journal()) / ".config" / "vertex-credentials.json"
        if Path(old_path).resolve() == canonical.resolve():
            try:
                canonical.unlink(missing_ok=True)
            except OSError:
                pass
        config["providers"].pop("vertex_credentials", None)
        config["providers"].setdefault("key_validation", {})
        config["providers"]["key_validation"].pop("google_vertex", None)

    _write_config(config)
    typer.echo(json.dumps({"configured": False}, indent=2))


@transcribe_app.command("show")
def transcribe_show() -> None:
    """Show transcription backend configuration."""
    from solstone.observe.transcribe import get_backend_list

    config = _get_config()
    transcribe_config = config.get("transcribe", {})
    backends = get_backend_list()
    api_keys = {}
    for backend in backends:
        env_key = backend.get("env_key")
        if env_key:
            api_keys[backend["name"]] = bool(os.getenv(env_key))
        else:
            api_keys[backend["name"]] = True
    result = {"backends": backends, "api_keys": api_keys, "config": transcribe_config}
    typer.echo(json.dumps(result, indent=2))


@transcribe_app.command("set-backend")
def transcribe_set_backend(
    backend: str = typer.Argument(..., help="Transcription backend."),
) -> None:
    """Set the transcription backend."""
    from solstone.observe.transcribe import BACKEND_REGISTRY

    if backend not in BACKEND_REGISTRY:
        typer.echo(
            f"Invalid backend: {backend}. Must be one of: {', '.join(sorted(BACKEND_REGISTRY.keys()))}",
            err=True,
        )
        raise typer.Exit(1)

    config = _get_config()
    config.setdefault("transcribe", {})
    config["transcribe"]["backend"] = backend
    _write_config(config)
    typer.echo(json.dumps(config["transcribe"], indent=2))


@transcribe_app.command("set")
def transcribe_set(
    enrich: bool | None = typer.Option(None, "--enrich/--no-enrich"),
    noise_upgrade: bool | None = typer.Option(
        None, "--noise-upgrade/--no-noise-upgrade"
    ),
) -> None:
    """Set transcription options."""
    config = _get_config()
    config.setdefault("transcribe", {})
    if enrich is not None:
        config["transcribe"]["enrich"] = enrich
    if noise_upgrade is not None:
        config["transcribe"]["noise_upgrade"] = noise_upgrade
    _write_config(config)
    typer.echo(json.dumps(config["transcribe"], indent=2))


@identity_app.command("show")
def identity_show() -> None:
    """Show journal identity config."""
    config = _get_config()
    identity = config.get("identity", {})
    typer.echo(json.dumps(identity, indent=2))


@identity_app.command("set")
def identity_set(
    name: str | None = typer.Option(None, "--name"),
    preferred: str | None = typer.Option(None, "--preferred"),
    bio: str | None = typer.Option(None, "--bio"),
    timezone_name: str | None = typer.Option(None, "--timezone"),
    pronouns: str | None = typer.Option(None, "--pronouns"),
    add_email: str | None = typer.Option(None, "--add-email"),
    remove_email: str | None = typer.Option(None, "--remove-email"),
    add_alias: str | None = typer.Option(None, "--add-alias"),
    remove_alias: str | None = typer.Option(None, "--remove-alias"),
) -> None:
    """Update journal owner identity."""
    config = _get_config()
    config.setdefault("identity", {})
    identity = config["identity"]

    if name is not None:
        identity["name"] = name
    if preferred is not None:
        identity["preferred"] = preferred
    if bio is not None:
        identity["bio"] = bio
    if timezone_name is not None:
        identity["timezone"] = timezone_name

    if pronouns is not None:
        try:
            identity["pronouns"] = json.loads(pronouns)
        except json.JSONDecodeError:
            typer.echo("Invalid JSON in pronouns", err=True)
            raise typer.Exit(1)

    if add_email is not None or remove_email is not None:
        emails = list(identity.get("email_addresses", []))
        if add_email is not None and add_email not in emails:
            emails.append(add_email)
        if remove_email is not None:
            emails = [email for email in emails if email != remove_email]
        identity["email_addresses"] = emails

    if add_alias is not None or remove_alias is not None:
        aliases = list(identity.get("aliases", []))
        if add_alias is not None and add_alias not in aliases:
            aliases.append(add_alias)
        if remove_alias is not None:
            aliases = [alias for alias in aliases if alias != remove_alias]
        identity["aliases"] = aliases

    _write_config(config)
    project_root = Path(get_project_root())
    subprocess.run(
        ["make", "skills"], cwd=project_root, check=False, capture_output=True
    )
    typer.echo(json.dumps(identity, indent=2))


@observer_app.command("show")
def observer_show() -> None:
    """Show observer configuration with defaults."""
    from solstone.apps.settings.routes import OBSERVE_TMUX_DEFAULTS

    config = _get_config()
    observe_config = config.get("observe", {})
    tmux_config = observe_config.get("tmux", {})
    result = {
        "tmux": {
            "enabled": tmux_config.get("enabled", OBSERVE_TMUX_DEFAULTS["enabled"]),
            "capture_interval": tmux_config.get(
                "capture_interval", OBSERVE_TMUX_DEFAULTS["capture_interval"]
            ),
        },
        "defaults": {"tmux": OBSERVE_TMUX_DEFAULTS},
    }
    typer.echo(json.dumps(result, indent=2))


@observer_app.command("set")
def observer_set(
    enabled: bool | None = typer.Option(None, "--enabled/--no-enabled"),
    capture_interval: int | None = typer.Option(None, "--capture-interval"),
) -> None:
    """Update observer capture settings."""
    from solstone.apps.settings.routes import OBSERVE_TMUX_DEFAULTS

    config = _get_config()
    config.setdefault("observe", {})
    config["observe"].setdefault("tmux", {})

    if capture_interval is not None:
        min_val = OBSERVE_TMUX_DEFAULTS["capture_interval_min"]
        max_val = OBSERVE_TMUX_DEFAULTS["capture_interval_max"]
        if capture_interval < min_val or capture_interval > max_val:
            typer.echo(
                f"tmux.capture_interval must be an integer between {min_val} and {max_val}",
                err=True,
            )
            raise typer.Exit(1)
        config["observe"]["tmux"]["capture_interval"] = capture_interval

    if enabled is not None:
        config["observe"]["tmux"]["enabled"] = enabled

    _write_config(config)
    typer.echo(json.dumps(config["observe"]["tmux"], indent=2))
