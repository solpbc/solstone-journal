# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import re
import shutil
import subprocess
import textwrap
from pathlib import Path
from typing import Mapping

import pytest

from scripts.build_convey_icons import (
    CONVEY_ICON_NAMES,
    load_lucide_icons,
    render_convey_icons_js,
    selected_icons,
)

ROOT = Path(__file__).resolve().parents[1]
CONVEY_ICONS_JS = ROOT / "solstone" / "convey" / "static" / "convey_icons.js"

CONVERTED_FILES = (
    ROOT / "solstone" / "apps" / "search" / "workspace.html",
    ROOT / "solstone" / "apps" / "sol" / "workspace.html",
    ROOT / "solstone" / "apps" / "import" / "workspace.html",
    ROOT / "solstone" / "apps" / "settings" / "workspace.html",
    ROOT / "solstone" / "apps" / "support" / "workspace.html",
    ROOT / "solstone" / "apps" / "support" / "static" / "support.js",
    ROOT / "solstone" / "apps" / "stats" / "workspace.html",
    ROOT / "solstone" / "apps" / "stats" / "static" / "dashboard.js",
    ROOT / "solstone" / "apps" / "activities" / "workspace.html",
    ROOT / "solstone" / "apps" / "transcripts" / "workspace.html",
    ROOT
    / "core"
    / "crates"
    / "solstone-core-convey-shell"
    / "assets"
    / "speakers"
    / "workspace.html",
    ROOT / "solstone" / "apps" / "entities" / "workspace.html",
    ROOT / "solstone" / "apps" / "health" / "workspace.html",
    ROOT / "solstone" / "apps" / "health" / "static" / "health.js",
    ROOT / "solstone" / "apps" / "network" / "workspace.html",
    ROOT / "solstone" / "convey" / "static" / "app.js",
    ROOT / "solstone" / "convey" / "static" / "status_pane.js",
    ROOT / "solstone" / "convey" / "static" / "shell.html",
)

CALL_RE = re.compile(
    r"window\.ConveyIcons\??\.svg\(\s*(?P<q>['\"`])(?P<name>[a-z0-9-]+)(?P=q)\s*\)"
)
SVG_RE = re.compile(r"<svg\b[\s\S]*?</svg>")
L2_ICON_SLOT_CLASSES = (
    "icon-slot",
    "entity-delete-btn",
    "voiceprint-icon",
    "facet-rel-voiceprint",
    "btn-icon",
    "vitals-chip",
    "trust-indicator",
    "link-hero-icon",
    "col-activity",
    "activity-item",
    "summary-item",
    "import-source-card-icon",
    "import-source-inline-icon",
    "import-content-summary-icon",
    "filter-icon",
    "result-agent-icon",
    "occ-activity-icon",
    "act-icon",
    "activity-detail-icon",
    "activity-icon",
    "activity-chip-icon",
)
ICON_SLOT_CLASSES = (
    "surface-state-icon",
    "empty-icon",
    *L2_ICON_SLOT_CLASSES,
)
ICON_SLOT_CLASS_PATTERN = "|".join(re.escape(cls) for cls in ICON_SLOT_CLASSES)
ICON_SLOT_OPEN_RE = re.compile(
    r"<(?P<tag>[A-Za-z][A-Za-z0-9:-]*)\b"
    r"(?=[^>]*\bclass=(?P<quote>['\"])[^'\"]*"
    rf"(?<![A-Za-z0-9_-])(?:{ICON_SLOT_CLASS_PATTERN})(?![A-Za-z0-9_-])"
    r"[^'\"]*(?P=quote))"
    r"[^>]*>",
    re.IGNORECASE,
)
EMPTY_ICONS_MAP_RE = re.compile(r"const\s+emptyIcons\s*=\s*\{[\s\S]*?\n\s*\};")
UNICODE_ESCAPE_RE = re.compile(
    r"\\U(?P<long>[0-9a-fA-F]{8})|\\u(?P<short>[0-9a-fA-F]{4})"
)
STYLE_BLOCK_RE = re.compile(r"<style[^>]*>(?P<body>[\s\S]*?)</style>", re.IGNORECASE)
L2_ICON_SLOT_CSS_FILES = {
    "icon-slot": ROOT / "solstone" / "convey" / "static" / "app.css",
    "entity-delete-btn": ROOT / "solstone" / "apps" / "entities" / "workspace.html",
    "voiceprint-icon": ROOT / "solstone" / "apps" / "entities" / "workspace.html",
    "facet-rel-voiceprint": ROOT / "solstone" / "apps" / "entities" / "workspace.html",
    "btn-icon": ROOT / "solstone" / "apps" / "entities" / "workspace.html",
    "vitals-chip": ROOT / "solstone" / "apps" / "health" / "workspace.html",
    "trust-indicator": ROOT / "solstone" / "apps" / "health" / "workspace.html",
    "link-hero-icon": ROOT / "solstone" / "apps" / "network" / "workspace.html",
    "col-activity": ROOT / "solstone" / "apps" / "sol" / "workspace.html",
    "activity-item": ROOT / "solstone" / "apps" / "sol" / "workspace.html",
    "summary-item": ROOT / "solstone" / "apps" / "sol" / "workspace.html",
    "import-source-card-icon": ROOT / "solstone" / "apps" / "import" / "workspace.html",
    "import-source-inline-icon": ROOT
    / "solstone"
    / "apps"
    / "import"
    / "workspace.html",
    "import-content-summary-icon": ROOT
    / "solstone"
    / "apps"
    / "import"
    / "workspace.html",
    "filter-icon": ROOT / "solstone" / "apps" / "search" / "workspace.html",
    "result-agent-icon": ROOT / "solstone" / "apps" / "search" / "workspace.html",
    "occ-activity-icon": ROOT / "solstone" / "apps" / "activities" / "workspace.html",
    "act-icon": ROOT / "solstone" / "apps" / "activities" / "workspace.html",
    "activity-detail-icon": ROOT
    / "solstone"
    / "apps"
    / "activities"
    / "workspace.html",
    "activity-icon": ROOT / "solstone" / "apps" / "settings" / "workspace.html",
    "activity-chip-icon": ROOT / "solstone" / "apps" / "settings" / "workspace.html",
}

CONVERTED_GLYPH_RESIDUE = {
    "solstone/apps/search/workspace.html": (
        'aria-hidden="true">🔍</div>',
        "icon: '🔍'",
    ),
    "solstone/apps/sol/workspace.html": (
        '<div class="empty-state-icon">🤖</div>',
        '<div class="empty-state-icon">⚠️</div>',
        '<th class="col-activity" title="thinking events">💭</th>',
        '<th class="col-activity" title="tool calls">🔧</th>',
        '<th class="col-activity" title="cost">💰</th>',
        '<span class="summary-item" title="${costInfo.title}">💰 ${costInfo.text}</span>',
        "thinking.innerHTML = `💭 ${agent.thinking_count}`;",
        "tools.innerHTML = `🔧 ${agent.tool_count}`;",
        "cost.innerHTML = `💰 ${costInfo.text}`;",
    ),
    "solstone/apps/import/workspace.html": (
        '<div class="no-imports-icon">📥</div>',
        '<div class="no-imports-icon">🔍</div>',
        "source.emoji",
        "source_emoji",
        "sourceEmojiByName",
        "import-source-card-emoji",
        "import-source-inline-emoji",
        '<h3 class="import-guide-title">⚡ quick import</h3>',
    ),
    "solstone/apps/support/static/support.js": (
        '<div class="support-empty-icon">🛟</div>',
        '<div class="support-empty-icon">⚠️</div>',
        '<div class="support-empty-icon">⋯</div>',
    ),
    "solstone/apps/support/workspace.html": (
        '<div class="support-empty-icon">⚠️</div>',
        '<div class="support-empty-icon">⋯</div>',
        '<p class="support-section-intro">💬 Share your impressions, ideas, or anything on your mind. Your feedback shapes the product.</p>',
        "<h3>🛟 getting help</h3>",
        "<h3>🔍 search the knowledge base</h3>",
        "<h3>🩺 run diagnostics</h3>",
        "<h3>📢 announcements</h3>",
        "<h3>🔒 privacy</h3>",
        "const icons = {'known-issue': '⚠️', 'maintenance': '🔧', 'info': '📢'};",
    ),
    "solstone/apps/stats/static/dashboard.js": (
        "['📊']",
        "|| '📊'",
        "['🎙️']",
        "emptyIcon: '🏷️'",
        "emptyIcon: '⚡'",
    ),
    "solstone/apps/activities/workspace.html": (
        '<div class="timeline-empty"><div class="empty-icon"><svg',
    ),
    "solstone/apps/transcripts/workspace.html": (
        '<div class="surface-state-icon" aria-hidden="true"><svg',
        "day: '<svg",
        "nothing: '<svg",
        "transcript: '<svg",
        "audio: '<svg",
        "screen: '<svg",
        "signals: '<svg",
        "icon: '🗑️'",
        "icon: '⚠️'",
        "icon: '↩️'",
        "icon: '⏱️'",
    ),
    "core/crates/solstone-core-convey-shell/assets/speakers/workspace.html": (
        '<div class="surface-state-icon" aria-hidden="true"><svg',
        "segment: '<svg",
        "cursor: '<svg",
        "people: '<svg",
        "text: '<svg",
        "audio: '<svg",
    ),
    "solstone/apps/entities/workspace.html": (
        "voiceprint.textContent = '🎤';",
        "indicators.push('🎤 Has voiceprint');",
        "generateBtn.textContent = '✨';",
        "voiceIcon.textContent = '🎤';",
        "deleteBtn.textContent = '🗑️';",
        "icon: '🗑️'",
        "icon: '↩️',\n        title: 'Delete cancelled'",
        "icon: '⏱️'",
        "icon: '⚠️'",
        "icon: '↩️',\n        title: doneMessage",
    ),
    "solstone/apps/settings/workspace.html": (
        "icon: '❌'",
        "icon: '✅'",
        "icon: '🔄'",
    ),
    "solstone/apps/support/background.html": ("icon: '🛟'",),
    "solstone/apps/health/workspace.html": (
        '<div class="trust-indicator" id="trustIndicator">🔒 all data stored locally on your device</div>',
    ),
    "solstone/apps/health/static/health.js": (
        "const due = s.due ? ' ⏰' : '';",
        "elements.trustIndicator.textContent = '🔒 all data stored locally on your device';",
        "elements.trustIndicator.textContent = '🔒 All data stored locally · Syncing to ' + s.host;",
    ),
    "solstone/apps/network/workspace.html": (
        '<div class="link-hero-icon" aria-hidden="true">📡</div>',
    ),
    "solstone/convey/static/status_pane.js": (
        "bell.textContent = '🔔';",
        "bell.textContent = '🔕';",
        "${escape(n.icon)}",
    ),
    "solstone/convey/static/websocket.js": (
        "icon: '✓'",
        "icon: '⚠️'",
    ),
    "solstone/convey/static/app.js": (
        'const ERROR_ICON = \'<svg viewBox="0 0 24 24"',
        "icon: options.icon || '📬'",
        "body: notif.message,\n          icon: notif.icon,",
        "${window.AppServices.escapeHtml(n.icon)}",
    ),
    "solstone/observe/sense.py": (
        'return "🎙️"',
        'return "👁️"',
        'return "🤖"',
        'icon = "🤖"',
        'icon = "🎙️"',
        'icon = "👁️"',
    ),
    "solstone/observe/describe.py": ('"icon": "👁️",',),
    "solstone/convey/static/shell.html": (
        '<button id="notif-bell" title="enable browser notifications" aria-label="enable browser notifications">🔔</button>',
    ),
}

OUT_OF_SCOPE_GLYPHS = {
    "solstone/apps/entities/workspace.html": (
        "⚠️ this will permanently remove this entity from all detected day files. this action cannot be undone.",
        "⚠️ this will permanently delete this entity and all associated data. this action cannot be undone.",
        "starBtn.textContent = '☆';",
    ),
    "solstone/apps/health/static/health.js": (
        '"LOGS_SERVICE_COLLAPSED": "── {service} ── ({n} lines, ★ {errors} errors)"',
        "const icon = e.type === 'agent' ? '⚙' : e.type === 'import' ? '↓' : '⚠';",
        "el.textContent = `⚠ Disconnected (${agoText})`;",
    ),
    "solstone/observe/categories/meeting.py": (
        'video = "📹" if p.get("video") else "🔇"',
    ),
    "solstone/apps/__init__.py": ('icon = metadata.get("icon", "📦")',),
    "solstone/think/facets.py": (
        '"emoji": "📦",',
        'emoji: str = "📦",',
    ),
    "solstone/apps/settings/routes.py": (
        'emoji: Icon emoji (optional, default: "📦")',
        'emoji = data.get("emoji", "📦")',
    ),
    "solstone/think/tools/call.py": (
        'emoji: str = typer.Option("📦", "--emoji", help="Icon emoji."),',
    ),
    "solstone/apps/network/workspace.html": (
        '<p class="link-pair-success-check" aria-hidden="true">✓</p>',
    ),
    "solstone/apps/sol/workspace.html": (
        "successBadge.textContent = '✓ ' + successCount;",
        "failBadge.textContent = '✗ ' + agent.failed_count;",
        "statusIcon.textContent = '\\u23f3';",
        "statusIcon.textContent = '\\u2717';",
        "statusIcon.textContent = '\\u2713';",
    ),
    "solstone/convey/static/shell.html": (
        '<button id="hamburger" aria-label="toggle navigation" aria-expanded="false">☰</button>',
    ),
}

NOTIFICATION_ICON_EMITTERS = {
    "solstone/apps/transcripts/workspace.html": (
        ("icon: '🗑️'", "icon: 'trash-2'"),
        ("icon: '⚠️'", "icon: 'triangle-alert'"),
        ("icon: '↩️'", "icon: 'undo-2'"),
        ("icon: '⏱️'", "icon: 'timer'"),
    ),
    "solstone/apps/settings/workspace.html": (
        ("icon: '❌'", "icon: 'circle-x'"),
        ("icon: '✅'", "icon: 'circle-check'"),
        ("icon: '🔄'", "icon: 'refresh-cw'"),
    ),
    "solstone/apps/entities/workspace.html": (
        ("icon: '🗑️'", "icon: 'trash-2'"),
        ("icon: '↩️'", "icon: 'undo-2'"),
        ("icon: '⏱️'", "icon: 'timer'"),
        ("icon: '⚠️'", "icon: 'triangle-alert'"),
    ),
    "solstone/apps/support/background.html": (("icon: '🛟'", "icon: 'life-buoy'"),),
    "solstone/convey/static/websocket.js": (
        ("icon: '✓'", "icon: 'check'"),
        ("icon: '⚠️'", "icon: 'triangle-alert'"),
    ),
    "solstone/convey/static/app.js": (
        ("icon: options.icon || '📬'", "_defaultIconName: 'mailbox'"),
        (
            "body: notif.message,\n          icon: notif.icon,",
            "tag: `${notif.app}-${notif.id}`",
        ),
    ),
    "solstone/observe/sense.py": (
        ('return "🎙️"', 'return "mic-vocal"'),
        ('return "👁️"', 'return "eye"'),
        ('return "🤖"', 'return "bot"'),
        ('icon = "🤖"', "icon=_handler_icon(handler_name)"),
        ('icon = "🎙️"', "icon=_handler_icon(handler_name)"),
        ('icon = "👁️"', "icon=_handler_icon(handler_name)"),
    ),
    "solstone/observe/describe.py": (('"icon": "👁️",', '"icon": "eye",'),),
}


def _node_or_skip() -> str:
    node = shutil.which("node")
    if node is None:
        pytest.skip("node is not available")
    return node


def _generated_icons_from_js(source: str) -> dict[str, str]:
    prefix = "  const ICONS = Object.freeze("
    start = source.index(prefix) + len(prefix)
    end = source.index("\n  });", start) + len("\n  }")
    return json.loads(source[start:end])


def _assert_icon_maps_match(actual: dict[str, str], expected: dict[str, str]) -> None:
    assert actual.keys() == expected.keys()
    mismatched = [
        name for name in sorted(expected) if actual.get(name) != expected[name]
    ]
    assert not mismatched, "mismatched Lucide icon(s): " + ", ".join(mismatched)


def _icon_slot_svgs(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    slots: list[str] = []
    for match in ICON_SLOT_OPEN_RE.finditer(text):
        tag = match.group("tag")
        close = re.search(
            rf"</{re.escape(tag)}\s*>", text[match.end() :], re.IGNORECASE
        )
        if close is None:
            continue
        slot_body = text[match.end() : match.end() + close.start()]
        slots.extend(svg.group(0) for svg in SVG_RE.finditer(slot_body))

    # Regression tripwire: the removed emptyIcons maps are icon sources too.
    for match in EMPTY_ICONS_MAP_RE.finditer(text):
        slots.extend(svg.group(0) for svg in SVG_RE.finditer(match.group(0)))
    return slots


def _converted_source() -> str:
    return "\n".join(path.read_text(encoding="utf-8") for path in CONVERTED_FILES)


def _combine_surrogate_pairs(text: str) -> str:
    chars: list[str] = []
    index = 0
    while index < len(text):
        codepoint = ord(text[index])
        if 0xD800 <= codepoint <= 0xDBFF and index + 1 < len(text):
            next_codepoint = ord(text[index + 1])
            if 0xDC00 <= next_codepoint <= 0xDFFF:
                chars.append(
                    chr(
                        0x10000
                        + ((codepoint - 0xD800) << 10)
                        + (next_codepoint - 0xDC00)
                    )
                )
                index += 2
                continue
        chars.append(text[index])
        index += 1
    return "".join(chars)


def _glyph_scan_source(text: str) -> str:
    def decode_match(match: re.Match[str]) -> str:
        raw_codepoint = match.group("long") or match.group("short")
        return chr(int(raw_codepoint, 16))

    decoded = UNICODE_ESCAPE_RE.sub(decode_match, text)
    return text + "\n" + _combine_surrogate_pairs(decoded)


def requested_icon_names(source: str) -> set[str]:
    return {match.group("name") for match in CALL_RE.finditer(source)}


def _inline_slot_icon_names(path: Path, svg_to_name: Mapping[str, str]) -> set[str]:
    return {svg_to_name[svg] for svg in _icon_slot_svgs(path) if svg in svg_to_name}


def _rule_bodies_for_svg_class(source: str, class_name: str) -> list[str]:
    class_ref = re.escape(class_name)
    bodies: list[str] = []
    style_blocks = [
        match.group("body") for match in STYLE_BLOCK_RE.finditer(source)
    ] or [source]
    for style_source in style_blocks:
        for match in re.finditer(
            r"(?P<selectors>[^{}]+)\{(?P<body>[^{}]+)\}", style_source
        ):
            selectors = [
                selector.strip() for selector in match.group("selectors").split(",")
            ]
            if any(
                re.search(
                    rf"(?<![A-Za-z0-9_-])\.{class_ref}(?![A-Za-z0-9_-])[^{{}}]*\bsvg\b",
                    selector,
                )
                for selector in selectors
            ):
                bodies.append(match.group("body"))
    return bodies


def test_convey_icons_runtime_accessor_in_browser_vm():
    node = _node_or_skip()
    script = textwrap.dedent(
        """
        const assert = require('assert');
        const fs = require('fs');
        const vm = require('vm');
        const source = fs.readFileSync(process.argv[1], 'utf8');
        const window = {};
        const context = { window };
        vm.createContext(context);
        vm.runInContext(source, context);
        assert(window.ConveyIcons);
        const names = JSON.parse(process.argv[2]);
        for (const name of names) {
          const svg = window.ConveyIcons.svg(name);
          assert(svg && svg.includes('<svg'), name + ' did not return SVG markup');
        }
        assert.strictEqual(window.ConveyIcons.svg('not-a-real-icon'), '');
        assert.doesNotThrow(() => window.ConveyIcons.svg(null));
        """
    )
    subprocess.run(
        [node, "-e", script, str(CONVEY_ICONS_JS), json.dumps(CONVEY_ICON_NAMES)],
        check=True,
        text=True,
    )


def test_convey_icons_match_lucide_and_generated_output():
    expected = selected_icons(load_lucide_icons())
    source = CONVEY_ICONS_JS.read_text(encoding="utf-8")
    _assert_icon_maps_match(_generated_icons_from_js(source), expected)
    assert source == render_convey_icons_js(expected)


def test_convey_icon_comparator_rejects_mismatched_svg():
    expected = {"search": "<svg>right</svg>"}
    actual = {"search": "<svg>wrong</svg>"}
    with pytest.raises(AssertionError, match="mismatched Lucide icon"):
        _assert_icon_maps_match(actual, expected)


def test_icon_slot_detector_finds_multiline_slot_svg(tmp_path: Path):
    fixture = tmp_path / "workspace.html"
    fixture.write_text(
        textwrap.dedent(
            """
            <div class="surface-state-icon" aria-hidden="true">
              <svg viewBox="0 0 24 24"><path d="M0 0h24v24H0z"></path></svg>
            </div>
            """
        ),
        encoding="utf-8",
    )

    assert _icon_slot_svgs(fixture) == [
        '<svg viewBox="0 0 24 24"><path d="M0 0h24v24H0z"></path></svg>'
    ]


def test_convey_icon_call_pattern_extracts_supported_literals_and_unknowns():
    source = "\n".join(
        (
            "window.ConveyIcons.svg('bell')",
            'window.ConveyIcons.svg("bell-off")',
            "window.ConveyIcons.svg(`trash-2`)",
            "window.ConveyIcons?.svg('sparkles')",
        )
    )
    assert requested_icon_names(source) == {"bell", "bell-off", "trash-2", "sparkles"}

    requested = requested_icon_names("window.ConveyIcons?.svg('not-real')")
    assert requested - set(CONVEY_ICON_NAMES) == {"not-real"}


def test_convey_icon_call_sites_are_allow_listed_and_complete():
    requested = requested_icon_names(_converted_source())
    svg_to_name = {svg: name for name, svg in load_lucide_icons().items()}
    inline = set()
    for path in CONVERTED_FILES:
        inline.update(_inline_slot_icon_names(path, svg_to_name))
    used = requested | inline
    allow_list = set(CONVEY_ICON_NAMES)
    assert used <= allow_list
    assert allow_list <= used


def test_convey_icon_slots_have_no_unvouched_inline_svg():
    lucide_values = set(load_lucide_icons().values())
    offenders = []
    for path in CONVERTED_FILES:
        for svg in _icon_slot_svgs(path):
            if svg not in lucide_values:
                offenders.append(f"{path.relative_to(ROOT)}: {svg[:80]}")
    assert not offenders, "unvouched icon-slot SVG(s): " + "; ".join(offenders)


def test_l2_icon_slot_css_declares_current_color_stroke_and_size():
    required = {
        "width": "1em",
        "height": "1em",
        "stroke-width": "1.5",
        "stroke": "currentColor",
        "fill": "none",
    }
    offenders = []
    for class_name, path in L2_ICON_SLOT_CSS_FILES.items():
        text = path.read_text(encoding="utf-8")
        bodies = _rule_bodies_for_svg_class(text, class_name)
        if not bodies:
            offenders.append(f"{path.relative_to(ROOT)}: .{class_name} svg missing")
            continue
        joined = "\n".join(bodies)
        for prop, value in required.items():
            if not re.search(
                rf"{re.escape(prop)}\s*:\s*{re.escape(value)}\s*;", joined
            ):
                offenders.append(
                    f"{path.relative_to(ROOT)}: .{class_name} svg missing {prop}: {value}"
                )
    assert not offenders


def test_converted_glyphs_are_gone_and_out_of_scope_glyphs_survive():
    for rel_path, snippets in CONVERTED_GLYPH_RESIDUE.items():
        text = _glyph_scan_source((ROOT / rel_path).read_text(encoding="utf-8"))
        for snippet in snippets:
            assert snippet not in text, f"converted glyph residue remains: {rel_path}"

    for rel_path, snippets in OUT_OF_SCOPE_GLYPHS.items():
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        for snippet in snippets:
            assert snippet in text, f"out-of-scope glyph changed: {rel_path}"


def test_notification_icon_emitters_use_lucide_names():
    for rel_path, rows in NOTIFICATION_ICON_EMITTERS.items():
        raw_text = (ROOT / rel_path).read_text(encoding="utf-8")
        scanned = _glyph_scan_source(raw_text)
        for old_snippet, new_snippet in rows:
            assert old_snippet not in scanned, f"notification glyph remains: {rel_path}"
            assert new_snippet in raw_text, (
                f"notification icon name missing: {rel_path}"
            )


def test_glyph_scan_source_decodes_unicode_escape_icon_literal():
    source = "icon: '\\uD83D\\uDD04'"

    assert "icon: '🔄'" in _glyph_scan_source(source)


def test_l2_icon_accessibility_and_call_idiom_are_preserved():
    l2_paths = (
        ROOT / "solstone" / "apps" / "entities" / "workspace.html",
        ROOT / "solstone" / "apps" / "health" / "workspace.html",
        ROOT / "solstone" / "apps" / "health" / "static" / "health.js",
        ROOT / "solstone" / "apps" / "network" / "workspace.html",
        ROOT / "solstone" / "apps" / "sol" / "workspace.html",
        ROOT / "solstone" / "convey" / "static" / "status_pane.js",
        ROOT / "solstone" / "convey" / "static" / "shell.html",
    )
    l2_source = "\n".join(path.read_text(encoding="utf-8") for path in l2_paths)
    l2_runtime_names = {
        "trash-2",
        "sparkles",
        "bell",
        "bell-off",
        "alarm-clock",
        "brain",
        "wrench",
        "coins",
        "lock",
        "mic-vocal",
    }
    offenders = []
    for match in CALL_RE.finditer(l2_source):
        if match.group("name") not in l2_runtime_names:
            continue
        call_end = match.end()
        if "?." not in match.group(0) or not re.match(
            r"\s*\|\|\s*''", l2_source[call_end:]
        ):
            offenders.append(match.group(0))
    assert not offenders
    assert not re.search(r"window\.ConveyIcons\??\.svg\([^)]*,", l2_source)

    sol = (ROOT / "solstone" / "apps" / "sol" / "workspace.html").read_text(
        encoding="utf-8"
    )
    assert '<span class="sr-only">thinking events</span>' in sol
    assert '<span class="sr-only">tool calls</span>' in sol
    assert '<span class="sr-only">cost</span>' in sol

    entities = (ROOT / "solstone" / "apps" / "entities" / "workspace.html").read_text(
        encoding="utf-8"
    )
    assert "deleteBtn.setAttribute('aria-label', deleteBtn.title);" in entities
    assert "generateBtn.setAttribute('aria-label', generateBtn.title);" in entities
    assert "voiceprint.setAttribute('role', 'img');" in entities
    assert "voiceprint.setAttribute('aria-label', voiceprint.title);" in entities
    assert "voiceIcon.setAttribute('role', 'img');" in entities
    assert "voiceIcon.setAttribute('aria-label', voiceIcon.title);" in entities
    assert "deleteBtn.onclick = (e) => {" in entities
    assert "generateBtn.onclick = () => {" in entities
    assert 'aria-hidden="true"' in l2_source


def test_l2_health_trust_text_and_escaping_are_preserved():
    health_js = (
        ROOT / "solstone" / "apps" / "health" / "static" / "health.js"
    ).read_text(encoding="utf-8")
    health_html = (ROOT / "solstone" / "apps" / "health" / "workspace.html").read_text(
        encoding="utf-8"
    )

    assert (
        "const escapeHtml = (value) => window.AppServices.escapeHtml(value);"
        in health_js
    )
    assert "escapeHtml(s.host)" in health_js
    assert "escapeHtml(key)" in health_js
    assert "escapeHtml(next)" in health_js

    assert "</span> all data stored locally on your device</div>" in health_html
    assert "lockIcon + ' all data stored locally on your device'" in health_js
    assert (
        "lockIcon + ' All data stored locally · Syncing to ' + escapeHtml(s.host)"
        in health_js
    )


def test_surface_state_error_actions_are_wired_after_conversion():
    support_js = (
        ROOT / "solstone" / "apps" / "support" / "static" / "support.js"
    ).read_text(encoding="utf-8")
    sol = (ROOT / "solstone" / "apps" / "sol" / "workspace.html").read_text(
        encoding="utf-8"
    )
    support = (ROOT / "solstone" / "apps" / "support" / "workspace.html").read_text(
        encoding="utf-8"
    )

    assert re.search(
        r"const retryBtn = list\.querySelector\('\.surface-state-retry'\);\s*"
        r"if \(retryBtn\) retryBtn\.addEventListener\('click', \(\) => "
        r"loadTickets\(deps\)\);",
        support_js,
    )
    assert re.search(
        r"loadingView\.querySelector\('\.surface-state-retry'\)\.onclick = "
        r"\(\) => loadTalents\(\);",
        sol,
    )
    assert re.search(
        r"const errorBackBtn = detail\.querySelector\('\.surface-state-secondary'\);\s*"
        r"if \(errorBackBtn\) errorBackBtn\.addEventListener\('click', \(\) => \{\s*"
        r"detail\.classList\.remove\('active'\);\s*"
        r"detail\.innerHTML = '';\s*"
        r"list\.style\.display = '';\s*"
        r"\}\);",
        support,
    )
