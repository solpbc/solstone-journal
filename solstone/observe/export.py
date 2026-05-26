# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Export journal data to a remote solstone instance.

Usage:
    journal export --to URL --key KEY [--only TYPE] [--dry-run] [--day YYYYMMDD]
    journal export --to LABEL [--only TYPE] [--dry-run] [--day YYYYMMDD]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import re
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import Any

import requests

from solstone.observe.peer_lookup import PeerInfo, PeerLookupError, resolve_peer
from solstone.observe.peer_unpair import maybe_prompt_unpair
from solstone.observe.pl_http import PlHttpSession
from solstone.observe.transfer import (
    RETRY_BACKOFF,
    _build_segment_manifest,
    _normalize_url,
    _parse_day_spec,
)
from solstone.think.entities.journal import load_all_journal_entities
from solstone.think.importers.sync import SYNCABLE_REGISTRY
from solstone.think.link.bundle import load_client_identity
from solstone.think.link.dialer import TunnelClient
from solstone.think.link.paths import relay_url
from solstone.think.utils import (
    day_path,
    get_config,
    get_journal,
    iter_segments,
    setup_cli,
)

logger = logging.getLogger(__name__)

UPLOAD_TIMEOUT = 300
_FACET_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_-]*$")
_DAY_RE = re.compile(r"^\d{8}$")
_DAY_JSONL_RE = re.compile(r"^\d{8}\.jsonl$")
_DAY_MD_RE = re.compile(r"^\d{8}\.md$")
_IMPORT_ID_RE = re.compile(r"^\d{8}_\d{6}$")
_NEVER_TRANSFER_PATHS = frozenset({"convey.password_hash", "convey.secret"})
EXPORT_AREAS = ("segments", "imports", "entities", "facets", "config")
FULL_EXPORT_SET = frozenset(EXPORT_AREAS)


@dataclass
class ExportResult:
    """Result of a single area export."""

    area: str = ""
    sent: int = 0
    skipped: int = 0
    staged: int = 0
    failed: int = 0
    errors: list[str] = field(default_factory=list)
    error: str | None = None


def _is_url_destination(to: str) -> bool:
    return to.startswith(("http://", "https://"))


def _resolve_destination(
    parser: argparse.ArgumentParser,
    to: str,
    key: str | None,
) -> tuple[str, str | None, PeerInfo | None]:
    if _is_url_destination(to):
        if key is None:
            parser.error("'--to <URL>' requires '--key <KEY>'")
        return ("dl", _normalize_url(to), None)
    if key is not None:
        parser.error(
            "'--key' is only valid with '--to <URL>'; "
            "use '--to <label>' for pl peer transfers"
        )
    try:
        return ("pl", None, resolve_peer(to))
    except PeerLookupError as exc:
        parser.error(str(exc))


def _parse_only_set(
    parser: argparse.ArgumentParser,
    raw: str | None,
) -> frozenset[str]:
    if raw is None:
        return FULL_EXPORT_SET
    areas = frozenset(part.strip() for part in raw.split(",") if part.strip())
    invalid = sorted(areas - FULL_EXPORT_SET)
    if invalid or not areas:
        parser.error(
            "--only must contain one or more of: " + ", ".join(sorted(FULL_EXPORT_SET))
        )
    return areas


def _query_manifest(
    session: requests.Session, base_url: str, key: str, area: str = "segments"
) -> dict[str, Any]:
    key_prefix = key[:8]
    url = f"{base_url}/app/import/journal/{key_prefix}/manifest/{area}"
    response = session.get(url, timeout=UPLOAD_TIMEOUT)
    if response.status_code == 401:
        raise ValueError("Authentication failed: invalid or missing API key")
    if response.status_code == 403:
        raise ValueError("Authentication failed: journal source revoked or disabled")
    if response.status_code != 200:
        raise ValueError(
            f"Manifest query failed: {response.status_code} {response.text}"
        )
    return response.json()


def _upload_segment(
    session: requests.Session,
    base_url: str,
    key: str,
    day: str,
    stream_name: str,
    segment_key: str,
    segment_path: Path,
) -> tuple[str, int]:
    files = [
        file_path
        for file_path in sorted(segment_path.iterdir())
        if file_path.is_file() and file_path.name != "stream.json"
    ]
    if not files:
        return ("skip", 0)

    bytes_sent = sum(file_path.stat().st_size for file_path in files)
    metadata = {
        "segments": [
            {
                "day": day,
                "stream": stream_name,
                "segment_key": segment_key,
                "files": [file_path.name for file_path in files],
            }
        ]
    }
    key_prefix = key[:8]
    url = f"{base_url}/app/import/journal/{key_prefix}/ingest/segments"

    for attempt, delay in enumerate(RETRY_BACKOFF):
        file_handles = []
        files_data = []
        try:
            for file_path in files:
                fh = open(file_path, "rb")
                file_handles.append(fh)
                files_data.append(
                    ("files_0", (file_path.name, fh, "application/octet-stream"))
                )

            response = session.post(
                url,
                data={"metadata": json.dumps(metadata)},
                files=files_data,
                timeout=UPLOAD_TIMEOUT,
            )
            if response.status_code == 200:
                return ("sent", bytes_sent)
            if response.status_code == 401:
                return ("auth_invalid", 0)
            if response.status_code == 403:
                return ("auth_revoked", 0)
            if 500 <= response.status_code <= 599:
                logger.warning(
                    "Upload attempt %s failed for %s/%s/%s: %s %s",
                    attempt + 1,
                    day,
                    stream_name,
                    segment_key,
                    response.status_code,
                    response.text,
                )
            else:
                logger.warning(
                    "Upload rejected for %s/%s/%s: %s %s",
                    day,
                    stream_name,
                    segment_key,
                    response.status_code,
                    response.text,
                )
                return ("error", 0)
        except (requests.RequestException, OSError) as e:
            logger.warning(
                "Upload attempt %s failed for %s/%s/%s: %s",
                attempt + 1,
                day,
                stream_name,
                segment_key,
                e,
            )
        finally:
            for fh in file_handles:
                try:
                    fh.close()
                except Exception:
                    pass

        if attempt < len(RETRY_BACKOFF) - 1:
            time.sleep(delay)

    return ("error", 0)


def _classify_facet_file(relative: PurePosixPath) -> str | None:
    """Classify a facet file by its relative path, returning the ingest type or None."""
    parts = relative.parts

    if parts == ("facet.json",):
        return "facet_json"

    if len(parts) >= 2 and parts[0] == "entities":
        if len(parts) == 3 and parts[2] == "entity.json":
            return "entity_relationship"
        if len(parts) == 3 and parts[2] == "observations.jsonl":
            return "entity_observations"
        if len(parts) == 2 and _DAY_JSONL_RE.match(parts[1]):
            return "detected_entities"
        return None

    if len(parts) >= 2 and parts[0] == "activities":
        if parts == ("activities", "activities.jsonl"):
            return "activity_config"
        if len(parts) == 2 and _DAY_JSONL_RE.match(parts[1]):
            return "activity_records"
        if len(parts) >= 4 and _DAY_RE.match(parts[1]):
            return "activity_output"
        return None

    if len(parts) == 2 and parts[0] == "todos" and _DAY_JSONL_RE.match(parts[1]):
        return "todos"

    if len(parts) == 2 and parts[0] == "news" and _DAY_MD_RE.match(parts[1]):
        return "news"

    if len(parts) == 2 and parts[0] == "logs" and _DAY_JSONL_RE.match(parts[1]):
        return "logs"

    return None


def _strip_never_transfer(config: dict) -> dict:
    """Return a deep copy of config with never-transfer fields removed."""
    import copy as _copy

    result = _copy.deepcopy(config)
    for path in _NEVER_TRANSFER_PATHS:
        parts = path.split(".")
        obj = result
        for part in parts[:-1]:
            if isinstance(obj, dict) and part in obj:
                obj = obj[part]
            else:
                break
        else:
            if isinstance(obj, dict):
                obj.pop(parts[-1], None)
    return result


def export_segments(
    base_url: str,
    key: str,
    days: list[str],
    dry_run: bool,
    session: requests.Session | None = None,
) -> ExportResult:
    own_session = session is None
    if own_session:
        session = requests.Session()
        session.headers["Authorization"] = f"Bearer {key}"

    result = ExportResult(area="segments")
    bytes_total = 0

    try:
        try:
            remote_manifest = _query_manifest(session, base_url, key)
        except requests.ConnectionError:
            result.error = f"Connection failed: could not reach {base_url}"
            print(result.error)
            return result
        except ValueError as e:
            result.error = str(e)
            print(result.error)
            return result

        for day in days:
            day_dir = day_path(day, create=False)
            if not day_dir.exists():
                continue

            segment_entries = iter_segments(day_dir)
            if not segment_entries:
                continue

            day_sent = 0
            day_bytes = 0

            for stream_name, seg_key, seg_path in segment_entries:
                manifest = _build_segment_manifest(seg_path)
                local_files = {
                    file_info["name"]: file_info["sha256"]
                    for file_info in manifest["files"]
                    if file_info["name"] != "stream.json"
                }
                if not local_files:
                    result.skipped += 1
                    continue

                remote_entry = remote_manifest.get(day, {}).get(
                    f"{stream_name}/{seg_key}", {}
                )
                remote_files = {
                    file_info["name"]: file_info["sha256"]
                    for file_info in remote_entry.get("files", [])
                }
                if local_files == remote_files:
                    result.skipped += 1
                    logger.info(f"  [skip] {day}/{stream_name}/{seg_key}")
                    continue

                if dry_run:
                    seg_bytes = sum(
                        file_info["size"]
                        for file_info in manifest["files"]
                        if file_info["name"] != "stream.json"
                    )
                    logger.info(f"  [would send] {day}/{stream_name}/{seg_key}")
                    day_sent += 1
                    day_bytes += seg_bytes
                    continue

                status, segment_bytes = _upload_segment(
                    session,
                    base_url,
                    key,
                    day,
                    stream_name,
                    seg_key,
                    seg_path,
                )
                if status == "sent":
                    logger.info(
                        f"  [sent] {day}/{stream_name}/{seg_key} ({segment_bytes} bytes)"
                    )
                    result.sent += 1
                    bytes_total += segment_bytes
                elif status == "skip":
                    result.skipped += 1
                elif status == "auth_invalid":
                    result.error = "Authentication failed: invalid or missing API key"
                    print(result.error)
                    return result
                elif status == "auth_revoked":
                    result.error = (
                        "Authentication failed: journal source revoked or disabled"
                    )
                    print(result.error)
                    return result
                else:
                    logger.info(f"  [FAILED] {day}/{stream_name}/{seg_key}")
                    result.failed += 1

            if dry_run:
                result.sent += day_sent
                if day_sent > 0:
                    print(f"  {day}: {day_sent} segment(s), {day_bytes} bytes")

        total = result.sent + result.skipped + result.failed
        if total == 0:
            print("No segments found to export")
            return result
        if dry_run:
            print(f"\nDry run: would send {result.sent}, skip {result.skipped}")
            return result

        print(
            f"\nExport complete: {result.sent} sent, {result.skipped} skipped, "
            f"{result.failed} failed, {bytes_total} bytes transferred"
        )
        if result.sent == 0 and result.skipped > 0 and result.failed == 0:
            print("Nothing to send - remote is up to date")
        return result
    finally:
        if own_session:
            session.close()


def export_entities(
    base_url: str,
    key: str,
    dry_run: bool,
    session: requests.Session | None = None,
) -> ExportResult:
    own_session = session is None
    if own_session:
        session = requests.Session()
        session.headers["Authorization"] = f"Bearer {key}"

    result = ExportResult(area="entities")

    try:
        try:
            remote_manifest = _query_manifest(session, base_url, key, area="entities")
        except requests.ConnectionError:
            result.error = f"Connection failed: could not reach {base_url}"
            print(result.error)
            return result
        except ValueError as e:
            result.error = str(e)
            print(result.error)
            return result

        received = remote_manifest.get("received", {})
        entities = load_all_journal_entities()
        if not entities:
            print("No entities found to export")
            return result

        new_count = 0
        changed_count = 0
        unchanged_count = 0
        to_send = []

        for entity_id, entity in entities.items():
            if entity.get("blocked"):
                continue

            content_hash = hashlib.sha256(
                json.dumps(entity, sort_keys=True, ensure_ascii=False).encode()
            ).hexdigest()
            if received.get(entity_id) == content_hash:
                unchanged_count += 1
                continue

            if entity_id in received:
                changed_count += 1
            else:
                new_count += 1
            to_send.append(entity)

        if dry_run:
            result.sent = len(to_send)
            result.skipped = unchanged_count
            print(
                f"Dry run: {new_count} new, {changed_count} changed, "
                f"{unchanged_count} unchanged"
            )
            return result

        if not to_send:
            result.skipped = unchanged_count
            print("Nothing to send - remote entities are up to date")
            return result

        key_prefix = key[:8]
        url = f"{base_url}/app/import/journal/{key_prefix}/ingest/entities"
        for attempt, delay in enumerate(RETRY_BACKOFF):
            try:
                response = session.post(
                    url, json={"entities": to_send}, timeout=UPLOAD_TIMEOUT
                )
                if response.status_code == 200:
                    break
                if response.status_code == 401:
                    result.error = "Authentication failed: invalid or missing API key"
                    print(result.error)
                    return result
                if response.status_code == 403:
                    result.error = (
                        "Authentication failed: journal source revoked or disabled"
                    )
                    print(result.error)
                    return result
                if 500 <= response.status_code <= 599:
                    logger.warning(
                        "Entity upload attempt %s failed: %s %s",
                        attempt + 1,
                        response.status_code,
                        response.text,
                    )
                else:
                    result.error = (
                        f"Entity upload failed: {response.status_code} {response.text}"
                    )
                    print(result.error)
                    return result
            except (requests.RequestException, OSError) as e:
                logger.warning("Entity upload attempt %s failed: %s", attempt + 1, e)
            if attempt < len(RETRY_BACKOFF) - 1:
                time.sleep(delay)
        else:
            result.error = "Entity upload failed after all retries"
            print(result.error)
            return result

        response_data = response.json()
        errors = [str(error) for error in response_data.get("errors", [])]
        result.sent = response_data.get("created", 0) + response_data.get(
            "auto_merged", 0
        )
        result.staged = response_data.get("staged", 0)
        result.skipped = response_data.get("skipped", 0)
        result.errors = errors
        if errors:
            for err in errors:
                print(f"  Error: {err}")
        print(
            f"\nExport complete: {response_data.get('created', 0)} created, "
            f"{response_data.get('auto_merged', 0)} merged, "
            f"{response_data.get('staged', 0)} staged, "
            f"{response_data.get('skipped', 0)} skipped"
        )
        if errors:
            print(f"  {len(errors)} error(s)")
        return result
    finally:
        if own_session:
            session.close()


def export_facets(
    base_url: str,
    key: str,
    dry_run: bool,
    session: requests.Session | None = None,
) -> ExportResult:
    own_session = session is None
    if own_session:
        session = requests.Session()
        session.headers["Authorization"] = f"Bearer {key}"

    result = ExportResult(area="facets")

    try:
        try:
            remote_manifest = _query_manifest(session, base_url, key, area="facets")
        except requests.ConnectionError:
            result.error = f"Connection failed: could not reach {base_url}"
            print(result.error)
            return result
        except ValueError as e:
            result.error = str(e)
            print(result.error)
            return result

        received = remote_manifest.get("received", {})

        facets_dir = Path(get_journal()) / "facets"
        if not facets_dir.is_dir():
            print("No facets found to export")
            return result

        facet_names = sorted(
            d.name
            for d in facets_dir.iterdir()
            if d.is_dir()
            and _FACET_NAME_RE.match(d.name)
            and (d / "facet.json").is_file()
        )
        if not facet_names:
            print("No facets found to export")
            return result

        total_new = 0
        total_changed = 0
        total_unchanged = 0
        total_facets_sent = 0
        total_facets_failed = 0
        total_facets_skipped = 0
        total_errors = 0

        key_prefix = key[:8]
        url = f"{base_url}/app/import/journal/{key_prefix}/ingest/facets"

        for facet_name in facet_names:
            facet_path = facets_dir / facet_name

            classified_files = []
            for abs_path in sorted(
                facet_path.rglob("*"), key=lambda path: path.as_posix()
            ):
                if not abs_path.is_file():
                    continue
                relative = PurePosixPath(abs_path.relative_to(facet_path))
                file_type = _classify_facet_file(relative)
                if file_type is None:
                    continue
                classified_files.append((str(relative), file_type, abs_path))

            if not classified_files:
                continue

            new_files = []
            changed_files = []
            unchanged_count = 0

            for rel_path, file_type, abs_path in classified_files:
                content_hash = hashlib.sha256(abs_path.read_bytes()).hexdigest()
                manifest_key = f"{facet_name}/{rel_path}"
                remote_hash = received.get(manifest_key)
                if remote_hash == content_hash:
                    unchanged_count += 1
                elif remote_hash is not None:
                    changed_files.append((rel_path, file_type, abs_path))
                else:
                    new_files.append((rel_path, file_type, abs_path))

            to_send = new_files + changed_files
            total_new += len(new_files)
            total_changed += len(changed_files)
            total_unchanged += unchanged_count

            if not to_send:
                total_facets_skipped += 1
                continue

            if dry_run:
                print(
                    f"  {facet_name}: {len(new_files)} new, "
                    f"{len(changed_files)} changed, {unchanged_count} unchanged"
                )
                total_facets_sent += 1
                continue

            metadata = {
                "facets": [
                    {
                        "name": facet_name,
                        "files": [
                            {"path": rel_path, "type": file_type}
                            for rel_path, file_type, _ in to_send
                        ],
                    }
                ]
            }

            for attempt, delay in enumerate(RETRY_BACKOFF):
                file_handles = []
                files_data = []
                try:
                    for file_idx, (_, _, abs_path) in enumerate(to_send):
                        fh = open(abs_path, "rb")
                        file_handles.append(fh)
                        files_data.append(
                            (
                                f"files_0_{file_idx}",
                                (abs_path.name, fh, "application/octet-stream"),
                            )
                        )

                    response = session.post(
                        url,
                        data={"metadata": json.dumps(metadata)},
                        files=files_data,
                        timeout=UPLOAD_TIMEOUT,
                    )
                    if response.status_code == 200:
                        response_data = response.json()
                        errors = [
                            str(error) for error in response_data.get("errors", [])
                        ]
                        result.errors.extend(errors)
                        total_errors += len(errors)
                        if errors:
                            for err in errors:
                                print(f"  Error ({facet_name}): {err}")
                        logger.info(
                            "  [sent] %s: %s created, %s merged, %s staged",
                            facet_name,
                            response_data.get("created", 0),
                            response_data.get("merged", 0),
                            response_data.get("staged", 0),
                        )
                        total_facets_sent += 1
                        break
                    if response.status_code == 401:
                        result.error = (
                            "Authentication failed: invalid or missing API key"
                        )
                        print(result.error)
                        return result
                    if response.status_code == 403:
                        result.error = (
                            "Authentication failed: journal source revoked or disabled"
                        )
                        print(result.error)
                        return result
                    if 500 <= response.status_code <= 599:
                        logger.warning(
                            "Facet upload attempt %s failed for %s: %s %s",
                            attempt + 1,
                            facet_name,
                            response.status_code,
                            response.text,
                        )
                    else:
                        logger.warning(
                            "Facet upload rejected for %s: %s %s",
                            facet_name,
                            response.status_code,
                            response.text,
                        )
                        total_facets_failed += 1
                        break
                except (requests.RequestException, OSError) as e:
                    logger.warning(
                        "Facet upload attempt %s failed for %s: %s",
                        attempt + 1,
                        facet_name,
                        e,
                    )
                finally:
                    for fh in file_handles:
                        try:
                            fh.close()
                        except Exception:
                            pass

                if attempt < len(RETRY_BACKOFF) - 1:
                    time.sleep(delay)
            else:
                logger.warning(
                    "Facet upload failed after all retries for %s", facet_name
                )
                total_facets_failed += 1

        if dry_run:
            if total_new + total_changed == 0:
                print("Nothing to send - remote facets are up to date")
            else:
                print(
                    f"\nDry run: {total_new} new files, {total_changed} changed, "
                    f"{total_unchanged} unchanged across {total_facets_sent} facet(s)"
                )
            result.sent = total_facets_sent
            result.skipped = total_facets_skipped
            result.failed = total_facets_failed
            return result

        if total_facets_sent == 0 and total_facets_failed == 0:
            result.skipped = total_facets_skipped
            print("Nothing to send - remote facets are up to date")
            return result

        print(
            f"\nFacet export complete: {total_facets_sent} sent, "
            f"{total_facets_skipped} skipped, {total_facets_failed} failed"
        )
        if total_errors:
            print(f"  {total_errors} error(s)")
        result.sent = total_facets_sent
        result.skipped = total_facets_skipped
        result.failed = total_facets_failed
        return result
    finally:
        if own_session:
            session.close()


def export_imports(
    base_url: str,
    key: str,
    dry_run: bool,
    session: requests.Session | None = None,
) -> ExportResult:
    """Export import metadata to a remote solstone instance."""
    own_session = session is None
    if own_session:
        session = requests.Session()
        session.headers["Authorization"] = f"Bearer {key}"

    result = ExportResult(area="imports")

    try:
        try:
            remote_manifest = _query_manifest(session, base_url, key, area="imports")
        except requests.ConnectionError:
            result.error = f"Connection failed: could not reach {base_url}"
            print(result.error)
            return result
        except ValueError as e:
            result.error = str(e)
            print(result.error)
            return result

        received = remote_manifest.get("received", {})

        journal_root = Path(get_journal())
        imports_dir = journal_root / "imports"
        if not imports_dir.is_dir():
            print("No imports directory found")
            return result

        sync_state_names = {f"{name}.json" for name in SYNCABLE_REGISTRY}

        to_send = []
        new_count = 0
        changed_count = 0
        unchanged_count = 0

        for entry in sorted(imports_dir.iterdir()):
            if entry.is_file() and entry.name in sync_state_names:
                continue
            if not entry.is_dir():
                continue
            if not _IMPORT_ID_RE.match(entry.name):
                continue

            import_id = entry.name
            import_json_path = entry / "import.json"
            imported_json_path = entry / "imported.json"
            manifest_path = entry / "content_manifest.jsonl"

            if not import_json_path.exists() or not imported_json_path.exists():
                continue

            try:
                import_json = json.loads(import_json_path.read_text(encoding="utf-8"))
                imported_json = json.loads(
                    imported_json_path.read_text(encoding="utf-8")
                )
                content_manifest = []
                if manifest_path.exists():
                    for line in manifest_path.read_text(encoding="utf-8").splitlines():
                        line = line.strip()
                        if line:
                            content_manifest.append(json.loads(line))
            except (json.JSONDecodeError, OSError) as exc:
                logger.warning("Failed to read import %s: %s", import_id, exc)
                continue

            hash_input = json.dumps(
                {
                    "import_json": import_json,
                    "imported_json": imported_json,
                    "content_manifest": content_manifest,
                },
                sort_keys=True,
                ensure_ascii=False,
            ).encode()
            content_hash = hashlib.sha256(hash_input).hexdigest()

            if received.get(import_id) == content_hash:
                unchanged_count += 1
                continue

            if import_id in received:
                changed_count += 1
            else:
                new_count += 1

            to_send.append(
                {
                    "id": import_id,
                    "import_json": import_json,
                    "imported_json": imported_json,
                    "content_manifest": content_manifest,
                }
            )

        if dry_run:
            result.sent = len(to_send)
            result.skipped = unchanged_count
            print(
                f"Dry run: {new_count} new, {changed_count} changed, "
                f"{unchanged_count} unchanged"
            )
            return result

        if not to_send:
            result.skipped = unchanged_count
            print("Nothing to send - remote imports are up to date")
            return result

        key_prefix = key[:8]
        url = f"{base_url}/app/import/journal/{key_prefix}/ingest/imports"
        for attempt, delay in enumerate(RETRY_BACKOFF):
            try:
                response = session.post(
                    url, json={"imports": to_send}, timeout=UPLOAD_TIMEOUT
                )
                if response.status_code == 200:
                    break
                if response.status_code == 401:
                    result.error = "Authentication failed: invalid or missing API key"
                    print(result.error)
                    return result
                if response.status_code == 403:
                    result.error = (
                        "Authentication failed: journal source revoked or disabled"
                    )
                    print(result.error)
                    return result
                if 500 <= response.status_code <= 599:
                    logger.warning(
                        "Import upload attempt %s failed: %s %s",
                        attempt + 1,
                        response.status_code,
                        response.text,
                    )
                else:
                    result.error = (
                        f"Import upload failed: {response.status_code} {response.text}"
                    )
                    print(result.error)
                    return result
            except (requests.RequestException, OSError) as e:
                logger.warning("Import upload attempt %s failed: %s", attempt + 1, e)
            if attempt < len(RETRY_BACKOFF) - 1:
                time.sleep(delay)
        else:
            result.error = "Import upload failed after all retries"
            print(result.error)
            return result

        response_data = response.json()
        errors = [str(error) for error in response_data.get("errors", [])]
        result.sent = response_data.get("copied", 0)
        result.staged = response_data.get("staged", 0)
        result.skipped = response_data.get("skipped", 0)
        result.errors = errors
        if errors:
            for err in errors:
                print(f"  Error: {err}")
        print(
            f"\nExport complete: {response_data.get('copied', 0)} copied, "
            f"{response_data.get('staged', 0)} staged, "
            f"{response_data.get('skipped', 0)} skipped"
        )
        if errors:
            print(f"  {len(errors)} error(s)")
        return result
    finally:
        if own_session:
            session.close()


def export_config(
    base_url: str,
    key: str,
    dry_run: bool,
    session: requests.Session | None = None,
) -> ExportResult:
    """Export config snapshot to a remote solstone instance."""
    own_session = session is None
    if own_session:
        session = requests.Session()
        session.headers["Authorization"] = f"Bearer {key}"

    result = ExportResult(area="config")

    try:
        try:
            remote_manifest = _query_manifest(session, base_url, key, area="config")
        except requests.ConnectionError:
            result.error = f"Connection failed: could not reach {base_url}"
            print(result.error)
            return result
        except ValueError as e:
            result.error = str(e)
            print(result.error)
            return result

        config = _strip_never_transfer(get_config())
        content_hash = hashlib.sha256(
            json.dumps(config, sort_keys=True, ensure_ascii=False).encode()
        ).hexdigest()

        if remote_manifest.get("last_hash") == content_hash:
            result.skipped = 1
            print("Nothing to send - remote config is up to date")
            return result

        if dry_run:
            result.staged = 1
            print("Dry run: config has changed, would send snapshot")
            return result

        key_prefix = key[:8]
        url = f"{base_url}/app/import/journal/{key_prefix}/ingest/config"
        for attempt, delay in enumerate(RETRY_BACKOFF):
            try:
                response = session.post(
                    url, json={"config": config}, timeout=UPLOAD_TIMEOUT
                )
                if response.status_code == 200:
                    break
                if response.status_code == 401:
                    result.error = "Authentication failed: invalid or missing API key"
                    print(result.error)
                    return result
                if response.status_code == 403:
                    result.error = (
                        "Authentication failed: journal source revoked or disabled"
                    )
                    print(result.error)
                    return result
                if 500 <= response.status_code <= 599:
                    logger.warning(
                        "Config upload attempt %s failed: %s %s",
                        attempt + 1,
                        response.status_code,
                        response.text,
                    )
                else:
                    result.error = (
                        f"Config upload failed: {response.status_code} {response.text}"
                    )
                    print(result.error)
                    return result
            except (requests.RequestException, OSError) as e:
                logger.warning("Config upload attempt %s failed: %s", attempt + 1, e)
            if attempt < len(RETRY_BACKOFF) - 1:
                time.sleep(delay)
        else:
            result.error = "Config upload failed after all retries"
            print(result.error)
            return result

        result_data = response.json()
        if result_data.get("staged"):
            result.staged = 1
            print(
                f"\nExport complete: config staged ({result_data.get('diff_fields', 0)} fields differ)"
            )
        elif result_data.get("skipped"):
            result.skipped = 1
            print("Nothing to send - remote config is up to date")
        return result
    finally:
        if own_session:
            session.close()


def _run_export_areas(
    base_url: str,
    key: str,
    days: list[str],
    dry_run: bool,
    session: requests.Session | PlHttpSession,
    areas: frozenset[str],
) -> list[ExportResult]:
    exporters = {
        "segments": lambda: export_segments(
            base_url,
            key,
            days,
            dry_run,
            session=session,
        ),
        "imports": lambda: export_imports(
            base_url,
            key,
            dry_run,
            session=session,
        ),
        "entities": lambda: export_entities(
            base_url,
            key,
            dry_run,
            session=session,
        ),
        "facets": lambda: export_facets(
            base_url,
            key,
            dry_run,
            session=session,
        ),
        "config": lambda: export_config(
            base_url,
            key,
            dry_run,
            session=session,
        ),
    }

    results: list[ExportResult] = []
    for area_name in EXPORT_AREAS:
        if area_name not in areas:
            continue
        try:
            results.append(exporters[area_name]())
        except Exception:
            logger.exception("Export failed for %s", area_name)
            results.append(
                ExportResult(
                    area=area_name,
                    error=f"Exception during {area_name} export",
                )
            )
            if area_name == "entities":
                print(
                    "  Warning: entity export failed - "
                    "facet entity mapping may be incomplete"
                )
    return results


def _print_export_summary(results: list[ExportResult]) -> bool:
    print("\n--- Export Summary ---")
    any_failed = False
    for area_result in results:
        if area_result.error:
            print(f"  {area_result.area}: FAILED ({area_result.error})")
            any_failed = True
            continue

        parts = []
        if area_result.sent:
            parts.append(f"{area_result.sent} sent")
        if area_result.skipped:
            parts.append(f"{area_result.skipped} skipped")
        if area_result.staged:
            parts.append(f"{area_result.staged} staged")
        if area_result.failed:
            parts.append(f"{area_result.failed} failed")
            any_failed = True
        if area_result.errors:
            parts.append(f"{len(area_result.errors)} error(s)")
        if not parts:
            parts.append("nothing to send")
        print(f"  {area_result.area}: {', '.join(parts)}")
    return any_failed


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Export journal data to a remote solstone instance"
    )
    parser.add_argument(
        "--to",
        required=True,
        help="Remote URL (http:// or https://) or paired peer label",
    )
    parser.add_argument(
        "--key",
        required=False,
        default=None,
        help="API key for URL mode",
    )
    parser.add_argument(
        "--only",
        default=None,
        help="Export only specific area (segments, entities, facets, imports, config)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be exported without sending",
    )
    parser.add_argument(
        "--day",
        default=None,
        help="Day or range (YYYYMMDD or YYYYMMDD-YYYYMMDD)",
    )
    args = setup_cli(parser)

    areas = _parse_only_set(parser, args.only)
    mode, base_url, peer = _resolve_destination(parser, args.to, args.key)
    try:
        days = _parse_day_spec(args.day, Path(get_journal()))
    except ValueError as exc:
        parser.error(str(exc))

    if mode == "dl":
        assert base_url is not None and args.key is not None
        session = requests.Session()
        session.headers["Authorization"] = f"Bearer {args.key}"
        try:
            results = _run_export_areas(
                base_url,
                args.key,
                days,
                args.dry_run,
                session,
                areas,
            )
        finally:
            session.close()
        any_failed = _print_export_summary(results)
        if any_failed:
            sys.exit(1)
        return

    assert peer is not None
    identity = load_client_identity(peer.dir)
    with TunnelClient(identity, relay_url=relay_url()) as tunnel:
        session = PlHttpSession(tunnel)
        results = _run_export_areas(
            "https://pl.peer",
            peer.instance_id,
            days,
            args.dry_run,
            session,
            areas,
        )
        any_failed = _print_export_summary(results)
        # Prompt fires after any successful full migration: explicit --only
        # and the no-flag default both execute the full area set.
        if not args.dry_run and areas == FULL_EXPORT_SET and not any_failed:
            maybe_prompt_unpair(peer, session)

    if any_failed:
        sys.exit(1)
