#!/usr/bin/env python3
"""
Unified benchmark analysis tool for SWE-bench Claude Code benchmarks.

Features:
- Auto-detect result type (suite vs individual task)
- Accept: file path, --latest, date filter, branch filter
- Output: summary stats, per-task breakdown, statistical tests
- Formats: terminal (default), --markdown, --json
- Includes: t-tests, confidence intervals, Cohen's d, effect size interpretation

Usage:
    python analyze.py --latest                     # Analyze latest benchmark
    python analyze.py results/suite_*.json        # Analyze specific file
    python analyze.py --compare main fix-102      # Compare branches (future)
    python analyze.py --latest --markdown         # Output as markdown
    python analyze.py --latest --json             # Output as JSON

Requirements:
    pip install scipy  # Optional: for precise p-value calculation
"""

import argparse
import glob
import json
import math
import os
import sys
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

# Optional scipy import for precise p-values
try:
    from scipy import stats
    SCIPY_AVAILABLE = True
except ImportError:
    SCIPY_AVAILABLE = False

RESULTS_DIR = Path(__file__).parent / "results"
ANALYSIS_DIR = Path(__file__).parent / "analysis"


@dataclass
class StatResult:
    """Statistical test result."""
    t_stat: float
    df: float
    se_diff: float
    ci_lower: float
    ci_upper: float
    cohens_d: float
    p_value: Optional[float]

    @property
    def significance(self) -> str:
        """Return significance stars based on p-value or t-stat."""
        if self.p_value is not None:
            if self.p_value < 0.001:
                return "***"
            elif self.p_value < 0.01:
                return "**"
            elif self.p_value < 0.05:
                return "*"
            return ""
        else:
            # Approximate from t-stat when scipy unavailable
            t = abs(self.t_stat)
            if t > 4.0:
                return "***"
            elif t > 3.0:
                return "**"
            elif t > 2.0:
                return "*"
            return ""

    @property
    def effect_interpretation(self) -> str:
        """Interpret Cohen's d effect size."""
        d = abs(self.cohens_d)
        if d >= 1.0:
            return "very large"
        elif d >= 0.8:
            return "large"
        elif d >= 0.5:
            return "medium"
        elif d >= 0.2:
            return "small"
        return "negligible"


def find_latest_results() -> Optional[Path]:
    """Find the most recent results file."""
    patterns = [
        "suite_results_*.json",
        "results_*.json",
    ]
    all_files = []
    for pattern in patterns:
        all_files.extend(RESULTS_DIR.glob(pattern))

    if not all_files:
        return None

    # Sort by modification time, newest first
    return max(all_files, key=lambda p: p.stat().st_mtime)


def find_results_by_date(date_str: str) -> List[Path]:
    """Find results files matching a date pattern (YYYYMMDD)."""
    pattern = f"*_{date_str}_*.json"
    return list(RESULTS_DIR.glob(pattern))


def detect_result_type(data: Dict) -> str:
    """Detect whether this is a suite or individual task result."""
    if "tasks" in data and isinstance(data.get("tasks"), list):
        return "suite"
    elif "conditions" in data:
        return "individual"
    else:
        raise ValueError("Unknown result format: no 'tasks' or 'conditions' key found")


def two_sample_t_test(mean1: float, std1: float, n1: int,
                      mean2: float, std2: float, n2: int) -> StatResult:
    """
    Perform Welch's t-test (unequal variances).
    Returns StatResult with t-statistic, degrees of freedom, SE, CI, Cohen's d.
    """
    # Handle edge cases
    if std1 == 0 and std2 == 0:
        # No variance - can't compute meaningful stats
        return StatResult(
            t_stat=float('inf') if mean1 != mean2 else 0,
            df=n1 + n2 - 2,
            se_diff=0,
            ci_lower=mean1 - mean2,
            ci_upper=mean1 - mean2,
            cohens_d=float('inf') if mean1 != mean2 else 0,
            p_value=0 if mean1 != mean2 else 1
        )

    # Standard errors
    se1 = std1 / math.sqrt(n1) if n1 > 0 else 0
    se2 = std2 / math.sqrt(n2) if n2 > 0 else 0

    # Standard error of difference
    se_diff = math.sqrt(se1**2 + se2**2)

    # t-statistic
    if se_diff > 0:
        t_stat = (mean1 - mean2) / se_diff
    else:
        t_stat = 0

    # Welch-Satterthwaite degrees of freedom
    if se1**2 + se2**2 > 0:
        df = (se1**2 + se2**2)**2 / (
            (se1**4 / (n1 - 1) if n1 > 1 else 0) +
            (se2**4 / (n2 - 1) if n2 > 1 else 0)
        )
    else:
        df = n1 + n2 - 2

    # Cohen's d (pooled std)
    pooled_std = math.sqrt((std1**2 + std2**2) / 2) if std1 + std2 > 0 else 1
    cohens_d = (mean1 - mean2) / pooled_std

    # 95% CI (t_critical ~ 2.0 for large df)
    t_critical = 2.0 if df > 30 else 2.1
    mean_diff = mean1 - mean2
    ci_lower = mean_diff - t_critical * se_diff
    ci_upper = mean_diff + t_critical * se_diff

    # P-value (if scipy available)
    p_value = None
    if SCIPY_AVAILABLE and se_diff > 0:
        p_value = 2 * (1 - stats.t.cdf(abs(t_stat), df))

    return StatResult(
        t_stat=t_stat,
        df=df,
        se_diff=se_diff,
        ci_lower=ci_lower,
        ci_upper=ci_upper,
        cohens_d=cohens_d,
        p_value=p_value
    )


def extract_aggregate_stats(condition_data: Dict) -> Dict[str, Any]:
    """Extract aggregate statistics from condition data."""
    # Suite format has stats directly, individual has 'aggregate' key
    if "aggregate" in condition_data:
        return condition_data["aggregate"]
    # Suite summary format
    return condition_data


def detect_conditions(data: Dict, result_type: str) -> Tuple[str, str]:
    """Detect which conditions to compare in the results."""
    if result_type == "suite":
        # Suite always has control and gabb in summary
        return "control", "gabb"
    else:
        conditions = list(data.get("conditions", {}).keys())
        # Prefer control vs gabb if available
        if "control" in conditions and "gabb" in conditions:
            return "control", "gabb"
        # Otherwise use first two conditions
        if len(conditions) >= 2:
            return conditions[0], conditions[1]
        raise ValueError(f"Need at least 2 conditions to compare, found: {conditions}")


def analyze_results(data: Dict, result_type: str) -> Dict[str, Any]:
    """Analyze results and return structured analysis."""
    analysis = {
        "result_type": result_type,
        "timestamp": data.get("timestamp", "unknown"),
        "metrics": {},
        "stats": {},
        "tool_usage": {},
        "insights": [],
    }

    # Detect which conditions to compare
    baseline_name, treatment_name = detect_conditions(data, result_type)
    analysis["baseline_condition"] = baseline_name
    analysis["treatment_condition"] = treatment_name

    if result_type == "suite":
        baseline = data["summary"][baseline_name]
        treatment = data["summary"][treatment_name]
        analysis["task_count"] = data.get("task_count", len(data.get("tasks", [])))
        analysis["runs_per_task"] = data.get("run_count", 10)
        # Total runs for statistical tests = task_count * runs_per_task
        analysis["run_count"] = baseline.get("run_count", analysis["task_count"] * analysis["runs_per_task"])

        # Pre-computed savings from suite
        if "time_savings_pct" in data["summary"]:
            analysis["time_savings_pct"] = data["summary"]["time_savings_pct"]["mean"]
        if "token_savings_pct" in data["summary"]:
            analysis["token_savings_pct"] = data["summary"]["token_savings_pct"]["mean"]
    else:
        baseline = extract_aggregate_stats(data["conditions"][baseline_name])
        treatment = extract_aggregate_stats(data["conditions"][treatment_name])
        analysis["task_id"] = data.get("task_id", "unknown")
        analysis["run_count"] = data.get("run_count", baseline.get("run_count", 0))

    # Use shortcuts for the rest of the function
    control = baseline
    gabb = treatment
    n = analysis["run_count"]

    # Primary metrics
    metrics = ["wall_time_seconds", "tokens_total", "tokens_input", "tokens_output"]
    for metric in metrics:
        if metric in control and metric in gabb:
            c_val = control[metric]
            g_val = gabb[metric]

            c_mean = c_val["mean"] if isinstance(c_val, dict) else c_val
            c_std = c_val.get("std", 0) if isinstance(c_val, dict) else 0
            g_mean = g_val["mean"] if isinstance(g_val, dict) else g_val
            g_std = g_val.get("std", 0) if isinstance(g_val, dict) else 0

            diff = g_mean - c_mean
            pct_change = (diff / c_mean * 100) if c_mean != 0 else 0

            analysis["metrics"][metric] = {
                "control": {"mean": c_mean, "std": c_std},
                "gabb": {"mean": g_mean, "std": g_std},
                "difference": diff,
                "pct_change": pct_change,
            }

            # Statistical test
            if c_std > 0 or g_std > 0:
                stat_result = two_sample_t_test(
                    c_mean, c_std, n,
                    g_mean, g_std, n
                )
                analysis["stats"][metric] = {
                    "t_stat": stat_result.t_stat,
                    "df": stat_result.df,
                    "p_value": stat_result.p_value,
                    "cohens_d": stat_result.cohens_d,
                    "ci_95": [stat_result.ci_lower, stat_result.ci_upper],
                    "significance": stat_result.significance,
                    "effect_size": stat_result.effect_interpretation,
                }

    # Success rates
    analysis["metrics"]["success_rate"] = {
        "control": control.get("success_rate", 0),
        "gabb": gabb.get("success_rate", 0),
    }

    # Tool usage comparison
    c_tools = control.get("tool_calls", {})
    g_tools = gabb.get("tool_calls", {})
    all_tools = set(c_tools.keys()) | set(g_tools.keys())

    for tool in sorted(all_tools):
        c_data = c_tools.get(tool, {"mean": 0})
        g_data = g_tools.get(tool, {"mean": 0})
        c_mean = c_data["mean"] if isinstance(c_data, dict) else c_data
        g_mean = g_data["mean"] if isinstance(g_data, dict) else g_data

        analysis["tool_usage"][tool] = {
            "control": c_mean,
            "gabb": g_mean,
        }

    # Generate insights
    treatment_name = analysis.get("treatment_condition", "gabb").title()
    if "wall_time_seconds" in analysis["metrics"]:
        time_pct = analysis["metrics"]["wall_time_seconds"]["pct_change"]
        if time_pct < -20:
            analysis["insights"].append(f"{treatment_name} is {abs(time_pct):.0f}% faster")
        elif time_pct > 20:
            analysis["insights"].append(f"{treatment_name} is {time_pct:.0f}% slower")

    if "tokens_total" in analysis["metrics"]:
        token_pct = analysis["metrics"]["tokens_total"]["pct_change"]
        if token_pct < -10:
            analysis["insights"].append(f"Token usage reduced by {abs(token_pct):.0f}%")
        elif token_pct > 10:
            analysis["insights"].append(f"Token usage increased by {token_pct:.0f}%")

    # Tool replacement insights
    if "Grep" in analysis["tool_usage"] and "mcp__gabb__gabb_symbols" in analysis["tool_usage"]:
        grep_reduction = analysis["tool_usage"]["Grep"]["control"] - analysis["tool_usage"]["Grep"]["gabb"]
        if grep_reduction > 1:
            analysis["insights"].append(f"Grep calls reduced by {grep_reduction:.1f} on average")

    if "Read" in analysis["tool_usage"]:
        read_reduction = analysis["tool_usage"]["Read"]["control"] - analysis["tool_usage"]["Read"]["gabb"]
        if read_reduction > 1:
            analysis["insights"].append(f"Read calls reduced by {read_reduction:.1f} on average")

    return analysis


def format_terminal(analysis: Dict) -> str:
    """Format analysis for terminal output."""
    lines = []

    # Get condition names (capitalize first letter)
    baseline = analysis.get("baseline_condition", "control").title()
    treatment = analysis.get("treatment_condition", "gabb").title()

    # Header
    lines.append("=" * 70)
    if analysis["result_type"] == "suite":
        runs_per = analysis.get('runs_per_task', analysis['run_count'])
        lines.append(f"BENCHMARK ANALYSIS: Suite ({analysis['task_count']} tasks, {runs_per} runs/task)")
    else:
        lines.append(f"BENCHMARK ANALYSIS: {analysis.get('task_id', 'unknown')}")
    lines.append(f"Comparing: {baseline} vs {treatment}")
    lines.append(f"Timestamp: {analysis['timestamp']}")
    lines.append("=" * 70)
    lines.append("")

    # Key insights
    if analysis["insights"]:
        lines.append("KEY INSIGHTS:")
        for insight in analysis["insights"]:
            lines.append(f"  - {insight}")
        lines.append("")

    # Primary metrics table
    lines.append("PRIMARY METRICS:")
    lines.append("-" * 70)
    lines.append(f"{'Metric':<25} {baseline:>15} {treatment:>15} {'Diff':>12}")
    lines.append("-" * 70)

    metric_display = {
        "wall_time_seconds": "Time (s)",
        "tokens_total": "Total Tokens",
        "tokens_input": "Input Tokens",
        "tokens_output": "Output Tokens",
    }

    for metric, display in metric_display.items():
        if metric in analysis["metrics"]:
            m = analysis["metrics"][metric]
            c_str = f"{m['control']['mean']:.1f}"
            if m['control']['std'] > 0:
                c_str += f" ± {m['control']['std']:.1f}"

            g_str = f"{m['gabb']['mean']:.1f}"
            if m['gabb']['std'] > 0:
                g_str += f" ± {m['gabb']['std']:.1f}"

            pct = m['pct_change']
            sign = "+" if pct > 0 else ""
            diff_str = f"{sign}{pct:.1f}%"

            # Add significance stars
            if metric in analysis["stats"]:
                diff_str += f" {analysis['stats'][metric]['significance']}"

            lines.append(f"{display:<25} {c_str:>15} {g_str:>15} {diff_str:>12}")

    # Success rates
    if "success_rate" in analysis["metrics"]:
        sr = analysis["metrics"]["success_rate"]
        lines.append(f"{'Success Rate':<25} {sr['control']*100:>14.1f}% {sr['gabb']*100:>14.1f}%")

    lines.append("-" * 70)
    lines.append("")

    # Statistical tests
    if analysis["stats"]:
        lines.append("STATISTICAL SIGNIFICANCE:")
        lines.append("-" * 70)
        for metric, stat in analysis["stats"].items():
            display = metric_display.get(metric, metric)
            p_str = f"p={stat['p_value']:.4f}" if stat['p_value'] else f"t={stat['t_stat']:.2f}"
            lines.append(
                f"  {display}: {p_str}, d={stat['cohens_d']:.2f} ({stat['effect_size']}), "
                f"95% CI [{stat['ci_95'][0]:.1f}, {stat['ci_95'][1]:.1f}]"
            )
        lines.append("")
        lines.append("  Significance: * p<0.05, ** p<0.01, *** p<0.001")
        lines.append("")

    # Tool usage
    if analysis["tool_usage"]:
        lines.append("TOOL USAGE (mean calls per run):")
        lines.append("-" * 70)
        lines.append(f"{'Tool':<40} {baseline:>10} {treatment:>10}")
        lines.append("-" * 70)

        # Sort by control usage, descending
        sorted_tools = sorted(
            analysis["tool_usage"].items(),
            key=lambda x: x[1]["control"],
            reverse=True
        )
        for tool, usage in sorted_tools:
            if usage["control"] > 0 or usage["gabb"] > 0:
                lines.append(f"{tool:<40} {usage['control']:>10.1f} {usage['gabb']:>10.1f}")
        lines.append("")

    return "\n".join(lines)


def format_markdown(analysis: Dict) -> str:
    """Format analysis as markdown report."""
    lines = []
    date_str = datetime.now().strftime("%Y-%m-%d")

    # Get condition names
    baseline = analysis.get("baseline_condition", "control").title()
    treatment = analysis.get("treatment_condition", "gabb").title()

    lines.append(f"# Benchmark Analysis: {date_str}")
    lines.append("")

    # Executive summary
    lines.append("## Executive Summary")
    lines.append("")
    if analysis["insights"]:
        for insight in analysis["insights"]:
            lines.append(f"- {insight}")
    else:
        lines.append("- No significant differences detected")
    lines.append("")

    # Metadata
    lines.append("## Benchmark Details")
    lines.append("")
    if analysis["result_type"] == "suite":
        runs_per = analysis.get('runs_per_task', analysis['run_count'])
        lines.append(f"- **Type:** Suite ({analysis['task_count']} tasks)")
        lines.append(f"- **Runs per task:** {runs_per}")
    else:
        lines.append(f"- **Task:** {analysis.get('task_id', 'unknown')}")
        lines.append(f"- **Runs:** {analysis['run_count']}")
    lines.append(f"- **Comparing:** {baseline} vs {treatment}")
    lines.append(f"- **Timestamp:** {analysis['timestamp']}")
    lines.append("")

    # Primary metrics table
    lines.append("## Primary Metrics")
    lines.append("")
    lines.append(f"| Metric | {baseline} | {treatment} | Difference |")
    lines.append("|--------|---------|------|------------|")

    metric_display = {
        "wall_time_seconds": "Time (s)",
        "tokens_total": "Total Tokens",
        "tokens_input": "Input Tokens",
        "tokens_output": "Output Tokens",
    }

    for metric, display in metric_display.items():
        if metric in analysis["metrics"]:
            m = analysis["metrics"][metric]
            c_str = f"{m['control']['mean']:.1f} ± {m['control']['std']:.1f}"
            g_str = f"{m['gabb']['mean']:.1f} ± {m['gabb']['std']:.1f}"
            pct = m['pct_change']
            sign = "+" if pct > 0 else ""
            diff_str = f"{sign}{pct:.1f}%"
            if metric in analysis["stats"]:
                diff_str += f" {analysis['stats'][metric]['significance']}"
            lines.append(f"| {display} | {c_str} | {g_str} | {diff_str} |")

    if "success_rate" in analysis["metrics"]:
        sr = analysis["metrics"]["success_rate"]
        lines.append(f"| Success Rate | {sr['control']*100:.1f}% | {sr['gabb']*100:.1f}% | - |")

    lines.append("")

    # Statistical significance
    if analysis["stats"]:
        lines.append("## Statistical Significance")
        lines.append("")
        lines.append("| Metric | t-statistic | p-value | Cohen's d | Effect Size | 95% CI |")
        lines.append("|--------|-------------|---------|-----------|-------------|--------|")
        for metric, stat in analysis["stats"].items():
            display = metric_display.get(metric, metric)
            p_str = f"{stat['p_value']:.4f}" if stat['p_value'] else "N/A"
            lines.append(
                f"| {display} | {stat['t_stat']:.2f} | {p_str} | "
                f"{stat['cohens_d']:.2f} | {stat['effect_size']} | "
                f"[{stat['ci_95'][0]:.1f}, {stat['ci_95'][1]:.1f}] |"
            )
        lines.append("")
        lines.append("*Significance: \\* p<0.05, \\*\\* p<0.01, \\*\\*\\* p<0.001*")
        lines.append("")

    # Tool usage
    if analysis["tool_usage"]:
        lines.append("## Tool Usage")
        lines.append("")
        lines.append("Mean calls per run:")
        lines.append("")
        lines.append(f"| Tool | {baseline} | {treatment} |")
        lines.append("|------|---------|------|")
        sorted_tools = sorted(
            analysis["tool_usage"].items(),
            key=lambda x: x[1]["control"],
            reverse=True
        )
        for tool, usage in sorted_tools:
            if usage["control"] > 0 or usage["gabb"] > 0:
                # Escape underscores for markdown
                tool_display = tool.replace("_", "\\_")
                lines.append(f"| {tool_display} | {usage['control']:.1f} | {usage['gabb']:.1f} |")
        lines.append("")

    # Recommendations placeholder
    lines.append("## Recommendations")
    lines.append("")
    lines.append("*Add recommendations based on analysis findings.*")
    lines.append("")

    return "\n".join(lines)


def format_json(analysis: Dict) -> str:
    """Format analysis as JSON."""
    return json.dumps(analysis, indent=2)


def main():
    parser = argparse.ArgumentParser(
        description="Analyze SWE-bench Claude Code benchmark results",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python analyze.py --latest              # Analyze most recent results
  python analyze.py results/suite_*.json  # Analyze specific file
  python analyze.py --latest --markdown   # Output as markdown
  python analyze.py --latest --json       # Output as JSON
  python analyze.py --date 20260104       # Find results from specific date
        """
    )

    parser.add_argument(
        "file",
        nargs="?",
        help="Results file to analyze (supports glob patterns)"
    )
    parser.add_argument(
        "--latest",
        action="store_true",
        help="Analyze the most recent results file"
    )
    parser.add_argument(
        "--date",
        help="Filter results by date (YYYYMMDD format)"
    )
    parser.add_argument(
        "--markdown", "-m",
        action="store_true",
        help="Output as markdown"
    )
    parser.add_argument(
        "--json", "-j",
        action="store_true",
        help="Output as JSON"
    )
    parser.add_argument(
        "--save",
        action="store_true",
        help="Save markdown report to analysis/ directory"
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Include detailed per-task breakdown (suite only)"
    )

    args = parser.parse_args()

    # Determine which file to analyze
    result_file = None

    if args.latest:
        result_file = find_latest_results()
        if not result_file:
            print("Error: No results files found in", RESULTS_DIR, file=sys.stderr)
            sys.exit(1)
        print(f"Analyzing: {result_file.name}", file=sys.stderr)

    elif args.date:
        files = find_results_by_date(args.date)
        if not files:
            print(f"Error: No results found for date {args.date}", file=sys.stderr)
            sys.exit(1)
        # Use the most recent matching file
        result_file = max(files, key=lambda p: p.stat().st_mtime)
        print(f"Analyzing: {result_file.name}", file=sys.stderr)

    elif args.file:
        # Support glob patterns
        matches = glob.glob(args.file)
        if not matches:
            # Try in results directory
            matches = glob.glob(str(RESULTS_DIR / args.file))
        if not matches:
            print(f"Error: File not found: {args.file}", file=sys.stderr)
            sys.exit(1)
        result_file = Path(matches[0])
        print(f"Analyzing: {result_file.name}", file=sys.stderr)

    else:
        parser.print_help()
        sys.exit(1)

    # Load and analyze
    with open(result_file) as f:
        data = json.load(f)

    result_type = detect_result_type(data)
    analysis = analyze_results(data, result_type)
    analysis["source_file"] = str(result_file.name)

    # Format output
    if args.json:
        output = format_json(analysis)
    elif args.markdown:
        output = format_markdown(analysis)
    else:
        output = format_terminal(analysis)

    print(output)

    # Save if requested
    if args.save or (args.markdown and args.save):
        ANALYSIS_DIR.mkdir(exist_ok=True)
        date_str = datetime.now().strftime("%Y-%m-%d")
        output_path = ANALYSIS_DIR / f"{date_str}.md"

        # Don't overwrite existing files
        counter = 1
        while output_path.exists():
            output_path = ANALYSIS_DIR / f"{date_str}-{counter}.md"
            counter += 1

        md_output = format_markdown(analysis) if not args.markdown else output
        with open(output_path, "w") as f:
            f.write(md_output)
        print(f"\nSaved to: {output_path}", file=sys.stderr)


if __name__ == "__main__":
    main()
