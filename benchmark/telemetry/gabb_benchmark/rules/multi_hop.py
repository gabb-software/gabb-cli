"""Rule: Multi-hop navigation → gabb_definition.

Detects patterns where multiple tool calls are used to navigate from
a symbol usage to its definition, which could be a single gabb_definition call.

Pattern detection:
- Grep for name → Read file → Reference specific line (3-hop pattern)
- Glob for files → Read file → Extract symbol location (3-hop pattern)
- Read file → Grep within context (2-hop pattern)
"""

from __future__ import annotations

import re
from typing import TYPE_CHECKING

from .base import Rule, RuleContext
from ..classifier import is_identifier_pattern
from ..estimator import estimate_gabb_tool_tokens

if TYPE_CHECKING:
    from ..schemas import Opportunity, ToolCall


# Patterns that indicate finding file:line references in grep output
FILE_LINE_PATTERN = re.compile(r"[^:]+:\d+:")


class MultiHopToDefinitionRule(Rule):
    """Detect multi-hop navigation patterns that could use gabb_definition."""

    @property
    def name(self) -> str:
        return "multi_hop_to_definition"

    @property
    def description(self) -> str:
        return "Multi-hop navigation to definition could use gabb_definition"

    def check(self, ctx: RuleContext) -> "Opportunity | None":
        """Check single tool call - not applicable for this rule."""
        # This rule uses sequence detection
        return None

    def check_sequence(self, ctx: RuleContext) -> "Opportunity | None":
        """Check for multi-hop navigation patterns."""
        from ..schemas import Opportunity, OpportunityType

        tc = ctx.current_tool_call

        # We look for Read calls that follow a grep/search
        # This indicates a "search → read" pattern
        if tc.tool_name != "Read":
            return None

        # Get previous tool calls
        prev_calls = ctx.get_previous_tool_calls(5)
        if not prev_calls:
            return None

        # Look for grep → read pattern
        grep_call = self._find_preceding_grep(prev_calls, ctx)
        if grep_call is None:
            return None

        # Check if the grep was for an identifier pattern
        grep_pattern = self._get_grep_pattern(grep_call)
        if not grep_pattern or not is_identifier_pattern(grep_pattern):
            return None

        # Check if this Read is targeting a file that was found in grep results
        file_path = tc.tool_input.get("file_path", "")
        if not self._file_in_grep_results(file_path, grep_call):
            return None

        # Calculate token savings for the sequence
        total_sequence_tokens = grep_call.result_tokens + tc.result_tokens
        gabb_tokens = estimate_gabb_tool_tokens("gabb_definition")
        savings = max(0, total_sequence_tokens - gabb_tokens)

        # Only report if savings are meaningful
        if savings < 100:
            return None

        confidence = 0.75
        if savings > 500:
            confidence = 0.85

        return Opportunity(
            type=OpportunityType.MULTI_HOP_TO_DEFINITION,
            turn_id=ctx.current_turn.turn_id,
            tool_call_index=ctx.current_tool_idx,
            original_command=f"grep '{grep_pattern}' → Read '{file_path}'",
            suggested_tool="gabb_definition",
            suggested_params={
                "file": file_path,
                "symbol_name": grep_pattern,
            },
            original_tokens=total_sequence_tokens,
            estimated_gabb_tokens=gabb_tokens,
            estimated_savings=savings,
            confidence=confidence,
            reason=f"Multi-hop navigation (grep→read) for '{grep_pattern}' - gabb_definition is single call",
        )

    def _find_preceding_grep(
        self, prev_calls: list["ToolCall"], ctx: RuleContext
    ) -> "ToolCall | None":
        """Find a preceding grep call in the sequence."""
        for tc in prev_calls:
            # Check Bash grep
            if tc.tool_name == "Bash" and tc.bash_info:
                if tc.bash_info.command_type == "grep":
                    return tc
            # Check Grep tool
            if tc.tool_name == "Grep":
                return tc
        return None

    def _get_grep_pattern(self, tc: "ToolCall") -> str | None:
        """Extract the grep pattern from a tool call."""
        if tc.tool_name == "Bash" and tc.bash_info:
            return tc.bash_info.pattern
        if tc.tool_name == "Grep":
            return tc.tool_input.get("pattern")
        return None

    def _file_in_grep_results(self, file_path: str, grep_call: "ToolCall") -> bool:
        """Check if a file path was mentioned in grep results."""
        if not grep_call.result_content:
            return False

        # Check if any part of the file path appears in results
        # Grep results typically show: file:line:content
        path_parts = file_path.replace("\\", "/").split("/")

        # Check for exact filename match
        filename = path_parts[-1] if path_parts else ""
        if filename and filename in grep_call.result_content:
            return True

        # Check for partial path match
        for i in range(len(path_parts)):
            partial = "/".join(path_parts[i:])
            if partial and partial in grep_call.result_content:
                return True

        return False
