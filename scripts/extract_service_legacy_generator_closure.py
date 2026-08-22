#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Extract and run a historical service-generator dependency closure.

This is a hand-run capture tool, not a CI dependency. It parses a checked-out
historical service module, retains only the two generator entry points and
their actual dependencies, and runs the resulting synthetic module under a
specified pinned interpreter.
"""

from __future__ import annotations

import argparse
import ast
import builtins
import hashlib
import json
import subprocess
import tempfile
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

ENTRY_POINTS = ("_generate_plist", "_generate_systemd_unit")
BUILTINS = frozenset(dir(builtins))
RUNTIME_GLOBALS = frozenset({"__builtins__", "__file__", "__name__", "__package__"})


class ClosureError(RuntimeError):
    """The requested generator closure cannot be safely resolved."""


@dataclass(frozen=True)
class Statement:
    path: Path
    source: str
    node: ast.stmt
    kind: str
    name: str

    @property
    def source_path(self) -> str:
        return self.path.as_posix()

    @property
    def source_sha256(self) -> str:
        return hashlib.sha256(self.source.encode("utf-8")).hexdigest()

    @property
    def text(self) -> str:
        segment = ast.get_source_segment(self.source, self.node)
        if segment is None:
            raise ClosureError(
                f"cannot recover source for {self.source_path}:{self.name}"
            )
        return segment

    def provenance(self) -> dict[str, object]:
        return {
            "end_line": self.node.end_lineno,
            "kind": self.kind,
            "name": self.name,
            "source_path": self.source_path,
            "source_sha256": self.source_sha256,
            "start_line": self.node.lineno,
        }


class FreeNameCollector(ast.NodeVisitor):
    """Collect module-scope names loaded by a statement and nested closures."""

    def __init__(self) -> None:
        self.loaded: set[str] = set()
        self.bound: set[str] = set()

    def visit_Name(self, node: ast.Name) -> None:
        if isinstance(node.ctx, ast.Load):
            self.loaded.add(node.id)
        elif isinstance(node.ctx, (ast.Store, ast.Del)):
            self.bound.add(node.id)

    def visit_arg(self, node: ast.arg) -> None:
        self.bound.add(node.arg)

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.bound.update(_argument_names(node.args))
        if node.decorator_list:
            for decorator in node.decorator_list:
                self.visit(decorator)
        for default in [*node.args.defaults, *node.args.kw_defaults]:
            if default is not None:
                self.visit(default)
        for annotation in _annotations(node.args, node.returns):
            self.visit(annotation)
        for child in node.body:
            self.visit(child)

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_Lambda(self, node: ast.Lambda) -> None:
        self.bound.update(_argument_names(node.args))
        for default in [*node.args.defaults, *node.args.kw_defaults]:
            if default is not None:
                self.visit(default)
        for annotation in _annotations(node.args, None):
            self.visit(annotation)
        self.visit(node.body)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        for decorator in node.decorator_list:
            self.visit(decorator)
        for base in node.bases:
            self.visit(base)
        for keyword in node.keywords:
            self.visit(keyword.value)
        for child in node.body:
            self.visit(child)

    def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
        if node.type is not None:
            self.visit(node.type)
        if node.name is not None:
            self.bound.add(node.name)
        for child in node.body:
            self.visit(child)

    def free_names(self, node: ast.AST) -> set[str]:
        self.visit(node)
        return self.loaded - self.bound - BUILTINS


def _annotations(args: ast.arguments, returns: ast.expr | None) -> list[ast.expr]:
    annotations = [
        argument.annotation
        for argument in [*args.posonlyargs, *args.args, *args.kwonlyargs]
        if argument.annotation
    ]
    if args.vararg and args.vararg.annotation:
        annotations.append(args.vararg.annotation)
    if args.kwarg and args.kwarg.annotation:
        annotations.append(args.kwarg.annotation)
    if returns:
        annotations.append(returns)
    return annotations


def _argument_names(args: ast.arguments) -> set[str]:
    names = {
        argument.arg for argument in [*args.posonlyargs, *args.args, *args.kwonlyargs]
    }
    if args.vararg:
        names.add(args.vararg.arg)
    if args.kwarg:
        names.add(args.kwarg.arg)
    return names


class ModuleScope:
    """Top-level bindings from one checked-out Python source file."""

    def __init__(self, root: Path, path: Path) -> None:
        self.root = root
        self.path = path.resolve()
        try:
            self.source = self.path.read_text(encoding="utf-8")
        except OSError as exc:
            raise ClosureError(f"cannot read source file: {path}") from exc
        self.tree = ast.parse(self.source, filename=str(self.path))
        self.bindings: dict[str, Statement] = {}
        self._index()

    def _index(self) -> None:
        for node in self.tree.body:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                self.bindings[node.name] = Statement(
                    self.path, self.source, node, "definition", node.name
                )
            elif isinstance(node, (ast.Assign, ast.AnnAssign)):
                targets = (
                    node.targets if isinstance(node, ast.Assign) else [node.target]
                )
                for target in targets:
                    for name in _bound_names(target):
                        self.bindings[name] = Statement(
                            self.path, self.source, node, "assignment", name
                        )
            elif isinstance(node, ast.Import):
                for alias in node.names:
                    name = alias.asname or alias.name.split(".", 1)[0]
                    self.bindings[name] = Statement(
                        self.path, self.source, node, "import", name
                    )
            elif isinstance(node, ast.ImportFrom):
                for alias in node.names:
                    name = alias.asname or alias.name
                    self.bindings[name] = Statement(
                        self.path, self.source, node, "import", name
                    )

    def relative_path(self) -> str:
        try:
            return self.path.relative_to(self.root).as_posix()
        except ValueError as exc:
            raise ClosureError(f"source lies outside checkout: {self.path}") from exc


def _bound_names(node: ast.AST) -> set[str]:
    if isinstance(node, ast.Name):
        return {node.id}
    if isinstance(node, (ast.Tuple, ast.List)):
        return set().union(*(_bound_names(item) for item in node.elts))
    return set()


class ClosureExtractor:
    def __init__(
        self, root: Path, service_path: Path, stdlib_modules: set[str]
    ) -> None:
        self.root = root.resolve()
        self.stdlib_modules = stdlib_modules
        self.scopes: dict[Path, ModuleScope] = {}
        self.service = self.scope_for(service_path)
        self.imports: dict[str, Statement] = {}
        self.definitions: dict[tuple[Path, int], Statement] = {}
        self.visiting: set[tuple[Path, int]] = set()

    def scope_for(self, path: Path) -> ModuleScope:
        resolved = path.resolve()
        if resolved not in self.scopes:
            self.scopes[resolved] = ModuleScope(self.root, resolved)
        return self.scopes[resolved]

    def extract(self) -> tuple[str, list[dict[str, object]]]:
        for name in ENTRY_POINTS:
            self.resolve_name(self.service, name)
        import_statements = list(_dedupe_statements(self.imports.values()))
        future_statements = self.future_imports()
        imports = [statement.text for statement in import_statements]
        definitions = [statement.text for statement in self.definitions.values()]
        future = [statement.text for statement in future_statements]
        future.extend(
            text for text in imports if text.startswith("from __future__ import ")
        )
        future = list(dict.fromkeys(future))
        ordinary_imports = [text for text in imports if text not in future]
        source = "\n\n".join([*future, *ordinary_imports, *definitions]) + "\n"
        provenance = [
            _relative_provenance(statement, self.root)
            for statement in [
                *future_statements,
                *import_statements,
                *self.definitions.values(),
            ]
        ]
        return source, provenance

    def future_imports(self) -> list[Statement]:
        imports: list[Statement] = []
        for node in self.service.tree.body:
            if isinstance(node, ast.ImportFrom) and node.module == "__future__":
                imports.append(
                    Statement(
                        self.service.path,
                        self.service.source,
                        node,
                        "future_import",
                        "__future__",
                    )
                )
        return imports

    def resolve_name(self, scope: ModuleScope, name: str) -> None:
        if name in RUNTIME_GLOBALS:
            return
        try:
            statement = scope.bindings[name]
        except KeyError as exc:
            raise ClosureError(
                f"unresolved closure name {name!r} in {scope.relative_path()}"
            ) from exc
        key = (statement.path, statement.node.lineno)
        if key in self.definitions or key in self.imports or key in self.visiting:
            return
        self.visiting.add(key)
        try:
            if statement.kind == "import":
                self.resolve_import(scope, statement)
            else:
                for dependency in FreeNameCollector().free_names(statement.node):
                    self.resolve_name(scope, dependency)
                self.definitions[key] = statement
        finally:
            self.visiting.remove(key)

    def resolve_import(self, scope: ModuleScope, statement: Statement) -> None:
        node = statement.node
        if isinstance(node, ast.Import):
            alias = next(
                alias
                for alias in node.names
                if (alias.asname or alias.name.split(".", 1)[0]) == statement.name
            )
            module = alias.name
            if self.is_stdlib(module):
                self.imports[(statement.path, statement.node.lineno).__repr__()] = (
                    statement
                )
                return
            first_party = self.module_file(scope, module, 0)
            if first_party is not None:
                raise ClosureError(
                    f"first-party module import requires a named symbol: {module}"
                )
            raise ClosureError(f"third-party import in generator closure: {module}")
        if not isinstance(node, ast.ImportFrom):
            raise AssertionError(f"unexpected import node: {type(node).__name__}")
        alias = next(
            alias
            for alias in node.names
            if (alias.asname or alias.name) == statement.name
        )
        module = node.module or ""
        first_party = self.module_file(scope, module, node.level)
        if first_party is not None:
            if alias.name == "*":
                raise ClosureError(
                    f"star import in generator closure: {scope.relative_path()}"
                )
            self.resolve_name(self.scope_for(first_party), alias.name)
            return
        if self.is_stdlib(module):
            self.imports[(statement.path, statement.node.lineno).__repr__()] = statement
            return
        raise ClosureError(f"third-party import in generator closure: {module}")

    def is_stdlib(self, module: str) -> bool:
        return module.split(".", 1)[0] in self.stdlib_modules

    def module_file(self, scope: ModuleScope, module: str, level: int) -> Path | None:
        if level:
            package = scope.path.parent
            for _ in range(level - 1):
                package = package.parent
            parts = [
                *package.relative_to(self.root).parts,
                *filter(None, module.split(".")),
            ]
        else:
            parts = list(filter(None, module.split(".")))
        if not parts:
            return None
        candidate = self.root.joinpath(*parts).with_suffix(".py")
        if candidate.is_file():
            return candidate
        package_init = self.root.joinpath(*parts, "__init__.py")
        return package_init if package_init.is_file() else None


def _relative_provenance(statement: Statement, root: Path) -> dict[str, object]:
    record = statement.provenance()
    record["source_path"] = statement.path.relative_to(root).as_posix()
    return record


def _dedupe_statements(statements: Iterable[Statement]) -> tuple[Statement, ...]:
    unique: dict[str, Statement] = {}
    for statement in statements:
        unique.setdefault(statement.text, statement)
    return tuple(unique.values())


def stdlib_modules(python: Path) -> set[str]:
    program = """
import json
import pkgutil
import sys
import sysconfig

stdlib = sysconfig.get_path("stdlib")
modules = {name for name in sys.builtin_module_names}
modules.update(name for _, name, _ in pkgutil.iter_modules([stdlib]))
modules.update({"__future__", "builtins"})
print(json.dumps(sorted(modules)))
"""
    result = subprocess.run(
        [str(python), "-c", program], check=True, capture_output=True, text=True
    )
    return set(json.loads(result.stdout))


def capture_environment(
    python: Path, sandbox: Path, journal: Path, site_packages: Path
) -> dict[str, str]:
    return {
        "HOME": str(sandbox / "home"),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "PATH": f"{python.parent}:/usr/bin:/bin",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONHASHSEED": "0",
        "PYTHONNOUSERSITE": "1",
        "PYTHONPATH": str(site_packages),
        "TZ": "UTC",
        "_SOLSTONE_JOURNAL_OVERRIDE": str(journal),
    }


def execute_capture(
    python: Path,
    source: str,
    sandbox: Path,
    port: int,
) -> dict[str, object]:
    source_file = sandbox / "synthetic_service.py"
    request_file = sandbox / "request.json"
    result_file = sandbox / "result.json"
    journal = sandbox / "journal"
    site_packages = sandbox / "site-packages"
    source_file.write_text(source, encoding="utf-8")
    request_file.write_text(
        json.dumps(
            {
                "env": {
                    "HOME": str(sandbox / "home"),
                    "PATH": f"{python.parent}:/usr/bin:/bin",
                    "_SOLSTONE_JOURNAL_OVERRIDE": str(journal),
                },
                "journal_path": str(journal),
                "port": port,
            }
        ),
        encoding="utf-8",
    )
    program = """
import base64
import inspect
import json
import plistlib
import sys

source_path, request_path, result_path = sys.argv[1:]
request = json.loads(open(request_path, encoding="utf-8").read())
namespace = {"__file__": source_path, "__name__": "service_legacy_synthetic"}
exec(compile(open(source_path, encoding="utf-8").read(), source_path, "exec"), namespace)

def call(name):
    function = namespace[name]
    parameters = inspect.signature(function).parameters
    kwargs = {"env": request["env"]}
    if "port" in parameters:
        kwargs["port"] = request["port"]
    if "journal_path" in parameters:
        kwargs["journal_path"] = request["journal_path"]
    return function(**kwargs), str(inspect.signature(function))

plist, plist_signature = call("_generate_plist")
unit, unit_signature = call("_generate_systemd_unit")
payload = {
    "plist_base64": base64.b64encode(plist).decode("ascii"),
    "plist_summary": plistlib.loads(plist),
    "plist_signature": plist_signature,
    "python_version": sys.version,
    "systemd_unit": unit,
    "systemd_signature": unit_signature,
}
open(result_path, "w", encoding="utf-8").write(json.dumps(payload, indent=2, sort_keys=True) + "\\n")
"""
    environment = capture_environment(python, sandbox, journal, site_packages)
    (sandbox / "home").mkdir()
    site_packages.mkdir()
    subprocess.run(
        [
            str(python),
            "-c",
            program,
            str(source_file),
            str(request_file),
            str(result_file),
        ],
        check=True,
        cwd=sandbox,
        env=environment,
    )
    return json.loads(result_file.read_text(encoding="utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root", type=Path, required=True, help="historical checkout root"
    )
    parser.add_argument(
        "--service", type=Path, required=True, help="service.py path relative to --root"
    )
    parser.add_argument(
        "--python", type=Path, required=True, help="pinned interpreter executable"
    )
    parser.add_argument(
        "--output", type=Path, required=True, help="scratch JSON output path"
    )
    parser.add_argument("--port", type=int, default=5015)
    args = parser.parse_args()
    root = args.root.resolve()
    service = (root / args.service).resolve()
    python = args.python.resolve()
    if not service.is_file():
        raise ClosureError(f"service source is missing: {service}")
    if not python.is_file():
        raise ClosureError(f"pinned interpreter is missing: {python}")
    extractor = ClosureExtractor(root, service, stdlib_modules(python))
    source, provenance = extractor.extract()
    with tempfile.TemporaryDirectory(prefix="service-legacy-generator-") as temporary:
        capture = execute_capture(python, source, Path(temporary), args.port)
    payload = {
        "capture": capture,
        "provenance": provenance,
        "service_path": service.relative_to(root).as_posix(),
        "synthetic_source": source,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
