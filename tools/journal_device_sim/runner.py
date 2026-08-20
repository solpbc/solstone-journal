# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Resumable fixture upload, reconciliation, and evidence generation."""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import time
import uuid
from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone
from enum import Enum
from pathlib import Path
from typing import Any

from .http_client import BridgeHttpClient, HttpRequestError, HttpResponse
from .manifest import FixtureManifest, FixtureSegment, ManifestError
from .process import LinkBridge, LinkProcessError

STATE_SCHEMA = "solstone.journal-device-sim.state.v1"
EVIDENCE_SCHEMA = "solstone.journal-device-sim.evidence.v1"


class RunOutcome(str, Enum):
    PASS = "PASS"
    FAIL = "FAIL"
    BLOCKED = "BLOCKED"
    INCONCLUSIVE = "INCONCLUSIVE"


class SimulationFailure(RuntimeError):
    """A product or fixture assertion failed."""


class SimulationInconclusive(RuntimeError):
    """The simulator could not determine whether bytes landed."""


@dataclass(frozen=True)
class SimulatorConfig:
    manifest: FixtureManifest
    profile: str
    carrier: str
    state_dir: Path
    evidence_path: Path
    bridge_url: str | None = None
    pair_code: str | None = None
    solstone_bin: str = "solstone"
    relay_url: str | None = None
    date_mode: str = "shift"
    anchor_day: str | None = None
    journal_root: Path | None = None
    request_timeout: float = 90.0
    processing_timeout: float = 0.0
    poll_interval: float = 1.0
    max_attempts: int = 3
    keep_credentials: bool = False


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _atomic_json(path: Path, value: dict[str, Any], mode: int = 0o600) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    encoded = json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if temporary.exists():
            temporary.unlink()


def _read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise SimulationFailure(
            f"cannot read simulator state {path}: {error}"
        ) from error
    if not isinstance(value, dict):
        raise SimulationFailure(f"simulator state {path} is not a JSON object")
    return value


def _parse_day(raw: str, where: str) -> date:
    try:
        return datetime.strptime(raw, "%Y%m%d").date()
    except ValueError as error:
        raise SimulationFailure(f"{where} must be YYYYMMDD") from error


def build_day_map(
    segments: tuple[FixtureSegment, ...], date_mode: str, anchor_day: str | None
) -> dict[str, str]:
    days = sorted({_parse_day(segment.day, "fixture day") for segment in segments})
    if date_mode == "preserve":
        return {day.strftime("%Y%m%d"): day.strftime("%Y%m%d") for day in days}
    if date_mode != "shift":
        raise SimulationFailure("date mode must be shift or preserve")
    anchor = _parse_day(anchor_day, "anchor day") if anchor_day else date.today()
    last = days[-1]
    return {
        day.strftime("%Y%m%d"): (anchor + timedelta(days=(day - last).days)).strftime(
            "%Y%m%d"
        )
        for day in days
    }


def _git_revision(path: Path) -> str | None:
    try:
        result = subprocess.run(
            ["git", "-C", str(path), "rev-parse", "HEAD"],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    return result.stdout.strip() or None


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


class Simulator:
    """Drive one manifest profile through a real local link bridge."""

    def __init__(self, config: SimulatorConfig) -> None:
        if config.carrier not in {"direct", "relay"}:
            raise ManifestError("carrier must be direct or relay")
        if bool(config.bridge_url) == bool(config.pair_code):
            raise ManifestError("provide exactly one of bridge_url or pair_code")
        if config.max_attempts < 1:
            raise ManifestError("max_attempts must be positive")
        if config.request_timeout <= 0:
            raise ManifestError("request_timeout must be positive")
        if config.processing_timeout < 0:
            raise ManifestError("processing_timeout cannot be negative")
        if config.poll_interval <= 0:
            raise ManifestError("poll_interval must be positive")
        fixture_root = config.manifest.root.resolve()
        state_dir = config.state_dir.resolve()
        evidence_path = config.evidence_path.resolve()
        for path, label in [
            (state_dir, "state directory"),
            (evidence_path, "evidence path"),
        ]:
            try:
                path.relative_to(fixture_root)
            except ValueError:
                pass
            else:
                raise ManifestError(f"{label} cannot be inside the fixture root")
        if evidence_path == state_dir / "state.json":
            raise ManifestError("evidence path cannot overwrite simulator state")
        if config.journal_root is not None:
            journal_root = config.journal_root.resolve()
            if journal_root == fixture_root:
                raise ManifestError(
                    "receiving journal root must differ from the fixture root"
                )
            if not journal_root.is_dir():
                raise ManifestError("journal_root must be an existing directory")
        self.config = config
        self.profile = config.manifest.profiles.get(config.profile)
        if self.profile is None:
            config.manifest.profile_segments(config.profile)
            raise AssertionError("profile lookup should have failed")
        self.segments = config.manifest.profile_segments(config.profile)
        self.day_map = build_day_map(self.segments, config.date_mode, config.anchor_day)
        self.state_path = config.state_dir / "state.json"
        self.state = self._load_or_create_state()
        self.evidence: dict[str, Any] = {
            "schema": EVIDENCE_SCHEMA,
            "run_id": self.state["run_id"],
            "started_at": self.state["started_at"],
            "finished_at": None,
            "result": None,
            "error": None,
            "profile": config.profile,
            "verify_processing": self.profile.verify_processing,
            "carrier": config.carrier,
            "bridge": {
                "ownership": "external" if config.bridge_url else "simulator",
                "carrier_assurance": (
                    "caller-asserted" if config.bridge_url else "native-child"
                ),
            },
            "manifest": {
                "path": str(config.manifest.path),
                "sha256": config.manifest.digest,
                "fixture_revision": _git_revision(config.manifest.root),
            },
            "simulator_revision": _git_revision(Path(__file__).resolve().parents[2]),
            "receiver": None,
            "day_map": self.day_map,
            "segments": [],
        }

    def _load_or_create_state(self) -> dict[str, Any]:
        self.config.state_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(self.config.state_dir, 0o700)
        if self.state_path.exists():
            state = _read_json(self.state_path)
            expected = {
                "schema": STATE_SCHEMA,
                "manifest_sha256": self.config.manifest.digest,
                "profile": self.config.profile,
                "carrier": self.config.carrier,
                "day_map": self.day_map,
            }
            for key, value in expected.items():
                if state.get(key) != value:
                    raise SimulationFailure(
                        f"existing state {key} does not match this run; choose a new state directory"
                    )
            state.setdefault("segments", {})
            return state
        state = {
            "schema": STATE_SCHEMA,
            "run_id": uuid.uuid4().hex,
            "started_at": _utc_now(),
            "manifest_sha256": self.config.manifest.digest,
            "profile": self.config.profile,
            "carrier": self.config.carrier,
            "day_map": self.day_map,
            "segments": {},
        }
        _atomic_json(self.state_path, state)
        return state

    def _save_state(self) -> None:
        _atomic_json(self.state_path, self.state)

    def _verify_fixture_bytes(self, segment: FixtureSegment) -> None:
        for item in segment.files:
            actual_size = item.path.stat().st_size
            if actual_size != item.size:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id}/{item.submitted} changed size during the run"
                )
            actual_sha = _sha256(item.path)
            if actual_sha != item.sha256:
                raise SimulationFailure(
                    f"fixture {segment.fixture_id}/{item.submitted} changed digest during the run"
                )

    def _envelope(self, segment: FixtureSegment, mapped_day: str) -> dict[str, Any]:
        return {
            "day": mapped_day,
            "segment": segment.segment,
            "source": segment.source,
            "meta": {
                **segment.meta,
                "fixture_id": segment.fixture_id,
                "fixture_manifest_sha256": self.config.manifest.digest,
            },
            "files": [
                {**item.metadata, "submitted": item.submitted} for item in segment.files
            ],
        }

    def _listing(
        self, client: BridgeHttpClient, segment: FixtureSegment, mapped_day: str
    ) -> dict[str, Any]:
        query = {"source": segment.source} if segment.source else None
        response = client.get_json(f"/app/devices/ingest/segments/{mapped_day}", query)
        if not 200 <= response.status < 300:
            reason = response.body.get("reason_code", "unknown")
            if response.status >= 500:
                raise HttpRequestError(
                    f"listing for {segment.fixture_id} returned transient HTTP "
                    f"{response.status} ({reason})"
                )
            raise SimulationFailure(
                f"listing for {segment.fixture_id} returned HTTP {response.status} ({reason})"
            )
        if response.body.get("protocol_version") != 3 or not isinstance(
            response.body.get("items"), list
        ):
            raise SimulationFailure(
                "ingest listing does not carry the v3 response shape"
            )
        return response.body

    def _bind_receiver(self, client: BridgeHttpClient) -> None:
        response = client.get_json("/app/link/api/identity")
        if not 200 <= response.status < 300:
            reason = response.body.get("reason_code", "unknown")
            raise SimulationFailure(
                f"receiver identity returned HTTP {response.status} ({reason})"
            )
        instance_id = response.body.get("instance_id")
        if response.body.get("committed") is not True or not isinstance(
            instance_id, str
        ):
            raise SimulationFailure(
                "receiver does not expose a committed journal identity"
            )
        prior = self.state.get("receiver_instance_id")
        if prior is not None and prior != instance_id:
            raise SimulationFailure(
                "state directory belongs to a different receiving journal"
            )
        self.state["receiver_instance_id"] = instance_id
        self._save_state()
        self.evidence["receiver"] = {"instance_id": instance_id}

    @staticmethod
    def _matched_files(
        item: dict[str, Any], segment: FixtureSegment
    ) -> list[dict[str, Any]] | None:
        raw_files = item.get("files")
        if not isinstance(raw_files, list):
            return None
        unmatched = list(raw_files)
        matched: list[dict[str, Any]] = []
        for expected in segment.files:
            match_index = None
            for index, candidate in enumerate(unmatched):
                if not isinstance(candidate, dict):
                    continue
                effective_name = candidate.get("submitted_name", candidate.get("name"))
                if (
                    effective_name == expected.submitted
                    and candidate.get("size") == expected.size
                    and candidate.get("sha256") == expected.sha256
                ):
                    match_index = index
                    break
            if match_index is None:
                return None
            candidate = unmatched.pop(match_index)
            assert isinstance(candidate, dict)
            matched.append(candidate)
        return matched

    def _find_listing_item(
        self,
        listing: dict[str, Any],
        segment: FixtureSegment,
        landed_segment: str | None,
    ) -> dict[str, Any] | None:
        candidates = []
        for item in listing.get("items", []):
            if not isinstance(item, dict) or self._matched_files(item, segment) is None:
                continue
            if landed_segment and item.get("key") != landed_segment:
                continue
            if not landed_segment and not (
                item.get("key") == segment.segment
                or item.get("original_key") == segment.segment
            ):
                continue
            candidates.append(item)
        if len(candidates) > 1:
            raise SimulationFailure(
                f"listing is ambiguous for fixture {segment.fixture_id}; {len(candidates)} matches"
            )
        return candidates[0] if candidates else None

    def _required_outputs_present(
        self, segment: FixtureSegment, mapped_day: str, landed_segment: str
    ) -> tuple[bool, str | None]:
        required = (
            set(segment.expectation.required_outputs)
            if self.profile.verify_processing
            else set()
        )
        if self.config.journal_root is not None:
            required.update({"stream.json", "ingest.json", "events.jsonl"})
        if not required:
            return True, None
        if self.config.journal_root is None:
            raise SimulationFailure(
                f"fixture {segment.fixture_id} requires white-box outputs but no journal root was provided"
            )
        day_root = self.config.journal_root / "chronicle" / mapped_day
        candidates = []
        for path in day_root.glob(f"*/{landed_segment}"):
            ingest_path = path / "ingest.json"
            if (
                not path.is_dir()
                or not all((path / output).is_file() for output in required)
                or not ingest_path.is_file()
            ):
                continue
            try:
                ingest_text = ingest_path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            if all(expected.sha256 in ingest_text for expected in segment.files):
                candidates.append(path)
        if len(candidates) > 1:
            raise SimulationFailure(
                f"multiple journal directories satisfy outputs for {segment.fixture_id}"
            )
        return bool(candidates), str(candidates[0]) if candidates else None

    def _reconcile(
        self,
        client: BridgeHttpClient,
        segment: FixtureSegment,
        mapped_day: str,
        landed_segment: str | None,
        wait: bool,
    ) -> tuple[dict[str, Any] | None, str | None, bool]:
        deadline = time.monotonic() + (self.config.processing_timeout if wait else 0.0)
        while True:
            listing = self._listing(client, segment, mapped_day)
            item = self._find_listing_item(listing, segment, landed_segment)
            if item is not None:
                matched_files = self._matched_files(item, segment)
                assert matched_files is not None
                statuses = {entry.get("status") for entry in matched_files}
                statuses_ok = statuses and statuses.issubset(
                    set(segment.expectation.file_statuses)
                )
                outputs_ok, journal_path = self._required_outputs_present(
                    segment, mapped_day, str(item["key"])
                )
                if statuses_ok and outputs_ok:
                    return item, journal_path, True
            if time.monotonic() >= deadline:
                return item, None, False
            time.sleep(self.config.poll_interval)

    @staticmethod
    def _landed_segment(response: HttpResponse) -> str | None:
        status = response.body.get("status")
        if status == "duplicate":
            value = response.body.get("existing_segment")
        else:
            value = response.body.get("segment")
        return value if isinstance(value, str) else None

    def _verify_duplicate(
        self,
        client: BridgeHttpClient,
        segment: FixtureSegment,
        envelope: dict[str, Any],
        landed_segment: str,
    ) -> dict[str, Any] | None:
        if not self.profile.verify_duplicate:
            return None
        self._verify_fixture_bytes(segment)
        duplicate = client.post_multipart(
            "/app/devices/ingest",
            envelope,
            ((file.submitted, file.path) for file in segment.files),
        )
        if duplicate.status != 200 or duplicate.body.get("status") != "duplicate":
            raise SimulationFailure(
                f"fixture {segment.fixture_id} duplicate replay was not idempotent"
            )
        if duplicate.body.get("existing_segment") != landed_segment:
            raise SimulationFailure(
                f"fixture {segment.fixture_id} duplicate resolved to another segment"
            )
        return duplicate.body

    def _finish_segment(
        self,
        *,
        client: BridgeHttpClient,
        segment: FixtureSegment,
        mapped_day: str,
        envelope: dict[str, Any],
        item: dict[str, Any],
        journal_path: str | None,
        upload_attempts: int,
        resumed: bool,
        response: HttpResponse | None,
    ) -> dict[str, Any]:
        landed_segment = str(item["key"])
        entry = self.state["segments"].setdefault(segment.fixture_id, {})
        entry.update(
            {
                "phase": "reconciled",
                "landed_segment": landed_segment,
                "last_response_status": response.status if response else None,
            }
        )
        self._save_state()
        duplicate_from_state = (
            self.profile.verify_duplicate and entry.get("duplicate_proven") is True
        )
        duplicate = (
            None
            if duplicate_from_state
            else self._verify_duplicate(client, segment, envelope, landed_segment)
        )
        if duplicate is not None:
            entry["duplicate_proven"] = True
        entry["phase"] = "complete"
        self._save_state()
        result = {
            "fixture_id": segment.fixture_id,
            "mapped_day": mapped_day,
            "requested_segment": segment.segment,
            "landed_segment": landed_segment,
            "upload_attempts": upload_attempts,
            "resumed": resumed,
            "response": response.body if response else None,
            "response_http_status": response.status if response else None,
            "listing": item,
            "journal_path": journal_path,
        }
        if duplicate is not None:
            result["duplicate_response"] = duplicate
        elif duplicate_from_state:
            result["duplicate_proven_from_state"] = True
        return result

    def _upload_one(
        self, client: BridgeHttpClient, segment: FixtureSegment
    ) -> dict[str, Any]:
        mapped_day = self.day_map[segment.day]
        prior = self.state["segments"].get(segment.fixture_id, {})
        self._verify_fixture_bytes(segment)
        envelope = self._envelope(segment, mapped_day)
        landed_segment = prior.get("landed_segment")
        retry_after_uncertainty = prior.get("phase") == "sending"
        if prior and (isinstance(landed_segment, str) or retry_after_uncertainty):
            item, journal_path, ready = self._reconcile(
                client,
                segment,
                mapped_day,
                landed_segment if isinstance(landed_segment, str) else None,
                wait=isinstance(landed_segment, str),
            )
            if item is not None and not ready:
                item, journal_path, ready = self._reconcile(
                    client, segment, mapped_day, str(item["key"]), wait=True
                )
            if item is not None and ready:
                return self._finish_segment(
                    client=client,
                    segment=segment,
                    mapped_day=mapped_day,
                    envelope=envelope,
                    item=item,
                    journal_path=journal_path,
                    upload_attempts=int(prior.get("upload_attempts", 0)),
                    resumed=True,
                    response=None,
                )
        last_error: str | None = None
        total_attempts = int(prior.get("upload_attempts", 0))
        attempts_this_run = 0
        while attempts_this_run < self.config.max_attempts:
            attempts_this_run += 1
            total_attempts += 1
            entry = self.state["segments"].setdefault(segment.fixture_id, {})
            entry.update(
                {
                    "mapped_day": mapped_day,
                    "requested_segment": segment.segment,
                    "upload_attempts": total_attempts,
                    "phase": "sending",
                }
            )
            self._save_state()
            response: HttpResponse | None = None
            try:
                response = client.post_multipart(
                    "/app/devices/ingest",
                    envelope,
                    ((item.submitted, item.path) for item in segment.files),
                )
                response_status = response.body.get("status")
                if 200 <= response.status < 300:
                    recovery_duplicate = (
                        response_status == "duplicate" and retry_after_uncertainty
                    )
                    if (
                        response_status not in segment.expectation.upload_statuses
                        and not recovery_duplicate
                    ):
                        raise SimulationFailure(
                            f"fixture {segment.fixture_id} expected upload status "
                            f"{segment.expectation.upload_statuses}, got {response_status!r}"
                        )
                    landed_segment = self._landed_segment(response)
                    if not landed_segment:
                        raise SimulationFailure(
                            f"fixture {segment.fixture_id} response omitted the landed segment"
                        )
                elif response.status < 500:
                    reason = response.body.get("reason_code", "unknown")
                    raise SimulationFailure(
                        f"fixture {segment.fixture_id} was refused with HTTP {response.status} ({reason})"
                    )
                else:
                    last_error = f"HTTP {response.status} ({response.body.get('reason_code', 'unknown')})"
                    retry_after_uncertainty = True
            except HttpRequestError as error:
                last_error = str(error)
                retry_after_uncertainty = True
            try:
                item, journal_path, ready = self._reconcile(
                    client,
                    segment,
                    mapped_day,
                    landed_segment,
                    wait=False,
                )
            except HttpRequestError as error:
                last_error = str(error)
                retry_after_uncertainty = True
                continue
            if item is not None:
                landed_segment = str(item["key"])
                if not ready:
                    item, journal_path, ready = self._reconcile(
                        client, segment, mapped_day, landed_segment, wait=True
                    )
                if not ready:
                    raise SimulationFailure(
                        f"fixture {segment.fixture_id} landed as {landed_segment}, but its "
                        "required custody or processing evidence did not become ready"
                    )
                return self._finish_segment(
                    client=client,
                    segment=segment,
                    mapped_day=mapped_day,
                    envelope=envelope,
                    item=item,
                    journal_path=journal_path,
                    upload_attempts=total_attempts,
                    resumed=False,
                    response=response,
                )
            retry_after_uncertainty = True
        raise SimulationInconclusive(
            f"fixture {segment.fixture_id} did not reconcile after {attempts_this_run} "
            f"attempts in this invocation ({total_attempts} total); "
            f"last uncertainty: {last_error or 'no matching listing'}"
        )

    def _write_evidence(self, outcome: RunOutcome, error: str | None) -> None:
        self.evidence["finished_at"] = _utc_now()
        self.evidence["result"] = outcome.value
        self.evidence["error"] = error
        _atomic_json(self.config.evidence_path, self.evidence)

    def _run_with_client(self, client: BridgeHttpClient) -> RunOutcome:
        self._bind_receiver(client)
        for segment in self.segments:
            self.evidence["segments"].append(self._upload_one(client, segment))
        return RunOutcome.PASS

    def run(self) -> RunOutcome:
        bridge: LinkBridge | None = None
        try:
            if self.config.bridge_url:
                base_url = self.config.bridge_url
            else:
                assert self.config.pair_code is not None
                bridge = LinkBridge(
                    solstone_bin=self.config.solstone_bin,
                    pair_code=self.config.pair_code,
                    state_dir=self.config.state_dir,
                    carrier=self.config.carrier,
                    relay_url=self.config.relay_url,
                    startup_timeout=self.config.request_timeout,
                )
                base_url = bridge.start()
            client = BridgeHttpClient(base_url, timeout=self.config.request_timeout)
            outcome = self._run_with_client(client)
            self._write_evidence(outcome, None)
            if bridge:
                bridge.stop()
                if not self.config.keep_credentials:
                    bridge.remove_credentials()
                bridge = None
            return outcome
        except (ManifestError, SimulationFailure) as error:
            self._write_evidence(RunOutcome.FAIL, str(error))
            return RunOutcome.FAIL
        except LinkProcessError as error:
            self._write_evidence(RunOutcome.BLOCKED, str(error))
            return RunOutcome.BLOCKED
        except SimulationInconclusive as error:
            self._write_evidence(RunOutcome.INCONCLUSIVE, str(error))
            return RunOutcome.INCONCLUSIVE
        except HttpRequestError as error:
            self._write_evidence(RunOutcome.INCONCLUSIVE, str(error))
            return RunOutcome.INCONCLUSIVE
        finally:
            if bridge:
                bridge.stop()
