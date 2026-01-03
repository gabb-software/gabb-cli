#!/usr/bin/env python3
"""
Compare benchmark results between branches.

Compares benchmark results from different git branches to measure the impact
of changes on Claude Code performance with gabb.

Usage:
    # Compare two result files directly
    python compare.py results/file_a.json results/file_b.json

    # Compare all results for two branches
    python compare.py --branch main --branch feature/new-symbols

    # Compare with statistical tests
    python compare.py --branch main --branch feature/new-symbols --stats

    # List available branches with results
    python compare.py --list-branches
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

BENCHMARK_DIR = Path(__file__).parent
RESULTS_DIR = BENCHMARK_DIR / "results"


@dataclass
class BranchResults:
    """Aggregated results for a branch."""

    branch: str
    commit: str
    run_count: int
    result_files: list[Path]
    # Per-condition aggregates (keyed by condition name)
    conditions: dict[str, dict[str, Any]]
    # Raw metrics for statistical tests
    raw_metrics: dict[str, list[float]]


def load_result_file(path: Path) -> dict[str, Any] | None:
    """Load a result JSON file."""
    try:
        with open(path) as f:
            return json.load(f)
    except (json.JSONDecodeError, OSError) as e:
        print(f"Warning: Failed to load {path}: {e}", file=sys.stderr)
        return None


def scan_results_by_branch(results_dir: Path) -> dict[str, list[Path]]:
    """Scan results directory and group files by branch name.

    Returns:
        Dict mapping branch name to list of result files.
    """
    branches: dict[str, list[Path]] = {}

    for path in results_dir.glob("*.json"):
        data = load_result_file(path)
        if not data:
            continue

        branch_info = data.get("branch_info", {})
        branch = branch_info.get("branch", "unknown")

        if branch not in branches:
            branches[branch] = []
        branches[branch].append(path)

    return branches


def load_branch_results(
    branch: str, result_files: list[Path]
) -> BranchResults | None:
    """Load and aggregate results for a branch.

    Args:
        branch: Branch name.
        result_files: List of result file paths for this branch.

    Returns:
        BranchResults with aggregated data, or None if no valid files.
    """
    if not result_files:
        return None

    # Collect raw metrics across all files for statistical tests
    raw_metrics: dict[str, list[float]] = {
        "wall_time_seconds": [],
        "tokens_total": [],
        "cost_usd": [],
        "tool_calls": [],
        "turns": [],
    }

    # Aggregate conditions
    conditions: dict[str, dict[str, Any]] = {}
    commit = "unknown"
    total_runs = 0

    for path in result_files:
        data = load_result_file(path)
        if not data:
            continue

        # Get commit from first file
        if commit == "unknown":
            commit = data.get("branch_info", {}).get("commit", "unknown")

        # Extract runs from conditions
        for cond_name, cond_data in data.get("conditions", {}).items():
            runs = cond_data.get("runs", [])
            total_runs += len(runs)

            # Collect raw metrics
            for run in runs:
                raw_metrics["wall_time_seconds"].append(run.get("wall_time_seconds", 0))
                raw_metrics["tokens_total"].append(
                    run.get("tokens_input", 0) + run.get("tokens_output", 0)
                )
                raw_metrics["cost_usd"].append(run.get("cost_usd", 0))
                raw_metrics["tool_calls"].append(sum(run.get("tool_calls", {}).values()))
                raw_metrics["turns"].append(run.get("turns", 0))

            # Store aggregate
            if cond_name not in conditions:
                conditions[cond_name] = cond_data.get("aggregate", {})

    if total_runs == 0:
        return None

    return BranchResults(
        branch=branch,
        commit=commit,
        run_count=total_runs,
        result_files=result_files,
        conditions=conditions,
        raw_metrics=raw_metrics,
    )


def compute_delta(a: float, b: float) -> tuple[float, float]:
    """Compute absolute and percentage delta.

    Args:
        a: Baseline value.
        b: Comparison value.

    Returns:
        Tuple of (absolute_delta, percentage_delta).
    """
    delta = b - a
    pct = (delta / a * 100) if a != 0 else 0
    return delta, pct


def mann_whitney_u_test(a: list[float], b: list[float]) -> float | None:
    """Run Mann-Whitney U test and return p-value.

    Args:
        a: First sample.
        b: Second sample.

    Returns:
        p-value or None if scipy not available.
    """
    try:
        from scipy import stats

        if len(a) < 2 or len(b) < 2:
            return None
        _, p_value = stats.mannwhitneyu(a, b, alternative="two-sided")
        return p_value
    except ImportError:
        return None


def format_pvalue(p: float | None) -> str:
    """Format p-value with significance indicators."""
    if p is None:
        return "N/A"
    if p < 0.001:
        return f"{p:.4f}***"
    elif p < 0.01:
        return f"{p:.3f}**"
    elif p < 0.05:
        return f"{p:.3f}*"
    return f"{p:.3f}"


def generate_comparison_report(
    results_a: BranchResults,
    results_b: BranchResults,
    include_stats: bool = False,
) -> str:
    """Generate markdown comparison report.

    Args:
        results_a: Baseline branch results.
        results_b: Comparison branch results.
        include_stats: Whether to include statistical tests.

    Returns:
        Markdown formatted report.
    """
    lines = []
    lines.append(f"## A/B Comparison: {results_a.branch} vs {results_b.branch}")
    lines.append("")
    lines.append(f"**Baseline:** {results_a.branch} @ {results_a.commit} (n={results_a.run_count})")
    lines.append(f"**Comparison:** {results_b.branch} @ {results_b.commit} (n={results_b.run_count})")
    lines.append("")

    # Metrics comparison table
    if include_stats:
        lines.append("| Metric | Baseline | Comparison | Delta | p-value |")
        lines.append("|--------|----------|------------|-------|---------|")
    else:
        lines.append("| Metric | Baseline | Comparison | Delta |")
        lines.append("|--------|----------|------------|-------|")

    metrics = [
        ("Time (s)", "wall_time_seconds", "{:.1f}"),
        ("Tokens", "tokens_total", "{:,.0f}"),
        ("Cost ($)", "cost_usd", "{:.4f}"),
        ("Tool Calls", "tool_calls", "{:.1f}"),
        ("Turns", "turns", "{:.1f}"),
    ]

    for label, key, fmt in metrics:
        a_vals = results_a.raw_metrics.get(key, [])
        b_vals = results_b.raw_metrics.get(key, [])

        if not a_vals or not b_vals:
            continue

        a_mean = sum(a_vals) / len(a_vals)
        b_mean = sum(b_vals) / len(b_vals)
        delta, pct = compute_delta(a_mean, b_mean)

        a_str = fmt.format(a_mean)
        b_str = fmt.format(b_mean)
        delta_str = f"{pct:+.1f}%"

        if include_stats:
            p_value = mann_whitney_u_test(a_vals, b_vals)
            p_str = format_pvalue(p_value)
            lines.append(f"| {label} | {a_str} | {b_str} | {delta_str} | {p_str} |")
        else:
            lines.append(f"| {label} | {a_str} | {b_str} | {delta_str} |")

    lines.append("")

    # Tool usage comparison
    lines.append("### Tool Usage Changes")
    lines.append("")

    # Collect all tools from both branches
    a_tools: dict[str, float] = {}
    b_tools: dict[str, float] = {}

    for cond in results_a.conditions.values():
        for tool, stats in cond.get("tool_calls", {}).items():
            mean = stats.get("mean", 0) if isinstance(stats, dict) else stats
            a_tools[tool] = a_tools.get(tool, 0) + mean

    for cond in results_b.conditions.values():
        for tool, stats in cond.get("tool_calls", {}).items():
            mean = stats.get("mean", 0) if isinstance(stats, dict) else stats
            b_tools[tool] = b_tools.get(tool, 0) + mean

    all_tools = sorted(set(a_tools.keys()) | set(b_tools.keys()))

    if all_tools:
        lines.append("| Tool | Baseline | Comparison | Delta |")
        lines.append("|------|----------|------------|-------|")

        for tool in all_tools:
            a_count = a_tools.get(tool, 0)
            b_count = b_tools.get(tool, 0)
            if a_count == 0 and b_count == 0:
                continue

            _, pct = compute_delta(a_count, b_count) if a_count > 0 else (b_count, 100)
            delta_str = f"{pct:+.0f}%" if a_count > 0 else "NEW"
            lines.append(f"| {tool} | {a_count:.1f} | {b_count:.1f} | {delta_str} |")

    return "\n".join(lines)


def list_branches(results_dir: Path) -> None:
    """List all branches with available results."""
    branches = scan_results_by_branch(results_dir)

    if not branches:
        print("No result files found.")
        return

    print("Available branches with results:\n")
    for branch, files in sorted(branches.items()):
        print(f"  {branch}: {len(files)} result file(s)")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Compare benchmark results between branches",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )

    parser.add_argument(
        "files",
        nargs="*",
        type=Path,
        help="Two result files to compare directly",
    )
    parser.add_argument(
        "--branch", "-b",
        action="append",
        dest="branches",
        help="Branch name to compare (use twice for A/B comparison)",
    )
    parser.add_argument(
        "--stats",
        action="store_true",
        help="Include statistical significance tests",
    )
    parser.add_argument(
        "--list-branches",
        action="store_true",
        help="List available branches with results",
    )
    parser.add_argument(
        "--results-dir",
        type=Path,
        default=RESULTS_DIR,
        help=f"Results directory (default: {RESULTS_DIR})",
    )

    args = parser.parse_args()

    if args.list_branches:
        list_branches(args.results_dir)
        return 0

    # Direct file comparison
    if args.files:
        if len(args.files) != 2:
            print("Error: Provide exactly 2 result files for comparison", file=sys.stderr)
            return 1

        data_a = load_result_file(args.files[0])
        data_b = load_result_file(args.files[1])

        if not data_a or not data_b:
            return 1

        branch_a = data_a.get("branch_info", {}).get("branch", "file_a")
        branch_b = data_b.get("branch_info", {}).get("branch", "file_b")

        results_a = load_branch_results(branch_a, [args.files[0]])
        results_b = load_branch_results(branch_b, [args.files[1]])

        if not results_a or not results_b:
            print("Error: Failed to load results", file=sys.stderr)
            return 1

        report = generate_comparison_report(results_a, results_b, args.stats)
        print(report)
        return 0

    # Branch comparison
    if args.branches:
        if len(args.branches) != 2:
            print("Error: Provide exactly 2 branches with --branch", file=sys.stderr)
            return 1

        all_branches = scan_results_by_branch(args.results_dir)

        branch_a, branch_b = args.branches

        if branch_a not in all_branches:
            print(f"Error: No results found for branch '{branch_a}'", file=sys.stderr)
            print(f"Available: {list(all_branches.keys())}", file=sys.stderr)
            return 1

        if branch_b not in all_branches:
            print(f"Error: No results found for branch '{branch_b}'", file=sys.stderr)
            print(f"Available: {list(all_branches.keys())}", file=sys.stderr)
            return 1

        results_a = load_branch_results(branch_a, all_branches[branch_a])
        results_b = load_branch_results(branch_b, all_branches[branch_b])

        if not results_a or not results_b:
            print("Error: Failed to load results", file=sys.stderr)
            return 1

        report = generate_comparison_report(results_a, results_b, args.stats)
        print(report)
        return 0

    parser.print_help()
    return 1


if __name__ == "__main__":
    sys.exit(main())
