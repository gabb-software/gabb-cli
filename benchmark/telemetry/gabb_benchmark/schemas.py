"""Data models for telemetry analysis."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any


class OpportunityType(Enum):
    """Types of gabb optimization opportunities."""

    # grep for symbol name → gabb_symbol or gabb_usages
    GREP_TO_SYMBOL = "grep_to_symbol"
    GREP_TO_USAGES = "grep_to_usages"

    # Full file read → gabb_structure first
    READ_TO_STRUCTURE = "read_to_structure"

    # Multi-hop navigation → gabb_definition
    MULTI_HOP_TO_DEFINITION = "multi_hop_to_definition"

    # find + grep combo → gabb_symbols with filters
    FIND_GREP_TO_SYMBOLS = "find_grep_to_symbols"

    # Generic grep → gabb_symbols with name filter
    GREP_TO_SYMBOLS = "grep_to_symbols"


@dataclass
class Opportunity:
    """A detected gabb optimization opportunity."""

    type: OpportunityType
    turn_id: int
    tool_call_index: int  # Index within the turn's tool_calls

    # What was originally done
    original_command: str

    # What gabb tool could replace it
    suggested_tool: str
    suggested_params: dict[str, Any] = field(default_factory=dict)

    # Token economics
    original_tokens: int = 0
    estimated_gabb_tokens: int = 0
    estimated_savings: int = 0

    # Confidence in this detection (0.0 - 1.0)
    confidence: float = 0.0

    # Human-readable explanation
    reason: str = ""

    def to_dict(self) -> dict[str, Any]:
        return {
            "type": self.type.value,
            "turn_id": self.turn_id,
            "tool_call_index": self.tool_call_index,
            "original_command": self.original_command,
            "suggested_tool": self.suggested_tool,
            "suggested_params": self.suggested_params,
            "original_tokens": self.original_tokens,
            "estimated_gabb_tokens": self.estimated_gabb_tokens,
            "estimated_savings": self.estimated_savings,
            "confidence": self.confidence,
            "reason": self.reason,
        }


@dataclass
class BashCommandInfo:
    """Parsed information about a Bash command."""

    raw_command: str
    command_type: str  # grep, find, cat, git, etc.
    pattern: str | None = None  # For grep/find patterns
    target_path: str | None = None  # File/directory being operated on
    is_recursive: bool = False
    flags: list[str] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "raw_command": self.raw_command,
            "command_type": self.command_type,
            "pattern": self.pattern,
            "target_path": self.target_path,
            "is_recursive": self.is_recursive,
            "flags": self.flags,
        }


@dataclass
class ToolCall:
    """A single tool call extracted from a transcript."""

    tool_name: str
    tool_input: dict[str, Any]
    tool_use_id: str
    result_content: str | None = None
    result_tokens: int = 0

    # For Bash commands, parsed info
    bash_info: BashCommandInfo | None = None

    # Token estimates
    input_tokens: int = 0
    output_tokens: int = 0

    def to_dict(self) -> dict[str, Any]:
        result = {
            "tool_name": self.tool_name,
            "tool_input": self.tool_input,
            "tool_use_id": self.tool_use_id,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "result_tokens": self.result_tokens,
        }
        if self.bash_info:
            result["bash_info"] = self.bash_info.to_dict()
        return result


@dataclass
class Turn:
    """A conversation turn (assistant response + tool results)."""

    turn_id: int
    tool_calls: list[ToolCall] = field(default_factory=list)
    assistant_text: str = ""

    # Token counts
    input_tokens: int = 0  # Cumulative context at start of turn
    output_tokens: int = 0  # Tokens generated this turn

    def to_dict(self) -> dict[str, Any]:
        return {
            "turn_id": self.turn_id,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "tool_calls": [tc.to_dict() for tc in self.tool_calls],
            "assistant_text_length": len(self.assistant_text),
        }


@dataclass
class TranscriptAnalysis:
    """Complete analysis of a transcript."""

    session_id: str | None = None
    task_description: str = ""
    turns: list[Turn] = field(default_factory=list)

    # Summary metrics
    total_input_tokens: int = 0
    total_output_tokens: int = 0
    file_content_tokens: int = 0  # Tokens from Read/tool results

    # Phase 2: Detected opportunities
    opportunities: list[Opportunity] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        # Compute tool distribution
        tool_dist: dict[str, dict[str, int]] = {}
        bash_breakdown: dict[str, int] = {}

        for turn in self.turns:
            for tc in turn.tool_calls:
                if tc.tool_name not in tool_dist:
                    tool_dist[tc.tool_name] = {"count": 0, "tokens": 0}
                tool_dist[tc.tool_name]["count"] += 1
                tool_dist[tc.tool_name]["tokens"] += tc.result_tokens

                # Track bash command types
                if tc.tool_name == "Bash" and tc.bash_info:
                    cmd_type = tc.bash_info.command_type
                    bash_breakdown[cmd_type] = bash_breakdown.get(cmd_type, 0) + 1

        # Compute opportunity summary
        total_savings = sum(opp.estimated_savings for opp in self.opportunities)
        total_tokens = self.total_input_tokens + self.total_output_tokens
        savings_percentage = (total_savings / total_tokens * 100) if total_tokens > 0 else 0

        return {
            "session_id": self.session_id,
            "task_description": self.task_description,
            "summary": {
                "total_turns": len(self.turns),
                "total_input_tokens": self.total_input_tokens,
                "total_output_tokens": self.total_output_tokens,
                "file_content_tokens": self.file_content_tokens,
                "tool_call_count": sum(len(t.tool_calls) for t in self.turns),
                "gabb_opportunity_count": len(self.opportunities),
                "potential_token_savings": total_savings,
                "savings_percentage": round(savings_percentage, 1),
            },
            "turns": [t.to_dict() for t in self.turns],
            "tool_distribution": tool_dist,
            "bash_breakdown": bash_breakdown,
            "opportunities": [opp.to_dict() for opp in self.opportunities],
        }
