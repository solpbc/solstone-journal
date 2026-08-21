# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

from tools.journal_device_sim.__main__ import main


class CliTests(unittest.TestCase):
    def test_validate_reports_explicit_verification_scope(self) -> None:
        manifest = (
            Path(__file__).resolve().parents[1] / "fixtures" / "smoke" / "manifest.json"
        )
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            status = main(["validate", "--manifest", str(manifest)])
        self.assertEqual(status, 0)
        payload = json.loads(stdout.getvalue())
        self.assertEqual(payload["profiles"]["smoke"]["verification"], "contract")
        self.assertNotIn("verify_processing", payload["profiles"]["smoke"])

    def test_paired_run_requires_explicit_state_directory(self) -> None:
        manifest = (
            Path(__file__).resolve().parents[1] / "fixtures" / "smoke" / "manifest.json"
        )
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            status = main(
                [
                    "run",
                    "--manifest",
                    str(manifest),
                    "--profile",
                    "smoke",
                    "--carrier",
                    "relay",
                    "--paired",
                ]
            )
        self.assertEqual(status, 1)
        self.assertEqual(
            stderr.getvalue(),
            "configuration error: --paired requires an explicit --state-dir\n",
        )

    def test_pair_and_run_reject_a_symlinked_state_directory(self) -> None:
        manifest = (
            Path(__file__).resolve().parents[1] / "fixtures" / "smoke" / "manifest.json"
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target"
            target.mkdir()
            linked = root / "linked"
            linked.symlink_to(target, target_is_directory=True)
            commands = (
                [
                    "pair",
                    "--pair-code",
                    "pair-code",
                    "--state-dir",
                    str(linked),
                    "--solstone-bin",
                    "unused",
                ],
                [
                    "run",
                    "--manifest",
                    str(manifest),
                    "--profile",
                    "smoke",
                    "--carrier",
                    "relay",
                    "--paired",
                    "--state-dir",
                    str(linked),
                ],
            )
            for command in commands:
                with self.subTest(command=command[0]):
                    stderr = io.StringIO()
                    with contextlib.redirect_stderr(stderr):
                        status = main(command)
                    self.assertEqual(status, 1)
                    self.assertIn("plain director", stderr.getvalue())

    def test_run_normalizes_a_cyclic_state_symlink(self) -> None:
        manifest = (
            Path(__file__).resolve().parents[1] / "fixtures" / "smoke" / "manifest.json"
        )
        with tempfile.TemporaryDirectory() as temporary:
            linked = Path(temporary) / "loop"
            linked.symlink_to(linked.name)
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                status = main(
                    [
                        "run",
                        "--manifest",
                        str(manifest),
                        "--profile",
                        "smoke",
                        "--carrier",
                        "relay",
                        "--paired",
                        "--state-dir",
                        str(linked),
                    ]
                )
            self.assertEqual(status, 1)
            self.assertEqual(
                stderr.getvalue(),
                "configuration error: state directory must be a plain directory\n",
            )

    def test_pair_and_run_reject_nonfinite_timeouts(self) -> None:
        manifest = (
            Path(__file__).resolve().parents[1] / "fixtures" / "smoke" / "manifest.json"
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            commands = (
                (
                    "request",
                    [
                        "pair",
                        "--pair-code",
                        "pair-code",
                        "--state-dir",
                        str(root / "pair"),
                        "--request-timeout",
                        "nan",
                    ],
                ),
                (
                    "processing",
                    [
                        "run",
                        "--manifest",
                        str(manifest),
                        "--profile",
                        "smoke",
                        "--carrier",
                        "relay",
                        "--pair-code",
                        "pair-code",
                        "--state-dir",
                        str(root / "processing"),
                        "--processing-timeout",
                        "inf",
                    ],
                ),
                (
                    "poll",
                    [
                        "run",
                        "--manifest",
                        str(manifest),
                        "--profile",
                        "smoke",
                        "--carrier",
                        "relay",
                        "--pair-code",
                        "pair-code",
                        "--state-dir",
                        str(root / "poll"),
                        "--poll-interval",
                        "nan",
                    ],
                ),
            )
            for name, command in commands:
                with self.subTest(name=name):
                    stderr = io.StringIO()
                    with contextlib.redirect_stderr(stderr):
                        status = main(command)
                    self.assertEqual(status, 1)
                    self.assertIn("finite", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
