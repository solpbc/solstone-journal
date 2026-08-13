# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
import shutil
import subprocess
from pathlib import Path

import pytest

STATIC_ROOT = Path("solstone/convey/static")
BODY_WORKSPACE = Path("core/crates/solstone-core-convey-shell/assets/body/workspace.html")
GATE_HARNESS = STATIC_ROOT / "tests" / "gate-drawer.html"


def _owner_regions(source: str) -> list[str]:
    return re.findall(
        r"// --- owner-facing strings ---\n(.*?)// --- end owner-facing strings ---",
        source,
        flags=re.DOTALL,
    )


def _node_or_skip() -> str:
    node = shutil.which("node")
    if node is None:
        pytest.skip("node is not available")
    return node


def test_drawer_static_headers_match_day_grid_byte_for_byte():
    assert (STATIC_ROOT / "drawer.js").read_bytes().splitlines(keepends=True)[:2] == (
        STATIC_ROOT / "day-grid.js"
    ).read_bytes().splitlines(keepends=True)[:2]
    assert (STATIC_ROOT / "gate-drawer.js").read_bytes().splitlines(keepends=True)[
        :2
    ] == (STATIC_ROOT / "day-grid.js").read_bytes().splitlines(keepends=True)[:2]
    assert (STATIC_ROOT / "drawer.css").read_bytes().splitlines(keepends=True)[:2] == (
        STATIC_ROOT / "day-grid.css"
    ).read_bytes().splitlines(keepends=True)[:2]


def test_drawer_shell_links_follow_day_grid():
    source = (STATIC_ROOT / "shell.html").read_text(encoding="utf-8")

    assert source.index("/static/day-grid.css") < source.index("/static/drawer.css")
    assert source.index("/static/day-grid.js") < source.index("/static/drawer.js")
    assert source.index("/static/drawer.js") < source.index("/static/gate-drawer.js")
    assert source.index("/static/gate-drawer.js") < source.index(
        "/static/shell_boot.js"
    )
    assert source.index("/static/drawer.js") < source.index("/static/shell_boot.js")


def test_drawer_js_contract_and_constraints():
    source = (STATIC_ROOT / "drawer.js").read_text(encoding="utf-8")

    assert "(function () {" in source
    assert "'use strict';" in source
    assert (
        "window.Drawer = Object.freeze({ render, preserveOpen, formatLine });" in source
    )
    assert "Storage" not in source
    assert "localStorage" not in source
    assert "sessionStorage" not in source
    assert "aria-" not in source


def test_drawer_owner_facing_region_is_empty():
    source = (STATIC_ROOT / "drawer.js").read_text(encoding="utf-8")
    regions = _owner_regions(source)

    assert regions == ["  "]


def test_gate_drawer_js_contract_and_owner_facing_region():
    source = (STATIC_ROOT / "gate-drawer.js").read_text(encoding="utf-8")
    regions = _owner_regions(source)
    carried_strings = {"Manual tags:", "Segments with audio:", "Embeddings:"}

    assert "(function () {" in source
    assert "'use strict';" in source
    assert "window.GateDrawer = Object.freeze({ render });" in source
    assert "Storage" not in source
    assert "localStorage" not in source
    assert "sessionStorage" not in source
    assert "<b>" not in source
    assert "drawer-chip" not in source
    assert "next_generic" not in source
    assert regions and len(regions) == 1
    assert "SPK_OVERVIEW_OWNER_COHESION_LABEL" in regions[0]
    assert "payload-key test" in regions[0]
    for reason in (
        "too_few_stmts",
        "median_duration_too_short",
        "cluster_too_diffuse",
    ):
        assert reason in regions[0]
    strings = re.findall(r"'([^']*)'", regions[0])
    assert strings
    assert carried_strings.issubset(strings)


def test_drawer_css_has_no_generated_owner_text_or_user_select():
    source = (STATIC_ROOT / "drawer.css").read_text(encoding="utf-8")

    assert "user-select" not in source
    generated = re.findall(r"content\s*:\s*([^;]+);", source)
    assert all(value.strip() in {'""', "''"} for value in generated)


def test_drawer_css_classes_have_consumers():
    css = (STATIC_ROOT / "drawer.css").read_text(encoding="utf-8")
    consumers = "\n".join(
        [
            (STATIC_ROOT / "drawer.js").read_text(encoding="utf-8"),
            (STATIC_ROOT / "gate-drawer.js").read_text(encoding="utf-8"),
            (STATIC_ROOT / "tests" / "drawer.html").read_text(encoding="utf-8"),
            GATE_HARNESS.read_text(encoding="utf-8"),
            BODY_WORKSPACE.read_text(encoding="utf-8"),
        ]
    )
    classes = set(re.findall(r"\.((?:drawer|gate)(?:-[A-Za-z0-9_]+)*|ev-meta)\b", css))

    assert classes
    assert {name for name in classes if name not in consumers} == set()


def test_drawer_smoke_harness_covers_contract():
    source = (STATIC_ROOT / "tests" / "drawer.html").read_text(encoding="utf-8")

    assert '<link rel="stylesheet" href="../drawer.css">' in source
    assert '<script src="../drawer.js"></script>' in source
    assert "function assert(name, condition, detail)" in source
    assert "function equal(name, actual, expected)" in source
    assert "data-drawer-id" in source
    assert "drawer-chev" in source
    assert "drawer-summary-text" in source
    assert "drawer-body" in source
    assert "drawer-chip--warn" in source
    assert "drawer-chip--danger" in source
    assert "emphasized line text" in source
    assert "digit runs emphasized" in source
    assert "label has no emphasis" in source
    assert "chip has no emphasis" in source
    assert "prose line has no emphasis" in source
    assert "no line omits line span" in source
    assert "preserve restores open id" in source
    assert "vanished id is clean no-op" in source


def test_gate_drawer_smoke_harness_covers_contract():
    source = GATE_HARNESS.read_text(encoding="utf-8")

    assert source.startswith(
        "<!-- SPDX-License-Identifier: AGPL-3.0-only -->\n"
        "<!-- Copyright (c) 2026 sol pbc -->\n"
        "<!doctype html>"
    )
    assert '<link rel="stylesheet" href="../tokens.css">' in source
    assert '<link rel="stylesheet" href="../drawer.css">' in source
    assert source.index('<script src="../drawer.js"></script>') < source.index(
        '<script src="../gate-drawer.js"></script>'
    )
    assert "function assert(name, condition, detail)" in source
    assert "function equal(name, actual, expected)" in source
    assert "speakers-owner-gate-diagnostics" in source
    assert "gate-row" in source
    assert "gate-need" in source
    assert "gate-bar" in source
    assert "gate-next" in source
    assert "too few line emphasizes observed first" in source
    assert "median line emphasizes seconds" in source
    assert "cohesion line has no digits" in source
    assert "zero observed renders" in source
    assert "missing threshold omits needs" in source
    assert "incomplete triple omits bar" in source
    assert "bar caps at one hundred" in source
    assert "action html renders" in source
    assert "preserve restores gate drawer" in source


def test_gate_drawer_render_contract_under_node():
    node = _node_or_skip()
    drawer_source = (STATIC_ROOT / "drawer.js").read_text(encoding="utf-8")
    gate_source = (STATIC_ROOT / "gate-drawer.js").read_text(encoding="utf-8")
    script = "\n".join(
        [
            "const window = {};",
            drawer_source,
            gate_source,
            "function assert(condition, message) { if (!condition) throw new Error(message); }",
            r"""
function decodeEntities(text) {
  const named = { amp: "&", lt: "<", gt: ">", quot: "\"", apos: "'" };
  return String(text).replace(/&(#\d+|#x[0-9a-fA-F]+|\w+);/g, (entity, body) => {
    if (body[0] === "#") {
      const value = body[1]?.toLowerCase() === "x"
        ? Number.parseInt(body.slice(2), 16)
        : Number.parseInt(body.slice(1), 10);
      return Number.isFinite(value) ? String.fromCodePoint(value) : entity;
    }
    return Object.prototype.hasOwnProperty.call(named, body) ? named[body] : entity;
  });
}

function textContentAfterUnescape(html) {
  return String(html).split(/(<[^>]*>)/g)
    .filter((chunk) => chunk && !chunk.startsWith("<"))
    .map(decodeEntities)
    .join("");
}

function lineHtml(rendered) {
  const match = rendered.match(/<span class="drawer-line">([\s\S]*?)<\/span>/);
  return match ? match[1] : "";
}

function bodyHtml(rendered) {
  const match = rendered.match(/<div class="drawer-body">([\s\S]*?)<\/div><\/details>$/);
  return match ? match[1] : "";
}

function payload(overrides = {}) {
  return {
    status: "low_quality",
    source: "candidate_pool",
    low_quality_reason: "too_few_stmts",
    observed_value: 5,
    threshold_value: 30,
    manual_tags_count: 11,
    segments_available: 12,
    embeddings_available: 13,
    can_build_from_tags: false,
    ...overrides,
  };
}

function formatMetric(value) {
  if (typeof value !== "number" || !Number.isFinite(value)) return "n/a";
  return Number.isInteger(value) ? String(value) : value.toFixed(2);
}

function formatWithUnit(value, unit) {
  const text = formatMetric(value);
  return text === "n/a" ? text : `${text}${unit}`;
}

function boldTexts(html) {
  return Array.from(html.matchAll(/<b>(.*?)<\/b>/g)).map((match) => match[1]);
}

function assertDeclined(name, rendered) {
  assert(rendered === "", `${name} renders no drawer`);
  assert(!rendered.includes("gate-bar"), `${name} has no bar`);
  assert(!rendered.includes("gate-row"), `${name} has no row`);
}

assertDeclined("route default low quality", window.GateDrawer.render(payload({
  low_quality_reason: "",
  observed_value: 0.0,
  threshold_value: 0.0,
})));

assertDeclined("unknown low quality reason", window.GateDrawer.render(payload({
  low_quality_reason: "some_new_gate",
})));

const directDrawer = window.Drawer.render({
  id: "direct",
  label: "direct",
  line: "median statement length 1.5s — needs 2s",
  bodyHtml: "",
});
assert(Object.keys(window.Drawer).join("|") === "render|preserveOpen|formatLine", "drawer exports formatLine");
assert(window.Drawer.formatLine("2 files created · <5>").includes("<b>2</b> files created · &lt;<b>5</b>&gt;"), "formatLine escapes and emphasizes digits");
assert(lineHtml(directDrawer).includes("<b>1.5s</b>"), "drawer emphasizes compact seconds");

const tooFewPayload = payload();
const tooFew = window.GateDrawer.render(tooFewPayload, {
  actionHtml: '<button id="spkOwnerBuildFromTags">build</button>',
});
const tooFewLine = lineHtml(tooFew);
const tooFewBody = bodyHtml(tooFew);
const tooFewLineText = textContentAfterUnescape(tooFewLine);
const tooFewBodyText = textContentAfterUnescape(tooFewBody);
const tooFewObserved = formatMetric(tooFewPayload.observed_value);
const tooFewThreshold = formatMetric(tooFewPayload.threshold_value);
assert(tooFew.includes('data-drawer-id="speakers-owner-gate-diagnostics"'), "drawer id renders");
assert(!tooFew.includes("drawer-chip"), "chip omitted");
assert(tooFewLineText.includes(tooFewObserved), "too few line includes observed value");
assert(tooFewLineText.includes(tooFewThreshold), "too few line includes threshold value");
assert(tooFewLineText.indexOf(tooFewObserved) < tooFewLineText.indexOf(tooFewThreshold), "too few line is observed first");
assert(boldTexts(tooFewLine).join("|") === [tooFewObserved, tooFewThreshold].join("|"), "too few emphasis follows line order");
assert(tooFewBody.includes("<span>statements</span>"), "statements row label renders");
assert(!tooFewBody.includes("too_few_stmts"), "raw reason token omitted");
assert(!tooFewLineText.includes(tooFewPayload.low_quality_reason), "raw reason token omitted from line");
assert(tooFewBodyText.includes(tooFewPayload.source), "source value is retained");
assert(tooFewBodyText.includes(String(tooFewPayload.manual_tags_count)), "manual tags value is retained");
assert(tooFewBodyText.includes(String(tooFewPayload.segments_available)), "segments value is retained");
assert(tooFewBodyText.includes(String(tooFewPayload.embeddings_available)), "embeddings value is retained");
assert(tooFewBody.includes("spkOwnerBuildFromTags"), "action html renders");
assert(!tooFewBody.includes("<b>"), "body has no authored emphasis");

const medianPayload = payload({
  low_quality_reason: "median_duration_too_short",
  observed_value: 1.5,
  threshold_value: 2,
});
const median = window.GateDrawer.render(medianPayload);
const medianLine = lineHtml(median);
const medianLineText = textContentAfterUnescape(medianLine);
const medianObserved = formatWithUnit(medianPayload.observed_value, "s");
const medianThreshold = formatWithUnit(medianPayload.threshold_value, "s");
assert(medianLineText.includes(medianObserved), "median line includes observed value");
assert(medianLineText.includes(medianThreshold), "median line includes threshold value");
assert(medianLineText.indexOf(medianObserved) < medianLineText.indexOf(medianThreshold), "median line is observed first");
assert(boldTexts(medianLine).join("|") === [medianObserved, medianThreshold].join("|"), "median seconds are emphasized");
assert(bodyHtml(median).includes("<span>median length</span>"), "median row label renders");

const diffuse = window.GateDrawer.render(payload({
  low_quality_reason: "cluster_too_diffuse",
  observed_value: 0.2,
  threshold_value: 0.3,
}));
const diffuseLine = lineHtml(diffuse);
assert(textContentAfterUnescape(diffuseLine).trim().length > 0, "diffuse line text renders");
assert(!/\d/.test(textContentAfterUnescape(diffuseLine)), "diffuse line has no digits");
assert(!diffuseLine.includes("<b>"), "diffuse line has no emphasis");
assert(bodyHtml(diffuse).includes("<span>consistency</span>"), "consistency row label renders");

const zeroPayload = payload({
  low_quality_reason: "median_duration_too_short",
  observed_value: 0,
  threshold_value: 1.5,
});
const zeroObserved = window.GateDrawer.render(zeroPayload);
assert(textContentAfterUnescape(lineHtml(zeroObserved)).includes(formatWithUnit(zeroPayload.observed_value, "s")), "zero observed renders");

const missingThreshold = window.GateDrawer.render(payload({ threshold_value: 0 }));
assert(!textContentAfterUnescape(lineHtml(missingThreshold)).includes("needs"), "threshold zero omits needs clause");
assert(!missingThreshold.includes("gate-bar"), "threshold zero omits bar");

const absentObserved = window.GateDrawer.render(payload({ observed_value: Number.NaN }));
assert(textContentAfterUnescape(lineHtml(absentObserved)).includes("n/a"), "absent observed renders n/a");
assert(!absentObserved.includes("NaN"), "nan never reaches html");
assert(!absentObserved.includes("undefined"), "undefined never reaches html");

const cappedPayload = payload({ observed_value: payload().threshold_value * 2 });
const capped = window.GateDrawer.render(cappedPayload);
assert(capped.includes('style="width:100.00%"'), "bar caps at one hundred");
""",
        ]
    )

    subprocess.run([node, "-e", script], check=True, text=True)
