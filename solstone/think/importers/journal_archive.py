# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Validator and importer for exported journal archives."""

from __future__ import annotations

import datetime as dt
import json
import logging
import os
import re
import shutil
import zipfile
from contextlib import contextmanager
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterator

from solstone.think.callosum import callosum_send
from solstone.think.entities.journal import get_journal_principal
from solstone.think.importers.file_importer import ImportPreview, ImportResult
from solstone.think.merge import ProgressCallback, merge_journals

logger = logging.getLogger(__name__)

DATE_RE = re.compile(r"^\d{8}$")
JOURNAL_ROOT_ENTRIES = {"chronicle", "entities", "facets", "imports", "_export.json"}
MANIFEST_FIELDS = (
    "solstone_version",
    "exported_at",
    "source_journal",
    "day_count",
    "entity_count",
    "facet_count",
)
SYMLINK_TYPE = 0xA000
SYMLINK_MASK = 0xF000
TEMP_EXTRACT_GLOB = "solstone-merge-*"
TEMP_EXTRACT_ROOT = Path("/var/tmp")


@dataclass
class ArchiveWarning:
    code: str
    message: str


@dataclass
class ArchiveValidation:
    ok: bool
    archive_path: Path
    root_prefix: str
    manifest: dict[str, Any] | None
    warnings: list[ArchiveWarning] = field(default_factory=list)
    day_count: int = 0
    entity_count: int = 0
    facet_count: int = 0


class MergeLockError(RuntimeError):
    """Raised when the journal merge/import lock cannot be acquired."""

    def __init__(self, pid: int | None, message: str):
        super().__init__(message)
        self.pid = pid


def _visible_name(name: str) -> str | None:
    parts = [part for part in name.split("/") if part]
    if not parts:
        return None
    if parts[0] == "__MACOSX":
        return None
    if parts[-1] == ".DS_Store":
        return None
    return "/".join(parts)


def _top_level_names(names: list[str]) -> set[str]:
    return {name.split("/", 1)[0] for name in names if name}


def _resolve_root_prefix(names: list[str]) -> str | None:
    top_level = _top_level_names(names)
    if top_level & JOURNAL_ROOT_ENTRIES:
        return ""

    top_level_dirs = {
        name for name in top_level if any(item.startswith(f"{name}/") for item in names)
    }
    if len(top_level_dirs) != 1:
        return None

    wrapper = next(iter(top_level_dirs))
    nested = []
    prefix = f"{wrapper}/"
    for name in names:
        if name.startswith(prefix):
            stripped = name[len(prefix) :]
            if stripped:
                nested.append(stripped)
    if _top_level_names(nested) & JOURNAL_ROOT_ENTRIES:
        return prefix
    return None


def _scan_counts(names: list[str], root_prefix: str) -> tuple[int, int, int]:
    day_dirs: set[str] = set()
    entity_slugs: set[str] = set()
    facet_slugs: set[str] = set()

    for name in names:
        if root_prefix and not name.startswith(root_prefix):
            continue
        relative_name = name[len(root_prefix) :] if root_prefix else name
        parts = relative_name.split("/")
        if len(parts) >= 2 and parts[0] == "chronicle" and DATE_RE.match(parts[1]):
            day_dirs.add(parts[1])
        if len(parts) == 3 and parts[0] == "entities" and parts[2] == "entity.json":
            entity_slugs.add(parts[1])
        if len(parts) == 3 and parts[0] == "facets" and parts[2] == "facet.json":
            facet_slugs.add(parts[1])

    return len(day_dirs), len(entity_slugs), len(facet_slugs)


def _is_symlink_entry(info: zipfile.ZipInfo) -> bool:
    return ((info.external_attr >> 16) & SYMLINK_MASK) == SYMLINK_TYPE


def _has_unsafe_path(name: str) -> bool:
    entry_path = Path(name)
    return entry_path.is_absolute() or ".." in entry_path.parts


def _build_fatal(
    archive_path: Path,
    code: str,
    message: str,
    *,
    warnings: list[ArchiveWarning] | None = None,
) -> ArchiveValidation:
    all_warnings = list(warnings or [])
    all_warnings.append(ArchiveWarning(code=code, message=message))
    return ArchiveValidation(
        ok=False,
        archive_path=archive_path,
        root_prefix="",
        manifest=None,
        warnings=all_warnings,
    )


def validate_journal_archive(
    path: Path,
    *,
    max_size_bytes: int = 50 * 1024**3,
) -> ArchiveValidation:
    archive_path = path.expanduser().resolve()
    warnings: list[ArchiveWarning] = []

    if not archive_path.exists():
        return _build_fatal(
            archive_path,
            "archive-not-found",
            "Archive file does not exist.",
        )

    if archive_path.stat().st_size > max_size_bytes:
        return _build_fatal(
            archive_path,
            "archive-too-large",
            "Archive exceeds 50 GiB safety limit.",
        )

    if not zipfile.is_zipfile(archive_path):
        return _build_fatal(
            archive_path,
            "archive-invalid-zip",
            "Archive is not a readable ZIP file.",
        )

    try:
        with zipfile.ZipFile(archive_path, "r") as archive:
            infos = archive.infolist()
            if any(info.flag_bits & 0x1 for info in infos):
                return _build_fatal(
                    archive_path,
                    "archive-encrypted",
                    "Encrypted ZIP entries are not supported.",
                )

            visible_names = [
                name
                for info in infos
                if (name := _visible_name(info.filename)) is not None
            ]
            root_prefix = _resolve_root_prefix(visible_names)
            if root_prefix is None:
                return _build_fatal(
                    archive_path,
                    "archive-structure-invalid",
                    "Archive does not contain a recognizable journal root.",
                    warnings=warnings,
                )

            for info in infos:
                if _has_unsafe_path(info.filename) or _is_symlink_entry(info):
                    return _build_fatal(
                        archive_path,
                        "archive-unsafe-path",
                        f"unsafe entry: {info.filename}",
                        warnings=warnings,
                    )

            day_count, entity_count, facet_count = _scan_counts(
                visible_names, root_prefix
            )
            manifest: dict[str, Any] | None = None
            manifest_name = f"{root_prefix}_export.json"
            try:
                manifest_bytes = archive.read(manifest_name)
            except KeyError:
                warnings.append(
                    ArchiveWarning(
                        code="manifest-missing",
                        message="Manifest is missing optional export metadata.",
                    )
                )
            else:
                try:
                    manifest = json.loads(manifest_bytes.decode("utf-8"))
                except (UnicodeDecodeError, json.JSONDecodeError):
                    warnings.append(
                        ArchiveWarning(
                            code="manifest-unparseable",
                            message="Manifest could not be parsed as JSON.",
                        )
                    )
                    manifest = None

            if manifest is not None:
                missing_fields = [
                    field for field in MANIFEST_FIELDS if field not in manifest
                ]
                if missing_fields:
                    warnings.append(
                        ArchiveWarning(
                            code="manifest-fields-missing",
                            message=(
                                "Manifest is missing required export metadata fields: "
                                + ", ".join(missing_fields)
                            ),
                        )
                    )
                for field_name, actual_value in (
                    ("day_count", day_count),
                    ("entity_count", entity_count),
                    ("facet_count", facet_count),
                ):
                    manifest_value = manifest.get(field_name)
                    if (
                        isinstance(manifest_value, int)
                        and manifest_value != actual_value
                    ):
                        warnings.append(
                            ArchiveWarning(
                                code="manifest-count-mismatch",
                                message=(
                                    f"Manifest {field_name}={manifest_value} does not match "
                                    f"archive contents ({actual_value})."
                                ),
                            )
                        )

            has_chronicle = any(
                name == f"{root_prefix}chronicle"
                or name.startswith(f"{root_prefix}chronicle/")
                for name in visible_names
            )
            if not has_chronicle:
                warnings.append(
                    ArchiveWarning(
                        code="chronicle-missing",
                        message="Archive has no chronicle/ directory; treating as partial journal.",
                    )
                )

            return ArchiveValidation(
                ok=True,
                archive_path=archive_path,
                root_prefix=root_prefix,
                manifest=manifest,
                warnings=warnings,
                day_count=day_count,
                entity_count=entity_count,
                facet_count=facet_count,
            )
    except (OSError, RuntimeError, zipfile.BadZipFile, zipfile.LargeZipFile):
        return _build_fatal(
            archive_path,
            "archive-invalid-zip",
            "Archive is not a readable ZIP file.",
            warnings=warnings,
        )


def _validation_messages(validation: ArchiveValidation) -> list[str]:
    return [warning.message for warning in validation.warnings]


def _archive_day_range(archive_path: Path, root_prefix: str) -> tuple[str, str] | None:
    try:
        with zipfile.ZipFile(archive_path, "r") as archive:
            names = [
                name
                for info in archive.infolist()
                if (name := _visible_name(info.filename)) is not None
            ]
    except (OSError, RuntimeError, zipfile.BadZipFile, zipfile.LargeZipFile):
        return None

    days = sorted(
        {
            parts[1]
            for name in names
            if (relative_name := name[len(root_prefix) :] if root_prefix else name)
            and (parts := relative_name.split("/"))
            and len(parts) >= 2
            and parts[0] == "chronicle"
            and DATE_RE.match(parts[1])
        }
    )
    if not days:
        return None
    return (days[0], days[-1])


def _format_preview_summary(validation: ArchiveValidation) -> str:
    base = (
        f"{validation.day_count} days, {validation.entity_count} entities, "
        f"{validation.facet_count} facets"
    )
    if validation.warnings:
        return f"Journal archive: {base} ({len(validation.warnings)} warnings)"
    return f"Journal archive: {base}"


def _merge_artifact_paths(journal_root: Path) -> tuple[Path, Path]:
    run_id = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    artifact_root = journal_root.parent / f"{journal_root.name}.merge" / run_id
    return artifact_root / "decisions.jsonl", artifact_root / "staging"


def _collect_day_range(root: Path) -> tuple[str, str] | None:
    chronicle_dir = root / "chronicle"
    if not chronicle_dir.is_dir():
        return None
    days = sorted(
        entry.name
        for entry in chronicle_dir.iterdir()
        if entry.is_dir() and DATE_RE.match(entry.name)
    )
    if not days:
        return None
    return (days[0], days[-1])


def _format_merge_summary(summary: dict[str, Any], *, dry_run: bool) -> str:
    prefix = "Dry run merge" if dry_run else "Merged archive"
    return (
        f"{prefix}: {summary['segments_copied']} segments copied, "
        f"{summary['segments_skipped']} skipped, "
        f"{summary['entities_created']} entities created, "
        f"{summary['entities_merged']} merged, "
        f"{summary['entities_staged']} staged, "
        f"{summary['facets_created']} facets created, "
        f"{summary['facets_merged']} merged, "
        f"{summary['imports_copied']} imports copied"
    )


def _lock_message(pid: int | None) -> str:
    if pid is None:
        return "another journal merge is in progress"
    return f"another journal merge is in progress (pid {pid})"


def _read_lock_owner(lock_path: Path) -> tuple[int | None, dict[str, Any] | None]:
    try:
        payload = json.loads(lock_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None, None
    raw_pid = payload.get("pid")
    if isinstance(raw_pid, int):
        return raw_pid, payload
    if isinstance(raw_pid, str) and raw_pid.isdigit():
        return int(raw_pid), payload
    return None, payload


@contextmanager
def acquire_merge_lock(
    journal_root: Path,
    kind: str,
    import_id: str,
) -> Iterator[None]:
    """Acquire the journal merge/import lock using an O_EXCL lockfile."""

    lock_path = journal_root / ".merge.lock"
    payload = {
        "pid": os.getpid(),
        "started_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "kind": kind,
        "import_id": import_id,
    }

    for attempt in range(2):
        try:
            fd = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        except FileExistsError:
            pid, _ = _read_lock_owner(lock_path)
            if pid is None:
                raise MergeLockError(None, _lock_message(None))
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                try:
                    lock_path.unlink()
                except FileNotFoundError:
                    pass
                if attempt == 0:
                    continue
                raise MergeLockError(pid, _lock_message(pid))
            except OSError:
                raise MergeLockError(pid, _lock_message(pid))
            raise MergeLockError(pid, _lock_message(pid))
        else:
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as handle:
                    json.dump(payload, handle)
            except Exception:
                try:
                    lock_path.unlink()
                except FileNotFoundError:
                    pass
                raise
            break
    else:
        raise MergeLockError(None, _lock_message(None))

    try:
        yield
    finally:
        try:
            lock_path.unlink()
        except FileNotFoundError:
            pass


def sweep_stale_extract_dirs(max_age_seconds: int = 86400) -> int:
    """Remove stale journal-archive extraction directories under /var/tmp."""

    swept = 0
    now = dt.datetime.now(dt.timezone.utc).timestamp()
    for path in TEMP_EXTRACT_ROOT.glob(TEMP_EXTRACT_GLOB):
        if not path.is_dir():
            continue
        try:
            age_seconds = now - path.stat().st_mtime
        except OSError:
            continue
        if age_seconds <= max_age_seconds:
            continue
        shutil.rmtree(path, ignore_errors=True)
        if not path.exists():
            swept += 1
    return swept


class JournalArchiveImporter:
    name = "journal_archive"
    display_name = "Journal Archive"
    file_patterns = ["*.zip"]
    description = "Merge an exported journal archive into the current journal"

    def detect(self, path: Path) -> bool:
        if not path.is_file():
            return False
        if path.suffix.lower() != ".zip":
            return False
        return validate_journal_archive(path).ok

    def preview(self, path: Path) -> ImportPreview:
        validation = validate_journal_archive(path)
        if not validation.ok:
            messages = _validation_messages(validation)
            return ImportPreview(
                date_range=("", ""),
                item_count=0,
                entity_count=0,
                summary=messages[-1]
                if messages
                else "Journal archive validation failed",
            )

        date_range = _archive_day_range(validation.archive_path, validation.root_prefix)
        return ImportPreview(
            date_range=date_range or ("", ""),
            item_count=validation.day_count,
            entity_count=validation.entity_count,
            summary=_format_preview_summary(validation),
        )

    def process(
        self,
        path: Path,
        journal_root: Path,
        *,
        facet: str | None = None,
        import_id: str | None = None,
        progress_callback: Callable | None = None,
        dry_run: bool = False,
    ) -> ImportResult:
        del facet
        import_id = import_id or dt.datetime.now().strftime("%Y%m%d_%H%M%S")
        validation = validate_journal_archive(path)
        if not validation.ok:
            messages = _validation_messages(validation)
            summary = messages[-1] if messages else "Journal archive validation failed"
            raise ValueError(summary)

        with acquire_merge_lock(journal_root, "journal-archive-import", import_id):
            extracted_root, extract_dir = self._safe_extract(
                validation.archive_path, validation, import_id
            )
            try:
                principal_collision = self._check_principal_collision(extracted_root)
                progress = self._bridge_progress(progress_callback)
                log_path, staging_path = _merge_artifact_paths(journal_root)
                summary = merge_journals(
                    extracted_root,
                    journal_root,
                    dry_run=dry_run,
                    log_path=log_path,
                    staging_path=staging_path,
                    progress=progress,
                )
                merge_summary = asdict(summary)
                if not dry_run:
                    ok = callosum_send(
                        "supervisor",
                        "request",
                        cmd=["journal", "indexer", "--rescan-full"],
                    )
                    if not ok:
                        logger.warning(
                            "post-merge full reindex: callosum_send returned false"
                        )

                return ImportResult(
                    entries_written=summary.segments_copied,
                    entities_seeded=summary.entities_created,
                    files_created=[],
                    errors=list(summary.errors),
                    summary=_format_merge_summary(
                        merge_summary,
                        dry_run=dry_run,
                    ),
                    date_range=_collect_day_range(extracted_root),
                    merge_summary=merge_summary,
                    principal_collision=principal_collision,
                    merge_log_path=str(log_path),
                    merge_staging_path=str(staging_path),
                )
            finally:
                shutil.rmtree(extract_dir, ignore_errors=True)

    def _safe_extract(
        self,
        archive_path: Path,
        validation: ArchiveValidation,
        import_id: str,
    ) -> tuple[Path, Path]:
        temp_name = (
            f"solstone-merge-{import_id}-{os.getpid()}-"
            f"{int(dt.datetime.now(dt.timezone.utc).timestamp() * 1000)}"
        )
        extract_dir = TEMP_EXTRACT_ROOT / temp_name
        previous_umask = os.umask(0o077)
        try:
            extract_dir.mkdir(mode=0o700)
            extract_root = extract_dir / "journal"
            extract_root.mkdir(mode=0o700)
            resolved_root = extract_root.resolve()

            with zipfile.ZipFile(archive_path, "r") as archive:
                for info in archive.infolist():
                    visible_name = _visible_name(info.filename)
                    if visible_name is None:
                        continue
                    if validation.root_prefix:
                        if not visible_name.startswith(validation.root_prefix):
                            continue
                        relative_name = visible_name[len(validation.root_prefix) :]
                    else:
                        relative_name = visible_name
                    if not relative_name:
                        continue

                    target_path = (extract_root / relative_name).resolve()
                    try:
                        target_path.relative_to(resolved_root)
                    except ValueError as exc:
                        raise ImportError(f"unsafe path {visible_name}") from exc
                    if _has_unsafe_path(relative_name) or _is_symlink_entry(info):
                        raise ImportError(f"unsafe path {visible_name}")

                    if info.is_dir():
                        target_path.mkdir(parents=True, exist_ok=True, mode=0o700)
                        continue

                    target_path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                    with (
                        archive.open(info, "r") as source,
                        open(target_path, "wb") as dest,
                    ):
                        shutil.copyfileobj(source, dest)

            return extract_root, extract_dir
        finally:
            os.umask(previous_umask)

    def _check_principal_collision(
        self,
        extracted_root: Path,
    ) -> dict[str, str] | None:
        target_principal = get_journal_principal()
        if not target_principal:
            return None

        candidate_paths = sorted((extracted_root / "entities").glob("*/entity.json"))
        candidate_paths.extend(
            sorted((extracted_root / "facets").glob("*/entities/*/entity.json"))
        )
        for path in candidate_paths:
            try:
                entity = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if not entity.get("is_principal"):
                continue
            source_entity_id = str(entity.get("id") or path.parent.name)
            target_entity_id = str(target_principal.get("id") or "")
            if (
                source_entity_id
                and target_entity_id
                and source_entity_id != target_entity_id
            ):
                return {
                    "source_entity_id": source_entity_id,
                    "source_name": str(entity.get("name") or source_entity_id),
                    "target_entity_id": target_entity_id,
                    "target_name": str(
                        target_principal.get("name") or target_entity_id
                    ),
                }
            return None
        return None

    def _bridge_progress(
        self,
        progress_callback: Callable | None,
    ) -> ProgressCallback | None:
        if progress_callback is None:
            return None

        def _bridge(
            phase: str,
            completed: int,
            total: int | None,
            item_name: str | None,
        ) -> None:
            del item_name
            progress_callback(completed, total or 0, stage=phase)

        return _bridge


importer = JournalArchiveImporter()
