#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture and independently verify Lane AF's retained text contracts.

This is a build-time evidence tool.  It must run under the pinned CPython
interpreter and never participates in product runtime or Rust tests.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import unicodedata
from pathlib import Path
from typing import Any, NoReturn

ROOT = Path(__file__).resolve().parents[1]
TOOL_PATH = Path("scripts/capture_health_text_reference.py")
SERVICE_PATH = Path("solstone/think/service.py")
HEALTH_FIXTURE_PATH = Path("core/fixtures/health_logs_reference.json")
DEFAULT_OUTPUT = ROOT / "core/fixtures/health_text_reference.json"

SERVICE_BLOB = "baa4f68d18830e92aa6ae215ffbf86cc8e14513f"
SERVICE_SHA256 = "62c31b78f97a2c147bf5873c1d732a61949c98ad54388038f313cfe23dfa8ae2"
HEALTH_FIXTURE_SHA256 = (
    "e7282efe72618ad6ff375fdd4065a7e60e3151a9b210f6c7a00377880a596a4b"
)
PYTHON_SHA256 = "255e900f44ce87c630e83b637a79435f9ae7778dd72f6e2a2f18a486e501d016"
PYTHON_VERSION = "3.14.6 (main, Jun 23 2026, 15:18:23) [Clang 22.1.3 ]"
UNICODE_VERSION = "16.0.0"
INT_MAX_STR_DIGITS = 4_300
WHITESPACE_COUNT = 29
DECIMAL_COUNT = 760
DECIMAL_ZERO_COUNT = 76
UNSAFE_CATEGORY_COUNTS = {"Cc": 65, "Cf": 170, "Zl": 1, "Zp": 1}
UNSAFE_UNION_COUNT = 237
UNSAFE_RANGE_COUNT = 23
_PINNED_SERVICE: Any | None = None


class VerificationError(RuntimeError):
    """A stable, named fixture-verification failure."""

    def __init__(self, code: str, detail: str):
        super().__init__(f"{code}: {detail}")
        self.code = code


def reject(code: str, detail: str) -> NoReturn:
    raise VerificationError(code, detail)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def run_git(*arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout


def committed_blob(path: Path) -> tuple[str, str]:
    try:
        line = run_git("ls-tree", "HEAD", "--", path.as_posix()).strip()
    except subprocess.CalledProcessError as error:
        reject("source-git", f"cannot inspect {path}: {error}")
    fields = line.split(None, 3)
    if len(fields) != 4 or fields[1] != "blob" or fields[3] != path.as_posix():
        reject("source-git", f"{path} is not one committed blob at HEAD")
    blob = fields[2]
    try:
        committed = subprocess.run(
            ["git", "show", f"HEAD:{path.as_posix()}"],
            cwd=ROOT,
            check=True,
            capture_output=True,
        ).stdout
    except subprocess.CalledProcessError as error:
        reject("source-git", f"cannot read committed {path}: {error}")
    current = (ROOT / path).read_bytes()
    if committed != current:
        reject("source-dirty", f"{path} differs from its committed blob")
    return blob, sha256_bytes(current)


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            reject("duplicate-field", f"duplicate JSON field {key!r}")
        result[key] = value
    return result


def load_json_bytes(data: bytes) -> dict[str, Any]:
    try:
        decoded = data.decode("utf-8")
        value = json.loads(decoded, object_pairs_hook=strict_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("json", str(error))
    if not isinstance(value, dict):
        reject("schema", "top-level fixture must be an object")
    return value


def exact_keys(value: dict[str, Any], expected: set[str], where: str) -> None:
    actual = set(value)
    if actual != expected:
        reject("schema", f"{where} keys differ: {sorted(actual ^ expected)}")


def checked_environment() -> tuple[dict[str, Any], dict[str, Any]]:
    if sys.version != PYTHON_VERSION:
        reject("runtime-python", f"expected {PYTHON_VERSION!r}, got {sys.version!r}")
    if unicodedata.unidata_version != UNICODE_VERSION:
        reject(
            "runtime-unicode",
            f"expected {UNICODE_VERSION}, got {unicodedata.unidata_version}",
        )
    if sys.get_int_max_str_digits() != INT_MAX_STR_DIGITS:
        reject(
            "runtime-digit-limit",
            f"expected {INT_MAX_STR_DIGITS}, got {sys.get_int_max_str_digits()}",
        )
    executable_sha = sha256_bytes(Path(sys.executable).read_bytes())
    if executable_sha != PYTHON_SHA256:
        reject("runtime-executable", f"unexpected interpreter digest {executable_sha}")

    service_blob, service_sha = committed_blob(SERVICE_PATH)
    if service_blob != SERVICE_BLOB or service_sha != SERVICE_SHA256:
        reject(
            "source-service", f"unexpected service source {service_blob}/{service_sha}"
        )
    tool_blob, tool_sha = committed_blob(TOOL_PATH)

    health_bytes = (ROOT / HEALTH_FIXTURE_PATH).read_bytes()
    health_sha = sha256_bytes(health_bytes)
    if health_sha != HEALTH_FIXTURE_SHA256:
        reject("source-health-fixture", f"unexpected fixture digest {health_sha}")
    health = load_json_bytes(health_bytes)
    exact_keys(
        health,
        {"regex", "rows", "runtime", "schema", "since", "source", "unicode_contract"},
        "health fixture",
    )
    if health["schema"] != 1:
        reject("source-health-schema", f"unexpected schema {health['schema']!r}")
    runtime = health["runtime"]
    if not isinstance(runtime, dict):
        reject("source-health-schema", "runtime is not an object")
    if runtime.get("python") != PYTHON_VERSION:
        reject("source-health-runtime", "Python version differs")
    if runtime.get("unicode") != UNICODE_VERSION:
        reject("source-health-runtime", "Unicode version differs")
    if runtime.get("executable_sha256") != PYTHON_SHA256:
        reject("source-health-runtime", "interpreter digest differs")
    unicode_contract = health["unicode_contract"]
    if not isinstance(unicode_contract, dict):
        reject("source-health-schema", "unicode_contract is not an object")
    whitespace = unicode_contract.get("whitespace_codepoints")
    decimal = unicode_contract.get("decimal_codepoints")
    decimal_zero = unicode_contract.get("decimal_zero_codepoints")
    if not isinstance(whitespace, list) or len(whitespace) != WHITESPACE_COUNT:
        reject("source-health-count", "whitespace denominator differs")
    if not isinstance(decimal, list) or len(decimal) != DECIMAL_COUNT:
        reject("source-health-count", "decimal denominator differs")
    if not isinstance(decimal_zero, list) or len(decimal_zero) != DECIMAL_ZERO_COUNT:
        reject("source-health-count", "decimal-zero denominator differs")
    expected_decimal = []
    expected_whitespace = []
    for codepoint in range(0x110000):
        if 0xD800 <= codepoint <= 0xDFFF:
            continue
        scalar = chr(codepoint)
        decimal_value = unicodedata.decimal(scalar, None)
        if decimal_value is not None:
            expected_decimal.append([codepoint, decimal_value])
        if scalar.isspace():
            expected_whitespace.append(codepoint)
    if decimal != expected_decimal:
        reject("source-health-decimal", "decimal table is not exhaustive and ordered")
    if whitespace != expected_whitespace:
        reject(
            "source-health-whitespace", "whitespace table is not exhaustive and ordered"
        )
    expected_zero = [codepoint for codepoint, value in decimal if value == 0]
    if decimal_zero != expected_zero:
        reject("source-health-decimal", "decimal-zero projection differs")

    provenance = {
        "capture_tool": {
            "git_blob": tool_blob,
            "path": TOOL_PATH.as_posix(),
            "sha256": tool_sha,
        },
        "health_fixture": {
            "path": HEALTH_FIXTURE_PATH.as_posix(),
            "sha256": health_sha,
        },
        "service_source": {
            "git_blob": service_blob,
            "path": SERVICE_PATH.as_posix(),
            "sha256": service_sha,
        },
    }
    return provenance, unicode_contract


def literal(identifier: str, text: str) -> dict[str, Any]:
    return {"id": identifier, "recipe": {"kind": "literal", "text": text}}


def codepoints(identifier: str, values: list[int]) -> dict[str, Any]:
    return {"id": identifier, "recipe": {"kind": "codepoints", "values": values}}


def repeated(
    identifier: str,
    codepoint: int,
    count: int,
    *,
    sign: str = "",
    separator: str = "",
    leading: list[int] | None = None,
    trailing: list[int] | None = None,
) -> dict[str, Any]:
    return {
        "id": identifier,
        "recipe": {
            "codepoint": codepoint,
            "count": count,
            "kind": "repeat",
            "leading": leading or [],
            "separator": separator,
            "sign": sign,
            "trailing": trailing or [],
        },
    }


def scalar_recipe_rows() -> list[dict[str, Any]]:
    rows = [
        literal("empty", ""),
        literal("zero", "0"),
        literal("negative-zero", "-0"),
        literal("positive-zero", "+0"),
        literal("positive", "42"),
        literal("negative", "-42"),
        literal("leading-zeros", "00000042"),
        literal("beyond-i128-positive", "340282366920938463463374607431768211456"),
        literal("beyond-i128-negative", "-340282366920938463463374607431768211457"),
        literal("plus-minus", "+-1"),
        literal("minus-plus", "-+1"),
        literal("double-plus", "++1"),
        literal("double-minus", "--1"),
        literal("sign-space", "+ 1"),
        literal("internal-space", "1 2"),
        literal("arabic-indic", "١٢٣"),
        literal("fullwidth", "１２３"),
        literal("mixed-script", "1٢３"),
        literal("superscript-no", "²"),
        literal("roman-nl", "Ⅻ"),
        literal("zero-width-space", "\u200b1"),
        literal("bom", "\ufeff1"),
        literal("unicode-minus", "−1"),
        literal("underscore-valid", "1_2_3"),
        literal("underscore-leading", "_1"),
        literal("underscore-trailing", "1_"),
        literal("underscore-double", "1__2"),
        literal("underscore-after-sign", "+_1"),
        literal("underscore-before-space", "1_ 2"),
        literal("decimal", "1.0"),
        literal("exponent", "1e3"),
        literal("hex-prefix", "0x10"),
        literal("nan", "NaN"),
        literal("infinity", "Infinity"),
        literal("nul", "1\x00"),
        literal("ordinary-text", "port"),
        codepoints("lone-surrogate", [0xD800]),
    ]
    rows.extend(
        [
            repeated("ascii-4300-positive", ord("9"), 4_300),
            repeated("ascii-4300-negative", ord("9"), 4_300, sign="-"),
            repeated("ascii-4301-positive", ord("9"), 4_301),
            repeated("ascii-4301-negative", ord("9"), 4_301, sign="-"),
            repeated("underscore-4300-positive", ord("9"), 4_300, separator="_"),
            repeated(
                "underscore-4300-negative", ord("9"), 4_300, sign="-", separator="_"
            ),
            repeated("underscore-4301-positive", ord("9"), 4_301, separator="_"),
            repeated(
                "underscore-4301-negative", ord("9"), 4_301, sign="-", separator="_"
            ),
            repeated(
                "arabic-4300-surrounded",
                0x0669,
                4_300,
                leading=[0x3000],
                trailing=[0x001C],
            ),
            repeated(
                "arabic-4301-surrounded",
                0x0669,
                4_301,
                leading=[0x3000],
                trailing=[0x001C],
            ),
            repeated("arabic-4300-negative", 0x0669, 4_300, sign="-"),
            repeated("arabic-4301-negative", 0x0669, 4_301, sign="-"),
        ]
    )
    return rows


def decode_scalar_recipe(recipe: dict[str, Any]) -> str:
    kind = recipe.get("kind")
    if kind == "literal":
        exact_keys(recipe, {"kind", "text"}, "literal recipe")
        text = recipe["text"]
        if not isinstance(text, str):
            reject("scalar-recipe", "literal text is not a string")
        return text
    if kind == "codepoints":
        exact_keys(recipe, {"kind", "values"}, "codepoint scalar recipe")
        values = recipe["values"]
        if (
            not isinstance(values, list)
            or not values
            or any(
                not isinstance(value, int) or value < 0 or value > 0x10FFFF
                for value in values
            )
        ):
            reject("scalar-recipe", "codepoint scalar values are invalid")
        return "".join(chr(value) for value in values)
    if kind == "repeat":
        exact_keys(
            recipe,
            {"codepoint", "count", "kind", "leading", "separator", "sign", "trailing"},
            "repeat recipe",
        )
        codepoint = recipe["codepoint"]
        count = recipe["count"]
        sign = recipe["sign"]
        separator = recipe["separator"]
        leading = recipe["leading"]
        trailing = recipe["trailing"]
        if (
            not isinstance(codepoint, int)
            or not isinstance(count, int)
            or count < 0
            or not isinstance(sign, str)
            or sign not in {"", "+", "-"}
            or not isinstance(separator, str)
            or not isinstance(leading, list)
            or not isinstance(trailing, list)
            or any(not isinstance(value, int) for value in [*leading, *trailing])
        ):
            reject("scalar-recipe", f"invalid repeat recipe {recipe!r}")
        digit = chr(codepoint)
        return (
            "".join(chr(value) for value in leading)
            + sign
            + separator.join([digit] * count)
            + "".join(chr(value) for value in trailing)
        )
    reject("scalar-recipe", f"unknown recipe kind {kind!r}")


def evaluate_scalar(text: str) -> dict[str, str]:
    try:
        value = int(text)
        return {"kind": "value", "value": str(value)}
    except ValueError:
        return {"kind": "ValueError"}


def port_recipes() -> list[dict[str, Any]]:
    return [
        {"id": "absent", "argv": {"kind": "text", "values": []}},
        {"id": "final-bare", "argv": {"kind": "text", "values": ["--port"]}},
        {"id": "separate-valid", "argv": {"kind": "text", "values": ["--port", "6"]}},
        {"id": "attached-valid", "argv": {"kind": "text", "values": ["--port=6"]}},
        {
            "id": "option-as-value",
            "argv": {"kind": "text", "values": ["--port", "--port=6"]},
        },
        {
            "id": "invalid-first-valid-later",
            "argv": {"kind": "text", "values": ["--port", "bad", "--port", "7"]},
        },
        {
            "id": "valid-first-invalid-later",
            "argv": {"kind": "text", "values": ["--port", "7", "--port", "bad"]},
        },
        {
            "id": "attached-valid-first",
            "argv": {"kind": "text", "values": ["--port=7", "--port=bad"]},
        },
        {
            "id": "attached-invalid-first",
            "argv": {"kind": "text", "values": ["--port=bad", "--port=7"]},
        },
        {"id": "empty-value", "argv": {"kind": "text", "values": ["--port", ""]}},
        {
            "id": "unrelated-then-final-bare",
            "argv": {"kind": "text", "values": ["--other", "--port"]},
        },
        {
            "id": "lone-surrogate",
            "argv": {"kind": "codepoints", "prefix": ["--port"], "values": [0xD800]},
        },
        {
            "id": "unix-invalid-byte",
            "argv": {
                "bytes_hex": "ff",
                "kind": "surrogateescape",
                "prefix": ["--port"],
            },
        },
    ]


def decode_argv_recipe(recipe: dict[str, Any]) -> list[str]:
    kind = recipe.get("kind")
    if kind == "text":
        exact_keys(recipe, {"kind", "values"}, "text argv recipe")
        values = recipe["values"]
        if not isinstance(values, list) or any(
            not isinstance(value, str) for value in values
        ):
            reject("port-recipe", "text argv values are invalid")
        return values
    if kind == "codepoints":
        exact_keys(recipe, {"kind", "prefix", "values"}, "codepoint argv recipe")
        prefix = recipe["prefix"]
        values = recipe["values"]
        if (
            not isinstance(prefix, list)
            or any(not isinstance(value, str) for value in prefix)
            or not isinstance(values, list)
            or any(not isinstance(value, int) for value in values)
        ):
            reject("port-recipe", "codepoint argv values are invalid")
        return [*prefix, "".join(chr(value) for value in values)]
    if kind == "surrogateescape":
        exact_keys(
            recipe, {"bytes_hex", "kind", "prefix"}, "surrogateescape argv recipe"
        )
        prefix = recipe["prefix"]
        value = recipe["bytes_hex"]
        if not isinstance(prefix, list) or any(
            not isinstance(item, str) for item in prefix
        ):
            reject("port-recipe", "surrogateescape prefix is invalid")
        if not isinstance(value, str):
            reject("port-recipe", "surrogateescape bytes are invalid")
        return [*prefix, os.fsdecode(bytes.fromhex(value))]
    reject("port-recipe", f"unknown argv recipe kind {kind!r}")


def evaluate_port(argv: list[str]) -> dict[str, Any]:
    service = load_pinned_service()

    stderr = io.StringIO()
    original_stderr = sys.stderr
    try:
        sys.stderr = stderr
        try:
            value = service._parse_port(argv)
        except SystemExit as error:
            return {
                "code": int(error.code),
                "kind": "exit",
                "stderr": stderr.getvalue(),
            }
        return {"kind": "return", "value": value}
    finally:
        sys.stderr = original_stderr


def load_pinned_service() -> Any:
    global _PINNED_SERVICE
    if _PINNED_SERVICE is not None:
        return _PINNED_SERVICE
    source = (ROOT / SERVICE_PATH).resolve()
    if sha256_bytes(source.read_bytes()) != SERVICE_SHA256:
        reject("source-service", "service source changed before import")
    module_name = "_lane_af_pinned_service_reference"
    spec = importlib.util.spec_from_file_location(module_name, source)
    if spec is None or spec.loader is None:
        reject("source-service-import", f"cannot load pinned source {source}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    try:
        spec.loader.exec_module(module)
    except Exception:
        sys.modules.pop(module_name, None)
        raise
    module_path = Path(module.__file__).resolve() if module.__file__ else None
    if (
        module_path != source
        or sha256_bytes(module_path.read_bytes()) != SERVICE_SHA256
    ):
        reject("source-service-import", "executed module is not the pinned source")
    _PINNED_SERVICE = module
    return module


def unsafe_unicode() -> dict[str, Any]:
    categories = {name: [] for name in UNSAFE_CATEGORY_COUNTS}
    for codepoint in range(0x110000):
        if 0xD800 <= codepoint <= 0xDFFF:
            continue
        category = unicodedata.category(chr(codepoint))
        if category in categories:
            categories[category].append(codepoint)
    union = sorted(codepoint for values in categories.values() for codepoint in values)
    ranges: list[dict[str, int | None]] = []
    if union:
        start = union[0]
        end = start
        for codepoint in union[1:]:
            if codepoint == end + 1:
                end = codepoint
                continue
            ranges.append(union_range(start, end))
            start = end = codepoint
        ranges.append(union_range(start, end))
    actual_counts = {name: len(values) for name, values in categories.items()}
    if actual_counts != UNSAFE_CATEGORY_COUNTS:
        reject("unicode-count", f"unsafe category counts differ: {actual_counts}")
    if len(union) != UNSAFE_UNION_COUNT or len(ranges) != UNSAFE_RANGE_COUNT:
        reject(
            "unicode-count",
            f"unsafe union/range counts differ: {len(union)}/{len(ranges)}",
        )
    union_set = set(union)
    for interval in ranges:
        for neighbor in (interval["lower"], interval["upper"]):
            if neighbor is not None and neighbor in union_set:
                reject("unicode-range", f"range neighbor U+{neighbor:04X} is unsafe")
    return {
        "categories": categories,
        "counts": {
            **actual_counts,
            "ranges": len(ranges),
            "union": len(union),
        },
        "ranges": ranges,
    }


def union_range(start: int, end: int) -> dict[str, int | None]:
    return {
        "end": end,
        "lower": start - 1 if start > 0 else None,
        "start": start,
        "upper": end + 1 if end < 0x10FFFF else None,
    }


def build_document() -> dict[str, Any]:
    provenance, contract = checked_environment()
    scalar_cases = []
    for row in scalar_recipe_rows():
        scalar_cases.append(
            {**row, "result": evaluate_scalar(decode_scalar_recipe(row["recipe"]))}
        )
    whitespace_cases = []
    for codepoint in contract["whitespace_codepoints"]:
        text = chr(codepoint) + "12" + chr(codepoint)
        whitespace_cases.append([codepoint, evaluate_scalar(text)])
    decimal_cases = []
    for codepoint, decimal_value in contract["decimal_codepoints"]:
        decimal_cases.append(
            [
                codepoint,
                decimal_value,
                evaluate_scalar(chr(codepoint)),
                evaluate_scalar("1" + chr(codepoint) + "2"),
            ]
        )
    port_cases = []
    for row in port_recipes():
        port_cases.append(
            {**row, "result": evaluate_port(decode_argv_recipe(row["argv"]))}
        )
    return {
        "decimal_cases": decimal_cases,
        "port_cases": port_cases,
        "provenance": provenance,
        "runtime": {
            "executable_sha256": PYTHON_SHA256,
            "int_max_str_digits": INT_MAX_STR_DIGITS,
            "python": PYTHON_VERSION,
            "unicode": UNICODE_VERSION,
        },
        "scalar_cases": scalar_cases,
        "schema": 1,
        "unsafe_unicode": unsafe_unicode(),
        "whitespace_cases": whitespace_cases,
    }


def canonical_bytes(document: dict[str, Any]) -> bytes:
    return (
        json.dumps(document, ensure_ascii=True, separators=(",", ":"), sort_keys=True)
        + "\n"
    ).encode("utf-8")


def verify_document(document: dict[str, Any]) -> None:
    provenance, contract = checked_environment()
    exact_keys(
        document,
        {
            "decimal_cases",
            "port_cases",
            "provenance",
            "runtime",
            "scalar_cases",
            "schema",
            "unsafe_unicode",
            "whitespace_cases",
        },
        "fixture",
    )
    if document["schema"] != 1:
        reject("schema", f"unexpected schema {document['schema']!r}")
    if document["provenance"] != provenance:
        reject("provenance", "source identity differs from committed inputs")
    expected_runtime = {
        "executable_sha256": PYTHON_SHA256,
        "int_max_str_digits": INT_MAX_STR_DIGITS,
        "python": PYTHON_VERSION,
        "unicode": UNICODE_VERSION,
    }
    if document["runtime"] != expected_runtime:
        reject("runtime", "runtime identity differs")

    expected_scalar = scalar_recipe_rows()
    actual_scalar = document["scalar_cases"]
    if not isinstance(actual_scalar, list) or len(actual_scalar) != len(
        expected_scalar
    ):
        reject("scalar-count", "scalar case denominator differs")
    for expected, actual in zip(expected_scalar, actual_scalar, strict=True):
        if not isinstance(actual, dict):
            reject("scalar-schema", "scalar row is not an object")
        exact_keys(actual, {"id", "recipe", "result"}, "scalar row")
        if actual["id"] != expected["id"] or actual["recipe"] != expected["recipe"]:
            reject("scalar-recipe", f"scalar recipe differs at {expected['id']}")
        computed = evaluate_scalar(decode_scalar_recipe(actual["recipe"]))
        if actual["result"] != computed:
            reject("scalar-result", f"scalar result differs at {expected['id']}")

    whitespace = contract["whitespace_codepoints"]
    actual_whitespace = document["whitespace_cases"]
    if (
        not isinstance(actual_whitespace, list)
        or len(actual_whitespace) != WHITESPACE_COUNT
    ):
        reject("whitespace-count", "whitespace case denominator differs")
    for expected_codepoint, actual in zip(whitespace, actual_whitespace, strict=True):
        if (
            not isinstance(actual, list)
            or len(actual) != 2
            or actual[0] != expected_codepoint
        ):
            reject(
                "whitespace-order",
                f"whitespace row differs at U+{expected_codepoint:04X}",
            )
        computed = evaluate_scalar(
            chr(expected_codepoint) + "12" + chr(expected_codepoint)
        )
        if actual[1] != computed:
            reject(
                "whitespace-result",
                f"whitespace result differs at U+{expected_codepoint:04X}",
            )

    decimal = contract["decimal_codepoints"]
    actual_decimal = document["decimal_cases"]
    if not isinstance(actual_decimal, list) or len(actual_decimal) != DECIMAL_COUNT:
        reject("decimal-count", "decimal case denominator differs")
    for expected, actual in zip(decimal, actual_decimal, strict=True):
        codepoint, decimal_value = expected
        if (
            not isinstance(actual, list)
            or len(actual) != 4
            or actual[0] != codepoint
            or actual[1] != decimal_value
        ):
            reject("decimal-order", f"decimal row differs at U+{codepoint:04X}")
        if actual[2] != evaluate_scalar(chr(codepoint)):
            reject(
                "decimal-result", f"single decimal result differs at U+{codepoint:04X}"
            )
        if actual[3] != evaluate_scalar("1" + chr(codepoint) + "2"):
            reject(
                "decimal-result", f"mixed decimal result differs at U+{codepoint:04X}"
            )

    expected_port = port_recipes()
    actual_port = document["port_cases"]
    if not isinstance(actual_port, list) or len(actual_port) != len(expected_port):
        reject("port-count", "port case denominator differs")
    for expected, actual in zip(expected_port, actual_port, strict=True):
        if not isinstance(actual, dict):
            reject("port-schema", "port row is not an object")
        exact_keys(actual, {"argv", "id", "result"}, "port row")
        if actual["id"] != expected["id"] or actual["argv"] != expected["argv"]:
            reject("port-recipe", f"port recipe differs at {expected['id']}")
        if actual["result"] != evaluate_port(decode_argv_recipe(actual["argv"])):
            reject("port-result", f"port result differs at {expected['id']}")

    expected_unsafe = unsafe_unicode()
    actual_unsafe = document["unsafe_unicode"]
    if not isinstance(actual_unsafe, dict):
        reject("unicode-schema", "unsafe_unicode is not an object")
    exact_keys(actual_unsafe, {"categories", "counts", "ranges"}, "unsafe_unicode")
    if actual_unsafe.get("counts") != expected_unsafe["counts"]:
        reject("unicode-count", "unsafe Unicode counts differ")
    if actual_unsafe.get("categories") != expected_unsafe["categories"]:
        reject("unicode-category", "unsafe Unicode category membership/order differs")
    if actual_unsafe.get("ranges") != expected_unsafe["ranges"]:
        reject("unicode-range", "unsafe Unicode ranges/neighbors differ")


def verify_raw(data: bytes, expected_sha256: str) -> dict[str, Any]:
    actual_sha = sha256_bytes(data)
    if actual_sha != expected_sha256:
        reject("raw-sha256", f"expected {expected_sha256}, got {actual_sha}")
    if canonical_bytes(load_json_bytes(data)) != data:
        reject("canonical-json", "fixture is not canonical compact JSON plus LF")
    return load_json_bytes(data)


def expect_failure(document: dict[str, Any], code: str) -> None:
    try:
        verify_document(document)
    except VerificationError as error:
        if error.code != code:
            reject("self-test", f"expected {code}, got {error.code}: {error}")
        return
    reject("self-test", f"mutation unexpectedly passed: {code}")


def self_test(data: bytes, expected_sha256: str) -> None:
    document = verify_raw(data, expected_sha256)
    verify_document(document)

    mutated_raw = data[:-1] + b" \n"
    try:
        verify_raw(mutated_raw, expected_sha256)
    except VerificationError as error:
        if error.code != "raw-sha256":
            reject("self-test", f"raw mutation reached {error.code}")
    else:
        reject("self-test", "raw mutation passed")

    mutations: list[tuple[str, Any]] = [
        (
            "scalar-recipe",
            lambda value: value["scalar_cases"][0].update({"id": "missing"}),
        ),
        (
            "scalar-result",
            lambda value: value["scalar_cases"][1].update(
                {"result": {"kind": "value", "value": "9"}}
            ),
        ),
        (
            "port-recipe",
            lambda value: value["port_cases"][0]["argv"].update({"values": ["--port"]}),
        ),
        (
            "port-result",
            lambda value: value["port_cases"][1].update(
                {"result": {"kind": "return", "value": 9}}
            ),
        ),
        (
            "whitespace-order",
            lambda value: value["whitespace_cases"][0].__setitem__(0, 32),
        ),
        (
            "decimal-result",
            lambda value: value["decimal_cases"][0].__setitem__(
                2, {"kind": "ValueError"}
            ),
        ),
        (
            "unicode-count",
            lambda value: value["unsafe_unicode"]["counts"].update({"union": 236}),
        ),
        (
            "unicode-category",
            lambda value: value["unsafe_unicode"]["categories"]["Cf"].pop(),
        ),
        (
            "unicode-range",
            lambda value: value["unsafe_unicode"]["ranges"][0].update({"end": 30}),
        ),
        ("runtime", lambda value: value["runtime"].update({"unicode": "15.1.0"})),
        (
            "provenance",
            lambda value: value["provenance"]["service_source"].update(
                {"git_blob": "0" * 40}
            ),
        ),
        ("schema", lambda value: value.update({"ignored": True})),
    ]
    for code, mutate in mutations:
        candidate = copy.deepcopy(document)
        mutate(candidate)
        # The semantic verifier is deliberately entered below the raw digest
        # boundary.  Otherwise a hash-only verifier would make every mutation red.
        expect_failure(candidate, code)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    capture = subparsers.add_parser("capture")
    capture.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    for name in ("verify", "self-test"):
        command = subparsers.add_parser(name)
        command.add_argument("--fixture", type=Path, default=DEFAULT_OUTPUT)
        command.add_argument("--expected-sha256", required=True)
    semantic = subparsers.add_parser("verify-semantics")
    semantic.add_argument("--fixture", type=Path, default=DEFAULT_OUTPUT)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.command == "capture":
            output = args.output.resolve()
            try:
                output.relative_to(ROOT)
            except ValueError:
                reject("output-path", "output must stay inside the repository")
            document = build_document()
            verify_document(document)
            output.write_bytes(canonical_bytes(document))
            print(f"wrote {output.relative_to(ROOT)}")
            return 0
        data = args.fixture.read_bytes()
        if args.command == "verify-semantics":
            verify_document(load_json_bytes(data))
        elif args.command == "verify":
            verify_document(verify_raw(data, args.expected_sha256))
        else:
            self_test(data, args.expected_sha256)
        print("health text reference verified")
        return 0
    except (OSError, subprocess.CalledProcessError, VerificationError) as error:
        print(f"health text reference failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
