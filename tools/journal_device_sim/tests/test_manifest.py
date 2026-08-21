# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path

from tools.journal_device_sim.manifest import ManifestError, load_manifest
from tools.journal_device_sim.runner import build_day_map


class ManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.payload = self.root / "fixture.jsonl"
        self.payload.write_text('{"t":"fixture","ts":0}\n', encoding="utf-8")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _manifest(self, **file_overrides: object) -> Path:
        raw = self.payload.read_bytes()
        file_entry = {
            "path": self.payload.name,
            "submitted": "capture.jsonl",
            "size": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
            **file_overrides,
        }
        manifest = {
            "schema": "solstone.journal-device-sim.fixtures.v1",
            "profiles": {"smoke": {"segments": ["one"], "verify_duplicate": True}},
            "segments": [
                {
                    "id": "one",
                    "day": "20260201",
                    "segment": "080000_30",
                    "source": "tmux",
                    "files": [file_entry],
                }
            ],
        }
        path = self.root / "manifest.json"
        path.write_text(json.dumps(manifest), encoding="utf-8")
        return path

    def test_load_manifest_pins_every_file_byte(self) -> None:
        manifest = load_manifest(self._manifest())
        self.assertEqual(manifest.profile_segments("smoke")[0].files[0].size, 23)
        self.payload.write_text("changed\n", encoding="utf-8")
        with self.assertRaisesRegex(ManifestError, "is 23, but .* is 8 bytes"):
            load_manifest(manifest.path)

    def test_manifest_refuses_journal_authored_sidecar(self) -> None:
        with self.assertRaisesRegex(ManifestError, "journal-authored"):
            load_manifest(self._manifest(submitted="stream.json"))

    def test_manifest_accepts_utf8_names_but_refuses_control_and_dot_names(
        self,
    ) -> None:
        manifest = load_manifest(self._manifest(submitted="café.jsonl"))
        self.assertEqual(
            manifest.profile_segments("smoke")[0].files[0].submitted,
            "café.jsonl",
        )
        for submitted in ["..", "bad\tname.jsonl", 'bad"name.jsonl']:
            with self.subTest(submitted=submitted):
                with self.assertRaises(ManifestError):
                    load_manifest(self._manifest(submitted=submitted))

    def test_manifest_refuses_fixture_root_escape(self) -> None:
        outside = self.root.parent / f"{self.root.name}-outside.jsonl"
        outside.write_text("outside\n", encoding="utf-8")
        self.addCleanup(outside.unlink)
        relative = f"../{outside.name}"
        with self.assertRaisesRegex(ManifestError, "escapes the fixture root"):
            load_manifest(self._manifest(path=relative))

    @unittest.skipUnless(os.name == "posix", "symlink fixture requires POSIX")
    def test_manifest_refuses_a_symlinked_fixture(self) -> None:
        manifest_path = self._manifest()
        untracked = self.root / "untracked-secret.jsonl"
        untracked.write_bytes(self.payload.read_bytes())
        self.payload.unlink()
        self.payload.symlink_to(untracked.name)
        with self.assertRaisesRegex(ManifestError, "cannot contain symlinks"):
            load_manifest(manifest_path)

    def test_day_shift_preserves_offsets(self) -> None:
        path = self._manifest()
        value = json.loads(path.read_text(encoding="utf-8"))
        second = dict(value["segments"][0])
        second["id"] = "two"
        second["day"] = "20260203"
        value["segments"].append(second)
        value["profiles"]["smoke"]["segments"].append("two")
        path.write_text(json.dumps(value), encoding="utf-8")
        manifest = load_manifest(path)
        mapping = build_day_map(manifest.profile_segments("smoke"), "shift", "20260820")
        self.assertEqual(mapping, {"20260201": "20260818", "20260203": "20260820"})


if __name__ == "__main__":
    unittest.main()
