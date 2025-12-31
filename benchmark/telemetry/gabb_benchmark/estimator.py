"""Token estimation for transcript analysis.

Uses tiktoken for accurate token counting, with fallback to character-based
estimation if tiktoken is not available.
"""

from __future__ import annotations

import json
from typing import Any

from .schemas import ToolCall, TranscriptAnalysis

# Try to import tiktoken for accurate token counting
try:
    import tiktoken

    # Use cl100k_base encoding (used by Claude and GPT-4)
    _ENCODER = tiktoken.get_encoding("cl100k_base")
    HAS_TIKTOKEN = True
except ImportError:
    _ENCODER = None
    HAS_TIKTOKEN = False


# Approximate tokens per character (used as fallback)
CHARS_PER_TOKEN = 4.0


def count_tokens(text: str) -> int:
    """Count tokens in a string.

    Uses tiktoken if available, otherwise estimates from character count.

    Args:
        text: The text to count tokens for.

    Returns:
        Estimated token count.
    """
    if not text:
        return 0

    if HAS_TIKTOKEN and _ENCODER:
        return len(_ENCODER.encode(text))
    else:
        # Fallback: estimate based on character count
        # This is approximate but reasonable for English/code text
        return max(1, int(len(text) / CHARS_PER_TOKEN))


def count_tokens_json(obj: Any) -> int:
    """Count tokens in a JSON-serializable object.

    Args:
        obj: Any JSON-serializable object.

    Returns:
        Estimated token count for the JSON representation.
    """
    text = json.dumps(obj)
    return count_tokens(text)


def estimate_tool_call_tokens(tc: ToolCall) -> None:
    """Estimate tokens for a tool call, updating the ToolCall in place.

    Args:
        tc: ToolCall to estimate (modified in place).
    """
    # Input tokens: tool name + serialized input
    input_text = f"{tc.tool_name}: {json.dumps(tc.tool_input)}"
    tc.input_tokens = count_tokens(input_text)

    # Output tokens: result content
    if tc.result_content:
        tc.result_tokens = count_tokens(tc.result_content)


def estimate_transcript_tokens(analysis: TranscriptAnalysis) -> None:
    """Estimate tokens for all parts of a transcript analysis.

    Updates token counts in the TranscriptAnalysis and its tool calls in place.

    Args:
        analysis: TranscriptAnalysis to process (modified in place).
    """
    cumulative_input = 0
    total_output = 0
    file_content_tokens = 0

    # Add tokens for task description (initial context)
    if analysis.task_description:
        cumulative_input += count_tokens(analysis.task_description)

    for turn in analysis.turns:
        # Track cumulative input at start of turn
        turn.input_tokens = cumulative_input

        # Estimate output tokens from assistant text
        turn_output = count_tokens(turn.assistant_text)

        # Process tool calls
        for tc in turn.tool_calls:
            estimate_tool_call_tokens(tc)

            # Add tool call overhead
            turn_output += tc.input_tokens

            # Tool results contribute to next turn's input
            cumulative_input += tc.result_tokens

            # Track file content tokens (Read tool results are file content)
            if tc.tool_name == "Read":
                file_content_tokens += tc.result_tokens
            elif tc.tool_name in ("Bash", "Grep", "Glob"):
                # Some portion of Bash/Grep output might be file listings
                # We'll count 50% as "file content" for these
                file_content_tokens += tc.result_tokens // 2

        turn.output_tokens = turn_output
        total_output += turn_output

        # Add assistant text to cumulative input for next turn
        cumulative_input += turn_output

    analysis.total_input_tokens = cumulative_input
    analysis.total_output_tokens = total_output
    analysis.file_content_tokens = file_content_tokens


def estimate_gabb_tool_tokens(tool_name: str, result_size: str = "typical") -> int:
    """Estimate tokens for a hypothetical gabb tool call result.

    This provides rough estimates for what gabb tool results would consume,
    useful for calculating potential savings.

    Args:
        tool_name: Name of the gabb tool (e.g., "gabb_symbol", "gabb_usages").
        result_size: Expected result size - "small", "typical", or "large".

    Returns:
        Estimated token count for the tool result.
    """
    # Typical token counts for gabb tool results
    # These are based on typical output sizes from gabb tools
    estimates = {
        "gabb_symbol": {"small": 50, "typical": 100, "large": 200},
        "gabb_symbols": {"small": 100, "typical": 300, "large": 800},
        "gabb_definition": {"small": 50, "typical": 150, "large": 400},
        "gabb_usages": {"small": 100, "typical": 250, "large": 600},
        "gabb_structure": {"small": 50, "typical": 200, "large": 500},
        "gabb_callers": {"small": 50, "typical": 200, "large": 500},
        "gabb_callees": {"small": 50, "typical": 200, "large": 500},
        "gabb_implementations": {"small": 50, "typical": 150, "large": 400},
    }

    tool_estimates = estimates.get(tool_name, {"small": 50, "typical": 150, "large": 400})
    return tool_estimates.get(result_size, tool_estimates["typical"])
