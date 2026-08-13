#!/usr/bin/env python3
"""Capture and verify read-only launchctl/systemctl observation evidence.

This build-time tool never mutates a service manager.  Its command registry is
closed below; capture refuses every platform/result shape outside that registry.
"""

from __future__ import annotations

import argparse
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import selectors
import signal
import subprocess
import sys
import time
from typing import Any


SCHEMA = 1
PRODUCT_GROUND = "cd08e591e0c43405bd6d46092e78a1e51329c808"
ABSENT_UNIT = "solstone-reference-reserved-260813-never-exists.service"
ABSENT_LABEL = "org.solpbc.solstone"
MAX_STREAM_BYTES = 256 * 1024
MAX_INPUT_BYTES = 1024 * 1024
TIMEOUT_SECONDS = 2.0
SYSTEMD_PROPERTIES = (
    "Id",
    "LoadState",
    "ActiveState",
    "SubState",
    "FragmentPath",
    "SourcePath",
    "DropInPaths",
    "ExecStart",
    "UnitFileState",
)
DARWIN_LOADED_LABELS = (
    ("loaded-running", "com.apple.cloudphotod"),
    ("loaded-stopped", "com.apple.enhancedloggingd"),
)
EXPECTED_PROFILES = {
    "linux-systemd-255-ubuntu": ("linux", "255", "x86_64", "ubuntu", "24.04"),
    "linux-systemd-258-fedora": ("linux", "258", "x86_64", "fedora", "43"),
    "linux-systemd-261-suse": (
        "linux",
        "261",
        "x86_64",
        "opensuse-tumbleweed",
        "20260720",
    ),
    "darwin-launchd-3102": ("darwin", "3102", "arm64", "darwin", "26.5"),
}

LAUNCHCTL_TOP_LEVEL_FIELDS = {
    "active count",
    "path",
    "type",
    "state",
    "program",
    "arguments",
    "inherited environment",
    "default environment",
    "environment",
    "domain",
    "asid",
    "minimum runtime",
    "base minimum runtime",
    "exit timeout",
    "runs",
    "pid",
    "immediate reason",
    "forks",
    "execs",
    "initialized",
    "trampolined",
    "started suspended",
    "proxy started suspended",
    "checked allocations",
    "checked allocations reason",
    "checked allocations flags",
    "last exit code",
    "event triggers",
    "endpoints",
    "event channels",
    "resource coalition",
    "jetsam coalition",
    "spawn type",
    "jetsam priority",
    "jetsam memory limit (active, soft)",
    "jetsam memory limit (inactive, soft)",
    "jetsam memory limit (inactive, hard)",
    "jetsamproperties category",
    "jetsam thread limit",
    "cpumon",
    "exponential throttling grace limit",
    "job state",
    "properties",
}


class EvidenceError(RuntimeError):
    pass


def canonical_bytes(value: Any) -> bytes:
    return (
        json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"))
        + "\n"
    ).encode("ascii")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def tool_sha256() -> str:
    return sha256(Path(__file__).read_bytes())


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


def unb64(value: str) -> bytes:
    try:
        return base64.b64decode(value, validate=True)
    except (ValueError, TypeError) as exc:
        raise EvidenceError("invalid base64") from exc


def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise EvidenceError(f"duplicate JSON key: {key!r}")
        value[key] = item
    return value


def load_json_bytes(data: bytes) -> Any:
    try:
        return json.loads(data.decode("ascii"), object_pairs_hook=reject_duplicate_keys)
    except UnicodeDecodeError as exc:
        raise EvidenceError("input is not ASCII JSON") from exc


def exact_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def validate_result(value: Any) -> dict[str, Any]:
    expected = {
        "argv",
        "returncode",
        "stdout_b64",
        "stderr_b64",
        "stdout_sha256",
        "stderr_sha256",
    }
    if not isinstance(value, dict) or set(value) != expected:
        raise EvidenceError("result shape mismatch")
    if (
        not isinstance(value["argv"], list)
        or not value["argv"]
        or not all(isinstance(item, str) for item in value["argv"])
        or not exact_int(value["returncode"])
    ):
        raise EvidenceError("result argv/returncode type mismatch")
    stdout = unb64(value["stdout_b64"])
    stderr = unb64(value["stderr_b64"])
    if len(stdout) > MAX_STREAM_BYTES or len(stderr) > MAX_STREAM_BYTES:
        raise EvidenceError("retained stream exceeds bound")
    if sha256(stdout) != value["stdout_sha256"] or sha256(stderr) != value["stderr_sha256"]:
        raise EvidenceError("raw digest mismatch")
    return value


def run_exact(argv: list[str], env: dict[str, str]) -> dict[str, Any]:
    try:
        child = subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            start_new_session=True,
        )
    except OSError as exc:
        raise EvidenceError(f"command failed to spawn: {argv!r}: {exc}") from exc

    assert child.stdout is not None and child.stderr is not None
    stdout_fd = child.stdout.fileno()
    stderr_fd = child.stderr.fileno()
    streams = {stdout_fd: bytearray(), stderr_fd: bytearray()}
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ)
    selector.register(child.stderr, selectors.EVENT_READ)
    deadline = time.monotonic() + TIMEOUT_SECONDS
    failure: str | None = None
    try:
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                failure = "timed out"
                break
            for key, _ in selector.select(remaining):
                chunk = os.read(key.fd, 65536)
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                target = streams[key.fd]
                target.extend(chunk)
                if len(target) > MAX_STREAM_BYTES:
                    failure = "output exceeded bound"
                    break
            if failure is not None:
                break
        if failure is None:
            remaining = max(0.0, deadline - time.monotonic())
            try:
                returncode = child.wait(timeout=remaining)
            except subprocess.TimeoutExpired:
                failure = "timed out"
        if failure is not None:
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                child.wait(timeout=0.25)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait()
            raise EvidenceError(f"command {failure}: {argv!r}")
    finally:
        selector.close()
        child.stdout.close()
        child.stderr.close()

    stdout = bytes(streams[stdout_fd])
    stderr = bytes(streams[stderr_fd])
    return {
        "argv": argv,
        "returncode": returncode,
        "stdout_b64": b64(stdout),
        "stderr_b64": b64(stderr),
        "stdout_sha256": sha256(stdout),
        "stderr_sha256": sha256(stderr),
    }


def controlled_env(kind: str) -> tuple[dict[str, str], dict[str, str]]:
    env = {"LANG": "C", "LC_ALL": "C", "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"}
    recorded = dict(env)
    if kind == "linux":
        runtime_dir = f"/run/user/{os.getuid()}"
        env["XDG_RUNTIME_DIR"] = runtime_dir
        recorded["XDG_RUNTIME_DIR"] = "/run/user/<uid>"
    return env, recorded


def systemd_argv(unit: str) -> list[str]:
    properties = ",".join(SYSTEMD_PROPERTIES)
    return [
        "/usr/bin/systemctl",
        "--user",
        "show",
        unit,
        f"--property={properties}",
        "--no-pager",
    ]


_EXEC_START = re.compile(
    r"^\{ path=(?P<path>[^ ;]+) ; argv\[\]=(?P<argv>.*?) ; "
    r"ignore_errors=(?:yes|no) ; .* \}$"
)


def parse_exec_start(value: str) -> dict[str, Any]:
    match = _EXEC_START.fullmatch(value)
    if match is None:
        raise EvidenceError("systemd ExecStart shape mismatch")
    argv = match.group("argv").split(" ")
    if not argv or any(not item for item in argv) or argv[0] != match.group("path"):
        raise EvidenceError("systemd ExecStart argv mismatch")
    return {"path": match.group("path"), "argv": argv}


def parse_systemd(raw: bytes) -> dict[str, Any]:
    try:
        text = raw.decode("utf-8", "strict")
    except UnicodeDecodeError as exc:
        raise EvidenceError("systemd output is not strict UTF-8") from exc
    values: dict[str, str] = {}
    for line in text.splitlines():
        if "=" not in line:
            raise EvidenceError(f"malformed systemd line: {line!r}")
        key, value = line.split("=", 1)
        if key not in SYSTEMD_PROPERTIES:
            raise EvidenceError(f"unknown systemd property: {key!r}")
        if key in values:
            raise EvidenceError(f"duplicate systemd property: {key!r}")
        values[key] = parse_exec_start(value) if key == "ExecStart" else value
    missing = set(SYSTEMD_PROPERTIES) - set(values)
    if missing:
        # systemctl deliberately omits ExecStart entirely for a not-found unit;
        # preserve that raw omission instead of manufacturing an empty value.
        if missing != {"ExecStart"} or values.get("LoadState") != "not-found":
            raise EvidenceError(f"missing systemd properties: {sorted(missing)!r}")
    return values


_DARWIN_HEADER = re.compile(r'^gui/(?P<uid>[0-9]+)/(?P<label>[^\n]+) = \{\n')


def parse_launchctl_loaded(raw: bytes) -> dict[str, Any]:
    try:
        text = raw.decode("utf-8", "strict")
    except UnicodeDecodeError as exc:
        raise EvidenceError("launchctl output is not strict UTF-8") from exc
    match = _DARWIN_HEADER.match(text)
    if match is None:
        raise EvidenceError("launchctl loaded header mismatch")
    if not text.endswith("}\n") or text.count("\n}\n") != 1:
        raise EvidenceError("launchctl outer framing mismatch")

    for line in text.splitlines():
        scalar = re.match(r"^\t([^\t={]+?) = .*$", line)
        if scalar is not None and scalar.group(1) not in LAUNCHCTL_TOP_LEVEL_FIELDS:
            raise EvidenceError(f"unknown launchctl top-level field: {scalar.group(1)!r}")

    def one(name: str) -> str:
        found = re.findall(rf"^\t{re.escape(name)} = (.*)$", text, re.MULTILINE)
        if len(found) != 1:
            raise EvidenceError(f"launchctl field {name!r} count is {len(found)}")
        return found[0]

    argument_blocks = list(re.finditer(r"^\targuments = \{\n(?P<body>.*?)^\t\}\n", text, re.MULTILINE | re.DOTALL))
    if len(argument_blocks) != 1:
        raise EvidenceError("launchctl arguments block missing")
    arguments_match = argument_blocks[0]
    arguments: list[str] = []
    for line in arguments_match.group("body").splitlines():
        if not line.startswith("\t\t"):
            raise EvidenceError(f"malformed launchctl argument: {line!r}")
        arguments.append(line[2:])
    if not arguments:
        raise EvidenceError("launchctl arguments block empty")
    return {
        "label": match.group("label"),
        "uid": int(match.group("uid")),
        "path": one("path"),
        "type": one("type"),
        "state": one("state"),
        "program": one("program"),
        "arguments": arguments,
    }


def parse_launchctl_absent(result: dict[str, Any]) -> dict[str, Any]:
    if result["returncode"] != 113:
        raise EvidenceError("reserved Darwin absence exit mismatch")
    if unb64(result["stdout_b64"]):
        raise EvidenceError("absent launchctl row unexpectedly wrote stdout")
    try:
        stderr = unb64(result["stderr_b64"]).decode("utf-8", "strict")
    except UnicodeDecodeError as exc:
        raise EvidenceError("absent launchctl stderr is not strict UTF-8") from exc
    match = re.fullmatch(
        r'Bad request\.\nCould not find service "(?P<label>[^"]+)" in domain for user gui: (?P<uid>[0-9]+)\n',
        stderr,
    )
    if match is None or match.group("label") != ABSENT_LABEL:
        raise EvidenceError("reserved Darwin absence shape mismatch")
    return {"label": match.group("label"), "uid": int(match.group("uid"))}


def checked_row(role: str, result: dict[str, Any], semantics: dict[str, Any]) -> dict[str, Any]:
    return {"role": role, "result": result, "semantics": semantics}


def replace_systemd_property(raw: bytes, key: str, value: str) -> bytes:
    text = raw.decode("utf-8", "strict")
    pattern = re.compile(rf"^{re.escape(key)}=.*$", re.MULTILINE)
    if len(pattern.findall(text)) != 1:
        raise EvidenceError(f"cannot derive systemd property {key!r}")
    return pattern.sub(f"{key}={value}", text).encode("utf-8")


def replace_launchctl_field(raw: bytes, key: str, value: str) -> bytes:
    text = raw.decode("utf-8", "strict")
    if key == "uid":
        changed, count = re.subn(
            r"^gui/[0-9]+/", f"gui/{value}/", text, count=1, flags=re.MULTILINE
        )
    elif key == "label":
        changed, count = re.subn(r"^(gui/[0-9]+/)[^\n]+( = \{)$", rf"\g<1>{value}\g<2>", text, count=1, flags=re.MULTILINE)
    elif key == "arguments":
        body = "".join(f"\t\t{item}\n" for item in value.split("\0"))
        changed, count = re.subn(
            r"(^\targuments = \{\n).*?(^\t\}\n)",
            rf"\g<1>{body}\g<2>",
            text,
            count=1,
            flags=re.MULTILINE | re.DOTALL,
        )
    else:
        changed, count = re.subn(
            rf"^\t{re.escape(key)} = .*$",
            f"\t{key} = {value}",
            text,
            count=1,
            flags=re.MULTILINE,
        )
    if count != 1:
        raise EvidenceError(f"cannot derive launchctl field {key!r}")
    return changed.encode("utf-8")


def linux_os_identity() -> dict[str, str]:
    path = Path("/etc/os-release")
    data = path.read_bytes()
    if len(data) > 64 * 1024:
        raise EvidenceError("os-release exceeds bound")
    try:
        text = data.decode("utf-8", "strict")
    except UnicodeDecodeError as exc:
        raise EvidenceError("os-release is not strict UTF-8") from exc
    values: dict[str, str] = {}
    for line in text.splitlines():
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        if key not in {"ID", "VERSION_ID"}:
            continue
        if key in values:
            raise EvidenceError(f"duplicate os-release field: {key}")
        if len(value) >= 2 and value[0] == value[-1] == '"':
            value = value[1:-1]
        if not re.fullmatch(r"[A-Za-z0-9._-]+", value):
            raise EvidenceError(f"invalid os-release field: {key}")
        values[key] = value
    if set(values) != {"ID", "VERSION_ID"}:
        raise EvidenceError("os-release identity fields missing")
    return {"id": values["ID"], "version_id": values["VERSION_ID"]}


def set_result_stream(result: dict[str, Any], stream: str, data: bytes) -> None:
    result[f"{stream}_b64"] = b64(data)
    result[f"{stream}_sha256"] = sha256(data)


def derivation_report(captures: list[dict[str, Any]]) -> dict[str, Any]:
    accepted_cases: list[str] = []
    rejected_cases: list[str] = []
    for capture in captures:
        profile = capture["profile"]
        if capture["platform"] == "linux":
            row = next(item for item in capture["rows"] if item["role"] == "loaded")
            raw = unb64(row["result"]["stdout_b64"])
            base = parse_systemd(raw)
            variants = {
                "Id": ("alpha.service", "beta.service"),
                "LoadState": ("loaded-alpha", "loaded-beta"),
                "FragmentPath": ("/one/alpha.service", "/two/beta.service"),
                "SourcePath": ("/one/source", "/two/source"),
                "DropInPaths": ("/one/a.conf", "/two/b.conf /two/c.conf"),
                "ExecStart": (
                    "{ path=/one ; argv[]=/one a ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }",
                    "{ path=/two ; argv[]=/two b ; ignore_errors=yes ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }",
                ),
                "ActiveState": ("active", "inactive"),
                "SubState": ("running", "dead"),
                "UnitFileState": ("enabled", "disabled"),
            }
            for key, values in variants.items():
                for index, value in enumerate(values):
                    parsed = parse_systemd(replace_systemd_property(raw, key, value))
                    expected = dict(base)
                    expected[key] = parse_exec_start(value) if key == "ExecStart" else value
                    if parsed != expected:
                        raise EvidenceError(f"systemd derivation mismatch: {key}")
                    accepted_cases.append(f"{profile}:systemd:{key}:{index}")
            original_exec = next(
                line.split("=", 1)[1]
                for line in raw.decode("utf-8", "strict").splitlines()
                if line.startswith("ExecStart=")
            )
            structural = parse_exec_start(original_exec)
            for index, decoration in enumerate(
                (
                    "start_time=[Thu 2026-01-01 00:00:00 UTC] ; stop_time=[n/a] ; pid=17 ; code=exited ; status=0/0",
                    "start_time=[n/a] ; stop_time=[Fri 2026-01-02 00:00:00 UTC] ; pid=99999 ; code=killed ; status=15/TERM",
                )
            ):
                changed = re.sub(
                    r"start_time=.* ; stop_time=.* ; pid=.* ; code=.* ; status=.*(?= \}$)",
                    decoration,
                    original_exec,
                )
                if parse_exec_start(changed) != structural:
                    raise EvidenceError("systemd dynamic ExecStart decoration leaked")
                accepted_cases.append(f"{profile}:systemd:ExecStart-decoration:{index}")
            malformed_rows = (
                ("duplicate-property", raw + b"Id=duplicate.service\n"),
                ("missing-property", raw.replace(b"LoadState=loaded\n", b"", 1)),
                ("unknown-property", raw + b"UnknownProperty=x\n"),
                ("malformed-line", raw + b"not-a-property\n"),
            )
            for name, malformed in malformed_rows:
                try:
                    parse_systemd(malformed)
                except EvidenceError:
                    rejected_cases.append(f"{profile}:systemd:{name}")
                else:
                    raise EvidenceError("systemd malformed derivation accepted")
        else:
            row = next(item for item in capture["rows"] if item["role"] == "loaded-running")
            raw = unb64(row["result"]["stdout_b64"])
            base = parse_launchctl_loaded(raw)
            variants = {
                "label": ("org.example.alpha", "org.example.beta"),
                "uid": ("502", "503"),
                "path": ("/Library/LaunchAgents/alpha.plist", "/Library/LaunchAgents/beta.plist"),
                "type": ("Background", "Interactive"),
                "program": ("/usr/bin/alpha", "/usr/bin/beta"),
                "state": ("running", "not running"),
                "arguments": ("/usr/bin/alpha\0one", "/usr/bin/beta\0two"),
            }
            for key, values in variants.items():
                for index, value in enumerate(values):
                    parsed = parse_launchctl_loaded(replace_launchctl_field(raw, key, value))
                    expected = dict(base)
                    if key == "arguments":
                        expected[key] = value.split("\0")
                    elif key == "uid":
                        expected[key] = int(value)
                    else:
                        expected[key] = value
                    if parsed != expected:
                        raise EvidenceError(f"launchctl derivation mismatch: {key}")
                    accepted_cases.append(f"{profile}:launchctl:{key}:{index}")
            malformed_rows = (
                ("missing-program", raw.replace(b"\tprogram = ", b"\tprogram-x = ", 1)),
                ("duplicate-program", raw + b"\tprogram = /duplicate\n"),
                ("bad-argument", raw.replace(b"\targuments = {\n", b"\targuments = {\n\tbad\n", 1)),
                ("unknown-top-level", raw.replace(b"\tpath = ", b"\tunknown = x\n\tpath = ", 1)),
            )
            for name, malformed in malformed_rows:
                try:
                    parse_launchctl_loaded(malformed)
                except EvidenceError:
                    rejected_cases.append(f"{profile}:launchctl:{name}")
                else:
                    raise EvidenceError("launchctl malformed derivation accepted")

        for row_index, row in enumerate(capture["rows"]):
            for mutation in ("wrong-exit", "wrong-stdout", "wrong-stderr"):
                changed = copy.deepcopy(capture)
                result = changed["rows"][row_index]["result"]
                if mutation == "wrong-exit":
                    result["returncode"] = 1 if result["returncode"] == 0 else 0
                else:
                    stream = mutation.removeprefix("wrong-")
                    set_result_stream(result, stream, unb64(result[f"{stream}_b64"]) + b"unexpected\n")
                try:
                    validate_partial(changed)
                except EvidenceError:
                    rejected_cases.append(f"{profile}:{row['role']}:{mutation}")
                else:
                    raise EvidenceError(f"row outcome mutation accepted: {profile}:{row['role']}:{mutation}")

    return {
        "accepted": len(accepted_cases),
        "accepted_cases": sorted(accepted_cases),
        "rejected": len(rejected_cases),
        "rejected_cases": sorted(rejected_cases),
    }


def capture_linux(profile: str) -> dict[str, Any]:
    env, recorded_env = controlled_env("linux")
    version = run_exact(["/usr/bin/systemctl", "--version"], env)
    absent = run_exact(systemd_argv(ABSENT_UNIT), env)
    loaded = run_exact(systemd_argv("dbus.service"), env)
    if version["returncode"] != 0 or version["stderr_b64"] != b64(b""):
        raise EvidenceError("systemctl version failed")
    absent_values = parse_systemd(unb64(absent["stdout_b64"]))
    loaded_values = parse_systemd(unb64(loaded["stdout_b64"]))
    if absent["returncode"] != 0 or unb64(absent["stderr_b64"]):
        raise EvidenceError("systemd absent observation failed")
    if absent_values["Id"] != ABSENT_UNIT or absent_values["LoadState"] != "not-found":
        raise EvidenceError("reserved Linux unit unexpectedly exists")
    if loaded["returncode"] != 0 or unb64(loaded["stderr_b64"]):
        raise EvidenceError("systemd loaded observation failed")
    if loaded_values["LoadState"] != "loaded" or not loaded_values["FragmentPath"]:
        raise EvidenceError("Linux loaded example is not file-backed")
    return {
        "schema": SCHEMA,
        "product_ground": PRODUCT_GROUND,
        "tool_sha256": tool_sha256(),
        "profile": profile,
        "platform": "linux",
        "machine": platform.machine(),
        "python_version": platform.python_version(),
        "os_identity": linux_os_identity(),
        "controlled_env": recorded_env,
        "version": version,
        "rows": [
            checked_row("absent", absent, absent_values),
            checked_row("loaded", loaded, loaded_values),
        ],
    }


def capture_darwin(profile: str) -> dict[str, Any]:
    env, recorded_env = controlled_env("darwin")
    version = run_exact(["/bin/launchctl", "version"], env)
    os_version = run_exact(["/usr/bin/sw_vers", "-productVersion"], env)
    uid = os.getuid()
    absent = run_exact(["/bin/launchctl", "print", f"gui/{uid}/{ABSENT_LABEL}"], env)
    if (
        version["returncode"] != 0
        or os_version["returncode"] != 0
        or unb64(version["stderr_b64"])
        or unb64(os_version["stderr_b64"])
    ):
        raise EvidenceError("Darwin version command failed")
    rows = [checked_row("absent", absent, parse_launchctl_absent(absent))]
    for role, label in DARWIN_LOADED_LABELS:
        result = run_exact(["/bin/launchctl", "print", f"gui/{uid}/{label}"], env)
        if result["returncode"] != 0 or unb64(result["stderr_b64"]):
            raise EvidenceError(f"Darwin loaded example unavailable: {label}")
        semantics = parse_launchctl_loaded(unb64(result["stdout_b64"]))
        if semantics["label"] != label:
            raise EvidenceError(f"Darwin loaded label mismatch: {label}")
        if role == "loaded-running" and semantics["state"] != "running":
            raise EvidenceError("Darwin running example is not running")
        if role == "loaded-stopped" and semantics["state"] == "running":
            raise EvidenceError("Darwin stopped example is running")
        rows.append(checked_row(role, result, semantics))
    return {
        "schema": SCHEMA,
        "product_ground": PRODUCT_GROUND,
        "tool_sha256": tool_sha256(),
        "profile": profile,
        "platform": "darwin",
        "machine": platform.machine(),
        "python_version": platform.python_version(),
        "controlled_env": recorded_env,
        "version": version,
        "os_version": os_version,
        "rows": rows,
    }


def validate_partial(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise EvidenceError("partial is not an object")
    common = {
        "schema",
        "product_ground",
        "tool_sha256",
        "profile",
        "platform",
        "machine",
        "python_version",
        "controlled_env",
        "version",
        "rows",
    }
    expected = common | ({"os_version"} if value.get("platform") == "darwin" else set())
    if value.get("platform") == "linux":
        expected.add("os_identity")
    if set(value) != expected:
        raise EvidenceError(f"partial keys mismatch: {sorted(set(value) ^ expected)!r}")
    if not exact_int(value["schema"]) or value["schema"] != SCHEMA or value["product_ground"] != PRODUCT_GROUND:
        raise EvidenceError("partial ground/schema mismatch")
    if value["tool_sha256"] != tool_sha256():
        raise EvidenceError("partial tool mismatch")
    if not isinstance(value["profile"], str) or not re.fullmatch(r"[a-z0-9][a-z0-9-]{2,63}", value["profile"]):
        raise EvidenceError("invalid profile")
    if value["platform"] not in {"linux", "darwin"}:
        raise EvidenceError("invalid platform")
    expected_profile = EXPECTED_PROFILES.get(value["profile"])
    if expected_profile is None or expected_profile[0] != value["platform"]:
        raise EvidenceError("profile/platform mismatch")
    if value["machine"] != expected_profile[2] or not isinstance(value["python_version"], str):
        raise EvidenceError("machine/Python version type mismatch")
    expected_env = {"LANG": "C", "LC_ALL": "C", "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"}
    if value["platform"] == "linux":
        expected_env["XDG_RUNTIME_DIR"] = "/run/user/<uid>"
    if value["controlled_env"] != expected_env:
        raise EvidenceError("controlled environment mismatch")
    version_result = validate_result(value["version"])
    version_stdout = unb64(version_result["stdout_b64"])
    if version_result["returncode"] != 0 or unb64(version_result["stderr_b64"]):
        raise EvidenceError("tool version result failed")
    version_text = version_stdout.decode("utf-8", "strict")
    if value["platform"] == "linux":
        if version_result["argv"] != ["/usr/bin/systemctl", "--version"]:
            raise EvidenceError("systemctl version argv mismatch")
        if not version_text.startswith(f"systemd {expected_profile[1]} "):
            raise EvidenceError("systemd version/profile mismatch")
        if value["os_identity"] != {"id": expected_profile[3], "version_id": expected_profile[4]}:
            raise EvidenceError("Linux OS identity/profile mismatch")
    else:
        os_version_result = validate_result(value["os_version"])
        if version_result["argv"] != ["/bin/launchctl", "version"] or os_version_result["argv"] != ["/usr/bin/sw_vers", "-productVersion"]:
            raise EvidenceError("Darwin version argv mismatch")
        if os_version_result["returncode"] != 0 or unb64(os_version_result["stderr_b64"]):
            raise EvidenceError("Darwin OS version result failed")
        if unb64(os_version_result["stdout_b64"]) != f"{expected_profile[4]}\n".encode("ascii"):
            raise EvidenceError("Darwin OS version/profile mismatch")
        if f"libxpc_executables-{expected_profile[1]}." not in version_text:
            raise EvidenceError("launchctl version/profile mismatch")
    if not isinstance(value["rows"], list):
        raise EvidenceError("rows is not a list")
    roles: set[str] = set()
    for row in value["rows"]:
        if not isinstance(row, dict) or set(row) != {"role", "result", "semantics"}:
            raise EvidenceError("row shape mismatch")
        if row["role"] in roles:
            raise EvidenceError("duplicate row role")
        roles.add(row["role"])
        result = validate_result(row["result"])
        stdout = unb64(result["stdout_b64"])
        stderr = unb64(result["stderr_b64"])
        if value["platform"] == "linux" and row["role"] in {"absent", "loaded"}:
            expected_argv = systemd_argv(ABSENT_UNIT if row["role"] == "absent" else "dbus.service")
            if result["argv"] != expected_argv:
                raise EvidenceError("Linux row argv mismatch")
            if result["returncode"] != 0 or stderr:
                raise EvidenceError("Linux row outcome mismatch")
            if parse_systemd(stdout) != row["semantics"]:
                raise EvidenceError("Linux semantic mismatch")
        elif value["platform"] == "darwin" and row["role"] == "absent":
            semantics = parse_launchctl_absent(result)
            expected_argv = ["/bin/launchctl", "print", f"gui/{semantics['uid']}/{ABSENT_LABEL}"]
            if result["argv"] != expected_argv:
                raise EvidenceError("Darwin absent argv mismatch")
            if semantics != row["semantics"]:
                raise EvidenceError("Darwin absent semantic mismatch")
        elif value["platform"] == "darwin" and row["role"] in {"loaded-running", "loaded-stopped"}:
            role_labels = dict(DARWIN_LOADED_LABELS)
            if result["returncode"] != 0 or stderr:
                raise EvidenceError("Darwin loaded outcome mismatch")
            semantics = parse_launchctl_loaded(stdout)
            expected_argv = [
                "/bin/launchctl",
                "print",
                f"gui/{semantics['uid']}/{role_labels[row['role']]}",
            ]
            if result["argv"] != expected_argv:
                raise EvidenceError("Darwin loaded argv mismatch")
            if semantics != row["semantics"]:
                raise EvidenceError("Darwin loaded semantic mismatch")
        else:
            raise EvidenceError("unknown row role")
    expected_roles = {"absent", "loaded"} if value["platform"] == "linux" else {"absent", "loaded-running", "loaded-stopped"}
    if roles != expected_roles:
        raise EvidenceError("row denominator mismatch")
    if value["platform"] == "darwin":
        uids = {row["semantics"]["uid"] for row in value["rows"]}
        if len(uids) != 1:
            raise EvidenceError("Darwin row UID mismatch")
    return value


def combine(paths: list[Path]) -> dict[str, Any]:
    captures = []
    for path in paths:
        data = path.read_bytes()
        if len(data) > MAX_INPUT_BYTES:
            raise EvidenceError(f"input exceeds bound: {path}")
        captures.append(validate_partial(load_json_bytes(data)))
    profiles = [item["profile"] for item in captures]
    if len(set(profiles)) != len(profiles):
        raise EvidenceError("duplicate capture profile")
    if set(profiles) != set(EXPECTED_PROFILES):
        raise EvidenceError(f"capture profile set mismatch: {sorted(profiles)!r}")
    report = derivation_report(captures)
    return {
        "schema": SCHEMA,
        "product_ground": PRODUCT_GROUND,
        "tool_sha256": tool_sha256(),
        "captures": sorted(captures, key=lambda item: item["profile"]),
        "derivation_report": report,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    capture_parser = sub.add_parser("capture")
    capture_parser.add_argument("--profile", required=True)
    combine_parser = sub.add_parser("combine")
    combine_parser.add_argument("inputs", nargs="+", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "capture":
            kind = sys.platform
            if kind.startswith("linux"):
                value = capture_linux(args.profile)
            elif kind == "darwin":
                value = capture_darwin(args.profile)
            else:
                raise EvidenceError(f"unsupported platform: {kind}")
            value = validate_partial(value)
        else:
            value = combine(args.inputs)
        sys.stdout.buffer.write(canonical_bytes(value))
        return 0
    except (EvidenceError, OSError, json.JSONDecodeError) as exc:
        print(f"service-runtime-reference: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
