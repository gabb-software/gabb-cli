#!/usr/bin/env python3
"""
A/B Testing workflow for gabb benchmark comparisons.

Automates the process of comparing benchmark results between two git branches:
1. Stashes uncommitted changes (or warns if dirty)
2. Checks out branch A, rebuilds gabb, runs benchmarks
3. Checks out branch B, rebuilds gabb, runs benchmarks
4. Restores original branch
5. Generates comparison report

Usage:
    # Run A/B test comparing two branches
    python ab_test.py --branch-a main --branch-b feature/new-symbols \
        --task sklearn-ridge-normalize --runs 10

    # Run with SWE-bench task
    python ab_test.py --branch-a main --branch-b feature/new-symbols \
        --swe-bench scikit-learn__scikit-learn-10297 --runs 5

    # Force run with dirty working tree (auto-stashes)
    python ab_test.py --branch-a main --branch-b feature/new-symbols \
        --task sklearn-ridge-normalize --force
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

BENCHMARK_DIR = Path(__file__).parent
REPO_ROOT = BENCHMARK_DIR.parent.parent
RESULTS_DIR = BENCHMARK_DIR / "results"


def run_cmd(
    cmd: list[str],
    cwd: Path | None = None,
    check: bool = True,
    capture: bool = False,
) -> subprocess.CompletedProcess:
    """Run a command and optionally capture output."""
    kwargs = {
        "cwd": cwd or REPO_ROOT,
        "check": check,
    }
    if capture:
        kwargs["capture_output"] = True
        kwargs["text"] = True
    return subprocess.run(cmd, **kwargs)


def get_current_branch() -> str:
    """Get current git branch name."""
    result = run_cmd(["git", "rev-parse", "--abbrev-ref", "HEAD"], capture=True)
    return result.stdout.strip()


def is_working_tree_dirty() -> bool:
    """Check if git working tree has uncommitted changes."""
    result = run_cmd(["git", "status", "--porcelain"], capture=True)
    return bool(result.stdout.strip())


def checkout_branch(branch: str) -> bool:
    """Checkout a git branch.

    Returns:
        True if successful, False otherwise.
    """
    try:
        run_cmd(["git", "checkout", branch])
        return True
    except subprocess.CalledProcessError as e:
        print(f"Error: Failed to checkout branch '{branch}': {e}", file=sys.stderr)
        return False


def stash_changes() -> bool:
    """Stash uncommitted changes.

    Returns:
        True if changes were stashed, False if nothing to stash.
    """
    if not is_working_tree_dirty():
        return False
    run_cmd(["git", "stash", "push", "-m", "ab_test.py auto-stash"])
    print("  Stashed uncommitted changes")
    return True


def pop_stash() -> None:
    """Pop stashed changes if any."""
    # Check if there are stashes
    result = run_cmd(["git", "stash", "list"], capture=True)
    if result.stdout.strip():
        try:
            run_cmd(["git", "stash", "pop"])
            print("  Restored stashed changes")
        except subprocess.CalledProcessError:
            print("  Warning: Failed to pop stash, manual recovery may be needed")


def build_gabb() -> Path | None:
    """Build gabb and return path to binary.

    Returns:
        Path to built binary, or None if build failed.
    """
    try:
        run_cmd(["cargo", "build", "--release"])
        binary = REPO_ROOT / "target" / "release" / "gabb"
        if binary.exists():
            return binary
        print("Error: Binary not found after build", file=sys.stderr)
        return None
    except subprocess.CalledProcessError as e:
        print(f"Error: Build failed: {e}", file=sys.stderr)
        return None


def run_benchmark(
    branch: str,
    task: str | None = None,
    swe_bench: str | None = None,
    runs: int = 1,
    condition: str = "both",
    workspace: Path | None = None,
) -> Path | None:
    """Run benchmark on current branch.

    Returns:
        Path to result file, or None if failed.
    """
    cmd = [sys.executable, str(BENCHMARK_DIR / "run.py")]

    if task:
        cmd.extend(["--task", task])
        if workspace:
            cmd.extend(["--workspace", str(workspace)])
    elif swe_bench:
        cmd.extend(["--swe-bench", swe_bench])

    cmd.extend(["--runs", str(runs)])
    cmd.extend(["--condition", condition])

    try:
        run_cmd(cmd, cwd=BENCHMARK_DIR)
        # Find the most recent result file
        result_files = sorted(RESULTS_DIR.glob("*.json"), key=lambda p: p.stat().st_mtime)
        if result_files:
            return result_files[-1]
        return None
    except subprocess.CalledProcessError as e:
        print(f"Error: Benchmark failed on branch {branch}: {e}", file=sys.stderr)
        return None


def run_comparison(result_a: Path, result_b: Path, stats: bool = False) -> None:
    """Run comparison between two result files."""
    cmd = [sys.executable, str(BENCHMARK_DIR / "compare.py"), str(result_a), str(result_b)]
    if stats:
        cmd.append("--stats")
    run_cmd(cmd, cwd=BENCHMARK_DIR)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run A/B benchmark test between two git branches",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )

    parser.add_argument(
        "--branch-a",
        required=True,
        help="Baseline branch (e.g., main)",
    )
    parser.add_argument(
        "--branch-b",
        required=True,
        help="Comparison branch (e.g., feature/new-symbols)",
    )

    task_group = parser.add_mutually_exclusive_group(required=True)
    task_group.add_argument(
        "--task",
        help="Manual task ID from tasks.json",
    )
    task_group.add_argument(
        "--swe-bench",
        help="SWE-bench task ID (e.g., scikit-learn__scikit-learn-10297)",
    )

    parser.add_argument(
        "--workspace",
        type=Path,
        help="Workspace path (required for manual tasks)",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=1,
        help="Number of runs per condition (default: 1)",
    )
    parser.add_argument(
        "--condition",
        choices=["control", "gabb", "both"],
        default="both",
        help="Which condition to run (default: both)",
    )
    parser.add_argument(
        "--stats",
        action="store_true",
        help="Include statistical tests in comparison",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Force run even with dirty working tree (auto-stashes)",
    )

    args = parser.parse_args()

    # Validate manual task requires workspace
    if args.task and not args.workspace:
        print("Error: --workspace required for manual tasks", file=sys.stderr)
        return 1

    # Check for same branch
    if args.branch_a == args.branch_b:
        print("Error: Cannot compare a branch to itself", file=sys.stderr)
        return 1

    # Save current state
    original_branch = get_current_branch()
    print(f"Current branch: {original_branch}")

    # Check for dirty working tree
    stashed = False
    if is_working_tree_dirty():
        if args.force:
            stashed = stash_changes()
        else:
            print("Error: Working tree has uncommitted changes", file=sys.stderr)
            print("  Use --force to auto-stash, or commit/stash manually", file=sys.stderr)
            return 1

    result_a: Path | None = None
    result_b: Path | None = None

    try:
        # Run on branch A
        print(f"\n[1/2] Running on branch '{args.branch_a}'...")
        if not checkout_branch(args.branch_a):
            return 1

        print("  Building gabb...")
        binary_a = build_gabb()
        if not binary_a:
            return 1
        print("  Build complete")

        print(f"  Running {args.runs} benchmark iteration(s)...")
        result_a = run_benchmark(
            args.branch_a,
            task=args.task,
            swe_bench=args.swe_bench,
            runs=args.runs,
            condition=args.condition,
            workspace=args.workspace,
        )
        if result_a:
            print(f"  Results: {result_a}")
        else:
            print("  Warning: No result file generated")

        # Run on branch B
        print(f"\n[2/2] Running on branch '{args.branch_b}'...")
        if not checkout_branch(args.branch_b):
            return 1

        print("  Building gabb...")
        binary_b = build_gabb()
        if not binary_b:
            return 1
        print("  Build complete")

        print(f"  Running {args.runs} benchmark iteration(s)...")
        result_b = run_benchmark(
            args.branch_b,
            task=args.task,
            swe_bench=args.swe_bench,
            runs=args.runs,
            condition=args.condition,
            workspace=args.workspace,
        )
        if result_b:
            print(f"  Results: {result_b}")
        else:
            print("  Warning: No result file generated")

    finally:
        # Restore original branch
        print(f"\nRestoring original branch '{original_branch}'...")
        checkout_branch(original_branch)

        # Pop stash if we stashed
        if stashed:
            pop_stash()

    # Generate comparison report
    if result_a and result_b:
        print("\nGenerating comparison report...\n")
        run_comparison(result_a, result_b, args.stats)
    else:
        print("\nWarning: Cannot generate comparison - missing result files")
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
