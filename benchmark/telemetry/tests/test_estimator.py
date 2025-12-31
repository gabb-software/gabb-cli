"""Tests for token estimation."""

import pytest

from gabb_benchmark.estimator import (
    count_tokens,
    estimate_transcript_tokens,
    estimate_gabb_tool_tokens,
    HAS_TIKTOKEN,
)
from gabb_benchmark.parser import parse_transcript
from gabb_benchmark.classifier import classify_tool_calls


class TestCountTokens:
    """Tests for count_tokens function."""

    def test_empty_string(self):
        """Test counting tokens in empty string."""
        assert count_tokens("") == 0

    def test_simple_text(self):
        """Test counting tokens in simple text."""
        tokens = count_tokens("Hello, world!")

        # Should be a reasonable number (not 0, not huge)
        assert 1 <= tokens <= 10

    def test_code_text(self):
        """Test counting tokens in code."""
        code = """
def hello():
    print("Hello, world!")
"""
        tokens = count_tokens(code)

        # Code typically has more tokens per character
        assert tokens > 5

    def test_long_text(self):
        """Test that longer text has more tokens."""
        short = "Hello"
        long = "Hello " * 100

        short_tokens = count_tokens(short)
        long_tokens = count_tokens(long)

        assert long_tokens > short_tokens


class TestEstimateTranscriptTokens:
    """Tests for estimate_transcript_tokens function."""

    def test_simple_transcript(self):
        """Test estimating tokens for a simple transcript."""
        data = {
            "messages": [
                {"role": "user", "content": "Find the auth function"},
                {
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "I'll search for it."},
                        {
                            "type": "tool_use",
                            "id": "toolu_1",
                            "name": "Bash",
                            "input": {"command": "grep auth src/"},
                        },
                    ],
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_1",
                            "content": "src/auth.ts:10: function authenticate()",
                        }
                    ],
                },
            ]
        }

        analysis = parse_transcript(data)
        classify_tool_calls(analysis)
        estimate_transcript_tokens(analysis)

        # Should have non-zero tokens
        assert analysis.total_input_tokens > 0
        assert analysis.total_output_tokens > 0

        # Tool call should have result tokens
        tc = analysis.turns[0].tool_calls[0]
        assert tc.result_tokens > 0

    def test_multiple_turns_accumulate(self):
        """Test that input tokens accumulate across turns."""
        data = {
            "messages": [
                {"role": "user", "content": "Turn 1"},
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Response 1"}],
                },
                {"role": "user", "content": "Turn 2"},
                {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Response 2"}],
                },
            ]
        }

        analysis = parse_transcript(data)
        estimate_transcript_tokens(analysis)

        # Second turn should have more input tokens than first
        # (because context accumulates)
        assert analysis.turns[1].input_tokens > analysis.turns[0].input_tokens


class TestGabbToolEstimates:
    """Tests for gabb tool token estimates."""

    def test_symbol_estimate(self):
        """Test gabb_symbol token estimate."""
        tokens = estimate_gabb_tool_tokens("gabb_symbol")

        assert 50 <= tokens <= 200

    def test_usages_estimate(self):
        """Test gabb_usages token estimate (typically larger)."""
        tokens = estimate_gabb_tool_tokens("gabb_usages")

        assert tokens > 100  # Usages typically return more data

    def test_size_variants(self):
        """Test that size variants affect estimates."""
        small = estimate_gabb_tool_tokens("gabb_symbols", "small")
        typical = estimate_gabb_tool_tokens("gabb_symbols", "typical")
        large = estimate_gabb_tool_tokens("gabb_symbols", "large")

        assert small < typical < large
