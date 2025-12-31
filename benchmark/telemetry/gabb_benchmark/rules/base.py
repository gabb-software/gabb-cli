"""Base classes for the rule engine."""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ..schemas import Opportunity, ToolCall, TranscriptAnalysis, Turn


@dataclass
class RuleContext:
    """Context provided to rules for opportunity detection.

    Provides access to the full analysis context, enabling rules to:
    - Examine individual tool calls
    - Look at sequences of tool calls across turns
    - Access file read history
    """

    analysis: "TranscriptAnalysis"

    # Index of current turn being analyzed
    current_turn_idx: int = 0

    # Index of current tool call within the turn
    current_tool_idx: int = 0

    # Track files that have been read (path -> tokens)
    files_read: dict[str, int] = field(default_factory=dict)

    # Track grep patterns seen (for sequence detection)
    grep_patterns: list[tuple[int, int, str]] = field(
        default_factory=list
    )  # (turn_idx, tool_idx, pattern)

    @property
    def current_turn(self) -> "Turn":
        """Get the current turn."""
        return self.analysis.turns[self.current_turn_idx]

    @property
    def current_tool_call(self) -> "ToolCall":
        """Get the current tool call."""
        return self.current_turn.tool_calls[self.current_tool_idx]

    def get_previous_tool_calls(self, n: int = 5) -> list["ToolCall"]:
        """Get the previous N tool calls across turns.

        Args:
            n: Maximum number of previous calls to return.

        Returns:
            List of previous tool calls, most recent first.
        """
        result: list[ToolCall] = []

        # First, add tool calls from current turn before current index
        for i in range(self.current_tool_idx - 1, -1, -1):
            result.append(self.current_turn.tool_calls[i])
            if len(result) >= n:
                return result

        # Then add tool calls from previous turns
        for turn_idx in range(self.current_turn_idx - 1, -1, -1):
            turn = self.analysis.turns[turn_idx]
            for i in range(len(turn.tool_calls) - 1, -1, -1):
                result.append(turn.tool_calls[i])
                if len(result) >= n:
                    return result

        return result


class Rule(ABC):
    """Base class for opportunity detection rules.

    Each rule examines tool calls and returns any detected opportunities.
    Rules can examine:
    - Single tool calls (most common)
    - Sequences of tool calls (for multi-hop detection)
    """

    @property
    @abstractmethod
    def name(self) -> str:
        """Short name for this rule."""
        ...

    @property
    @abstractmethod
    def description(self) -> str:
        """Human-readable description of what this rule detects."""
        ...

    @abstractmethod
    def check(self, ctx: RuleContext) -> "Opportunity | None":
        """Check if the current tool call represents an opportunity.

        Args:
            ctx: Rule context with access to current and previous tool calls.

        Returns:
            An Opportunity if detected, or None if no match.
        """
        ...

    def check_sequence(self, ctx: RuleContext) -> "Opportunity | None":
        """Check for multi-tool-call patterns.

        Override this to detect patterns spanning multiple tool calls.
        By default, returns None (single-call detection only).

        Args:
            ctx: Rule context with access to tool call history.

        Returns:
            An Opportunity if a sequence pattern is detected, or None.
        """
        return None


class RuleRegistry:
    """Registry for opportunity detection rules.

    Manages a collection of rules and runs them against transcripts.
    """

    def __init__(self) -> None:
        self._rules: list[Rule] = []

    def register(self, rule: Rule) -> None:
        """Register a rule for opportunity detection.

        Args:
            rule: The rule instance to register.
        """
        self._rules.append(rule)

    def get_rules(self) -> list[Rule]:
        """Get all registered rules."""
        return list(self._rules)

    def detect_all(self, analysis: "TranscriptAnalysis") -> list["Opportunity"]:
        """Run all rules against a transcript analysis.

        Args:
            analysis: The transcript analysis to examine.

        Returns:
            List of all detected opportunities, sorted by savings (descending).
        """
        from ..schemas import Opportunity

        opportunities: list[Opportunity] = []
        ctx = RuleContext(analysis=analysis)

        # Track what we've read for context
        for turn_idx, turn in enumerate(analysis.turns):
            ctx.current_turn_idx = turn_idx

            for tool_idx, tc in enumerate(turn.tool_calls):
                ctx.current_tool_idx = tool_idx

                # Update context tracking
                if tc.tool_name == "Read":
                    file_path = tc.tool_input.get("file_path", "")
                    ctx.files_read[file_path] = tc.result_tokens

                if tc.bash_info and tc.bash_info.command_type == "grep":
                    pattern = tc.bash_info.pattern
                    if pattern:
                        ctx.grep_patterns.append((turn_idx, tool_idx, pattern))

                # Run all rules
                for rule in self._rules:
                    # Check single-call patterns
                    opp = rule.check(ctx)
                    if opp is not None:
                        opportunities.append(opp)

                    # Check sequence patterns
                    seq_opp = rule.check_sequence(ctx)
                    if seq_opp is not None:
                        opportunities.append(seq_opp)

        # Sort by estimated savings (descending)
        opportunities.sort(key=lambda x: x.estimated_savings, reverse=True)

        return opportunities
