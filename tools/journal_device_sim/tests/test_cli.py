# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

from tools.journal_device_sim.__main__ import _parser, main


class CliTests(unittest.TestCase):
    def test_pair_and_run_default_to_the_source_built_launcher(self) -> None:
        parser = _parser()
        pair = parser.parse_args(
            ["pair", "--pair-code", "pair-code", "--state-dir", "state"]
        )
        run = parser.parse_args(
            [
                "run",
                "--manifest",
                "manifest.json",
                "--profile",
                "smoke",
                "--carrier",
                "direct",
                "--bridge-url",
                "http://127.0.0.1:43127",
            ]
        )
        self.assertIsNone(pair.solstone_bin)
        self.assertIsNone(run.solstone_bin)
        for command in ("pair", "run"):
            with self.subTest(command=command):
                stdout = io.StringIO()
                with contextlib.redirect_stdout(stdout), self.assertRaises(
                    SystemExit
                ) as exit:
                    parser.parse_args([command, "--help"])
                self.assertEqual(exit.exception.code, 0)
                help_text = " ".join(stdout.getvalue().split())
                self.assertIn("source-built core/target/debug/solstone", help_text)
                self.assertIn("bare/relative paths are rejected", help_text)

    def test_pair_rejects_relative_and_bare_solstone_bin(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            for value in ("solstone", "./solstone"):
                with self.subTest(value=value):
                    stderr = io.StringIO()
                    with contextlib.redirect_stderr(stderr):
                        status = main(
                            [
                                "pair",
                                "--pair-code",
                                "pair-code",
                                "--state-dir",
                                str(Path(temporary) / value.replace("/", "_")),
                                "--solstone-bin",
                                value,
                            ]
                        )
                    self.assertEqual(status, 1)
                    self.assertIn("solstone_bin must be an absolute path", stderr.getvalue())

    def test_simulator_owned_run_reports_relative_solstone_bin_configuration(self) -> None:
        manifest = (
            Path(__file__).resolve().parents[1] / "fixtures" / "smoke" / "manifest.json"
        )
        with tempfile.TemporaryDirectory() as temporary:
            for value in ("solstone", "./solstone"):
                with self.subTest(value=value):
                    evidence = Path(temporary) / f"{value.replace('/', '_')}.json"
                    stdout = io.StringIO()
                    with contextlib.redirect_stdout(stdout):
                        status = main(
                            [
                                "run",
                                "--manifest",
                                str(manifest),
                                "--profile",
                                "smoke",
                                "--carrier",
                                "direct",
                                "--pair-code",
                                "pair-code",
                                "--state-dir",
                                str(Path(temporary) / value.replace("/", "_")),
                                "--evidence",
                                str(evidence),
                                "--solstone-bin",
                                value,
                            ]
                        )
                    self.assertEqual(status, 2)
                    self.assertIn("BLOCKED", stdout.getvalue())
                    payload = json.loads(evidence.read_text(encoding="utf-8"))
                    self.assertIn(
                        "solstone_bin must be an absolute path", payload["error"]
                    )

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
            native = root / "native-solstone"
            native.write_text(
                "#!/bin/sh\n"
                "case \"$1\" in\n"
                "  --help) printf '%s\\n' 'solstone - journal access CLI' ;;\n"
                "  --version) printf '%s\\n' 'solstone-test 3.1.4' ;;\n"
                "  *) exit 8 ;;\n"
                "esac\n",
                encoding="utf-8",
            )
            native.chmod(0o700)
            commands = (
                [
                    "pair",
                    "--pair-code",
                    "pair-code",
                    "--state-dir",
                    str(linked),
                    "--solstone-bin",
                    str(native),
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
