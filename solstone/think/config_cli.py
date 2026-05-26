# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""journal config — show and rewrite the embedded journal path in the wrapper."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from dataclasses import dataclass
from enum import Enum
from pathlib import Path

from solstone.think.install_guard import (
    alias_path,
    alias_paths,
    install_wrappers,
    parse_wrapper,
    validate_journal_path_for_wrapper,
)
from solstone.think.service import service_is_installed, service_is_running
from solstone.think.utils import (
    SolstoneNotConfigured,
    get_journal_info,
    get_project_root,
    is_source_checkout,
    journal_is_active,
)

MERGE_INSTRUCTIONS = "\n".join(
    [
        "sol config: --merge is not handled here.",
        "use 'sol call journal merge <source> --dry-run' to preview the merge.",
        "use 'sol call journal merge <source>' to perform the merge.",
    ]
)


class RequestedAction(Enum):
    MOVE = "move"
    SWITCH = "switch"
    MERGE = "merge"
    FORCE = "force"


class Action(Enum):
    PROCEED = "proceed"
    MOVE = "move"
    SWITCH = "switch"
    MERGE = "merge"
    NOOP = "noop"
    REFUSE = "refuse"


@dataclass(frozen=True)
class JournalChange:
    current_path: Path
    target_path: Path
    paths_equal: bool
    current_active: bool
    target_active: bool
    current_exists: bool
    target_exists: bool
    target_parent_exists: bool
    current_device: int | None
    target_parent_device: int | None
    same_filesystem: bool | None
    service_installed: bool
    service_running: bool
    action: RequestedAction | None
    yes: bool
    dry_run: bool
    sol_bin: str
    service_bin: str
    alias: Path


@dataclass(frozen=True)
class Decision:
    action: Action
    exit_code: int
    message: str | None = None
    plan_only: bool = False


def _read_wrapper_status() -> tuple[str, str | None]:
    alias = alias_path()
    if not alias.exists() and not alias.is_symlink():
        return "absent", None
    if alias.is_symlink():
        return "legacy-symlink", None

    try:
        content = alias.read_text(encoding="utf-8")
    except OSError:
        return "foreign", None

    parsed = parse_wrapper(content)
    if parsed is None:
        return "foreign", None
    return "managed", parsed["journal"]


def _wrapper_refusal(alias: Path) -> str:
    return (
        "sol config: refused: "
        f"{alias} is not a managed wrapper (run 'journal setup' from the solstone "
        "source checkout to install the wrapper first)"
    )


def _state_label(active: bool) -> str:
    return "active" if active else "not active"


def _valid_flags(change: JournalChange) -> str:
    if change.current_active and not change.target_active:
        return "--move, --switch"
    return "--switch, --merge, --force"


def _refusal_message(change: JournalChange) -> str:
    return (
        "sol config: refused: "
        f"current is {_state_label(change.current_active)} and target is "
        f"{_state_label(change.target_active)}; valid flags: {_valid_flags(change)}"
    )


def _move_target_exists_message(change: JournalChange) -> str:
    return f"sol config: refused: move target already exists: {change.target_path}"


def _move_missing_current_message(change: JournalChange) -> str:
    return f"sol config: refused: move source does not exist: {change.current_path}"


def _move_missing_parent_message(change: JournalChange) -> str:
    return f"sol config: refused: move target parent does not exist: {change.target_path.parent}"


def _move_cross_filesystem_message(change: JournalChange) -> str:
    return (
        "sol config: refused: cannot move across filesystems "
        f"(current device={change.current_device}, target parent device={change.target_parent_device}); "
        "use 'sol call journal merge <source>' instead"
    )


def _move_requires_inactive_target_message(change: JournalChange) -> str:
    return (
        "sol config: refused: "
        f"--move requires a not active target; current is {_state_label(change.current_active)} "
        f"and target is {_state_label(change.target_active)}; valid flags: --switch, --merge, --force"
    )


def _plan_closer(change: JournalChange) -> str:
    if change.dry_run:
        return "dry-run: yes; nothing will be changed"
    return "re-run with --yes to proceed"


def _service_summary(change: JournalChange, decision: Decision) -> str:
    if decision.action is Action.MOVE:
        if not change.service_installed:
            return "service: not installed; will move and rewrite wrapper"
        if not change.service_running:
            return "service: installed but not running; will move and rewrite wrapper"
        return (
            "service: installed and running; will stop, move, rewrite wrapper, restart"
        )

    if not change.service_installed:
        return "service: not installed; will rewrite wrapper"
    if not change.service_running:
        return "service: installed but not running; will rewrite wrapper"
    return "service: installed and running; will rewrite wrapper, restart"


def render_plan(change: JournalChange, decision: Decision) -> str:
    lines = [
        "journal config journal - plan summary",
        "",
        f"current: {change.current_path} ({_state_label(change.current_active)})",
        f"target:  {change.target_path} ({_state_label(change.target_active)})",
        f"action:  {decision.action.value}",
        _service_summary(change, decision),
    ]

    if decision.action is Action.MOVE:
        filesystem = "same device" if change.same_filesystem else "different devices"
        lines.append(f"filesystem: {filesystem}")

    if decision.action is Action.SWITCH:
        lines.extend(
            [
                "",
                "current journal is left intact. "
                f"to re-adopt it later: journal config journal {change.current_path} --switch --yes",
            ]
        )

    lines.extend(["", _plan_closer(change)])
    return "\n".join(lines)


def _rewrite_wrapper(change: JournalChange) -> str | None:
    target_str = str(change.target_path)
    sol_bins: dict[str, str] = {}
    for binary, alias in alias_paths().items():
        if not alias.exists() and not alias.is_symlink():
            if binary == "journal":
                sol_bins[binary] = str(Path(change.sol_bin).with_name("journal"))
                continue
            print(_wrapper_refusal(alias), file=sys.stderr)
            return None
        if alias.is_symlink():
            print(_wrapper_refusal(alias), file=sys.stderr)
            return None
        try:
            current_content = alias.read_text(encoding="utf-8")
        except OSError as exc:
            print(
                f"sol config: refused: cannot read {alias}: {exc}",
                file=sys.stderr,
            )
            return None
        current = parse_wrapper(current_content)
        if current is None:
            print(_wrapper_refusal(alias), file=sys.stderr)
            return None
        sol_bins[binary] = current["sol_bin"]

    install_wrappers(target_str, sol_bins)
    return sol_bins["journal"]


def _service_command(sol_bin: str, subcommand: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sol_bin, "service", subcommand],
        check=False,
        capture_output=True,
        text=True,
    )


def _maybe_restart_current_service(change: JournalChange) -> None:
    if not change.service_running:
        return
    try:
        _service_command(change.service_bin, "start")
    except FileNotFoundError as exc:
        print(
            f"sol config: rollback warning: could not restart service ({exc})",
            file=sys.stderr,
        )


def _run_switch(change: JournalChange) -> int:
    try:
        change.target_path.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        print(
            f"sol config: refused: cannot create {change.target_path}: {exc}",
            file=sys.stderr,
        )
        return 1

    try:
        restart_sol = _rewrite_wrapper(change)
    except OSError as exc:
        print(
            f"sol config: refused: cannot rewrite {change.alias}: {exc}",
            file=sys.stderr,
        )
        return 1

    if restart_sol is None:
        return 1

    if not change.service_installed:
        print("service not installed; wrapper updated.")
        return 0

    if not change.service_running:
        print("service installed but not running; wrapper updated.")
        return 0

    try:
        result = subprocess.run(
            [restart_sol, "service", "restart", "--if-installed"],
            check=False,
        )
    except FileNotFoundError as exc:
        print(
            f"sol config: wrapper rewritten to {change.target_path} but journal service restart could not run ({exc}); restart manually",
            file=sys.stderr,
        )
        return 2

    if result.returncode != 0:
        print(
            "sol config: wrapper rewritten to "
            f"{change.target_path} but 'journal service restart --if-installed' exited "
            f"{result.returncode}; investigate and restart manually",
            file=sys.stderr,
        )
        return 2

    print("wrapper updated; service restarted.")
    return 0


def _run_move(change: JournalChange) -> int:
    current = change.current_path
    target = change.target_path

    if not change.target_parent_exists:
        print(_move_missing_parent_message(change), file=sys.stderr)
        return 1
    if not current.exists():
        print(_move_missing_current_message(change), file=sys.stderr)
        return 1
    if target.exists() or target.is_symlink():
        print(_move_target_exists_message(change), file=sys.stderr)
        return 1
    if change.same_filesystem is False:
        print(_move_cross_filesystem_message(change), file=sys.stderr)
        return 1

    if change.service_running:
        try:
            stop_result = _service_command(change.service_bin, "stop")
        except FileNotFoundError as exc:
            print(
                f"sol config: could not stop service before move ({exc})",
                file=sys.stderr,
            )
            return 2
        if stop_result.returncode != 0:
            print(
                "sol config: could not stop service before move",
                file=sys.stderr,
            )
            return 2

    try:
        os.rename(current, target)
    except OSError as exc:
        _maybe_restart_current_service(change)
        print(f"sol config: move failed: {exc}", file=sys.stderr)
        return 1

    try:
        restart_sol = _rewrite_wrapper(change)
    except OSError as exc:
        rollback_ok = True
        try:
            if target.exists():
                os.rename(target, current)
        except OSError as rollback_exc:
            rollback_ok = False
            print(
                f"sol config: rollback failed after wrapper write error: {rollback_exc}",
                file=sys.stderr,
            )
        _maybe_restart_current_service(change)
        message = f"sol config: move failed during wrapper update: {exc}"
        if rollback_ok:
            message += "; restored original journal"
        print(message, file=sys.stderr)
        return 2

    if restart_sol is None:
        try:
            os.rename(target, current)
        except OSError as rollback_exc:
            print(
                f"sol config: rollback failed after wrapper validation error: {rollback_exc}",
                file=sys.stderr,
            )
        _maybe_restart_current_service(change)
        return 1

    if not change.service_installed:
        print("service not installed; journal moved; wrapper updated.")
        return 0

    if not change.service_running:
        print("service installed but not running; journal moved; wrapper updated.")
        return 0

    try:
        start_result = _service_command(restart_sol, "start")
    except FileNotFoundError:
        print(
            f"wrapper updated to {target} but service start failed; restart manually",
            file=sys.stderr,
        )
        return 2

    if start_result.returncode != 0:
        print(
            f"wrapper updated to {target} but service start failed; restart manually",
            file=sys.stderr,
        )
        return 2

    print("journal moved; wrapper updated; service restarted.")
    return 0


def _run_noop(change: JournalChange, _decision: Decision) -> int:
    print(f"sol config: journal already set to {change.target_path}")
    return 0


def _refuse(decision: Decision) -> int:
    if decision.message:
        print(decision.message, file=sys.stderr)
    return decision.exit_code


def decide(change: JournalChange) -> Decision:
    if change.action is RequestedAction.MERGE:
        return Decision(Action.MERGE, 1, MERGE_INSTRUCTIONS)

    if change.paths_equal:
        return Decision(Action.NOOP, 0)

    if change.action is None:
        if not change.current_active and not change.target_active:
            return Decision(Action.PROCEED, 0)
        return Decision(Action.REFUSE, 1, _refusal_message(change))

    if change.action is RequestedAction.FORCE:
        return Decision(Action.SWITCH, 0)

    if change.action is RequestedAction.MOVE:
        if not change.target_parent_exists:
            return Decision(Action.REFUSE, 1, _move_missing_parent_message(change))
        if not change.current_exists:
            return Decision(Action.REFUSE, 1, _move_missing_current_message(change))
        if change.target_exists:
            return Decision(Action.REFUSE, 1, _move_target_exists_message(change))
        if change.target_active:
            return Decision(
                Action.REFUSE, 1, _move_requires_inactive_target_message(change)
            )
        if change.same_filesystem is False:
            return Decision(Action.REFUSE, 1, _move_cross_filesystem_message(change))
        if change.dry_run:
            return Decision(Action.MOVE, 0, plan_only=True)
        if not change.yes:
            return Decision(Action.MOVE, 1, plan_only=True)
        return Decision(Action.MOVE, 0)

    if change.action is RequestedAction.SWITCH:
        if change.dry_run:
            return Decision(Action.SWITCH, 0, plan_only=True)
        if not change.yes:
            return Decision(Action.SWITCH, 1, plan_only=True)
        return Decision(Action.SWITCH, 0)

    return Decision(Action.REFUSE, 1, _refusal_message(change))


def execute(change: JournalChange, decision: Decision) -> int:
    if change.action is RequestedAction.FORCE:
        print(
            "sol config: warning: --force bypasses confirmation and target activity checks",
            file=sys.stderr,
        )

    if decision.action is Action.MERGE:
        print(decision.message or MERGE_INSTRUCTIONS)
        return decision.exit_code
    if decision.action is Action.REFUSE:
        return _refuse(decision)
    if decision.action is Action.NOOP:
        return _run_noop(change, decision)
    if decision.plan_only:
        print(render_plan(change, decision))
        return decision.exit_code
    if decision.action in {Action.PROCEED, Action.SWITCH}:
        return _run_switch(change)
    if decision.action is Action.MOVE:
        return _run_move(change)
    return 1


def build_change(
    args: argparse.Namespace, *, alias_path: Path, sol_bin: str, current_path: Path
) -> JournalChange:
    target_path = Path(args.path).expanduser().resolve()
    current_path = current_path.expanduser().resolve()
    current_exists = current_path.exists()
    target_exists = target_path.exists() or target_path.is_symlink()
    target_parent_exists = target_path.parent.exists()
    current_device = None
    target_parent_device = None
    same_filesystem = None
    if current_exists:
        try:
            current_device = os.stat(current_path).st_dev
        except OSError:
            current_device = None
    if target_parent_exists:
        try:
            target_parent_device = os.stat(target_path.parent).st_dev
        except OSError:
            target_parent_device = None
    if current_device is not None and target_parent_device is not None:
        same_filesystem = current_device == target_parent_device

    installed = service_is_installed()
    running = service_is_running() if installed else False

    return JournalChange(
        current_path=current_path,
        target_path=target_path,
        paths_equal=current_path == target_path,
        current_active=journal_is_active(current_path),
        target_active=journal_is_active(target_path),
        current_exists=current_exists,
        target_exists=target_exists,
        target_parent_exists=target_parent_exists,
        current_device=current_device,
        target_parent_device=target_parent_device,
        same_filesystem=same_filesystem,
        service_installed=installed,
        service_running=running,
        action=args.action,
        yes=args.yes,
        dry_run=args.dry_run,
        sol_bin=sol_bin,
        service_bin=str(Path(sol_bin).with_name("journal")),
        alias=alias_path,
    )


def cmd_show() -> int:
    wrapper_status, embedded_journal = _read_wrapper_status()

    try:
        path, info_source = get_journal_info()
    except SolstoneNotConfigured as exc:
        print(f"sol config: {exc}", file=sys.stderr)
        return 1

    if info_source == "env":
        if (
            embedded_journal is not None
            and os.environ.get("SOLSTONE_JOURNAL") == embedded_journal
        ):
            user_source = "wrapper-embedded"
        else:
            user_source = "caller-override"
    elif info_source == "config":
        user_source = "user config (~/.config/solstone/config.toml)"
    elif info_source == "default":
        user_source = "built-in default (~/journal)"
    else:  # "source"
        user_source = "source-tree fallback"

    print(f"path: {path}")
    print(f"source: {user_source}")
    print(f"wrapper-status: {wrapper_status}")
    return 0


def cmd_journal(
    target_path: str,
    *,
    action: RequestedAction | None = None,
    yes: bool = False,
    dry_run: bool = False,
) -> int:
    target = Path(target_path).expanduser().resolve()
    target_str = str(target)

    try:
        validate_journal_path_for_wrapper(target_str)
    except ValueError as exc:
        print(f"sol config: refused: {exc}", file=sys.stderr)
        return 1

    project_root = Path(get_project_root())
    source_tree_journal = (project_root / "journal").resolve()
    if target == source_tree_journal and not is_source_checkout():
        print(
            "sol config: refused: "
            f"{target_str} is the source-tree fallback path but this is not a "
            "source checkout",
            file=sys.stderr,
        )
        return 1

    if action is RequestedAction.MOVE and not target.parent.exists():
        print(
            f"sol config: refused: move target parent does not exist: {target.parent}",
            file=sys.stderr,
        )
        return 1

    alias = alias_path()
    if not alias.exists() or alias.is_symlink():
        print(_wrapper_refusal(alias), file=sys.stderr)
        return 1

    try:
        content = alias.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"sol config: refused: cannot read {alias}: {exc}", file=sys.stderr)
        return 1

    parsed = parse_wrapper(content)
    if parsed is None:
        print(_wrapper_refusal(alias), file=sys.stderr)
        return 1

    args = argparse.Namespace(
        path=target_str,
        action=action,
        yes=yes,
        dry_run=dry_run,
    )
    change = build_change(
        args,
        alias_path=alias,
        sol_bin=parsed["sol_bin"],
        current_path=Path(parsed["journal"]),
    )
    decision = decide(change)
    return execute(change, decision)


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="cmd", required=True)
    subparsers.add_parser("show", help="show the configured journal path and source")
    journal_parser = subparsers.add_parser(
        "journal",
        help="rewrite the wrapper's embedded journal path",
    )
    journal_parser.add_argument(
        "path", help="absolute path to the new journal directory"
    )
    action_group = journal_parser.add_mutually_exclusive_group()
    action_group.add_argument(
        "--move",
        dest="action",
        action="store_const",
        const=RequestedAction.MOVE,
    )
    action_group.add_argument(
        "--switch",
        dest="action",
        action="store_const",
        const=RequestedAction.SWITCH,
    )
    action_group.add_argument(
        "--merge",
        dest="action",
        action="store_const",
        const=RequestedAction.MERGE,
    )
    action_group.add_argument(
        "--force",
        dest="action",
        action="store_const",
        const=RequestedAction.FORCE,
    )
    confirm_group = journal_parser.add_mutually_exclusive_group()
    confirm_group.add_argument("--yes", action="store_true")
    confirm_group.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    if args.cmd == "show":
        return cmd_show()
    if args.cmd == "journal":
        return cmd_journal(
            args.path,
            action=args.action,
            yes=args.yes,
            dry_run=args.dry_run,
        )
    return 1


if __name__ == "__main__":
    sys.exit(main())
