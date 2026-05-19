# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Linux observer installer."""

from __future__ import annotations

import datetime as dt
import json
import shutil
from pathlib import Path

from solstone.apps.observer.utils import list_observers

from .common import (
    InstallError,
    create_or_reuse_registration,
    default_server_url,
    default_stream,
    emit_json,
    marker_path,
    observer_key_prefix_from_config,
    pipx_install,
    poll_status_until,
    print_summary,
    read_marker,
    run_probe,
    run_step,
    write_marker,
    xdg_install_dir,
)

PLATFORM = "linux"
INSTALL_NAME = "solstone-linux"
PACKAGE_NAME = "solstone-linux"
PACKAGE_VERSION = "0.1.0"
UNIT_NAME = "solstone-linux.service"
CONFIG_PATH = (
    Path.home() / ".local" / "share" / "solstone-linux" / "config" / "config.json"
)
OS_RELEASE_PATH = Path("/etc/os-release")
DEFAULT_CONFIG = {
    "server_url": "",
    "key": "",
    "stream": "",
    "segment_interval": 300,
    "sync_retry_delays": [5, 30, 120, 300],
    "sync_max_retries": 10,
    "cache_retention_days": 7,
}

DISTRO_PACKAGES = {
    "fedora": (
        [
            "python3-gobject",
            "gtk4",
            "gstreamer1-plugins-base",
            "gstreamer1-plugin-pipewire",
            "pipewire-gstreamer",
            "alsa-lib-devel",
            "pulseaudio-utils",
            "pipewire-pulseaudio",
            "xdg-desktop-portal",
            "pipx",
        ],
        "sudo dnf install python3-gobject gtk4 gstreamer1-plugins-base gstreamer1-plugin-pipewire pipewire-gstreamer alsa-lib-devel pulseaudio-utils pipewire-pulseaudio xdg-desktop-portal pipx",
        "rpm",
    ),
    "debian-ubuntu": (
        [
            "python3-gi",
            "gir1.2-gdk-4.0",
            "gir1.2-gtk-4.0",
            "gstreamer1.0-pipewire",
            "libasound2-dev",
            "pulseaudio-utils",
            "pipewire-pulse",
            "xdg-desktop-portal",
            "pipx",
        ],
        "sudo apt install python3-gi gir1.2-gdk-4.0 gir1.2-gtk-4.0 gstreamer1.0-pipewire libasound2-dev pulseaudio-utils pipewire-pulse xdg-desktop-portal pipx",
        "dpkg",
    ),
    "arch": (
        [
            "python-gobject",
            "gtk4",
            "gstreamer",
            "gst-plugin-pipewire",
            "libpulse",
            "alsa-lib",
            "xdg-desktop-portal",
            "pipx",
        ],
        "sudo pacman -S python-gobject gtk4 gstreamer gst-plugin-pipewire libpulse alsa-lib xdg-desktop-portal pipx",
        "pacman",
    ),
    "opensuse": (
        [
            "python3-gobject",
            "python3-gobject-Gdk",
            "typelib-1_0-Gtk-4_0",
            "gtk4-tools",
            "gstreamer-plugins-base",
            "gstreamer-plugin-pipewire",
            "pipewire-pulseaudio",
            "pulseaudio-utils",
            "alsa-devel",
            "xdg-desktop-portal",
            "python3-pipx",
        ],
        "sudo zypper install python3-gobject python3-gobject-Gdk typelib-1_0-Gtk-4_0 \\\n  gtk4-tools gstreamer-plugins-base gstreamer-plugin-pipewire \\\n  pipewire-pulseaudio pulseaudio-utils alsa-devel \\\n  xdg-desktop-portal python3-pipx",
        "rpm",
    ),
}


def detect_distro() -> str | None:
    """Detect the Linux package family."""
    values = _read_os_release()
    distro_id = values.get("id", "")
    id_like = values.get("id_like", "").split()
    for candidate in [distro_id]:
        mapped = _map_os_release_id(candidate)
        if mapped:
            return mapped
    for candidate in id_like:
        mapped = _map_os_release_like(candidate)
        if mapped:
            return mapped
    for binary, distro in (
        ("zypper", "opensuse"),
        ("dnf", "fedora"),
        ("dpkg", "debian-ubuntu"),
        ("pacman", "arch"),
        ("rpm", "fedora"),
    ):
        if shutil.which(binary):
            return distro
    return None


class LinuxDriver:
    """Install and manage the Linux observer service."""

    def run(self, args) -> int:
        distro = detect_distro()
        if distro is None:
            raise InstallError(
                "unsupported Linux distribution",
                hint="install the observer dependencies from solstone-linux/INSTALL.md",
            )
        if distro not in DISTRO_PACKAGES:
            raise InstallError("unsupported Linux distribution")

        server_url = default_server_url(args.server_url)
        name = default_stream("linux", args.name)
        requested_version = args.observer_version or PACKAGE_VERSION
        install_dir = xdg_install_dir(INSTALL_NAME)
        marker = read_marker(INSTALL_NAME)
        if marker and marker.get("name") != name and not args.force:
            raise InstallError(
                f"{INSTALL_NAME} is already installed for {marker.get('name')}",
                hint="rerun with --force to replace the existing install marker",
            )

        package_statuses = _check_packages(distro)
        if args.dry_run:
            if not args.json_output:
                _print_dry_run(
                    name,
                    server_url,
                    install_dir,
                    requested_version,
                    distro,
                    package_statuses,
                )
            if args.json_output:
                emit_json(
                    _result(name, server_url, install_dir, "planned", None, None, True)
                )
            return 0

        _raise_for_preflight(package_statuses, distro)
        active = _active_registration(name)
        service_active = _service_is_active()
        config_prefix = observer_key_prefix_from_config(CONFIG_PATH)

        if (
            marker
            and not args.force
            and marker.get("source") == f"pypi:{PACKAGE_NAME}"
            and marker.get("version") == requested_version
            and active
            and config_prefix == active.get("key", "")[:8]
        ):
            if service_active:
                result = _result(
                    name,
                    server_url,
                    install_dir,
                    "already_installed",
                    active.get("key", "")[:8],
                    requested_version,
                    False,
                )
                _output_result(result, args.json_output)
                return 0
            run_step(
                f"restart {UNIT_NAME}",
                ["systemctl", "--user", "restart", UNIT_NAME],
                json_output=args.json_output,
            )
            status = poll_status_until(name)
            result = _result(
                name,
                server_url,
                install_dir,
                status,
                active.get("key", "")[:8],
                requested_version,
                False,
            )
            _output_result(result, args.json_output)
            return 0

        registration = create_or_reuse_registration(name, force=args.force)
        _write_config(server_url, registration.key, name)
        try:
            pipx_install(
                PACKAGE_NAME,
                requested_version,
                system_site_packages=True,
                json_output=args.json_output,
                dry_run=False,
            )
        except InstallError as exc:
            raise InstallError(
                str(exc),
                hint=(
                    f"pipx install failed for {PACKAGE_NAME}=={requested_version}. "
                    "Check that pipx is installed and on PATH (`pipx --version`); "
                    f"try `pipx install --force {PACKAGE_NAME}=={requested_version}` "
                    "manually for more detail."
                ),
            ) from exc

        if shutil.which(PACKAGE_NAME) is None:
            raise InstallError(
                f"{PACKAGE_NAME} is not on PATH after pipx install",
                hint=(
                    f"pipx installed {PACKAGE_NAME} but the script is not on PATH. "
                    "Run `pipx ensurepath` and restart your shell, then retry "
                    "`sol observer install --platform linux`."
                ),
            )

        try:
            run_step(
                f"run {PACKAGE_NAME} install-service",
                [PACKAGE_NAME, "install-service"],
                json_output=args.json_output,
            )
        except InstallError as exc:
            raise InstallError(
                str(exc),
                hint=(
                    f"{PACKAGE_NAME} install-service failed after pipx placed the "
                    f"script on PATH. Run `{PACKAGE_NAME} doctor` (or the observer's "
                    "preflight) for detail."
                ),
            ) from exc
        run_step(
            f"restart {UNIT_NAME}",
            ["systemctl", "--user", "restart", UNIT_NAME],
            json_output=args.json_output,
        )
        status = poll_status_until(name)
        _write_install_marker(marker, name, requested_version)
        result = _result(
            name,
            server_url,
            install_dir,
            status,
            registration.prefix,
            requested_version,
            False,
        )
        _output_result(result, args.json_output)
        return 0


def _read_os_release() -> dict[str, str]:
    try:
        lines = OS_RELEASE_PATH.read_text(encoding="utf-8").splitlines()
    except OSError:
        return {}
    values: dict[str, str] = {}
    for line in lines:
        if "=" not in line or line.startswith("#"):
            continue
        key, value = line.split("=", 1)
        values[key.lower()] = value.strip().strip('"').strip("'").lower()
    return values


def _map_os_release_id(value: str) -> str | None:
    if value == "fedora":
        return "fedora"
    if value in {"debian", "ubuntu", "pop", "linuxmint"}:
        return "debian-ubuntu"
    if value in {"arch", "manjaro"}:
        return "arch"
    if value in {"opensuse", "opensuse-leap", "opensuse-tumbleweed", "sles", "suse"}:
        return "opensuse"
    return None


def _map_os_release_like(value: str) -> str | None:
    if value in {"fedora", "rhel", "centos"}:
        return "fedora"
    if value in {"debian", "ubuntu"}:
        return "debian-ubuntu"
    if value == "arch":
        return "arch"
    if value in {"suse", "opensuse"}:
        return "opensuse"
    return None


_PIPX_PACKAGE_NAMES = {"pipx", "python3-pipx"}


def _check_packages(distro: str) -> list[tuple[str, bool]]:
    packages, _install_command, query_method = DISTRO_PACKAGES[distro]
    pipx_on_path = run_probe(["sh", "-c", "command -v pipx"]).returncode == 0
    result = []
    for package in packages:
        if package in _PIPX_PACKAGE_NAMES and pipx_on_path:
            result.append((package, True))
            continue
        if query_method == "dpkg":
            cmd = ["dpkg", "-s", package]
        elif query_method == "pacman":
            cmd = ["pacman", "-Q", package]
        else:
            cmd = ["rpm", "-q", package]
        result.append((package, run_probe(cmd).returncode == 0))
    return result


def _raise_for_preflight(
    package_statuses: list[tuple[str, bool]],
    distro: str,
) -> None:
    missing_packages = [package for package, ok in package_statuses if not ok]
    if missing_packages:
        _packages, install_command, _query_method = DISTRO_PACKAGES[distro]
        raise InstallError(
            "missing required system packages: " + ", ".join(missing_packages),
            hint=install_command,
        )


def _write_config(server_url: str, key: str, name: str) -> None:
    config = dict(DEFAULT_CONFIG)
    config.update({"server_url": server_url, "key": key, "stream": name})
    try:
        CONFIG_PATH.parent.mkdir(parents=True, exist_ok=True)
        with CONFIG_PATH.open("w", encoding="utf-8") as handle:
            json.dump(config, handle, indent=2)
            handle.write("\n")
    except OSError as exc:
        raise InstallError(f"failed to write {CONFIG_PATH}", hint=str(exc)) from exc


def _write_install_marker(marker: dict | None, name: str, version: str | None) -> None:
    now = _now_utc()
    write_marker(
        INSTALL_NAME,
        {
            "name": name,
            "platform": PLATFORM,
            "source": f"pypi:{PACKAGE_NAME}",
            "installed_at": (marker.get("installed_at") if marker else None) or now,
            "last_run": now,
            "version": version,
        },
    )


def _active_registration(name: str) -> dict | None:
    for observer in list_observers():
        if observer.get("name") == name and not observer.get("revoked", False):
            return observer
    return None


def _service_is_active() -> bool:
    process = run_probe(["systemctl", "--user", "is-active", UNIT_NAME])
    return process.returncode == 0 and process.stdout.strip() == "active"


def _now_utc() -> str:
    return (
        dt.datetime.now(dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )


def _result(
    name: str,
    server_url: str,
    install_dir: Path,
    status: str,
    key_prefix: str | None,
    version: str | None,
    dry_run: bool,
) -> dict:
    return {
        "platform": PLATFORM,
        "name": name,
        "source_path": str(install_dir),
        "service_unit": UNIT_NAME,
        "key_prefix": key_prefix,
        "server_url": server_url,
        "config_path": str(CONFIG_PATH),
        "marker_path": str(marker_path(INSTALL_NAME)),
        "status": status,
        "version": version,
        "dry_run": dry_run,
    }


def _output_result(result: dict, json_output: bool) -> None:
    if json_output:
        emit_json(result)
    else:
        print_summary(result)


def _print_dry_run(
    name: str,
    server_url: str,
    install_dir: Path,
    requested_version: str,
    distro: str,
    package_statuses: list[tuple[str, bool]],
) -> None:
    print("Dry-run: would install solstone-linux observer")
    print()
    print("Platform: linux")
    print(f"Stream:   {name}")
    print(f"Server:   {server_url}")
    print(f"Package:  {PACKAGE_NAME}=={requested_version}")
    print(f"Target:   {install_dir}")
    print(f"Config:   {CONFIG_PATH}")
    print(f"Service:  {UNIT_NAME}")
    print(f"Marker:   {marker_path(INSTALL_NAME)}")
    print()
    print("Preflight:")
    print(f"  ✓ distro detected: {distro}")
    _packages, install_command, _query_method = DISTRO_PACKAGES[distro]
    for package, ok in package_statuses:
        if ok:
            print(f"  ✓ package {package} installed")
        else:
            print(f"  ✗ package {package} missing")
            print(f"    {install_command}")
    print()
    print("Plan:")
    print(f"  would create observer registration '{name}'")
    print(f"  would write {CONFIG_PATH}")
    print(
        "  would run pipx install --force --system-site-packages "
        f"{PACKAGE_NAME}=={requested_version}"
    )
    print(f"  would run {PACKAGE_NAME} install-service")
    print(f"  would restart {UNIT_NAME}")
    print("  would wait up to 30s for observer status")
    print(f"  would write marker {marker_path(INSTALL_NAME)}")
    print()
    print("Summary:")
    print("  Key prefix: <not generated in dry-run>")
    print(f"  Logs:       journalctl --user -u {UNIT_NAME} -f")
    print(f"  Status:     sol observer status {name}")
    print()
    print("Dry-run complete; no files were written.")
