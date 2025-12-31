"""Report generation for transcript analysis.

Generates formatted reports showing tool usage breakdown, token counts,
and (in Phase 2) gabb optimization opportunities.
"""

from __future__ import annotations

import json
from typing import Any

from .schemas import TranscriptAnalysis

# Try to import rich for pretty output
try:
    from rich.console import Console
    from rich.table import Table
    from rich.panel import Panel

    HAS_RICH = True
except ImportError:
    HAS_RICH = False


def format_number(n: int) -> str:
    """Format a number with thousands separators."""
    return f"{n:,}"


def generate_json_report(analysis: TranscriptAnalysis) -> str:
    """Generate a JSON report from the analysis.

    Args:
        analysis: The analyzed transcript.

    Returns:
        JSON string with the full report.
    """
    return json.dumps(analysis.to_dict(), indent=2)


def generate_text_report(analysis: TranscriptAnalysis) -> str:
    """Generate a plain text report from the analysis.

    Args:
        analysis: The analyzed transcript.

    Returns:
        Formatted text report.
    """
    lines = []
    data = analysis.to_dict()
    summary = data["summary"]

    # Header
    lines.append("=" * 70)
    lines.append("                    Gabb Benchmark Report")
    lines.append("=" * 70)
    lines.append("")

    # Task info
    if analysis.task_description:
        desc = analysis.task_description[:80]
        if len(analysis.task_description) > 80:
            desc += "..."
        lines.append(f"Task: {desc}")
        lines.append("")

    # Token summary
    lines.append("TOKEN SUMMARY")
    lines.append("-" * 70)
    lines.append(f"  Total turns:          {summary['total_turns']}")
    lines.append(f"  Total tool calls:     {summary['tool_call_count']}")
    lines.append(f"  Input tokens:         {format_number(summary['total_input_tokens'])}")
    lines.append(f"  Output tokens:        {format_number(summary['total_output_tokens'])}")

    total_tokens = summary["total_input_tokens"] + summary["total_output_tokens"]
    file_pct = (
        (summary["file_content_tokens"] / total_tokens * 100)
        if total_tokens > 0
        else 0
    )
    lines.append(
        f"  File content tokens:  {format_number(summary['file_content_tokens'])} ({file_pct:.0f}%)"
    )
    lines.append("")

    # Tool distribution
    lines.append("TOOL DISTRIBUTION")
    lines.append("-" * 70)
    tool_dist = data.get("tool_distribution", {})
    for tool, stats in sorted(tool_dist.items()):
        count = stats["count"]
        tokens = stats["tokens"]
        lines.append(f"  {tool:<30} {count:>5} calls  {format_number(tokens):>10} tokens")

    # Bash breakdown if present
    bash_breakdown = data.get("bash_breakdown", {})
    if bash_breakdown:
        lines.append("")
        lines.append("  Bash command breakdown:")
        for cmd, count in sorted(bash_breakdown.items(), key=lambda x: -x[1]):
            lines.append(f"    {cmd:<26} {count:>5}")

    lines.append("")

    # Per-turn summary (compact)
    if analysis.turns:
        lines.append("TURN BREAKDOWN")
        lines.append("-" * 70)
        lines.append(f"  {'Turn':<6} {'Tools':<8} {'Input':>12} {'Output':>12}")
        for turn in analysis.turns:
            turn_data = turn.to_dict()
            lines.append(
                f"  {turn_data['turn_id']:<6} {len(turn.tool_calls):<8} "
                f"{format_number(turn_data['input_tokens']):>12} "
                f"{format_number(turn_data['output_tokens']):>12}"
            )

    lines.append("")
    lines.append("=" * 70)

    return "\n".join(lines)


def print_rich_report(analysis: TranscriptAnalysis) -> None:
    """Print a rich-formatted report to the console.

    Args:
        analysis: The analyzed transcript.
    """
    if not HAS_RICH:
        print(generate_text_report(analysis))
        return

    console = Console()
    data = analysis.to_dict()
    summary = data["summary"]

    # Header
    console.print()
    console.print(
        Panel.fit(
            "[bold]Gabb Benchmark Report[/bold]",
            border_style="blue",
        )
    )
    console.print()

    # Task info
    if analysis.task_description:
        desc = analysis.task_description[:100]
        if len(analysis.task_description) > 100:
            desc += "..."
        console.print(f"[dim]Task:[/dim] {desc}")
        console.print()

    # Token summary table
    token_table = Table(title="Token Summary", show_header=False, box=None)
    token_table.add_column("Metric", style="cyan")
    token_table.add_column("Value", justify="right")

    token_table.add_row("Total turns", str(summary["total_turns"]))
    token_table.add_row("Total tool calls", str(summary["tool_call_count"]))
    token_table.add_row("Input tokens", format_number(summary["total_input_tokens"]))
    token_table.add_row("Output tokens", format_number(summary["total_output_tokens"]))

    total_tokens = summary["total_input_tokens"] + summary["total_output_tokens"]
    file_pct = (
        (summary["file_content_tokens"] / total_tokens * 100)
        if total_tokens > 0
        else 0
    )
    token_table.add_row(
        "File content tokens",
        f"{format_number(summary['file_content_tokens'])} ({file_pct:.0f}%)",
    )

    console.print(token_table)
    console.print()

    # Tool distribution
    tool_table = Table(title="Tool Distribution")
    tool_table.add_column("Tool", style="cyan")
    tool_table.add_column("Calls", justify="right")
    tool_table.add_column("Tokens", justify="right")

    tool_dist = data.get("tool_distribution", {})
    for tool, stats in sorted(tool_dist.items(), key=lambda x: -x[1]["count"]):
        tool_table.add_row(
            tool,
            str(stats["count"]),
            format_number(stats["tokens"]),
        )

    console.print(tool_table)

    # Bash breakdown if present
    bash_breakdown = data.get("bash_breakdown", {})
    if bash_breakdown:
        console.print()
        bash_table = Table(title="Bash Command Breakdown")
        bash_table.add_column("Command", style="cyan")
        bash_table.add_column("Count", justify="right")

        for cmd, count in sorted(bash_breakdown.items(), key=lambda x: -x[1]):
            bash_table.add_row(cmd, str(count))

        console.print(bash_table)

    console.print()
