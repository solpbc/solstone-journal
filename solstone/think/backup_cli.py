# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Service CLI for solstone backup."""

from __future__ import annotations

import json
import sys
from dataclasses import asdict
from typing import NoReturn

import typer

from solstone.think.backup.destination import (
    Destination,
    DestinationStatus,
    validate_destination,
)
from solstone.think.backup.engine import run_backup, run_prune
from solstone.think.backup.hosted import HostedBinding, save_hosted_binding
from solstone.think.backup.install import ensure_restic
from solstone.think.backup.keys import (
    confirm_recovery_key,
    format_recovery_key_display,
    generate_daily_key,
)
from solstone.think.backup.repo import ResticKeyError, init_repository
from solstone.think.backup.restore import restore_journal
from solstone.think.backup.rotation import rotate_recovery_key
from solstone.think.backup.state import (
    generate_and_store_keys,
    get_backup_config,
    get_destination,
    get_keys,
    set_destination,
    set_enabled,
    set_recovery_key_confirmed,
    status_view,
)
from solstone.think.backup.teardown import teardown_backup
from solstone.think.offload import OffloadResult, format_offload_result, run_offload
from solstone.think.offload_restore import (
    build_offload_status,
    restore_all,
    restore_day,
)
from solstone.think.utils import init_cli_runtime

app = typer.Typer(help="Manage solstone backup.", no_args_is_help=True)
destination_app = typer.Typer(help="Manage backup destination.", no_args_is_help=True)
recovery_key_app = typer.Typer(help="Manage backup recovery key.", no_args_is_help=True)
offload_app = typer.Typer(help="Manage media offload.", no_args_is_help=True)
app.add_typer(destination_app, name="destination")
app.add_typer(recovery_key_app, name="recovery-key")
app.add_typer(offload_app, name="offload")

_BACKEND_REQUIRED_CREDENTIALS = {
    "s3": ("access_key_id", "secret_access_key"),
    "b2": ("account_id", "account_key"),
}


def _die(message: str, code: int = 1) -> NoReturn:
    typer.echo(message, err=True)
    raise typer.Exit(code)


def _read_stdin_json(*, allow_empty: bool = False) -> dict[str, object]:
    """Parse a single JSON object from stdin."""
    raw = sys.stdin.read().strip()
    if not raw:
        if allow_empty:
            return {}
        typer.echo("Error: expected JSON object on stdin.", err=True)
        raise typer.Exit(1)

    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        typer.echo(f"Error: invalid JSON on stdin: {exc}", err=True)
        raise typer.Exit(1) from None

    if not isinstance(payload, dict):
        typer.echo("Error: expected JSON object on stdin.", err=True)
        raise typer.Exit(1)
    return payload


def _echo_json(payload: object) -> None:
    typer.echo(json.dumps(payload, indent=2, sort_keys=True))


def _print_recovery_grid(display: str) -> None:
    groups = display.split()
    for index in range(0, len(groups), 4):
        typer.echo(" ".join(groups[index : index + 4]))


def _destination_from_payload(payload: dict[str, object]) -> Destination:
    repository = payload.get("repository")
    if not isinstance(repository, str) or not repository.strip():
        _die("Missing repository.")

    backend = payload.get("backend")
    if not isinstance(backend, str) or not backend.strip():
        _die("Missing backend.")
    backend = backend.strip()
    if backend not in _BACKEND_REQUIRED_CREDENTIALS:
        _die("Unsupported backend.")

    raw_credentials = payload.get("credentials")
    if not isinstance(raw_credentials, dict):
        _die("Missing credentials.")

    credentials: dict[str, str] = {}
    for key in _BACKEND_REQUIRED_CREDENTIALS[backend]:
        value = raw_credentials.get(key)
        if not isinstance(value, str) or not value.strip():
            _die(f"Missing credential: {key}.")
        credentials[key] = value.strip()

    return Destination(
        repository=repository.strip(),
        backend=backend,
        credentials=credentials,
    )


def _hosted_binding_from_payload(payload: dict[str, object]) -> HostedBinding:
    broker_endpoint = payload.get("broker_endpoint")
    if not isinstance(broker_endpoint, str) or not broker_endpoint.strip():
        _die("Missing broker_endpoint.")

    account_id = payload.get("account_id")
    if not isinstance(account_id, str) or not account_id.strip():
        _die("Missing account_id.")

    instance_id = payload.get("instance_id")
    if not isinstance(instance_id, str) or not instance_id.strip():
        _die("Missing instance_id.")

    bucket = payload.get("bucket")
    if not isinstance(bucket, str) or not bucket.strip():
        _die("Missing bucket.")

    prefix = payload.get("prefix")
    if not isinstance(prefix, str) or not prefix.strip():
        _die("Missing prefix.")

    broker_token = payload.get("broker_token")
    if not isinstance(broker_token, str) or not broker_token.strip():
        _die("Missing broker_token.")

    return HostedBinding(
        broker_endpoint=broker_endpoint.strip(),
        account_id=account_id.strip(),
        instance_id=instance_id.strip(),
        bucket=bucket.strip(),
        prefix=prefix,
        broker_token=broker_token.strip(),
    )


def _destination_status_payload(dest_status: DestinationStatus) -> dict[str, object]:
    return {
        "reachable": dest_status.reachable,
        "repo_exists": dest_status.repo_exists,
        "reason_code": dest_status.reason_code,
        "message": dest_status.message,
    }


@app.callback()
def _configure(
    verbose: bool = typer.Option(
        False, "-v", "--verbose", help="Enable verbose output"
    ),
    debug: bool = typer.Option(False, "-d", "--debug", help="Enable debug logging"),
) -> None:
    """Manage solstone backup."""
    init_cli_runtime(verbose, debug)


@app.command("status")
def status() -> None:
    """Show backup status."""
    _echo_json(status_view())


@destination_app.command("show")
def destination_show() -> None:
    """Show the configured backup destination."""
    _echo_json(status_view()["destination"])


@destination_app.command("set")
def destination_set() -> None:
    """Set the backup destination from a JSON object on stdin."""
    destination = _destination_from_payload(_read_stdin_json())
    set_destination(destination)
    keys = get_keys()
    password = keys.daily_key if keys is not None else generate_daily_key()
    restic_path = ensure_restic()
    dest_status = validate_destination(destination, password, restic_path=restic_path)
    _echo_json(_destination_status_payload(dest_status))
    if dest_status.reason_code in {"auth_failed", "timeout", "unreachable"}:
        raise typer.Exit(1)


@destination_app.command("set-hosted")
def destination_set_hosted() -> None:
    """Set the hosted-tier broker binding from a JSON object on stdin."""
    payload = _read_stdin_json()
    binding = _hosted_binding_from_payload(payload)
    save_hosted_binding(binding)
    _echo_json(
        {
            "broker_endpoint": binding.broker_endpoint,
            "account_id": binding.account_id,
            "instance_id": binding.instance_id,
            "bucket": binding.bucket,
            "prefix": binding.prefix,
            "bound": True,
        }
    )


@app.command("enable")
def enable() -> None:
    """Enable solstone backup."""
    destination = get_destination()
    if destination is None:
        _die("Set a destination first: journal backup destination set")

    config = get_backup_config()
    daily = config["daily_key"]
    recovery = config["recovery_key"]
    confirmed = config["confirmed_recovery_key"] is True

    if confirmed or (daily is not None and recovery is None):
        keys = get_keys()
        restic_path = ensure_restic()
        if keys is not None:
            try:
                init_repository(
                    destination,
                    daily_key=keys.daily_key,
                    recovery_key=keys.recovery_key,
                    restic_path=restic_path,
                )
            except ResticKeyError as exc:
                _die(f"Repository initialization failed (code={exc.returncode}).")
        else:
            dest_status = validate_destination(
                destination,
                daily,
                restic_path=restic_path,
            )
            if not dest_status.repo_exists:
                _die(
                    "Repository not found; pre-initialize it or set "
                    "backup.recovery_key in config."
                )
        set_enabled(True)
        typer.echo("Backup enabled.")
        return

    keys = generate_and_store_keys()
    entered = "" if sys.stdin.isatty() else sys.stdin.read().strip()
    if not entered:
        typer.echo("Your recovery key (write it down - it is the only way to restore):")
        _print_recovery_grid(format_recovery_key_display(keys.recovery_key))
        typer.echo("")
        typer.echo("Confirm by piping the key back: journal backup enable")
        return

    if not confirm_recovery_key(entered, keys.recovery_key):
        _die("Recovery key did not match.")

    set_recovery_key_confirmed(True)
    restic_path = ensure_restic()
    try:
        init_repository(
            destination,
            daily_key=keys.daily_key,
            recovery_key=keys.recovery_key,
            restic_path=restic_path,
        )
    except ResticKeyError as exc:
        _die(f"Repository initialization failed (code={exc.returncode}).")
    set_enabled(True)
    typer.echo("Backup enabled. Your recovery key:")
    _print_recovery_grid(format_recovery_key_display(keys.recovery_key))


@app.command("run")
def run() -> None:
    """Run backup now."""
    result = run_backup()
    if result.status == "error":
        _die(f"Backup failed: {result.error_reason}.")
    if result.status == "skipped":
        typer.echo("Backup skipped (not enabled or not configured).")
        return
    if result.status == "ok":
        typer.echo(f"Backup complete (snapshot {result.snapshot_id}).")
        return
    raise RuntimeError(f"unknown backup status: {result.status}")


@app.command("prune")
def prune() -> None:
    """Apply backup retention pruning."""
    result = run_prune()
    if result.status == "error":
        _die(f"Prune failed: {result.error_reason}.")
    if result.status == "skipped":
        typer.echo("Prune skipped (not enabled or not configured).")
        return
    if result.status == "ok":
        typer.echo("Retention prune complete.")
        return
    raise RuntimeError(f"unknown prune status: {result.status}")


def _print_offload_result(result: OffloadResult) -> None:
    typer.echo(format_offload_result(result))


@offload_app.command("status")
def offload_status(
    json_output: bool = typer.Option(
        False,
        "--json",
        help="Print machine-readable JSON.",
    ),
) -> None:
    """Show media offload status."""
    payload = build_offload_status()
    if json_output:
        _echo_json(payload)
        return
    offload = payload["offload"]
    raw = payload["raw_media"]
    backup_only = payload["backup_only"]
    pending_release = payload["pending_release"]
    typer.echo(
        "backup offload: "
        f"enabled={offload['enabled']} "
        f"raw_media_bytes={raw['total_bytes']} "
        f"pending_release_bytes={pending_release['total_bytes']} "
        f"backup_only_bytes={backup_only['total_bytes']} "
        f"degraded={backup_only['degraded']}"
    )


@offload_app.command("run")
def offload_run(
    dry_run: bool = typer.Option(False, "--dry-run", help="Preview media offload."),
) -> None:
    """Run media offload now."""
    _print_offload_result(run_offload(dry_run=dry_run))


@offload_app.command("restore")
def offload_restore(
    day: str | None = typer.Argument(None, help="Day to restore in YYYYMMDD form."),
    all_: bool = typer.Option(False, "--all", help="Restore all offloaded media."),
    json_output: bool = typer.Option(
        False,
        "--json",
        help="Print machine-readable JSON.",
    ),
) -> None:
    """Restore offloaded media for one day or all days."""
    if all_ and day is not None:
        _die("Use either a day or --all, not both.")
    if not all_ and day is None:
        _die("Provide a day or --all.")
    try:
        result = restore_all() if all_ else restore_day(str(day))
    except ValueError:
        _die("Invalid day.")
    if json_output:
        _echo_json(asdict(result))
    else:
        typer.echo(
            "backup offload restore: "
            f"status={result.status} reason={result.reason} "
            f"segments_restored={result.segments_restored} "
            f"files_restored={result.files_restored} "
            f"bytes_restored={result.bytes_restored}"
        )
    if result.status in {"refused", "degraded", "error"}:
        raise typer.Exit(1)


@recovery_key_app.command("show")
def recovery_key_show() -> None:
    """Show the configured recovery key."""
    keys = get_keys()
    if keys is None:
        _die("No recovery key is set.")
    _print_recovery_grid(keys.recovery_key_display)


@recovery_key_app.command("rotate")
def recovery_key_rotate() -> None:
    """Rotate the backup recovery key."""
    result = rotate_recovery_key()
    if result.status == "error":
        _die(f"Rotation failed: {result.reason_code}.")
    if result.status == "skipped":
        typer.echo("Recovery key rotation skipped (backup not configured).")
        return
    if result.status == "ok":
        typer.echo("New recovery key (write it down):")
        _print_recovery_grid(result.recovery_key_display)
        return
    raise RuntimeError(f"unknown rotation status: {result.status}")


@app.command("restore")
def restore() -> None:
    """Restore a journal from backup."""
    payload = _read_stdin_json()
    destination = _destination_from_payload(payload)
    recovery_key = payload.get("recovery_key")
    if not isinstance(recovery_key, str) or not recovery_key.strip():
        _die("Missing recovery_key.")

    result = restore_journal(destination, recovery_key.strip())
    if result.status == "error":
        _die(f"Restore failed: {result.reason_code}.")
    if result.status == "degraded":
        if result.reason_code == "integrity_unverified":
            detail = (
                "integrity verification could not run "
                "(the repository was busy or timed out)"
            )
        else:
            detail = "integrity verification failed — the backup copy may be damaged"
        _die(
            f"Restored {result.bytes_restored} bytes and saved the recovery key, "
            f"but {detail} (reason_code={result.reason_code})."
        )
    if result.status == "ok":
        typer.echo(
            f"Restore complete: {result.bytes_restored} bytes, "
            f"integrity_ok={result.integrity_ok}, resumable={result.resumable}."
        )
        return
    raise RuntimeError(f"unknown restore status: {result.status}")


@app.command("off")
def off(
    yes: bool = typer.Option(
        False,
        "--yes",
        help="Confirm teardown of backup snapshots.",
    ),
) -> None:
    """Turn off solstone backup and forget snapshots."""
    if not yes:
        _die("Refusing to tear down backup without --yes. This forgets all snapshots.")

    result = teardown_backup()
    if result.status == "error":
        _die(f"Teardown failed: {result.reason_code}.")
    if result.status in {"skipped", "ok"}:
        typer.echo("Backup turned off.")
        return
    raise RuntimeError(f"unknown teardown status: {result.status}")


def main() -> None:
    """Entry point for ``journal backup``."""
    app()


if __name__ == "__main__":
    main()
