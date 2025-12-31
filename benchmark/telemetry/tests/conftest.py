"""Pytest configuration and fixtures."""

from pathlib import Path

import pytest


FIXTURES_DIR = Path(__file__).parent / "fixtures"


@pytest.fixture
def simple_transcript_path() -> Path:
    """Path to a simple transcript with grep and read."""
    return FIXTURES_DIR / "simple_transcript.json"


@pytest.fixture
def multi_tool_transcript_path() -> Path:
    """Path to a transcript with multiple parallel tool calls."""
    return FIXTURES_DIR / "multi_tool_transcript.json"


@pytest.fixture
def claude_code_native_path() -> Path:
    """Path to a Claude Code native JSONL transcript."""
    return FIXTURES_DIR / "claude_code_native.jsonl"
