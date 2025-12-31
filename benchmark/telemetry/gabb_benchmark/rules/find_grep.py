"""Rule: Find + Grep combo → gabb_symbols with filters.

Detects patterns where find and grep are used together to search for
code patterns in specific file types, which could be replaced with
gabb_symbols using file and name filters.

Pattern detection:
- find . -name "*.ts" followed by grep for pattern
- find with -exec grep
- Glob for file pattern followed by Grep
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from .base import Rule, RuleContext
from ..classifier import is_identifier_pattern
from ..estimator import estimate_gabb_tool_tokens

if TYPE_CHECKING:
    from ..schemas import Opportunity, ToolCall


# Map file extensions to glob patterns
EXTENSION_TO_GLOB = {
    ".ts": "**/*.ts",
    ".tsx": "**/*.tsx",
    ".js": "**/*.js",
    ".jsx": "**/*.jsx",
    ".py": "**/*.py",
    ".rs": "**/*.rs",
    ".kt": "**/*.kt",
    ".cpp": "**/*.cpp",
    ".hpp": "**/*.hpp",
    ".c": "**/*.c",
    ".h": "**/*.h",
    ".go": "**/*.go",
    ".java": "**/*.java",
}


class FindGrepToSymbolsRule(Rule):
    """Detect find+grep patterns that could use gabb_symbols."""

    @property
    def name(self) -> str:
        return "find_grep_to_symbols"

    @property
    def description(self) -> str:
        return "Find+grep for code patterns could use gabb_symbols"

    def check(self, ctx: RuleContext) -> "Opportunity | None":
        """Check for find with -exec grep pattern."""
        from ..schemas import Opportunity, OpportunityType

        tc = ctx.current_tool_call

        # Check for find with -exec grep
        if tc.tool_name == "Bash" and tc.bash_info:
            if tc.bash_info.command_type == "find":
                if "-exec" in tc.bash_info.raw_command and "grep" in tc.bash_info.raw_command:
                    return self._detect_find_exec_grep(ctx)

        return None

    def check_sequence(self, ctx: RuleContext) -> "Opportunity | None":
        """Check for find → grep or Glob → Grep sequences."""
        from ..schemas import Opportunity, OpportunityType

        tc = ctx.current_tool_call

        # Look for grep following a find/glob
        if tc.tool_name == "Grep" or (
            tc.tool_name == "Bash" and tc.bash_info and tc.bash_info.command_type == "grep"
        ):
            return self._check_find_then_grep(ctx)

        return None

    def _detect_find_exec_grep(self, ctx: RuleContext) -> "Opportunity | None":
        """Detect find -exec grep pattern."""
        from ..schemas import Opportunity, OpportunityType

        tc = ctx.current_tool_call
        bash_info = tc.bash_info

        if not bash_info:
            return None

        # Extract file pattern from find command
        file_pattern = bash_info.pattern  # -name pattern

        # Try to extract grep pattern from the command
        # Common pattern: find . -name "*.ts" -exec grep -l "pattern" {} \;
        grep_pattern = self._extract_exec_grep_pattern(bash_info.raw_command)

        if not grep_pattern or not is_identifier_pattern(grep_pattern):
            return None

        # Convert file pattern to gabb glob format
        file_glob = self._convert_to_glob(file_pattern)

        original_tokens = tc.result_tokens
        gabb_tokens = estimate_gabb_tool_tokens("gabb_symbols")
        savings = max(0, original_tokens - gabb_tokens)

        if savings < 50:
            return None

        confidence = 0.70
        if savings > 300:
            confidence = 0.80

        return Opportunity(
            type=OpportunityType.FIND_GREP_TO_SYMBOLS,
            turn_id=ctx.current_turn.turn_id,
            tool_call_index=ctx.current_tool_idx,
            original_command=bash_info.raw_command[:100],
            suggested_tool="gabb_symbols",
            suggested_params={
                "file": file_glob,
                "name_contains": grep_pattern,
            },
            original_tokens=original_tokens,
            estimated_gabb_tokens=gabb_tokens,
            estimated_savings=savings,
            confidence=confidence,
            reason=f"find+grep for '{grep_pattern}' in {file_pattern or 'all files'} - gabb_symbols provides indexed search",
        )

    def _check_find_then_grep(self, ctx: RuleContext) -> "Opportunity | None":
        """Check for find/Glob followed by grep sequence."""
        from ..schemas import Opportunity, OpportunityType

        tc = ctx.current_tool_call
        prev_calls = ctx.get_previous_tool_calls(3)

        # Look for a preceding find or Glob
        find_call: ToolCall | None = None
        for prev in prev_calls:
            if prev.tool_name == "Glob":
                find_call = prev
                break
            if prev.tool_name == "Bash" and prev.bash_info:
                if prev.bash_info.command_type == "find":
                    find_call = prev
                    break

        if find_call is None:
            return None

        # Get grep pattern
        if tc.tool_name == "Grep":
            grep_pattern = tc.tool_input.get("pattern", "")
        elif tc.bash_info:
            grep_pattern = tc.bash_info.pattern or ""
        else:
            return None

        if not grep_pattern or not is_identifier_pattern(grep_pattern):
            return None

        # Get file pattern from find/Glob
        if find_call.tool_name == "Glob":
            file_pattern = find_call.tool_input.get("pattern", "")
        elif find_call.bash_info:
            file_pattern = find_call.bash_info.pattern or ""
        else:
            file_pattern = ""

        # Calculate combined tokens
        total_tokens = find_call.result_tokens + tc.result_tokens
        gabb_tokens = estimate_gabb_tool_tokens("gabb_symbols")
        savings = max(0, total_tokens - gabb_tokens)

        if savings < 100:
            return None

        confidence = 0.75
        if savings > 500:
            confidence = 0.85

        return Opportunity(
            type=OpportunityType.FIND_GREP_TO_SYMBOLS,
            turn_id=ctx.current_turn.turn_id,
            tool_call_index=ctx.current_tool_idx,
            original_command=f"find/Glob → grep '{grep_pattern}'",
            suggested_tool="gabb_symbols",
            suggested_params={
                "file": file_pattern or "**/*",
                "name_contains": grep_pattern,
            },
            original_tokens=total_tokens,
            estimated_gabb_tokens=gabb_tokens,
            estimated_savings=savings,
            confidence=confidence,
            reason=f"find+grep sequence for '{grep_pattern}' - gabb_symbols combines both in one indexed call",
        )

    def _extract_exec_grep_pattern(self, command: str) -> str | None:
        """Extract the grep pattern from a find -exec grep command."""
        import shlex

        try:
            parts = shlex.split(command)
        except ValueError:
            parts = command.split()

        # Look for grep and extract the pattern after it
        in_grep = False
        for i, part in enumerate(parts):
            if part == "grep" or part.endswith("/grep"):
                in_grep = True
                continue
            if in_grep:
                # Skip flags
                if part.startswith("-"):
                    continue
                # This should be the pattern
                if part not in ("{}", "\\;", ";"):
                    return part
        return None

    def _convert_to_glob(self, pattern: str | None) -> str:
        """Convert a find -name pattern to a gabb glob pattern."""
        if not pattern:
            return "**/*"

        # If pattern is like "*.ts", convert to "**/\*.ts"
        if pattern.startswith("*."):
            return f"**/{pattern}"

        return pattern
