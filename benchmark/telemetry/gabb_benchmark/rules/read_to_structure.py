"""Rule: Full file read → gabb_structure first.

Detects Read tool calls on large files without offset/limit that could
benefit from using gabb_structure first to understand file layout.

Pattern detection:
- Read entire file (no offset/limit) on file > 100 lines / > 500 tokens
- Suggests: gabb_structure first, then targeted Read with offset/limit
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from .base import Rule, RuleContext
from ..estimator import estimate_gabb_tool_tokens

if TYPE_CHECKING:
    from ..schemas import Opportunity


# Supported code file extensions for gabb_structure
CODE_EXTENSIONS = {
    ".ts",
    ".tsx",
    ".js",
    ".jsx",
    ".py",
    ".pyi",
    ".rs",
    ".kt",
    ".kts",
    ".cpp",
    ".cc",
    ".cxx",
    ".c++",
    ".hpp",
    ".hh",
    ".hxx",
    ".h++",
    ".c",
    ".h",
    ".go",
    ".java",
    ".scala",
}

# Minimum tokens to consider a file "large"
MIN_TOKENS_FOR_STRUCTURE = 500


class ReadToStructureRule(Rule):
    """Detect full file reads that could benefit from gabb_structure first."""

    @property
    def name(self) -> str:
        return "read_to_structure"

    @property
    def description(self) -> str:
        return "Full file read could use gabb_structure first for overview"

    def check(self, ctx: RuleContext) -> "Opportunity | None":
        """Check if a Read call could benefit from gabb_structure."""
        from ..schemas import Opportunity, OpportunityType

        tc = ctx.current_tool_call

        if tc.tool_name != "Read":
            return None

        file_path = tc.tool_input.get("file_path", "")
        has_offset = tc.tool_input.get("offset") is not None
        has_limit = tc.tool_input.get("limit") is not None

        # Skip if already using targeted reading
        if has_offset or has_limit:
            return None

        # Check if it's a code file
        if not self._is_code_file(file_path):
            return None

        # Check if the file is large enough to warrant structure
        if tc.result_tokens < MIN_TOKENS_FOR_STRUCTURE:
            return None

        # This is a large file read that could benefit from structure first
        original_tokens = tc.result_tokens
        structure_tokens = estimate_gabb_tool_tokens("gabb_structure", "typical")

        # Estimate that structure + targeted read would use ~30% of full read
        # This is conservative - in practice it could be much less
        estimated_targeted_read = int(original_tokens * 0.3)
        gabb_approach_tokens = structure_tokens + estimated_targeted_read

        savings = max(0, original_tokens - gabb_approach_tokens)

        # Higher confidence for larger files
        confidence = 0.70
        if original_tokens > 1000:
            confidence = 0.80
        if original_tokens > 2000:
            confidence = 0.90

        return Opportunity(
            type=OpportunityType.READ_TO_STRUCTURE,
            turn_id=ctx.current_turn.turn_id,
            tool_call_index=ctx.current_tool_idx,
            original_command=f"Read full file '{file_path}'",
            suggested_tool="gabb_structure",
            suggested_params={"file": file_path},
            original_tokens=original_tokens,
            estimated_gabb_tokens=gabb_approach_tokens,
            estimated_savings=savings,
            confidence=confidence,
            reason=f"Read full file ({original_tokens} tokens) - gabb_structure shows layout without token cost",
        )

    def _is_code_file(self, file_path: str) -> bool:
        """Check if the file path has a supported code extension."""
        lower_path = file_path.lower()
        return any(lower_path.endswith(ext) for ext in CODE_EXTENSIONS)
