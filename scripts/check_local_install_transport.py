#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Prevent retired Python local-install implementation from returning."""

from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OWNER_FUNCTIONS: frozenset[tuple[str, str]] = frozenset(
    {("solstone/apps/thinking/local_bootstrap.py", "_mark_native_launch_failure")}
)
TRANSPORT_MODULES = frozenset({
    "solstone/think/providers/local_install.py",
    "solstone/think/providers/mlx_install.py",
})
CALLER_MODULES = frozenset({
    "solstone/think/install_provider.py",
    "solstone/apps/thinking/local_bootstrap.py",
})
RETIRED_DEFINITIONS = frozenset({
    "_safe_extract_tarball", "_download_file", "_chmod_executable",
    "_clear_macos_quarantine", "_is_legacy_cuda_oci_tree",
    "_cleanup_legacy_cuda_oci_dirs", "install_llama_server",
    "_install_cuda_llama_server", "install_model", "_MLX_MODEL_REGISTRY",
    "create_gemma4_variant", "_rewrite_config", "_rewrite_processor_config",
    "_manifest_inventory_for_tree", "_write_snapshot_manifest", "_write_variant_manifest",
    "_sha256_file", "_verify_sha256", "_write_vulkan_manifest",
    "_write_cuda_manifest", "_write_model_manifest", "LLAMA_SERVER_PINS",
    "CUDA_SERVER_PIN",
})
RETIRED_LOCAL_REFERENCES = frozenset({
    "CUDA_SERVER_PIN", "LLAMA_SERVER_PINS", "_safe_extract_tarball",
    "_download_file", "_chmod_executable", "_clear_macos_quarantine",
    "_is_legacy_cuda_oci_tree", "_cleanup_legacy_cuda_oci_dirs",
    "install_llama_server", "_install_cuda_llama_server", "install_model",
    "_sha256_file", "_verify_sha256", "_write_vulkan_manifest",
    "_write_cuda_manifest", "_write_model_manifest",
})
RETIRED_MLX_REFERENCES = frozenset({
    "_MLX_MODEL_REGISTRY", "create_gemma4_variant", "_rewrite_config",
    "_rewrite_processor_config", "_manifest_inventory_for_tree",
    "_write_snapshot_manifest", "_write_variant_manifest",
})
RETIRED_IMPORTS = frozenset({"tarfile", "hashlib", "httpx"})


def _call_name(node: ast.Call) -> str | None:
    if isinstance(node.func, ast.Name):
        return node.func.id
    if isinstance(node.func, ast.Attribute):
        return node.func.attr
    return None


def _local_argument(node: ast.Call) -> bool:
    values = [*node.args, *(keyword.value for keyword in node.keywords)]
    return any(isinstance(value, ast.Constant) and value.value == "local" for value in values)


class _BodyWalker(ast.NodeVisitor):
    def __init__(self, relative_path: str, function_name: str) -> None:
        self.relative_path = relative_path
        self.function_name = function_name
        self.findings: list[tuple[str, int, str]] = []

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        return None

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        return None

    def visit_Import(self, node: ast.Import) -> None:
        if self.relative_path in TRANSPORT_MODULES and any(alias.name.split(".")[0] in RETIRED_IMPORTS for alias in node.names):
            self.findings.append((self.relative_path, node.lineno, self.function_name))

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if self.relative_path in TRANSPORT_MODULES and node.module and node.module.split(".")[0] in RETIRED_IMPORTS:
            self.findings.append((self.relative_path, node.lineno, self.function_name))

    def visit_Call(self, node: ast.Call) -> None:
        if self.relative_path in CALLER_MODULES and (self.relative_path, self.function_name) not in OWNER_FUNCTIONS:
            if _call_name(node) in {"acquire_install_lease", "begin_or_replace_install_attempt"} and _local_argument(node):
                self.findings.append((self.relative_path, node.lineno, self.function_name))
        self.generic_visit(node)


def scan_source(source: str, filename: str, relative_path: str) -> list[tuple[str, int, str]]:
    tree = ast.parse(source, filename=filename)
    findings: list[tuple[str, int, str]] = []
    for node in tree.body:
        if isinstance(node, (ast.Assign, ast.AnnAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            if relative_path in TRANSPORT_MODULES and any(isinstance(target, ast.Name) and target.id in RETIRED_DEFINITIONS for target in targets):
                findings.append((relative_path, node.lineno, "<module>"))
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            if relative_path in TRANSPORT_MODULES and node.name in RETIRED_DEFINITIONS:
                findings.append((relative_path, node.lineno, node.name))
            walker = _BodyWalker(relative_path, node.name)
            for statement in node.body:
                walker.visit(statement)
            findings.extend(walker.findings)
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            walker = _BodyWalker(relative_path, "<module>")
            walker.visit(node)
            findings.extend(walker.findings)
    return findings


def scan_file(path: Path, relative_path: str | None = None) -> list[tuple[str, int, str]]:
    return scan_source(path.read_text(encoding="utf-8"), str(path), relative_path or path.as_posix())


def scan_directory(directory: Path) -> list[tuple[str, int, str]]:
    findings: list[tuple[str, int, str]] = []
    for path in sorted(directory.rglob("*.py")):
        name = path.relative_to(directory).as_posix()
        source = path.read_text(encoding="utf-8")
        # Negative twins exercise both rule families without mirroring production paths.
        findings.extend(scan_source(source, str(path), "solstone/think/providers/local_install.py"))
        findings.extend(scan_source(source, str(path), "solstone/apps/thinking/local_bootstrap.py"))
        findings = [(name, line, function) for _path, line, function in findings]
    return findings


def scan_retired_references(root: Path) -> list[tuple[str, int, str]]:
    """Reject retired transport symbols imported from either former owner anywhere."""
    findings: list[tuple[str, int, str]] = []
    excluded = {
        "scripts/check_local_install_transport.py",
        "scripts/fixtures/retired_local_install_transport.py",
    }
    for path in sorted(root.rglob("*.py")):
        relative = path.relative_to(root).as_posix()
        if relative in excluded:
            continue
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        local_modules = {"local_install"}
        mlx_modules = {"mlx_install"}
        imported_local: set[str] = set()
        imported_mlx: set[str] = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for alias in node.names:
                    if alias.name.endswith(".local_install"):
                        local_modules.add(alias.asname or alias.name.rsplit(".", 1)[-1])
                    if alias.name.endswith(".mlx_install"):
                        mlx_modules.add(alias.asname or alias.name.rsplit(".", 1)[-1])
            elif isinstance(node, ast.ImportFrom) and node.module:
                if node.module.endswith(".local_install"):
                    imported_local.update(
                        (alias.asname or alias.name)
                        for alias in node.names
                        if alias.name in RETIRED_LOCAL_REFERENCES
                    )
                if node.module.endswith(".mlx_install"):
                    imported_mlx.update(
                        (alias.asname or alias.name)
                        for alias in node.names
                        if alias.name in RETIRED_MLX_REFERENCES
                    )
        for node in ast.walk(tree):
            if isinstance(node, ast.Name) and (
                node.id in imported_local or node.id in imported_mlx
            ):
                findings.append((relative, node.lineno, "retired_import"))
            elif isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name):
                if (
                    node.value.id in local_modules
                    and node.attr in RETIRED_LOCAL_REFERENCES
                ) or (
                    node.value.id in mlx_modules
                    and node.attr in RETIRED_MLX_REFERENCES
                ):
                    findings.append((relative, node.lineno, "retired_reference"))
    return findings


def scan_production(root: Path) -> list[tuple[str, int, str]]:
    findings = [
        finding
        for relative in sorted(TRANSPORT_MODULES | CALLER_MODULES)
        for finding in scan_file(root / relative, relative)
    ]
    return findings + scan_retired_references(root)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Check local install transport ownership")
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    findings = scan_production(args.root)
    if not findings:
        return 0
    print("local-install-transport: violations:", file=sys.stderr)
    for path, line, function in findings:
        print(f"  {path}:{line}: {function}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
