# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the native speaker artifact wipe route."""

from __future__ import annotations

from flask import Flask


def test_wipe_dry_run_forwards_native_request(speakers_env, monkeypatch) -> None:
    from solstone.apps.speakers import routes

    env = speakers_env()
    expected = {
        "dry_run": True,
        "segment_embeddings": {
            "count": 1,
            "bytes": 9,
            "paths": ["chronicle/20240101/test/120000_300/audio.npz"],
        },
        "speaker_labels": {
            "count": 2,
            "bytes": 18,
            "paths": [
                "chronicle/20240101/test/120000_300/agents/speaker_labels.json",
                "chronicle/20240101/test/120000_300/talents/speaker_labels.json",
            ],
        },
        "speaker_corrections": {
            "count": 2,
            "bytes": 18,
            "paths": [
                "chronicle/20240101/test/120000_300/agents/speaker_corrections.json",
                "chronicle/20240101/test/120000_300/talents/speaker_corrections.json",
            ],
        },
        "entity_voiceprints": {
            "count": 1,
            "bytes": 9,
            "paths": ["entities/a/voiceprints.npz"],
        },
        "owner_centroids": {"count": 0, "bytes": 0, "paths": []},
        "owner_candidate": {"count": 0, "bytes": 0, "paths": []},
        "total_files": 6,
        "total_bytes": 54,
    }
    calls = []

    def wipe(journal_root, *, dry_run):
        calls.append((journal_root, dry_run))
        return expected

    monkeypatch.setattr(routes.native_speakers, "wipe_speaker_artifacts", wipe)
    app = Flask(__name__)
    app.register_blueprint(routes.speakers_bp)

    with app.test_client() as client:
        response = client.post("/app/speakers/api/wipe", json={})

    assert response.status_code == 200
    assert response.get_json() == expected
    assert calls == [(str(env.journal), True)]


def test_wipe_commit_forwards_native_request(speakers_env, monkeypatch) -> None:
    from solstone.apps.speakers import routes

    env = speakers_env()
    expected = {
        "dry_run": False,
        "segment_embeddings": {
            "count": 1,
            "bytes": 9,
            "paths": ["chronicle/20240101/test/120000_300/audio.npz"],
        },
        "speaker_labels": {
            "count": 2,
            "bytes": 18,
            "paths": [
                "chronicle/20240101/test/120000_300/agents/speaker_labels.json",
                "chronicle/20240101/test/120000_300/talents/speaker_labels.json",
            ],
        },
        "speaker_corrections": {
            "count": 2,
            "bytes": 18,
            "paths": [
                "chronicle/20240101/test/120000_300/agents/speaker_corrections.json",
                "chronicle/20240101/test/120000_300/talents/speaker_corrections.json",
            ],
        },
        "entity_voiceprints": {
            "count": 1,
            "bytes": 9,
            "paths": ["entities/a/voiceprints.npz"],
        },
        "owner_centroids": {
            "count": 1,
            "bytes": 9,
            "paths": ["entities/a/owner_centroid.npz"],
        },
        "owner_candidate": {
            "count": 1,
            "bytes": 9,
            "paths": ["awareness/owner_candidate.npz"],
        },
        "total_files": 8,
        "total_bytes": 72,
    }
    calls = []

    def wipe(journal_root, *, dry_run):
        calls.append((journal_root, dry_run))
        return expected

    monkeypatch.setattr(routes.native_speakers, "wipe_speaker_artifacts", wipe)
    app = Flask(__name__)
    app.register_blueprint(routes.speakers_bp)

    with app.test_client() as client:
        response = client.post("/app/speakers/api/wipe", json={"commit": True})

    assert response.status_code == 200
    assert response.get_json() == expected
    assert calls == [(str(env.journal), False)]
