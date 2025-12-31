"""Tests for transcript parser."""

import json
from pathlib import Path

import pytest

from gabb_benchmark.parser import (
    parse_transcript,
    load_transcript,
    load_jsonl_transcript,
    parse_claude_code_jsonl,
)
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


# Claude Code native JSONL format tests


def test_load_claude_code_native_jsonl(claude_code_native_path: Path):
    """Test parsing Claude Code native JSONL format."""
    analysis = load_jsonl_transcript(claude_code_native_path)

    assert isinstance(analysis, TranscriptAnalysis)
    assert analysis.session_id == "test-session-123"
    assert "QDP" in analysis.task_description


def test_claude_code_jsonl_extracts_turns(claude_code_native_path: Path):
    """Test that turns are correctly extracted from Claude Code JSONL."""
    analysis = load_jsonl_transcript(claude_code_native_path)

    # Should have 4 assistant turns (text, tool_use, text+tool_use, text)
    assert len(analysis.turns) == 4


def test_claude_code_jsonl_extracts_tool_calls(claude_code_native_path: Path):
    """Test that tool calls are extracted from Claude Code JSONL."""
    analysis = load_jsonl_transcript(claude_code_native_path)

    # Find all tool calls
    all_tool_calls = []
    for turn in analysis.turns:
        all_tool_calls.extend(turn.tool_calls)

    assert len(all_tool_calls) == 2

    # First tool call is gabb_symbols
    assert all_tool_calls[0].tool_name == "mcp__gabb__gabb_symbols"
    assert all_tool_calls[0].tool_input["name_contains"] == "QDP"
    assert all_tool_calls[0].tool_input["kind"] == "class"

    # Second tool call is gabb_structure
    assert all_tool_calls[1].tool_name == "mcp__gabb__gabb_structure"
    assert all_tool_calls[1].tool_input["file"] == "astropy/io/ascii/qdp.py"


def test_claude_code_jsonl_extracts_tool_results(claude_code_native_path: Path):
    """Test that tool results are matched to tool calls."""
    analysis = load_jsonl_transcript(claude_code_native_path)

    # Find tool calls with results
    tool_calls_with_results = []
    for turn in analysis.turns:
        for tc in turn.tool_calls:
            if tc.result_content:
                tool_calls_with_results.append(tc)

    assert len(tool_calls_with_results) == 2

    # First result contains QDP classes
    assert "QDPSplitter" in tool_calls_with_results[0].result_content
    assert "QDP" in tool_calls_with_results[0].result_content

    # Second result contains structure JSON
    assert "symbols" in tool_calls_with_results[1].result_content


def test_claude_code_jsonl_extracts_token_usage(claude_code_native_path: Path):
    """Test that token usage is extracted from Claude Code JSONL."""
    analysis = load_jsonl_transcript(claude_code_native_path)

    # Total tokens should be sum of all assistant message usage
    # Based on fixture: 1000+1050+1200+1300 = 4550 input, 50+100+75+60 = 285 output
    assert analysis.total_input_tokens == 4550
    assert analysis.total_output_tokens == 285


def test_claude_code_jsonl_skips_queue_operations():
    """Test that queue-operation records are skipped."""
    records = [
        {"type": "queue-operation", "operation": "dequeue", "sessionId": "session-1"},
        {"type": "user", "sessionId": "session-1", "message": {"role": "user", "content": "Hello"}},
    ]
    analysis = parse_claude_code_jsonl(records)

    assert analysis.session_id == "session-1"
    assert len(analysis.turns) == 0  # No assistant turns yet


def test_format_detection_claude_code():
    """Test that Claude Code JSONL format is correctly detected."""
    from gabb_benchmark.parser import load_jsonl_transcript
    import tempfile

    # Create a temp file with Claude Code format
    with tempfile.NamedTemporaryFile(mode="w", suffix=".jsonl", delete=False) as f:
        f.write('{"type":"user","sessionId":"s1","message":{"role":"user","content":"Hi"}}\n')
        f.write('{"type":"assistant","sessionId":"s1","message":{"content":[{"type":"text","text":"Hello!"}]}}\n')
        temp_path = f.name

    try:
        analysis = load_jsonl_transcript(temp_path)
        assert analysis.session_id == "s1"
        assert len(analysis.turns) == 1
    finally:
        Path(temp_path).unlink()


def test_format_detection_messages_api():
    """Test that Messages API format is still detected in JSONL context."""
    from gabb_benchmark.parser import load_jsonl_transcript
    import tempfile

    # Create a temp file with Messages API format (single JSON per line)
    with tempfile.NamedTemporaryFile(mode="w", suffix=".jsonl", delete=False) as f:
        f.write('{"messages":[{"role":"user","content":"Hi"},{"role":"assistant","content":"Hello!"}]}\n')
        temp_path = f.name

    try:
        analysis = load_jsonl_transcript(temp_path)
        assert len(analysis.turns) == 1
    finally:
        Path(temp_path).unlink()
