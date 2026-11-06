#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from qualify_gpu_lifecycle import BUNDLE, app_exited_zero, install_app


# The fields the qualifier reads, trimmed from retained `--json-output` reports written by devicectl 518.33.
INSTALLED = {"info": {"outcome": "success"}, "result": {"installedApplications": [{"bundleID": BUNDLE}]}}
EXIT_ZERO = {"info": {"outcome": "success"},
             "result": {"terminationResult": {"exitCode": 0, "wasCoreDumpCreated": False}}}
EXIT_ONE = {"info": {"outcome": "success"},
            "result": {"terminationResult": {"exitCode": 1, "wasCoreDumpCreated": False}}}


class DeviceReports(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.output = Path(temporary.name)

    def install(self, report):
        def run(argv, stdout, **kwargs):
            stdout.write(b"text that devicectl may reword")
            (self.output / "install.json").write_text(json.dumps(report))
            self.argv = argv
        with patch("subprocess.run", run):
            install_app("fixture", self.output / "App.app", self.output)

    def test_install_trusts_the_report_not_the_text(self):
        self.install(INSTALLED)
        self.assertEqual(self.argv[self.argv.index("--json-output") + 1], str(self.output / "install.json"))

    def test_install_of_another_app_or_an_empty_report_refuses(self):
        other = {"result": {"installedApplications": [{"bundleID": BUNDLE + ".other"}]}}
        for report in (other, {"result": {}}, {}):
            with self.subTest(report=report), self.assertRaises(ValueError):
                self.install(report)

    def test_launch_exit_comes_from_the_report(self):
        report = self.output / "launch.json"
        for value, expected in ((EXIT_ZERO, True), (EXIT_ONE, False)):
            with self.subTest(value=value):
                report.write_text(json.dumps(value))
                self.assertEqual(app_exited_zero(report), expected)

    def test_launch_report_without_an_exit_code_blames_the_schema(self):
        report = self.output / "launch.json"
        for value in ({"result": {"terminationResult": {}}}, {"result": {}}, {}):
            with self.subTest(value=value):
                report.write_text(json.dumps(value))
                with self.assertRaisesRegex(ValueError, "not the app"):
                    app_exited_zero(report)


if __name__ == "__main__":
    unittest.main()
