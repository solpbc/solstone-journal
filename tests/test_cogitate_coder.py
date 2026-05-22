# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for cogitate coder mode: write flag, coder agent."""

import asyncio
import importlib
from pathlib import Path
from unittest.mock import AsyncMock, patch

# ---------------------------------------------------------------------------
# Write flag — Anthropic provider
# ---------------------------------------------------------------------------


class TestAnthropicWriteFlag:
    """Verify --allowedTools is controlled by config write flag."""

    def _provider(self):
        return importlib.import_module("solstone.think.providers.anthropic")

    @patch(
        "solstone.think.providers.anthropic.bundled.resolve_bundled_binary",
        return_value=Path("/usr/bin/claude"),
    )
    @patch("solstone.think.providers.anthropic.CLIRunner")
    def test_no_write_restricts_tools(self, mock_runner_cls, mock_resolve):
        """Without write flag, --allowedTools restricts to sol."""
        provider = self._provider()
        mock_instance = AsyncMock()
        mock_instance.run = AsyncMock(return_value="result")
        mock_instance.cli_session_id = None
        mock_runner_cls.return_value = mock_instance

        config = {"prompt": "test", "model": "claude-sonnet-4-20250514"}
        asyncio.run(provider.run_cogitate(config))

        cmd = mock_runner_cls.call_args.kwargs["cmd"]
        assert "--allowedTools" in cmd
        assert "Bash(sol *)" in cmd

    @patch(
        "solstone.think.providers.anthropic.bundled.resolve_bundled_binary",
        return_value=Path("/usr/bin/claude"),
    )
    @patch("solstone.think.providers.anthropic.CLIRunner")
    def test_write_true_grants_full_access(self, mock_runner_cls, mock_resolve):
        """With write=True, --allowedTools is omitted for full tool access."""
        provider = self._provider()
        mock_instance = AsyncMock()
        mock_instance.run = AsyncMock(return_value="result")
        mock_instance.cli_session_id = None
        mock_runner_cls.return_value = mock_instance

        config = {"prompt": "test", "model": "claude-sonnet-4-20250514", "write": True}
        asyncio.run(provider.run_cogitate(config))

        cmd = mock_runner_cls.call_args.kwargs["cmd"]
        assert "--allowedTools" not in cmd

    @patch(
        "solstone.think.providers.anthropic.bundled.resolve_bundled_binary",
        return_value=Path("/usr/bin/claude"),
    )
    @patch("solstone.think.providers.anthropic.CLIRunner")
    def test_write_false_restricts_tools(self, mock_runner_cls, mock_resolve):
        """Explicit write=False keeps restriction."""
        provider = self._provider()
        mock_instance = AsyncMock()
        mock_instance.run = AsyncMock(return_value="result")
        mock_instance.cli_session_id = None
        mock_runner_cls.return_value = mock_instance

        config = {"prompt": "test", "model": "claude-sonnet-4-20250514", "write": False}
        asyncio.run(provider.run_cogitate(config))

        cmd = mock_runner_cls.call_args.kwargs["cmd"]
        assert "--allowedTools" in cmd


# ---------------------------------------------------------------------------
# Write flag — OpenAI provider
# ---------------------------------------------------------------------------


class TestOpenAIWriteFlag:
    """Verify sandbox mode is controlled by config write flag."""

    def _provider(self):
        return importlib.import_module("solstone.think.providers.openai")

    @patch(
        "solstone.think.providers.openai.bundled.resolve_bundled_binary",
        return_value=Path("/usr/bin/codex"),
    )
    @patch("solstone.think.providers.openai.CLIRunner")
    def test_no_write_uses_readonly_sandbox(self, mock_runner_cls, mock_resolve):
        """Without write flag, sandbox is read-only."""
        provider = self._provider()
        mock_instance = AsyncMock()
        mock_instance.run = AsyncMock(return_value="result")
        mock_instance.cli_session_id = None
        mock_runner_cls.return_value = mock_instance

        config = {"prompt": "test", "model": "gpt-5.2"}
        asyncio.run(provider.run_cogitate(config))

        cmd = mock_runner_cls.call_args.kwargs["cmd"]
        # Find the -s flag and its value
        s_idx = cmd.index("-s")
        assert cmd[s_idx + 1] == "read-only"

    @patch(
        "solstone.think.providers.openai.bundled.resolve_bundled_binary",
        return_value=Path("/usr/bin/codex"),
    )
    @patch("solstone.think.providers.openai.CLIRunner")
    def test_write_true_uses_write_sandbox(self, mock_runner_cls, mock_resolve):
        """With write=True, sandbox is write."""
        provider = self._provider()
        mock_instance = AsyncMock()
        mock_instance.run = AsyncMock(return_value="result")
        mock_instance.cli_session_id = None
        mock_runner_cls.return_value = mock_instance

        config = {"prompt": "test", "model": "gpt-5.2", "write": True}
        asyncio.run(provider.run_cogitate(config))

        cmd = mock_runner_cls.call_args.kwargs["cmd"]
        s_idx = cmd.index("-s")
        assert cmd[s_idx + 1] == "workspace-write"

    @patch(
        "solstone.think.providers.openai.bundled.resolve_bundled_binary",
        return_value=Path("/usr/bin/codex"),
    )
    @patch("solstone.think.providers.openai.CLIRunner")
    def test_write_true_with_session_resume(self, mock_runner_cls, mock_resolve):
        """Write flag works correctly with session resume path."""
        provider = self._provider()
        mock_instance = AsyncMock()
        mock_instance.run = AsyncMock(return_value="result")
        mock_instance.cli_session_id = None
        mock_runner_cls.return_value = mock_instance

        config = {
            "prompt": "test",
            "model": "gpt-5.2",
            "write": True,
            "session_id": "sess-123",
        }
        asyncio.run(provider.run_cogitate(config))

        cmd = mock_runner_cls.call_args.kwargs["cmd"]
        s_idx = cmd.index("-s")
        assert cmd[s_idx + 1] == "workspace-write"
        assert "resume" in cmd


# ---------------------------------------------------------------------------
# Write flag — Google provider
# ---------------------------------------------------------------------------


class TestGoogleWriteFlag:
    """Verify Google SDK policy behavior is controlled by config write flag."""

    def test_no_write_uses_yolo_with_policy(self, tmp_path):
        """Without write flag, policy denies writes and non-sol shell commands."""
        from solstone.think.cogitate_policy import CogitatePolicy

        policy = CogitatePolicy(write=False, allowed_roots=[tmp_path])

        allowed, reason = policy.check("write_file", {"file_path": "x"})
        assert allowed is False
        assert reason.startswith("policy_deny:")
        assert policy.check("run_shell_command", {"command": "rm -rf /tmp/x"})[0] is (
            False
        )
        assert policy.check(
            "run_shell_command", {"command": "sol call activities list"}
        ) == (True, "ok")
        assert policy.check("read_file", {"file_path": str(tmp_path / "x")}) == (
            True,
            "ok",
        )

    def test_write_true_uses_yolo_mode(self, tmp_path):
        """With write=True, policy allows all tool calls."""
        from solstone.think.cogitate_policy import CogitatePolicy

        policy = CogitatePolicy(write=True, allowed_roots=[tmp_path])

        assert policy.check("write_file", {"file_path": "x"}) == (True, "ok")
        assert policy.check("run_shell_command", {"command": "rm -rf /tmp/x"}) == (
            True,
            "ok",
        )


# ---------------------------------------------------------------------------
# talent/coder.md existence and frontmatter
# ---------------------------------------------------------------------------


class TestCoderAgent:
    """Verify talent/coder.md exists with correct frontmatter."""

    def test_coder_md_exists(self):
        """talent/coder.md must exist in the repo."""
        from pathlib import Path

        coder_path = Path(__file__).parent.parent / "solstone" / "talent" / "coder.md"
        assert coder_path.exists(), "talent/coder.md not found"

    def test_coder_frontmatter(self):
        """coder.md must have write: true and type: cogitate."""
        from pathlib import Path

        import frontmatter

        coder_path = Path(__file__).parent.parent / "solstone" / "talent" / "coder.md"
        post = frontmatter.load(coder_path)

        assert post.metadata.get("type") == "cogitate"
        assert post.metadata.get("write") is True
        assert post.metadata.get("title") == "Coder"
        assert "description" in post.metadata

    def test_coder_references_coding_skill(self):
        """coder.md must reference the developer docs instead of inlining guidelines."""
        from pathlib import Path

        coder_path = Path(__file__).parent.parent / "solstone" / "talent" / "coder.md"
        content = coder_path.read_text(encoding="utf-8")

        # Should reference the developer guide/docs, not inline dev guidelines
        assert "AGENTS.md" in content
        assert "docs/project-structure.md" in content
        assert "single source of truth" in content

        docs_dir = Path(__file__).parent.parent / "docs"
        assert (docs_dir / "coding-standards.md").exists()
        assert (docs_dir / "project-structure.md").exists()
        assert (docs_dir / "testing.md").exists()
        assert (docs_dir / "environment.md").exists()
