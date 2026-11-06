#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Build and run an observation derivative on a dedicated foreground iPhone."""

import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import subprocess
import time

from prepare_gpu_lifecycle import prepare


BUNDLE = "app.solstone.journal.runtime.dev"
MODES = ["positive", "positive", "proofs", "nonce", "evidence", "cancel", "positive", "deadline", "positive"]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def install_app(device, app, output):
    report = output / "install.json"
    with (output / "install.log").open("wb") as log:
        subprocess.run(["xcrun", "devicectl", "device", "install", "app", "--device", device,
                        "--json-output", str(report), str(app)],
                       stdout=log, stderr=subprocess.STDOUT, check=True)
    installed = json.loads(report.read_text()).get("result", {}).get("installedApplications", [])
    if BUNDLE not in [row.get("bundleID") for row in installed]:
        raise ValueError("installation did not confirm the built app identity")


def app_exited_zero(report):
    termination = json.loads(report.read_text()).get("result", {}).get("terminationResult", {})
    if "exitCode" not in termination:
        raise ValueError("devicectl launch report has no terminationResult.exitCode: its schema or this tool, not the app")
    return termination["exitCode"] == 0


def source_inputs(work):
    paths = set()
    for command in (["git", "ls-files", "-z"], ["git", "ls-files", "--others", "--exclude-standard", "-z"]):
        paths.update(value.decode() for value in subprocess.check_output(command, cwd=work).split(b"\0") if value)
    # Xcode source-directory globs also include ignored files.
    for folder in ["Host", "Shared", "Appraisal", "native/src", "appraisal-helper/src"]:
        paths.update(str(path.relative_to(work)) for path in (work / "tools/ios-runtime" / folder).rglob("*") if path.is_file())
    return {name: sha(work / name) for name in sorted(paths) if (work / name).is_file()}


def qualify(commit, work, build, fixture, device, output):
    source = Path(__file__).resolve().parents[2]
    work, build, output = work.resolve(), build.resolve(), output.resolve()
    if build.exists() or output.exists():
        raise ValueError("build and evidence destinations must be absent")
    destinations = [work, build, output]
    for destination in destinations:
        if destination == source or source in destination.parents:
            raise ValueError("outputs must be outside the committed source tree")
        for other in destinations:
            if destination != other and (destination in other.parents or other in destination.parents):
                raise ValueError("source, build and evidence outputs must be separate directories")
    if len(set(destinations)) != 3:
        raise ValueError("source, build and evidence outputs must be separate directories")
    output.mkdir(mode=0o700)
    result = {"started": time.time(), "passed": False}
    try:
        record = prepare(source, commit, work, fixture)
        expected = source_inputs(work)
        result["source"] = record
        result["source_inputs"] = expected
        with (output / "build.log").open("wb") as log:
            run = subprocess.run([
                "make", "-C", str(work / "tools/ios-runtime"), "build", f"BUILD_ROOT={build}",
            ], stdout=log, stderr=subprocess.STDOUT)
        result["build_exit"] = run.returncode
        if run.returncode:
            raise ValueError("diagnostic build failed; see build.log")
        if source_inputs(work) != expected:
            raise ValueError("diagnostic build inputs changed during build")
        app = build / "derived/Build/Products/Debug-iphoneos/JournalRuntimeDev.app"
        info = plistlib.loads((app / "Info.plist").read_bytes())
        if info.get("CFBundleIdentifier") != BUNDLE:
            raise ValueError("built diagnostic app has the wrong bundle ID")
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        files = []
        for path in sorted(app.rglob("*")):
            if path.is_symlink():
                raise ValueError("unexpected app symlink")
            if path.is_file():
                files.append({"path": str(path.relative_to(app)), "bytes": path.stat().st_size, "sha256": sha(path)})
        result["app"] = files
        install_app(device, app, output)
        subprocess.run([
            "xcrun", "devicectl", "device", "copy", "to", "--device", device,
            "--domain-type", "appDataContainer", "--domain-identifier", BUNDLE,
            "--source", str(work / "verifier-fixture.json"), "--destination", "Documents/verifier-fixture.json",
        ], check=True)
        console, launch = output / "phone.log", output / "launch.json"
        with console.open("wb") as log:
            run = subprocess.run([
                "xcrun", "devicectl", "device", "process", "launch", "--device", device,
                "--console", "--terminate-existing", "--json-output", str(launch), BUNDLE, "--verifier-lifecycle",
            ], stdout=log, stderr=subprocess.STDOUT, timeout=180)
        result["launcher_exit"] = run.returncode
        text = console.read_text(errors="replace")
        rows = []
        for line in text.splitlines():
            if line.startswith("{"):
                try:
                    rows.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
        cases = [row for row in rows if row.get("event") == "verifier.lifecycle"]
        final = [row for row in rows if row.get("event") == "verifier.lifecycle.finished"]
        result["cases"] = cases
        if run.returncode or not app_exited_zero(launch):
            raise ValueError("physical app did not exit successfully")
        if len(final) != 1 or final[0].get("value", {}).get("passed") is not True:
            raise ValueError("physical qualification did not complete")
        if [row["value"]["mode"] for row in cases] != MODES:
            raise ValueError("physical control sequence is incomplete")
        if len({row["value"]["pid"] for row in cases}) != len(MODES):
            raise ValueError("physical helper PID reused")
        result["passed"] = True
    except Exception as error:
        result["error"] = str(error)
        raise
    finally:
        result["finished"] = time.time()
        (output / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--build", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--device", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = qualify(args.commit, args.work, args.build, args.fixture, args.device, args.output)
    print(json.dumps({"passed": result["passed"], "cases": len(result["cases"]), "output": str(args.output)}))


if __name__ == "__main__":
    main()
