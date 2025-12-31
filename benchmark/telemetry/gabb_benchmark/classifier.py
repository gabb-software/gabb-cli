"""Tool call classifier - parses and classifies tool calls, especially Bash commands.

Decomposes Bash commands into structured information:
- Command type (grep, find, cat, git, etc.)
- Patterns and arguments
- Target paths
- Recursive flags
"""

from __future__ import annotations

import re
import shlex
from typing import Any

from .schemas import BashCommandInfo, ToolCall, TranscriptAnalysis


# Regex patterns for common command types
GREP_PATTERN = re.compile(r"^(?:grep|rg|ag|ack)\b")
FIND_PATTERN = re.compile(r"^find\b")
CAT_PATTERN = re.compile(r"^(?:cat|head|tail|less|more)\b")
GIT_PATTERN = re.compile(r"^git\b")
LS_PATTERN = re.compile(r"^ls\b")
ECHO_PATTERN = re.compile(r"^echo\b")
SED_AWK_PATTERN = re.compile(r"^(?:sed|awk)\b")
CURL_WGET_PATTERN = re.compile(r"^(?:curl|wget)\b")
NPM_YARN_PATTERN = re.compile(r"^(?:npm|yarn|pnpm|bun)\b")
CARGO_PATTERN = re.compile(r"^cargo\b")
PYTHON_PATTERN = re.compile(r"^(?:python3?|pip3?)\b")

# Recursive flags for grep
RECURSIVE_FLAGS = {"-r", "-R", "--recursive", "-rn", "-Rn", "-nr", "-nR"}


def parse_bash_command(command: str) -> BashCommandInfo:
    """Parse a Bash command into structured information.

    Args:
        command: The raw command string.

    Returns:
        BashCommandInfo with parsed details.
    """
    # Handle empty or whitespace-only commands
    command = command.strip()
    if not command:
        return BashCommandInfo(raw_command=command, command_type="empty")

    # Try to parse using shlex for proper quoting handling
    try:
        parts = shlex.split(command)
    except ValueError:
        # Fallback to simple split if shlex fails (e.g., unmatched quotes)
        parts = command.split()

    if not parts:
        return BashCommandInfo(raw_command=command, command_type="empty")

    base_cmd = parts[0]

    # Determine command type
    if GREP_PATTERN.match(base_cmd):
        return _parse_grep_command(command, parts)
    elif FIND_PATTERN.match(base_cmd):
        return _parse_find_command(command, parts)
    elif CAT_PATTERN.match(base_cmd):
        return _parse_cat_command(command, parts)
    elif GIT_PATTERN.match(base_cmd):
        return _parse_git_command(command, parts)
    elif LS_PATTERN.match(base_cmd):
        return BashCommandInfo(
            raw_command=command,
            command_type="ls",
            target_path=_extract_path_arg(parts[1:]),
        )
    elif ECHO_PATTERN.match(base_cmd):
        return BashCommandInfo(raw_command=command, command_type="echo")
    elif SED_AWK_PATTERN.match(base_cmd):
        return BashCommandInfo(raw_command=command, command_type=base_cmd)
    elif CURL_WGET_PATTERN.match(base_cmd):
        return BashCommandInfo(raw_command=command, command_type=base_cmd)
    elif NPM_YARN_PATTERN.match(base_cmd):
        return BashCommandInfo(raw_command=command, command_type="npm")
    elif CARGO_PATTERN.match(base_cmd):
        return BashCommandInfo(raw_command=command, command_type="cargo")
    elif PYTHON_PATTERN.match(base_cmd):
        return BashCommandInfo(raw_command=command, command_type="python")
    else:
        return BashCommandInfo(raw_command=command, command_type=base_cmd)


def _parse_grep_command(command: str, parts: list[str]) -> BashCommandInfo:
    """Parse a grep/rg command."""
    flags = []
    pattern = None
    target_path = None
    is_recursive = False

    # Parse arguments
    i = 1
    while i < len(parts):
        arg = parts[i]

        if arg.startswith("-"):
            flags.append(arg)
            # Check for recursive flag
            if arg in RECURSIVE_FLAGS or "-r" in arg or "-R" in arg:
                is_recursive = True
            # Handle -e pattern (only capture first)
            if arg == "-e" and i + 1 < len(parts):
                i += 1
                if pattern is None:
                    pattern = parts[i]
        elif pattern is None:
            # First non-flag arg is the pattern
            pattern = arg
        else:
            # Subsequent non-flag args are paths
            target_path = arg

        i += 1

    return BashCommandInfo(
        raw_command=command,
        command_type="grep",
        pattern=pattern,
        target_path=target_path,
        is_recursive=is_recursive,
        flags=flags,
    )


def _parse_find_command(command: str, parts: list[str]) -> BashCommandInfo:
    """Parse a find command."""
    flags = []
    pattern = None
    target_path = None

    i = 1
    while i < len(parts):
        arg = parts[i]

        if arg == "-name" or arg == "-iname":
            if i + 1 < len(parts):
                pattern = parts[i + 1]
                i += 1
        elif arg.startswith("-"):
            flags.append(arg)
        elif target_path is None:
            target_path = arg

        i += 1

    return BashCommandInfo(
        raw_command=command,
        command_type="find",
        pattern=pattern,
        target_path=target_path or ".",
        is_recursive=True,  # find is inherently recursive
        flags=flags,
    )


def _parse_cat_command(command: str, parts: list[str]) -> BashCommandInfo:
    """Parse a cat/head/tail command."""
    cmd_type = parts[0]
    flags = []
    target_path = None

    # Flags that take a value argument
    value_flags = {"-n", "-c", "--lines", "--bytes"}

    i = 1
    while i < len(parts):
        arg = parts[i]
        if arg.startswith("-"):
            flags.append(arg)
            # Skip the value if this flag takes one
            if arg in value_flags and i + 1 < len(parts):
                i += 1
                flags.append(parts[i])
        elif target_path is None:
            target_path = arg
        i += 1

    return BashCommandInfo(
        raw_command=command,
        command_type=cmd_type,
        target_path=target_path,
        flags=flags,
    )


def _parse_git_command(command: str, parts: list[str]) -> BashCommandInfo:
    """Parse a git command."""
    subcommand = parts[1] if len(parts) > 1 else ""

    return BashCommandInfo(
        raw_command=command,
        command_type=f"git-{subcommand}" if subcommand else "git",
        flags=parts[2:] if len(parts) > 2 else [],
    )


def _extract_path_arg(args: list[str]) -> str | None:
    """Extract the first non-flag argument as a path."""
    for arg in args:
        if not arg.startswith("-"):
            return arg
    return None


def classify_tool_calls(analysis: TranscriptAnalysis) -> None:
    """Classify all tool calls in an analysis, updating in place.

    This parses Bash commands and adds BashCommandInfo to each tool call.

    Args:
        analysis: TranscriptAnalysis to process (modified in place).
    """
    for turn in analysis.turns:
        for tc in turn.tool_calls:
            if tc.tool_name == "Bash":
                command = tc.tool_input.get("command", "")
                tc.bash_info = parse_bash_command(command)


def is_identifier_pattern(pattern: str) -> bool:
    """Check if a pattern looks like a code identifier.

    Identifiers are typically PascalCase, camelCase, snake_case, or SCREAMING_SNAKE_CASE.

    Args:
        pattern: The search pattern to check.

    Returns:
        True if the pattern looks like an identifier.
    """
    if not pattern:
        return False

    # Strip common regex anchors/escapes
    pattern = pattern.strip("^$.*?+[](){}|\\")

    # Check for common identifier patterns
    # PascalCase: starts with uppercase, has lowercase
    if re.match(r"^[A-Z][a-zA-Z0-9]*$", pattern):
        return True

    # camelCase: starts with lowercase, has uppercase
    if re.match(r"^[a-z][a-zA-Z0-9]*$", pattern) and any(c.isupper() for c in pattern):
        return True

    # snake_case: lowercase with underscores
    if re.match(r"^[a-z][a-z0-9_]*$", pattern) and "_" in pattern:
        return True

    # SCREAMING_SNAKE_CASE: uppercase with underscores
    if re.match(r"^[A-Z][A-Z0-9_]*$", pattern) and "_" in pattern:
        return True

    # Simple word (no spaces, reasonable length for identifier)
    if re.match(r"^[a-zA-Z_][a-zA-Z0-9_]*$", pattern) and 2 < len(pattern) < 50:
        return True

    return False


def get_tool_summary(analysis: TranscriptAnalysis) -> dict[str, Any]:
    """Get a summary of tool usage from an analysis.

    Args:
        analysis: The analyzed transcript.

    Returns:
        Dictionary with tool usage statistics.
    """
    tool_counts: dict[str, int] = {}
    bash_cmd_counts: dict[str, int] = {}
    total_result_tokens = 0

    for turn in analysis.turns:
        for tc in turn.tool_calls:
            tool_counts[tc.tool_name] = tool_counts.get(tc.tool_name, 0) + 1
            total_result_tokens += tc.result_tokens

            if tc.bash_info:
                cmd_type = tc.bash_info.command_type
                bash_cmd_counts[cmd_type] = bash_cmd_counts.get(cmd_type, 0) + 1

    return {
        "tool_counts": tool_counts,
        "bash_breakdown": bash_cmd_counts,
        "total_tool_calls": sum(tool_counts.values()),
        "total_result_tokens": total_result_tokens,
    }
