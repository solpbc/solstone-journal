#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture raw historical service-generator evidence from git blobs.

This is a hand-run evidence-capture tool, not a CI dependency.  It extracts
only each generator's AST closure from historical source read through git,
then captures both platform artifacts under the pinned interpreter.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

from extract_service_legacy_generator_closure import ClosureExtractor, stdlib_modules

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE_ROOT = Path(
    os.environ.get(
        "SERVICE_LEGACY_EVIDENCE_ROOT",
        ROOT / "core/fixtures/service_legacy_evidence",
    )
).resolve()
CENSUS_PATH = EVIDENCE_ROOT / "follow-census.json"
INTERPRETERS_PATH = EVIDENCE_ROOT / "interpreters.json"
PYTHON_CACHE_ROOT = Path(
    os.environ.get(
        "SERVICE_LEGACY_PYTHON_CACHE_ROOT",
        ROOT / ".cache/service-legacy-evidence/python",
    )
).resolve()
SYNTHETIC_SANDBOX_ROOT = Path("/opt/solstone-service-legacy-evidence")
SCHEMA = "service-legacy-raw-evidence"
SCHEMA_VERSION = 1
OPTIONAL_KEYS = {
    "ANTHROPIC_API_KEY": "__SERVICE_LEGACY_DUMMY_ANTHROPIC__",
    "OPENAI_API_KEY": "__SERVICE_LEGACY_DUMMY_OPENAI__",
    "GOOGLE_API_KEY": "__SERVICE_LEGACY_DUMMY_GOOGLE__",
    "REVAI_ACCESS_TOKEN": "__SERVICE_LEGACY_DUMMY_REVAI__",
    "PLAUD_ACCESS_TOKEN": "__SERVICE_LEGACY_DUMMY_PLAUD__",
}


class CaptureError(RuntimeError):
    """A historical source or generator could not be captured safely."""


@dataclass(frozen=True)
class Profile:
    name: str
    port: int
    path: str
    optional_keys: bool


BASE_PROFILES = (
    Profile("default", 5015, "/usr/bin:/bin", False),
    Profile("spaces_nonascii", 5015, "/usr/bin:/bin", False),
    Profile("alt_port_path", 5815, "/bin:/opt/service-legacy-alt/bin:/usr/bin", False),
)
KEY_PROFILES = (
    Profile("keys_present", 5015, "/usr/bin:/bin", True),
    Profile("keys_absent", 5015, "/usr/bin:/bin", False),
)


def materialize_file(commit: str, source_path: str, root: Path) -> bool:
    result = subprocess.run(
        ["git", "show", f"{commit}:{source_path}"],
        cwd=ROOT,
        check=False,
        capture_output=True,
    )
    if result.returncode:
        return False
    destination = root / source_path
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(result.stdout)
    return True


def materialize_closure_sources(entry: dict[str, object], source_root: Path) -> Path:
    shutil.rmtree(source_root, ignore_errors=True)
    source_root.mkdir(parents=True)
    commit = str(entry["commit"])
    service_path = str(entry["path"])
    if not materialize_file(commit, service_path, source_root):
        raise CaptureError(f"missing historical service source {commit}:{service_path}")
    prefix = Path(service_path).parent
    # These are the only first-party modules established by the API survey as
    # reachable from the generator closure.  Missing candidates are normal.
    for name in ("utils.py", "install_guard.py"):
        materialize_file(commit, (prefix / name).as_posix(), source_root)
    return source_root / service_path


def profiles_for(index: int) -> tuple[Profile, ...]:
    return BASE_PROFILES + (KEY_PROFILES if index <= 18 else ())


def bucket_for(index: int) -> str:
    # This is the source-review compatibility division, not an API-era branch.
    return "cpython37" if index <= 25 else "cpython39"


def interpreter_paths() -> dict[str, Path]:
    manifest = json.loads(INTERPRETERS_PATH.read_text(encoding="utf-8"))
    paths: dict[str, Path] = {}
    for bucket, declaration in manifest["buckets"].items():
        executable = PYTHON_CACHE_ROOT / bucket / declaration["executable"]
        if not executable.is_file():
            raise CaptureError(f"pinned interpreter is missing: {executable}")
        digest = hashlib.sha256(executable.read_bytes()).hexdigest()
        if digest != declaration["executable_sha256"]:
            raise CaptureError(f"pinned interpreter hash mismatch: {executable}")
        paths[bucket] = executable
    return paths


def profile_values(blob: str, profile: Profile) -> tuple[Path, Path]:
    base = SYNTHETIC_SANDBOX_ROOT / blob / profile.name
    if profile.name == "spaces_nonascii":
        return base / "home space café", base / "journal space café"
    return base / "home", base / "journal"


def synthetic_executable(bucket: str) -> str:
    if bucket not in {"cpython37", "cpython39"}:
        raise CaptureError(f"unknown interpreter bucket: {bucket}")
    executable = SYNTHETIC_SANDBOX_ROOT / "interpreters" / bucket / "bin" / "python"
    if executable.name != "python":
        raise CaptureError(f"synthetic interpreter basename changed: {executable}")
    return str(executable)


def service_environment(
    index: int, home: Path, path: str, journal: Path, profile: Profile
) -> dict[str, str]:
    """Build the historical _collect_env-equivalent service dictionary."""
    environment = {"HOME": str(home), "PATH": path}
    # Indices 0-4 always put the override in _collect_env. Indices 5-8 do
    # so only when the process has it, which this capture intentionally does.
    if index <= 8:
        environment["_SOLSTONE_JOURNAL_OVERRIDE"] = str(journal)
    if index >= 19:
        environment["PYTHONUNBUFFERED"] = "1"
    if profile.optional_keys:
        environment.update(OPTIONAL_KEYS)
    return environment


def capture_environment(
    home: Path, path: str, journal: Path, index: int
) -> dict[str, str]:
    environment = {
        "HOME": str(home),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "PATH": path,
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONHASHSEED": "0",
        "PYTHONNOUSERSITE": "1",
        "TZ": "UTC",
    }
    if 1 <= index <= 10:
        environment["_SOLSTONE_JOURNAL_OVERRIDE"] = str(journal)
    elif 11 <= index <= 12:
        environment["SOLSTONE_JOURNAL"] = str(journal)
    return environment


CHILD_PROGRAM = r"""
import base64
import hashlib
import inspect
import json
import os
import sys
from pathlib import Path

source_path, request_path, result_path = sys.argv[1:]
request = json.loads(open(request_path, encoding="utf-8").read())
if not request["synthetic_executable"].endswith("/bin/python"):
    raise ValueError("synthetic executable must end in /bin/python")
sys.executable = request["synthetic_executable"]
namespace = {"__file__": source_path, "__name__": "service_legacy_synthetic"}
source = open(source_path, encoding="utf-8").read()
exec(compile(source, source_path, "exec"), namespace)

def call(name):
    function = namespace[name]
    parameters = inspect.signature(function).parameters
    kwargs = {"env": request["env"]}
    if "port" in parameters:
        kwargs["port"] = request["port"]
    if "journal_path" in parameters:
        kwargs["journal_path"] = request["journal_path"]
    return function(**kwargs), str(inspect.signature(function))

original_makedirs = os.makedirs
original_path_mkdir = Path.mkdir
try:
    os.makedirs = lambda *_args, **_kwargs: None
    Path.mkdir = lambda *_args, **_kwargs: None
    plist, plist_signature = call("_generate_plist")
    unit, unit_signature = call("_generate_systemd_unit")
finally:
    os.makedirs = original_makedirs
    Path.mkdir = original_path_mkdir
if not isinstance(plist, bytes):
    raise TypeError("_generate_plist did not return bytes")
if not isinstance(unit, str):
    raise TypeError("_generate_systemd_unit did not return str")
raw = plist + b"\0" + unit.encode("utf-8")
payload = {
    "plist_base64": base64.b64encode(plist).decode("ascii"),
    "plist_signature": plist_signature,
    "raw_sha256": hashlib.sha256(raw).hexdigest(),
    "systemd_signature": unit_signature,
    "systemd_unit": unit,
}
open(result_path, "w", encoding="utf-8").write(
    json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
)
"""


def capture_profile(
    *,
    entry: dict[str, object],
    source: str,
    python: Path,
    profile: Profile,
    work_root: Path,
) -> dict[str, object]:
    blob = str(entry["blob"])
    home, journal = profile_values(blob, profile)
    runtime = work_root / "runtime" / blob / profile.name
    shutil.rmtree(runtime, ignore_errors=True)
    runtime.mkdir(parents=True)
    source_file = runtime / "synthetic_service.py"
    request_file = runtime / "request.json"
    result_file = runtime / "result.json"
    source_file.write_text(source, encoding="utf-8")
    index = int(entry["index"])
    bucket = bucket_for(index)
    environment_values = service_environment(
        index, home, profile.path, journal, profile
    )
    request_file.write_text(
        json.dumps(
            {
                "env": environment_values,
                "journal_path": str(journal),
                "port": profile.port,
                "synthetic_executable": synthetic_executable(bucket),
            },
            indent=2,
            sort_keys=True,
            ensure_ascii=False,
        )
        + "\n",
        encoding="utf-8",
    )
    subprocess.run(
        [
            str(python),
            "-c",
            CHILD_PROGRAM,
            str(source_file),
            str(request_file),
            str(result_file),
        ],
        check=True,
        cwd=work_root,
        env=capture_environment(home, profile.path, journal, index),
    )
    return json.loads(result_file.read_text(encoding="utf-8"))


def verify_selected_interpreter(python: Path, source: str) -> None:
    result = subprocess.run(
        [
            str(python),
            "-c",
            "import sys; compile(sys.stdin.read(), '<synthetic>', 'exec')",
        ],
        check=False,
        input=source,
        text=True,
        capture_output=True,
    )
    if result.returncode:
        raise CaptureError(
            f"synthetic closure does not compile under {python}: {result.stderr.strip()}"
        )


def fixture_payload(
    entry: dict[str, object],
    bucket: str,
    platform: str,
    profile: Profile,
    capture: dict[str, object],
) -> dict[str, object]:
    return {
        "blob": entry["blob"],
        "commit": entry["commit"],
        "inputs": {
            "env": capture["input_env"],
            "journal_path": capture["journal_path"],
            "port": capture["port"],
        },
        "interpreter_bucket": bucket,
        "path": entry["path"],
        "platform": platform,
        "profile": profile.name,
        "raw": {
            "plist_base64": capture["plist_base64"],
            "sha256": capture["raw_sha256"],
            "systemd_unit": capture["systemd_unit"],
        },
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
    }


def write_json(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def capture_all(output_root: Path, scratch_root: Path) -> int:
    if output_root.exists():
        raise CaptureError(f"output root already exists: {output_root}")
    entries = json.loads(CENSUS_PATH.read_text(encoding="utf-8"))["entries"]
    if len(entries) != 44:
        raise CaptureError(f"expected 44 follow-census entries, found {len(entries)}")
    interpreters = interpreter_paths()
    stdlib_by_bucket = {
        bucket: stdlib_modules(python) for bucket, python in interpreters.items()
    }
    scratch_root.mkdir(parents=True, exist_ok=True)
    fixture_count = 0
    for entry in entries:
        index = int(entry["index"])
        blob = str(entry["blob"])
        bucket = bucket_for(index)
        python = interpreters[bucket]
        service_path = materialize_closure_sources(
            entry, scratch_root / "sources" / blob
        )
        extractor = ClosureExtractor(
            service_path.parents[len(Path(str(entry["path"])).parts) - 1],
            service_path,
            stdlib_by_bucket[bucket],
        )
        source, _provenance = extractor.extract()
        verify_selected_interpreter(python, source)
        for profile in profiles_for(index):
            capture = capture_profile(
                entry=entry,
                source=source,
                python=python,
                profile=profile,
                work_root=scratch_root,
            )
            home, journal = profile_values(blob, profile)
            capture["input_env"] = service_environment(
                index, home, profile.path, journal, profile
            )
            capture["journal_path"] = str(journal)
            capture["port"] = profile.port
            for platform in ("linux", "macos"):
                write_json(
                    output_root / blob / platform / f"{profile.name}.json",
                    fixture_payload(entry, bucket, platform, profile, capture),
                )
                fixture_count += 1
        print(f"captured {index:02d} {blob} ({bucket})", file=sys.stderr)
    return fixture_count


def self_test() -> None:
    source = """
import plistlib
import sys
def _generate_plist(env):
    return plistlib.dumps({'EnvironmentVariables': env, 'ProgramArguments': [sys.executable]})
def _generate_systemd_unit(env):
    return '[Service]\\nExecStart=' + sys.executable + '\\n' + ''.join('Environment=' + key + '=' + value + '\\n' for key, value in env.items())
"""
    real = sys.executable
    if not real.startswith("/"):
        raise AssertionError("positive-control interpreter path is not absolute")
    with tempfile.TemporaryDirectory(
        prefix="service-legacy-synthetic-test-"
    ) as temporary:
        captured = capture_profile(
            entry={"blob": "0" * 40, "index": 0},
            source=source,
            python=Path(real),
            profile=BASE_PROFILES[0],
            work_root=Path(temporary),
        )
    plist = plistlib.loads(base64.b64decode(captured["plist_base64"]))
    expected = synthetic_executable("cpython37")
    if plist["ProgramArguments"] != [expected]:
        raise AssertionError(
            "historical closure did not observe synthetic sys.executable"
        )
    if f"ExecStart={expected}" not in captured["systemd_unit"]:
        raise AssertionError("systemd closure did not observe synthetic sys.executable")
    if real == expected:
        raise AssertionError(
            "synthetic identity control cannot distinguish the real runner"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-root", type=Path, help="new raw-fixture directory")
    parser.add_argument(
        "--scratch-root", type=Path, help="throwaway source/materialization directory"
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        print("service-legacy synthetic interpreter self-test passed")
        return 0
    if args.output_root is None or args.scratch_root is None:
        parser.error("--output-root and --scratch-root are required")
    count = capture_all(args.output_root.resolve(), args.scratch_root.resolve())
    print(f"wrote {count} raw fixtures", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
