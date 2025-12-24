#!/usr/bin/env python3
"""
Gabb Benchmark Runner.

Phase 1: Run a single SWE-bench task and compare Control vs Gabb agents.
Phase 2: Run multiple tasks concurrently with full reporting.

Usage:
    # Phase 1: Single task test
    python run.py --task scikit-learn__scikit-learn-10297

    # Phase 2: Full benchmark suite
    python run.py --tasks 20 --concurrent 5
"""

from __future__ import annotations

import argparse
import asyncio
import csv
import json
import logging
import os
import sys
from dataclasses import asdict, dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

from dotenv import load_dotenv
from rich.console import Console
from rich.progress import Progress, SpinnerColumn, TextColumn, BarColumn, TimeElapsedColumn
from rich.table import Table

from core.agent import AgentConfig, AgentMetrics, ControlAgent, GabbAgent, create_agent
from core.dataset import BenchmarkTask, SWEBenchDataset, load_swebench
from core.env import BenchmarkEnv, EnvConfig

# Paths
BENCHMARK_DIR = Path(__file__).parent

# Load environment variables from .env file
load_dotenv(BENCHMARK_DIR / ".env")


def setup_docker_host() -> None:
    """Auto-detect Docker socket on macOS if DOCKER_HOST not set."""
    import platform
    if os.environ.get("DOCKER_HOST"):
        return  # Already set

    if platform.system() == "Darwin":
        # macOS Docker Desktop socket locations
        socket_paths = [
            Path.home() / ".docker/run/docker.sock",
            Path("/var/run/docker.sock"),
        ]
        for sock in socket_paths:
            if sock.exists():
                os.environ["DOCKER_HOST"] = f"unix://{sock}"
                return


# Auto-detect Docker socket
setup_docker_host()

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    datefmt="%H:%M:%S",
)
logger = logging.getLogger(__name__)

# Rich console for pretty output
console = Console()
LINUX_BINARY = BENCHMARK_DIR / "bin" / "gabb"
RESULTS_DIR = BENCHMARK_DIR / "results"


@dataclass
class TaskResult:
    """Result of running both agents on a task."""

    instance_id: str
    repo: str
    gold_files: list[str]

    # Control agent results
    control_success: bool
    control_final_answer: str | None
    control_tokens_input: int
    control_tokens_output: int
    control_turns: int
    control_tool_calls: int
    control_time_seconds: float
    control_tool_usage: dict[str, int]

    # Gabb agent results
    gabb_success: bool
    gabb_final_answer: str | None
    gabb_tokens_input: int
    gabb_tokens_output: int
    gabb_turns: int
    gabb_tool_calls: int
    gabb_time_seconds: float
    gabb_tool_usage: dict[str, int]

    # Comparison metrics
    @property
    def token_savings(self) -> float:
        """Token savings of gabb vs control (percentage)."""
        control_total = self.control_tokens_input + self.control_tokens_output
        gabb_total = self.gabb_tokens_input + self.gabb_tokens_output
        if control_total == 0:
            return 0.0
        return (control_total - gabb_total) / control_total * 100

    @property
    def speedup(self) -> float:
        """Speedup of gabb vs control (multiplier)."""
        if self.gabb_time_seconds == 0:
            return 0.0
        return self.control_time_seconds / self.gabb_time_seconds

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON export (includes nested dicts)."""
        return {
            "instance_id": self.instance_id,
            "repo": self.repo,
            "gold_files": "|".join(self.gold_files),
            "control_success": self.control_success,
            "control_final_answer": self.control_final_answer,
            "control_tokens_input": self.control_tokens_input,
            "control_tokens_output": self.control_tokens_output,
            "control_turns": self.control_turns,
            "control_tool_calls": self.control_tool_calls,
            "control_time_seconds": round(self.control_time_seconds, 2),
            "control_tool_usage": self.control_tool_usage,
            "gabb_success": self.gabb_success,
            "gabb_final_answer": self.gabb_final_answer,
            "gabb_tokens_input": self.gabb_tokens_input,
            "gabb_tokens_output": self.gabb_tokens_output,
            "gabb_turns": self.gabb_turns,
            "gabb_tool_calls": self.gabb_tool_calls,
            "gabb_time_seconds": round(self.gabb_time_seconds, 2),
            "gabb_tool_usage": self.gabb_tool_usage,
            "token_savings_pct": round(self.token_savings, 1),
            "speedup_x": round(self.speedup, 2),
        }

    def to_csv_dict(self) -> dict[str, Any]:
        """Convert to dictionary for CSV export (flattens tool_usage)."""
        return {
            "instance_id": self.instance_id,
            "repo": self.repo,
            "gold_files": "|".join(self.gold_files),
            "control_success": self.control_success,
            "control_final_answer": self.control_final_answer,
            "control_tokens_input": self.control_tokens_input,
            "control_tokens_output": self.control_tokens_output,
            "control_turns": self.control_turns,
            "control_tool_calls": self.control_tool_calls,
            "control_time_seconds": round(self.control_time_seconds, 2),
            "control_tool_usage": json.dumps(self.control_tool_usage),
            "gabb_success": self.gabb_success,
            "gabb_final_answer": self.gabb_final_answer,
            "gabb_tokens_input": self.gabb_tokens_input,
            "gabb_tokens_output": self.gabb_tokens_output,
            "gabb_turns": self.gabb_turns,
            "gabb_tool_calls": self.gabb_tool_calls,
            "gabb_time_seconds": round(self.gabb_time_seconds, 2),
            "gabb_tool_usage": json.dumps(self.gabb_tool_usage),
            "token_savings_pct": round(self.token_savings, 1),
            "speedup_x": round(self.speedup, 2),
        }


def check_success(final_answer: str | None, gold_files: list[str]) -> bool:
    """
    Check if the agent found the correct file(s).

    Args:
        final_answer: The agent's final answer (may contain multiple files).
        gold_files: The list of expected files.

    Returns:
        True if any gold file is mentioned in the answer.
    """
    if not final_answer:
        return False

    # Normalize paths and check for matches
    answer_lower = final_answer.lower()

    for gold_file in gold_files:
        # Check for exact match or partial path match
        gold_lower = gold_file.lower()
        if gold_lower in answer_lower:
            return True

        # Also check just the filename
        filename = Path(gold_file).name.lower()
        if filename in answer_lower:
            return True

    return False


async def run_single_task(
    task: BenchmarkTask,
    agent_type: str,
    env_config: EnvConfig,
    agent_config: AgentConfig,
) -> AgentMetrics:
    """
    Run a single agent on a task.

    Args:
        task: The benchmark task.
        agent_type: Type of agent ('control' or 'gabb').
        env_config: Environment configuration.
        agent_config: Agent configuration.

    Returns:
        AgentMetrics from the run.
    """
    async with BenchmarkEnv(env_config) as env:
        await env.setup(task)

        agent = create_agent(agent_type, env, agent_config)

        # Build the problem statement
        problem = f"""## Issue

{task.problem_statement}

## Hints

{task.hints_text if task.hints_text else 'No hints available.'}

## Task

Find the file(s) that need to be modified to fix this issue.
When you are confident, output: FINAL_ANSWER: <filepath>"""

        metrics = await agent.run(problem)
        return metrics


async def run_task_comparison(
    task: BenchmarkTask,
    gabb_binary: Path | None,
    agent_config: AgentConfig,
) -> TaskResult:
    """
    Run both control and gabb agents on a task.

    Args:
        task: The benchmark task.
        gabb_binary: Path to the gabb binary (None for control-only).
        agent_config: Agent configuration.

    Returns:
        TaskResult with comparison data.
    """
    logger.info(f"Running task: {task.instance_id}")

    # Configure environments
    control_env_config = EnvConfig(gabb_binary_path=None)
    gabb_env_config = EnvConfig(gabb_binary_path=gabb_binary)

    # Run control agent
    logger.info(f"Running control agent...")
    control_metrics = await run_single_task(
        task, "control", control_env_config, agent_config
    )

    # Run gabb agent (if binary available)
    if gabb_binary and gabb_binary.exists():
        logger.info(f"Running gabb agent...")
        gabb_metrics = await run_single_task(
            task, "gabb", gabb_env_config, agent_config
        )
    else:
        logger.warning("Gabb binary not found, skipping gabb agent")
        gabb_metrics = AgentMetrics()

    # Build result
    return TaskResult(
        instance_id=task.instance_id,
        repo=task.repo,
        gold_files=task.gold_files,
        control_success=check_success(control_metrics.final_answer, task.gold_files),
        control_final_answer=control_metrics.final_answer,
        control_tokens_input=control_metrics.tokens_input,
        control_tokens_output=control_metrics.tokens_output,
        control_turns=control_metrics.turns,
        control_tool_calls=control_metrics.tool_calls,
        control_time_seconds=control_metrics.time_seconds,
        control_tool_usage=control_metrics.tool_usage,
        gabb_success=check_success(gabb_metrics.final_answer, task.gold_files),
        gabb_final_answer=gabb_metrics.final_answer,
        gabb_tokens_input=gabb_metrics.tokens_input,
        gabb_tokens_output=gabb_metrics.tokens_output,
        gabb_turns=gabb_metrics.turns,
        gabb_tool_calls=gabb_metrics.tool_calls,
        gabb_time_seconds=gabb_metrics.time_seconds,
        gabb_tool_usage=gabb_metrics.tool_usage,
    )


async def run_benchmark_suite(
    tasks: list[BenchmarkTask],
    gabb_binary: Path | None,
    agent_config: AgentConfig,
    concurrent: int = 1,
    progress_callback=None,
) -> list[TaskResult]:
    """
    Run the benchmark suite on multiple tasks.

    Args:
        tasks: List of tasks to run.
        gabb_binary: Path to gabb binary.
        agent_config: Agent configuration.
        concurrent: Number of concurrent tasks.
        progress_callback: Optional callback for progress updates.

    Returns:
        List of TaskResults.
    """
    results = []

    if concurrent == 1:
        # Sequential execution
        for i, task in enumerate(tasks):
            result = await run_task_comparison(task, gabb_binary, agent_config)
            results.append(result)
            if progress_callback:
                progress_callback(i + 1, len(tasks), result)
    else:
        # Concurrent execution
        semaphore = asyncio.Semaphore(concurrent)

        async def run_with_semaphore(task: BenchmarkTask) -> TaskResult:
            async with semaphore:
                return await run_task_comparison(task, gabb_binary, agent_config)

        # Create all tasks
        coros = [run_with_semaphore(task) for task in tasks]

        # Run with progress tracking
        for i, coro in enumerate(asyncio.as_completed(coros)):
            result = await coro
            results.append(result)
            if progress_callback:
                progress_callback(i + 1, len(tasks), result)

    return results


def print_result_table(results: list[TaskResult]) -> None:
    """Print a summary table of results."""
    table = Table(title="Benchmark Results")

    table.add_column("Instance ID", style="cyan", no_wrap=True)
    table.add_column("Control", justify="center")
    table.add_column("Gabb", justify="center")
    table.add_column("Token Savings", justify="right")
    table.add_column("Speedup", justify="right")

    for result in results:
        control_status = "[green]PASS[/green]" if result.control_success else "[red]FAIL[/red]"
        gabb_status = "[green]PASS[/green]" if result.gabb_success else "[red]FAIL[/red]"

        table.add_row(
            result.instance_id[:40],
            control_status,
            gabb_status,
            f"{result.token_savings:.1f}%",
            f"{result.speedup:.2f}x",
        )

    console.print(table)


def print_summary(results: list[TaskResult]) -> None:
    """Print summary statistics."""
    if not results:
        console.print("[yellow]No results to summarize[/yellow]")
        return

    # Calculate aggregate metrics
    control_passes = sum(1 for r in results if r.control_success)
    gabb_passes = sum(1 for r in results if r.gabb_success)

    total_control_tokens = sum(r.control_tokens_input + r.control_tokens_output for r in results)
    total_gabb_tokens = sum(r.gabb_tokens_input + r.gabb_tokens_output for r in results)

    avg_token_savings = sum(r.token_savings for r in results) / len(results)
    avg_speedup = sum(r.speedup for r in results if r.speedup > 0) / max(1, sum(1 for r in results if r.speedup > 0))

    console.print("\n[bold]Summary Statistics[/bold]")
    console.print("=" * 50)
    console.print(f"Tasks run: {len(results)}")
    console.print(f"Control recall: {control_passes}/{len(results)} ({control_passes/len(results)*100:.1f}%)")
    console.print(f"Gabb recall: {gabb_passes}/{len(results)} ({gabb_passes/len(results)*100:.1f}%)")
    console.print(f"Total control tokens: {total_control_tokens:,}")
    console.print(f"Total gabb tokens: {total_gabb_tokens:,}")
    console.print(f"Average token savings: {avg_token_savings:.1f}%")
    console.print(f"Average speedup: {avg_speedup:.2f}x")
    console.print("=" * 50)

    # Aggregate tool usage
    control_tools: dict[str, int] = {}
    gabb_tools: dict[str, int] = {}
    for r in results:
        for tool, count in r.control_tool_usage.items():
            control_tools[tool] = control_tools.get(tool, 0) + count
        for tool, count in r.gabb_tool_usage.items():
            gabb_tools[tool] = gabb_tools.get(tool, 0) + count

    if control_tools or gabb_tools:
        console.print("\n[bold]Tool Usage[/bold]")
        console.print("-" * 50)
        if control_tools:
            console.print("[cyan]Control agent:[/cyan]")
            for tool, count in sorted(control_tools.items(), key=lambda x: -x[1]):
                console.print(f"  {tool:25} {count:5}")
        if gabb_tools:
            console.print("[cyan]Gabb agent:[/cyan]")
            for tool, count in sorted(gabb_tools.items(), key=lambda x: -x[1]):
                console.print(f"  {tool:25} {count:5}")
        console.print("-" * 50)


def save_results(results: list[TaskResult], output_dir: Path) -> None:
    """Save results to CSV and JSON files."""
    output_dir.mkdir(parents=True, exist_ok=True)

    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")

    # Save CSV
    csv_path = output_dir / f"results_{timestamp}.csv"
    with open(csv_path, "w", newline="") as f:
        if results:
            writer = csv.DictWriter(f, fieldnames=results[0].to_csv_dict().keys())
            writer.writeheader()
            for result in results:
                writer.writerow(result.to_csv_dict())

    console.print(f"\n[green]Results saved to {csv_path}[/green]")

    # Save JSON for detailed analysis
    json_path = output_dir / f"results_{timestamp}.json"
    with open(json_path, "w") as f:
        json.dump([r.to_dict() for r in results], f, indent=2)

    console.print(f"[green]JSON saved to {json_path}[/green]")


async def main_async(args: argparse.Namespace) -> int:
    """Async main function."""
    # Check for gabb binary
    gabb_binary = LINUX_BINARY if LINUX_BINARY.exists() else None
    if not gabb_binary:
        console.print("[yellow]Warning: Gabb binary not found. Run 'python setup.py' first.[/yellow]")
        console.print("[yellow]Will run control agent only.[/yellow]")

    # Load dataset
    console.print("Loading SWE-bench dataset...")
    dataset = load_swebench(split="test")
    console.print(f"Loaded {dataset.task_count} tasks")

    # Select tasks
    if args.task:
        # Single task mode (Phase 1)
        task = dataset.get_task(args.task)
        if not task:
            console.print(f"[red]Task not found: {args.task}[/red]")
            console.print(f"Available tasks (sample): {dataset.task_ids[:5]}")
            return 1
        tasks = [task]
    else:
        # Multiple tasks mode (Phase 2)
        tasks = list(dataset.iter_tasks(limit=args.tasks))

    console.print(f"\nRunning benchmark on {len(tasks)} task(s)...")

    # Configure agent
    agent_config = AgentConfig(
        model=args.model,
        max_turns=args.max_turns,
        temperature=0.0,
    )

    # Run benchmark
    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        TextColumn("[progress.percentage]{task.percentage:>3.0f}%"),
        TimeElapsedColumn(),
        console=console,
    ) as progress:
        task_id = progress.add_task("Running benchmark...", total=len(tasks))

        def on_progress(completed: int, total: int, result: TaskResult):
            progress.update(task_id, completed=completed)
            status = "PASS" if result.gabb_success or result.control_success else "FAIL"
            logger.info(f"Completed {result.instance_id}: {status}")

        results = await run_benchmark_suite(
            tasks=tasks,
            gabb_binary=gabb_binary,
            agent_config=agent_config,
            concurrent=args.concurrent,
            progress_callback=on_progress,
        )

    # Display results
    print_result_table(results)
    print_summary(results)

    # Save results
    if not args.no_save:
        save_results(results, RESULTS_DIR)

    return 0


def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(
        description="Gabb Benchmark Runner",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    # Phase 1: Test with a single task
    python run.py --task scikit-learn__scikit-learn-10297

    # Phase 2: Run on 20 tasks with 5 concurrent containers
    python run.py --tasks 20 --concurrent 5

    # Run with different model
    python run.py --task django__django-11099 --model claude-3-haiku-20240307
        """,
    )

    parser.add_argument(
        "--task",
        type=str,
        help="Run a specific task by instance_id (Phase 1 mode)",
    )
    parser.add_argument(
        "--tasks",
        type=int,
        default=1,
        help="Number of tasks to run (Phase 2 mode)",
    )
    parser.add_argument(
        "--concurrent",
        type=int,
        default=1,
        help="Number of concurrent containers",
    )
    parser.add_argument(
        "--model",
        type=str,
        default="claude-sonnet-4-20250514",
        help="Anthropic model to use",
    )
    parser.add_argument(
        "--max-turns",
        type=int,
        default=20,
        help="Maximum turns per agent",
    )
    parser.add_argument(
        "--no-save",
        action="store_true",
        help="Don't save results to files",
    )
    parser.add_argument(
        "-v", "--verbose",
        action="store_true",
        help="Enable verbose logging",
    )

    args = parser.parse_args()

    if args.verbose:
        logging.getLogger().setLevel(logging.DEBUG)

    # Check for API key
    if not os.environ.get("ANTHROPIC_API_KEY"):
        console.print("[red]Error: ANTHROPIC_API_KEY not found[/red]")
        console.print("Create a .env file in the benchmark folder with:")
        console.print("  ANTHROPIC_API_KEY=your-api-key-here")
        return 1

    # Run async main
    return asyncio.run(main_async(args))


if __name__ == "__main__":
    sys.exit(main())
