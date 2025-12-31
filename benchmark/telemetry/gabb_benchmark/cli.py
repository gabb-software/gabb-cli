"""CLI for gabb-benchmark telemetry analysis.

Usage:
    gabb-benchmark analyze <transcript.json>
    gabb-benchmark analyze <transcript.json> --format json
    gabb-benchmark analyze <transcript.json> --verbose
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import NoReturn

from . import __version__
from .parser import load_transcript, load_jsonl_transcript
from .classifier import classify_tool_calls
from .estimator import estimate_transcript_tokens
from .reporter import generate_json_report, generate_text_report, print_rich_report


def main(args: list[str] | None = None) -> int:
    """Main CLI entry point.

    Args:
        args: Command line arguments (uses sys.argv if None).

    Returns:
        Exit code (0 for success, non-zero for errors).
    """
    parser = argparse.ArgumentParser(
        prog="gabb-benchmark",
        description="Analyze Claude Code transcripts to identify gabb optimization opportunities",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    # Analyze a single transcript
    gabb-benchmark analyze conversation.json

    # Output as JSON for further processing
    gabb-benchmark analyze conversation.json --format json > report.json

    # Analyze with verbose output
    gabb-benchmark analyze conversation.json --verbose

    # Analyze multiple transcripts
    gabb-benchmark analyze *.json --summary
""",
    )
    parser.add_argument(
        "--version",
        action="version",
        version=f"%(prog)s {__version__}",
    )

    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Analyze command
    analyze_parser = subparsers.add_parser(
        "analyze",
        help="Analyze a transcript file",
        description="Parse and analyze a Claude Code conversation transcript",
    )
    analyze_parser.add_argument(
        "files",
        nargs="+",
        type=Path,
        help="Transcript file(s) to analyze (JSON or JSONL)",
    )
    analyze_parser.add_argument(
        "--format",
        "-f",
        choices=["text", "json", "rich"],
        default="rich",
        help="Output format (default: rich)",
    )
    analyze_parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Show detailed per-turn breakdown",
    )
    analyze_parser.add_argument(
        "--summary",
        "-s",
        action="store_true",
        help="Show aggregate summary across all files",
    )

    parsed = parser.parse_args(args)

    if not parsed.command:
        parser.print_help()
        return 1

    if parsed.command == "analyze":
        return cmd_analyze(parsed)

    return 0


def cmd_analyze(args: argparse.Namespace) -> int:
    """Handle the analyze command.

    Args:
        args: Parsed arguments.

    Returns:
        Exit code.
    """
    all_analyses = []

    for filepath in args.files:
        if not filepath.exists():
            print(f"Error: File not found: {filepath}", file=sys.stderr)
            return 1

        try:
            # Load transcript(s)
            if filepath.suffix == ".jsonl":
                analyses = load_jsonl_transcript(filepath)
            else:
                analyses = [load_transcript(filepath)]

            # Process each transcript
            for analysis in analyses:
                # Classify tool calls (parse Bash commands)
                classify_tool_calls(analysis)

                # Estimate tokens
                estimate_transcript_tokens(analysis)

                all_analyses.append(analysis)

                # Output individual report (unless --summary)
                if not args.summary:
                    if args.format == "json":
                        print(generate_json_report(analysis))
                    elif args.format == "text":
                        print(generate_text_report(analysis))
                    else:  # rich
                        print_rich_report(analysis)

        except json.JSONDecodeError as e:
            print(f"Error: Invalid JSON in {filepath}: {e}", file=sys.stderr)
            return 1
        except Exception as e:
            print(f"Error processing {filepath}: {e}", file=sys.stderr)
            return 1

    # Print summary if requested
    if args.summary and all_analyses:
        print_summary(all_analyses, args.format)

    return 0


def print_summary(analyses: list, output_format: str) -> None:
    """Print aggregate summary across multiple analyses.

    Args:
        analyses: List of TranscriptAnalysis objects.
        output_format: Output format (text, json, rich).
    """
    # Aggregate metrics
    total_turns = sum(len(a.turns) for a in analyses)
    total_tool_calls = sum(
        sum(len(t.tool_calls) for t in a.turns) for a in analyses
    )
    total_input_tokens = sum(a.total_input_tokens for a in analyses)
    total_output_tokens = sum(a.total_output_tokens for a in analyses)
    total_file_tokens = sum(a.file_content_tokens for a in analyses)

    # Aggregate tool distribution
    tool_counts: dict[str, int] = {}
    bash_breakdown: dict[str, int] = {}

    for a in analyses:
        for turn in a.turns:
            for tc in turn.tool_calls:
                tool_counts[tc.tool_name] = tool_counts.get(tc.tool_name, 0) + 1
                if tc.bash_info:
                    cmd = tc.bash_info.command_type
                    bash_breakdown[cmd] = bash_breakdown.get(cmd, 0) + 1

    summary = {
        "transcript_count": len(analyses),
        "total_turns": total_turns,
        "total_tool_calls": total_tool_calls,
        "total_input_tokens": total_input_tokens,
        "total_output_tokens": total_output_tokens,
        "total_file_content_tokens": total_file_tokens,
        "tool_distribution": tool_counts,
        "bash_breakdown": bash_breakdown,
    }

    if output_format == "json":
        print(json.dumps(summary, indent=2))
    else:
        # Text/rich output
        print()
        print("=" * 70)
        print("              AGGREGATE SUMMARY")
        print("=" * 70)
        print(f"  Transcripts analyzed:  {len(analyses)}")
        print(f"  Total turns:           {total_turns}")
        print(f"  Total tool calls:      {total_tool_calls}")
        print(f"  Total input tokens:    {total_input_tokens:,}")
        print(f"  Total output tokens:   {total_output_tokens:,}")
        print(f"  File content tokens:   {total_file_tokens:,}")
        print()
        print("Tool distribution:")
        for tool, count in sorted(tool_counts.items(), key=lambda x: -x[1]):
            print(f"  {tool:<30} {count:>5}")
        if bash_breakdown:
            print()
            print("Bash command breakdown:")
            for cmd, count in sorted(bash_breakdown.items(), key=lambda x: -x[1]):
                print(f"  {cmd:<30} {count:>5}")
        print("=" * 70)


if __name__ == "__main__":
    sys.exit(main())
