"""Tests for transcript parser."""

import json
from pathlib import Path

import pytest

from gabb_benchmark.parser import parse_transcript, load_transcript
from gabb_benchmark.schemas import TranscriptAnalysis


def test_parse_simple_transcript(simple_transcript_path: Path):
    """Test parsing a simple transcript with grep and read."""
    analysis = load_transcript(simple_transcript_path)

    assert isinstance(analysis, TranscriptAnalysis)
    assert analysis.task_description.startswith("Find where")
    assert len(analysis.turns) == 3

    # First turn has one Bash tool call
    assert len(analysis.turns[0].tool_calls) == 1
    assert analysis.turns[0].tool_calls[0].tool_name == "Bash"
    assert "grep" in analysis.turns[0].tool_calls[0].tool_input["command"]

    # Second turn has one Read tool call
    assert len(analysis.turns[1].tool_calls) == 1
    assert analysis.turns[1].tool_calls[0].tool_name == "Read"

    # Third turn has no tool calls (just text response)
    assert len(analysis.turns[2].tool_calls) == 0


def test_parse_multi_tool_transcript(multi_tool_transcript_path: Path):
    """Test parsing a transcript with parallel tool calls."""
    analysis = load_transcript(multi_tool_transcript_path)

    assert len(analysis.turns) == 3

    # First turn has two parallel tool calls
    assert len(analysis.turns[0].tool_calls) == 2
    tool_names = {tc.tool_name for tc in analysis.turns[0].tool_calls}
    assert tool_names == {"Bash", "Grep"}


def test_parse_empty_transcript():
    """Test parsing an empty transcript."""
    analysis = parse_transcript({"messages": []})

    assert len(analysis.turns) == 0
    assert analysis.task_description == ""


def test_parse_user_only_message():
    """Test parsing with only user messages (no assistant response yet)."""
    data = {
        "messages": [
            {"role": "user", "content": "Hello!"}
        ]
    }
    analysis = parse_transcript(data)

    assert len(analysis.turns) == 0
    assert analysis.task_description == "Hello!"


def test_tool_result_attached_to_call():
    """Test that tool results are attached to their corresponding calls."""
    data = {
        "messages": [
            {"role": "user", "content": "Test"},
            {
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "toolu_123", "name": "Bash", "input": {"command": "echo hi"}}
                ]
            },
            {
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_123", "content": "hi"}
                ]
            }
        ]
    }
    analysis = parse_transcript(data)

    assert len(analysis.turns) == 1
    assert len(analysis.turns[0].tool_calls) == 1
    tc = analysis.turns[0].tool_calls[0]
    assert tc.tool_use_id == "toolu_123"
    assert tc.result_content == "hi"
