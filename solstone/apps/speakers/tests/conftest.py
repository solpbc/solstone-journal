# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Self-contained fixtures for speakers app tests."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

import numpy as np
import pytest

from solstone.think.utils import get_project_root

ROOT = Path(get_project_root())
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from solstone.think.entities import entity_slug

# Default stream name for test fixtures
STREAM = "test"


def journal_tree_hash(journal_root: Path) -> dict[str, str]:
    """Return stable content hashes for every file under a journal root."""
    return {
        path.relative_to(journal_root).as_posix(): hashlib.sha256(
            path.read_bytes()
        ).hexdigest()
        for path in sorted(journal_root.rglob("*"))
        if path.is_file()
    }


@pytest.fixture(autouse=True)
def _skip_supervisor_check(monkeypatch):
    """Allow app CLI tests to run without a live solstone supervisor."""
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")


@pytest.fixture(autouse=True)
def _native_speaker_write_bridge(monkeypatch: pytest.MonkeyPatch) -> None:
    """Use the checked-out native writer for speaker app integration tests."""
    from solstone.apps.speakers import native
    from solstone.think import core_handshake

    helper = ROOT / "core" / "target" / "debug" / "solstone-core"
    if not helper.is_file():
        return
    real_run = native._run_speaker_resolve

    def run(verb, request, **kwargs):
        kwargs.setdefault(
            "handshake_checker", lambda: core_handshake.CoreHandshakeResult("ok")
        )
        kwargs.setdefault("helper_locator", lambda: helper)
        kwargs.setdefault("platform_covered", lambda: True)
        return real_run(verb, request, **kwargs)

    monkeypatch.setattr(native, "_run_speaker_resolve", run)


@pytest.fixture
def speakers_env(tmp_path, monkeypatch):
    """Create a temporary journal environment for speaker tests.

    Provides helpers to create:
    - Day directories with sentence embeddings
    - Journal-level entities with voiceprints

    Usage:
        def test_example(speakers_env):
            env = speakers_env()
            env.create_segment("20240101", "143022_300", ["mic_audio"])
            env.create_entity("Alice Test")
            # Now SOLSTONE_JOURNAL is set and data exists
    """

    class SpeakersEnv:
        def __init__(self, journal_path: Path):
            self.journal = journal_path
            monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_path))
            monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
            config_path = journal_path / "config" / "journal.json"
            config_path.parent.mkdir(parents=True, exist_ok=True)
            config_path.write_text(
                json.dumps({"setup": {"completed_at": 1700000000000}}) + "\n",
                encoding="utf-8",
            )
            import solstone.think.utils as think_utils

            think_utils._journal_path_cache = None
            from solstone.apps.speakers.owner import clear_owner_provisional_cache

            clear_owner_provisional_cache()

        def set_identity(self, **identity: object) -> None:
            """Set identity config while preserving existing journal setup."""
            config_path = self.journal / "config" / "journal.json"
            with open(config_path, encoding="utf-8") as f:
                config = json.load(f)
            config["identity"] = dict(identity)
            config_path.write_text(
                json.dumps(config) + "\n",
                encoding="utf-8",
            )

        def _segment_dirs(
            self,
            day: str,
            segment_key: str,
            *,
            stream: str | None = None,
        ) -> tuple[Path, Path]:
            stream_name = stream or STREAM
            chronicle_day = self.journal / "chronicle" / day
            chronicle_day.mkdir(parents=True, exist_ok=True)
            flat_day = self.journal / day
            if not flat_day.exists():
                flat_day.symlink_to(chronicle_day, target_is_directory=True)
            flat_dir = flat_day / stream_name / segment_key
            chronicle_dir = chronicle_day / stream_name / segment_key
            flat_dir.mkdir(parents=True, exist_ok=True)
            chronicle_dir.mkdir(parents=True, exist_ok=True)
            return flat_dir, chronicle_dir

        def create_segment(
            self,
            day: str,
            segment_key: str,
            sources: list[str],
            num_sentences: int = 5,
            *,
            stream: str | None = None,
            embeddings: np.ndarray | None = None,
            audio_extension: str = ".flac",
        ) -> Path:
            """Create a segment with sentence embeddings.

            Creates both JSONL transcripts and NPZ embedding files.

            Args:
                day: Day string (YYYYMMDD)
                segment_key: Segment key (HHMMSS_LEN)
                sources: List of audio sources (e.g., ["mic_audio", "sys_audio"])
                num_sentences: Number of sentences to create
            """
            flat_dir, chronicle_dir = self._segment_dirs(
                day,
                segment_key,
                stream=stream,
            )

            sentence_count = (
                embeddings.shape[0] if embeddings is not None else num_sentences
            )

            for source in sources:
                lines = [
                    json.dumps(
                        {"raw": f"{source}{audio_extension}", "model": "medium.en"}
                    )
                ]

                # Parse segment_key to get base time (e.g., "143022_300" -> 14:30:22)
                # This matches real transcriber output which uses absolute timestamps
                time_part = segment_key.split("_")[0]
                base_h = int(time_part[0:2])
                base_m = int(time_part[2:4])
                base_s = int(time_part[4:6])
                base_seconds = base_h * 3600 + base_m * 60 + base_s

                for i in range(sentence_count):
                    offset = i * 5  # 5 seconds per sentence
                    abs_seconds = base_seconds + offset
                    h = (abs_seconds // 3600) % 24
                    m = (abs_seconds % 3600) // 60
                    s = abs_seconds % 60
                    lines.append(
                        json.dumps(
                            {
                                "start": f"{h:02d}:{m:02d}:{s:02d}",
                                "text": f"This is sentence {i + 1}.",
                            }
                        )
                    )
                for segment_dir in (flat_dir, chronicle_dir):
                    (segment_dir / f"{source}.jsonl").write_text(
                        "\n".join(lines) + "\n"
                    )

                # Create NPZ embeddings
                if embeddings is None:
                    source_embeddings = np.random.randn(sentence_count, 256).astype(
                        np.float32
                    )
                    norms = np.linalg.norm(source_embeddings, axis=1, keepdims=True)
                    source_embeddings = source_embeddings / norms
                else:
                    source_embeddings = embeddings.astype(np.float32)
                statement_ids = np.arange(1, sentence_count + 1, dtype=np.int32)
                for segment_dir in (flat_dir, chronicle_dir):
                    np.savez_compressed(
                        segment_dir / f"{source}.npz",
                        embeddings=source_embeddings,
                        statement_ids=statement_ids,
                    )
                    (segment_dir / f"{source}{audio_extension}").write_bytes(b"")

            return flat_dir

        def create_embedding(self, vector: list[float] | None = None) -> np.ndarray:
            """Create a normalized 256-dim embedding."""
            if vector is None:
                emb = np.random.randn(256).astype(np.float32)
            else:
                emb = np.array(vector + [0.0] * (256 - len(vector)), dtype=np.float32)
            return emb / np.linalg.norm(emb)

        def create_entity(
            self,
            name: str,
            voiceprints: list[tuple[str, str, str, int]] | None = None,
            is_principal: bool = False,
        ) -> Path:
            """Create a journal-level entity with optional voiceprint files.

            Args:
                name: Entity name
                voiceprints: Optional list of (day, segment_key, source, sentence_id)
                            tuples for voiceprints
                is_principal: If True, mark this entity as the principal (self)
            """
            # Create journal-level entity
            entity_id = entity_slug(name)
            journal_entity_dir = self.journal / "entities" / entity_id
            journal_entity_dir.mkdir(parents=True, exist_ok=True)
            journal_entity = {
                "id": entity_id,
                "name": name,
                "type": "Person",
                "created_at": 1700000000000,
            }
            if is_principal:
                journal_entity["is_principal"] = True
            with open(journal_entity_dir / "entity.json", "w", encoding="utf-8") as f:
                json.dump(journal_entity, f)

            # Create voiceprints.npz at journal level if specified
            if voiceprints:
                all_embeddings = []
                all_metadata = []
                for day, segment_key, source, sentence_id in voiceprints:
                    emb = self.create_embedding()
                    all_embeddings.append(emb)
                    metadata = {
                        "day": day,
                        "segment_key": segment_key,
                        "source": source,
                        "sentence_id": sentence_id,
                        "added_at": 1700000000000,
                    }
                    all_metadata.append(json.dumps(metadata))

                np.savez_compressed(
                    journal_entity_dir / "voiceprints.npz",
                    embeddings=np.array(all_embeddings, dtype=np.float32),
                    metadata=np.array(all_metadata, dtype=str),
                )

            return journal_entity_dir

        def create_speakers_json(
            self,
            day: str,
            segment_key: str,
            speakers: list[str],
            *,
            raw: bytes | str | None = None,
        ) -> Path:
            """Create a speakers.json file in a segment directory.

            Args:
                day: Day string (YYYYMMDD)
                segment_key: Segment key (HHMMSS_LEN)
                speakers: List of speaker names
                raw: Optional raw file content to write verbatim
            """
            flat_dir, chronicle_dir = self._segment_dirs(day, segment_key)
            paths = []
            for segment_dir in (flat_dir, chronicle_dir):
                agents_dir = segment_dir / "talents"
                agents_dir.mkdir(parents=True, exist_ok=True)
                speakers_path = agents_dir / "speakers.json"
                if isinstance(raw, bytes):
                    speakers_path.write_bytes(raw)
                elif isinstance(raw, str):
                    speakers_path.write_text(raw, encoding="utf-8")
                else:
                    with open(speakers_path, "w", encoding="utf-8") as f:
                        json.dump(speakers, f)
                paths.append(speakers_path)

            return paths[0]

        def create_speaker_labels(
            self,
            day: str,
            segment_key: str,
            labels: list[dict],
            metadata: dict | None = None,
        ) -> Path:
            """Create a speaker_labels.json file in a segment directory.

            Args:
                day: Day string (YYYYMMDD)
                segment_key: Segment key (HHMMSS_LEN)
                labels: List of label dicts with sentence_id, speaker, confidence,
                    method
                metadata: Optional extra metadata (owner_centroid_last_refreshed_at,
                    voiceprint_versions)
            """
            data = {"labels": labels}
            if metadata:
                data.update(metadata)
            else:
                data["owner_centroid_last_refreshed_at"] = None
                data["voiceprint_versions"] = {}
            flat_dir, chronicle_dir = self._segment_dirs(day, segment_key)
            paths = []
            for segment_dir in (flat_dir, chronicle_dir):
                agents_dir = segment_dir / "talents"
                agents_dir.mkdir(parents=True, exist_ok=True)
                labels_path = agents_dir / "speaker_labels.json"
                with open(labels_path, "w", encoding="utf-8") as f:
                    json.dump(data, f)
                paths.append(labels_path)

            return paths[0]

        def create_speaker_corrections(
            self,
            day: str,
            segment_key: str,
            corrections: list[dict],
            *,
            stream: str | None = None,
        ) -> Path:
            """Create a speaker_corrections.json file in a segment directory.

            Args:
                day: Day string (YYYYMMDD)
                segment_key: Segment key (HHMMSS_LEN)
                corrections: List of correction dicts with sentence_id,
                    original_speaker, corrected_speaker, timestamp
                stream: Optional stream name (defaults to STREAM)
            """
            data = {"corrections": corrections}
            flat_dir, chronicle_dir = self._segment_dirs(
                day,
                segment_key,
                stream=stream,
            )
            paths = []
            for segment_dir in (flat_dir, chronicle_dir):
                agents_dir = segment_dir / "talents"
                agents_dir.mkdir(parents=True, exist_ok=True)
                corrections_path = agents_dir / "speaker_corrections.json"
                with open(corrections_path, "w", encoding="utf-8") as f:
                    json.dump(data, f)
                paths.append(corrections_path)

            return paths[0]

        def create_screen_json(
            self,
            day: str,
            segment_key: str,
            attendees: list[str],
            *,
            stream: str | None = None,
            malformed: bool = False,
        ) -> Path:
            """Create byte-realistic screen talent output for a segment."""
            flat_dir, chronicle_dir = self._segment_dirs(
                day,
                segment_key,
                stream=stream,
            )
            paths = []
            for segment_dir in (flat_dir, chronicle_dir):
                agents_dir = segment_dir / "talents"
                agents_dir.mkdir(parents=True, exist_ok=True)
                screen_path = agents_dir / "screen.json"
                if malformed:
                    screen_path.write_bytes(b"{not json")
                else:
                    screen_path.write_text(
                        json.dumps(
                            {
                                "narrative": "Meeting participants were visible.",
                                "entities": [
                                    {
                                        "type": "Person",
                                        "name": name,
                                        "role": "attendee",
                                        "context": "Visible in participant tile.",
                                    }
                                    for name in attendees
                                ],
                            }
                        ),
                        encoding="utf-8",
                    )
                paths.append(screen_path)

            return paths[0]

        def create_meetings_md(
            self,
            day: str,
            participants: list[str],
            *,
            malformed: bool = False,
        ) -> Path:
            """Create byte-realistic daily meetings talent output."""
            chronicle_day = self.journal / "chronicle" / day
            chronicle_day.mkdir(parents=True, exist_ok=True)
            flat_day = self.journal / day
            if not flat_day.exists():
                flat_day.symlink_to(chronicle_day, target_is_directory=True)
            agents_dir = chronicle_day / "talents"
            agents_dir.mkdir(parents=True, exist_ok=True)
            meetings_path = agents_dir / "meetings.md"
            if malformed:
                meetings_path.write_bytes(b"\xff\xfe\xfa")
            else:
                meetings_path.write_text(
                    f"**Participants:** {', '.join(participants)}\n",
                    encoding="utf-8",
                )

            return meetings_path

        def create_facet_relationship(
            self,
            facet: str,
            entity_id: str,
            *,
            description: str = "",
            attached_at: int = 1700000000000,
            updated_at: int | None = None,
            last_seen: str | None = None,
            observations: list[str] | None = None,
        ) -> Path:
            """Create a facet relationship for an entity.

            Args:
                facet: Facet name (e.g., "work", "personal")
                entity_id: Entity ID (slug)
                description: Relationship description
                attached_at: When the relationship was created
                updated_at: Last update timestamp
                last_seen: Last seen day string (YYYYMMDD)
                observations: Optional list of observation strings
            """
            rel_dir = self.journal / "facets" / facet / "entities" / entity_id
            rel_dir.mkdir(parents=True, exist_ok=True)

            relationship: dict = {
                "entity_id": entity_id,
                "attached_at": attached_at,
            }
            if description:
                relationship["description"] = description
            if updated_at is not None:
                relationship["updated_at"] = updated_at
            if last_seen is not None:
                relationship["last_seen"] = last_seen

            with open(rel_dir / "entity.json", "w", encoding="utf-8") as f:
                json.dump(relationship, f, indent=2)

            if observations:
                with open(rel_dir / "observations.jsonl", "w", encoding="utf-8") as f:
                    for obs in observations:
                        f.write(
                            json.dumps({"content": obs, "observed_at": 1700000000000})
                            + "\n"
                        )

            return rel_dir

        def create_import_segment(
            self,
            day: str,
            segment_key: str,
            speakers: list[tuple[str, str]],
            *,
            stream: str = "import.granola",
            embeddings: np.ndarray | None = None,
            setting: str | None = None,
        ) -> Path:
            """Create an import segment with conversation_transcript and embeddings.

            Creates both a conversation_transcript.jsonl (with speaker labels) and
            imported_audio.{jsonl,npz,flac} (with aligned embeddings) in the
            same segment directory.

            Args:
                day: Day string (YYYYMMDD)
                segment_key: Segment key (HHMMSS_LEN)
                speakers: List of (speaker_name, text) tuples for each sentence
                stream: Import stream name (default: import.granola)
                embeddings: Optional pre-built embeddings array (num_sentences x 256)
                setting: Optional setting field for imported_audio.jsonl header
            """
            flat_dir, chronicle_dir = self._segment_dirs(
                day,
                segment_key,
                stream=stream,
            )

            num_sentences = len(speakers)

            time_part = segment_key.split("_")[0]
            base_h = int(time_part[0:2])
            base_m = int(time_part[2:4])
            base_s = int(time_part[4:6])
            base_seconds = base_h * 3600 + base_m * 60 + base_s

            ct_lines = [
                json.dumps({"imported": {"id": "test-import"}, "topics": "test"})
            ]
            for i, (speaker, text) in enumerate(speakers):
                offset = i * 5
                abs_seconds = base_seconds + offset
                h = (abs_seconds // 3600) % 24
                m = (abs_seconds % 3600) // 60
                s = abs_seconds % 60
                ct_lines.append(
                    json.dumps(
                        {
                            "start": f"{h:02d}:{m:02d}:{s:02d}",
                            "speaker": speaker,
                            "text": text,
                            "source": "import",
                        }
                    )
                )
            for segment_dir in (flat_dir, chronicle_dir):
                (segment_dir / "conversation_transcript.jsonl").write_text(
                    "\n".join(ct_lines) + "\n"
                )

            audio_header = {"raw": "imported_audio.flac", "model": "medium.en"}
            if setting is not None:
                audio_header["setting"] = setting
            audio_lines = [json.dumps(audio_header)]
            for i, (_speaker, text) in enumerate(speakers):
                offset = i * 5
                abs_seconds = base_seconds + offset
                h = (abs_seconds // 3600) % 24
                m = (abs_seconds % 3600) // 60
                s = abs_seconds % 60
                audio_lines.append(
                    json.dumps(
                        {
                            "start": f"{h:02d}:{m:02d}:{s:02d}",
                            "text": text,
                        }
                    )
                )
            for segment_dir in (flat_dir, chronicle_dir):
                (segment_dir / "imported_audio.jsonl").write_text(
                    "\n".join(audio_lines) + "\n"
                )

            if embeddings is None:
                source_embeddings = np.random.randn(num_sentences, 256).astype(
                    np.float32
                )
                norms = np.linalg.norm(source_embeddings, axis=1, keepdims=True)
                source_embeddings = source_embeddings / norms
            else:
                source_embeddings = embeddings.astype(np.float32)
            statement_ids = np.arange(1, num_sentences + 1, dtype=np.int32)
            for segment_dir in (flat_dir, chronicle_dir):
                np.savez_compressed(
                    segment_dir / "imported_audio.npz",
                    embeddings=source_embeddings,
                    statement_ids=statement_ids,
                )
                (segment_dir / "imported_audio.flac").write_bytes(b"")

            return flat_dir

    def _create():
        return SpeakersEnv(tmp_path)

    yield _create
    import solstone.think.utils as think_utils

    think_utils._journal_path_cache = None
