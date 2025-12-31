"""Report generation for transcript analysis.

Generates formatted reports showing tool usage breakdown, token counts,
and gabb optimization opportunities (Phase 2).
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

    # Gabb opportunities (Phase 2)
    if analysis.opportunities:
        lines.append("GABB OPPORTUNITIES DETECTED")
        lines.append("-" * 70)

        # Group by impact
        high_impact = [o for o in analysis.opportunities if o.estimated_savings >= 1000]
        medium_impact = [o for o in analysis.opportunities if 500 <= o.estimated_savings < 1000]
        low_impact = [o for o in analysis.opportunities if o.estimated_savings < 500]

        total_savings = sum(o.estimated_savings for o in analysis.opportunities)
        lines.append(f"  Total opportunities: {len(analysis.opportunities)}")
        lines.append(f"  Potential savings:   {format_number(total_savings)} tokens")
        lines.append("")

        if high_impact:
            lines.append("  HIGH IMPACT (>1,000 tokens each):")
            for opp in high_impact[:5]:
                lines.append(f"    Turn {opp.turn_id}: {opp.original_command[:60]}")
                lines.append(f"      -> {opp.suggested_tool}")
                lines.append(f"      Savings: {format_number(opp.estimated_savings)} tokens ({opp.confidence:.0%} confidence)")
                lines.append(f"      {opp.reason}")
                lines.append("")

        if medium_impact:
            lines.append("  MEDIUM IMPACT (500-1,000 tokens each):")
            for opp in medium_impact[:3]:
                lines.append(f"    Turn {opp.turn_id}: {opp.original_command[:60]}")
                lines.append(f"      -> {opp.suggested_tool}, Savings: {format_number(opp.estimated_savings)}")
            lines.append("")

        if low_impact and not high_impact and not medium_impact:
            lines.append("  LOW IMPACT (<500 tokens each):")
            lines.append(f"    {len(low_impact)} additional opportunities detected")
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

    # Gabb opportunities (Phase 2)
    if analysis.opportunities:
        console.print()

        # Summary panel
        total_savings = sum(o.estimated_savings for o in analysis.opportunities)
        total_tokens = summary["total_input_tokens"] + summary["total_output_tokens"]
        savings_pct = (total_savings / total_tokens * 100) if total_tokens > 0 else 0

        console.print(
            Panel.fit(
                f"[bold green]{len(analysis.opportunities)}[/bold green] opportunities detected\n"
                f"Potential savings: [bold]{format_number(total_savings)}[/bold] tokens ({savings_pct:.1f}%)",
                title="[bold]Gabb Optimization Opportunities[/bold]",
                border_style="green",
            )
        )

        # Opportunities table
        opp_table = Table(title="Top Opportunities")
        opp_table.add_column("Turn", justify="right", style="dim")
        opp_table.add_column("Original", max_width=40)
        opp_table.add_column("Suggested", style="green")
        opp_table.add_column("Savings", justify="right", style="bold")
        opp_table.add_column("Conf.", justify="right")

        # Show top 10 opportunities
        for opp in analysis.opportunities[:10]:
            opp_table.add_row(
                str(opp.turn_id),
                opp.original_command[:40] + ("..." if len(opp.original_command) > 40 else ""),
                opp.suggested_tool,
                format_number(opp.estimated_savings),
                f"{opp.confidence:.0%}",
            )

        console.print(opp_table)

        # Show reasons for top 3
        if analysis.opportunities:
            console.print()
            console.print("[dim]Top recommendations:[/dim]")
            for i, opp in enumerate(analysis.opportunities[:3], 1):
                console.print(f"  {i}. {opp.reason}")

    console.print()
