# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.local_thinking_install.harness import cleanup_namespace, run_scenario


class CleanupSafety(unittest.TestCase):
    def test_refused_admission_does_not_remove_namespace(self):
        with tempfile.TemporaryDirectory() as root:
            case = Path(root) / "case"
            source = case / "source-root"
            ns = Path(root) / "namespaces" / ("a" * 64)
            ns.mkdir(parents=True)
            sentinel = ns / "sentinel"
            sentinel.write_text("preexisting")
            with patch("tools.local_thinking_install.harness.setup_disposable_source_root", return_value=source), patch(
                "tools.local_thinking_install.harness.helper_admit",
                return_value={"ok": False, "error": "namespace_exists", "namespace_path": str(ns)},
            ):
                result = run_scenario(Path(root), case, "fresh", Path(root), Path(root)/"helper")
            self.assertEqual(result.reason_code, "admission_failed")
            self.assertEqual(sentinel.read_text(), "preexisting")

    def test_cleanup_rejects_unowned_path(self):
        with tempfile.TemporaryDirectory() as root:
            source = Path(root)
            self.assertIsNotNone(cleanup_namespace({"ok": False}, source))
            self.assertTrue(source.exists())

    def test_cleanup_only_removes_admitted_namespace_and_retains_receipt(self):
        with tempfile.TemporaryDirectory() as root:
            source = Path(root) / "case" / "source-root"
            source.mkdir(parents=True)
            ns = Path(root) / "namespaces" / ("a" * 64)
            ns.mkdir(parents=True)
            (ns / "record").write_text("fixture")
            admission = {"ok": True, "root": str(source.resolve()), "namespace_path": str(ns), "namespace_hex": ns.name}
            self.assertIsNone(cleanup_namespace(admission, source))
            self.assertFalse(ns.exists())
            self.assertEqual((source.parent / "identity-before-cleanup/record").read_text(), "fixture")
