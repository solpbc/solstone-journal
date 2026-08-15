#!/usr/bin/env python3
"""Sweep for GET-only Flask routes that reach a write primitive.

🔴 A READER IS EXACTLY THE INSTRUMENT THE NAME DEFEATS — the whole defect class is
names misleading readers, so read for guard semantics and SWEEP for read-named
writers. This keys on the ROUTE'S METHODS, not on the callee's name, because the
fourth variant of the class has an honestly-named callee and an accurate
docstring: the lie is in the HTTP method.

⚠ A POSITIVE CONTROL PROVES THE INSTRUMENT SEES SOMETHING, NOT THAT IT SEES AT
THE DEPTH THE DEFECT LIVES — the control has to RESEMBLE the defect. `import` is
the known positive: two GET routes reach `generate_content_manifest`, which ends
in `atomic_replace`. Resolution therefore crosses the FILE boundary; a sweep that
stops at it comes back clean on a cross-module writer WITH ITS CONTROLS FIRING.
If `import` returns anything but 2, this sweep is broken and every other zero in
the run means nothing.

⚠ KNOWN BLIND SPOT: hook calls are included in every route's reached set, but
imported helpers still stop after one file-boundary hop. `support_bp` puts its
acknowledgement drain on `before_request` AND `after_request`; `HOOK_DECORATORS`
below is the hook vocabulary folded into each GET-only route.

Usage:
    python scripts/check_get_only_writers.py                 # the default app set
    python scripts/check_get_only_writers.py health import   # named apps
"""
from __future__ import annotations

import ast
import subprocess
import sys

CONTROL_APP = "import"
# `87d708aa4` deleted solstone/apps/import/routes.py.  Its parent has the known
# (11, 2) control: import_content_list and import_content_detail both reach
# generate_content_manifest -> atomic_replace.
CONTROL_REV = "87d708aa4^"
CONTROL_EXPECTED = (11, 2)

#: Names that mean a filesystem write WHEREVER they appear. Every one of these is
#: unambiguous — no builtin type carries a method of the same name.
WRITE_PRIMITIVES = {
    "atomic_replace", "write_text", "write_bytes", "makedirs", "mkdir", "touch",
    "unlink", "rmtree", "rename", "utime", "chmod", "symlink_to", "writelines",
}

#: ⚠ Names that mean a write ONLY when qualified by their module. Left bare they
#: collide with harmless builtins — `dict.copy()`, `str.replace()`, and
#: `list`/`io` `write`. MEASURED: with bare `copy` in the set above, the sweep
#: flagged `sol api/preview` because `think.talent.get_talent` does
#: `_DEFAULT_LOAD.copy()` on a dict. ⛔ The fix is to DISCRIMINATE, never to drop
#: the check — a detector loosened until its output is clean is a masking rule.
QUALIFIED_WRITES = {
    ("shutil", "copy"), ("shutil", "copy2"), ("shutil", "copytree"),
    ("shutil", "move"), ("shutil", "rmtree"),
    ("os", "replace"), ("os", "rename"), ("os", "remove"), ("os", "unlink"),
    ("json", "dump"), ("Path", "replace"),
}
WRITING_HELPERS = {
    "mutate_journal_config", "ensure_identity_directory", "write_identity",
    "update_identity_section", "request_brain_refresh", "callosum_send",
    "begin_operation", "mark_completed", "mark_failed", "mark_in_progress",
    "mark_acknowledged", "release_retryable_lease", "append_chat_event",
    "record_draft_captured", "append_journal_action_log", "reprocess_day",
    "generate_content_manifest", "save_token_cache", "drain_pending_acknowledgements",
    "register", "ensure_registered", "_save_keypair", "_save_token", "_save_tos",
    "set_schedule_entries", "compact_expired_terminal_records",
}


def called_names(node: ast.AST) -> set[str]:
    names: set[str] = set()
    for sub in ast.walk(node):
        if isinstance(sub, ast.Call):
            fn = sub.func
            if isinstance(fn, ast.Name):
                names.add(fn.id)
            elif isinstance(fn, ast.Attribute):
                names.add(fn.attr)
                if isinstance(fn.value, ast.Name) and (fn.value.id, fn.attr) in QUALIFIED_WRITES:
                    names.add(f"{fn.value.id}.{fn.attr}<write>")
        # open(path, "w") / open(path, mode="w")
        if isinstance(sub, ast.Call) and isinstance(sub.func, ast.Name) and sub.func.id == "open":
            for arg in list(sub.args[1:]) + [k.value for k in sub.keywords if k.arg == "mode"]:
                if isinstance(arg, ast.Constant) and isinstance(arg.value, str) and (
                    "w" in arg.value or "a" in arg.value or "+" in arg.value
                ):
                    names.add("open<write>")
    return names


#: Hook decorators register a function that runs on EVERY request to the blueprint
#: or app. A writer registered here is invisible to any sweep keyed on route
#: decorators — which is the whole point: `support_bp` puts its portal drain on
#: before_request AND after_request, so every one of its GET routes reaches a
#: remote write while a route-only sweep reports the app clean.
HOOK_DECORATORS = {
    "before_request", "after_request", "teardown_request",
    "before_app_request", "after_app_request", "teardown_app_request",
}


def hook_kind(dec: ast.expr) -> str | None:
    """Return the hook name a decorator registers, or None."""
    fn = dec.func if isinstance(dec, ast.Call) else dec
    if isinstance(fn, ast.Attribute) and fn.attr in HOOK_DECORATORS:
        return fn.attr
    return None


def route_methods(dec: ast.expr) -> set[str] | None:
    """Return the methods a route decorator declares, or None if not a route."""
    if not isinstance(dec, ast.Call):
        return None
    fn = dec.func
    if not isinstance(fn, ast.Attribute):
        return None
    if fn.attr in ("get",):
        return {"GET"}
    if fn.attr in ("post", "put", "delete", "patch"):
        return {fn.attr.upper()}
    if fn.attr != "route":
        return None
    for kw in dec.keywords:
        if kw.arg == "methods" and isinstance(kw.value, (ast.List, ast.Tuple)):
            return {
                elt.value.upper()
                for elt in kw.value.elts
                if isinstance(elt, ast.Constant) and isinstance(elt.value, str)
            }
    return {"GET"}


def _module_source(dotted: str, rev: str) -> str:
    """Fetch a `solstone.x.y` module's source at *rev*, or empty if absent."""
    path = dotted.replace(".", "/") + ".py"
    out = subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True, text=True)
    if out.returncode == 0 and out.stdout:
        return out.stdout
    out = subprocess.run(
        ["git", "show", f"{rev}:{dotted.replace('.', '/')}/__init__.py"],
        capture_output=True, text=True,
    )
    return out.stdout if out.returncode == 0 else ""


def _imported_origins(tree: ast.AST) -> dict[str, str]:
    """Map an imported local name -> the dotted module it came from.

    🔴 SECOND FALSE-NEGATIVE MODE. One-hop resolution that stops at the FILE
    boundary misses cross-module writers: a handler calling an imported helper
    that writes comes back clean, WITH THE CONTROLS FIRING. A positive control
    proves the instrument sees something, not that it sees at the DEPTH the
    defect lives — the control has to resemble the defect.
    """
    origins: dict[str, str] = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and node.module:
            for alias in node.names:
                origins[alias.asname or alias.name] = node.module
    return origins


def _reached_calls(
    seeds: set[str], module_fns: dict[str, ast.AST], origins: dict[str, str],
    foreign_fns: dict[str, dict[str, ast.AST]], rev: str,
) -> set[str]:
    """Apply the existing same-file then one-hop imported-helper reachability."""
    reached = set(seeds)
    for name in list(reached):
        helper = module_fns.get(name)
        if helper is not None:
            reached |= called_names(helper)
    # Intentionally stops after this imported function (the known line-185 blind spot).
    for name in list(reached):
        dotted = origins.get(name)
        if dotted is None or not dotted.startswith("solstone"):
            continue
        foreign = foreign_fns.get(dotted)
        if foreign is None:
            src_f = _module_source(dotted, rev)
            foreign = {}
            if src_f:
                try:
                    for n in ast.parse(src_f).body:
                        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
                            foreign[n.name] = n
                except SyntaxError:
                    pass
            foreign_fns[dotted] = foreign
        target = foreign.get(name)
        if target is not None:
            reached |= called_names(target)
    return reached


def sweep(app: str, rev: str = "origin/main") -> tuple[int, int] | None:
    path = f"solstone/apps/{app}/routes.py"
    src = subprocess.run(
        ["git", "show", f"{rev}:{path}"], capture_output=True, text=True
    ).stdout
    if not src:
        print(f"  {app}: NO SOURCE at {rev}:{path}")
        return None
    tree = ast.parse(src)
    module_fns = {
        n.name: n for n in tree.body if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))
    }
    origins = _imported_origins(tree)
    foreign_fns: dict[str, dict[str, ast.AST]] = {}
    hook_seeds: set[str] = set()
    for node in module_fns.values():
        if any(hook_kind(dec) is not None for dec in node.decorator_list):
            hook_seeds |= called_names(node)
    get_only = 0
    findings = 0
    for node in tree.body:
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        methods: set[str] | None = None
        for dec in node.decorator_list:
            m = route_methods(dec)
            if m is not None:
                methods = m if methods is None else methods | m
        if methods is None or methods - {"GET", "HEAD", "OPTIONS"}:
            continue
        get_only += 1
        reached = _reached_calls(called_names(node) | hook_seeds, module_fns, origins, foreign_fns, rev)
        hits = (reached & WRITE_PRIMITIVES) | (reached & WRITING_HELPERS)
        hits |= {n for n in reached if n.endswith("<write>")}
        if "open<write>" in reached:
            hits = hits | {"open<write>"}
        if hits:
            findings += 1
            print(f"  🔴 {app}: GET-only `{node.name}` (line {node.lineno}) reaches {sorted(hits)}")
    return (get_only, findings)


if __name__ == "__main__":
    apps = sys.argv[1:] or ["health", "stats", "tokens", "sol", "support", "import"]
    total = 0
    print("=== GET-only routes reaching a write primitive (origin/main except pinned control) ===")
    for app in apps:
        rev = CONTROL_REV if app == CONTROL_APP else "origin/main"
        result = sweep(app, rev)
        if result is None:
            if app == CONTROL_APP:
                raise SystemExit(f"{CONTROL_APP} control source is missing at {CONTROL_REV}")
            print(f"  {app}: 0 GET-only routes, 0 flagged")
            continue
        g, f = result
        if app == CONTROL_APP and result != CONTROL_EXPECTED:
            raise SystemExit(f"{CONTROL_APP} control expected {CONTROL_EXPECTED}, got {result}")
        total += f
        print(f"  {app}: {g} GET-only routes, {f} flagged")
    print(f"\nTOTAL flagged: {total}")
    print("⚠ `import` is the POSITIVE CONTROL — if it flagged 0, this sweep is broken "
          "and every other zero in this run means nothing.")
