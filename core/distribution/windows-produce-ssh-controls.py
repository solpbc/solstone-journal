#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Local transport fault controls; no network or Windows execution."""

import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time


def main():
    if len(sys.argv) != 2:
        raise ValueError("fresh fixture directory argument required")
    root = Path(sys.argv[1]).absolute()
    root.mkdir()
    source = Path(__file__).with_name("windows-produce-ssh")
    tools = root / "tools"
    tools.mkdir()
    timeout = shutil.which("timeout")
    if timeout is None:
        raise ValueError("GNU timeout required")
    # The exact wrapper is invoked. Only its SSH peer is substituted locally;
    # timeout controls use the real timeout executable with a shorter duration.
    ssh = tools / "ssh"
    ssh.write_text("""#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path
Path(os.environ['CONTROL_SSH_ARGS']).write_text(json.dumps(sys.argv[1:]))
if os.environ['CONTROL_MODE'] == 'receipt-failure':
    Path(os.environ['CONTROL_RECEIPT_COLLISION']).write_text('original occupied leaf')
os.write(1, bytes([0, 255, 13, 10]))
os.write(2, b'fixture stderr\\r\\n')
if os.environ['CONTROL_MODE'] == 'timeout':
    time.sleep(5)
sys.exit(int(os.environ['CONTROL_EXIT']))
""")
    ssh.chmod(0o755)
    adapter = tools / "timeout"
    adapter.write_text("""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args=sys.argv[1:]
Path(os.environ['CONTROL_TIMEOUT_ARGS']).write_text(json.dumps(args))
if os.environ['CONTROL_MODE'] == 'timeout':
    assert args[3] == '150s'
    args[3] = '0.2s'
os.execv(os.environ['CONTROL_REAL_TIMEOUT'], ['timeout', *args])
""")
    adapter.chmod(0o755)
    digest = "a" * 64
    remote_driver = "C:\\fixture\\driver's literal.ps1"
    remote_parameters = "C:\\fixture\\parameters.json"
    cases = []
    specifications = [
        ("success", "normal", 0, 0, {}),
        ("native-nonzero", "normal", 17, 17, {}),
        ("transport-failure", "normal", 255, 255, {}),
        ("receipt-failure", "receipt-failure", 17, 1, {}),
        ("timeout", "timeout", 0, 124, {}),
        ("option-host", "normal", 0, 2, {0: "-oProxyCommand=bad"}),
        ("leading-zero-budget", "normal", 0, 2, {5: "030"}),
        ("oversized-budget", "normal", 0, 2, {5: "999999999999999999999"}),
        ("insufficient-margin", "normal", 0, 2, {6: "149"}),
        ("invalid-path-newline", "normal", 0, 1, {1: "C:\\bad\npath"}),
    ]
    failure = None
    try:
        for name, mode, child_exit, expected_exit, changes in specifications:
            case_root = root / name
            case_root.mkdir()
            evidence = case_root / "evidence"
            argv = ["fixture-host", remote_driver, digest, remote_parameters,
                    digest, "30", "150", str(evidence)]
            for index, value in changes.items():
                argv[index] = value
            environment = os.environ.copy()
            environment.update({
                "PATH": str(tools) + os.pathsep + environment["PATH"],
                "CONTROL_REAL_TIMEOUT": timeout,
                "CONTROL_MODE": mode,
                "CONTROL_EXIT": str(child_exit),
                "CONTROL_RECEIPT_COLLISION": str(evidence / "transport.json"),
                "CONTROL_SSH_ARGS": str(case_root / "ssh-args.json"),
                "CONTROL_TIMEOUT_ARGS": str(case_root / "timeout-args.json"),
            })
            started = time.monotonic()
            with (case_root / "stdout").open("xb") as stdout, (case_root / "stderr").open("xb") as stderr:
                result = subprocess.run(["bash", str(source), *argv], env=environment,
                                        stdout=stdout, stderr=stderr, timeout=10, check=False)
            elapsed = time.monotonic() - started
            record = {"name": name, "actual_exit": result.returncode,
                      "expected_exit": expected_exit, "elapsed_seconds": elapsed}
            cases.append(record)
            assert result.returncode == expected_exit, record
            if changes:
                assert not (case_root / "ssh-args.json").exists(), name
                continue
            if mode == "receipt-failure":
                assert (evidence / "transport.exit").read_text() == "17\n"
                assert (evidence / "transport.json").read_text() == "original occupied leaf"
                assert b"transport exit=17; transport receipt failed=1" in (case_root / "stderr").read_bytes()
                continue
            receipt = json.loads((evidence / "transport.json").read_text())
            assert receipt["timeout_transport_exit"] == expected_exit, receipt
            assert receipt["remote_completion_uncertain"] == (expected_exit != 0)
            assert receipt["remote_cleanup_authorized"] is False
            assert (evidence / "transport.exit").read_text() == f"{expected_exit}\n"
            assert (evidence / "ssh.stdout").read_bytes() == bytes([0, 255, 13, 10])
            for member, expected_hash in receipt["snapshots"].items():
                assert hashlib.sha256((evidence / member).read_bytes()).hexdigest() == expected_hash
            timeout_args = json.loads((case_root / "timeout-args.json").read_text())
            assert timeout_args[:5] == ["--foreground", "--signal=TERM", "--kill-after=10s", "150s", "ssh"]
            ssh_args = json.loads((case_root / "ssh-args.json").read_text())
            assert ssh_args[:10] == ["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=15",
                                     "-o", "ControlMaster=no", "-o", "ControlPath=none", "fixture-host"]
            encoded = (evidence / "command.base64").read_text().strip()
            assert ssh_args[-1] == encoded
            command = base64.b64decode(encoded, validate=True).decode("utf-16-le")
            assert "$driver='C:\\fixture\\driver''s literal.ps1'" in command
            assert "& $driver @arguments\nexit $LASTEXITCODE" in command
            assert elapsed < 3 if mode == "timeout" else elapsed < 10
    except BaseException as error:
        failure = repr(error)
        raise
    finally:
        report = {
            "scope": "Exact Bash transport with local SSH substitute. Real GNU timeout with only duration shortened for timeout control. No remote PowerShell parser, native producer, network, fence, or cleanup proof.",
            "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
            "controls_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "cases": cases, "failure": failure,
        }
        (root / "controls.json").write_text(json.dumps(report, indent=2) + "\n")
    print("WINDOWS_PRODUCER_SSH_CONTROLS_PASS")


if __name__ == "__main__":
    main()
