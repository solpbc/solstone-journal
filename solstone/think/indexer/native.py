# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Native journal-indexer command runner.

The journal indexer CLI sends command write operations and journal-index reads
to `solstone-core indexer`. In-process index writers such as backup-restore
rescans, direct `index_file()` callers, chat stream appends, importers,
day-accumulator writes, observer prune, share delete, and entity-merge edge
maintenance continue to call the Python indexer APIs directly.
"""

from __future__ import annotations

import argparse
import json
import logging
import subprocess
import sys
from collections.abc import Callable, Iterable
from pathlib import Path
from typing import Any

from packaging import tags

from solstone.think import core_handshake, probe
from solstone.think.utils import resolve_journal_path

logger = logging.getLogger(__name__)

EXIT_UNAVAILABLE = 69
EXIT_USAGE = 64
EXIT_TEMPFAIL = 75

UNSUPPORTED_HOST_MESSAGE = (
    "journal indexer requires a compatible solstone-core wheel for command "
    "writes. Supported wheel platforms: Linux x86_64 glibc 2.17+ "
    "(manylinux2014); Linux aarch64 glibc 2.17+ (manylinux2014); macOS 14.0+ "
    "arm64. This host has no compatible solstone-core wheel. Use a supported "
    "host or install a compatible solstone-core wheel, then rerun journal "
    "indexer."
)
HANDSHAKE_SKIP_MESSAGE = (
    "journal indexer requires solstone-core for command writes, but "
    "solstone-core distribution metadata is missing in this source checkout. "
    "Run make install to restore the journal-host environment, then rerun "
    "journal indexer."
)
HANDSHAKE_FAIL_MESSAGE = "journal indexer cannot run native command writes: {message}"
NATIVE_USAGE_MESSAGE = (
    "journal indexer usage error: native solstone-core indexer rejected the "
    "command arguments with exit 64. This is a command argument-construction "
    "bug; update solstone or report the full journal indexer command."
)
NATIVE_UNAVAILABLE_MESSAGE = (
    "journal indexer native command exited 69 (unsupported input). The Python "
    "command-write path has been retired; remove or change the unsupported "
    "input and rerun journal indexer."
)
NATIVE_TEMPFAIL_MESSAGE = (
    "journal indexer native command exited 75 (temporary failure). Fix the "
    "reported cause and rerun journal indexer."
)
NATIVE_LAUNCH_FAILED_MESSAGE = (
    "journal indexer failed to launch solstone-core indexer: {error}. Run make "
    "install in a source checkout or reinstall solstone-journal, then rerun "
    "journal indexer."
)
NATIVE_SIGNAL_MESSAGE = (
    "journal indexer native command died from signal {signal_number} "
    "(returncode {returncode}); treating as temporary failure. Fix the cause "
    "and rerun journal indexer."
)
NATIVE_OTHER_NONZERO_MESSAGE = (
    "journal indexer native command exited {returncode}. Fix the reported "
    "cause and rerun journal indexer."
)
EMPTY_TAIL_MESSAGE = (
    "journal indexer native command had no write operation flags to pass. This "
    "is a command argument-construction bug."
)
COMPOSED_COMMAND_WARNING = (
    "journal indexer warning: this command combined multiple write operations "
    "and the native command did not complete. Some earlier operations may "
    "already have run. Fix the reported cause, then rerun the whole journal "
    "indexer command."
)

READ_UNSUPPORTED_HOST_MESSAGE = (
    "journal indexer requires a compatible solstone-core wheel for journal index "
    "reads. Supported wheel platforms: Linux x86_64 glibc 2.17+ "
    "(manylinux2014); Linux aarch64 glibc 2.17+ (manylinux2014); macOS 14.0+ "
    "arm64. This host has no compatible solstone-core wheel. Use a supported "
    "host or install a compatible solstone-core wheel, then retry the read."
)
READ_HANDSHAKE_SKIP_MESSAGE = (
    "journal indexer requires solstone-core for journal index reads, but "
    "solstone-core distribution metadata is missing in this source checkout. "
    "Run make install to restore the journal-host environment, then retry the read."
)
READ_HANDSHAKE_FAIL_MESSAGE = (
    "journal indexer cannot run native journal index reads: {message}"
)
READ_NATIVE_USAGE_MESSAGE = (
    "journal indexer usage error: native solstone-core indexer rejected the read "
    "arguments with exit 64. This is a command argument-construction bug; update "
    "solstone or report the full journal indexer command."
)
READ_NATIVE_LAUNCH_FAILED_MESSAGE = (
    "journal indexer failed to launch solstone-core indexer for a journal index read: "
    "{error}. Run make install in a source checkout or reinstall solstone-journal, "
    "then retry the read."
)
READ_NATIVE_SIGNAL_MESSAGE = (
    "journal indexer native read command died from signal {signal_number} "
    "(returncode {returncode}); fix the cause and retry the read."
)
READ_NATIVE_OTHER_NONZERO_MESSAGE = (
    "journal indexer native read command exited {returncode}. Fix the reported "
    "cause and retry the read."
)

HandshakeChecker = Callable[[], core_handshake.CoreHandshakeResult]
HelperLocator = Callable[[], Path]
NativeRunner = Callable[..., subprocess.CompletedProcess[Any]]
PlatformReader = Callable[[], probe.CorePlatform]
PlatformTagReader = Callable[[], Iterable[str]]


class NativeIndexerReadError(RuntimeError):
    """Raised when the native indexer cannot satisfy a journal read."""

    def __init__(self, message: str, *, reason: str | None = None) -> None:
        super().__init__(message)
        self.reason = reason


def _packaging_platform_tags() -> set[str]:
    return {tag.platform for tag in tags.sys_tags()}


def runtime_has_solstone_core_wheel_coverage(
    *,
    platform_reader: PlatformReader = probe.current_solstone_core_platform,
    platform_tag_reader: PlatformTagReader = _packaging_platform_tags,
) -> bool:
    """Return whether this runtime can install the packaged helper wheel."""
    platform_tuple = platform_reader()
    if platform_tuple not in probe.SOLSTONE_CORE_COVERED_PLATFORMS:
        return False
    expected_platforms = probe.SOLSTONE_CORE_PLATFORM_TAGS.get(platform_tuple)
    if expected_platforms is None:
        return False

    expected = set(expected_platforms.split("."))
    actual = set(platform_tag_reader())
    return not expected.isdisjoint(actual)


def run_native_indexer(
    args: argparse.Namespace,
    journal: str,
    *,
    handshake_checker: HandshakeChecker = core_handshake.check_solstone_core_handshake,
    helper_locator: HelperLocator = core_handshake.helper_path_for_executable,
    native_runner: NativeRunner = subprocess.run,
    platform_reader: PlatformReader = probe.current_solstone_core_platform,
    platform_tag_reader: PlatformTagReader = _packaging_platform_tags,
) -> int:
    """Run the native indexer command and return its process-style status code."""
    if not runtime_has_solstone_core_wheel_coverage(
        platform_reader=platform_reader,
        platform_tag_reader=platform_tag_reader,
    ):
        _emit_error(UNSUPPORTED_HOST_MESSAGE)
        return core_handshake.EX_CONFIG

    handshake = handshake_checker()
    if handshake.status == "skip":
        _emit_error(HANDSHAKE_SKIP_MESSAGE)
        return core_handshake.EX_CONFIG
    if handshake.status == "fail":
        _emit_error(
            HANDSHAKE_FAIL_MESSAGE.format(
                message=handshake.message or "unknown reason",
            )
        )
        return core_handshake.EX_CONFIG

    operation_flags = _build_operation_flags(args, journal)
    if not operation_flags:
        raise RuntimeError(EMPTY_TAIL_MESSAGE)

    helper_path = helper_locator()
    argv = [str(helper_path), "indexer", "--journal", journal, *operation_flags]
    try:
        completed = native_runner(argv, check=False)
    except OSError as exc:
        _emit_error(
            NATIVE_LAUNCH_FAILED_MESSAGE.format(
                error=exc,
            )
        )
        return EXIT_TEMPFAIL

    return _map_native_returncode(completed.returncode, _operation_count(args))


def run_native_indexer_search(
    query: str,
    journal: str,
    *,
    limit: int,
    offset: int,
    day: str | None = None,
    day_from: str | None = None,
    day_to: str | None = None,
    facet: str | None = None,
    agent: str | None = None,
    stream: str | None = None,
    time_bucket: str | None = None,
    relax: bool = False,
    include_counts: bool = True,
    **di_kwargs: Any,
) -> dict[str, Any]:
    """Run a native search and return its JSON response envelope."""
    argv_tail = ["search", query, "--limit", str(limit), "--offset", str(offset)]
    for flag, value in (
        ("--day", day),
        ("--day-from", day_from),
        ("--day-to", day_to),
        ("--facet", facet),
        ("--agent", agent),
        ("--stream", stream),
        ("--time-bucket", time_bucket),
    ):
        if value is not None:
            argv_tail.extend([flag, value])
    if relax:
        argv_tail.append("--relax")
    if include_counts:
        argv_tail.append("--counts")
    response = _run_native_indexer_read(argv_tail, journal, **di_kwargs)
    if not isinstance(response, dict):
        raise NativeIndexerReadError(
            "native journal search returned a non-object JSON response"
        )
    return response


def run_native_indexer_agents(
    journal: str,
    **di_kwargs: Any,
) -> list[str]:
    """Return distinct agents from the native indexer."""
    response = _run_native_indexer_read(["agents"], journal, **di_kwargs)
    if not isinstance(response, list) or not all(
        isinstance(agent, str) for agent in response
    ):
        raise NativeIndexerReadError(
            "native journal agents returned an invalid JSON response"
        )
    return response


def run_native_indexer_coverage(
    journal: str,
    **di_kwargs: Any,
) -> dict[str, Any]:
    """Return native index coverage JSON."""
    response = _run_native_indexer_read(["coverage"], journal, **di_kwargs)
    if not isinstance(response, dict):
        raise NativeIndexerReadError(
            "native journal coverage returned a non-object JSON response"
        )
    return response


def _run_native_indexer_read(
    argv_tail: list[str],
    journal: str,
    *,
    handshake_checker: HandshakeChecker = core_handshake.check_solstone_core_handshake,
    helper_locator: HelperLocator = core_handshake.helper_path_for_executable,
    native_runner: NativeRunner = subprocess.run,
    platform_reader: PlatformReader = probe.current_solstone_core_platform,
    platform_tag_reader: PlatformTagReader = _packaging_platform_tags,
) -> dict[str, Any] | list[Any]:
    if not runtime_has_solstone_core_wheel_coverage(
        platform_reader=platform_reader,
        platform_tag_reader=platform_tag_reader,
    ):
        raise NativeIndexerReadError(READ_UNSUPPORTED_HOST_MESSAGE)

    handshake = handshake_checker()
    if handshake.status == "skip":
        raise NativeIndexerReadError(READ_HANDSHAKE_SKIP_MESSAGE)
    if handshake.status == "fail":
        raise NativeIndexerReadError(
            READ_HANDSHAKE_FAIL_MESSAGE.format(
                message=handshake.message or "unknown reason",
            )
        )

    argv = [
        str(helper_locator()),
        "indexer",
        *argv_tail,
        "--journal",
        journal,
        "--json",
    ]
    try:
        completed = native_runner(argv, capture_output=True, text=True, check=False)
    except OSError as exc:
        raise NativeIndexerReadError(
            READ_NATIVE_LAUNCH_FAILED_MESSAGE.format(error=exc)
        ) from exc

    if completed.returncode == 0:
        try:
            return json.loads(completed.stdout)
        except json.JSONDecodeError as exc:
            raise NativeIndexerReadError(
                "native journal index read returned invalid JSON"
            ) from exc
    if completed.returncode in (EXIT_UNAVAILABLE, EXIT_TEMPFAIL):
        try:
            error = json.loads(completed.stdout)["error"]
            message = error["message"]
            reason = error["reason"]
        except (KeyError, TypeError, json.JSONDecodeError) as exc:
            raise NativeIndexerReadError(
                "native journal index read returned an invalid error response"
            ) from exc
        raise NativeIndexerReadError(message, reason=reason)
    if completed.returncode < 0:
        raise NativeIndexerReadError(
            READ_NATIVE_SIGNAL_MESSAGE.format(
                signal_number=abs(completed.returncode),
                returncode=completed.returncode,
            )
        )
    if completed.returncode == EXIT_USAGE:
        raise NativeIndexerReadError(READ_NATIVE_USAGE_MESSAGE)
    raise NativeIndexerReadError(
        READ_NATIVE_OTHER_NONZERO_MESSAGE.format(returncode=completed.returncode)
    )


def _operation_count(args: argparse.Namespace) -> int:
    count = 0
    if args.reset:
        count += 1
    if args.rebuild_edges:
        count += 1
    if args.rescan_file or args.rescan_full or args.rescan:
        count += 1
    return count


def _build_operation_flags(args: argparse.Namespace, journal: str) -> list[str]:
    flags: list[str] = []
    if args.reset:
        flags.append("--reset")
    if args.rebuild_edges:
        flags.append("--rebuild-edges")

    if args.rescan_file:
        flags.extend(
            ["--rescan-file", _normalize_rescan_file(journal, args.rescan_file)]
        )
    elif args.rescan_full:
        flags.append("--rescan-full")
    elif args.rescan:
        flags.append("--rescan")

    return flags


def _normalize_rescan_file(journal: str, file_path: str) -> str:
    journal_path = Path(journal).resolve()
    path = Path(file_path)
    if path.is_absolute():
        return str(path.resolve())
    return str(resolve_journal_path(journal_path, file_path).resolve())


def _map_native_returncode(
    returncode: int,
    operation_count: int,
) -> int:
    if returncode == 0:
        return 0
    if returncode < 0:
        _emit_error(
            NATIVE_SIGNAL_MESSAGE.format(
                signal_number=abs(returncode),
                returncode=returncode,
            )
        )
        _emit_composed_warning(operation_count)
        return EXIT_TEMPFAIL
    if returncode == EXIT_USAGE:
        _emit_error(NATIVE_USAGE_MESSAGE)
        return EXIT_USAGE
    if returncode == EXIT_UNAVAILABLE:
        _emit_error(NATIVE_UNAVAILABLE_MESSAGE)
        _emit_composed_warning(operation_count)
        return EXIT_UNAVAILABLE
    if returncode == EXIT_TEMPFAIL:
        _emit_error(NATIVE_TEMPFAIL_MESSAGE)
        _emit_composed_warning(operation_count)
        return EXIT_TEMPFAIL

    _emit_error(
        NATIVE_OTHER_NONZERO_MESSAGE.format(
            returncode=returncode,
        )
    )
    _emit_composed_warning(operation_count)
    return returncode


def _emit_composed_warning(operation_count: int) -> None:
    if operation_count > 1:
        _emit_warning(COMPOSED_COMMAND_WARNING)


def _emit_error(message: str) -> None:
    print(message, file=sys.stderr)
    logger.error(message)


def _emit_warning(message: str) -> None:
    print(message, file=sys.stderr)
    logger.warning(message)
