# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from tools.journal_device_sim.field_manifest import build_field_manifest
from tools.journal_device_sim.manifest import ManifestError


class FieldManifestTests(unittest.TestCase):
    def test_generator_uses_only_tracked_closed_raw_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            audio_dir = root / "journal/20260201/field.audio/080000_600"
            screen_dir = root / "journal/20260201/field.screen/090000_60"
            audio_dir.mkdir(parents=True)
            screen_dir.mkdir(parents=True)
            audio = audio_dir / "audio.wav"
            with audio.open("wb") as handle:
                handle.write(b"RIFF")
                handle.seek(19_200_077)
                handle.write(b"\x00")
            (audio_dir / "stream.json").write_text("{}\n", encoding="utf-8")
            (audio_dir / "runtime.log").write_text("ignored\n", encoding="utf-8")
            (screen_dir / "screen.mp4").write_bytes(b"synthetic-mp4")
            (screen_dir / "stream.json").write_text("{}\n", encoding="utf-8")
            (root / "manifest.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "built": "test",
                        "segments": [
                            {
                                "day": "20260201",
                                "stream": "field.audio",
                                "segment": "080000_600",
                                "source": "chime6",
                                "source_id": "S01",
                                "license": "test-only",
                                "exercises": ["transcription"],
                            },
                            {
                                "day": "20260201",
                                "stream": "field.screen",
                                "segment": "090000_60",
                                "source": "screen-source",
                                "source_id": "screen-1",
                                "license": "test-only",
                                "exercises": ["screen"],
                            },
                        ],
                    }
                ),
                encoding="utf-8",
            )
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(root),
                    "config",
                    "user.email",
                    "test@example.invalid",
                ],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(root), "config", "user.name", "test"], check=True
            )
            subprocess.run(
                ["git", "-C", str(root), "add", "manifest.json", "journal"], check=True
            )
            subprocess.run(
                ["git", "-C", str(root), "commit", "-qm", "fixtures"], check=True
            )
            manifest = build_field_manifest(root)
            self.assertEqual(len(manifest["segments"]), 2)
            self.assertEqual(
                [item["submitted"] for item in manifest["segments"][0]["files"]],
                ["audio.wav"],
            )
            serialized = json.dumps(manifest)
            self.assertNotIn("stream.json", serialized)
            self.assertNotIn("runtime.log", serialized)
            self.assertEqual(
                manifest["profiles"]["field-large"]["segments"],
                ["20260201-audio-080000_600-chime6"],
            )
            self.assertFalse(manifest["profiles"]["field-large"]["verify_processing"])
            self.assertTrue(manifest["profiles"]["field-smoke"]["verify_processing"])

            audio.write_bytes(b"changed after commit")
            with self.assertRaisesRegex(ManifestError, "differ from HEAD"):
                build_field_manifest(root)


if __name__ == "__main__":
    unittest.main()
