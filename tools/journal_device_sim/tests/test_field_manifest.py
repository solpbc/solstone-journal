# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
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
            (audio_dir / "audio.flac").write_bytes(b"fLaCsynthetic")
            (audio_dir / "audio.mp3").write_bytes(b"ID3synthetic")
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
            self.assertEqual(len(manifest["segments"]), 4)
            self.assertEqual(
                [segment["files"][0]["submitted"] for segment in manifest["segments"]],
                ["audio.flac", "audio.mp3", "audio.wav", "screen.mp4"],
            )
            self.assertTrue(
                all(len(segment["files"]) == 1 for segment in manifest["segments"])
            )
            self.assertEqual(
                [segment["source"] for segment in manifest["segments"]],
                ["audio-flac", "audio-mp3", "audio-wav", "screen-mp4"],
            )
            self.assertEqual(
                [
                    segment["expect"]["processing"][0]
                    for segment in manifest["segments"]
                ],
                [
                    {
                        "input": "audio.flac",
                        "output": "audio.jsonl",
                        "handler": "transcribe",
                    },
                    {
                        "input": "audio.mp3",
                        "output": "audio.jsonl",
                        "handler": "transcribe",
                    },
                    {
                        "input": "audio.wav",
                        "output": "audio.jsonl",
                        "handler": "transcribe",
                    },
                    {
                        "input": "screen.mp4",
                        "output": "screen.jsonl",
                        "handler": "describe",
                    },
                ],
            )
            serialized = json.dumps(manifest)
            self.assertNotIn("stream.json", serialized)
            self.assertNotIn("runtime.log", serialized)
            self.assertEqual(
                manifest["profiles"]["field-large"]["segments"],
                ["20260201-audio-wav-080000_600-chime6"],
            )
            self.assertEqual(
                manifest["profiles"]["field-large"]["verification"],
                "custody",
            )
            self.assertEqual(
                manifest["profiles"]["field-smoke-processing"]["verification"],
                "processing",
            )
            self.assertEqual(
                set(manifest["profiles"]),
                {
                    "field-smoke-custody",
                    "field-smoke-processing",
                    "field-large",
                    "field-full-custody",
                    "field-full-processing",
                },
            )

            audio.write_bytes(b"changed after commit")
            with self.assertRaisesRegex(ManifestError, "differ from HEAD"):
                build_field_manifest(root)

            if os.name == "posix":
                with audio.open("wb") as handle:
                    handle.write(b"RIFF")
                    handle.seek(19_200_077)
                    handle.write(b"\x00")
                untracked = audio_dir / "untracked-secret.wav"
                untracked.write_bytes(audio.read_bytes())
                audio.unlink()
                audio.symlink_to(untracked.name)
                subprocess.run(
                    ["git", "-C", str(root), "add", audio.relative_to(root)],
                    check=True,
                )
                subprocess.run(
                    ["git", "-C", str(root), "commit", "-qm", "symlink fixture"],
                    check=True,
                )
                with self.assertRaisesRegex(ManifestError, "cannot be a symlink"):
                    build_field_manifest(root)


if __name__ == "__main__":
    unittest.main()
