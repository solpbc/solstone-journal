# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for observer app utilities."""

from __future__ import annotations

import json
import logging

import pytest

from solstone.apps.observer.utils import (
    ObserverRegistry,
    append_history_record,
    find_observer_by_name,
    find_segment_by_sha256,
    get_hist_dir,
    get_observers_dir,
    increment_stat,
    list_observers,
    load_history,
    load_observer,
    load_observer_by_fingerprint,
    mint_pl_observer_record,
    observer_filename_prefix,
    save_observer,
)

FINGERPRINT = "sha256:" + ("a" * 64)
FINGERPRINT_2 = "sha256:" + ("b" * 64)


@pytest.fixture
def storage_env(tmp_path, monkeypatch):
    """Create a temporary journal environment for storage tests."""
    from solstone.convey import state

    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr(state, "journal_root", str(journal))

    # Create observers directory
    observers_dir = journal / "apps" / "observer" / "observers"
    observers_dir.mkdir(parents=True)

    class Env:
        def __init__(self):
            self.journal = journal
            self.observers_dir = observers_dir

    return Env()


class TestObserverStorage:
    """Tests for observer metadata storage."""

    def test_get_observers_dir_creates_directory(self, storage_env):
        """get_observers_dir creates and returns observers directory."""
        result = get_observers_dir()
        assert result.exists()
        assert result == storage_env.observers_dir

    def test_save_and_load_observer(self, storage_env):
        """save_observer and load_observer work together."""
        observer = {
            "key": "testkey123456789",
            "name": "test-observer",
            "stats": {"segments_received": 0},
        }

        assert save_observer(observer) is True

        loaded = load_observer("testkey123456789")
        assert loaded is not None
        assert loaded["name"] == "test-observer"
        assert loaded["mode"] == "dl"
        assert loaded["filename_prefix"] == "testkey1"

    def test_observer_filename_prefix_for_dl_and_pl(self, storage_env):
        assert observer_filename_prefix({"key": "abcdef123456"}) == "abcdef12"
        assert observer_filename_prefix({"fingerprint": FINGERPRINT}) == "a" * 16

    def test_save_and_load_pl_observer(self, storage_env):
        observer = {
            "fingerprint": FINGERPRINT,
            "name": "pl-observer",
            "stats": {"segments_received": 0},
        }

        assert save_observer(observer) is True

        loaded = load_observer_by_fingerprint(FINGERPRINT)
        assert loaded is not None
        assert loaded["name"] == "pl-observer"
        assert loaded["mode"] == "pl"
        assert loaded["filename_prefix"] == "a" * 16
        assert (storage_env.observers_dir / f"{'a' * 16}.json").exists()

    def test_mint_pl_observer_record(self, storage_env):
        path = mint_pl_observer_record(FINGERPRINT, "laptop", "2026-04-20T00:00:00Z")

        assert path == storage_env.observers_dir / f"{'a' * 16}.json"
        loaded = load_observer_by_fingerprint(FINGERPRINT)
        assert loaded is not None
        assert loaded["name"] == "laptop"
        assert loaded["paired_at"] == "2026-04-20T00:00:00Z"
        assert loaded["enabled"] is True

    def test_mint_pl_observer_record_refuses_existing(self, storage_env):
        mint_pl_observer_record(FINGERPRINT, "laptop", "2026-04-20T00:00:00Z")

        with pytest.raises(FileExistsError):
            mint_pl_observer_record(FINGERPRINT, "laptop", "2026-04-20T00:00:01Z")

    def test_observer_registry_skips_invalid_records(self, storage_env, caplog):
        caplog.set_level(logging.WARNING)
        invalid_path = storage_env.observers_dir / "bad.json"
        invalid_path.write_text(
            json.dumps({"key": "badkey123", "fingerprint": FINGERPRINT}) + "\n",
            encoding="utf-8",
        )
        ObserverRegistry.singleton().invalidate()

        assert list_observers() == []
        assert "Skipping invalid observer record" in caplog.text

    def test_observer_registry_by_prefix(self, storage_env):
        save_observer({"key": "dlkey123456789", "name": "dl", "stats": {}})
        save_observer({"fingerprint": FINGERPRINT_2, "name": "pl", "stats": {}})

        registry = ObserverRegistry.singleton()
        assert registry.by_prefix("dlkey123")["name"] == "dl"
        assert registry.by_prefix("b" * 16)["name"] == "pl"

    def test_load_observer_wrong_key(self, storage_env):
        """load_observer returns None for wrong key."""
        observer = {
            "key": "testkey123456789",
            "name": "test-observer",
            "stats": {},
        }
        save_observer(observer)

        # Same prefix but different key
        result = load_observer("testkey1xxxxxxxx")
        assert result is None

    def test_load_observer_not_found(self, storage_env):
        """load_observer returns None when observer doesn't exist."""
        result = load_observer("nonexistent12345")
        assert result is None

    def test_list_observers_empty(self, storage_env):
        """list_observers returns empty list when no observers."""
        result = list_observers()
        assert result == []

    def test_list_observers_returns_all(self, storage_env):
        """list_observers returns all registered observers."""
        for i in range(3):
            save_observer(
                {
                    "key": f"obs{i:05d}123456789",
                    "name": f"observer-{i}",
                    "created_at": 1000 + i,
                    "stats": {},
                }
            )

        result = list_observers()
        assert len(result) == 3
        # Sorted by created_at descending
        assert result[0]["name"] == "observer-2"
        assert result[1]["name"] == "observer-1"
        assert result[2]["name"] == "observer-0"

    def test_find_observer_by_name(self, storage_env):
        """find_observer_by_name finds existing observer."""
        save_observer(
            {
                "key": "findme123456789",
                "name": "find-me",
                "stats": {},
            }
        )

        result = find_observer_by_name("find-me")
        assert result is not None
        assert result["key"] == "findme123456789"

    def test_find_observer_by_name_not_found(self, storage_env):
        """find_observer_by_name returns None for unknown name."""
        result = find_observer_by_name("unknown")
        assert result is None


class TestHistoryStorage:
    """Tests for sync history storage."""

    def test_get_hist_dir_creates_directory(self, storage_env):
        """get_hist_dir creates history directory."""
        result = get_hist_dir("testkey1")
        assert result.exists()
        assert result == storage_env.observers_dir / "testkey1" / "hist"

    def test_get_hist_dir_no_create(self, storage_env):
        """get_hist_dir with ensure_exists=False doesn't create."""
        result = get_hist_dir("nonexistent", ensure_exists=False)
        assert not result.exists()

    def test_append_history_record(self, storage_env):
        """append_history_record creates and appends to JSONL file."""
        append_history_record(
            "testkey1", "20250103", {"type": "upload", "segment": "120000_300"}
        )
        append_history_record(
            "testkey1", "20250103", {"type": "observed", "segment": "120000_300"}
        )

        hist_path = storage_env.observers_dir / "testkey1" / "hist" / "20250103.jsonl"
        assert hist_path.exists()

        with open(hist_path) as f:
            lines = f.readlines()

        assert len(lines) == 2
        assert json.loads(lines[0])["type"] == "upload"
        assert json.loads(lines[1])["type"] == "observed"

    def test_load_history_empty(self, storage_env):
        """load_history returns empty list when no history."""
        result = load_history("testkey1", "20250103")
        assert result == []

    def test_load_history(self, storage_env):
        """load_history returns all records."""
        append_history_record("testkey1", "20250103", {"segment": "a"})
        append_history_record("testkey1", "20250103", {"segment": "b"})

        result = load_history("testkey1", "20250103")
        assert len(result) == 2
        assert result[0]["segment"] == "a"
        assert result[1]["segment"] == "b"


class TestIncrementStat:
    """Tests for stat increment."""

    def test_increment_stat_new_counter(self, storage_env):
        """increment_stat creates new counter."""
        save_observer(
            {
                "key": "testkey123456789",
                "name": "test",
                "stats": {},
            }
        )

        increment_stat("testkey1", "segments_observed")

        loaded = load_observer("testkey123456789")
        assert loaded["stats"]["segments_observed"] == 1

    def test_increment_stat_existing_counter(self, storage_env):
        """increment_stat increments existing counter."""
        save_observer(
            {
                "key": "testkey123456789",
                "name": "test",
                "stats": {"segments_observed": 5},
            }
        )

        increment_stat("testkey1", "segments_observed")

        loaded = load_observer("testkey123456789")
        assert loaded["stats"]["segments_observed"] == 6

    def test_increment_stat_missing_observer(self, storage_env):
        """increment_stat handles missing observer gracefully."""
        # Should not raise
        increment_stat("nonexistent", "segments_observed")


class TestAtomicWriteCrashSafety:
    """Tests for atomic write crash safety."""

    def test_save_observer_crash_preserves_existing_file(
        self, storage_env, monkeypatch
    ):
        """save_observer leaves prior observer data intact on replace failure."""
        observer = {
            "key": "testkey123456789",
            "name": "original",
            "stats": {},
        }
        assert save_observer(observer) is True

        def raising_stub(*args, **kwargs):
            raise OSError("simulated crash")

        monkeypatch.setattr("solstone.think.entities.core.os.replace", raising_stub)

        updated_observer = {
            "key": "testkey123456789",
            "name": "updated",
            "stats": {},
        }
        assert save_observer(updated_observer) is False

        loaded = load_observer("testkey123456789")
        assert loaded is not None
        assert loaded["name"] == "original"
        assert list(storage_env.observers_dir.glob(".tmp_*")) == []

    def test_increment_stat_crash_preserves_existing_file(
        self, storage_env, monkeypatch
    ):
        """increment_stat leaves prior observer data intact on replace failure."""
        observer = {
            "key": "testkey123456789",
            "name": "test",
            "stats": {"events_received": 5},
        }
        assert save_observer(observer) is True

        def raising_stub(*args, **kwargs):
            raise OSError("simulated crash")

        monkeypatch.setattr("solstone.think.entities.core.os.replace", raising_stub)

        increment_stat("testkey1", "events_received")

        loaded = load_observer("testkey123456789")
        assert loaded is not None
        assert loaded["stats"]["events_received"] == 5
        assert list(storage_env.observers_dir.glob(".tmp_*")) == []


class TestFindSegmentBySha256:
    """Tests for find_segment_by_sha256."""

    def test_no_history_returns_no_match(self, storage_env):
        """Returns (None, empty set) when no history exists."""
        segment, matched = find_segment_by_sha256(
            "testkey1", "20250103", {"sha256_abc"}
        )
        assert segment is None
        assert matched == set()

    def test_full_match_returns_segment(self, storage_env):
        """Returns segment key when all SHA256s match."""
        # Create history with segment upload
        append_history_record(
            "testkey1",
            "20250103",
            {
                "segment": "120000_300",
                "files": [
                    {"sha256": "sha256_aaa", "written": "audio.flac"},
                    {"sha256": "sha256_bbb", "written": "screen.mp4"},
                ],
            },
        )

        segment, matched = find_segment_by_sha256(
            "testkey1", "20250103", {"sha256_aaa", "sha256_bbb"}
        )
        assert segment == "120000_300"
        assert matched == {"sha256_aaa", "sha256_bbb"}

    def test_partial_match_returns_matched_set(self, storage_env):
        """Returns (None, matched set) when only some SHA256s match."""
        append_history_record(
            "testkey1",
            "20250103",
            {
                "segment": "120000_300",
                "files": [
                    {"sha256": "sha256_aaa", "written": "audio.flac"},
                ],
            },
        )

        # Request includes one matching and one new
        segment, matched = find_segment_by_sha256(
            "testkey1", "20250103", {"sha256_aaa", "sha256_new"}
        )
        assert segment is None
        assert matched == {"sha256_aaa"}

    def test_no_match_returns_empty_set(self, storage_env):
        """Returns (None, empty set) when no SHA256s match."""
        append_history_record(
            "testkey1",
            "20250103",
            {
                "segment": "120000_300",
                "files": [
                    {"sha256": "sha256_aaa", "written": "audio.flac"},
                ],
            },
        )

        segment, matched = find_segment_by_sha256(
            "testkey1", "20250103", {"sha256_xxx", "sha256_yyy"}
        )
        assert segment is None
        assert matched == set()

    def test_skips_observed_records(self, storage_env):
        """Ignores records with type field (e.g., 'observed')."""
        # Upload record
        append_history_record(
            "testkey1",
            "20250103",
            {
                "segment": "120000_300",
                "files": [
                    {"sha256": "sha256_aaa", "written": "audio.flac"},
                ],
            },
        )
        # Observed record
        append_history_record(
            "testkey1",
            "20250103",
            {
                "type": "observed",
                "segment": "120000_300",
            },
        )

        segment, matched = find_segment_by_sha256(
            "testkey1", "20250103", {"sha256_aaa"}
        )
        assert segment == "120000_300"
        assert matched == {"sha256_aaa"}

    def test_subset_match_returns_segment(self, storage_env):
        """Returns segment when incoming is subset of existing files."""
        # Segment has 3 files
        append_history_record(
            "testkey1",
            "20250103",
            {
                "segment": "120000_300",
                "files": [
                    {"sha256": "sha256_aaa", "written": "audio.flac"},
                    {"sha256": "sha256_bbb", "written": "screen.mp4"},
                    {"sha256": "sha256_ccc", "written": "audio.jsonl"},
                ],
            },
        )

        # Request only 2 of the 3 files (subset)
        segment, matched = find_segment_by_sha256(
            "testkey1", "20250103", {"sha256_aaa", "sha256_bbb"}
        )
        assert segment == "120000_300"
        assert matched == {"sha256_aaa", "sha256_bbb"}
