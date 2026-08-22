#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Guard the active-brain health cutover."""

from __future__ import annotations

import argparse
import ast
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE_SUFFIXES = {".py", ".md", ".js", ".html"}
LEGACY_HEALTH_FILE = "talents" + ".json"
PROVIDER_CLI_TOKEN = "providers" + "_cli"
PROVIDER_WORD = "providers"
CHECK_WORD = "check"
PROVIDER_CHECK_TEXT = PROVIDER_WORD + " " + CHECK_WORD
JOURNAL_PROVIDER_CHECK = "jour" + "nal " + PROVIDER_CHECK_TEXT
SOL_PROVIDER_CHECK = "s" + "ol " + PROVIDER_CHECK_TEXT
PROVIDER_CHECK_PREFIXES = (
    tuple(JOURNAL_PROVIDER_CHECK.split()),
    tuple(SOL_PROVIDER_CHECK.split()),
)
OWNER_LABELS = ("Provider " + "Readiness", "Agents " + "Health")
LEGACY_QUOTED_KEYS = (
    '"' + "provider" + "_readiness" + '"',
    "'" + "provider" + "_readiness" + "'",
    '"' + "ai" + "_readiness" + '"',
    "'" + "ai" + "_readiness" + "'",
)
COMMAND_LIST_RE = re.compile(
    r"\[\s*['\"](?:jour"
    r"nal|s"
    r"ol)['\"]\s*,\s*['\"]providers['\"]\s*,\s*['\"]check['\"]"
)
BRAIN_READER_ALLOWLIST = {
    "solstone/apps/health/routes.py",
    "solstone/apps/home/routes.py",
    "solstone/apps/support/diagnostics.py",
    "solstone/apps/thinking/routes.py",
    "solstone/think/brain_cli.py",
    "solstone/think/brain_health.py",
    "solstone/think/cortex.py",
    "solstone/think/doctor.py",
    "solstone/think/surfaces/health.py",
    "solstone/think/top.py",
}
BRAIN_READER_NAMES = {
    "inspect_brain_state",
    "build_brain_snapshot",
    "build_brain_presentation",
}
PROCESS_LOCAL_ATTESTATION_ALLOWLIST = {
    "solstone/think/brain_cli.py",
    "solstone/think/services/spp_transport.py",
    "solstone/think/services/spp.py",
    "solstone/observe/transcribe/confidential.py",
    "solstone/think/providers/local_endpoint.py",
    "solstone/think/providers/state.py",
}
PROCESS_LOCAL_ATTESTATION_MODULES = {
    "solstone.think.services.spp": {"get_attestation_state"},
    "solstone.think.providers.local_endpoint": {"probe_local_endpoint"},
    "solstone.think.services.spp_transport": {
        "recheck_confidential_attestation",
    },
}
PROCESS_LOCAL_ATTESTATION_IMPORT_MODULES = {
    "solstone.think.services": {
        "spp": "solstone.think.services.spp",
        "spp_transport": "solstone.think.services.spp_transport",
    },
    "solstone.think.providers": {
        "local_endpoint": "solstone.think.providers.local_endpoint",
    },
}
BRAIN_WRITE_CALLEES = {
    "open",
    "atomic_replace",
    "write_json",
    "hold_lock",
    "acquire_file_lease",
}
BRAIN_PATH_WRITE_METHODS = {"write_text", "write_bytes"}
BRAIN_PATH_HELPERS = {
    "brain_state_path",
    "brain_fingerprint_key_path",
    "brain_refresh_lease_path",
}
BRAIN_PATH_FRAGMENTS = (
    "health/brain.json",
    "brain-fingerprint.key",
    "brain-refresh.lease",
)


@dataclass(frozen=True)
class Finding:
    path: str
    rule: str
    detail: str


def _tracked_files(root: Path, *, all_files: bool) -> list[Path]:
    if all_files:
        return [
            path
            for path in root.rglob("*")
            if path.is_file() and path.suffix in SOURCE_SUFFIXES
        ]
    result = subprocess.run(
        ["git", "ls-files"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    files: list[Path] = []
    for line in result.stdout.splitlines():
        if not line or Path(line).suffix not in SOURCE_SUFFIXES:
            continue
        path = root / line
        if path.is_file():
            files.append(path)
    return files


def _rel(root: Path, path: Path) -> str:
    return path.relative_to(root).as_posix()


def _is_test_path(rel: str) -> bool:
    parts = Path(rel).parts
    return "tests" in parts or Path(rel).name.startswith("test_")


def _contains_command_list(path: Path, text: str) -> bool:
    if path.suffix == ".py":
        try:
            tree = ast.parse(text, filename=str(path))
        except SyntaxError:
            return False
        for node in ast.walk(tree):
            if not isinstance(node, (ast.List, ast.Tuple)) or len(node.elts) < 3:
                continue
            values = [
                item.value if isinstance(item, ast.Constant) else None
                for item in node.elts[:3]
            ]
            if tuple(values) in PROVIDER_CHECK_PREFIXES:
                return True
        return False
    return COMMAND_LIST_RE.search(text) is not None


def _imported_names(path: Path) -> set[str]:
    if path.suffix != ".py":
        return set()
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except SyntaxError:
        return set()
    names: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom):
            for alias in node.names:
                names.add(alias.name)
        elif isinstance(node, ast.Import):
            for alias in node.names:
                names.add(alias.name.rsplit(".", 1)[-1])
    return names


def _dotted_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        parent = _dotted_name(node.value)
        if parent:
            return f"{parent}.{node.attr}"
    return None


def _process_local_attestation_calls(path: Path, text: str) -> set[str]:
    if path.suffix != ".py":
        return set()
    try:
        tree = ast.parse(text, filename=str(path))
    except SyntaxError:
        return set()

    module_aliases: dict[str, str] = {}
    function_aliases: dict[str, str] = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name in PROCESS_LOCAL_ATTESTATION_MODULES:
                    local = alias.asname or alias.name.rsplit(".", 1)[-1]
                    module_aliases[local] = alias.name
        elif isinstance(node, ast.ImportFrom):
            imported_modules = PROCESS_LOCAL_ATTESTATION_IMPORT_MODULES.get(
                node.module or "",
                {},
            )
            target_functions = PROCESS_LOCAL_ATTESTATION_MODULES.get(
                node.module or "",
                set(),
            )
            for alias in node.names:
                local = alias.asname or alias.name
                if alias.name in imported_modules:
                    module_aliases[local] = imported_modules[alias.name]
                if alias.name in target_functions:
                    function_aliases[local] = f"{node.module}.{alias.name}"

    findings: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        dotted = _dotted_name(node.func)
        if dotted is None:
            continue
        if dotted in function_aliases:
            findings.add(function_aliases[dotted])
            continue
        for module, functions in PROCESS_LOCAL_ATTESTATION_MODULES.items():
            for function in functions:
                target = f"{module}.{function}"
                if dotted == target:
                    findings.add(target)
                    continue
                base, _, attr = dotted.rpartition(".")
                if attr != function:
                    continue
                if module_aliases.get(base) == module:
                    findings.add(target)
    return findings


def _brain_write_bindings(
    tree: ast.AST,
) -> tuple[dict[str, str], set[str], set[str], dict[str, str]]:
    """Return writer aliases and names assigned from brain-path helpers."""
    writer_aliases: dict[str, str] = {}
    helper_aliases: set[str] = set(BRAIN_PATH_HELPERS)
    module_aliases: dict[str, str] = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                local = alias.asname or alias.name.rsplit(".", 1)[-1]
                module_aliases[local] = alias.name
        elif isinstance(node, ast.ImportFrom):
            for alias in node.names:
                local = alias.asname or alias.name
                if alias.name in BRAIN_WRITE_CALLEES:
                    writer_aliases[local] = alias.name
                if alias.name in BRAIN_PATH_HELPERS:
                    helper_aliases.add(local)

    helper_paths: set[str] = set()
    for node in ast.walk(tree):
        value: ast.AST | None = None
        targets: list[ast.expr] = []
        if isinstance(node, ast.Assign):
            value = node.value
            targets = node.targets
        elif isinstance(node, ast.AnnAssign):
            value = node.value
            targets = [node.target]
        if not isinstance(value, ast.Call):
            continue
        if not _is_brain_path_helper_call(value, helper_aliases, module_aliases):
            continue
        for target in targets:
            if isinstance(target, ast.Name):
                helper_paths.add(target.id)
    return writer_aliases, helper_paths, helper_aliases, module_aliases


def _is_brain_path_helper_call(
    node: ast.AST,
    helper_aliases: set[str],
    module_aliases: dict[str, str],
) -> bool:
    if not isinstance(node, ast.Call):
        return False
    dotted = _dotted_name(node.func)
    if dotted is None:
        return False
    helper_name = dotted.rsplit(".", 1)[-1]
    helper_module = dotted.rpartition(".")[0]
    is_helper = helper_name in helper_aliases
    if helper_module and module_aliases.get(helper_module) == (
        "solstone.think.providers.brain_state"
    ):
        is_helper = helper_name in BRAIN_PATH_HELPERS
    return is_helper


def _brain_writer_callee(
    func: ast.expr,
    writer_aliases: dict[str, str],
) -> str | None:
    dotted = _dotted_name(func)
    if dotted is None:
        return None
    if dotted in writer_aliases:
        return writer_aliases[dotted]
    if dotted in BRAIN_WRITE_CALLEES:
        return dotted
    attribute = dotted.rsplit(".", 1)[-1]
    if attribute in BRAIN_WRITE_CALLEES:
        return attribute
    if attribute in BRAIN_PATH_WRITE_METHODS:
        return f"Path.{attribute}"
    return None


def _contains_brain_path_literal(node: ast.AST) -> bool:
    return any(
        isinstance(child, ast.Constant)
        and isinstance(child.value, str)
        and any(fragment in child.value for fragment in BRAIN_PATH_FRAGMENTS)
        for child in ast.walk(node)
    )


def _brain_state_write_calls(root: Path, path: Path, text: str) -> list[Finding]:
    if path.suffix != ".py":
        return []
    try:
        tree = ast.parse(text, filename=str(path))
    except SyntaxError:
        return []
    writer_aliases, helper_paths, helper_aliases, module_aliases = (
        _brain_write_bindings(tree)
    )
    findings: list[Finding] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        callee = _brain_writer_callee(node.func, writer_aliases)
        if callee is None:
            continue
        literal_target = any(
            _contains_brain_path_literal(argument)
            for argument in (*node.args, *(keyword.value for keyword in node.keywords))
        )
        helper_target = (
            bool(node.args)
            and isinstance(node.args[0], ast.Name)
            and (node.args[0].id in helper_paths)
        )
        inline_helper_target = bool(node.args) and _is_brain_path_helper_call(
            node.args[0], helper_aliases, module_aliases
        )
        method_helper_target = (
            callee in {"Path.write_text", "Path.write_bytes"}
            and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id in helper_paths
        )
        inline_method_helper_target = (
            callee in {"Path.write_text", "Path.write_bytes"}
            and isinstance(node.func, ast.Attribute)
            and _is_brain_path_helper_call(
                node.func.value, helper_aliases, module_aliases
            )
        )
        if (
            literal_target
            or helper_target
            or inline_helper_target
            or method_helper_target
            or inline_method_helper_target
        ):
            findings.append(
                Finding(
                    f"{_rel(root, path)}:{node.lineno}",
                    "python-brain-state-writer",
                    callee,
                )
            )
    return findings


def scan(root: Path, *, all_files: bool = False) -> list[Finding]:
    findings: list[Finding] = []
    for path in _tracked_files(root, all_files=all_files):
        rel = _rel(root, path)
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue

        if LEGACY_HEALTH_FILE in text:
            findings.append(Finding(rel, "legacy-health-file", LEGACY_HEALTH_FILE))

        if PROVIDER_CLI_TOKEN in text:
            findings.append(Finding(rel, "legacy-provider-cli", PROVIDER_CLI_TOKEN))
        if JOURNAL_PROVIDER_CHECK in text or SOL_PROVIDER_CHECK in text:
            findings.append(
                Finding(rel, "legacy-provider-check-text", PROVIDER_CHECK_TEXT)
            )
        if _contains_command_list(path, text):
            findings.append(
                Finding(rel, "legacy-provider-check-cmd", PROVIDER_CHECK_TEXT)
            )

        for label in OWNER_LABELS:
            if label in text:
                findings.append(Finding(rel, "legacy-owner-label", label))
        for key in LEGACY_QUOTED_KEYS:
            if key in text:
                findings.append(Finding(rel, "legacy-payload-key", key))

        if rel.startswith(("solstone/", "scripts/")) and not _is_test_path(rel):
            imported = _imported_names(path)
            if BRAIN_READER_NAMES & imported:
                if rel not in BRAIN_READER_ALLOWLIST:
                    findings.append(
                        Finding(
                            rel,
                            "unauthorized-brain-health-reader",
                            ", ".join(sorted(BRAIN_READER_NAMES & imported)),
                        )
                    )
            attestation_calls = _process_local_attestation_calls(path, text)
            if attestation_calls and rel not in PROCESS_LOCAL_ATTESTATION_ALLOWLIST:
                findings.append(
                    Finding(
                        rel,
                        "unauthorized-process-local-attestation",
                        ", ".join(sorted(attestation_calls)),
                    )
                )
            findings.extend(_brain_state_write_calls(root, path, text))
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument(
        "--all-files",
        action="store_true",
        help="Scan source files under --root instead of git-tracked files.",
    )
    args = parser.parse_args(argv)
    root = args.root.resolve()
    findings = scan(root, all_files=args.all_files)
    if not findings:
        return 0
    print("brain-health cutover guard failed:")
    for finding in findings:
        print(f"  {finding.path}: {finding.rule}: {finding.detail}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
