#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Offline architecture, publication and complete-installer checks."""

import argparse
import base64
import contextlib
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import yaml

import winget as W


class WingetControls(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name) / "manifests"
        self.addCleanup(patch.stopall)
        patch.object(W, "DIRECTORY", self.directory).start()
        self.version = "3.4.5"
        self.args = argparse.Namespace(version=self.version, release_date="2026-10-22",
                                       with_arm64=True, allow_pending=False)
        self.output = io.StringIO()
        self.feed = patch.object(W, "published_version", return_value=self.version).start()
        self.download = patch.object(W, "download_digest", side_effect=self.installer).start()

    def installer(self, url):
        body = ("complete published installer " + url).encode()
        return hashlib.sha256(body).hexdigest().upper(), len(body)

    def prepare(self):
        with contextlib.redirect_stdout(self.output):
            W.prepare(self.args)
        return yaml.safe_load((self.directory / f"{W.PACKAGE}.installer.yaml").read_text())

    def check(self):
        with contextlib.redirect_stdout(self.output):
            return W.check(self.args)

    def test_default_keeps_only_the_x64_channel_and_installer(self):
        self.args.with_arm64 = False
        manifest = self.prepare()
        self.feed.assert_called_once_with("x64")
        self.assertEqual([row["Architecture"] for row in manifest["Installers"]], ["x64"])
        self.assertEqual(manifest["MinimumOSVersion"], "10.0.19045.0")
        self.assertEqual(self.download.call_args.args, (W.installer_url(self.version),))

    def test_same_version_dual_architecture_manifest_and_community_check(self):
        manifest = self.prepare()
        self.assertEqual([call.args[0] for call in self.feed.call_args_list], ["x64", "arm64"])
        self.assertEqual([row["Architecture"] for row in manifest["Installers"]], ["x64", "arm64"])
        self.assertEqual(manifest["Installers"][1]["MinimumOSVersion"], "10.0.22000.0")
        for row in manifest["Installers"]:
            self.assertEqual(row["InstallerSha256"], self.installer(row["InstallerUrl"])[0])
        merged = {"content": base64.b64encode(yaml.safe_dump(manifest).encode()).decode()}
        with patch.object(W, "github_json", return_value=merged):
            self.assertEqual(self.check(), 0)
        result = json.loads(self.output.getvalue().splitlines()[-1])
        self.assertEqual(result["state"], "CURRENT")
        self.assertEqual(set(result["installers"]), {"x64", "arm64"})

    def test_stale_arm_feed_refuses_before_download_or_manifest_write(self):
        self.feed.side_effect = [self.version, "3.4.4"]
        with self.assertRaisesRegex(ValueError, "arm64 feed carries"):
            self.prepare()
        self.download.assert_not_called()
        self.assertFalse(self.directory.exists())

    def test_check_cannot_ignore_an_arm_row_without_explicit_selection(self):
        self.prepare()
        self.args.with_arm64 = False
        self.download.reset_mock()
        with self.assertRaisesRegex(ValueError, "explicit selection"):
            self.check()
        self.download.assert_not_called()

    def test_wrong_arm_digest_refuses_before_community_status(self):
        manifest = self.prepare()
        manifest["Installers"][1]["InstallerSha256"] = "0" * 64
        (self.directory / f"{W.PACKAGE}.installer.yaml").write_text(yaml.safe_dump(manifest))
        with patch.object(W, "github_json") as github:
            with self.assertRaisesRegex(ValueError, "arm64 installer digest mismatch"):
                self.check()
            github.assert_not_called()

    def test_missing_duplicate_wrong_url_and_windows_10_arm_rows_refuse(self):
        original = self.prepare()
        for change in ("missing", "duplicate", "wrong-url", "windows-10"):
            with self.subTest(change=change):
                manifest = json.loads(json.dumps(original))
                if change == "missing":
                    manifest["Installers"].pop()
                elif change == "duplicate":
                    manifest["Installers"][1]["Architecture"] = "x64"
                elif change == "wrong-url":
                    manifest["Installers"][1]["InstallerUrl"] = W.installer_url(self.version)
                else:
                    manifest["Installers"][1]["MinimumOSVersion"] = "10.0.19045.0"
                (self.directory / f"{W.PACKAGE}.installer.yaml").write_text(yaml.safe_dump(manifest))
                with self.assertRaises(ValueError):
                    self.check()


class DownloadControls(unittest.TestCase):
    def response(self, body, length=None):
        response = io.BytesIO(body)
        response.headers = {} if length is None else {"Content-Length": str(length)}
        return response

    def test_complete_stream_is_hashed_and_truncation_refuses(self):
        body = b"start" + bytes(range(256)) * 5000 + b"different final bytes"
        with patch.object(W.HTTP, "open", return_value=self.response(body, len(body))):
            self.assertEqual(W.download_digest("https://updates.solstone.app/fixture"),
                             (hashlib.sha256(body).hexdigest().upper(), len(body)))
        with patch.object(W.HTTP, "open", return_value=self.response(body, len(body) + 1)):
            with self.assertRaisesRegex(ValueError, "incomplete installer"):
                W.download_digest("https://updates.solstone.app/fixture")

    def test_empty_and_excessive_installer_and_feed_refuse(self):
        with patch.object(W.HTTP, "open", return_value=self.response(b"")):
            with self.assertRaisesRegex(ValueError, "empty installer"):
                W.download_digest("https://updates.solstone.app/fixture")
        with patch.object(W.HTTP, "open", return_value=self.response(b"x", W.MAX_INSTALLER + 1)):
            with self.assertRaisesRegex(ValueError, "length exceeds"):
                W.download_digest("https://updates.solstone.app/fixture")
        with patch.object(W.HTTP, "open", return_value=self.response(b"x" * (W.MAX_FEED + 1))):
            with self.assertRaisesRegex(ValueError, "feed exceeds"):
                W.published_version("arm64")


if __name__ == "__main__":
    unittest.main()
