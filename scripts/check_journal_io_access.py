#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Journal I/O write-primitive access lint.

This check is CI mechanics, not a permissions model. The shared
``solstone.think.journal_io`` write and lock primitives are mechanisms; the L2
domain write-owner set is the policy. A non-owner module violates the rule only
when it imports one of the gated journal_io primitives and calls through that
import binding.

Policy source: ``AGENTS.md`` §7, L2 — Domain write ownership. This script
transcribes that owner set into path exclusions so the check can enforce the
policy without restating it in prose.

Design decisions:

  D1 — Import-binding resolution. Bare-name matching is forbidden. For every
  scanned module, the detector first records imports that bind names or module
  aliases to ``solstone.think.journal_io`` or its write-primitive submodules,
  then flags calls only when the call target resolves through one of those
  bindings.

  D2 — Allowed callers. The journal_io package itself and the L2 domain
  write-owner files/directories are excluded during module discovery, so owner
  modules are never scanned or counted as violations.

  D3 — Violation kind and message. The violation ``kind`` is the primitive name
  itself, and every failure line names both the file and primitive.

  D4 — Script home and scope. This is a standalone sibling check for
  journal_io access. It intentionally does not modify
  ``scripts/check_layer_hygiene.py``.

The check ships green via a committed ``ALLOWLIST`` keyed by ``(file, kind)``
with an allowed **count**. A new violation that pushes any ``(file, kind)``
count above its allowed number fails the check; fixing occurrences lets the
allowed count be lowered, so the allowlist ratchets toward empty. It is never
keyed by line number and never a blanket per-file disable.

Exit codes:
  0 — no un-allowlisted violations
  1 — a (file, kind) count exceeds its allowlisted number
"""

from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

GATED_PRIMITIVES: frozenset[str] = frozenset(
    {
        "append_jsonl",
        "append_text",
        "atomic_replace",
        "acquire_file_lease",
        "adopt_inherited_file_lease_fd",
        "assert_file_lease_owned",
        "hold_lock",
        "install_file",
        "probe_file_lease_free",
        "probe_file_lease_held",
        "read_file_lease_fd",
        "read_file_lease_offset_token",
        "save_npz",
        "set_file_lease_offset_token",
        "update_npz",
        "write_bytes_exclusive",
        "write_json",
        "write_jsonl",
        "write_npz",
        "write_text",
    }
)

MODULE_PRIMITIVES: dict[str, frozenset[str]] = {
    "solstone.think.journal_io": GATED_PRIMITIVES,
    "solstone.think.journal_io.append": frozenset({"append_jsonl", "append_text"}),
    "solstone.think.journal_io.atomic": frozenset(
        {
            "atomic_replace",
            "install_file",
            "write_bytes_exclusive",
            "write_json",
            "write_jsonl",
            "write_text",
        }
    ),
    "solstone.think.journal_io.locking": frozenset({"hold_lock"}),
    "solstone.think.journal_io.lease": frozenset(
        {
            "acquire_file_lease",
            "adopt_inherited_file_lease_fd",
            "assert_file_lease_owned",
            "probe_file_lease_free",
            "probe_file_lease_held",
            "read_file_lease_fd",
            "read_file_lease_offset_token",
            "set_file_lease_offset_token",
        }
    ),
    "solstone.think.journal_io.npz": frozenset({"save_npz", "update_npz", "write_npz"}),
}

OWNER_FILES: frozenset[str] = frozenset(
    {
        "solstone/apps/chat/config.py",
        # Support portal operation ledger and local fingerprint key.
        "solstone/apps/support/operations.py",
        "solstone/convey/config.py",
        "solstone/apps/speakers/attribution.py",
        "solstone/apps/speakers/candidate_tracker.py",
        "solstone/apps/import/journal_sources.py",
        "solstone/apps/observer/utils.py",
        "solstone/think/activities.py",
        "solstone/think/awareness.py",
        # One-shot backfill of empty processing records onto header-only native describe/transcribe outputs.
        "solstone/think/backfill_processing_records.py",
        # Sole ops/runtime writer of health/catchup-state.json; uses hold_lock for cross-process RMW.
        "solstone/think/catchup_state.py",
        "solstone/think/day_accumulator.py",
        "solstone/think/entities/history.py",
        "solstone/think/entities/journal.py",
        "solstone/think/entities/merge.py",
        "solstone/think/entities/observations.py",
        "solstone/think/entities/relationships.py",
        "solstone/think/entities/ambiguities.py",
        "solstone/think/entities/review_candidates.py",
        "solstone/think/entities/saving.py",
        "solstone/think/entities/voiceprints.py",
        "solstone/think/facet_review_candidates.py",
        "solstone/think/speaker_candidate_pair_review_candidates.py",
        "solstone/think/speaker_cluster_dismissals.py",
        "solstone/think/speaker_identify_operations.py",
        "solstone/think/speaker_keep_separate.py",
        "solstone/think/speaker_review_candidates.py",
        "solstone/think/facets.py",
        "solstone/think/identity.py",
        "solstone/think/journal_config.py",
        "solstone/think/offload_ledger.py",
        # Sole writer of content-free bundled-local inference telemetry.
        "solstone/think/providers/local_admission.py",
        # Active-brain state, fingerprint key, and refresh lease.
        "solstone/think/providers/brain_state.py",
        # Provider install status, proof cache, and artifact manifests.
        "solstone/think/providers/artifact_proof.py",
        "solstone/think/providers/install_state.py",
        # Provider cache-local nvattest artifacts and install single-flight lock.
        "solstone/think/providers/nvattest_install.py",
        "solstone/think/providers/runtime_health.py",
        # Native speakers-analyze install-generation proof record and lease.
        "solstone/think/speakers_analyze_installation.py",
        "solstone/think/schedule_config.py",
        "solstone/think/push/devices.py",
        # Backup hosted-tier binding (0600 broker-token cache).
        "solstone/think/backup/hosted.py",
        # Link domain — device-pairing service state.
        "solstone/think/link/auth.py",
        "solstone/think/link/ca.py",
        "solstone/think/link/establish.py",
        "solstone/think/link/nonces.py",
        "solstone/think/link/paths.py",
        "solstone/think/sense_splitter.py",
        "solstone/think/streams.py",
        "solstone/think/talent_provenance.py",
        "solstone/think/thinking.py",
        # Chronicle day content direct writers. Importer modules that merely
        # route through importers/shared.py are not direct owners and are
        # intentionally omitted.
        "solstone/observe/depict.py",
        "solstone/observe/transcribe/main.py",
        "solstone/observe/transfer.py",
        "solstone/think/importers/cli.py",
        "solstone/think/importers/documents.py",
        "solstone/think/importers/images.py",
        "solstone/think/importers/shared.py",
        "solstone/convey/chat_stream.py",
        # imports/** bundle + sync-cursor writers (local/CLI import flows).
        "solstone/think/importers/plaud.py",  # streamed imported-audio install.
        "solstone/think/importers/sync.py",
        "solstone/think/importers/utils.py",
    }
)

OWNER_PREFIXES: tuple[str, ...] = (
    "solstone/apps/facets/",
    "solstone/think/indexer/",
)

# Committed allowlist of violations on the current tree, keyed by
# (posix-relative-path, kind) -> allowed count. Ratchets toward empty: lower a
# count as occurrences are fixed; never raise one to admit a new violation.
# Never line-keyed; never a per-file blanket.
ALLOWLIST: dict[tuple[str, str], int] = {}


def _is_owner(rel: Path) -> bool:
    rel_str = rel.as_posix()
    return rel_str in OWNER_FILES or any(
        rel_str.startswith(prefix) for prefix in OWNER_PREFIXES
    )


def _is_test_file(rel: Path) -> bool:
    return (
        "tests" in rel.parts
        or rel.name == "conftest.py"
        or (rel.name.startswith("test_") and rel.suffix == ".py")
    )


def discover_modules(root: Path) -> list[Path]:
    """Return posix-relative non-owner, non-test modules under ``solstone/``."""
    scope = root / "solstone"
    if not scope.is_dir():
        return []

    found: list[Path] = []
    for path in sorted(scope.rglob("*.py")):
        rel = path.relative_to(root)
        rel_str = rel.as_posix()
        if "__pycache__" in rel.parts:
            continue
        if rel_str.startswith("solstone/think/journal_io/"):
            continue
        if _is_test_file(rel):
            continue
        if _is_owner(rel):
            continue
        found.append(rel)
    return found


def _dotted_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        base = _dotted_name(node.value)
        if base:
            return f"{base}.{node.attr}"
    return None


def _bind_from_import(
    node: ast.ImportFrom,
    direct_bindings: dict[str, str],
    module_bindings: dict[str, frozenset[str]],
) -> None:
    module = node.module or ""
    if module == "solstone.think":
        for alias in node.names:
            if alias.name == "journal_io":
                module_bindings[alias.asname or alias.name] = GATED_PRIMITIVES
        return

    primitives = MODULE_PRIMITIVES.get(module)
    if not primitives:
        return

    for alias in node.names:
        if alias.name == "*":
            for primitive in primitives:
                direct_bindings[primitive] = primitive
        elif alias.name in primitives:
            direct_bindings[alias.asname or alias.name] = alias.name


def _bind_import(
    node: ast.Import,
    module_bindings: dict[str, frozenset[str]],
    dotted_bindings: dict[str, frozenset[str]],
) -> None:
    for alias in node.names:
        primitives = MODULE_PRIMITIVES.get(alias.name)
        if not primitives:
            continue
        if alias.asname:
            module_bindings[alias.asname] = primitives
        else:
            dotted_bindings[alias.name] = primitives


def _collect_bindings(
    tree: ast.AST,
) -> tuple[
    dict[str, str],
    dict[str, frozenset[str]],
    dict[str, frozenset[str]],
]:
    direct_bindings: dict[str, str] = {}
    module_bindings: dict[str, frozenset[str]] = {}
    dotted_bindings: dict[str, frozenset[str]] = {}

    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom):
            _bind_from_import(node, direct_bindings, module_bindings)
        elif isinstance(node, ast.Import):
            _bind_import(node, module_bindings, dotted_bindings)

    return direct_bindings, module_bindings, dotted_bindings


def _called_primitive(
    func: ast.expr,
    direct_bindings: dict[str, str],
    module_bindings: dict[str, frozenset[str]],
    dotted_bindings: dict[str, frozenset[str]],
) -> tuple[str, str] | None:
    if isinstance(func, ast.Name):
        primitive = direct_bindings.get(func.id)
        if primitive:
            return primitive, func.id
        return None

    if not isinstance(func, ast.Attribute):
        return None

    if (
        isinstance(func.value, ast.Name)
        and func.value.id in module_bindings
        and func.attr in module_bindings[func.value.id]
    ):
        return func.attr, f"{func.value.id}.{func.attr}"

    dotted = _dotted_name(func)
    if not dotted:
        return None
    for prefix, primitives in dotted_bindings.items():
        for primitive in primitives:
            if dotted == f"{prefix}.{primitive}":
                return primitive, dotted
    return None


def scan_source(source: str, filename: str = "<source>") -> list[tuple[int, str, str]]:
    """Return ``(lineno, primitive, bound_name)`` violations for source."""
    tree = ast.parse(source, filename=filename)
    direct_bindings, module_bindings, dotted_bindings = _collect_bindings(tree)

    findings: list[tuple[int, str, str]] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        called = _called_primitive(
            node.func,
            direct_bindings,
            module_bindings,
            dotted_bindings,
        )
        if called:
            primitive, bound_name = called
            findings.append((node.lineno, primitive, bound_name))
    findings.sort()
    return findings


def scan_file(path: Path) -> list[tuple[int, str, str]]:
    return scan_source(path.read_text(encoding="utf-8"), filename=str(path))


def count_violations(root: Path) -> dict[tuple[str, str], int]:
    """Map ``(posix-relpath, primitive)`` -> occurrence count across the tree."""
    counts: dict[tuple[str, str], int] = {}
    for rel in discover_modules(root):
        for _lineno, primitive, _bound_name in scan_file(root / rel):
            key = (rel.as_posix(), primitive)
            counts[key] = counts.get(key, 0) + 1
    return counts


def evaluate(
    root: Path,
    allowlist: dict[tuple[str, str], int],
) -> tuple[list[str], list[str]]:
    """Return ``(new_violations, tracked)`` human-readable lines."""
    new: list[str] = []
    tracked: list[str] = []
    for rel in discover_modules(root):
        rel_str = rel.as_posix()
        findings = scan_file(root / rel)
        by_primitive: dict[str, list[int]] = {}
        for lineno, primitive, _bound_name in findings:
            by_primitive.setdefault(primitive, []).append(lineno)
        for primitive, linenos in sorted(by_primitive.items()):
            count = len(linenos)
            allowed = allowlist.get((rel_str, primitive), 0)
            if count > allowed:
                lines = ", ".join(str(n) for n in sorted(linenos))
                new.append(
                    f"{rel_str}: {primitive} imported from journal_io and called "
                    f"by a non-owner ({count} occurrence(s), allowed {allowed}) "
                    f"at line(s) {lines}"
                )
            elif allowed:
                tracked.append(
                    f"{rel_str}: {count}/{allowed} {primitive} (allowlisted)"
                )
    return new, tracked


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Journal I/O access lint")
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="Repository root to scan (defaults to the checkout root).",
    )
    args = parser.parse_args(argv)

    new, tracked = evaluate(args.root, ALLOWLIST)

    if tracked:
        print("journal-io-access: known violations (allowlisted, ratcheting down):")
        for line in tracked:
            print(f"  {line}")
        print()

    if new:
        print("journal-io-access: NEW violations:", file=sys.stderr)
        for line in new:
            print(f"  {line}", file=sys.stderr)
        print(file=sys.stderr)
        print(
            "Route writes through the L2 owner for that domain — see AGENTS.md §7.",
            file=sys.stderr,
        )
        return 1

    print("journal-io-access: pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
