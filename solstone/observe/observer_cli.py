# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI for observer management.

Provides commands for creating, listing, revoking, and checking status
of observer registrations. Operates directly on the journal
filesystem — no dependency on the Convey web server.

Usage:
    sol observer create <name>           Create a new observer
    sol observer list                    List all registered observers
    sol observer revoke <name-or-prefix> Revoke an observer registration
    sol observer status [name-or-prefix] Show observer status details
"""

from __future__ import annotations

import argparse
import base64
import datetime
import json
import logging
import secrets
import sys

from solstone.apps.observer.utils import (
    find_observer_by_name,
    get_hist_dir,
    get_observers_dir,
    list_observers,
    load_history,
    observer_filename_prefix,
    observer_mode,
    save_observer,
)
from solstone.apps.utils import log_app_action
from solstone.observe.copy import (
    OBSERVER_LOCALHOST_BANNER_LINE_1,
    OBSERVER_LOCALHOST_BANNER_LINE_2,
    OBSERVER_LOCALHOST_BANNER_LINE_3,
    OBSERVER_LOCALHOST_BANNER_LINE_4,
    OBSERVER_LOCALHOST_REMINDER,
)
from solstone.think.utils import get_config, now_ms, require_solstone, setup_cli

logger = logging.getLogger(__name__)

# Key: 256 bits = 32 bytes, URL-safe base64 (same as web API)
KEY_BYTES = 32

# Connected threshold: last_seen within 2 minutes (matches web UI)
CONNECTED_THRESHOLD_MS = 2 * 60 * 1000


def _generate_key() -> str:
    """Generate a URL-safe key for observer authentication."""
    return base64.urlsafe_b64encode(secrets.token_bytes(KEY_BYTES)).decode().rstrip("=")


def _find_observer(identifier: str) -> dict | None:
    """Find an observer by name or key prefix."""
    # Try name first
    observer = find_observer_by_name(identifier)
    if observer:
        return observer

    # Try key prefix (file is named <prefix>.json)
    observers_dir = get_observers_dir()
    observer_path = observers_dir / f"{identifier}.json"
    if observer_path.exists():
        try:
            with open(observer_path) as f:
                return json.load(f)
        except (json.JSONDecodeError, OSError):
            pass

    return None


def _status_label(observer: dict) -> str:
    """Get human-readable connection status."""
    if observer.get("revoked", False):
        return "revoked"
    last_seen = observer.get("last_seen")
    if last_seen is None:
        return "disconnected"
    if now_ms() - last_seen < CONNECTED_THRESHOLD_MS:
        return "connected"
    return "disconnected"


def _fmt_bytes(n: int) -> str:
    """Format byte count for display."""
    if n < 1024:
        return f"{n} B"
    elif n < 1024 * 1024:
        return f"{n / 1024:.1f} KB"
    elif n < 1024 * 1024 * 1024:
        return f"{n / (1024 * 1024):.1f} MB"
    return f"{n / (1024 * 1024 * 1024):.1f} GB"


def _fmt_time(ms: int | None) -> str:
    """Format millisecond timestamp for display."""
    if ms is None:
        return "never"
    dt = datetime.datetime.fromtimestamp(ms / 1000)
    return dt.strftime("%Y-%m-%d %H:%M")


def create_observer_record(
    name: str,
    *,
    permit_duplicate_name: bool = False,
    reuse_existing: bool = False,
) -> tuple[dict, str, bool]:
    """Create and save an observer record, returning record, raw key, and reused flag."""
    existing_active = [
        observer
        for observer in list_observers()
        if observer.get("name") == name and not observer.get("revoked", False)
    ]
    if existing_active and reuse_existing:
        return existing_active[0], existing_active[0]["key"], True
    if existing_active and not permit_duplicate_name:
        raise ValueError(f"observer already exists: {name}")

    key = _generate_key()
    observer_data = {
        "key": key,
        "name": name,
        "created_at": now_ms(),
        "last_seen": None,
        "last_segment": None,
        "enabled": True,
        "stats": {
            "segments_received": 0,
            "bytes_received": 0,
        },
    }

    if not save_observer(observer_data):
        raise RuntimeError("failed to save observer")

    log_app_action(
        app="observer",
        facet=None,
        action="observer_create",
        params={"name": name, "key_prefix": key[:8]},
    )
    return observer_data, key, False


def revoke_observer_record(identifier: str) -> dict:
    """Revoke an observer registration and return the mutated record."""
    observer = _find_observer(identifier)
    if not observer:
        raise ValueError(f"observer not found: {identifier}")

    if observer.get("revoked", False):
        raise ValueError(f"observer already revoked: {observer.get('name')}")

    name = observer.get("name", "")
    key_prefix = observer_filename_prefix(observer)
    observer["revoked"] = True
    observer["revoked_at"] = now_ms()

    if not save_observer(observer):
        raise RuntimeError("failed to save observer")

    log_app_action(
        app="observer",
        facet=None,
        action="observer_revoke",
        params={"name": name, "key_prefix": key_prefix},
    )
    return observer


# === Subcommands ===


def cmd_create(args: argparse.Namespace) -> int:
    """Create a new observer registration."""
    name = args.name

    try:
        observer_data, key, reused = create_observer_record(
            name, reuse_existing=args.reuse_existing
        )
    except ValueError:
        print(f"Error: observer '{name}' already exists", file=sys.stderr)
        return 1
    except RuntimeError:
        print("Error: failed to save observer", file=sys.stderr)
        return 1

    if args.json_output:
        print(json.dumps({"name": name, "key": key, "prefix": key[:8]}))
        return 0

    allow_network_access = bool(
        get_config().get("convey", {}).get("allow_network_access", False)
    )
    if not allow_network_access:
        print()
        print(OBSERVER_LOCALHOST_BANNER_LINE_1)
        print(OBSERVER_LOCALHOST_BANNER_LINE_2)
        print(OBSERVER_LOCALHOST_BANNER_LINE_3)
        print(OBSERVER_LOCALHOST_BANNER_LINE_4)
        print()
    print("Reusing existing observer:" if reused else "Observer created:")
    print(f"  Name:       {name}")
    print(f"  Prefix:     {key[:8]}")
    print("  server url:  (set during server configuration)")
    print(f"  api key:     {key}")
    if not allow_network_access:
        print()
        print(OBSERVER_LOCALHOST_REMINDER)
    return 0


def cmd_list(args: argparse.Namespace) -> int:
    """List all registered observers."""
    observers = list_observers()

    if args.json_output:
        result = []
        for r in observers:
            stats = r.get("stats", {})
            result.append(
                {
                    "name": r.get("name", ""),
                    "mode": observer_mode(r),
                    "prefix": observer_filename_prefix(r),
                    "status": _status_label(r),
                    "last_seen": r.get("last_seen"),
                    "segments": stats.get("segments_received", 0),
                    "bytes": stats.get("bytes_received", 0),
                }
            )
        print(json.dumps(result))
        return 0

    if not observers:
        print("No observers registered.")
        return 0

    print(
        f"{'Name':<20} {'Mode':<5} {'Prefix':<18} {'Status':<14} "
        f"{'Last Seen':<18} {'Segments':>10} {'Bytes':>12}"
    )
    print("-" * 100)

    for r in observers:
        name = r.get("name", "")
        mode = observer_mode(r)
        prefix = observer_filename_prefix(r)
        status = _status_label(r)
        last_seen = _fmt_time(r.get("last_seen"))
        stats = r.get("stats", {})
        segments = stats.get("segments_received", 0)
        bytes_recv = _fmt_bytes(stats.get("bytes_received", 0))
        print(
            f"{name:<20} {mode:<5} {prefix:<18} {status:<14} "
            f"{last_seen:<18} {segments:>10} {bytes_recv:>12}"
        )

    return 0


def cmd_revoke(args: argparse.Namespace) -> int:
    """Revoke an observer registration (soft-delete)."""
    identifier = args.identifier

    try:
        observer = revoke_observer_record(identifier)
    except ValueError as exc:
        message = str(exc)
        if message.startswith("observer not found:"):
            print(f"Error: observer '{identifier}' not found", file=sys.stderr)
            return 1
        name = message.removeprefix("observer already revoked: ")
        print(f"Observer '{name}' is already revoked.", file=sys.stderr)
        return 1
    except RuntimeError:
        print("Error: failed to save observer", file=sys.stderr)
        return 1

    name = observer.get("name", "")
    key_prefix = observer_filename_prefix(observer)

    if args.json_output:
        print(json.dumps({"name": name, "prefix": key_prefix, "revoked": True}))
        return 0

    print(f"Revoked observer '{name}' ({key_prefix})")
    return 0


def cmd_install(args: argparse.Namespace) -> int:
    """Install an observer for this host."""
    from solstone.observe.observer_install import run_install

    return run_install(args)


def cmd_rename(args: argparse.Namespace) -> int:
    """Rename an observer (affects future stream names)."""
    identifier = args.identifier
    new_name = args.new_name

    observer = _find_observer(identifier)
    if not observer:
        print(f"Error: observer '{identifier}' not found", file=sys.stderr)
        return 1

    # Check new name isn't taken
    existing = find_observer_by_name(new_name)
    if existing and existing.get("key") != observer.get("key"):
        print(f"Error: observer '{new_name}' already exists", file=sys.stderr)
        return 1

    old_name = observer.get("name", "")
    if old_name == new_name:
        print(f"Observer is already named '{new_name}'.", file=sys.stderr)
        return 1

    key_prefix = observer_filename_prefix(observer)
    observer["name"] = new_name

    if not save_observer(observer):
        print("Error: failed to save observer", file=sys.stderr)
        return 1

    log_app_action(
        app="observer",
        facet=None,
        action="observer_rename",
        params={"old_name": old_name, "new_name": new_name, "key_prefix": key_prefix},
    )

    if args.json_output:
        print(
            json.dumps(
                {"old_name": old_name, "new_name": new_name, "prefix": key_prefix}
            )
        )
        return 0

    print(f"Renamed observer '{old_name}' -> '{new_name}' ({key_prefix})")
    print(f"  Future segments will use stream: {new_name}")
    print(f"  Existing segments remain under stream: {old_name}")
    return 0


def cmd_status(args: argparse.Namespace) -> int:
    """Show observer status details."""
    if args.identifier:
        return _status_single(args.identifier, json_output=args.json_output)
    return _status_all(json_output=args.json_output)


def _status_single(identifier: str, json_output: bool = False) -> int:
    """Detailed status for a single observer."""
    observer = _find_observer(identifier)
    if not observer:
        print(f"Error: observer '{identifier}' not found", file=sys.stderr)
        return 1

    name = observer.get("name", "")
    mode = observer_mode(observer)
    key_prefix = observer_filename_prefix(observer)
    stats = observer.get("stats", {})

    if json_output:
        print(
            json.dumps(
                {
                    "name": name,
                    "mode": mode,
                    "prefix": key_prefix,
                    "status": _status_label(observer),
                    "created_at": observer.get("created_at"),
                    "last_seen": observer.get("last_seen"),
                    "revoked": observer.get("revoked", False),
                    "segments": stats.get("segments_received", 0),
                    "bytes": stats.get("bytes_received", 0),
                }
            )
        )
        return 0

    print(f"Observer: {name}")
    print(f"  Mode:       {mode}")
    print(f"  Prefix:     {key_prefix}")
    print(f"  Status:     {_status_label(observer)}")
    print(f"  Created:    {_fmt_time(observer.get('created_at'))}")
    print(f"  Last seen:  {_fmt_time(observer.get('last_seen'))}")
    if observer.get("revoked"):
        print(f"  Revoked at: {_fmt_time(observer.get('revoked_at'))}")
    print(f"  Segments:   {stats.get('segments_received', 0)}")
    print(f"  Bytes:      {_fmt_bytes(stats.get('bytes_received', 0))}")
    if stats.get("duplicates_rejected"):
        print(f"  Duplicates: {stats['duplicates_rejected']} rejected")
    _print_install_status(name)

    # Today's sync history
    today = datetime.date.today().strftime("%Y%m%d")
    history = load_history(key_prefix, today)
    if history:
        uploads = [r for r in history if not r.get("type")]
        print(f"\n  Today ({today}): {len(uploads)} segment(s) synced")
        for rec in uploads[-5:]:
            seg = rec.get("segment", "?")
            files = rec.get("files", [])
            total = sum(f.get("size", 0) for f in files)
            ts = _fmt_time(rec.get("ts"))
            print(f"    {seg}  {len(files)} file(s)  {_fmt_bytes(total)}  {ts}")

    # Segment count by recent days
    hist_dir = get_hist_dir(key_prefix, ensure_exists=False)
    if hist_dir.exists():
        day_files = sorted(hist_dir.glob("*.jsonl"), reverse=True)[:7]
        if day_files:
            print("\n  Recent days:")
            for df in day_files:
                day = df.stem
                records = load_history(key_prefix, day)
                day_uploads = [r for r in records if not r.get("type")]
                print(f"    {day}: {len(day_uploads)} segment(s)")

    return 0


def _print_install_status(name: str) -> None:
    """Print install marker details for human status output."""
    from solstone.observe.observer_install.common import (
        SERVICE_UNITS,
        find_marker_for_observer,
        run_probe,
    )

    marker = find_marker_for_observer(name)
    if marker is None:
        return

    _path, data = marker
    platform_name = data.get("platform", "unknown")
    version = data.get("version") or "unknown"
    short_version = version[:12] if version != "unknown" else version
    installed_at = data.get("installed_at") or "unknown"
    print(f"  Installed: {installed_at} ({platform_name}, version {short_version})")

    unit_name = SERVICE_UNITS.get(platform_name)
    if not unit_name:
        return
    process = run_probe(["systemctl", "--user", "is-active", unit_name])
    service_status = process.stdout.strip()
    if not service_status:
        service_status = "missing" if process.returncode == 127 else "inactive"
    print(f"  Service:   {unit_name} — {service_status}")


def _status_all(json_output: bool = False) -> int:
    """Health overview for all observers."""
    observers = list_observers()

    if not observers and not json_output:
        print("No observers registered.")
        return 0

    connected = sum(1 for r in observers if _status_label(r) == "connected")
    disconnected = sum(1 for r in observers if _status_label(r) == "disconnected")
    revoked = sum(1 for r in observers if _status_label(r) == "revoked")
    total_segments = sum(
        r.get("stats", {}).get("segments_received", 0) for r in observers
    )
    total_bytes = sum(r.get("stats", {}).get("bytes_received", 0) for r in observers)

    if json_output:
        print(
            json.dumps(
                {
                    "total": len(observers),
                    "connected": connected,
                    "disconnected": disconnected,
                    "revoked": revoked,
                    "total_segments": total_segments,
                    "total_bytes": total_bytes,
                    "observers": [
                        {
                            "name": r.get("name", ""),
                            "mode": observer_mode(r),
                            "prefix": observer_filename_prefix(r),
                            "status": _status_label(r),
                            "last_seen": r.get("last_seen"),
                        }
                        for r in observers
                    ],
                }
            )
        )
        return 0

    print(f"Observers: {len(observers)} total")
    print(f"  Connected:    {connected}")
    print(f"  Disconnected: {disconnected}")
    print(f"  Revoked:      {revoked}")
    print(f"  Total segments: {total_segments}")
    print(f"  Total bytes:    {_fmt_bytes(total_bytes)}")

    print(f"\n{'Name':<20} {'Mode':<5} {'Prefix':<18} {'Status':<14} {'Last Seen':<18}")
    print("-" * 80)
    for r in observers:
        name = r.get("name", "")
        mode = observer_mode(r)
        prefix = observer_filename_prefix(r)
        status = _status_label(r)
        last_seen = _fmt_time(r.get("last_seen"))
        print(f"{name:<20} {mode:<5} {prefix:<18} {status:<14} {last_seen:<18}")

    return 0


# === Entry point ===


def main() -> None:
    """Entry point for sol observer CLI."""
    parser = argparse.ArgumentParser(
        prog="sol observer",
        description="Manage observer registrations",
    )

    parser.add_argument(
        "--json", action="store_true", dest="json_output", help="Output as JSON"
    )

    sub = parser.add_subparsers(dest="command")

    # create
    p_create = sub.add_parser("create", help="Create a new observer")
    p_create.add_argument("name", help="Name for the observer")
    p_create.add_argument(
        "--reuse-existing",
        action="store_true",
        dest="reuse_existing",
        help="Reuse an active observer with this name instead of failing.",
    )

    # list
    sub.add_parser("list", help="List all registered observers")

    # rename
    p_rename = sub.add_parser(
        "rename", help="Rename an observer (affects future streams)"
    )
    p_rename.add_argument("identifier", help="Observer name or key prefix")
    p_rename.add_argument("new_name", help="New name for the observer")

    # revoke
    p_revoke = sub.add_parser("revoke", help="Revoke an observer registration")
    p_revoke.add_argument("identifier", help="Observer name or key prefix")

    # status
    p_status = sub.add_parser("status", help="Show observer status details")
    p_status.add_argument(
        "identifier",
        nargs="?",
        default=None,
        help="Observer name or key prefix (omit for overview)",
    )

    # install
    p_install = sub.add_parser("install", help="Install an observer for this host")
    p_install.add_argument(
        "name",
        nargs="?",
        default=None,
        help="Observer name (defaults to this host)",
    )
    p_install.add_argument(
        "--platform",
        choices=["linux", "tmux", "macos"],
        default=None,
        help="Observer platform (default: auto-detect)",
    )
    p_install.add_argument("--server-url", default=None, help="solstone server URL")
    p_install.add_argument(
        "--dry-run", action="store_true", help="Show the install plan without writes"
    )
    p_install.add_argument(
        "--force", action="store_true", help="Recreate registration and rerun install"
    )
    p_install.add_argument(
        "--observer-version",
        default=None,
        dest="observer_version",
        metavar="VERSION",
        help=(
            "Override the pinned observer package version "
            "(developer use; bypasses the version pinned by sol)."
        ),
    )

    args = setup_cli(parser)

    # Keep app helpers aligned with the active CLI journal.
    import solstone.convey.state as convey_state
    from solstone.think.utils import get_journal

    convey_state.journal_root = get_journal()

    if args.command != "install":
        require_solstone()

    if not args.command:
        parser.print_help()
        sys.exit(1)

    handlers = {
        "create": cmd_create,
        "list": cmd_list,
        "rename": cmd_rename,
        "revoke": cmd_revoke,
        "status": cmd_status,
        "install": cmd_install,
    }

    sys.exit(handlers[args.command](args))
