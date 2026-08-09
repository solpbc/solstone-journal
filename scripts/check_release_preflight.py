#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Release environment checks for the Rust toolchain and wheel rail."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Literal, Mapping, Sequence

ROOT = Path(__file__).resolve().parent.parent
_SCRIPTS_DIR = Path(__file__).resolve().parent
for _path in (str(ROOT), str(_SCRIPTS_DIR)):
    if _path not in sys.path:
        sys.path.insert(0, _path)

from scripts.release_tool_pins import (  # noqa: E402
    CARGO_DENY_PIN,
    CARGO_DENY_VERSION,
    CARGO_VERSION_PIN,
    MACOS_CODESIGN_PATH,
    MACOS_CODESIGN_PUBLIC_PIN,
    MACOS_NOTARYTOOL_PIN,
    MACOS_SIGNING_MODE,
    MACOS_SWIFT_PIN,
    MACOS_XCODE_BUILD,
    MACOS_XCODE_PIN,
    MACOS_XCODE_VERSION,
    MATURIN_PIN,
    PYTHON_MACOS_VERSION,
    PYTHON_SOURCE_LINUX_VERSION,
    RUSTC_VERSION_BANNER,
    UV_PIN,
    ZIG_PIN,
    ZIG_VERSION,
    tool_value_matches_pin,
)

TOOLCHAIN_FILE = "rust-toolchain.toml"
COMPONENT_BINARIES = {
    "rustfmt": "rustfmt",
    "clippy": "cargo-clippy",
}
LaneName = Literal[
    "source",
    "linux-x86_64-musl",
    "linux-aarch64-musl",
    "macos-arm64",
]
LANE_TOOL_KEYS: dict[LaneName, tuple[str, ...]] = {
    "source": ("python", "rustc", "cargo", "uv", "maturin", "cargo-deny"),
    "linux-x86_64-musl": (
        "python",
        "rustc",
        "cargo",
        "uv",
        "maturin",
        "cargo-deny",
        "zig",
    ),
    "linux-aarch64-musl": (
        "python",
        "rustc",
        "cargo",
        "uv",
        "maturin",
        "cargo-deny",
        "zig",
    ),
    "macos-arm64": (
        "python",
        "rustc",
        "cargo",
        "uv",
        "maturin",
        "cargo-deny",
        "xcode",
        "swift",
        "codesign",
        "notarytool",
        "signing_mode",
    ),
}
PRESIGN_LANE_TOOL_KEYS: dict[LaneName, tuple[str, ...]] = {
    **{lane: keys for lane, keys in LANE_TOOL_KEYS.items() if lane != "macos-arm64"},
    "macos-arm64": tuple(
        key for key in LANE_TOOL_KEYS["macos-arm64"] if key != "signing_mode"
    ),
}


@dataclass(frozen=True)
class ToolchainSpec:
    channel: str
    components: tuple[str, ...]
    targets: tuple[str, ...]
    profile: str | None


@dataclass(frozen=True)
class Failure:
    error: str
    expected: str
    actual: str
    repair: str


Runner = Callable[..., subprocess.CompletedProcess[str]]


def _format_failures(failures: Sequence[Failure]) -> None:
    for failure in failures:
        print(f"ERROR: {failure.error}", file=sys.stderr)
        print(f"  expected: {failure.expected}", file=sys.stderr)
        print(f"  actual: {failure.actual}", file=sys.stderr)
        print(f"  repair command: {failure.repair}", file=sys.stderr)


def load_toolchain_spec(root: Path) -> ToolchainSpec:
    path = root / TOOLCHAIN_FILE
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    toolchain = data.get("toolchain", {})
    channel = toolchain.get("channel")
    components = toolchain.get("components", [])
    targets = toolchain.get("targets", [])
    profile = toolchain.get("profile")
    if not isinstance(channel, str) or not channel:
        raise ValueError(f"{TOOLCHAIN_FILE} must set [toolchain].channel")
    if not isinstance(components, list) or not all(
        isinstance(item, str) for item in components
    ):
        raise ValueError(f"{TOOLCHAIN_FILE} [toolchain].components must be strings")
    if not isinstance(targets, list) or not all(
        isinstance(item, str) for item in targets
    ):
        raise ValueError(f"{TOOLCHAIN_FILE} [toolchain].targets must be strings")
    if profile is not None and not isinstance(profile, str):
        raise ValueError(f"{TOOLCHAIN_FILE} [toolchain].profile must be a string")
    return ToolchainSpec(
        channel=channel,
        components=tuple(components),
        targets=tuple(targets),
        profile=profile,
    )


def check_rustup_override(
    expected: str,
    env: Mapping[str, str],
) -> list[Failure]:
    actual = env.get("RUSTUP_TOOLCHAIN")
    if actual is None or actual == expected:
        return []
    return [
        Failure(
            error="RUSTUP_TOOLCHAIN overrides the pinned release toolchain",
            expected=f"RUSTUP_TOOLCHAIN unset or {expected}",
            actual=actual,
            repair=f"unset RUSTUP_TOOLCHAIN || export RUSTUP_TOOLCHAIN={expected}",
        )
    ]


def rustup_home(env: Mapping[str, str]) -> Path:
    value = env.get("RUSTUP_HOME")
    if value:
        return Path(value).expanduser()
    return Path.home() / ".rustup"


def find_installed_toolchain(expected: str, rustup_home_path: Path) -> Path | None:
    toolchains = rustup_home_path / "toolchains"
    if not toolchains.is_dir():
        return None
    candidates = sorted(
        path
        for path in toolchains.iterdir()
        if path.is_dir()
        and (path.name == expected or path.name.startswith(f"{expected}-"))
        and (path / "bin" / "rustc").is_file()
    )
    return candidates[0] if candidates else None


def check_toolchain_installed(
    expected: str,
    rustup_home_path: Path,
) -> tuple[Path | None, list[Failure]]:
    toolchain_dir = find_installed_toolchain(expected, rustup_home_path)
    if toolchain_dir is not None:
        return toolchain_dir, []
    return (
        None,
        [
            Failure(
                error="required Rust toolchain is not installed",
                expected=f"installed rustup toolchain {expected}",
                actual=f"no {expected}-* directory with bin/rustc under {rustup_home_path / 'toolchains'}",
                repair=f"rustup toolchain install {expected}",
            )
        ],
    )


def check_rustc_version(
    toolchain_dir: Path,
    expected: str,
    *,
    runner: Runner = subprocess.run,
) -> list[Failure]:
    rustc = toolchain_dir / "bin" / "rustc"
    result = runner(
        [str(rustc), "--version"],
        capture_output=True,
        text=True,
        check=False,
    )
    actual = (
        result.stdout.strip() or result.stderr.strip() or f"exit {result.returncode}"
    )
    if result.returncode != 0 or not actual.startswith(f"rustc {expected} "):
        return [
            Failure(
                error="rustc version does not match the pinned release toolchain",
                expected=f"rustc {expected}",
                actual=actual,
                repair=f"rustup toolchain install {expected}",
            )
        ]
    return []


def check_toolchain_components(
    toolchain_dir: Path,
    channel: str,
    components: Sequence[str],
) -> list[Failure]:
    failures: list[Failure] = []
    for component in components:
        binary = COMPONENT_BINARIES.get(component)
        if binary is None:
            failures.append(
                Failure(
                    error="release toolchain component has no filesystem check",
                    expected=f"known component in {sorted(COMPONENT_BINARIES)}",
                    actual=component,
                    repair=f"rustup component add --toolchain {channel} {component}",
                )
            )
            continue
        path = toolchain_dir / "bin" / binary
        if path.is_file():
            continue
        failures.append(
            Failure(
                error="release toolchain component is not installed",
                expected=f"{component} component marker {path}",
                actual="missing",
                repair=f"rustup component add --toolchain {channel} {component}",
            )
        )
    return failures


def check_toolchain_targets(
    toolchain_dir: Path,
    channel: str,
    targets: Sequence[str],
) -> list[Failure]:
    failures: list[Failure] = []
    for target in targets:
        path = toolchain_dir / "lib" / "rustlib" / target / "lib"
        if path.is_dir():
            continue
        failures.append(
            Failure(
                error="release toolchain target is not installed",
                expected=f"target stdlib directory {path}",
                actual="missing",
                repair=f"rustup target add --toolchain {channel} {target}",
            )
        )
    return failures


def check_zig(
    expected: str = ZIG_VERSION,
    *,
    which: Callable[[str], str | None] = shutil.which,
    runner: Runner = subprocess.run,
) -> list[Failure]:
    zig = which("zig")
    if zig is None:
        return [
            Failure(
                error="zig is not on PATH",
                expected=f"zig {expected}",
                actual="not found",
                repair=f"python3 -m pip install --user ziglang=={expected}",
            )
        ]
    result = runner(
        [zig, "version"],
        capture_output=True,
        text=True,
        check=False,
    )
    actual = (
        result.stdout.strip() or result.stderr.strip() or f"exit {result.returncode}"
    )
    if result.returncode != 0 or actual != expected:
        return [
            Failure(
                error="zig version does not match the supported wheel rail",
                expected=expected,
                actual=actual,
                repair=f"python3 -m pip install --user ziglang=={expected}",
            )
        ]
    return []


def check_cargo_deny(
    expected: str = CARGO_DENY_VERSION,
    *,
    which: Callable[[str], str | None] = shutil.which,
    runner: Runner = subprocess.run,
) -> list[Failure]:
    cargo_deny = which("cargo-deny")
    repair = f"cargo install cargo-deny@{expected} --locked --force"
    if cargo_deny is None:
        return [
            Failure(
                error="cargo-deny is not on PATH",
                expected=expected,
                actual="not found",
                repair=repair,
            )
        ]
    result = runner(
        [cargo_deny, "--version"],
        capture_output=True,
        text=True,
        check=False,
    )
    actual = (
        result.stdout.strip() or result.stderr.strip() or f"exit {result.returncode}"
    )
    parts = actual.split()
    if (
        result.returncode != 0
        or len(parts) < 2
        or parts[0] != "cargo-deny"
        or parts[1] != expected
    ):
        return [
            Failure(
                error="cargo-deny version does not match the Rust dependency policy baseline",
                expected=expected,
                actual=actual,
                repair=repair,
            )
        ]
    return []


def _expected_lane_tool_evidence(
    lane: LaneName,
    *,
    keys_by_lane: Mapping[LaneName, tuple[str, ...]],
) -> dict[str, str]:
    if lane not in keys_by_lane:
        raise ValueError(f"unknown release lane: {lane}")
    common = {
        "python": PYTHON_MACOS_VERSION
        if lane == "macos-arm64"
        else PYTHON_SOURCE_LINUX_VERSION,
        "rustc": RUSTC_VERSION_BANNER,
        "cargo": CARGO_VERSION_PIN,
        "uv": UV_PIN,
        "maturin": MATURIN_PIN,
        "cargo-deny": CARGO_DENY_PIN,
    }
    if lane in {"linux-x86_64-musl", "linux-aarch64-musl"}:
        common["zig"] = ZIG_PIN
    if lane == "macos-arm64":
        common.update(
            {
                "xcode": MACOS_XCODE_PIN,
                "swift": MACOS_SWIFT_PIN,
                "codesign": MACOS_CODESIGN_PUBLIC_PIN,
                "notarytool": MACOS_NOTARYTOOL_PIN,
                "signing_mode": MACOS_SIGNING_MODE,
            }
        )
    return {key: common[key] for key in keys_by_lane[lane]}


def expected_lane_tool_evidence(lane: LaneName) -> dict[str, str]:
    return _expected_lane_tool_evidence(lane, keys_by_lane=LANE_TOOL_KEYS)


def expected_presign_lane_tool_evidence(lane: LaneName) -> dict[str, str]:
    return _expected_lane_tool_evidence(lane, keys_by_lane=PRESIGN_LANE_TOOL_KEYS)


def check_lane_tool_evidence(
    lane: LaneName,
    evidence: Mapping[str, str],
) -> list[Failure]:
    expected = expected_lane_tool_evidence(lane)
    failures: list[Failure] = []
    if set(evidence) != set(expected):
        failures.append(
            Failure(
                error="release lane tool evidence keys do not match lane",
                expected=", ".join(expected),
                actual=", ".join(sorted(evidence)) or "<empty>",
                repair=f"python3 scripts/check_release_preflight.py lane-tools --lane {lane}",
            )
        )
    for key, expected_value in expected.items():
        actual = evidence.get(key)
        if not tool_value_matches_pin(key, expected_value, actual):
            failures.append(
                Failure(
                    error=f"release lane tool {key} is not pinned",
                    expected=expected_value,
                    actual=str(actual),
                    repair=f"python3 scripts/check_release_preflight.py lane-tools --lane {lane}",
                )
            )
    return failures


def check_presign_lane_tool_evidence(
    lane: LaneName,
    evidence: Mapping[str, str],
) -> list[Failure]:
    expected = expected_presign_lane_tool_evidence(lane)
    failures: list[Failure] = []
    if set(evidence) != set(expected):
        failures.append(
            Failure(
                error="pre-sign lane tool evidence keys do not match lane",
                expected=", ".join(expected),
                actual=", ".join(sorted(evidence)) or "<empty>",
                repair=f"python3 scripts/check_release_preflight.py lane-tools --lane {lane}",
            )
        )
    for key, expected_value in expected.items():
        actual = evidence.get(key)
        if not tool_value_matches_pin(key, expected_value, actual):
            failures.append(
                Failure(
                    error=f"pre-sign lane tool {key} is not pinned",
                    expected=expected_value,
                    actual=str(actual),
                    repair=f"python3 scripts/check_release_preflight.py lane-tools --lane {lane}",
                )
            )
    return failures


def finalize_macos_tool_evidence(
    preflight_evidence: Mapping[str, str],
    native_records: Sequence[Mapping[str, object]],
) -> tuple[dict[str, str] | None, list[Failure]]:
    failures = check_presign_lane_tool_evidence("macos-arm64", preflight_evidence)
    roles = {
        str(record.get("role")): record
        for record in native_records
        if isinstance(record, Mapping)
    }
    required_roles = {"root", "core", "speakers-analyze"}
    if not required_roles.issubset(roles):
        failures.append(
            Failure(
                error="macOS signed tool finalizer requires all native records",
                expected="at least root, core, and speakers-analyze native records",
                actual=", ".join(sorted(roles)) or "<empty>",
                repair="bash scripts/release.sh --candidate",
            )
        )
    for role, record in sorted(roles.items()):
        signing = record.get("signing")
        if record.get("signing_mode") != MACOS_SIGNING_MODE:
            failures.append(
                Failure(
                    error="macOS native record signing_mode is not final",
                    expected=MACOS_SIGNING_MODE,
                    actual=repr(record.get("signing_mode")),
                    repair="bash scripts/release.sh --candidate",
                )
            )
        if not isinstance(signing, Mapping) or any(
            signing.get(key) is not True
            for key in (
                "signer_pinned",
                "team_pinned",
                "hardened_runtime",
                "trusted_timestamp",
            )
        ):
            failures.append(
                Failure(
                    error="macOS native record signing verification is incomplete",
                    expected=f"{role} signed and timestamped native record",
                    actual=repr(signing),
                    repair="bash scripts/release.sh --candidate",
                )
            )
        if record.get("notarization_status") != "accepted":
            failures.append(
                Failure(
                    error="macOS native record notarization is not accepted",
                    expected="accepted",
                    actual=repr(record.get("notarization_status")),
                    repair="bash scripts/release.sh --candidate",
                )
            )
    if failures:
        return None, failures
    final = dict(preflight_evidence)
    final["signing_mode"] = MACOS_SIGNING_MODE
    final_failures = check_lane_tool_evidence("macos-arm64", final)
    if final_failures:
        return None, final_failures
    return final, []


def _tool_output(
    name: str,
    args: Sequence[str],
    *,
    which: Callable[[str], str | None],
    runner: Runner,
) -> str:
    path = which(name)
    if path is None:
        return "not found"
    result = runner(
        [path, *args],
        capture_output=True,
        text=True,
        check=False,
    )
    return result.stdout.strip() or result.stderr.strip() or f"exit {result.returncode}"


def _first_line(value: str) -> str:
    return value.splitlines()[0].strip() if value.splitlines() else value.strip()


def _xcode_evidence(output: str) -> str:
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    if (
        f"Xcode {MACOS_XCODE_VERSION}" in lines
        and f"Build version {MACOS_XCODE_BUILD}" in lines
    ):
        return MACOS_XCODE_PIN
    return "; ".join(lines) or "not found"


def collect_lane_tool_evidence(
    lane: LaneName,
    *,
    which: Callable[[str], str | None] = shutil.which,
    runner: Runner = subprocess.run,
    python_executable: str = sys.executable,
) -> dict[str, str]:
    if lane not in LANE_TOOL_KEYS:
        raise ValueError(f"unknown release lane: {lane}")
    python_result = runner(
        [python_executable, "--version"],
        capture_output=True,
        text=True,
        check=False,
    )
    python_output = (
        python_result.stdout.strip()
        or python_result.stderr.strip()
        or f"exit {python_result.returncode}"
    )
    evidence = {
        "python": python_output.removeprefix("Python ").strip(),
        "rustc": _tool_output("rustc", ["--version"], which=which, runner=runner),
        "cargo": _tool_output("cargo", ["--version"], which=which, runner=runner),
        "uv": _tool_output("uv", ["--version"], which=which, runner=runner),
        "maturin": _tool_output("maturin", ["--version"], which=which, runner=runner),
        "cargo-deny": _tool_output(
            "cargo-deny", ["--version"], which=which, runner=runner
        ),
    }
    if lane in {"linux-x86_64-musl", "linux-aarch64-musl"}:
        zig = _tool_output("zig", ["version"], which=which, runner=runner)
        evidence["zig"] = f"zig {zig}" if zig != "not found" else zig
    if lane == "macos-arm64":
        evidence["xcode"] = _xcode_evidence(
            _tool_output("xcodebuild", ["-version"], which=which, runner=runner)
        )
        evidence["swift"] = _first_line(
            _tool_output("swift", ["--version"], which=which, runner=runner)
        )
        codesign_path = which("codesign")
        evidence["codesign"] = (
            MACOS_CODESIGN_PUBLIC_PIN
            if codesign_path == MACOS_CODESIGN_PATH
            else codesign_path or "not found"
        )
        evidence["notarytool"] = _tool_output(
            "xcrun", ["notarytool", "--version"], which=which, runner=runner
        )
    return {key: evidence[key] for key in LANE_TOOL_KEYS[lane] if key in evidence}


def check_lane_tools(
    lane: LaneName,
    *,
    which: Callable[[str], str | None] = shutil.which,
    runner: Runner = subprocess.run,
    python_executable: str = sys.executable,
) -> list[Failure]:
    return check_presign_lane_tool_evidence(
        lane,
        collect_lane_tool_evidence(
            lane,
            which=which,
            runner=runner,
            python_executable=python_executable,
        ),
    )


def check_local_clean_status(status_output: str) -> list[Failure]:
    paths = [line for line in status_output.splitlines() if line.strip()]
    if not paths:
        return []
    return [
        Failure(
            error="working tree is dirty",
            expected="no tracked or untracked changes",
            actual="; ".join(paths),
            repair="git status --short --untracked-files=normal",
        )
    ]


def local_status(root: Path, *, runner: Runner = subprocess.run) -> str:
    result = runner(
        ["git", "status", "--porcelain", "--untracked-files=normal"],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "git status failed")
    return result.stdout


def check_remote_state(
    expected_ref: str,
    actual_ref: str,
    status_output: str,
    *,
    label: str,
) -> list[Failure]:
    failures: list[Failure] = []
    actual_ref = actual_ref.strip()
    if actual_ref != expected_ref:
        failures.append(
            Failure(
                error=f"{label} is not on the release ref",
                expected=expected_ref,
                actual=actual_ref or "<empty>",
                repair="python3 scripts/check_release_preflight.py remote-state --help",
            )
        )
    for failure in check_local_clean_status(status_output):
        failures.append(
            Failure(
                error=f"{label} working tree is dirty",
                expected=failure.expected,
                actual=failure.actual,
                repair="git status --short --untracked-files=normal",
            )
        )
    return failures


def check_pinned_toolchain(root: Path, env: Mapping[str, str]) -> list[Failure]:
    try:
        spec = load_toolchain_spec(root)
    except (OSError, tomllib.TOMLDecodeError, ValueError) as exc:
        return [
            Failure(
                error="release toolchain file is invalid",
                expected=f"valid {TOOLCHAIN_FILE} with [toolchain].channel",
                actual=str(exc),
                repair="$EDITOR rust-toolchain.toml",
            )
        ]
    failures = check_rustup_override(spec.channel, env)
    toolchain_dir, install_failures = check_toolchain_installed(
        spec.channel,
        rustup_home(env),
    )
    failures.extend(install_failures)
    if toolchain_dir is not None:
        failures.extend(check_rustc_version(toolchain_dir, spec.channel))
        failures.extend(
            check_toolchain_components(toolchain_dir, spec.channel, spec.components)
        )
        failures.extend(
            check_toolchain_targets(toolchain_dir, spec.channel, spec.targets)
        )
    return failures


def check_named_toolchain(toolchain: str, env: Mapping[str, str]) -> list[Failure]:
    toolchain_dir, failures = check_toolchain_installed(toolchain, rustup_home(env))
    if toolchain_dir is not None:
        failures.extend(check_rustc_version(toolchain_dir, toolchain))
    return failures


def _cmd_local(args: argparse.Namespace) -> int:
    root = args.root.resolve()
    failures = check_pinned_toolchain(root, os.environ)
    failures.extend(check_zig())
    if args.require_clean:
        try:
            status = local_status(root)
        except RuntimeError as exc:
            failures.append(
                Failure(
                    error="could not inspect local working tree",
                    expected="git status succeeds",
                    actual=str(exc),
                    repair="git status --short --untracked-files=normal",
                )
            )
        else:
            failures.extend(check_local_clean_status(status))
    if failures:
        _format_failures(failures)
        return 1
    print("release local preflight ok")
    return 0


def _cmd_msrv(args: argparse.Namespace) -> int:
    failures = check_named_toolchain(args.toolchain, os.environ)
    if failures:
        _format_failures(failures)
        return 1
    print(f"rust MSRV toolchain {args.toolchain} ok")
    return 0


def _cmd_cargo_deny(_args: argparse.Namespace) -> int:
    failures = check_cargo_deny()
    if failures:
        _format_failures(failures)
        return 1
    print(f"cargo-deny {CARGO_DENY_VERSION} ok")
    return 0


def _cmd_lane_tools(args: argparse.Namespace) -> int:
    failures = check_lane_tools(args.lane)
    if failures:
        _format_failures(failures)
        return 1
    print(f"{args.lane} preflight-valid non-release tool evidence ok")
    return 0


def _cmd_remote_state(args: argparse.Namespace) -> int:
    status = args.status_file.read_text(encoding="utf-8")
    failures = check_remote_state(
        args.expected_ref,
        args.actual_ref,
        status,
        label=args.label,
    )
    if failures:
        _format_failures(failures)
        return 1
    print(f"{args.label} release state ok")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    local = subparsers.add_parser("local")
    local.add_argument("--root", type=Path, default=Path("."))
    local.add_argument("--require-clean", action="store_true")
    local.set_defaults(func=_cmd_local)

    msrv = subparsers.add_parser("msrv")
    msrv.add_argument("--toolchain", required=True)
    msrv.set_defaults(func=_cmd_msrv)

    cargo_deny = subparsers.add_parser("cargo-deny")
    cargo_deny.set_defaults(func=_cmd_cargo_deny)

    lane_tools = subparsers.add_parser("lane-tools")
    lane_tools.add_argument(
        "--lane",
        choices=tuple(LANE_TOOL_KEYS),
        required=True,
    )
    lane_tools.set_defaults(func=_cmd_lane_tools)

    remote = subparsers.add_parser("remote-state")
    remote.add_argument("--label", required=True)
    remote.add_argument("--expected-ref", required=True)
    remote.add_argument("--actual-ref", required=True)
    remote.add_argument("--status-file", type=Path, required=True)
    remote.set_defaults(func=_cmd_remote_state)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
