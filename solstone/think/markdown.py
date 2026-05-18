# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Semantic markdown chunking formatter.

This module provides semantic chunking for markdown files, breaking documents
into context-aware chunks that preserve header hierarchy and structural context.
Each chunk is self-contained with its relevant headers included in the AST.
"""

import copy
import logging
from typing import Any

LOG = logging.getLogger(__name__)

_MAX_LINE_CHARS = 2048
_MAX_CHUNK_CHARS = 4096
_MAX_EXTRACTION_CHARS = 32_768
_EXTRACTION_BOUND_MARKER = (
    "\n\n[solstone: extraction output bounded before journaling - "
    "degenerate length sanitized/truncated]"
)

import mistune
from mistune.core import BlockState
from mistune.plugins.table import table
from mistune.renderers.markdown import MarkdownRenderer


class ExtendedMarkdownRenderer(MarkdownRenderer):
    """MarkdownRenderer extended with table support."""

    def table(self, token, state):
        return self.render_children(token, state) + "\n"

    def table_head(self, token, state):
        cells = token.get("children", [])
        if not cells:
            return ""

        header_line = (
            "| "
            + " | ".join(self.render_children(cell, state).strip() for cell in cells)
            + " |"
        )

        sep_parts = []
        for cell in cells:
            align = cell.get("attrs", {}).get("align")
            if align == "left":
                sep_parts.append(":---")
            elif align == "right":
                sep_parts.append("---:")
            elif align == "center":
                sep_parts.append(":---:")
            else:
                sep_parts.append("---")
        sep_line = "| " + " | ".join(sep_parts) + " |"

        return header_line + "\n" + sep_line + "\n"

    def table_body(self, token, state):
        return self.render_children(token, state)

    def table_row(self, token, state):
        cells = token.get("children", [])
        return (
            "| "
            + " | ".join(self.render_children(cell, state).strip() for cell in cells)
            + " |\n"
        )

    def table_cell(self, token, state):
        return self.render_children(token, state)


def extract_text(node) -> str:
    """Recursively extract raw text from a node for preview purposes."""
    if isinstance(node, str):
        return node
    if isinstance(node, list):
        return " ".join(extract_text(n) for n in node)
    if isinstance(node, dict):
        if "raw" in node:
            return node["raw"]
        if "children" in node:
            return extract_text(node["children"])
    return ""


def get_header_path(header_stack: list) -> list[dict]:
    """Extract header text path from header stack."""
    path = []
    for h in header_stack:
        level = h.get("attrs", {}).get("level", 1)
        text = extract_text(h.get("children", []))
        path.append({"level": level, "text": text})
    return path


def find_next_content_node(ast_data: list, start_idx: int) -> dict | None:
    """Find the next non-blank node after start_idx."""
    for i in range(start_idx + 1, len(ast_data)):
        if ast_data[i].get("type") != "blank_line":
            return ast_data[i]
    return None


def is_intro_paragraph(node: dict, next_content_node: dict | None) -> bool:
    """Check if a paragraph is an intro (precedes a list or table)."""
    if node.get("type") != "paragraph":
        return False
    if not next_content_node or next_content_node.get("type") not in ("list", "table"):
        return False
    return True


def is_simple_text_item(item: dict) -> bool:
    """Check if list item is simple text (no complex sub-structures)."""
    children = item.get("children", [])
    if len(children) != 1:
        return False
    child = children[0]
    if child.get("type") not in ("paragraph", "block_text"):
        return False
    return True


def is_definition_item(item: dict) -> bool:
    """Check if item matches **field:** value pattern (no trailing period)."""
    if not is_simple_text_item(item):
        return False
    text_block = item["children"][0]
    kids = text_block.get("children", [])
    if not kids or kids[0].get("type") != "strong":
        return False
    strong_text = extract_text(kids[0])
    following_text = extract_text(kids[1:]) if len(kids) > 1 else ""
    has_colon = strong_text.rstrip().endswith(
        ":"
    ) or following_text.lstrip().startswith(":")
    if not has_colon:
        return False
    full_text = extract_text(text_block).strip()
    return not full_text.endswith(".")


def is_definition_list(list_node: dict) -> bool:
    """Check if list is primarily definition-style (2+ matching items, >=50%)."""
    items = [c for c in list_node.get("children", []) if c.get("type") == "list_item"]
    if len(items) < 2:
        return False
    matches = sum(1 for item in items if is_definition_item(item))
    return matches >= 2 and matches >= len(items) * 0.5


def chunk_ast(ast_data: list) -> list[dict]:
    """Process Mistune AST into context-aware semantic chunks.

    Returns a list of dicts with:
        - index: chunk index
        - type: chunk type (paragraph, list_item, table_row, etc.)
        - header_path: list of {level, text} for header context
        - intro: optional intro paragraph text
        - preview: text preview of the chunk content
        - ast: the chunk's AST (headers + intro + content)
    """
    chunks = []
    header_stack = []
    intro_paragraph = None

    for i, node in enumerate(ast_data):
        node_type = node.get("type")
        next_content = find_next_content_node(ast_data, i)

        # Handle Headings (Context Builders)
        if node_type == "heading":
            level = node.get("attrs", {}).get("level", 1)
            header_stack = [
                h for h in header_stack if h.get("attrs", {}).get("level", 0) < level
            ]
            header_stack.append(node)
            intro_paragraph = None

        # Handle Paragraphs
        elif node_type == "paragraph":
            if is_intro_paragraph(node, next_content):
                intro_paragraph = node
            else:
                intro_paragraph = None
                chunk_ast_nodes = copy.deepcopy(header_stack)
                chunk_ast_nodes.append(node)
                chunks.append(
                    {
                        "index": len(chunks),
                        "type": "paragraph",
                        "header_path": get_header_path(header_stack),
                        "preview": extract_text(node)[:100],
                        "ast": chunk_ast_nodes,
                    }
                )

        # Handle Lists (Container Nodes)
        elif node_type == "list":
            if is_definition_list(node):
                chunk_ast_list = copy.deepcopy(header_stack)
                if intro_paragraph:
                    chunk_ast_list.append(copy.deepcopy(intro_paragraph))
                chunk_ast_list.append(node)
                chunks.append(
                    {
                        "index": len(chunks),
                        "type": "definition_list",
                        "header_path": get_header_path(header_stack),
                        "intro": (
                            extract_text(intro_paragraph)[:100]
                            if intro_paragraph
                            else None
                        ),
                        "preview": extract_text(node)[:100],
                        "ast": chunk_ast_list,
                    }
                )
            else:
                for item in node.get("children", []):
                    if item.get("type") == "list_item":
                        synthetic_list = copy.deepcopy(node)
                        synthetic_list["children"] = [item]

                        chunk_ast_list = copy.deepcopy(header_stack)
                        if intro_paragraph:
                            chunk_ast_list.append(copy.deepcopy(intro_paragraph))
                        chunk_ast_list.append(synthetic_list)
                        chunks.append(
                            {
                                "index": len(chunks),
                                "type": "list_item",
                                "header_path": get_header_path(header_stack),
                                "intro": (
                                    extract_text(intro_paragraph)[:100]
                                    if intro_paragraph
                                    else None
                                ),
                                "preview": extract_text(item)[:100],
                                "ast": chunk_ast_list,
                            }
                        )
            intro_paragraph = None

        # Handle Tables (Complex Container Nodes)
        elif node_type == "table":
            children = node.get("children", [])
            thead = next((c for c in children if c["type"] == "table_head"), None)
            tbody = next((c for c in children if c["type"] == "table_body"), None)

            if tbody:
                for row in tbody.get("children", []):
                    if row.get("type") == "table_row":
                        synthetic_table = copy.deepcopy(node)
                        synthetic_body = copy.deepcopy(tbody)
                        synthetic_body["children"] = [row]

                        new_children = []
                        if thead:
                            new_children.append(thead)
                        new_children.append(synthetic_body)
                        synthetic_table["children"] = new_children

                        chunk_ast_nodes = copy.deepcopy(header_stack)
                        if intro_paragraph:
                            chunk_ast_nodes.append(copy.deepcopy(intro_paragraph))
                        chunk_ast_nodes.append(synthetic_table)
                        chunks.append(
                            {
                                "index": len(chunks),
                                "type": "table_row",
                                "header_path": get_header_path(header_stack),
                                "intro": (
                                    extract_text(intro_paragraph)[:100]
                                    if intro_paragraph
                                    else None
                                ),
                                "preview": extract_text(row)[:100],
                                "ast": chunk_ast_nodes,
                            }
                        )
            intro_paragraph = None

        # Handle Block Code
        elif node_type == "block_code":
            chunk_ast_nodes = copy.deepcopy(header_stack)
            chunk_ast_nodes.append(node)
            info = node.get("attrs", {}).get("info", "")
            raw = node.get("raw", "")[:80]
            chunks.append(
                {
                    "index": len(chunks),
                    "type": "block_code",
                    "header_path": get_header_path(header_stack),
                    "preview": f"[{info}] {raw}" if info else raw,
                    "ast": chunk_ast_nodes,
                }
            )

        # Handle Blockquotes
        elif node_type == "block_quote":
            chunk_ast_nodes = copy.deepcopy(header_stack)
            chunk_ast_nodes.append(node)
            chunks.append(
                {
                    "index": len(chunks),
                    "type": "block_quote",
                    "header_path": get_header_path(header_stack),
                    "preview": extract_text(node)[:100],
                    "ast": chunk_ast_nodes,
                }
            )

        # Skip Thematic Breaks (no indexable content)

    return chunks


def parse_markdown(text: str) -> list:
    """Parse markdown text into AST tokens."""
    md = mistune.create_markdown(renderer=None, plugins=[table])
    return md(text)


def render_chunk(chunk: dict) -> str:
    """Render a chunk's AST back to markdown."""
    renderer = ExtendedMarkdownRenderer()
    return renderer(chunk["ast"], state=BlockState())


def chunk_markdown(text: str) -> list[dict]:
    """Parse markdown and return semantic chunks."""
    ast = parse_markdown(text)
    return chunk_ast(ast)


def sanitize_markdown(text: str) -> str:
    """Drop degenerate lines that exceed the max line length.

    AI models (notably older Gemini Flash) sometimes produce lines with
    thousands of repeated characters or whitespace-padded table cells.
    These are not useful content and bloat the index.
    """
    lines = text.split("\n")
    clean: list[str] = []
    dropped = 0
    for line in lines:
        if len(line) > _MAX_LINE_CHARS:
            dropped += 1
            continue
        clean.append(line)
    if dropped:
        LOG.warning(
            "Dropped %d line(s) exceeding %d chars during markdown sanitization",
            dropped,
            _MAX_LINE_CHARS,
        )
    return "\n".join(clean)


def bound_extraction_markdown(text: str) -> str:
    """Bound a degenerate extraction value before it is journaled.

    Phase-3 observe/describe markdown extraction can occasionally produce a
    runaway-generation blob (huge whitespace runs or millions of short
    repeated lines). Reuse sanitize_markdown() to drop over-long lines, then
    apply a hard total-character cap as the backstop sanitize alone misses
    (the many-short-lines shape). When the value is altered, append a
    human-readable marker so a consumer reading the journal artifact can tell
    it was bounded. Healthy output passes through byte-identical with no
    marker.
    """
    sanitized = sanitize_markdown(text)
    changed = sanitized != text
    budget = _MAX_EXTRACTION_CHARS - len(_EXTRACTION_BOUND_MARKER)
    truncated = len(sanitized) > budget
    if truncated:
        sanitized = sanitized[:budget]
        changed = True
    if not changed:
        return sanitized
    LOG.warning(
        "Bounded extraction markdown: %d -> %d chars (cap-truncated=%s)",
        len(text),
        len(sanitized) + len(_EXTRACTION_BOUND_MARKER),
        truncated,
    )
    return sanitized + _EXTRACTION_BOUND_MARKER


def _render_header_stub(raw_chunk: dict, original_size: int) -> str:
    """Render a header-only stub for an oversized chunk."""
    parts = []
    for h in raw_chunk.get("header_path", []):
        prefix = "#" * h["level"]
        parts.append(f"{prefix} {h['text']}")
    parts.append(f"\n[Content too large to index: {original_size:,} chars]")
    return "\n\n".join(parts)


def format_markdown(
    text: str,
    context: dict[str, Any] | None = None,
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """Format markdown text into semantic chunks.

    This is the formatter interface for markdown files. Each chunk contains
    its full context (headers, intro paragraphs) rendered back to markdown.

    Note: Unlike JSONL formatters, this does not return indexer metadata.
    Agent for markdown files is derived from path by extract_path_metadata().

    Args:
        text: Markdown text to chunk
        context: Optional context dict (unused, for formatter interface compatibility)

    Returns:
        Tuple of (chunks, meta) where:
            - chunks: List of {"markdown": str} dicts (timestamp omitted)
            - meta: Empty dict (no header or indexer - context is in each chunk,
              agent is path-derived)
    """
    text = sanitize_markdown(text)
    raw_chunks = chunk_markdown(text)
    chunks = []
    for rc in raw_chunks:
        rendered = render_chunk(rc)
        if len(rendered) > _MAX_CHUNK_CHARS:
            rendered = _render_header_stub(rc, len(rendered))
        chunks.append({"markdown": rendered})
    return chunks, {}
