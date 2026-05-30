# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unified CLI for solstone - AI-driven desktop journaling toolkit.

Usage:
    sol                     Show status and available commands
    sol <command> [args]    Run a subcommand
    sol <module> [args]     Run by module path (e.g., sol solstone.think.importers.cli)

Examples:
    sol import data.json    Import data into journal
    journal think 20250101      Run daily processing for a day
    sol solstone.think.talents -h    Show help for specific module
"""

from __future__ import annotations

import importlib
import os
import sys
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _pkg_version
from typing import Any, Literal, NamedTuple

import setproctitle

# =============================================================================
# Command Registry
# =============================================================================
# Maps short command names to module paths.
# All modules must have a main() function as entry point.
#
# To add a new command:
#   1. Add entry here: "name": "package.module"
#   2. Ensure module has main() function
#
# Aliases for compound commands can be added to ALIASES dict below.
# =============================================================================


class Command(NamedTuple):
    module: str
    surface: Literal["access", "service", "universal"]


class Alias(NamedTuple):
    module: str
    preset_args: list[str]
    surface: Literal["access", "service", "universal"]


class HelpGroup(NamedTuple):
    heading: str
    commands: tuple[str, ...]


JOURNAL_ACCESS_CMD_ERROR = (
    "'{cmd}' is a journal-access command — run it with 'sol {cmd}' instead.\n"
    "('journal' surfaces only journal-service commands; see 'journal --help'.)"
)
SOL_SERVICE_CMD_REMOVED_ERROR = (
    "'{cmd}' moved to 'journal {cmd}' in solstone 0.4.0 — run that instead.\n"
    "('sol' is the journal-access surface; 'journal' surfaces journal-service "
    "commands; see 'journal --help'.)"
)

SOL_HELP_GROUP_CONVERSATION = "Conversation"
SOL_HELP_GROUP_YOUR_JOURNAL = "Your journal"
SOL_HELP_GROUP_DIAGNOSE = "See & diagnose"
SOL_HELP_GROUP_TOOLS = "Tools"
SOL_HELP_GROUP_SERVICE_HEADING = "Journal service (also available as `journal <cmd>`)"
SOL_HELP_GROUP_ALIASES = "Aliases"


COMMANDS: dict[str, Command] = {
    # think package - daily processing and analysis
    "import": Command("solstone.think.importers.cli", "access"),
    "think": Command("solstone.think.thinking", "service"),
    "indexer": Command("solstone.think.indexer", "access"),
    "start": Command("solstone.think.start", "service"),
    "supervisor": Command("solstone.think.supervisor", "service"),
    "schedule": Command("solstone.think.scheduler", "service"),
    "top": Command("solstone.think.top", "access"),
    "health": Command("solstone.think.health_cli", "access"),
    "notify": Command("solstone.think.notify_cli", "access"),
    "doctor": Command("solstone.think.doctor", "universal"),
    "config": Command("solstone.think.config_cli", "service"),
    "install-models": Command("solstone.think.install_models", "service"),
    "skills": Command("solstone.think.skills_cli", "access"),
    "password": Command("solstone.think.password_cli", "service"),
    "streams": Command("solstone.think.streams", "access"),
    "segment": Command("solstone.think.segment", "access"),
    "journal-stats": Command("solstone.think.journal_stats", "access"),
    "reprocess": Command("solstone.think.reprocess", "access"),
    # observe package - multimodal capture
    "transcribe": Command("solstone.observe.transcribe", "service"),
    "describe": Command("solstone.observe.describe", "service"),
    "sense": Command("solstone.observe.sense", "service"),
    "transfer": Command("solstone.observe.transfer", "service"),
    "export": Command("solstone.observe.export", "service"),
    "grab": Command("solstone.observe.grab", "service"),
    "observer": Command("solstone.observe.observer_cli", "access"),
    # AI providers and talent execution
    "providers": Command("solstone.think.providers_cli", "access"),
    "cortex": Command("solstone.think.cortex", "service"),
    "talent": Command("solstone.think.talent_cli", "service"),
    "link": Command("solstone.think.link", "access"),
    "spl": Command("solstone.think.spl", "service"),
    "call": Command("solstone.think.call", "access"),
    "engage": Command("solstone.think.engage", "access"),
    "chat": Command("solstone.think.chat_cli", "access"),
    "heartbeat": Command("solstone.think.heartbeat", "service"),
    # convey package - web UI
    "convey": Command("solstone.convey.cli", "service"),
    "restart-convey": Command("solstone.convey.restart", "access"),
    "maint": Command("solstone.convey.maint_cli", "service"),
    "service": Command("solstone.think.service", "service"),
    "services": Command("solstone.think.services", "service"),
    "setup": Command("solstone.think.setup", "service"),
}

# =============================================================================
# Aliases for Compound Commands
# =============================================================================
# Maps alias names to (module, default_args) tuples.
# These provide shortcuts for common operations with preset arguments.
#
# Example: "reindex": ("solstone.think.indexer", ["--rescan"])
#   Running "sol reindex" is equivalent to "sol indexer --rescan"
# =============================================================================

ALIASES: dict[str, Alias] = {
    "up": Alias("solstone.think.service", ["up"], "service"),
    "down": Alias("solstone.think.service", ["down"], "service"),
}

# Owner-facing command groupings for `sol --help`.
#
# Access-tagged commands are assigned to one of the four intent groups below.
# Future access commands must be assigned here deliberately.
ACCESS_HELP_GROUPS: tuple[HelpGroup, ...] = (
    HelpGroup(SOL_HELP_GROUP_CONVERSATION, ("chat", "engage")),
    HelpGroup(
        SOL_HELP_GROUP_YOUR_JOURNAL,
        ("call", "import", "journal-stats", "segment", "streams", "indexer"),
    ),
    HelpGroup(
        SOL_HELP_GROUP_DIAGNOSE, ("top", "health", "notify", "doctor", "reprocess")
    ),
    HelpGroup(
        SOL_HELP_GROUP_TOOLS,
        ("providers", "observer", "skills", "restart-convey", "link"),
    ),
)


def get_status() -> dict[str, Any]:
    """Return current journal status information."""
    from solstone.think.utils import get_journal_info

    path, source = get_journal_info()

    return {
        "journal_path": path,
        "journal_source": source,
        "journal_exists": os.path.isdir(path),
    }


def print_status() -> None:
    """Print current journal status."""
    status = get_status()

    print(f"Journal: {status['journal_path']}")
    if status["journal_exists"]:
        from solstone.think.utils import day_dirs

        print(f"Days: {len(day_dirs())}")
    print()


def service_help_group() -> HelpGroup:
    """Return the derived Journal service help group in registry order."""
    return HelpGroup(
        SOL_HELP_GROUP_SERVICE_HEADING,
        tuple(
            name for name, command in COMMANDS.items() if command.surface == "service"
        ),
    )


def help_groups() -> tuple[HelpGroup, ...]:
    """Return all owner-facing help groups in display order."""
    return ACCESS_HELP_GROUPS


def _print_help_group(group: HelpGroup) -> None:
    print(group.heading)
    for cmd in group.commands:
        if cmd in COMMANDS:
            module = COMMANDS[cmd].module
            print(f"  {cmd:16} {module}")
    print()


def _alias_target_label(alias: Alias) -> str:
    for name, command in COMMANDS.items():
        if command.module == alias.module and not alias.preset_args:
            return name
        if command.module == alias.module and alias.preset_args:
            return " ".join([name] + alias.preset_args)
    return " ".join([alias.module] + alias.preset_args)


def print_help() -> None:
    """Print help with status and available commands."""
    print("sol - solstone unified CLI\n")
    print_status()

    print("Usage: sol <command> [args...]\n")

    for group in help_groups():
        _print_help_group(group)

    # Print call sub-apps
    try:
        from solstone.think.call import call_app

        print("Apps (sol call <app>):")
        for group in call_app.registered_groups:
            name = group.name or ""
            help_text = group.typer_instance.info.help
            if not isinstance(help_text, str):
                help_text = ""
            print(f"  call {name:16} {help_text}")
        print()
    except Exception:
        pass


def print_journal_help() -> None:
    """Print help for the journal service command surface."""
    print("journal - solstone journal service CLI\n")
    print_status()

    print("Usage: journal <command> [options]\n")

    print("Commands:")
    for name, command in sorted(COMMANDS.items()):
        if command.surface in ("service", "universal"):
            print(f"  {name:16} {command.module}")
    print()

    service_aliases = [
        (name, command_alias)
        for name, command_alias in ALIASES.items()
        if command_alias.surface == "service"
    ]
    if service_aliases:
        print("Aliases:")
        for name, command_alias in service_aliases:
            args_str = (
                " ".join(command_alias.preset_args) if command_alias.preset_args else ""
            )
            print(f"  {name:16} → {command_alias.module} {args_str}")
        print()

    print("Options:")
    print("  --help, -h        Show this help")
    print("  --version, -V     Show version")
    print("  --path            Print resolved journal path")
    print("  root              Print project root")
    print()
    print("Direct module syntax: journal <module.path> [args]")
    print("Example: journal solstone.think.supervisor --help")


def resolve_command(name: str) -> tuple[str, list[str], str]:
    """Resolve command name to module path and any preset args.

    Args:
        name: Command name, alias, or module path

    Returns:
        Tuple of (module_path, preset_args, surface)

    Raises:
        ValueError: If command not found
    """
    # Check aliases first (they override commands)
    if name in ALIASES:
        command_alias = ALIASES[name]
        return command_alias.module, command_alias.preset_args, command_alias.surface

    # Check command registry
    if name in COMMANDS:
        command = COMMANDS[name]
        return command.module, [], command.surface

    # Check if it looks like a module path (contains ".")
    if "." in name:
        return name, [], "service"

    # Not found
    available = sorted(set(COMMANDS.keys()) | set(ALIASES.keys()))
    raise ValueError(
        f"Unknown command: {name}\nAvailable commands: {', '.join(available[:10])}..."
    )


def run_command(module_path: str) -> int:
    """Import and run a module's main() function.

    Args:
        module_path: Dotted module path (e.g., "solstone.think.importers.cli")

    Returns:
        Exit code (0 for success)
    """
    try:
        module = importlib.import_module(module_path)
    except ImportError as e:
        print(f"Error: Could not import module '{module_path}': {e}", file=sys.stderr)
        return 1

    if not hasattr(module, "main"):
        print(f"Error: Module '{module_path}' has no main() function", file=sys.stderr)
        return 1

    # Call main - it may call sys.exit() internally
    try:
        result = module.main()
        return 0 if result is None else int(result)
    except SystemExit as e:
        # Preserve exit code from subcommand
        # SystemExit can have int code, string message, or None
        if isinstance(e.code, int):
            return e.code
        elif isinstance(e.code, str):
            print(e.code, file=sys.stderr)
            return 1
        else:
            return 0 if not e.code else 1


def _dispatch(binary: str, allowed_surfaces: frozenset[str] | None) -> None:
    """Dispatch a top-level CLI binary to a registered command."""
    # No arguments - show status and help
    if len(sys.argv) < 2:
        if binary == "journal":
            print_journal_help()
        else:
            print_help()
        return

    cmd = sys.argv[1]

    # Help flags
    if cmd in ("--help", "-h"):
        if binary == "journal":
            print_journal_help()
        else:
            print_help()
        return
    if cmd == "help" and len(sys.argv) <= 2:
        if binary == "journal":
            print_journal_help()
        else:
            print_help()
        return

    # Version flag
    if cmd in ("--version", "-V"):
        try:
            _v = _pkg_version("solstone")
        except PackageNotFoundError:
            _v = "0.0.0+source"
        print(f"{binary} (solstone) {_v}")
        return

    # Path flag
    if cmd == "--path":
        from solstone.think.utils import get_journal_info

        path, _source = get_journal_info()
        print(path)
        return

    # Root command — print repo root for scripting: SOL=$(sol root)
    if cmd == "root":
        from solstone.think.utils import get_project_root

        print(get_project_root())
        return

    # Resolve command to module path
    rest = sys.argv[2:]
    try:
        module_path, preset_args, surface = resolve_command(cmd)
    except ValueError as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)

    if binary == "sol" and surface == "service":
        from solstone.think.service import _managed_wrapper, reconcile_installed_unit

        reconciled = reconcile_installed_unit()
        if (
            reconciled.was_stale
            and reconciled.stale_binary == "sol"
            and reconciled.stale_verb == cmd
        ):
            journal_wrapper = _managed_wrapper("journal")
            # Route through `journal start` (the canonical entry that runs the
            # version-marker / wrapper / skill refresh), not `journal {cmd}`.
            # The unit was just rewritten to `journal start <rest>`; the shim
            # exec should match so the upgrade-time refresh fires on this
            # boot rather than waiting for the next restart.
            os.execv(str(journal_wrapper), [str(journal_wrapper), "start", *rest])
        print(SOL_SERVICE_CMD_REMOVED_ERROR.format(cmd=cmd), file=sys.stderr)
        sys.exit(2)

    if allowed_surfaces is not None and surface not in allowed_surfaces:
        sys.stderr.write(JOURNAL_ACCESS_CMD_ERROR.format(cmd=cmd) + "\n")
        sys.exit(2)

    # Set process title for ps/top visibility
    setproctitle.setproctitle(f"{binary}:{cmd}")

    # Adjust sys.argv for the subcommand
    # Original: ["sol", "import", "--day", "20250101"]
    # Becomes:  ["sol import", "--day", "20250101"]
    # This makes argparse show "usage: <binary> <command> ..." in help.
    sys.argv = [f"{binary} {cmd}"] + preset_args + rest

    # Run the command
    exit_code = run_command(module_path)
    sys.exit(exit_code)


def main() -> None:
    """Main entry point for sol CLI."""
    _dispatch("sol", allowed_surfaces=None)


def journal_main() -> None:
    """Main entry point for journal service CLI."""
    _dispatch("journal", allowed_surfaces=frozenset({"service", "universal"}))


if __name__ == "__main__":
    main()
