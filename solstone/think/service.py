# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Cross-platform background service management for solstone.

Usage:
    journal service install [--port PORT]  Install solstone as a background service
    journal service uninstall              Remove the background service
    journal service start                  Start the background service
    journal service stop                   Stop the background service
    journal service restart [--if-installed]  Restart the background service
    journal service status                 Show service installation and runtime status
    journal service logs                   View service logs
    journal service logs -f                Follow service logs

    journal up                             Install (if needed), start, and show status
    journal down                           Stop the background service

Default convey port for installed services is 5015.
"""

from __future__ import annotations

import os
import plistlib
import subprocess
import sys
from pathlib import Path
from xml.parsers.expat import ExpatError

from solstone.think.install_guard import validate_journal_path_for_wrapper
from solstone.think.readiness import clear_ready, wait_ready
from solstone.think.utils import get_journal, get_journal_info

SERVICE_LABEL = "org.solpbc.solstone"
SYSTEMD_UNIT = "solstone"
DEFAULT_SERVICE_PORT = 5015
READY_TIMEOUT_SECONDS = 60.0


def _ready_timeout_message() -> str:
    return (
        f"Service did not become ready within {READY_TIMEOUT_SECONDS:g}s — "
        "run 'journal service status' or 'sol doctor' for diagnostics"
    )


def _platform() -> str:
    """Return 'darwin', 'linux', or raise on unsupported."""
    if sys.platform == "darwin":
        return "darwin"
    elif sys.platform.startswith("linux"):
        return "linux"
    else:
        print(f"Error: unsupported platform '{sys.platform}'", file=sys.stderr)
        sys.exit(1)


def _plist_path() -> Path:
    return Path.home() / "Library" / "LaunchAgents" / f"{SERVICE_LABEL}.plist"


def _unit_path() -> Path:
    return Path.home() / ".config" / "systemd" / "user" / f"{SYSTEMD_UNIT}.service"


def _managed_wrapper(binary: str) -> str:
    """Return absolute path to a managed user wrapper."""
    return str(Path.home() / ".local" / "bin" / binary)


def service_is_installed() -> bool:
    """Return whether the user service definition is installed."""
    return _plist_path().exists() if _platform() == "darwin" else _unit_path().exists()


def service_is_running() -> bool:
    """Return whether the background service is currently running."""
    if not service_is_installed():
        return False
    if _platform() == "darwin":
        result = subprocess.run(
            ["launchctl", "print", f"gui/{os.getuid()}/{SERVICE_LABEL}"],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            return False
        return "\n\tstate = running\n" in result.stdout
    result = subprocess.run(
        ["systemctl", "--user", "is-active", SYSTEMD_UNIT],
        capture_output=True,
        text=True,
    )
    return result.stdout.strip() == "active"


def _collect_env() -> dict[str, str]:
    """Collect environment variables for the service file.

    Captures HOME and PATH (with venv bin prepended). Real PATH is read
    from os.environ so installed services inherit the shell's tool
    visibility. Falls back to /usr/local/bin:/usr/bin:/bin if PATH is unset.
    Sets PYTHONUNBUFFERED so service logs are flushed promptly.
    API keys are NOT written into service files — the supervisor reads them
    from journal.json at process startup via setup_cli().

    Never propagate SOLSTONE_JOURNAL into service files. Installed services
    invoke ~/.local/bin/journal, which is a managed wrapper that sets
    SOLSTONE_JOURNAL itself. The service's job is to start the wrapper, not
    to configure the journal path.
    """
    venv_bin = str(Path(sys.executable).parent)
    base_path = os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin")
    path = ":".join(dict.fromkeys([venv_bin] + base_path.split(":")))

    return {
        "HOME": str(Path.home()),
        "PATH": path,
        "PYTHONUNBUFFERED": "1",
    }


def _generate_plist(
    env: dict[str, str],
    *,
    port: int = DEFAULT_SERVICE_PORT,
    journal_path: str,
) -> bytes:
    """Generate a launchd plist for the solstone supervisor."""
    validate_journal_path_for_wrapper(journal_path)
    journal = _managed_wrapper("journal")
    service_log = str(Path(journal_path) / "health" / "service.log")

    plist = {
        "Label": SERVICE_LABEL,
        "ProgramArguments": [journal, "supervisor", str(port)],
        "EnvironmentVariables": env,
        "StandardOutPath": service_log,
        "StandardErrorPath": service_log,
        "RunAtLoad": True,
        "KeepAlive": {"SuccessfulExit": False},
    }
    return plistlib.dumps(plist)


def remove_stale_plists() -> tuple[int, int]:
    """Remove stale launchd plists from prior installs."""
    if sys.platform != "darwin":
        return (0, 0)

    scan_dir = _plist_path().parent
    if not scan_dir.is_dir():
        return (0, 0)

    current_wrappers = {_managed_wrapper("sol"), _managed_wrapper("journal")}
    uid = os.getuid()
    removed = 0
    failed = 0
    not_loaded_markers = (
        "could not find",
        "service not found",
        "no such process",
        "not currently loaded",
    )

    for path in sorted(scan_dir.glob("*.plist")):
        try:
            with path.open("rb") as handle:
                data = plistlib.load(handle)
        except (plistlib.InvalidFileException, ValueError, ExpatError, OSError) as exc:
            print(f"skipping {path}: {type(exc).__name__}: {exc}", file=sys.stderr)
            continue

        label = data.get("Label")
        if not isinstance(label, str):
            continue
        if label != SERVICE_LABEL and not label.startswith(f"{SERVICE_LABEL}."):
            continue

        program_arguments = data.get("ProgramArguments")
        program = data.get("Program")
        if (
            isinstance(program_arguments, list)
            and program_arguments
            and program_arguments[0]
        ):
            extracted = str(program_arguments[0])
        elif program:
            extracted = str(program)
        else:
            print(f"skipping {path}: no Program or ProgramArguments", file=sys.stderr)
            continue

        if not extracted.endswith(("/sol", "/journal")):
            continue

        if extracted in current_wrappers:
            continue

        result = subprocess.run(
            ["launchctl", "bootout", f"gui/{uid}", str(path)],
            capture_output=True,
            text=True,
        )
        stderr = result.stderr or ""
        if result.returncode != 0 and not any(
            marker in stderr.lower() for marker in not_loaded_markers
        ):
            print(
                f"launchctl bootout {path}: exit={result.returncode} stderr={stderr!r}",
                file=sys.stderr,
            )

        try:
            path.unlink()
        except (OSError, PermissionError) as exc:
            print(f"failed to remove {path}: {exc}", file=sys.stderr)
            failed += 1
            continue

        print(
            f"Removed stale launchd plist {path} "
            f"(referenced {extracted}, current wrappers are {sorted(current_wrappers)})"
        )
        removed += 1

    return (removed, failed)


def _generate_systemd_unit(
    env: dict[str, str],
    *,
    port: int = DEFAULT_SERVICE_PORT,
    journal_path: str,
) -> str:
    """Generate a systemd user unit for the solstone supervisor."""
    validate_journal_path_for_wrapper(journal_path)
    journal = _managed_wrapper("journal")
    env_lines = "\n".join(f"Environment={k}={v}" for k, v in sorted(env.items()))
    service_log = str(Path(journal_path) / "health" / "service.log")

    return (
        f"[Unit]\n"
        f"Description=Solstone Supervisor\n"
        f"After=default.target\n"
        f"StartLimitIntervalSec=120\n"
        f"StartLimitBurst=10\n"
        f"\n"
        f"[Service]\n"
        f"Type=notify\n"
        f"ExecStart={journal} supervisor {port}\n"
        f"Restart=on-failure\n"
        f"RestartSec=5\n"
        f"KillMode=control-group\n"
        f"TimeoutStopSec=30\n"
        f"StandardOutput=append:{service_log}\n"
        f"StandardError=inherit\n"
        f"{env_lines}\n"
        f"\n"
        f"[Install]\n"
        f"WantedBy=default.target\n"
    )


def _check_linger() -> None:
    """Warn if systemd linger is not enabled for the current user."""
    try:
        result = subprocess.run(
            ["loginctl", "show-user", os.environ.get("USER", ""), "--property=Linger"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if result.returncode == 0 and "Linger=no" in result.stdout:
            print(
                "Warning: systemd linger is not enabled. "
                "The service will stop when you log out.\n"
                "Enable it with: sudo loginctl enable-linger $USER"
            )
    except (subprocess.TimeoutExpired, FileNotFoundError):
        pass


def _install(port: int = DEFAULT_SERVICE_PORT) -> int:
    platform = _platform()
    env = _collect_env()

    journal_path, _source = get_journal_info()
    Path(journal_path, "health").mkdir(parents=True, exist_ok=True)
    clear_ready()

    if platform == "darwin":
        remove_stale_plists()
        plist_data = _generate_plist(env, port=port, journal_path=journal_path)
        path = _plist_path()
        path.parent.mkdir(parents=True, exist_ok=True)

        uid = os.getuid()
        subprocess.run(
            ["launchctl", "bootout", f"gui/{uid}", str(path)],
            capture_output=True,
        )

        path.write_bytes(plist_data)
        print(f"Wrote {path}")

        result = subprocess.run(
            ["launchctl", "bootstrap", f"gui/{uid}", str(path)],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"Error loading service: {result.stderr.strip()}", file=sys.stderr)
            return 1
        print("Service loaded into launchd")

    else:
        unit_content = _generate_systemd_unit(env, port=port, journal_path=journal_path)
        path = _unit_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(unit_content)
        print(f"Wrote {path}")

        print("Reloading systemd user units...")
        subprocess.run(["systemctl", "--user", "daemon-reload"], check=True)
        print("Enabling solstone.service...")
        subprocess.run(["systemctl", "--user", "enable", SYSTEMD_UNIT], check=True)
        print("Service enabled")

        _check_linger()

    return 0


def _uninstall() -> int:
    platform = _platform()

    if platform == "darwin":
        path = _plist_path()
        uid = os.getuid()
        subprocess.run(
            ["launchctl", "bootout", f"gui/{uid}", str(path)],
            capture_output=True,
        )
        if path.exists():
            path.unlink()
            print(f"Removed {path}")
        else:
            print("Service was not installed")

    else:
        path = _unit_path()
        subprocess.run(
            ["systemctl", "--user", "stop", SYSTEMD_UNIT],
            capture_output=True,
        )
        subprocess.run(
            ["systemctl", "--user", "disable", SYSTEMD_UNIT],
            capture_output=True,
        )
        if path.exists():
            path.unlink()
            subprocess.run(
                ["systemctl", "--user", "daemon-reload"],
                capture_output=True,
            )
            print(f"Removed {path}")
        else:
            print("Service was not installed")

    return 0


def _start() -> int:
    platform = _platform()
    if platform == "darwin":
        uid = os.getuid()
        path = _plist_path()
        if not path.exists():
            print(
                "Error: service not installed. Run 'journal service install' first.",
                file=sys.stderr,
            )
            return 1
        result = subprocess.run(
            ["launchctl", "kickstart", f"gui/{uid}/{SERVICE_LABEL}"],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"Error starting service: {result.stderr.strip()}", file=sys.stderr)
            return 1
    else:
        if not _unit_path().exists():
            print(
                "Error: service not installed. Run 'journal service install' first.",
                file=sys.stderr,
            )
            return 1
        result = subprocess.run(
            ["systemctl", "--user", "start", SYSTEMD_UNIT],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"Error starting service: {result.stderr.strip()}", file=sys.stderr)
            return 1

    print("Service started")
    return 0


def _stop() -> int:
    platform = _platform()
    if platform == "darwin":
        uid = os.getuid()
        result = subprocess.run(
            ["launchctl", "kill", "SIGTERM", f"gui/{uid}/{SERVICE_LABEL}"],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"Error stopping service: {result.stderr.strip()}", file=sys.stderr)
            return 1
    else:
        result = subprocess.run(
            ["systemctl", "--user", "stop", SYSTEMD_UNIT],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"Error stopping service: {result.stderr.strip()}", file=sys.stderr)
            return 1

    print("Service stopped")
    return 0


def _restart(if_installed: bool = False) -> int:
    platform = _platform()
    if not service_is_installed():
        if if_installed:
            return 0
        print(
            "Error: service not installed. Run 'journal service install' first.",
            file=sys.stderr,
        )
        return 1

    print(
        "Stopping old supervisor (waits for in-flight work to finish — may take a moment)..."
    )

    if platform == "darwin":
        uid = os.getuid()
        subprocess.run(
            ["launchctl", "kill", "SIGTERM", f"gui/{uid}/{SERVICE_LABEL}"],
            capture_output=True,
        )
        clear_ready()
        result = subprocess.run(
            ["launchctl", "kickstart", f"gui/{uid}/{SERVICE_LABEL}"],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"Error restarting service: {result.stderr.strip()}", file=sys.stderr)
            return 1
    else:
        clear_ready()
        result = subprocess.run(
            ["systemctl", "--user", "restart", SYSTEMD_UNIT],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"Error restarting service: {result.stderr.strip()}", file=sys.stderr)
            return 1

    if wait_ready(timeout=READY_TIMEOUT_SECONDS) is None:
        print(_ready_timeout_message(), file=sys.stderr)
        return 1

    print("Service restarted.")
    return 0


def _status() -> int:
    platform = _platform()

    if not service_is_installed():
        print("Service: not installed")
        print(
            "Run 'journal service install' to install, or 'journal up' to install and start."
        )
        return 1

    print("Service: installed")

    if service_is_running():
        if platform == "darwin":
            print("State: running (launchd)")
        else:
            print("State: running (systemd)")
    elif platform == "darwin":
        print("State: stopped")
        return 0
    else:
        result = subprocess.run(
            ["systemctl", "--user", "is-active", SYSTEMD_UNIT],
            capture_output=True,
            text=True,
        )
        state = result.stdout.strip()
        print(f"State: {state}")
        return 0

    print()
    from solstone.think.health_cli import health_check

    return health_check()


def _logs(follow: bool = False) -> int:
    _platform()
    journal_path = Path(get_journal())
    service_log = journal_path / "health" / "service.log"

    if follow:
        if not service_log.exists():
            print("No service log file found", file=sys.stderr)
            return 1
        result = subprocess.run(["/usr/bin/tail", "-f", str(service_log)])
        return result.returncode
    else:
        if service_log.exists():
            print(f"=== {service_log.name} ===")
            print(service_log.read_text(errors="replace")[-10000:])
        else:
            print(f"=== {service_log.name} === (not found)")
        return 0


def _up(port: int = DEFAULT_SERVICE_PORT) -> int:
    """Install if needed, start if not running, show status."""
    if not service_is_installed():
        print("Installing service...")
        rc = _install(port=port)
        if rc != 0:
            return rc

    if not service_is_running():
        print("Starting service...")
        clear_ready()
        rc = _start()
        if rc != 0:
            return rc

    if wait_ready(timeout=READY_TIMEOUT_SECONDS) is None:
        print(_ready_timeout_message(), file=sys.stderr)
        return 1

    # wait_ready() succeeding is the authoritative readiness signal per the
    # readiness primitive contract. _status() is invoked for human-readable
    # output, but its return code (which folds in health_check()'s 10s callosum
    # status probe) is NOT the gate. The callosum bus warms up over ~30-90s
    # post-readiness while convey/cortex/link come online; allowing _status()
    # to fail _up() here re-introduces the same premature-failure that the
    # readiness primitive was meant to retire.
    _status()
    return 0


def _down() -> int:
    """Stop the service."""
    return _stop()


_SUBCOMMANDS = {
    "uninstall": _uninstall,
    "start": _start,
    "stop": _stop,
    "status": _status,
    "down": lambda **_kw: _down(),
}


def _print_usage() -> None:
    print("Usage: journal service <install|uninstall|start|stop|restart|status|logs>")
    print("       journal service install [--port PORT]  (default: 5015)")
    print(
        "       journal service restart [--if-installed]  "
        "(restart; --if-installed noops if not installed)"
    )
    print("       journal up [--port PORT]               (install + start + status)")
    print("       journal down                           (stop)")


def _parse_port(args: list[str]) -> int:
    """Extract --port PORT from args, return DEFAULT_SERVICE_PORT if absent."""
    for i, arg in enumerate(args):
        if arg == "--port" and i + 1 < len(args):
            try:
                return int(args[i + 1])
            except ValueError:
                print(f"Error: invalid port '{args[i + 1]}'", file=sys.stderr)
                sys.exit(1)
        if arg.startswith("--port="):
            try:
                return int(arg.split("=", 1)[1])
            except ValueError:
                print(f"Error: invalid port '{arg}'", file=sys.stderr)
                sys.exit(1)
    return DEFAULT_SERVICE_PORT


def main() -> None:
    """Entry point for ``journal service``."""
    args = sys.argv[1:]

    if args and args[0] == "logs":
        follow = "-f" in args[1:] or "--follow" in args[1:]
        sys.exit(_logs(follow=follow))

    if "--help" in args or "-h" in args:
        _print_usage()
        sys.exit(0)

    if not args:
        _print_usage()
        sys.exit(1)

    subcmd = args[0]
    rest = args[1:]

    if subcmd == "install":
        sys.exit(_install(port=_parse_port(rest)))
    elif subcmd == "up":
        sys.exit(_up(port=_parse_port(rest)))
    elif subcmd == "restart":
        if_installed = "--if-installed" in rest
        sys.exit(_restart(if_installed=if_installed))
    elif subcmd in _SUBCOMMANDS:
        sys.exit(_SUBCOMMANDS[subcmd]())
    else:
        print(f"Unknown subcommand: {subcmd}", file=sys.stderr)
        print("Available: install, uninstall, start, stop, restart, status, logs")
        sys.exit(1)
