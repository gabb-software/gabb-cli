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
import time
from pathlib import Path

BENCHMARK_DIR = Path(__file__).parent


class ProgressTracker:
    """Track progress across multiple phases with time estimation."""

    def __init__(self, phases: list[tuple[str, float]]):
        """Initialize with phases as (name, weight) tuples.

        Weights are relative - they determine what fraction of progress
        each phase represents. A phase with weight 2 takes twice as long
        as a phase with weight 1.
        """
        self.phases = phases
        self.phase_names = [p[0] for p in phases]
        self.phase_weights = [p[1] for p in phases]
        self.total_weight = sum(self.phase_weights)
        self.current_phase = 0
        self.start_time = time.time()
        self.phase_start_time = self.start_time

    def start_phase(self, phase_name: str) -> None:
        """Mark the start of a phase."""
        try:
            self.current_phase = self.phase_names.index(phase_name)
        except ValueError:
            pass  # Unknown phase, ignore
        self.phase_start_time = time.time()
        self._print_status()

    def complete_phase(self, phase_name: str) -> None:
        """Mark a phase as complete."""
        try:
            idx = self.phase_names.index(phase_name)
            if idx == self.current_phase:
                self.current_phase = idx + 1
        except ValueError:
            pass

    def _get_progress_percent(self) -> float:
        """Calculate overall progress percentage."""
        completed_weight = sum(self.phase_weights[:self.current_phase])
        return (completed_weight / self.total_weight) * 100

    def _format_duration(self, seconds: float) -> str:
        """Format seconds as human-readable duration."""
        if seconds < 60:
            return f"{seconds:.0f}s"
        minutes = int(seconds // 60)
        secs = int(seconds % 60)
        if minutes < 60:
            return f"{minutes}m {secs}s"
        hours = minutes // 60
        mins = minutes % 60
        return f"{hours}h {mins}m"

    def _estimate_remaining(self) -> float | None:
        """Estimate remaining time based on elapsed time and progress."""
        if self.current_phase == 0:
            return None  # Not enough data yet

        elapsed = time.time() - self.start_time
        completed_weight = sum(self.phase_weights[:self.current_phase])
        remaining_weight = self.total_weight - completed_weight

        if completed_weight == 0:
            return None

        # Time per unit weight
        rate = elapsed / completed_weight
        return rate * remaining_weight

    def _print_status(self) -> None:
        """Print current progress status."""
        elapsed = time.time() - self.start_time
        percent = self._get_progress_percent()
        remaining = self._estimate_remaining()

        # Build progress bar
        bar_width = 20
        filled = int(bar_width * percent / 100)
        bar = "█" * filled + "░" * (bar_width - filled)

        # Build time string
        elapsed_str = self._format_duration(elapsed)
        if remaining is not None:
            remaining_str = self._format_duration(remaining)
            time_str = f"elapsed: {elapsed_str}, remaining: ~{remaining_str}"
        else:
            time_str = f"elapsed: {elapsed_str}"

        phase_name = self.phase_names[self.current_phase] if self.current_phase < len(self.phase_names) else "Done"
        print(f"\n[{bar}] {percent:5.1f}% | {phase_name} | {time_str}")

    def finish(self) -> None:
        """Mark all phases complete and print final status."""
        self.current_phase = len(self.phases)
        elapsed = time.time() - self.start_time
        bar = "█" * 20
        print(f"\n[{bar}] 100.0% | Complete | total time: {self._format_duration(elapsed)}")


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

    # Initialize progress tracker with phases and weights
    # Weights reflect relative duration: builds are quick, benchmarks take longer
    benchmark_weight = 4 * args.runs  # Scale with number of runs
    progress = ProgressTracker([
        (f"Build {args.branch_a}", 1),
        (f"Benchmark {args.branch_a}", benchmark_weight),
        (f"Build {args.branch_b}", 1),
        (f"Benchmark {args.branch_b}", benchmark_weight),
        ("Generate comparison", 0.5),
    ])

    try:
        # Run on branch A
        print(f"\n[1/2] Running on branch '{args.branch_a}'...")
        if not checkout_branch(args.branch_a):
            return 1

        progress.start_phase(f"Build {args.branch_a}")
        print("  Building gabb...")
        binary_a = build_gabb()
        if not binary_a:
            return 1
        print("  Build complete")
        progress.complete_phase(f"Build {args.branch_a}")

        progress.start_phase(f"Benchmark {args.branch_a}")
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
        progress.complete_phase(f"Benchmark {args.branch_a}")

        # Run on branch B
        print(f"\n[2/2] Running on branch '{args.branch_b}'...")
        if not checkout_branch(args.branch_b):
            return 1

        progress.start_phase(f"Build {args.branch_b}")
        print("  Building gabb...")
        binary_b = build_gabb()
        if not binary_b:
            return 1
        print("  Build complete")
        progress.complete_phase(f"Build {args.branch_b}")

        progress.start_phase(f"Benchmark {args.branch_b}")
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
        progress.complete_phase(f"Benchmark {args.branch_b}")

    finally:
        # Restore original branch
        print(f"\nRestoring original branch '{original_branch}'...")
        checkout_branch(original_branch)

        # Pop stash if we stashed
        if stashed:
            pop_stash()

    # Generate comparison report
    if result_a and result_b:
        progress.start_phase("Generate comparison")
        print("\nGenerating comparison report...\n")
        run_comparison(result_a, result_b, args.stats)
        progress.finish()
    else:
        print("\nWarning: Cannot generate comparison - missing result files")
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
