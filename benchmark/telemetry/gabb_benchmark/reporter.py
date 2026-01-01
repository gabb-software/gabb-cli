"""Report generation for transcript analysis.

Generates formatted reports showing tool usage breakdown, token counts,
and gabb optimization opportunities.

Phase 3: Reporting and Visualization
- Summary report with high-level statistics
- Per-turn breakdown with detailed analysis
- Recommendations based on detected patterns
- Multiple output formats: JSON, Markdown, text, terminal (rich)
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


def generate_recommendations(analysis: TranscriptAnalysis) -> list[dict]:
    """Generate actionable recommendations based on detected patterns.

    Args:
        analysis: The analyzed transcript.

    Returns:
        List of recommendation dicts with priority, title, description, and impact.
    """
    recommendations = []
    opportunities = analysis.opportunities

    if not opportunities:
        return recommendations

    # Count opportunity types
    type_counts: dict[str, int] = {}
    type_savings: dict[str, int] = {}
    for opp in opportunities:
        opp_type = opp.type.value
        type_counts[opp_type] = type_counts.get(opp_type, 0) + 1
        type_savings[opp_type] = type_savings.get(opp_type, 0) + opp.estimated_savings

    # Recommendation 1: gabb_structure for large file reads
    read_structure_count = type_counts.get("read_to_structure", 0)
    if read_structure_count >= 1:
        savings = type_savings.get("read_to_structure", 0)
        recommendations.append({
            "priority": 1,
            "title": "Use gabb_structure before reading large files",
            "description": (
                f"Found {read_structure_count} large file read(s) that could benefit from "
                f"gabb_structure first. This tool returns a cheap overview of symbols and "
                f"line numbers without the token cost of reading the full file."
            ),
            "impact": f"~{format_number(savings)} tokens saved",
            "example": "gabb_structure file=\"src/large_file.rs\" → then Read with offset/limit",
        })

    # Recommendation 2: gabb_usages for grep of symbol names
    grep_usages_count = type_counts.get("grep_to_usages", 0)
    if grep_usages_count >= 1:
        savings = type_savings.get("grep_to_usages", 0)
        recommendations.append({
            "priority": 2,
            "title": "Use gabb_usages for finding symbol references",
            "description": (
                f"Found {grep_usages_count} grep command(s) searching for symbol names. "
                f"gabb_usages provides semantic accuracy (won't match comments/strings) "
                f"and returns precise file:line:column locations."
            ),
            "impact": f"~{format_number(savings)} tokens saved",
            "example": "gabb_usages file=\"src/auth.ts\" line=42 character=10",
        })

    # Recommendation 3: gabb_symbol for finding definitions
    grep_symbol_count = type_counts.get("grep_to_symbol", 0)
    if grep_symbol_count >= 1:
        savings = type_savings.get("grep_to_symbol", 0)
        recommendations.append({
            "priority": 3,
            "title": "Use gabb_symbol for finding symbol definitions",
            "description": (
                f"Found {grep_symbol_count} grep command(s) that appear to be searching "
                f"for where a symbol is defined. gabb_symbol provides instant lookup by name."
            ),
            "impact": f"~{format_number(savings)} tokens saved",
            "example": "gabb_symbol name=\"MyFunction\"",
        })

    # Recommendation 4: gabb_definition for multi-hop navigation
    multi_hop_count = type_counts.get("multi_hop_to_definition", 0)
    if multi_hop_count >= 1:
        savings = type_savings.get("multi_hop_to_definition", 0)
        recommendations.append({
            "priority": 4,
            "title": "Use gabb_definition for navigation chains",
            "description": (
                f"Found {multi_hop_count} multi-hop navigation pattern(s) (grep → read → locate). "
                f"gabb_definition collapses this to a single call that jumps from usage to definition."
            ),
            "impact": f"~{format_number(savings)} tokens saved",
            "example": "gabb_definition file=\"src/app.ts\" line=10 character=5",
        })

    # Recommendation 5: gabb_symbols for find+grep combos
    find_grep_count = type_counts.get("find_grep_to_symbols", 0)
    if find_grep_count >= 1:
        savings = type_savings.get("find_grep_to_symbols", 0)
        recommendations.append({
            "priority": 5,
            "title": "Use gabb_symbols with filters for find+grep patterns",
            "description": (
                f"Found {find_grep_count} find+grep combination(s). gabb_symbols supports "
                f"file glob patterns and name filters in a single query."
            ),
            "impact": f"~{format_number(savings)} tokens saved",
            "example": "gabb_symbols file=\"src/**/*.ts\" name_contains=\"handle\"",
        })

    # Recommendation 6: Generic grep to symbols
    grep_symbols_count = type_counts.get("grep_to_symbols", 0)
    if grep_symbols_count >= 1:
        savings = type_savings.get("grep_to_symbols", 0)
        recommendations.append({
            "priority": 6,
            "title": "Use gabb_symbols for code pattern searches",
            "description": (
                f"Found {grep_symbols_count} grep command(s) searching for code patterns. "
                f"gabb_symbols provides indexed search with kind filters (function, class, etc.)."
            ),
            "impact": f"~{format_number(savings)} tokens saved",
            "example": "gabb_symbols kind=\"function\" name_contains=\"validate\"",
        })

    # Sort by priority
    recommendations.sort(key=lambda r: r["priority"])

    return recommendations


def generate_markdown_report(analysis: TranscriptAnalysis, verbose: bool = False) -> str:
    """Generate a Markdown report from the analysis.

    Args:
        analysis: The analyzed transcript.
        verbose: Include detailed per-turn breakdown.

    Returns:
        Markdown-formatted report string.
    """
    lines = []
    data = analysis.to_dict()
    summary = data["summary"]

    # Header
    lines.append("# Gabb Benchmark Report")
    lines.append("")

    # Task info
    if analysis.task_description:
        lines.append(f"**Task:** {analysis.task_description}")
        lines.append("")

    # Summary section
    lines.append("## Summary")
    lines.append("")
    total_tokens = summary["total_input_tokens"] + summary["total_output_tokens"]
    file_pct = (
        (summary["file_content_tokens"] / total_tokens * 100)
        if total_tokens > 0
        else 0
    )

    lines.append(f"| Metric | Value |")
    lines.append(f"|--------|-------|")
    lines.append(f"| Total turns | {summary['total_turns']} |")
    lines.append(f"| Total tool calls | {summary['tool_call_count']} |")
    lines.append(f"| Input tokens | {format_number(summary['total_input_tokens'])} |")
    lines.append(f"| Output tokens | {format_number(summary['total_output_tokens'])} |")
    lines.append(f"| File content tokens | {format_number(summary['file_content_tokens'])} ({file_pct:.0f}%) |")
    lines.append("")

    # Tool distribution
    lines.append("## Tool Distribution")
    lines.append("")
    lines.append("| Tool | Calls | Tokens |")
    lines.append("|------|-------|--------|")
    tool_dist = data.get("tool_distribution", {})
    for tool, stats in sorted(tool_dist.items(), key=lambda x: -x[1]["count"]):
        lines.append(f"| {tool} | {stats['count']} | {format_number(stats['tokens'])} |")
    lines.append("")

    # Bash breakdown
    bash_breakdown = data.get("bash_breakdown", {})
    if bash_breakdown:
        lines.append("### Bash Command Breakdown")
        lines.append("")
        lines.append("| Command | Count |")
        lines.append("|---------|-------|")
        for cmd, count in sorted(bash_breakdown.items(), key=lambda x: -x[1]):
            lines.append(f"| {cmd} | {count} |")
        lines.append("")

    # Per-turn breakdown (verbose mode)
    if verbose and analysis.turns:
        lines.append("## Per-Turn Breakdown")
        lines.append("")
        lines.append("| Turn | Tools | Input Tokens | Output Tokens | Details |")
        lines.append("|------|-------|--------------|---------------|---------|")
        for turn in analysis.turns:
            tool_names = [tc.tool_name for tc in turn.tool_calls]
            tool_summary = ", ".join(tool_names[:3])
            if len(tool_names) > 3:
                tool_summary += f" (+{len(tool_names) - 3})"
            lines.append(
                f"| {turn.turn_id} | {len(turn.tool_calls)} | "
                f"{format_number(turn.input_tokens)} | "
                f"{format_number(turn.output_tokens)} | "
                f"{tool_summary} |"
            )
        lines.append("")

        # Detailed tool calls per turn
        lines.append("### Detailed Tool Calls")
        lines.append("")
        for turn in analysis.turns:
            if turn.tool_calls:
                lines.append(f"**Turn {turn.turn_id}:**")
                lines.append("")
                for i, tc in enumerate(turn.tool_calls, 1):
                    if tc.tool_name == "Bash" and tc.bash_info:
                        cmd_preview = tc.bash_info.raw_command[:60]
                        if len(tc.bash_info.raw_command) > 60:
                            cmd_preview += "..."
                        lines.append(f"{i}. `Bash`: `{cmd_preview}`")
                    elif tc.tool_name == "Read":
                        file_path = tc.tool_input.get("file_path", "unknown")
                        lines.append(f"{i}. `Read`: `{file_path}`")
                    elif tc.tool_name == "Grep":
                        pattern = tc.tool_input.get("pattern", "")
                        lines.append(f"{i}. `Grep`: pattern=`{pattern}`")
                    elif tc.tool_name == "Glob":
                        pattern = tc.tool_input.get("pattern", "")
                        lines.append(f"{i}. `Glob`: `{pattern}`")
                    else:
                        lines.append(f"{i}. `{tc.tool_name}`")
                lines.append("")

    # Gabb opportunities
    if analysis.opportunities:
        lines.append("## Gabb Optimization Opportunities")
        lines.append("")

        total_savings = sum(o.estimated_savings for o in analysis.opportunities)
        savings_pct = (total_savings / total_tokens * 100) if total_tokens > 0 else 0

        lines.append(f"**{len(analysis.opportunities)} opportunities detected** with potential savings of **{format_number(total_savings)} tokens ({savings_pct:.1f}%)**")
        lines.append("")

        # Group by impact
        high_impact = [o for o in analysis.opportunities if o.estimated_savings >= 1000]
        medium_impact = [o for o in analysis.opportunities if 500 <= o.estimated_savings < 1000]
        low_impact = [o for o in analysis.opportunities if o.estimated_savings < 500]

        if high_impact:
            lines.append("### High Impact (>1,000 tokens each)")
            lines.append("")
            for opp in high_impact:
                lines.append(f"- **Turn {opp.turn_id}**: `{opp.original_command[:60]}{'...' if len(opp.original_command) > 60 else ''}`")
                lines.append(f"  - → `{opp.suggested_tool}`")
                lines.append(f"  - Savings: {format_number(opp.estimated_savings)} tokens ({opp.confidence:.0%} confidence)")
                lines.append(f"  - {opp.reason}")
            lines.append("")

        if medium_impact:
            lines.append("### Medium Impact (500-1,000 tokens)")
            lines.append("")
            for opp in medium_impact:
                lines.append(f"- **Turn {opp.turn_id}**: `{opp.original_command[:50]}...` → `{opp.suggested_tool}` ({format_number(opp.estimated_savings)} tokens)")
            lines.append("")

        if low_impact:
            lines.append(f"### Low Impact (<500 tokens): {len(low_impact)} additional opportunities")
            lines.append("")

    # Recommendations
    recommendations = generate_recommendations(analysis)
    if recommendations:
        lines.append("## Recommendations")
        lines.append("")
        for rec in recommendations:
            lines.append(f"### {rec['priority']}. {rec['title']}")
            lines.append("")
            lines.append(rec["description"])
            lines.append("")
            lines.append(f"**Impact:** {rec['impact']}")
            lines.append("")
            lines.append(f"**Example:**")
            lines.append(f"```")
            lines.append(rec["example"])
            lines.append(f"```")
            lines.append("")

    return "\n".join(lines)


def print_rich_report(analysis: TranscriptAnalysis, verbose: bool = False) -> None:
    """Print a rich-formatted report to the console.

    Args:
        analysis: The analyzed transcript.
        verbose: Include detailed per-turn breakdown.
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

    # Per-turn breakdown (verbose mode)
    if verbose and analysis.turns:
        console.print()
        turn_table = Table(title="Per-Turn Breakdown")
        turn_table.add_column("Turn", justify="right", style="dim")
        turn_table.add_column("Tools", justify="right")
        turn_table.add_column("Input", justify="right")
        turn_table.add_column("Output", justify="right")
        turn_table.add_column("Details", max_width=40)

        for turn in analysis.turns:
            tool_names = [tc.tool_name for tc in turn.tool_calls]
            tool_summary = ", ".join(tool_names[:3])
            if len(tool_names) > 3:
                tool_summary += f" (+{len(tool_names) - 3})"

            turn_table.add_row(
                str(turn.turn_id),
                str(len(turn.tool_calls)),
                format_number(turn.input_tokens),
                format_number(turn.output_tokens),
                tool_summary,
            )

        console.print(turn_table)

        # Detailed tool calls
        console.print()
        console.print("[bold]Detailed Tool Calls:[/bold]")
        for turn in analysis.turns:
            if turn.tool_calls:
                console.print(f"\n[cyan]Turn {turn.turn_id}:[/cyan]")
                for i, tc in enumerate(turn.tool_calls, 1):
                    if tc.tool_name == "Bash" and tc.bash_info:
                        cmd_preview = tc.bash_info.raw_command[:50]
                        if len(tc.bash_info.raw_command) > 50:
                            cmd_preview += "..."
                        console.print(f"  {i}. [yellow]Bash[/yellow]: {cmd_preview}")
                    elif tc.tool_name == "Read":
                        file_path = tc.tool_input.get("file_path", "unknown")
                        console.print(f"  {i}. [green]Read[/green]: {file_path}")
                    elif tc.tool_name == "Grep":
                        pattern = tc.tool_input.get("pattern", "")
                        console.print(f"  {i}. [magenta]Grep[/magenta]: pattern={pattern}")
                    elif tc.tool_name == "Glob":
                        pattern = tc.tool_input.get("pattern", "")
                        console.print(f"  {i}. [blue]Glob[/blue]: {pattern}")
                    else:
                        console.print(f"  {i}. {tc.tool_name}")

    # Gabb opportunities
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

    # Recommendations section
    recommendations = generate_recommendations(analysis)
    if recommendations:
        console.print()
        console.print(
            Panel.fit(
                "[bold]Recommendations[/bold]",
                border_style="yellow",
            )
        )
        for rec in recommendations:
            console.print()
            console.print(f"[bold yellow]{rec['priority']}.[/bold yellow] [bold]{rec['title']}[/bold]")
            console.print(f"   {rec['description']}")
            console.print(f"   [dim]Impact:[/dim] {rec['impact']}")
            console.print(f"   [dim]Example:[/dim] [green]{rec['example']}[/green]")

    console.print()
