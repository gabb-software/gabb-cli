"""Transcript parser for Claude Code conversation JSON format.

Parses the Messages API format used by Claude Code:
{
  "messages": [
    {"role": "user", "content": "Fix the auth bug"},
    {"role": "assistant", "content": [
      {"type": "text", "text": "I'll look into this..."},
      {"type": "tool_use", "id": "toolu_01", "name": "Bash", "input": {"command": "grep -rn 'handleAuth' src/"}}
    ]},
    {"role": "user", "content": [
      {"type": "tool_result", "tool_use_id": "toolu_01", "content": "src/auth.ts:42: export function handleAuth..."}
    ]}
  ]
}
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .schemas import ToolCall, Turn, TranscriptAnalysis


def parse_transcript(data: dict[str, Any]) -> TranscriptAnalysis:
    """Parse a Claude Code transcript into structured analysis data.

    Args:
        data: Parsed JSON data from a Claude Code transcript.
              Can be in Messages API format or Claude Code output format.

    Returns:
        TranscriptAnalysis with extracted turns and tool calls.
    """
    analysis = TranscriptAnalysis()

    # Try to extract session ID and task from various formats
    analysis.session_id = data.get("session_id") or data.get("id")

    # Handle different transcript formats
    messages = data.get("messages", [])

    # If no messages, try Claude Code output format
    if not messages and "result" in data:
        # This is a Claude Code -p output, not a full transcript
        # Extract what we can from the single-turn result
        analysis.task_description = "Single-turn task"
        if "usage" in data:
            analysis.total_input_tokens = data["usage"].get("input_tokens", 0)
            analysis.total_output_tokens = data["usage"].get("output_tokens", 0)
        return analysis

    # Extract task description from first user message
    if messages and messages[0].get("role") == "user":
        first_content = messages[0].get("content", "")
        if isinstance(first_content, str):
            analysis.task_description = first_content[:200]
        elif isinstance(first_content, list):
            for block in first_content:
                if isinstance(block, dict) and block.get("type") == "text":
                    analysis.task_description = block.get("text", "")[:200]
                    break

    # Parse messages into turns
    current_turn: Turn | None = None
    turn_id = 0
    pending_tool_calls: dict[str, ToolCall] = {}  # Map tool_use_id -> ToolCall

    for msg in messages:
        role = msg.get("role", "")
        content = msg.get("content", "")

        if role == "assistant":
            # Start a new turn
            turn_id += 1
            current_turn = Turn(turn_id=turn_id)

            # Parse content blocks
            if isinstance(content, str):
                current_turn.assistant_text = content
            elif isinstance(content, list):
                text_parts = []
                for block in content:
                    if not isinstance(block, dict):
                        continue

                    block_type = block.get("type", "")

                    if block_type == "text":
                        text_parts.append(block.get("text", ""))

                    elif block_type == "tool_use":
                        tool_call = ToolCall(
                            tool_name=block.get("name", "unknown"),
                            tool_input=block.get("input", {}),
                            tool_use_id=block.get("id", ""),
                        )
                        current_turn.tool_calls.append(tool_call)
                        pending_tool_calls[tool_call.tool_use_id] = tool_call

                current_turn.assistant_text = "\n".join(text_parts)

            analysis.turns.append(current_turn)

        elif role == "user":
            # Check for tool results
            if isinstance(content, list):
                for block in content:
                    if not isinstance(block, dict):
                        continue

                    if block.get("type") == "tool_result":
                        tool_use_id = block.get("tool_use_id", "")
                        result_content = block.get("content", "")

                        # Find the corresponding tool call
                        if tool_use_id in pending_tool_calls:
                            tc = pending_tool_calls[tool_use_id]
                            if isinstance(result_content, str):
                                tc.result_content = result_content
                            elif isinstance(result_content, list):
                                # Handle structured content (e.g., images)
                                text_parts = []
                                for part in result_content:
                                    if isinstance(part, dict) and part.get("type") == "text":
                                        text_parts.append(part.get("text", ""))
                                tc.result_content = "\n".join(text_parts)

    return analysis


def load_transcript(path: Path | str) -> TranscriptAnalysis:
    """Load and parse a transcript from a file.

    Args:
        path: Path to JSON transcript file.

    Returns:
        TranscriptAnalysis with extracted data.

    Raises:
        FileNotFoundError: If file doesn't exist.
        json.JSONDecodeError: If file isn't valid JSON.
    """
    path = Path(path)
    with open(path, "r") as f:
        data = json.load(f)
    return parse_transcript(data)


def load_jsonl_transcript(path: Path | str) -> list[TranscriptAnalysis]:
    """Load transcripts from a JSONL file (one JSON object per line).

    Args:
        path: Path to JSONL file.

    Returns:
        List of TranscriptAnalysis objects.
    """
    path = Path(path)
    results = []
    with open(path, "r") as f:
        for line in f:
            line = line.strip()
            if line:
                data = json.loads(line)
                results.append(parse_transcript(data))
    return results
