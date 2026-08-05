"""Extract the entity route surface and its refusal vocabulary from the reference.

Two things the port needs an oracle for, both of which disappear when the
reference does:

  routes   -- every route+method pair the entity module serves, so a port can
              assert coverage as a subset rather than against a hand-typed list.
  refusals -- every reason-code emission site, with the route and method it
              belongs to, the status it is emitted at, and whether it sits in a
              route handler or inside the shared error classifier.

The second distinction is the load-bearing one. The classifier is reached from
only two routes, so a port that reaches a code *only* through it leaves every
route-level emission of that same code untested while a per-code check stays
green.

Static analysis, no imports of the module under inspection.
"""

import ast
import json
import sys
from pathlib import Path

ROUTES_MODULE = "solstone/apps/entities/routes.py"
REASONS_MODULE = "solstone/convey/reasons.py"
CLASSIFIER = "_entity_operation_error"
DEFAULT_STATUS = 400


def declared_reasons(repo: Path) -> dict[str, dict]:
    """Return {python_name: {code, status}} for every declared Reason."""
    tree = ast.parse((repo / REASONS_MODULE).read_text())
    declared: dict[str, dict] = {}
    for node in tree.body:
        if not isinstance(node, ast.Assign) or not isinstance(node.value, ast.Call):
            continue
        if getattr(node.value.func, "id", None) != "Reason":
            continue
        target = node.targets[0]
        if not isinstance(target, ast.Name):
            continue
        args = node.value.args
        if not args or not isinstance(args[0], ast.Constant):
            continue
        status = DEFAULT_STATUS
        if len(args) >= 3 and isinstance(args[2], ast.Constant):
            status = args[2].value
        for keyword in node.value.keywords:
            if keyword.arg == "status" and isinstance(keyword.value, ast.Constant):
                status = keyword.value.value
        declared[target.id] = {"code": args[0].value, "status": status}
    return declared


def route_decorations(node: ast.FunctionDef) -> list[dict]:
    """Return the route+method pairs a handler declares."""
    pairs = []
    for decorator in node.decorator_list:
        if not isinstance(decorator, ast.Call):
            continue
        attribute = decorator.func
        if not isinstance(attribute, ast.Attribute) or attribute.attr != "route":
            continue
        if not decorator.args or not isinstance(decorator.args[0], ast.Constant):
            continue
        path = decorator.args[0].value
        methods = ["GET"]
        for keyword in decorator.keywords:
            if keyword.arg == "methods" and isinstance(keyword.value, ast.List):
                methods = [
                    element.value
                    for element in keyword.value.elts
                    if isinstance(element, ast.Constant)
                ]
        pairs.extend({"route": path, "method": method} for method in methods)
    return pairs


def main() -> int:
    repo = Path(__file__).resolve().parent.parent
    reasons = declared_reasons(repo)
    tree = ast.parse((repo / ROUTES_MODULE).read_text())

    functions = [
        node
        for node in ast.walk(tree)
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    ]

    routes: list[dict] = []
    handler_of: dict[str, list[dict]] = {}
    for function in functions:
        pairs = route_decorations(function)
        if pairs:
            routes.extend(pairs)
            handler_of[function.name] = pairs

    # Every error_response(...) call, attributed to its enclosing function.
    sites: list[dict] = []
    for function in functions:
        for node in ast.walk(function):
            if not isinstance(node, ast.Call):
                continue
            if getattr(node.func, "id", None) != "error_response":
                continue
            if not node.args or not isinstance(node.args[0], ast.Name):
                continue
            name = node.args[0].id
            if name not in reasons:
                continue
            declared = reasons[name]
            status = declared["status"]
            explicit = False
            for keyword in node.keywords:
                if keyword.arg == "status" and isinstance(keyword.value, ast.Constant):
                    status = keyword.value.value
                    explicit = True
            in_classifier = function.name == CLASSIFIER
            pairs = handler_of.get(function.name, [])
            sites.append(
                {
                    "code": declared["code"],
                    "status": status,
                    "status_overrides_default": explicit
                    and status != declared["status"],
                    "declared_default_status": declared["status"],
                    "site": "classifier" if in_classifier else "route",
                    "function": function.name,
                    "routes": pairs,
                    "line": node.lineno,
                }
            )

    # Which store operations each route depends on. A route cannot be served
    # until every operation beneath it exists, so this is the real ordering
    # constraint on porting the surface.
    store_names: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and node.module and "entities" in node.module:
            for alias in node.names:
                store_names.add(alias.asname or alias.name)

    route_dependencies: list[dict] = []
    operation_use: dict[str, int] = {}
    for function in functions:
        pairs = handler_of.get(function.name)
        if not pairs:
            continue
        calls = sorted(
            {
                call.func.id
                for call in ast.walk(function)
                if isinstance(call, ast.Call)
                and isinstance(call.func, ast.Name)
                and call.func.id in store_names
            }
        )
        for name in calls:
            operation_use[name] = operation_use.get(name, 0) + len(pairs)
        route_dependencies.append({"routes": pairs, "store_operations": calls})

    by_code: dict[str, dict] = {}
    for site in sites:
        entry = by_code.setdefault(
            site["code"], {"route_level": 0, "classifier": 0, "statuses": []}
        )
        entry["route_level" if site["site"] == "route" else "classifier"] += 1
        if site["status"] not in entry["statuses"]:
            entry["statuses"].append(site["status"])
    for entry in by_code.values():
        entry["statuses"].sort()

    overrides = [s for s in sites if s["status_overrides_default"]]
    classifier_only = sorted(
        code for code, entry in by_code.items() if entry["route_level"] == 0
    )

    artifact = {
        "note": (
            "The entity route surface and refusal vocabulary, extracted statically "
            "from the reference before it is removed. `routes` is every route+method "
            "pair the module serves. `refusal_sites` is every reason-code emission, "
            "attributed to the route it belongs to and marked route-level or "
            "classifier."
        ),
        "why_the_site_distinction_matters": (
            "The shared error classifier is reached from only two routes. A port that "
            "reaches a code only through the classifier leaves every route-level "
            "emission of that same code unexercised, while a per-code coverage check "
            "stays green. Assert coverage per SITE, not per code."
        ),
        "when_this_file_reddens": (
            "A later failure means the reference's route surface or refusal "
            "vocabulary changed. Re-extract and decide deliberately; do not "
            "regenerate to absorb a difference."
        ),
        "source": ROUTES_MODULE,
        "counts": {
            "route_method_pairs": len(routes),
            "distinct_codes": len(by_code),
            "refusal_sites": len(sites),
            "route_level_sites": sum(1 for s in sites if s["site"] == "route"),
            "classifier_sites": sum(1 for s in sites if s["site"] == "classifier"),
            "codes_reachable_only_through_the_classifier": len(classifier_only),
            "sites_overriding_their_declared_status": len(overrides),
            "distinct_store_operations_the_surface_depends_on": len(operation_use),
        },
        "codes_reachable_only_through_the_classifier": classifier_only,
        "status_overrides": [
            {
                "code": s["code"],
                "status": s["status"],
                "declared_default_status": s["declared_default_status"],
                "routes": s["routes"],
            }
            for s in overrides
        ],
        "why_the_store_dependencies_matter": (
            "A route cannot be served until every store operation beneath it exists. "
            "These names are the real ordering constraint on porting this surface -- "
            "scoping the routes without checking them assumes a complete store."
        ),
        "store_operation_use": dict(
            sorted(operation_use.items(), key=lambda kv: (-kv[1], kv[0]))
        ),
        "route_store_dependencies": route_dependencies,
        "by_code": dict(sorted(by_code.items())),
        "routes": sorted(routes, key=lambda pair: (pair["route"], pair["method"])),
        "refusal_sites": sites,
    }

    destination = repo / "core/fixtures/entity_route_surface.json"
    destination.write_text(
        json.dumps(artifact, indent=1, ensure_ascii=False) + "\n"
    )
    print(json.dumps(artifact["counts"], indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
