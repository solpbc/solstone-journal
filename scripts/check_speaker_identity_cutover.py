#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Guard the speaker-identity durable-state cutover.

The native speaker commands own ordinary writes to the guarded paths.  This
checker deliberately scans every Python file rather than exempting a subtree:
the only direct-writer exceptions are the role-verified fixture oracle and
the individually recorded entity-merge calls in the committed census.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import subprocess
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
CENSUS_RELATIVE_PATH = Path("scripts/speaker_identity_cutover_census.json")
ROLE_MARKER = "SPEAKER_IDENTITY_CUTOVER_ROLE"
FIXTURE_ROLE = "differential_fixture_oracle_builder"
ENTITY_MERGE_WRITER_ROLE = "entity_merge_writer"
RUNTIME_READER_ROLE = "runtime_reader"
RUNTIME_TRANSPORT_ROLE = "runtime_native_transport"
LEGACY_SHARED_WRITER_ROLE = "legacy_fixture_or_entity_merge_writer"
TEST_FIXTURE_SETUP_ROLE = "test_fixture_setup"

TARGETS = (
    "entities/<id>/voiceprints.npz",
    "entities/<id>/owner_centroid.npz",
    "awareness/owner_candidate.npz",
    "chronicle/**/talents/speaker_labels.json",
    "chronicle/**/talents/speaker_corrections.json",
    "speakers/identify-operations.jsonl",
    "speakers/backfill-operations.jsonl",
)

SEMANTIC_WRITERS: dict[str, tuple[str, ...]] = {
    "save_voiceprints_batch": (TARGETS[0],),
    "rewrite_voiceprint_metadata": (TARGETS[0],),
    "apply_entity_merge_voiceprint_inverse": (TARGETS[0],),
    "update_speaker_labels": (TARGETS[3],),
    "remap_speaker_corrections_for_entity_merge": (TARGETS[4],),
    "apply_entity_merge_segment_inverse": (TARGETS[3], TARGETS[4]),
    "_apply_speaker_label_inverse_file": (TARGETS[3],),
    "_apply_speaker_correction_inverse_file": (TARGETS[4],),
}
SEMANTIC_READERS = {
    "load_entity_voiceprints_file": TARGETS[0],
    "load_existing_voiceprint_keys": TARGETS[0],
    # This is deliberately listed: verify_speaker_verdict uses the shared
    # voiceprint normalizer as a read-only differential-oracle dependency.
    "normalize_embedding": TARGETS[0],
}
WRITER_NAMES = {
    "update_npz",
    "save_npz",
    "write_npz",
    "write_json",
    "atomic_write",
    "atomic_replace",
    "dump",
    "open",
    "unlink",
    "rmtree",
    "remove",
}
READER_NAMES = {"load_npz", "read_json", "load", "read_text", "read_bytes"}
PATH_WRITE_METHODS = {"write_text", "write_bytes", "unlink"}
PATH_READ_METHODS = {"read_text", "read_bytes"}
VOICEPRINT_PATH_HELPERS = {"voiceprint_file_path", "_entity_voiceprint_path"}
OWNER_CANDIDATE_PATH_HELPERS = {"_owner_candidate_path", "owner_candidate_path"}

# These writers construct their paths from a runtime segment directory, so the
# generic literal/path-helper resolver cannot identify their target.  They are
# the four definitions retained solely for the explicit entity-merge
# flow; keep the mapping at this exact function/callee granularity rather than
# widening a file-level exemption.
DEFINITION_WRITER_TARGETS = {
    ("<module>.update_speaker_labels", "write_json"): TARGETS[3],
    (
        "<module>.remap_speaker_corrections_for_entity_merge",
        "atomic_replace",
    ): TARGETS[4],
    ("<module>._apply_speaker_label_inverse_file", "write_json"): TARGETS[3],
    (
        "<module>._apply_speaker_correction_inverse_file",
        "atomic_replace",
    ): TARGETS[4],
}


@dataclass(frozen=True, order=True)
class CensusEntry:
    role: str
    target: str
    file: str
    function: str
    line: int
    callee: str
    fingerprint: str
    classification: str

    def identity(self) -> tuple[str, str, str, str, str, str]:
        return (
            self.role,
            self.target,
            self.file,
            self.function,
            self.callee,
            self.fingerprint,
        )

    def semantic_identity(self) -> tuple[str, str, str, str, str]:
        return (
            self.target,
            self.file,
            self.function,
            self.callee,
            self.fingerprint,
        )


@dataclass(frozen=True)
class Finding:
    path: str
    rule: str
    detail: str


def _tracked_files(root: Path, *, all_files: bool) -> list[Path]:
    if all_files:
        ignored = {".git", ".venv", "__pycache__", "target", "node_modules"}
        return sorted(
            path
            for path in root.rglob("*.py")
            if path.is_file() and not (set(path.relative_to(root).parts) & ignored)
        )
    result = subprocess.run(
        ["git", "ls-files", "*.py"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    source_files: list[Path] = []
    for line in result.stdout.splitlines():
        if not line:
            continue
        path = root / line
        if not path.is_file():
            continue
        if (
            line.startswith(("solstone/", "scripts/"))
            or line == "tests/verify_speaker_verdict.py"
        ):
            source_files.append(path)
    return sorted(source_files)


def _rel(root: Path, path: Path) -> str:
    return path.relative_to(root).as_posix()


def _dotted_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        parent = _dotted_name(node.value)
        return f"{parent}.{node.attr}" if parent else None
    return None


def _target_for_text(value: str) -> str | None:
    normalized = value.replace("\\\\", "/")
    if "voiceprints.npz" in normalized:
        return TARGETS[0]
    if "owner_centroid.npz" in normalized:
        return TARGETS[1]
    if "awareness/owner_candidate.npz" in normalized or normalized.endswith(
        "owner_candidate.npz"
    ):
        return TARGETS[2]
    if "speaker_labels.json" in normalized and "talents" in normalized:
        return TARGETS[3]
    if "speaker_corrections.json" in normalized and "talents" in normalized:
        return TARGETS[4]
    if "speakers/identify-operations.jsonl" in normalized:
        return TARGETS[5]
    if "speakers/backfill-operations.jsonl" in normalized:
        return TARGETS[6]
    return None


def _path_target(
    node: ast.AST | None,
    paths: dict[str, str],
    path_helpers: dict[str, str],
) -> str | None:
    if node is None:
        return None
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return _target_for_text(node.value)
    if isinstance(node, ast.Name):
        return paths.get(node.id)
    if isinstance(node, ast.BinOp) and isinstance(node.op, (ast.Div, ast.Add)):
        return _path_target(node.left, paths, path_helpers) or _path_target(
            node.right, paths, path_helpers
        )
    if isinstance(node, ast.JoinedStr):
        values = [
            value.value
            for value in node.values
            if isinstance(value, ast.Constant) and isinstance(value.value, str)
        ]
        return _target_for_text("".join(values))
    if isinstance(node, ast.Call):
        dotted = _dotted_name(node.func)
        if dotted:
            name = dotted.rsplit(".", 1)[-1]
            if name in path_helpers:
                return path_helpers[name]
            if name == "with_name" and node.args:
                return _path_target(node.args[0], paths, path_helpers)
        for argument in (*node.args, *(keyword.value for keyword in node.keywords)):
            target = _path_target(argument, paths, path_helpers)
            if target:
                return target
    if isinstance(node, ast.Attribute):
        return _path_target(node.value, paths, path_helpers)
    return None


def _first_target_argument(
    node: ast.Call,
    callee: str,
    paths: dict[str, str],
    path_helpers: dict[str, str],
) -> str | None:
    if callee == "dump":
        arguments = node.args[1:]
    elif callee == "open":
        mode = node.args[1] if len(node.args) > 1 else None
        if not (
            isinstance(mode, ast.Constant)
            and isinstance(mode.value, str)
            and any(flag in mode.value for flag in ("w", "a", "x", "+"))
        ):
            return None
        arguments = node.args[:1]
    else:
        arguments = node.args[:1]
    for argument in (*arguments, *(keyword.value for keyword in node.keywords)):
        target = _path_target(argument, paths, path_helpers)
        if target:
            return target
    return None


def _fingerprint(
    *,
    file: str,
    function: str,
    callee: str,
    target: str,
    classification: str,
    occurrence: int,
) -> str:
    payload = "\0".join(
        (file, function, callee, target, classification, str(occurrence))
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()[:16]


class _FileScanner(ast.NodeVisitor):
    def __init__(self, root: Path, path: Path) -> None:
        self.root = root
        self.path = path
        self.rel = _rel(root, path)
        self.aliases: dict[str, str] = {}
        self.path_scopes: list[dict[str, str]] = [{}]
        self.path_helpers: dict[str, str] = {
            name: TARGETS[0] for name in VOICEPRINT_PATH_HELPERS
        }
        self.path_helpers.update(
            {name: TARGETS[2] for name in OWNER_CANDIDATE_PATH_HELPERS}
        )
        self.function_stack: list[str] = ["<module>"]
        self.entries: list[CensusEntry] = []
        self.occurrences: dict[tuple[str, str, str, str], int] = {}
        self.unresolved: list[Finding] = []

    @property
    def function(self) -> str:
        return ".".join(self.function_stack)

    @property
    def paths(self) -> dict[str, str]:
        return self.path_scopes[-1]

    def _record(
        self, node: ast.Call, callee: str, target: str, classification: str
    ) -> None:
        role = (
            RUNTIME_READER_ROLE if classification == "read" else RUNTIME_TRANSPORT_ROLE
        )
        occurrence_key = (self.function, callee, target, classification)
        occurrence = self.occurrences.get(occurrence_key, 0) + 1
        self.occurrences[occurrence_key] = occurrence
        self.entries.append(
            CensusEntry(
                role=role,
                target=target,
                file=self.rel,
                function=self.function,
                line=node.lineno,
                callee=callee,
                fingerprint=_fingerprint(
                    file=self.rel,
                    function=self.function,
                    callee=callee,
                    target=target,
                    classification=classification,
                    occurrence=occurrence,
                ),
                classification=classification,
            )
        )

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            self.aliases[alias.asname or alias.name.rsplit(".", 1)[-1]] = alias.name

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        module = node.module or ""
        for alias in node.names:
            local = alias.asname or alias.name
            self.aliases[local] = f"{module}.{alias.name}" if module else alias.name

    def visit_Assign(self, node: ast.Assign) -> None:
        self._visit_assignment(node.targets, node.value)
        self.generic_visit(node)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        if node.value is not None:
            self._visit_assignment([node.target], node.value)
        self.generic_visit(node)

    def _visit_assignment(self, targets: list[ast.expr], value: ast.AST) -> None:
        target = _path_target(value, self.paths, self.path_helpers)
        dotted = _dotted_name(value)
        for assigned in targets:
            if not isinstance(assigned, ast.Name):
                continue
            if target:
                self.paths[assigned.id] = target
            if dotted and dotted in self.aliases:
                self.aliases[assigned.id] = self.aliases[dotted]
            elif dotted in SEMANTIC_WRITERS or dotted in SEMANTIC_READERS:
                self.aliases[assigned.id] = dotted

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.function_stack.append(node.name)
        self.path_scopes.append(self.paths.copy())
        self.generic_visit(node)
        self.path_scopes.pop()
        self.function_stack.pop()

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_Call(self, node: ast.Call) -> None:
        dotted = _dotted_name(node.func)
        callee = dotted.rsplit(".", 1)[-1] if dotted else "<dynamic>"
        resolved = self.aliases.get(dotted or "", self.aliases.get(callee, callee))
        resolved_name = resolved.rsplit(".", 1)[-1]

        if resolved_name in SEMANTIC_WRITERS:
            for target in SEMANTIC_WRITERS[resolved_name]:
                self._record(node, resolved_name, target, "direct_write")
        elif resolved_name in SEMANTIC_READERS:
            self._record(node, resolved_name, SEMANTIC_READERS[resolved_name], "read")
        else:
            method = callee if isinstance(node.func, ast.Attribute) else resolved_name
            is_path_writer = method in PATH_WRITE_METHODS
            is_path_reader = method in PATH_READ_METHODS
            is_writer = resolved_name in WRITER_NAMES or is_path_writer
            is_reader = resolved_name in READER_NAMES or is_path_reader
            if is_writer or is_reader:
                target_node: ast.AST | None
                if is_path_writer or is_path_reader:
                    target_node = (
                        node.func.value
                        if isinstance(node.func, ast.Attribute)
                        else None
                    )
                    target = _path_target(target_node, self.paths, self.path_helpers)
                else:
                    target = _first_target_argument(
                        node, resolved_name, self.paths, self.path_helpers
                    )
                target = target or DEFINITION_WRITER_TARGETS.get(
                    (self.function, resolved_name)
                )
                if target:
                    self._record(
                        node,
                        resolved_name,
                        target,
                        "direct_write" if is_writer else "read",
                    )
                elif is_writer and self._looks_like_guarded_path(node, is_path_writer):
                    self.unresolved.append(
                        Finding(
                            f"{self.rel}:{node.lineno}",
                            "unresolved-speaker-identity-writer",
                            resolved_name,
                        )
                    )
        self.generic_visit(node)

    def _looks_like_guarded_path(self, node: ast.Call, is_path_writer: bool) -> bool:
        if is_path_writer and isinstance(node.func, ast.Attribute):
            candidate = node.func.value
        elif node.args:
            candidate = (
                node.args[1]
                if _dotted_name(node.func) == "json.dump" and len(node.args) > 1
                else node.args[0]
            )
        else:
            return False
        return any(
            isinstance(child, ast.Constant)
            and isinstance(child.value, str)
            and any(
                token in child.value
                for token in (
                    "voiceprint",
                    "owner_centroid",
                    "owner_candidate",
                    "speaker_labels",
                    "speaker_corrections",
                    "identify-operations",
                    "backfill-operations",
                )
            )
            for child in ast.walk(candidate)
        )


def _module_role(tree: ast.AST) -> str | None:
    for statement in tree.body if isinstance(tree, ast.Module) else []:
        if not isinstance(statement, (ast.Assign, ast.AnnAssign)):
            continue
        targets = (
            statement.targets
            if isinstance(statement, ast.Assign)
            else [statement.target]
        )
        if not any(
            isinstance(target, ast.Name) and target.id == ROLE_MARKER
            for target in targets
        ):
            continue
        value = statement.value
        if isinstance(value, ast.Constant) and isinstance(value.value, str):
            return value.value
    return None


def _runtime_imports_modules(root: Path, files: list[Path], modules: set[str]) -> bool:
    for path in files:
        if not _rel(root, path).startswith("solstone/"):
            continue
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        except (OSError, UnicodeDecodeError, SyntaxError):
            continue
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                if any(
                    alias.name.rsplit(".", 1)[-1] in modules for alias in node.names
                ):
                    return True
            elif (
                isinstance(node, ast.ImportFrom)
                and (node.module or "").rsplit(".", 1)[-1] in modules
            ):
                return True
    return False


def _fixture_role_is_valid(root: Path, files: list[Path], role_paths: set[str]) -> bool:
    if len(role_paths) != 1:
        return False
    role_path = Path(next(iter(role_paths)))
    if role_path.parent != Path("scripts"):
        return False
    module = role_path.stem
    if _runtime_imports_modules(root, files, {module}):
        return False
    builder = root / "scripts/build_core_fixtures.py"
    makefile = root / "Makefile"
    if not builder.is_file() or not makefile.is_file():
        return False
    builder_text = builder.read_text(encoding="utf-8")
    make_text = makefile.read_text(encoding="utf-8")
    return (
        f"import {module}" in builder_text
        and "core-fixtures:" in make_text
        and "check-core-fixtures:" in make_text
        and "build_core_fixtures.py" in make_text
    )


def scan(
    root: Path, *, all_files: bool = False
) -> tuple[list[CensusEntry], list[Finding], dict[str, str]]:
    """Return detected target accesses, fail-closed writer findings, and module roles."""
    files = _tracked_files(root, all_files=all_files)
    entries: list[CensusEntry] = []
    findings: list[Finding] = []
    roles: dict[str, str] = {}
    for path in files:
        try:
            text = path.read_text(encoding="utf-8")
            tree = ast.parse(text, filename=str(path))
        except (OSError, UnicodeDecodeError, SyntaxError):
            continue
        role = _module_role(tree)
        if role:
            roles[_rel(root, path)] = role
        scanner = _FileScanner(root, path)
        scanner.visit(tree)
        entries.extend(scanner.entries)
        findings.extend(scanner.unresolved)
    fixture_paths = {path for path, role in roles.items() if role == FIXTURE_ROLE}
    if fixture_paths and not _fixture_role_is_valid(root, files, fixture_paths):
        findings.append(
            Finding(
                ", ".join(sorted(fixture_paths)),
                "invalid-differential-fixture-role",
                "role must be fixture-build-only and never runtime-imported",
            )
        )
    return (
        sorted(set(entries)),
        sorted(findings, key=lambda item: (item.path, item.rule)),
        roles,
    )


def _load_census(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        payload = json.load(handle)
    if payload.get("schema_version") != 1:
        raise ValueError("unsupported speaker identity census schema")
    if tuple(payload.get("targets", [])) != TARGETS:
        raise ValueError(
            "speaker identity census targets do not match the seven-path contract"
        )
    return payload


def _entry_from_json(value: dict[str, Any]) -> CensusEntry:
    return CensusEntry(
        role=value["role"],
        target=value["target"],
        file=value["file"],
        function=value["function"],
        line=int(value["line"]),
        callee=value["callee"],
        fingerprint=value["fingerprint"],
        classification=value["classification"],
    )


def _role_for_entry(
    entry: CensusEntry,
    roles: dict[str, str],
    expected: dict[tuple[str, str, str, str, str], CensusEntry],
) -> CensusEntry:
    expected_entry = expected.get(entry.semantic_identity())
    if expected_entry and expected_entry.role in {
        FIXTURE_ROLE,
        ENTITY_MERGE_WRITER_ROLE,
        LEGACY_SHARED_WRITER_ROLE,
        TEST_FIXTURE_SETUP_ROLE,
    }:
        return CensusEntry(**{**asdict(entry), "role": expected_entry.role})
    if (
        entry.classification == "direct_write"
        and entry.file in roles
        and roles[entry.file] == FIXTURE_ROLE
    ):
        return CensusEntry(**{**asdict(entry), "role": FIXTURE_ROLE})
    return entry


def _entity_merge_writer_entry_is_valid(entry: CensusEntry) -> bool:
    """Keep each temporary entity-merge write explicit and bounded."""
    allowed = {
        "solstone/think/entities/merge.py": {
            ("update_speaker_labels", TARGETS[3], "<module>._apply_segment_plan"),
            (
                "remap_speaker_corrections_for_entity_merge",
                TARGETS[4],
                "<module>._apply_segment_plan",
            ),
            (
                "apply_entity_merge_segment_inverse",
                TARGETS[3],
                "<module>._undo_segment_remaps",
            ),
            (
                "apply_entity_merge_segment_inverse",
                TARGETS[4],
                "<module>._undo_segment_remaps",
            ),
            ("save_voiceprints_batch", TARGETS[0], "<module>._commit_merge"),
            (
                "apply_entity_merge_voiceprint_inverse",
                TARGETS[0],
                "<module>._undo_voiceprints",
            ),
        },
        "solstone/apps/speakers/attribution.py": {
            ("write_json", TARGETS[3], "<module>.update_speaker_labels"),
            (
                "atomic_replace",
                TARGETS[4],
                "<module>.remap_speaker_corrections_for_entity_merge",
            ),
            (
                "_apply_speaker_label_inverse_file",
                TARGETS[3],
                "<module>.apply_entity_merge_segment_inverse",
            ),
            (
                "_apply_speaker_correction_inverse_file",
                TARGETS[4],
                "<module>.apply_entity_merge_segment_inverse",
            ),
            (
                "write_json",
                TARGETS[3],
                "<module>._apply_speaker_label_inverse_file",
            ),
            (
                "atomic_replace",
                TARGETS[4],
                "<module>._apply_speaker_correction_inverse_file",
            ),
        },
        "solstone/apps/entities/tests/test_merge_undo.py": {
            (
                "update_speaker_labels",
                TARGETS[3],
                "<module>._concurrent_speaker_shift_worker",
            ),
            (
                "write_bytes",
                TARGETS[0],
                "<module>._corrupt_preflight_owner",
            ),
            (
                "write_text",
                TARGETS[0],
                "<module>._corrupt_preflight_owner",
            ),
        },
    }
    return (entry.callee, entry.target, entry.function) in allowed.get(
        entry.file, set()
    )


def _legacy_shared_writer_entry_is_valid(entry: CensusEntry) -> bool:
    """Bound legacy direct writers to their fixture/entity-merge compatibility surface."""
    return (entry.callee, entry.target, entry.function) in {
        (
            "update_npz",
            TARGETS[0],
            "<module>.save_voiceprints_batch",
        ),
        (
            "update_npz",
            TARGETS[0],
            "<module>.rewrite_voiceprint_metadata",
        ),
        (
            "update_npz",
            TARGETS[0],
            "<module>.apply_entity_merge_voiceprint_inverse",
        ),
    } and entry.file == "solstone/think/entities/voiceprints.py"


def _test_fixture_setup_entry_is_valid(entry: CensusEntry) -> bool:
    """Allow only the named test-data seeder, never a test-file exemption."""
    return (
        entry.file == "solstone/apps/speakers/tests/test_voiceprint_refinement.py"
        and entry.callee == "save_voiceprints_batch"
        and entry.target == TARGETS[0]
        and entry.function == "<module>._write_voiceprints"
    )


def check(root: Path, census_path: Path, *, all_files: bool = False) -> list[Finding]:
    census = _load_census(census_path)
    expected_entries = [_entry_from_json(value) for value in census.get("entries", [])]
    expected_by_semantic_identity = {
        entry.semantic_identity(): entry for entry in expected_entries
    }
    live_entries, findings, roles = scan(root, all_files=all_files)
    live_entries = [
        _role_for_entry(entry, roles, expected_by_semantic_identity)
        for entry in live_entries
    ]

    for entry in expected_entries:
        if entry.role == ENTITY_MERGE_WRITER_ROLE and not _entity_merge_writer_entry_is_valid(entry):
            findings.append(
                Finding(
                    f"{entry.file}:{entry.line}",
                    "invalid-entity-merge-role",
                    f"{entry.callee} -> {entry.target}",
                )
            )
        elif (
            entry.role == LEGACY_SHARED_WRITER_ROLE
            and not _legacy_shared_writer_entry_is_valid(entry)
        ):
            findings.append(
                Finding(
                    f"{entry.file}:{entry.line}",
                    "invalid-legacy-shared-writer-role",
                    f"{entry.callee} -> {entry.target}",
                )
            )
        elif (
            entry.role == TEST_FIXTURE_SETUP_ROLE
            and not _test_fixture_setup_entry_is_valid(entry)
        ):
            findings.append(
                Finding(
                    f"{entry.file}:{entry.line}",
                    "invalid-test-fixture-setup-role",
                    f"{entry.callee} -> {entry.target}",
                )
            )

    expected_identities = {entry.identity(): entry for entry in expected_entries}
    live_identities = {entry.identity(): entry for entry in live_entries}
    for identity, entry in sorted(live_identities.items()):
        expected = expected_identities.get(identity)
        if expected is None:
            findings.append(
                Finding(
                    f"{entry.file}:{entry.line}",
                    "unexpected-speaker-identity-access",
                    f"{entry.classification} {entry.callee} -> {entry.target} ({entry.function})",
                )
            )
        elif (
            expected.role != entry.role
            or expected.classification != entry.classification
        ):
            findings.append(
                Finding(
                    f"{entry.file}:{entry.line}",
                    "speaker-identity-census-role-drift",
                    f"expected {expected.role}/{expected.classification}, got {entry.role}/{entry.classification}",
                )
            )
    for identity, entry in sorted(expected_identities.items()):
        if identity not in live_identities:
            findings.append(
                Finding(
                    f"{entry.file}:{entry.line}",
                    "missing-speaker-identity-census-access",
                    f"{entry.classification} {entry.callee} -> {entry.target} ({entry.function})",
                )
            )

    fixture_entries = [entry for entry in live_entries if entry.role == FIXTURE_ROLE]
    for entry in fixture_entries:
        if entry.classification != "direct_write":
            findings.append(
                Finding(
                    f"{entry.file}:{entry.line}",
                    "invalid-differential-fixture-role",
                    "fixture role may only cover direct writers",
                )
            )
    return sorted(set(findings), key=lambda item: (item.path, item.rule, item.detail))


def census_payload(entries: list[CensusEntry]) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "targets": list(TARGETS),
        "entries": [asdict(entry) for entry in sorted(entries)],
    }


def prune_stale_census(
    root: Path,
    census_path: Path,
    *,
    all_files: bool = False,
) -> list[CensusEntry]:
    """Drop expected entries that no longer exist in the live AST census.

    This deliberately never adds live accesses: the committed census is the
    desired post-cutover policy, so adding current direct writers would weaken
    the gate rather than regenerate its stale records.
    """
    census = _load_census(census_path)
    expected_entries = [_entry_from_json(value) for value in census["entries"]]
    expected_by_semantic_identity = {
        entry.semantic_identity(): entry for entry in expected_entries
    }
    live_entries, _findings, roles = scan(root, all_files=all_files)
    live_identities = {
        _role_for_entry(entry, roles, expected_by_semantic_identity).identity()
        for entry in live_entries
    }
    retained = [
        entry for entry in expected_entries if entry.identity() in live_identities
    ]
    census_path.write_text(
        json.dumps(census_payload(retained), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return retained


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--census", type=Path)
    parser.add_argument("--all-files", action="store_true")
    parser.add_argument(
        "--emit-live-census",
        action="store_true",
        help="print the raw live access census without checking it",
    )
    parser.add_argument(
        "--prune-stale-census",
        action="store_true",
        help="remove committed entries absent from the live AST census",
    )
    args = parser.parse_args(argv)
    root = args.root.resolve()
    census_path = (args.census or root / CENSUS_RELATIVE_PATH).resolve()
    if args.prune_stale_census:
        retained = prune_stale_census(root, census_path, all_files=args.all_files)
        print(f"pruned speaker-identity census to {len(retained)} live entries")
        return 0
    if args.emit_live_census:
        entries, findings, _roles = scan(root, all_files=args.all_files)
        print(json.dumps(census_payload(entries), indent=2, sort_keys=True))
        for finding in findings:
            print(f"{finding.path}: {finding.rule}: {finding.detail}")
        return 1 if findings else 0
    try:
        findings = check(root, census_path, all_files=args.all_files)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"speaker-identity cutover guard failed: {error}")
        return 1
    if not findings:
        return 0
    print("speaker-identity cutover guard failed:")
    for finding in findings:
        print(f"  {finding.path}: {finding.rule}: {finding.detail}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
