# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Build and validate the journal at-rest contract bundle.

Floor model
-----------
Each schema's ``$defs.header`` and ``$defs.record`` ``required`` arrays define
the universal at-rest floor every producer of that format must meet. ``raw`` is
not part of that floor. It is a producer-owned invariant emitted by the native
screen describer (``solstone-core-describe``) and the audio
transcriber (``solstone.observe.transcribe.main``), pinned by producer tests
rather than the shared floor.

Producers with no source media, such as a terminal/tmux observer, legitimately
omit ``raw`` and must still validate. ``raw`` remains in each schema's
``properties`` so a present value must type-check as a string, and it remains in
``key_fields``, but it is no longer ``required``.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator
from jsonschema.exceptions import SchemaError

ROOT = Path(__file__).resolve().parents[3]
ARTIFACT_PATH = ROOT / "solstone" / "talent" / "journal" / "contract" / "bundle.json"
LAYOUT_PATH = ROOT / "solstone" / "think" / "contract" / "layout.json"
CONTRACT_META = "x-journal-contract"
GENERATED_DIR = Path("solstone/talent/journal/contract")


@dataclass(frozen=True)
class ContractIssue:
    path: str
    message: str

    def __str__(self) -> str:
        return f"{self.path}: {self.message}" if self.path else self.message


def _repo_relative(path: Path, root: Path = ROOT) -> str:
    try:
        return path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return path.as_posix()


def _load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def _meta(schema: dict[str, Any], *, source: str = "<schema>") -> dict[str, Any]:
    meta = schema.get(CONTRACT_META)
    if not isinstance(meta, dict):
        raise ValueError(f"{source}: missing {CONTRACT_META}")
    required = {
        "format_id",
        "schema_owner",
        "reference_writer",
        "allowed_producers",
        "write_discipline",
        "file_kind",
        "key_fields",
    }
    missing = sorted(required - set(meta))
    if missing:
        raise ValueError(f"{source}: missing contract metadata: {', '.join(missing)}")
    if not isinstance(meta.get("allowed_producers"), list):
        raise ValueError(f"{source}: allowed_producers must be a list")
    if not isinstance(meta.get("key_fields"), list):
        raise ValueError(f"{source}: key_fields must be a list")
    return meta


def discover_schema_sources(root: Path = ROOT) -> list[Path]:
    """Return writer-adjacent contract schema files, excluding generated copies."""
    sources: list[Path] = []
    for path in root.glob("solstone/**/*.schema.json"):
        rel = path.relative_to(root)
        if rel.parts[:4] == tuple(GENERATED_DIR.parts):
            continue
        try:
            data = _load_json(path)
        except json.JSONDecodeError:
            continue
        if isinstance(data, dict) and isinstance(data.get(CONTRACT_META), dict):
            sources.append(path)
    return sorted(sources, key=lambda p: _repo_relative(p, root))


def _load_schema(path: Path) -> dict[str, Any]:
    data = _load_json(path)
    if not isinstance(data, dict):
        raise ValueError(f"{_repo_relative(path)}: schema must be a JSON object")
    try:
        Draft202012Validator.check_schema(data)
    except SchemaError as exc:
        raise ValueError(
            f"{_repo_relative(path)}: invalid JSON Schema: {exc.message}"
        ) from exc
    _meta(data, source=_repo_relative(path))
    return data


def build_bundle(root: Path = ROOT) -> dict[str, Any]:
    """Build the deterministic journal contract bundle object."""
    layout = _load_json(
        LAYOUT_PATH if root == ROOT else root / LAYOUT_PATH.relative_to(ROOT)
    )
    schemas: dict[str, dict[str, Any]] = {}
    for path in discover_schema_sources(root):
        schema = _load_schema(path)
        meta = _meta(schema, source=_repo_relative(path, root))
        format_id = str(meta["format_id"])
        if format_id in schemas:
            raise ValueError(f"{format_id}: duplicate journal contract format id")
        schemas[format_id] = {
            "source": _repo_relative(path, root),
            "schema": schema,
        }
    return {
        "contract": "solstone-journal-at-rest",
        "contract_version": 1,
        "generated_by": "python -m solstone.think.contract_cli build",
        "description": (
            "Generated journal at-rest contract bundle. Do not hand-edit; "
            "regenerate with `python -m solstone.think.contract_cli build`."
        ),
        "layout": layout,
        "schemas": {key: schemas[key] for key in sorted(schemas)},
    }


def render_bundle_json(bundle: dict[str, Any] | None = None) -> str:
    return json.dumps(bundle or build_bundle(), indent=2, sort_keys=True) + "\n"


def write_bundle(path: Path = ARTIFACT_PATH) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(render_bundle_json(), encoding="utf-8")


def check_artifact(path: Path = ARTIFACT_PATH) -> list[str]:
    expected = render_bundle_json()
    try:
        current = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        current = ""
    if current == expected:
        return []
    return [
        f"{_repo_relative(path)} is stale; run "
        "`python -m solstone.think.contract_cli build`"
    ]


def classify_breaking_changes(
    current: dict[str, Any], committed: dict[str, Any]
) -> list[str]:
    """Return human-readable breaking changes between two journal bundles."""
    breaking: list[str] = []
    current_schemas = current.get("schemas", {})
    committed_schemas = committed.get("schemas", {})
    if not isinstance(current_schemas, dict) or not isinstance(committed_schemas, dict):
        return ["journal contract bundle is malformed"]

    for format_id in sorted(set(committed_schemas) - set(current_schemas)):
        breaking.append(f"{format_id}: removed format")

    for format_id in sorted(set(committed_schemas) & set(current_schemas)):
        current_meta = _entry_meta(current_schemas[format_id])
        committed_meta = _entry_meta(committed_schemas[format_id])
        current_fields = set(_string_list(current_meta.get("key_fields")))
        committed_fields = set(_string_list(committed_meta.get("key_fields")))
        for field in sorted(committed_fields - current_fields):
            breaking.append(f"{format_id}: removed key field '{field}'")

        current_paths = set(_producer_paths(current_meta))
        committed_paths = set(_producer_paths(committed_meta))
        for path in sorted(committed_paths - current_paths):
            breaking.append(f"{format_id}: removed producer path '{path}'")
    return breaking


def _entry_meta(entry: Any) -> dict[str, Any]:
    if not isinstance(entry, dict):
        return {}
    schema = entry.get("schema", {})
    if not isinstance(schema, dict):
        return {}
    meta = schema.get(CONTRACT_META, {})
    return meta if isinstance(meta, dict) else {}


def _string_list(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, str)]


def _producer_paths(meta: dict[str, Any]) -> list[str]:
    values: list[str] = []
    for key in ("producer_write_paths", "produced_paths"):
        values.extend(_string_list(meta.get(key)))
    return values


def _schema_ref(schema: dict[str, Any], ref: str) -> dict[str, Any]:
    if not ref.startswith("#/$defs/"):
        raise ValueError(f"unsupported local schema ref: {ref}")
    name = ref.rsplit("/", 1)[-1]
    target = schema.get("$defs", {}).get(name)
    if not isinstance(target, dict):
        raise ValueError(f"missing schema definition: {ref}")
    return target


def _validator(schema: dict[str, Any], ref: str | None = None) -> Draft202012Validator:
    return Draft202012Validator(_schema_ref(schema, ref) if ref else schema)


def _format_errors(
    validator: Draft202012Validator,
    value: Any,
    *,
    path: str,
) -> list[ContractIssue]:
    issues: list[ContractIssue] = []
    for error in sorted(validator.iter_errors(value), key=lambda item: list(item.path)):
        location = ".".join(str(part) for part in error.path)
        label = f"{path}:{location}" if location else path
        issues.append(ContractIssue(label, error.message))
    return issues


def validate_contract_file(
    filename: str, content: bytes, schema: dict[str, Any]
) -> list[ContractIssue]:
    """Validate one normalized observer-produced file against a contract schema."""
    meta = _meta(schema)
    kind = str(meta.get("file_kind"))
    if kind == "json":
        try:
            value = json.loads(content.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            return [ContractIssue(filename, f"invalid JSON: {exc}")]
        return _format_errors(_validator(schema), value, path=filename)
    if kind == "headered_jsonl":
        return _validate_headered_jsonl(filename, content, schema)
    if kind == "ingest_envelope":
        try:
            value = json.loads(content.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            return [ContractIssue(filename, f"invalid JSON: {exc}")]
        return _format_errors(_validator(schema), value, path=filename)
    return [ContractIssue(filename, f"unsupported contract file kind: {kind}")]


def _validate_headered_jsonl(
    filename: str, content: bytes, schema: dict[str, Any]
) -> list[ContractIssue]:
    try:
        text = content.decode("utf-8")
    except UnicodeDecodeError as exc:
        return [ContractIssue(filename, f"invalid UTF-8: {exc}")]
    lines = [line for line in text.splitlines() if line.strip()]
    if not lines:
        return [ContractIssue(filename, "headered JSONL requires a header line")]

    issues: list[ContractIssue] = []
    header_validator = _validator(schema, "#/$defs/header")
    record_validator = _validator(schema, "#/$defs/record")
    for index, line in enumerate(lines, start=1):
        try:
            value = json.loads(line)
        except json.JSONDecodeError as exc:
            issues.append(
                ContractIssue(f"{filename}:{index}", f"invalid JSON: {exc.msg}")
            )
            continue
        if not isinstance(value, dict):
            issues.append(
                ContractIssue(f"{filename}:{index}", "line must be a JSON object")
            )
            continue
        if index == 1:
            issues.extend(
                _format_errors(header_validator, value, path=f"{filename}:{index}")
            )
        else:
            issues.extend(
                _format_errors(record_validator, value, path=f"{filename}:{index}")
            )
    return issues


def _bundle_schemas(bundle: dict[str, Any] | None = None) -> dict[str, dict[str, Any]]:
    bundle = bundle or build_bundle()
    entries = bundle.get("schemas", {})
    schemas: dict[str, dict[str, Any]] = {}
    if not isinstance(entries, dict):
        return schemas
    for format_id, entry in entries.items():
        if not isinstance(entry, dict):
            continue
        schema = entry.get("schema")
        if isinstance(format_id, str) and isinstance(schema, dict):
            schemas[format_id] = schema
    return schemas


def schema_for_filename(
    filename: str, bundle: dict[str, Any] | None = None
) -> dict[str, Any] | None:
    """Return the schema for a normalized observer-produced filename."""
    schemas = _bundle_schemas(bundle)
    if filename == "stream.json":
        return schemas.get("stream-json")
    if filename == "ingest.json":
        return schemas.get("observer-ingest-json")
    if filename == "audio.jsonl" or filename.endswith("_audio.jsonl"):
        return schemas.get("audio-jsonl")
    if filename == "screen.jsonl" or filename.endswith("_screen.jsonl"):
        return schemas.get("screen-jsonl")
    if filename.startswith("browser_") and filename.endswith(".jsonl"):
        return schemas.get("browser-jsonl")
    return None


def validate_journal_tree(
    journal: Path, bundle: dict[str, Any] | None = None
) -> list[ContractIssue]:
    """Validate contract-covered fixture or journal files under one journal root."""
    bundle = bundle or build_bundle()
    issues: list[ContractIssue] = []
    for path in sorted((journal / "chronicle").glob("*/*/*/*")):
        if not path.is_file():
            continue
        schema = schema_for_filename(path.name, bundle)
        if schema is None:
            continue
        rel = _repo_relative(path, journal)
        issues.extend(validate_contract_file(rel, path.read_bytes(), schema))
    return issues
