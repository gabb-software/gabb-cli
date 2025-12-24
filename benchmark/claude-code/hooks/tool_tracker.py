#!/usr/bin/env python3
"""
PostToolUse hook that logs every tool call to a JSONL file.

Receives structured input from Claude Code and appends to a log file.
The log file path is passed via environment variable BENCHMARK_TOOL_LOG.

Hook Input Schema (from Claude Code):
{
    "session_id": "abc123",
    "tool_name": "Grep",
    "tool_input": {...},
    "tool_use_id": "toolu_xxx",
    "transcript_path": "/path/to/conversation.jsonl"
}
"""

import json
import os
import sys
from datetime import datetime
from pathlib import Path


def main():
    # Read hook input from stdin
    try:
        hook_input = json.load(sys.stdin)
    except json.JSONDecodeError:
        # Silent fail - don't block Claude Code
        sys.exit(0)

    # Get log file from env (set by benchmark runner)
    log_file = os.environ.get("BENCHMARK_TOOL_LOG")
    if not log_file:
        # Fallback to temp location
        log_file = Path("/tmp/claude_code_benchmark_tools.jsonl")
    else:
        log_file = Path(log_file)

    # Ensure parent directory exists
    log_file.parent.mkdir(parents=True, exist_ok=True)

    # Extract relevant fields
    record = {
        "timestamp": datetime.now().isoformat(),
        "session_id": hook_input.get("session_id"),
        "tool_name": hook_input.get("tool_name"),
        "tool_use_id": hook_input.get("tool_use_id"),
    }

    # Append to log
    try:
        with open(log_file, "a") as f:
            f.write(json.dumps(record) + "\n")
    except Exception:
        # Silent fail - don't block Claude Code
        pass

    # Exit 0 to allow tool to proceed
    sys.exit(0)


if __name__ == "__main__":
    main()
