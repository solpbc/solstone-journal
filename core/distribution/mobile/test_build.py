# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Failure controls for the mobile build receipt and artifact boundary."""
import argparse
import contextlib
import concurrent.futures
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "scripts"))
import mobile_native_build as mobile


class BuildControls(unittest.TestCase):
    def test_running_receipt_cannot_report_success(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = argparse.Namespace(work_dir=root / "build", cache_dir=root / "cache",
                                      jobs=1, platform="android", components=["ced"], full_run=False)
            build = mobile.Build(args)
            build.run("control", [sys.executable, "-c", "pass"])
            receipt = json.loads((build.receipts / "mobile-native-build.json").read_text())
            self.assertEqual(receipt["status"], "running")
            self.assertIsNone(receipt["exit"])
            self.assertFalse(receipt["complete"])
            with self.assertRaises(FileExistsError):
                mobile.Build(args)

    def test_command_failure_reaches_process_and_terminal_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def fail(build):
                build.run("failed-control", [sys.executable, "-c", "raise SystemExit(7)"])
            argv = ["mobile-native-build", "--platform", "android", "--work-dir", str(root / "build"),
                    "--cache-dir", str(root / "cache"), "--components", "ced"]
            with mock.patch.object(sys, "argv", argv), mock.patch.object(mobile.Build, "execute", fail):
                with contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(mobile.main(), 7)
            receipt = json.loads((root / "build/receipts/mobile-native-build.json").read_text())
            self.assertEqual(receipt["status"], "failed")
            self.assertEqual(receipt["exit"], 7)
            self.assertFalse(receipt["complete"])
            self.assertEqual(receipt["steps"][-1]["exit"], 7)

    def test_mixed_or_missing_architecture_is_refused(self):
        mobile.require_android_architecture("  Machine: AArch64\n  Machine: AArch64\n")
        for text in ("", "Machine: Advanced Micro Devices X86-64\n",
                     "Machine: AArch64\nMachine: ARM\n"):
            with self.assertRaises(ValueError):
                mobile.require_android_architecture(text)
        mobile.require_ios_platform("platform 2\nminos 26.0\nplatform 2\nminos 13.0\n")
        mobile.require_ios_platform("platform 2\nminos 26.0\nLoad command 3\ncmd LC_SOURCE_VERSION\nversion 1267.0\n")
        for text in ("", "platform 2\nplatform 1\n", "platform 7\n", "platform 2\nminos 26.1\n"):
            with self.assertRaises(ValueError):
                mobile.require_ios_platform(text)

    def test_changed_cached_bytes_are_not_repaired_or_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "input.tar.gz"
            archive.write_bytes(b"verified input")
            pin = {"filename": archive.name, "url": "https://invalid.example/input", "sha256": mobile.sha256(archive)}
            self.assertEqual(mobile.acquire(pin, root), archive)
            archive.write_bytes(b"changed input")
            with self.assertRaisesRegex(ValueError, "input digest mismatch"):
                mobile.acquire(pin, root)

    def test_untracked_source_changes_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "-q", root], check=True)
            before = mobile.source_digest(root)
            (root / "recipe.py").write_text("new source\n")
            self.assertNotEqual(before, mobile.source_digest(root))

    def test_concurrent_acquisitions_share_only_verified_final_bytes(self):
        barrier = threading.Barrier(2, timeout=5)
        class Response(io.BytesIO):
            first = True
            def read(self, size=-1):
                if self.first:
                    self.first = False
                    barrier.wait()
                return super().read(size)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            payload = b"verified concurrent input"
            pin = {"filename": "input.tar.gz", "url": "https://invalid.example/input",
                   "sha256": mobile.hashlib.sha256(payload).hexdigest()}
            with mock.patch("android_cross_build.urllib.request.urlopen", side_effect=lambda *a, **k: Response(payload)):
                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as workers:
                    results = list(workers.map(lambda _: mobile.acquire(pin, root), range(2)))
            self.assertEqual(results, [root / pin["filename"]] * 2)
            self.assertEqual(results[0].read_bytes(), payload)
            self.assertEqual(list(root.iterdir()), [results[0]])


if __name__ == "__main__":
    unittest.main()
