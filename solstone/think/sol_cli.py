# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Journal host CLI dispatcher.

The public `sol` and `solstone` access commands are root-owned launchers for the
sibling `solstone-core` binary. This module survives only as the `journal`
console script entry point for service and universal commands.
"""

from __future__ import annotations

import importlib
import logging
import os
import sys
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _pkg_version
from pathlib import Path
from typing import Any, Literal, NamedTuple

import setproctitle

from solstone.think.generated.access_rejections import JOURNAL_ACCESS_ONLY_COMMANDS


class Command(NamedTuple):
    module: str
    surface: Literal["service", "universal"]
    native: bool = False


class Alias(NamedTuple):
    module: str
    preset_args: list[str]
    surface: Literal["service", "universal"]


class HelpGroup(NamedTuple):
    heading: str
    commands: tuple[str, ...]


JOURNAL_VERSION_MISMATCH_ERROR = """Journal package versions are out of sync.

solstone is installed at {solstone_version}, but {leaf_name} is installed at {leaf_version}.
This usually happens when a bare `pip install --upgrade solstone` upgraded the
thin client without upgrading the journal package.

Upgrade the installed journal package:
    pip install --upgrade {leaf_name}
    uv tool install --upgrade {leaf_name}
"""
JOURNAL_HOST_SHIM_MIGRATION_ERROR = """solstone[journal] and solstone-journal-host have moved.

The journal is now its own package:

    pip install solstone-journal          # the journal (CPU)
    pip install solstone-journal-cuda     # the journal on NVIDIA CUDA

One-time migration for uv tool installs:

    uv tool uninstall solstone && uv tool install solstone-journal && uv tool install solstone

Nothing was changed by this failed command.
See https://github.com/solpbc/solstone-journal/blob/main/INSTALL.md
"""
JOURNAL_ACCESS_CMD_ERROR = (
    "'{cmd}' is a journal-access command — run it with 'sol {cmd}' instead.\n"
    "('journal' surfaces only journal-service commands; see 'journal --help'.)"
)
_DESCRIBE_BINARY_NAME = "solstone-core-describe"
_DESCRIBE_BINARY_ENV = "SOLSTONE_DESCRIBE_BIN"


def describe_path_for_executable(executable: str | None = None) -> Path:
    """Return the installed describe sibling for an executable path."""
    return Path(executable or sys.executable).with_name(_DESCRIBE_BINARY_NAME)


def resolve_describe_binary(executable: str | None = None) -> Path | None:
    """Resolve installed describe first, then the explicit development override."""
    sibling = describe_path_for_executable(executable)
    if sibling.is_file() and os.access(sibling, os.X_OK):
        return sibling
    override = os.environ.get(_DESCRIBE_BINARY_ENV)
    if not override:
        return None
    candidate = Path(override)
    if candidate.is_file() and os.access(candidate, os.X_OK):
        return candidate
    return None


def _installed_packaging_versions() -> dict[str, str | None]:
    """Installed versions of the split dists, each a version string or None."""
    result: dict[str, str | None] = {}
    for name in (
        "solstone",
        "solstone-journal",
        "solstone-journal-cuda",
        "solstone-journal-host",
    ):
        try:
            result[name] = _pkg_version(name)
        except PackageNotFoundError:
            result[name] = None
    return result


def _guard_journal_coherence() -> None:
    versions = _installed_packaging_versions()
    solstone_version = versions["solstone"]
    if solstone_version is None:
        return  # source checkout / exotic env
    leaves = {
        name: v
        for name, v in (
            ("solstone-journal", versions["solstone-journal"]),
            ("solstone-journal-cuda", versions["solstone-journal-cuda"]),
        )
        if v is not None
    }
    if any(v == solstone_version for v in leaves.values()):
        return
    if leaves:
        leaf_name = (
            "solstone-journal-cuda"
            if "solstone-journal-cuda" in leaves
            else "solstone-journal"
        )
        leaf_version = leaves[leaf_name]
        sys.stderr.write(
            JOURNAL_VERSION_MISMATCH_ERROR.format(
                solstone_version=solstone_version,
                leaf_name=leaf_name,
                leaf_version=leaf_version,
            )
        )
        sys.exit(1)
    if versions["solstone-journal-host"] is not None:
        sys.stderr.write(JOURNAL_HOST_SHIM_MIGRATION_ERROR)
        sys.exit(1)


COMMANDS: dict[str, Command] = {
    "backup": Command("solstone.think.backup_cli", "service"),
    "importer": Command("solstone.think.importers.cli", "service"),
    "think": Command("solstone.think.thinking", "service"),
    "indexer": Command("solstone.think.indexer", "service"),
    "start": Command("solstone.think.start", "service"),
    "supervisor": Command("solstone.think.supervisor", "service"),
    "schedule": Command("solstone.think.scheduler", "service"),
    "maintenance": Command("solstone.think.maintenance_cli", "service"),
    "top": Command("solstone.think.top", "service"),
    "health": Command("solstone.think.health_cli", "service"),
    "doctor": Command("solstone.think.doctor", "universal"),
    "check": Command("solstone.think.check", "universal"),
    "contract": Command("solstone.think.contract_cli", "universal"),
    "config": Command("solstone.think.config_cli", "service"),
    "install-models": Command("solstone.think.install_models", "service"),
    "install-provider": Command("solstone.think.install_provider", "service"),
    "settings": Command("solstone.think.settings_cli", "service"),
    "streams": Command("solstone.think.streams", "service"),
    "segment": Command("solstone.think.segment", "service"),
    "journal-stats": Command("solstone.think.journal_stats", "service"),
    "reprocess": Command("solstone.think.reprocess", "service"),
    "backfill-processing-records": Command(
        "solstone.think.backfill_processing_records", "service"
    ),
    "warm": Command("solstone.think.warm", "service"),
    "transcribe": Command("solstone.observe.transcribe", "service"),
    "describe": Command("solstone-core-describe", "service", native=True),
    "depict": Command("solstone.observe.depict", "service"),
    "sense": Command("solstone.observe.sense", "service"),
    "transfer": Command("solstone.observe.transfer", "service"),
    "export": Command("solstone.observe.export", "service"),
    "grab": Command("solstone.observe.grab", "service"),
    "observer": Command("solstone.observe.observer_cli", "service"),
    "brain": Command("solstone.think.brain_cli", "service"),
    "facet-candidates": Command("solstone.think.facet_candidates_cli", "service"),
    "cortex": Command("solstone.think.cortex", "service"),
    "talent": Command("solstone.think.talent_cli", "service"),
    "spl": Command("solstone.think.spl_native", "service"),
    "navigate": Command("solstone.think.tools.navigate", "service"),
    "identity": Command("solstone.think.tools.sol", "service"),
    "engage": Command("solstone.think.engage", "service"),
    "heartbeat": Command("solstone.think.heartbeat", "service"),
    "convey": Command("solstone.convey.cli", "service"),
    "restart-convey": Command("solstone.convey.restart", "service"),
    "maint": Command("solstone.convey.maint_cli", "service"),
    "service": Command("solstone.think.service", "service"),
    "setup": Command("solstone.think.setup", "service"),
}

ALIASES: dict[str, Alias] = {
    "up": Alias("solstone.think.service", ["up"], "service"),
    "down": Alias("solstone.think.service", ["down"], "service"),
}


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
        "Journal service commands",
        tuple(
            name for name, command in COMMANDS.items() if command.surface == "service"
        ),
    )


def print_journal_help() -> None:
    """Print help for the journal service command surface."""
    print("journal - the journal host CLI (solstone)\n")
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
            print(f"  {name:16} -> {command_alias.module} {args_str}")
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
    """Resolve command name to module path and any preset args."""
    if name in ALIASES:
        command_alias = ALIASES[name]
        return command_alias.module, command_alias.preset_args, command_alias.surface

    if name in COMMANDS:
        command = COMMANDS[name]
        return command.module, [], command.surface

    if "." in name:
        return name, [], "service"

    available = sorted(set(COMMANDS.keys()) | set(ALIASES.keys()))
    raise ValueError(
        f"Unknown command: {name}\nAvailable commands: {', '.join(available[:10])}..."
    )


def run_command(module_path: str) -> int:
    """Import and run a module's main() function."""
    try:
        module = importlib.import_module(module_path)
    except ImportError as e:
        print(f"Error: Could not import module '{module_path}': {e}", file=sys.stderr)
        return 1

    if not hasattr(module, "main"):
        print(f"Error: Module '{module_path}' has no main() function", file=sys.stderr)
        return 1

    try:
        result = module.main()
        return 0 if result is None else int(result)
    except SystemExit as e:
        if isinstance(e.code, int):
            return e.code
        if isinstance(e.code, str):
            print(e.code, file=sys.stderr)
            return 1
        return 0 if not e.code else 1


def _dispatch(binary: str) -> None:
    """Dispatch the journal CLI binary to a registered command."""
    if len(sys.argv) < 2:
        print_journal_help()
        return

    if sys.argv[1] in ("-v", "--verbose"):
        logging.basicConfig(level=logging.DEBUG)
        del sys.argv[1]
        if len(sys.argv) < 2:
            print_journal_help()
            return

    cmd = sys.argv[1]

    if cmd in ("--help", "-h") or (cmd == "help" and len(sys.argv) <= 2):
        print_journal_help()
        return

    if cmd in ("--version", "-V"):
        try:
            _v = _pkg_version("solstone")
        except PackageNotFoundError:
            _v = "0.0.0+source"
        print(f"{binary} (solstone) {_v}")
        return

    if cmd == "--path":
        from solstone.think.utils import get_journal_info

        path, _source = get_journal_info()
        print(path)
        return

    if cmd == "root":
        from solstone.think.utils import get_project_root

        print(get_project_root())
        return

    if cmd in JOURNAL_ACCESS_ONLY_COMMANDS:
        print(JOURNAL_ACCESS_CMD_ERROR.format(cmd=cmd), file=sys.stderr)
        sys.exit(2)

    rest = sys.argv[2:]
    try:
        module_path, preset_args, surface = resolve_command(cmd)
    except ValueError as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)

    if surface == "service":
        _guard_journal_coherence()

    setproctitle.setproctitle(f"{binary}:{cmd}")
    command = COMMANDS.get(cmd)
    if command is not None and command.native:
        native_binary = resolve_describe_binary()
        if native_binary is None:
            print(
                f"Error: native {_DESCRIBE_BINARY_NAME} binary was not found beside "
                f"{sys.executable}; set {_DESCRIBE_BINARY_ENV} to an executable path.",
                file=sys.stderr,
            )
            sys.exit(127)
        os.execv(str(native_binary), [str(native_binary), *rest])

    sys.argv = [f"{binary} {cmd}"] + preset_args + rest
    exit_code = run_command(module_path)
    sys.exit(exit_code)


def journal_main() -> None:
    """Main entry point for journal service CLI."""
    _dispatch("journal")


if __name__ == "__main__":
    journal_main()
