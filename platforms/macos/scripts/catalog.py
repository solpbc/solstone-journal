#!/usr/bin/env python3
"""Compile the Journal for Mac catalog and check its native resources."""
import argparse
import copy
import re
import hashlib
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CATALOG = ROOT / "i18n/catalog"
GENERATED = ROOT / "i18n/generated"
RUNTIME = ROOT / "Sources/JournalRuntime"
LOCK = ROOT / "i18n/generated-lock.json"
PLIST = ROOT / "Sources/journal/Info.plist"
ACCESSORS = RUNTIME / "JournalCopy.generated.swift"


def dump(value):
    return json.dumps(value, ensure_ascii=False, indent=2) + "\n"


def snapshot():
    paths = [ROOT / "scripts/catalog.py", ROOT / "i18n/info.json", ACCESSORS]
    for directory in [CATALOG, GENERATED, RUNTIME / "Localization"]:
        paths.extend(p for p in directory.rglob("*") if p.is_file())
    hashes = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in sorted(paths)}
    plist = plistlib.loads(PLIST.read_bytes())
    keys = list(json.loads((ROOT / "i18n/info.json").read_text()))
    keys += ["CFBundleDevelopmentRegion", "CFBundleLocalizations"]
    hashes["Info.plist localized fields"] = hashlib.sha256(
        dump({k: plist.get(k) for k in keys}).encode()).hexdigest()
    return hashes


def compile_table(table, path, destination, previous_table=None):
    rendered = dump(table)
    path.write_text(rendered)
    if shutil.which("xcrun"):
        if destination.is_dir():
            shutil.rmtree(destination)
        destination.mkdir(parents=True, exist_ok=True)
        subprocess.run(["xcrun", "xcstringstool", "compile", path,
                        "--output-directory", destination], check=True)
    elif previous_table is not None and previous_table == rendered and destination.is_dir() and any(destination.rglob("*")):
        return
    else:
        raise SystemExit(
            f"xcrun is not available and {path.relative_to(ROOT)} changed; "
            "run platforms/macos/scripts/catalog.py on macOS to recompile."
        )


def generate(compiler, channel):
    has_xcrun = shutil.which("xcrun") is not None
    prev_journal = (GENERATED / "Journal.xcstrings").read_text() if (GENERATED / "Journal.xcstrings").is_file() else None
    prev_info = (GENERATED / "InfoPlist.xcstrings").read_text() if (GENERATED / "InfoPlist.xcstrings").is_file() else None
    with tempfile.TemporaryDirectory() as temp:
        out = Path(temp) / "targets"
        ir_file = Path(temp) / "ir.json"
        subprocess.run([compiler, "targets", CATALOG, "--channel", channel,
                        "--target", "apple", "--out", out], check=True)
        subprocess.run([compiler, "emit", CATALOG, "--channel", channel,
                        "--out", ir_file], check=True)
        table = json.loads((out / "apple/journal-mac.xcstrings").read_text())
        ir = json.loads(ir_file.read_text())
    if has_xcrun:
        for directory in [GENERATED, RUNTIME / "Localization"]:
            if directory.is_dir():
                shutil.rmtree(directory)
        GENERATED.mkdir(parents=True)
    else:
        GENERATED.mkdir(parents=True, exist_ok=True)
    functions = ["// Generated from the Journal for Mac catalog by scripts/catalog.py.",
                 "extension JournalCopy {",
                 "    static let packagedLanguages = " + json.dumps(ir["locales"]), ""]
    info_keys = json.loads((ROOT / "i18n/info.json").read_text())
    plist = plistlib.loads(PLIST.read_bytes())
    info = dict(sourceLanguage=table["sourceLanguage"], strings={}, version=table["version"])
    for message in ir["messages"]:
        identity, key = message["id"], message["resource"]
        entry = table["strings"][key]
        # The English source interpolates verbatim. Remove only the compiler's
        # format-placeholder isolation, before any owner values are inserted.
        def source_format(node):
            if isinstance(node, dict):
                if "value" in node:
                    node["value"] = re.sub(r"\u2068(%[0-9]+\$(?:@|lld))\u2069", r"\1", node["value"])
                for child in node.values():
                    source_format(child)
        source_format(entry["localizations"]["en-US"])
        meta = json.loads((CATALOG / "meta" / (identity + ".json")).read_text())
        if meta["risk"] in {"legal", "statutory"}:
            for tag in entry["localizations"]:
                entry["localizations"][tag] = copy.deepcopy(entry["localizations"]["en-US"])
        if identity in info_keys.values():
            name = next(name for name, id_ in info_keys.items() if id_ == identity)
            info["strings"][name] = entry
            plist[name] = entry["localizations"]["en-US"]["stringUnit"]["value"].replace("%%", "%")
        parts = identity.split(".")[2:]
        name = parts[0] + "".join(part.title() for part in parts[1:])
        args = meta["arguments"]
        parameters = ", ".join(f"_ {arg['name']}: " + ("Int" if arg["type"] == "cardinal" else "String") for arg in args)
        values = [arg["name"] for arg in args]
        if meta["format_intent"] == "cardinal":
            selector = meta["selector"]["name"]
            values.append(f"String({selector})")
            values[values.index(selector)] = f"Int64({selector})"
            renderer = "renderPlural"
        else:
            renderer = "render"
        functions += [f"    public static func {name}({parameters}) -> String {{",
                      f'        {renderer}("{key}", [{", ".join(values)}])', "    }", ""]
    ACCESSORS.write_text("\n".join(functions + ["}", ""]))
    compile_table(table, GENERATED / "Journal.xcstrings", RUNTIME / "Localization", previous_table=prev_journal)
    compile_table(info, GENERATED / "InfoPlist.xcstrings", GENERATED / "AppResources", previous_table=prev_info)
    plist["CFBundleDevelopmentRegion"] = "en-US"
    plist["CFBundleLocalizations"] = ir["locales"]
    PLIST.write_bytes(plistlib.dumps(plist, sort_keys=False))
    LOCK.write_text(dump(snapshot()))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--compiler", type=Path)
    group.add_argument("--check", action="store_true")
    parser.add_argument("--channel", default="production", choices=["production", "development"])
    options = parser.parse_args()
    if options.check:
        expected, actual = json.loads(LOCK.read_text()), snapshot()
        differences = sorted(k for k in expected.keys() | actual.keys() if expected.get(k) != actual.get(k))
        if differences:
            raise SystemExit("Regenerate the Journal for Mac catalog. Changed files:\n" + "\n".join(differences))
    else:
        generate(options.compiler.resolve(), options.channel)
