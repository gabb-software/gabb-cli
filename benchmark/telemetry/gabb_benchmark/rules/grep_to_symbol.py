"""Rule: Grep for symbol names → gabb_symbol or gabb_usages.

Detects grep commands that search for identifier-like patterns
(function names, class names, etc.) and suggests using gabb tools instead.

Pattern detection:
- grep -r "SymbolName" → gabb_usages (finding all references)
- grep -rn "functionName" → gabb_usages (finding all references)
- grep "^function " → gabb_symbols kind=function (finding definitions)
- Grep tool with identifier pattern → gabb_symbol/gabb_usages
"""

from __future__ import annotations

import re
from typing import TYPE_CHECKING

from .base import Rule, RuleContext
from ..classifier import is_identifier_pattern
from ..estimator import estimate_gabb_tool_tokens

if TYPE_CHECKING:
    from ..schemas import Opportunity


# Patterns that suggest looking for definitions
DEFINITION_PATTERNS = [
    re.compile(r"^\^?(?:function|def|class|interface|type|struct|enum|trait)\s"),
    re.compile(r"^\^?export\s+(?:function|class|interface|type)"),
    re.compile(r"^\^?pub\s+(?:fn|struct|enum|trait)"),
    re.compile(r"^\^?(?:const|let|var)\s+\w+\s*="),
]


class GrepToSymbolRule(Rule):
    """Detect grep commands that could be replaced with gabb_symbol/gabb_usages."""

    @property
    def name(self) -> str:
        return "grep_to_symbol"

    @property
    def description(self) -> str:
        return "Grep for symbol names could use gabb_symbol or gabb_usages"

    def check(self, ctx: RuleContext) -> "Opportunity | None":
        """Check if a grep command could be replaced with gabb tools."""
        from ..schemas import Opportunity, OpportunityType

        tc = ctx.current_tool_call

        # Check for Bash grep commands
        if tc.tool_name == "Bash" and tc.bash_info:
            if tc.bash_info.command_type == "grep":
                return self._check_bash_grep(ctx)

        # Check for Claude's Grep tool
        if tc.tool_name == "Grep":
            return self._check_grep_tool(ctx)

        return None

    def _check_bash_grep(self, ctx: RuleContext) -> "Opportunity | None":
        """Check bash grep command."""
        from ..schemas import Opportunity, OpportunityType

        tc = ctx.current_tool_call
        bash_info = tc.bash_info

        if not bash_info or not bash_info.pattern:
            return None

        pattern = bash_info.pattern

        # Check if pattern looks like an identifier
        if not is_identifier_pattern(pattern):
            return None

        # Determine if this looks like a definition search or usage search
        is_definition_search = any(
            p.match(pattern) for p in DEFINITION_PATTERNS
        )

        # Recursive grep is more likely looking for all usages
        is_usage_search = bash_info.is_recursive

        # Calculate confidence and suggested tool
        if is_definition_search:
            suggested_tool = "gabb_symbols"
            opportunity_type = OpportunityType.GREP_TO_SYMBOLS
            confidence = 0.75
            reason = f"Grep for definition pattern '{pattern}' - gabb_symbols provides indexed lookup"
            suggested_params = {"name_contains": pattern.strip("^$")}
        elif is_usage_search:
            suggested_tool = "gabb_usages"
            opportunity_type = OpportunityType.GREP_TO_USAGES
            confidence = 0.85
            reason = f"Recursive grep for symbol '{pattern}' - gabb_usages provides semantic reference search"
            suggested_params = {"name": pattern}
        else:
            suggested_tool = "gabb_symbol"
            opportunity_type = OpportunityType.GREP_TO_SYMBOL
            confidence = 0.70
            reason = f"Grep for identifier '{pattern}' - gabb_symbol provides precise definition lookup"
            suggested_params = {"name": pattern}

        # Estimate token savings
        original_tokens = tc.result_tokens
        gabb_tokens = estimate_gabb_tool_tokens(suggested_tool)
        savings = max(0, original_tokens - gabb_tokens)

        # Higher confidence if significant savings
        if savings > 500:
            confidence = min(1.0, confidence + 0.1)

        return Opportunity(
            type=opportunity_type,
            turn_id=ctx.current_turn.turn_id,
            tool_call_index=ctx.current_tool_idx,
            original_command=bash_info.raw_command,
            suggested_tool=suggested_tool,
            suggested_params=suggested_params,
            original_tokens=original_tokens,
            estimated_gabb_tokens=gabb_tokens,
            estimated_savings=savings,
            confidence=confidence,
            reason=reason,
        )

    def _check_grep_tool(self, ctx: RuleContext) -> "Opportunity | None":
        """Check Claude's Grep tool."""
        from ..schemas import Opportunity, OpportunityType

        tc = ctx.current_tool_call
        pattern = tc.tool_input.get("pattern", "")

        if not pattern or not is_identifier_pattern(pattern):
            return None

        # Grep tool is typically used for usage search
        suggested_tool = "gabb_usages"
        opportunity_type = OpportunityType.GREP_TO_USAGES
        confidence = 0.80
        reason = f"Grep for identifier '{pattern}' - gabb_usages provides semantic accuracy"

        original_tokens = tc.result_tokens
        gabb_tokens = estimate_gabb_tool_tokens(suggested_tool)
        savings = max(0, original_tokens - gabb_tokens)

        if savings > 500:
            confidence = min(1.0, confidence + 0.1)

        return Opportunity(
            type=opportunity_type,
            turn_id=ctx.current_turn.turn_id,
            tool_call_index=ctx.current_tool_idx,
            original_command=f"Grep pattern='{pattern}'",
            suggested_tool=suggested_tool,
            suggested_params={"name": pattern},
            original_tokens=original_tokens,
            estimated_gabb_tokens=gabb_tokens,
            estimated_savings=savings,
            confidence=confidence,
            reason=reason,
        )
