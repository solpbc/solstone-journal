// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! This test statically analyzes Python Callosum emitters. When an emitter moves to Rust, this
//! test no longer sees it and must be extended with matching Rust-side coverage.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;

const PYTHON_PROGRAM: &str = r#"
import ast
import json
import os
from pathlib import Path

ROOT = Path(os.environ["SOLSTONE_REPO_ROOT"])
SOURCE_ROOT = ROOT / "solstone"
SKIPPED_PARTS = {"build", ".venv", "node_modules", "tests", "site-packages"}
EMITTER_NAMES = {"emit", "callosum_send", "callosum_send_classified"}


def relative(path):
    return path.relative_to(ROOT).as_posix()


def site(path, node):
    return f"{relative(path)}:{node.lineno}"


def literal_string(node):
    return node.value if isinstance(node, ast.Constant) and isinstance(node.value, str) else None


def call_name(node):
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    return None


def call_snippet(node):
    try:
        return ast.unparse(node)
    except Exception:
        return "<dynamic Callosum emission>"


def source_files():
    for path in sorted(SOURCE_ROOT.rglob("*.py")):
        parts = set(path.relative_to(ROOT).parts)
        if parts & SKIPPED_PARTS or path.name.startswith("test_"):
            continue
        yield path


def parse(path):
    try:
        return ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except (OSError, SyntaxError, UnicodeError):
        return None


def keyword_values(call):
    return {
        keyword.arg: keyword.value
        for keyword in call.keywords
        if keyword.arg is not None
    }


def bridge_forwarding_call_ids(parsed_files):
    path = ROOT / "solstone/convey/bridge.py"
    tree = parsed_files.get(path)
    if tree is None:
        return set()
    for function in tree.body:
        if not isinstance(function, ast.FunctionDef) or function.name != "emit":
            continue
        parameter_names = {argument.arg for argument in function.args.args}
        if not {"tract", "event"}.issubset(parameter_names):
            continue
        calls = set()
        for node in ast.walk(function):
            if (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Attribute)
                and isinstance(node.func.value, ast.Name)
                and node.func.value.id == "_CALLOSUM_CONNECTION"
                and node.func.attr == "emit"
                and len(node.args) >= 2
                and isinstance(node.args[0], ast.Name)
                and node.args[0].id == "tract"
                and isinstance(node.args[1], ast.Name)
                and node.args[1].id == "event"
            ):
                calls.add(id(node))
        return calls
    return set()


def direct_emitters(parsed_files, produced, unresolved):
    bridge_forwarding_calls = bridge_forwarding_call_ids(parsed_files)
    for path, tree in parsed_files.items():
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call) or call_name(node.func) not in EMITTER_NAMES:
                continue
            # The shared Convey bridge forwards its public emit() API to its connection.
            # Its callers are scanned separately, so this internal handoff is not a new site.
            if id(node) in bridge_forwarding_calls:
                continue
            keywords = keyword_values(node)
            tract_node = node.args[0] if len(node.args) >= 1 else keywords.get("tract")
            event_node = node.args[1] if len(node.args) >= 2 else keywords.get("event")
            tract = literal_string(tract_node)
            event = literal_string(event_node)
            if tract is not None and event is not None:
                produced.setdefault((tract, event), site(path, node))
            elif len(node.args) >= 2 or "tract" in keywords:
                unresolved.append({"site": site(path, node), "snippet": call_snippet(node)})


# Resolve thinking.emit(), which supplies the think tract internally.
def resolve_think_wrapper(parsed_files, produced):
    targets = {Path("solstone/think/thinking.py")}
    for path, tree in parsed_files.items():
        for node in ast.walk(tree):
            if not isinstance(node, ast.ImportFrom):
                continue
            if node.module != "solstone.think.thinking":
                continue
            if any(alias.name == "emit" and alias.asname is None for alias in node.names):
                targets.add(Path(relative(path)))
    for target in targets:
        path = ROOT / target
        tree = parsed_files.get(path)
        if tree is None:
            continue
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Name):
                continue
            if node.func.id != "emit" or not node.args:
                continue
            event = literal_string(node.args[0])
            if event is not None:
                produced.setdefault(("think", event), site(path, node))


# Resolve chat's closed _VALID_KINDS producer vocabulary.
def resolve_chat_vocabulary(parsed_files, produced):
    path = ROOT / "solstone/convey/chat_stream.py"
    tree = parsed_files.get(path)
    if tree is None:
        return
    for node in ast.walk(tree):
        if not isinstance(node, ast.Assign) or not isinstance(node.value, ast.Dict):
            continue
        if not any(isinstance(target, ast.Name) and target.id == "_VALID_KINDS" for target in node.targets):
            continue
        for key in node.value.keys:
            event = literal_string(key)
            if event is not None:
                produced.setdefault(("chat", event), site(path, key))


# Resolve cortex's verbatim stdout relay and chat's explicit cortex helper.
def cortex_durable_info_dict_ids(parsed_files):
    path = ROOT / "solstone/think/cortex.py"
    tree = parsed_files.get(path)
    if tree is None:
        return set()
    info_dicts = set()
    for handler in ast.walk(tree):
        if not isinstance(handler, ast.ExceptHandler):
            continue
        error_type = handler.type
        is_json_decode_error = (
            isinstance(error_type, ast.Name) and error_type.id == "JSONDecodeError"
        ) or (
            isinstance(error_type, ast.Attribute)
            and isinstance(error_type.value, ast.Name)
            and error_type.value.id == "json"
            and error_type.attr == "JSONDecodeError"
        )
        if not is_json_decode_error:
            continue
        for node in ast.walk(handler):
            if not isinstance(node, ast.Dict):
                continue
            if any(
                literal_string(key) == "event" and literal_string(value) == "info"
                for key, value in zip(node.keys, node.values)
            ):
                info_dicts.add(id(node))
    return info_dicts


def resolve_cortex_relay(parsed_files, produced):
    cortex_durable_info_dicts = cortex_durable_info_dict_ids(parsed_files)
    targets = [
        ROOT / "solstone/think/cortex.py",
        ROOT / "solstone/think/talents.py",
        ROOT / "solstone/convey/chat.py",
    ]
    targets.extend(sorted((ROOT / "solstone/think/providers").rglob("*.py")))
    for path in targets:
        tree = parsed_files.get(path)
        if tree is None:
            continue
        for node in ast.walk(tree):
            if isinstance(node, ast.Dict):
                for key, value in zip(node.keys, node.values):
                    if literal_string(key) == "event":
                        event = literal_string(value)
                        if event is not None:
                            # Cortex's non-JSON stdout fallback writes info only to the
                            # durable use-log; it does not pass through the bus relay.
                            if id(node) in cortex_durable_info_dicts:
                                continue
                            produced.setdefault(("cortex", event), site(path, value))
            elif isinstance(node, ast.Call):
                if call_name(node.func) != "_emit_cortex_event" or not node.args:
                    continue
                event = literal_string(node.args[0])
                if event is not None:
                    produced.setdefault(("cortex", event), site(path, node))


# Resolve the secure-listener callback, which supplies the link tract internally.
def resolve_link_wrapper(parsed_files, produced):
    path = ROOT / "solstone/convey/secure_listener/accept.py"
    tree = parsed_files.get(path)
    if tree is None:
        return
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Attribute):
            continue
        if not isinstance(node.func.value, ast.Name) or node.func.value.id != "self":
            continue
        if node.func.attr != "_emit" or not node.args:
            continue
        event = literal_string(node.args[0])
        if event is not None:
            produced.setdefault(("link", event), site(path, node))


def analyze():
    parsed_files = {}
    for path in source_files():
        tree = parse(path)
        if tree is not None:
            parsed_files[path] = tree

    produced = {}
    unresolved = []
    direct_emitters(parsed_files, produced, unresolved)
    resolve_think_wrapper(parsed_files, produced)
    resolve_chat_vocabulary(parsed_files, produced)
    resolve_cortex_relay(parsed_files, produced)
    resolve_link_wrapper(parsed_files, produced)

    return {
        "produced": [
            {"tract": tract, "event": event, "site": produced[(tract, event)]}
            for tract, event in sorted(produced)
        ],
        "unresolved": sorted(unresolved, key=lambda item: (item["site"], item["snippet"])),
    }


try:
    report = analyze()
except Exception as error:
    report = {
        "produced": [],
        "unresolved": [{"site": "<analyzer>", "snippet": repr(error)}],
    }
print(json.dumps(report, sort_keys=True))
"#;

const REGISTRY_FIXTURE: &str = include_str!("../../../fixtures/callosum_registry.json");
const UNRESOLVED_DYNAMIC_ALLOWLIST: &[&str] = &[
    "solstone/apps/observer/routes.py",
    "solstone/convey/chat.py",
    "solstone/convey/chat_stream.py",
    "solstone/convey/secure_listener/runtime.py",
    "solstone/think/cortex.py",
    "solstone/think/thinking.py",
];

#[derive(Debug, Deserialize)]
struct PythonReport {
    produced: Vec<ProducedPair>,
    unresolved: Vec<UnresolvedSite>,
}

#[derive(Debug, Deserialize)]
struct ProducedPair {
    tract: String,
    event: String,
    site: String,
}

#[derive(Debug, Deserialize)]
struct UnresolvedSite {
    site: String,
    snippet: String,
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn python() -> PathBuf {
    let venv = repository_root().join(".venv/bin/python3");
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python3")
    }
}

fn declared_pairs() -> (BTreeSet<(String, String)>, BTreeSet<String>) {
    let fixture: Value = serde_json::from_str(REGISTRY_FIXTURE).expect("valid registry fixture");
    let registry = fixture["registry"]
        .as_object()
        .expect("registry fixture has a registry object");
    let mut declared = BTreeSet::new();
    let mut wildcard_tracts = BTreeSet::new();

    for (tract, events) in registry {
        for event in events.as_array().expect("registry event list") {
            let event = event.as_str().expect("registry event string");
            if event == "*" {
                wildcard_tracts.insert(tract.clone());
            } else {
                declared.insert((tract.clone(), event.to_owned()));
            }
        }
    }

    (declared, wildcard_tracts)
}

fn site_file(site: &str) -> &str {
    site.rsplit_once(':').map_or(site, |(file, _)| file)
}

#[test]
fn declared_registry_covers_producible_python_pairs() {
    let output = Command::new(python())
        .args(["-c", PYTHON_PROGRAM])
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .output()
        .expect("run Python Callosum registry analyzer");
    assert!(
        output.status.success(),
        "Python Callosum registry analyzer failed:\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let report: PythonReport =
        serde_json::from_slice(&output.stdout).expect("parse Python Callosum registry report");
    let (declared, wildcard_tracts) = declared_pairs();

    let mut produced = BTreeMap::new();
    for pair in report.produced {
        produced
            .entry((pair.tract, pair.event))
            .or_insert(pair.site);
    }

    let undeclared = produced
        .iter()
        .filter(|((tract, event), _site)| {
            !wildcard_tracts.contains(tract) && !declared.contains(&(tract.clone(), event.clone()))
        })
        .map(|((tract, event), site)| format!("{tract}.{event} ({site})"))
        .collect::<Vec<_>>();
    assert!(
        undeclared.is_empty(),
        "Callosum registry is missing producible pairs:\n{}",
        undeclared.join("\n"),
    );

    let missing_producers = declared
        .iter()
        .filter(|pair| !produced.contains_key(*pair))
        .map(|(tract, event)| format!("{tract}.{event}"))
        .collect::<Vec<_>>();
    println!(
        "Declared Callosum pairs without static Python producers: {}",
        missing_producers.join(", ")
    );

    let unexpected_unresolved = report
        .unresolved
        .into_iter()
        .filter(|entry| !UNRESOLVED_DYNAMIC_ALLOWLIST.contains(&site_file(&entry.site)))
        .map(|entry| format!("{} {}", entry.site, entry.snippet))
        .collect::<Vec<_>>();
    assert!(
        unexpected_unresolved.is_empty(),
        "unrecognized dynamic Callosum emitters:\n{}",
        unexpected_unresolved.join("\n"),
    );
}
